//! Revisioned content document reads, compare-and-swap writes, and active state.

use super::{
    Connection, CoreError, CoreResult, DateTime, DeserializeOwned, Digest, DocumentTable,
    OptionalExtension, Provenance, RevisionEventKind, RevisionWrite, Serialize, Sha256,
    Sha256Digest, Storage, StoredRevision, Transaction, TransactionBehavior, Utc, Uuid,
    decode_stored_document, document_provenance, document_schema_version, encode_document,
    i64_revision, not_found, params, parse_datetime, revision_conflict, revision_diff_json,
    sha256_hex, source_kind_str, storage_corrupted, storage_db_error, u64_revision,
    validate_identifier, validate_optional_sha256,
};

pub(super) fn get_document<T>(
    storage: &Storage,
    table: DocumentTable,
    id: &str,
    include_deleted: bool,
) -> CoreResult<StoredRevision<T>>
where
    T: DeserializeOwned,
{
    let deleted_clause = if include_deleted {
        ""
    } else {
        " AND object.deleted_at IS NULL"
    };
    let sql = format!(
        "SELECT revision.document_json, state.state_version, revision.id,
                object.created_at, state.updated_at, object.deleted_at
         FROM content_objects AS object
         JOIN content_object_state AS state
           ON state.object_id = object.id
         JOIN content_revisions AS revision
           ON revision.object_id = object.id
          AND revision.id = state.active_revision_id
         WHERE object.id = ?1 AND object.object_kind = ?2{deleted_clause}"
    );
    let row = storage
        .connection()?
        .query_row(&sql, params![id, table.object_kind()], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, i64>(1)?,
                row.get::<_, Option<String>>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, String>(4)?,
                row.get::<_, Option<String>>(5)?,
            ))
        })
        .optional()
        .map_err(storage_db_error)?
        .ok_or_else(|| not_found(table.object_kind()))?;
    decode_stored_document(table.object_kind(), row)
}

pub(super) fn list_documents<T>(
    storage: &Storage,
    table: DocumentTable,
) -> CoreResult<Vec<StoredRevision<T>>>
where
    T: DeserializeOwned,
{
    let connection = storage.connection()?;
    let mut statement = connection
        .prepare(
            "SELECT revision.document_json, state.state_version, revision.id,
                    object.created_at, state.updated_at, object.deleted_at
             FROM content_objects AS object
             JOIN content_object_state AS state
               ON state.object_id = object.id
             JOIN content_revisions AS revision
               ON revision.object_id = object.id
              AND revision.id = state.active_revision_id
             WHERE object.object_kind = ?1 AND object.deleted_at IS NULL
             ORDER BY state.updated_at DESC, object.id",
        )
        .map_err(storage_db_error)?;
    let rows = statement
        .query_map([table.object_kind()], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, i64>(1)?,
                row.get::<_, Option<String>>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, String>(4)?,
                row.get::<_, Option<String>>(5)?,
            ))
        })
        .map_err(storage_db_error)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(storage_db_error)?;
    rows.into_iter()
        .map(|row| decode_stored_document(table.object_kind(), row))
        .collect()
}

pub(super) fn list_documents_page<T>(
    connection: &Connection,
    table: DocumentTable,
    after: Option<(&DateTime<Utc>, &str)>,
    limit: u32,
) -> CoreResult<Vec<StoredRevision<T>>>
where
    T: DeserializeOwned,
{
    let after_updated_at = after.map(|(updated_at, _)| updated_at.to_rfc3339());
    let after_object_id = after.map(|(_, object_id)| object_id);
    let mut statement = connection
        .prepare(
            "SELECT revision.document_json, state.state_version, revision.id,
                    object.created_at, state.updated_at, object.deleted_at
             FROM content_objects AS object
             JOIN content_object_state AS state
               ON state.object_id = object.id
             JOIN content_revisions AS revision
               ON revision.object_id = object.id
              AND revision.id = state.active_revision_id
             WHERE object.object_kind = ?1 AND object.deleted_at IS NULL
               AND (
                    ?2 IS NULL
                    OR state.updated_at < ?2
                    OR (state.updated_at = ?2 AND object.id > ?3)
               )
             ORDER BY state.updated_at DESC, object.id
             LIMIT ?4",
        )
        .map_err(storage_db_error)?;
    let rows = statement
        .query_map(
            params![
                table.object_kind(),
                after_updated_at,
                after_object_id,
                i64::from(limit)
            ],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, Option<String>>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, String>(4)?,
                    row.get::<_, Option<String>>(5)?,
                ))
            },
        )
        .map_err(storage_db_error)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(storage_db_error)?;
    rows.into_iter()
        .map(|row| decode_stored_document(table.object_kind(), row))
        .collect()
}

pub(super) fn persona_catalog_revision(connection: &Connection) -> CoreResult<Sha256Digest> {
    let mut statement = connection
        .prepare(
            "SELECT object.id, state.state_version, state.active_revision_id,
                    state.updated_at
             FROM content_objects AS object
             JOIN content_object_state AS state ON state.object_id = object.id
             WHERE object.object_kind = 'persona' AND object.deleted_at IS NULL
             ORDER BY object.id",
        )
        .map_err(storage_db_error)?;
    let mut digest = Sha256::new();
    digest.update(b"lorepia:persona-catalog:v2\0");
    let mut rows = statement.query([]).map_err(storage_db_error)?;
    while let Some(row) = rows.next().map_err(storage_db_error)? {
        let persona_id = row.get::<_, String>(0).map_err(storage_db_error)?;
        let state_version = row.get::<_, i64>(1).map_err(storage_db_error)?;
        let active_revision_id = row.get::<_, String>(2).map_err(storage_db_error)?;
        let updated_at = row.get::<_, String>(3).map_err(storage_db_error)?;
        update_length_prefixed_digest(&mut digest, persona_id.as_bytes())?;
        digest.update(u64_revision(state_version)?.to_be_bytes());
        update_length_prefixed_digest(&mut digest, active_revision_id.as_bytes())?;
        update_length_prefixed_digest(&mut digest, updated_at.as_bytes())?;
    }
    Sha256Digest::parse(hex::encode(digest.finalize()))
        .map_err(|_| CoreError::internal("persona catalog revision could not be encoded"))
}

fn update_length_prefixed_digest(digest: &mut Sha256, value: &[u8]) -> CoreResult<()> {
    let length = u64::try_from(value.len())
        .map_err(|_| CoreError::internal("persona catalog identity exceeds platform limits"))?;
    digest.update(length.to_be_bytes());
    digest.update(value);
    Ok(())
}

struct CurrentContentRevision {
    state_version: i64,
    previous_json: String,
    created_at: String,
    deleted_at: Option<String>,
}

struct PreparedContentRevision {
    revision_no: u64,
    parent_revision_id: Option<String>,
    state_version: u64,
    created_at: DateTime<Utc>,
    previous_json: Option<String>,
}

fn load_current_content_revision(
    transaction: &Transaction<'_>,
    table: DocumentTable,
    id: &str,
) -> CoreResult<Option<CurrentContentRevision>> {
    transaction
        .query_row(
            "SELECT state.state_version, revision.document_json,
                    object.created_at, object.deleted_at
             FROM content_objects AS object
             JOIN content_object_state AS state
               ON state.object_id = object.id
             JOIN content_revisions AS revision
               ON revision.object_id = object.id
              AND revision.id = state.active_revision_id
             WHERE object.id = ?1 AND object.object_kind = ?2",
            params![id, table.object_kind()],
            |row| {
                Ok(CurrentContentRevision {
                    state_version: row.get(0)?,
                    previous_json: row.get(1)?,
                    created_at: row.get(2)?,
                    deleted_at: row.get(3)?,
                })
            },
        )
        .optional()
        .map_err(storage_db_error)
}

fn prepare_content_revision(
    transaction: &Transaction<'_>,
    table: DocumentTable,
    id: &str,
    expected_revision: Option<u64>,
    current: Option<CurrentContentRevision>,
    now: DateTime<Utc>,
    now_text: &str,
) -> CoreResult<PreparedContentRevision> {
    match (expected_revision, current) {
        (None, None) => {
            transaction
                .execute(
                    "INSERT INTO content_objects
                     (id, object_kind, created_at, deleted_at)
                     VALUES (?1, ?2, ?3, NULL)",
                    params![id, table.object_kind(), now_text],
                )
                .map_err(storage_db_error)?;
            Ok(PreparedContentRevision {
                revision_no: 1,
                parent_revision_id: None,
                state_version: 1,
                created_at: now,
                previous_json: None,
            })
        }
        (None, Some(current)) => Err(revision_conflict(
            table.object_kind(),
            id,
            None,
            Some(u64_revision(current.state_version)?),
        )),
        (Some(expected), None) => Err(revision_conflict(
            table.object_kind(),
            id,
            Some(expected),
            None,
        )),
        (Some(expected), Some(current)) => {
            prepare_content_revision_update(transaction, table, id, expected, current)
        }
    }
}

fn prepare_content_revision_update(
    transaction: &Transaction<'_>,
    table: DocumentTable,
    id: &str,
    expected: u64,
    current: CurrentContentRevision,
) -> CoreResult<PreparedContentRevision> {
    let actual = u64_revision(current.state_version)?;
    if current.deleted_at.is_some() || actual != expected {
        return Err(revision_conflict(
            table.object_kind(),
            id,
            Some(expected),
            Some(actual),
        ));
    }
    let (latest_revision_id, latest_revision_no) = transaction
        .query_row(
            "SELECT id, revision_no
             FROM content_revisions
             WHERE object_id = ?1
             ORDER BY revision_no DESC
             LIMIT 1",
            [id],
            |row| Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?)),
        )
        .map_err(storage_db_error)?;
    Ok(PreparedContentRevision {
        revision_no: u64_revision(latest_revision_no)?
            .checked_add(1)
            .ok_or_else(|| CoreError::internal("content revision overflow"))?,
        parent_revision_id: Some(latest_revision_id),
        state_version: expected
            .checked_add(1)
            .ok_or_else(|| CoreError::internal("content state revision overflow"))?,
        created_at: parse_datetime("content object created_at", &current.created_at)?,
        previous_json: Some(current.previous_json),
    })
}

struct ContentRevisionRecord<'a> {
    table: DocumentTable,
    id: &'a str,
    revision_id: &'a str,
    schema_version: u32,
    document_json: &'a str,
    document_sha256: &'a str,
    source_kind: &'a str,
    source_hash: Option<&'a str>,
    provenance_json: &'a str,
    created_at: &'a str,
}

fn insert_content_revision_record(
    transaction: &Transaction<'_>,
    record: &ContentRevisionRecord<'_>,
    prepared: &PreparedContentRevision,
) -> CoreResult<()> {
    transaction
        .execute(
            "INSERT INTO content_revisions
             (id, object_id, object_kind, revision_no, parent_revision_id,
              schema_version, document_json, document_sha256, source_kind,
              source_hash, provenance_json, local_override_of_revision_id,
              created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, NULL, ?12)",
            params![
                record.revision_id,
                record.id,
                record.table.object_kind(),
                i64_revision(prepared.revision_no)?,
                prepared.parent_revision_id,
                record.schema_version,
                record.document_json,
                record.document_sha256,
                record.source_kind,
                record.source_hash,
                record.provenance_json,
                record.created_at,
            ],
        )
        .map_err(storage_db_error)?;
    Ok(())
}

fn update_content_object_state(
    transaction: &Transaction<'_>,
    table: DocumentTable,
    id: &str,
    revision_id: &str,
    expected_revision: Option<u64>,
    state_version: u64,
    now_text: &str,
) -> CoreResult<()> {
    let Some(expected_revision) = expected_revision else {
        transaction
            .execute(
                "INSERT INTO content_object_state
                 (object_id, active_revision_id, state_version, updated_at)
                 VALUES (?1, ?2, 1, ?3)",
                params![id, revision_id, now_text],
            )
            .map_err(storage_db_error)?;
        return Ok(());
    };
    let changed = transaction
        .execute(
            "UPDATE content_object_state
             SET active_revision_id = ?2, state_version = ?3, updated_at = ?4
             WHERE object_id = ?1 AND state_version = ?5",
            params![
                id,
                revision_id,
                i64_revision(state_version)?,
                now_text,
                i64_revision(expected_revision)?,
            ],
        )
        .map_err(storage_db_error)?;
    if changed != 1 {
        return Err(revision_conflict(
            table.object_kind(),
            id,
            Some(expected_revision),
            None,
        ));
    }
    Ok(())
}

struct ContentRevisionEvent<'a> {
    id: &'a str,
    event_kind: RevisionEventKind,
    parent_revision_id: Option<&'a str>,
    revision_id: &'a str,
    diff_json: &'a str,
    diff_sha256: &'a str,
    created_at: &'a str,
}

fn insert_content_revision_event(
    transaction: &Transaction<'_>,
    event: &ContentRevisionEvent<'_>,
) -> CoreResult<()> {
    transaction
        .execute(
            "INSERT INTO content_revision_events
             (id, object_id, event_kind, from_revision_id, to_revision_id,
              diff_json, diff_sha256, plan_sha256, idempotency_key, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, NULL, ?8, ?9)",
            params![
                Uuid::new_v4().to_string(),
                event.id,
                event.event_kind.as_str(),
                event.parent_revision_id,
                event.revision_id,
                event.diff_json,
                event.diff_sha256,
                Uuid::new_v4().to_string(),
                event.created_at,
            ],
        )
        .map_err(storage_db_error)?;
    Ok(())
}

#[allow(clippy::too_many_arguments)]
pub(super) fn append_content_revision<T>(
    transaction: &Transaction<'_>,
    table: DocumentTable,
    id: &str,
    schema_version: u32,
    value: &T,
    provenance: &Provenance,
    expected_revision: Option<u64>,
    event_kind: RevisionEventKind,
) -> CoreResult<RevisionWrite>
where
    T: Serialize + DeserializeOwned,
{
    validate_identifier(table.object_kind(), id)?;
    if schema_version == 0 {
        return Err(CoreError::invalid(format!(
            "{} schema version must be positive",
            table.object_kind()
        )));
    }
    let (document_json, document_sha256) = encode_document(table.object_kind(), value)?;
    let (provenance_json, _) = encode_document("content provenance", provenance)?;
    let source_kind = source_kind_str(&provenance.source_kind);
    validate_optional_sha256("content source hash", provenance.source_hash.as_deref())?;
    let now = Utc::now();
    let now_text = now.to_rfc3339();
    let current = load_current_content_revision(transaction, table, id)?;
    let revision_id = Uuid::new_v4().to_string();
    let prepared = prepare_content_revision(
        transaction,
        table,
        id,
        expected_revision,
        current,
        now,
        &now_text,
    )?;
    insert_content_revision_record(
        transaction,
        &ContentRevisionRecord {
            table,
            id,
            revision_id: &revision_id,
            schema_version,
            document_json: &document_json,
            document_sha256: &document_sha256,
            source_kind,
            source_hash: provenance.source_hash.as_deref(),
            provenance_json: &provenance_json,
            created_at: &now_text,
        },
        &prepared,
    )?;
    update_content_object_state(
        transaction,
        table,
        id,
        &revision_id,
        expected_revision,
        prepared.state_version,
        &now_text,
    )?;
    let diff_json = revision_diff_json(prepared.previous_json.as_deref(), &document_json)?;
    let diff_sha256 = sha256_hex(diff_json.as_bytes());
    insert_content_revision_event(
        transaction,
        &ContentRevisionEvent {
            id,
            event_kind,
            parent_revision_id: prepared.parent_revision_id.as_deref(),
            revision_id: &revision_id,
            diff_json: &diff_json,
            diff_sha256: &diff_sha256,
            created_at: &now_text,
        },
    )?;
    Ok(RevisionWrite {
        state_version: prepared.state_version,
        revision_id,
        created_at: prepared.created_at,
        updated_at: now,
    })
}

#[allow(clippy::too_many_arguments)]
pub(super) fn save_content_object<T>(
    storage: &Storage,
    table: DocumentTable,
    id: &str,
    schema_version: u32,
    value: &T,
    provenance: &Provenance,
    expected_revision: Option<u64>,
    event_kind: RevisionEventKind,
    write_projection: impl FnOnce(&Transaction<'_>, &str, &str) -> CoreResult<()>,
    delete_after_write: bool,
) -> CoreResult<StoredRevision<T>>
where
    T: Clone + Serialize + DeserializeOwned,
{
    let mut connection = storage.connection()?;
    let transaction = connection
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(storage_db_error)?;
    let written = append_content_revision(
        &transaction,
        table,
        id,
        schema_version,
        value,
        provenance,
        expected_revision,
        event_kind,
    )?;
    let (document_json, _) = encode_document(table.object_kind(), value)?;
    write_projection(&transaction, &written.revision_id, &document_json)?;
    let deleted_at = if delete_after_write {
        let deleted_at = Utc::now();
        let changed = transaction
            .execute(
                "UPDATE content_objects
                 SET deleted_at = ?2
                 WHERE id = ?1 AND object_kind = ?3 AND deleted_at IS NULL",
                params![id, deleted_at.to_rfc3339(), table.object_kind()],
            )
            .map_err(storage_db_error)?;
        if changed != 1 {
            return Err(revision_conflict(
                table.object_kind(),
                id,
                expected_revision,
                None,
            ));
        }
        if let Some(current_table) = table.current_table() {
            let sql = format!(
                "UPDATE {current_table}
                 SET deleted_at = ?2, updated_at = ?2, revision = ?3
                 WHERE id = ?1 AND deleted_at IS NULL"
            );
            let changed = transaction
                .execute(
                    &sql,
                    params![
                        id,
                        deleted_at.to_rfc3339(),
                        i64_revision(written.state_version)?
                    ],
                )
                .map_err(storage_db_error)?;
            if changed != 1 {
                return Err(storage_corrupted(format!(
                    "{} current projection is missing during soft delete",
                    table.object_kind()
                )));
            }
        }
        Some(deleted_at)
    } else {
        None
    };
    transaction.commit().map_err(storage_db_error)?;
    Ok(StoredRevision {
        value: value.clone(),
        revision: written.state_version,
        revision_id: Some(written.revision_id),
        created_at: written.created_at,
        updated_at: written.updated_at,
        deleted_at,
    })
}

pub(super) fn soft_delete_content_object<T>(
    storage: &Storage,
    table: DocumentTable,
    id: &str,
    expected_revision: u64,
    write_projection: impl FnOnce(&Transaction<'_>, &str, &str) -> CoreResult<()>,
) -> CoreResult<StoredRevision<T>>
where
    T: Clone + Serialize + DeserializeOwned,
{
    let current = get_document::<T>(storage, table, id, false)?;
    if current.revision != expected_revision {
        return Err(revision_conflict(
            table.object_kind(),
            id,
            Some(expected_revision),
            Some(current.revision),
        ));
    }
    let provenance = document_provenance(table, &current.value)?;
    let schema_version = document_schema_version(table, &current.value)?;
    save_content_object(
        storage,
        table,
        id,
        schema_version,
        &current.value,
        &provenance,
        Some(expected_revision),
        RevisionEventKind::SoftDelete,
        write_projection,
        true,
    )
}

pub(super) fn content_revision_no(
    transaction: &Transaction<'_>,
    revision_id: &str,
) -> CoreResult<u64> {
    transaction
        .query_row(
            "SELECT revision_no FROM content_revisions WHERE id = ?1",
            [revision_id],
            |row| row.get::<_, i64>(0),
        )
        .map_err(storage_db_error)
        .and_then(u64_revision)
}

pub(super) fn active_content_revision_id(
    transaction: &Transaction<'_>,
    id: &str,
    object_kind: &str,
) -> CoreResult<String> {
    transaction
        .query_row(
            "SELECT state.active_revision_id
             FROM content_objects AS object
             JOIN content_object_state AS state ON state.object_id = object.id
             WHERE object.id = ?1 AND object.object_kind = ?2
               AND object.deleted_at IS NULL",
            params![id, object_kind],
            |row| row.get::<_, String>(0),
        )
        .optional()
        .map_err(storage_db_error)?
        .ok_or_else(|| not_found(object_kind))
}

pub(super) fn content_revision_number(
    transaction: &Transaction<'_>,
    revision_id: &str,
) -> CoreResult<u64> {
    transaction
        .query_row(
            "SELECT revision_no FROM content_revisions WHERE id = ?1",
            [revision_id],
            |row| row.get::<_, i64>(0),
        )
        .map_err(storage_db_error)
        .and_then(u64_revision)
}
