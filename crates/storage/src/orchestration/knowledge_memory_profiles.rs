//! Knowledge-book and memory-profile document stores and projections.

use super::{
    BTreeSet, CoreError, CoreResult, DocumentTable, KnowledgeBook, KnowledgeBookId,
    KnowledgeEntryId, MemoryProfile, MemoryProfileId, OptionalExtension, Provenance,
    RevisionEventKind, Storage, StoredRevision, TaskProfileId, Transaction, Utc,
    ValidateOrchestration, Value, VersionedJson, active_content_revision_id,
    append_content_revision, content_revision_number, encode_document, enum_wire, get_document,
    i64_revision, list_documents, params, save_content_object, sha256_hex,
    soft_delete_content_object, source_kind_str, storage_corrupted, storage_db_error, usize_to_i64,
};

impl Storage {
    pub fn get_knowledge_book(
        &self,
        id: &KnowledgeBookId,
    ) -> CoreResult<StoredRevision<KnowledgeBook>> {
        get_document(self, DocumentTable::KnowledgeBooks, id.as_str(), false)
    }

    pub fn list_knowledge_books(&self) -> CoreResult<Vec<StoredRevision<KnowledgeBook>>> {
        list_documents(self, DocumentTable::KnowledgeBooks)
    }

    pub fn save_knowledge_book(
        &self,
        book: &KnowledgeBook,
        expected_revision: Option<u64>,
    ) -> CoreResult<StoredRevision<KnowledgeBook>> {
        book.validate()
            .map_err(|error| CoreError::invalid(format!("knowledge book is invalid: {error}")))?;
        save_content_object(
            self,
            DocumentTable::KnowledgeBooks,
            book.id.as_str(),
            book.schema_version,
            book,
            &book.provenance,
            expected_revision,
            if expected_revision.is_some() {
                RevisionEventKind::Update
            } else {
                RevisionEventKind::Create
            },
            |transaction, revision_id, document_json| {
                write_knowledge_book_projection(
                    transaction,
                    revision_id,
                    book,
                    document_json,
                    expected_revision,
                )
            },
            false,
        )
    }

    pub fn soft_delete_knowledge_book(
        &self,
        id: &KnowledgeBookId,
        expected_revision: u64,
    ) -> CoreResult<StoredRevision<KnowledgeBook>> {
        soft_delete_content_object(
            self,
            DocumentTable::KnowledgeBooks,
            id.as_str(),
            expected_revision,
            |transaction, revision_id, _document_json| {
                clone_knowledge_book_tombstone_projection(transaction, revision_id)
            },
        )
    }

    pub fn get_memory_profile(
        &self,
        id: &MemoryProfileId,
    ) -> CoreResult<StoredRevision<MemoryProfile>> {
        get_document(self, DocumentTable::MemoryProfiles, id.as_str(), false)
    }

    pub fn list_memory_profiles(&self) -> CoreResult<Vec<StoredRevision<MemoryProfile>>> {
        list_documents(self, DocumentTable::MemoryProfiles)
    }

    pub fn save_memory_profile(
        &self,
        profile: &MemoryProfile,
        expected_revision: Option<u64>,
    ) -> CoreResult<StoredRevision<MemoryProfile>> {
        profile
            .validate()
            .map_err(|error| CoreError::invalid(format!("memory profile is invalid: {error}")))?;
        save_content_object(
            self,
            DocumentTable::MemoryProfiles,
            profile.id.as_str(),
            profile.schema_version,
            profile,
            &profile.provenance,
            expected_revision,
            if expected_revision.is_some() {
                RevisionEventKind::Update
            } else {
                RevisionEventKind::Create
            },
            |transaction, revision_id, document_json| {
                write_memory_profile_projection(
                    transaction,
                    revision_id,
                    profile,
                    document_json,
                    expected_revision,
                )
            },
            false,
        )
    }

    pub fn soft_delete_memory_profile(
        &self,
        id: &MemoryProfileId,
        expected_revision: u64,
    ) -> CoreResult<StoredRevision<MemoryProfile>> {
        soft_delete_content_object(
            self,
            DocumentTable::MemoryProfiles,
            id.as_str(),
            expected_revision,
            |transaction, revision_id, _document_json| {
                clone_memory_profile_tombstone_projection(transaction, revision_id)
            },
        )
    }
}

pub(super) fn write_memory_profile_projection(
    transaction: &Transaction<'_>,
    revision_id: &str,
    profile: &MemoryProfile,
    document_json: &str,
    expected_revision: Option<u64>,
) -> CoreResult<()> {
    validate_memory_profile(profile)?;
    let authority =
        resolve_memory_profile_projection_authority(transaction, profile, expected_revision)?;
    upsert_memory_profile_projection(transaction, profile, document_json, &authority)?;
    insert_memory_profile_revision_projection(
        transaction,
        revision_id,
        profile,
        document_json,
        &authority,
    )
}

fn parent_content_revision_id(
    transaction: &Transaction<'_>,
    revision_id: &str,
    object_kind: &str,
) -> CoreResult<String> {
    transaction
        .query_row(
            "SELECT parent_revision_id
             FROM content_revisions
             WHERE id = ?1 AND object_kind = ?2",
            params![revision_id, object_kind],
            |row| row.get::<_, Option<String>>(0),
        )
        .optional()
        .map_err(storage_db_error)?
        .flatten()
        .ok_or_else(|| {
            storage_corrupted(format!(
                "{object_kind} tombstone revision has no source revision"
            ))
        })
}

/// Copies the already-persisted relational projection for a tombstone.
///
/// A readable legacy document may no longer satisfy today's live-write
/// policy. Deletion must preserve its immutable bytes and CAS lineage without
/// routing the obsolete payload back through current creation validators.
fn clone_memory_profile_tombstone_projection(
    transaction: &Transaction<'_>,
    revision_id: &str,
) -> CoreResult<()> {
    let parent_revision_id =
        parent_content_revision_id(transaction, revision_id, "memory_profile")?;
    let revision_no = content_revision_number(transaction, revision_id)?;
    let changed = transaction
        .execute(
            "INSERT INTO memory_profile_revisions
             (revision_id, memory_profile_id, revision_no, name,
              summary_task_profile_revision_id,
              embedding_task_profile_revision_id, turns_per_summary,
              recent_raw_budget, episodic_budget, semantic_budget,
              retrieval_count, recency_weight_millionths,
              similarity_weight_millionths, importance_weight_millionths,
              preserve_invalidated_records, summary_schema_revision_id,
              document_json)
             SELECT ?1, memory_profile_id, ?2, name,
                    summary_task_profile_revision_id,
                    embedding_task_profile_revision_id, turns_per_summary,
                    recent_raw_budget, episodic_budget, semantic_budget,
                    retrieval_count, recency_weight_millionths,
                    similarity_weight_millionths, importance_weight_millionths,
                    preserve_invalidated_records, summary_schema_revision_id,
                    document_json
             FROM memory_profile_revisions
             WHERE revision_id = ?3",
            params![revision_id, i64_revision(revision_no)?, parent_revision_id],
        )
        .map_err(storage_db_error)?;
    if changed != 1 {
        return Err(storage_corrupted(
            "memory profile source projection is missing during soft delete",
        ));
    }
    Ok(())
}

struct MemoryProfileProjectionAuthority {
    summary_schema_revision: String,
    summary_task_revision: String,
    embedding_task_revision: Option<String>,
    state_version: u64,
    now: String,
    provenance_json: String,
}

fn resolve_memory_profile_projection_authority(
    transaction: &Transaction<'_>,
    profile: &MemoryProfile,
    expected_revision: Option<u64>,
) -> CoreResult<MemoryProfileProjectionAuthority> {
    let summary_schema_revision =
        ensure_memory_summary_schema(transaction, &profile.summary_schema, &profile.provenance)?;
    let summary_task_revision =
        active_content_revision_id(transaction, profile.summary_task.as_str(), "task_profile")?;
    let embedding_task_revision = profile
        .embedding_task
        .as_ref()
        .map(|id| active_content_revision_id(transaction, id.as_str(), "task_profile"))
        .transpose()?;
    let provenance_json = serde_json::to_string(&profile.provenance).map_err(|error| {
        CoreError::invalid(format!("cannot encode memory profile provenance: {error}"))
    })?;
    Ok(MemoryProfileProjectionAuthority {
        summary_schema_revision,
        summary_task_revision,
        embedding_task_revision,
        state_version: expected_revision.map_or(1, |value| value.saturating_add(1)),
        now: Utc::now().to_rfc3339(),
        provenance_json,
    })
}

fn upsert_memory_profile_projection(
    transaction: &Transaction<'_>,
    profile: &MemoryProfile,
    document_json: &str,
    authority: &MemoryProfileProjectionAuthority,
) -> CoreResult<()> {
    transaction
        .execute(
            "INSERT INTO memory_profiles
             (id, name, schema_version, revision, summary_task_profile_id,
              embedding_task_profile_id, turns_per_summary, recent_raw_budget,
              episodic_budget, semantic_budget, retrieval_count, recency_weight,
              similarity_weight, importance_weight, preserve_invalidated_records,
              summary_schema_id, document_json, provenance_json, created_at,
              updated_at, deleted_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12,
                     ?13, ?14, ?15, ?16, ?17, ?18, ?19, ?19, NULL)
             ON CONFLICT(id) DO UPDATE SET
                 name = excluded.name,
                 schema_version = excluded.schema_version,
                 revision = excluded.revision,
                 summary_task_profile_id = excluded.summary_task_profile_id,
                 embedding_task_profile_id = excluded.embedding_task_profile_id,
                 turns_per_summary = excluded.turns_per_summary,
                 recent_raw_budget = excluded.recent_raw_budget,
                 episodic_budget = excluded.episodic_budget,
                 semantic_budget = excluded.semantic_budget,
                 retrieval_count = excluded.retrieval_count,
                 recency_weight = excluded.recency_weight,
                 similarity_weight = excluded.similarity_weight,
                 importance_weight = excluded.importance_weight,
                 preserve_invalidated_records =
                     excluded.preserve_invalidated_records,
                 summary_schema_id = excluded.summary_schema_id,
                 document_json = excluded.document_json,
                 provenance_json = excluded.provenance_json,
                 updated_at = excluded.updated_at",
            params![
                profile.id.as_str(),
                profile.name,
                profile.schema_version,
                i64_revision(authority.state_version)?,
                profile.summary_task.as_str(),
                profile.embedding_task.as_ref().map(TaskProfileId::as_str),
                profile.turns_per_summary,
                profile.recent_raw_budget.max_tokens,
                profile.episodic_budget.max_tokens,
                profile.semantic_budget.max_tokens,
                profile.retrieval_count,
                profile.recency_weight,
                profile.similarity_weight,
                profile.importance_weight,
                profile.preserve_invalidated_records,
                profile.summary_schema.as_str(),
                document_json,
                authority.provenance_json,
                authority.now,
            ],
        )
        .map_err(storage_db_error)?;
    Ok(())
}

fn insert_memory_profile_revision_projection(
    transaction: &Transaction<'_>,
    revision_id: &str,
    profile: &MemoryProfile,
    document_json: &str,
    authority: &MemoryProfileProjectionAuthority,
) -> CoreResult<()> {
    transaction
        .execute(
            "INSERT INTO memory_profile_revisions
             (revision_id, memory_profile_id, revision_no, name,
              summary_task_profile_revision_id,
              embedding_task_profile_revision_id, turns_per_summary,
              recent_raw_budget, episodic_budget, semantic_budget,
              retrieval_count, recency_weight_millionths,
              similarity_weight_millionths, importance_weight_millionths,
              preserve_invalidated_records, summary_schema_revision_id,
              document_json)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12,
                     ?13, ?14, ?15, ?16, ?17)",
            params![
                revision_id,
                profile.id.as_str(),
                i64_revision(content_revision_number(transaction, revision_id)?)?,
                profile.name,
                authority.summary_task_revision,
                authority.embedding_task_revision,
                profile.turns_per_summary,
                profile.recent_raw_budget.max_tokens,
                profile.episodic_budget.max_tokens,
                profile.semantic_budget.max_tokens,
                profile.retrieval_count,
                weight_millionths(profile.recency_weight)?,
                weight_millionths(profile.similarity_weight)?,
                weight_millionths(profile.importance_weight)?,
                profile.preserve_invalidated_records,
                authority.summary_schema_revision,
                document_json,
            ],
        )
        .map_err(storage_db_error)?;
    Ok(())
}

fn validate_memory_profile(profile: &MemoryProfile) -> CoreResult<()> {
    profile
        .validate()
        .map_err(|error| CoreError::invalid(format!("memory profile is invalid: {error}")))
}

fn weight_millionths(value: f32) -> CoreResult<u32> {
    if !value.is_finite() || !(0.0..=1.0).contains(&value) {
        return Err(CoreError::invalid(
            "memory weight must be within zero and one",
        ));
    }
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    Ok((f64::from(value) * 1_000_000.0).round() as u32)
}

fn ensure_memory_summary_schema(
    transaction: &Transaction<'_>,
    id: &lorepia_domain::SummarySchemaId,
    provenance: &Provenance,
) -> CoreResult<String> {
    if let Some(revision_id) = transaction
        .query_row(
            "SELECT state.active_revision_id
             FROM content_objects AS object
             JOIN content_object_state AS state ON state.object_id = object.id
             WHERE object.id = ?1
               AND object.object_kind = 'memory_summary_schema'
               AND object.deleted_at IS NULL",
            [id.as_str()],
            |row| row.get::<_, String>(0),
        )
        .optional()
        .map_err(storage_db_error)?
    {
        return Ok(revision_id);
    }
    let schema = VersionedJson {
        schema_version: 1,
        value: serde_json::json!({
            "type": "object",
            "additionalProperties": true,
        }),
    };
    let written = append_content_revision(
        transaction,
        DocumentTable::MemorySummarySchemas,
        id.as_str(),
        1,
        &schema,
        provenance,
        None,
        RevisionEventKind::Create,
    )?;
    let (document_json, _) = encode_document("memory summary schema", &schema)?;
    let schema_json = serde_json::to_string(&schema.value).map_err(|error| {
        CoreError::invalid(format!("cannot encode memory summary schema: {error}"))
    })?;
    let schema_sha256 = sha256_hex(schema_json.as_bytes());
    let now = Utc::now().to_rfc3339();
    transaction
        .execute(
            "INSERT INTO memory_summary_schemas
             (id, name, schema_version, revision, schema_json, document_json,
              created_at, updated_at, deleted_at)
             VALUES (?1, ?1, 1, 1, ?2, ?3, ?4, ?4, NULL)",
            params![id.as_str(), schema_json, document_json, now],
        )
        .map_err(storage_db_error)?;
    transaction
        .execute(
            "INSERT INTO memory_summary_schema_revisions
             (revision_id, summary_schema_id, revision_no, name, schema_json,
              schema_sha256, document_json)
             VALUES (?1, ?2, 1, ?2, ?3, ?4, ?5)",
            params![
                written.revision_id,
                id.as_str(),
                schema_json,
                schema_sha256,
                document_json,
            ],
        )
        .map_err(storage_db_error)?;
    Ok(written.revision_id)
}

pub(super) fn write_knowledge_book_projection(
    transaction: &Transaction<'_>,
    revision_id: &str,
    book: &KnowledgeBook,
    document_json: &str,
    expected_revision: Option<u64>,
) -> CoreResult<()> {
    book.validate()
        .map_err(|error| CoreError::invalid(format!("knowledge book is invalid: {error}")))?;
    let state_version = expected_revision.map_or(1, |value| value.saturating_add(1));
    let now = Utc::now().to_rfc3339();
    let provenance_json = serde_json::to_string(&book.provenance)
        .map_err(|error| CoreError::invalid(format!("cannot encode book provenance: {error}")))?;
    transaction
        .execute(
            "INSERT INTO knowledge_books
             (id, name, schema_version, revision, scan_depth, token_budget,
              recursive, max_recursion_depth, document_json, provenance_json,
              source_kind, source_hash, created_at, updated_at, deleted_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12,
                     ?13, ?13, NULL)
             ON CONFLICT(id) DO UPDATE SET
                 name = excluded.name,
                 schema_version = excluded.schema_version,
                 revision = excluded.revision,
                 scan_depth = excluded.scan_depth,
                 token_budget = excluded.token_budget,
                 recursive = excluded.recursive,
                 max_recursion_depth = excluded.max_recursion_depth,
                 document_json = excluded.document_json,
                 provenance_json = excluded.provenance_json,
                 source_kind = excluded.source_kind,
                 source_hash = excluded.source_hash,
                 updated_at = excluded.updated_at",
            params![
                book.id.as_str(),
                book.name,
                book.schema_version,
                i64_revision(state_version)?,
                book.scan_depth,
                book.token_budget.max_tokens,
                book.recursive,
                book.max_recursion_depth,
                document_json,
                provenance_json,
                source_kind_str(&book.provenance.source_kind),
                book.provenance.source_hash,
                now,
            ],
        )
        .map_err(storage_db_error)?;
    let revision_no = content_revision_number(transaction, revision_id)?;
    transaction
        .execute(
            "INSERT INTO knowledge_book_revisions
             (revision_id, knowledge_book_id, revision_no, name, description,
              token_budget, scan_depth, recursive, max_recursion_depth,
              document_json)
             VALUES (?1, ?2, ?3, ?4, '', ?5, ?6, ?7, ?8, ?9)",
            params![
                revision_id,
                book.id.as_str(),
                i64_revision(revision_no)?,
                book.name,
                book.token_budget.max_tokens,
                book.scan_depth,
                book.recursive,
                book.max_recursion_depth,
                document_json,
            ],
        )
        .map_err(storage_db_error)?;
    write_knowledge_entries(transaction, revision_id, book)
}

/// Clones immutable knowledge configuration rows for a tombstone without
/// revalidating a readable pre-canonical document as a new live write.
fn clone_knowledge_book_tombstone_projection(
    transaction: &Transaction<'_>,
    revision_id: &str,
) -> CoreResult<()> {
    let parent_revision_id =
        parent_content_revision_id(transaction, revision_id, "knowledge_book")?;
    let revision_no = content_revision_number(transaction, revision_id)?;
    let changed = transaction
        .execute(
            "INSERT INTO knowledge_book_revisions
             (revision_id, knowledge_book_id, revision_no, name, description,
              token_budget, scan_depth, recursive, max_recursion_depth,
              document_json)
             SELECT ?1, knowledge_book_id, ?2, name, description,
                    token_budget, scan_depth, recursive, max_recursion_depth,
                    document_json
             FROM knowledge_book_revisions
             WHERE revision_id = ?3",
            params![revision_id, i64_revision(revision_no)?, parent_revision_id],
        )
        .map_err(storage_db_error)?;
    if changed != 1 {
        return Err(storage_corrupted(
            "knowledge book source projection is missing during soft delete",
        ));
    }
    transaction
        .execute(
            "INSERT INTO knowledge_entries
             (book_revision_id, entry_id, ordinal, parent_entry_id, name,
              content, enabled, activation_kind, activation_json, priority,
              importance, placement, token_priority, min_tokens, max_tokens,
              reserve_tokens, overflow_policy,
              activation_probability_basis_points, cacheable, provenance_json,
              document_json)
             SELECT ?1, entry_id, ordinal, parent_entry_id, name, content,
                    enabled, activation_kind, activation_json, priority,
                    importance, placement, token_priority, min_tokens,
                    max_tokens, reserve_tokens, overflow_policy,
                    activation_probability_basis_points, cacheable,
                    provenance_json, document_json
             FROM knowledge_entries
             WHERE book_revision_id = ?2
             ORDER BY rowid",
            params![revision_id, parent_revision_id],
        )
        .map_err(storage_db_error)?;
    transaction
        .execute(
            "INSERT INTO knowledge_activation_terms
             (book_revision_id, entry_id, rule_path, term_ordinal, term_kind,
              term_text, normalized_term, term_json, case_sensitive,
              whole_word)
             SELECT ?1, entry_id, rule_path, term_ordinal, term_kind, term_text,
                    normalized_term, term_json, case_sensitive, whole_word
             FROM knowledge_activation_terms
             WHERE book_revision_id = ?2",
            params![revision_id, parent_revision_id],
        )
        .map_err(storage_db_error)?;
    Ok(())
}

fn write_knowledge_entries(
    transaction: &Transaction<'_>,
    revision_id: &str,
    book: &KnowledgeBook,
) -> CoreResult<()> {
    let mut pending = book.entries.iter().enumerate().collect::<Vec<_>>();
    let mut inserted = BTreeSet::new();
    let mut all_ids = BTreeSet::new();
    for (_, entry) in &pending {
        if entry.book_id != book.id || !all_ids.insert(entry.id.as_str()) {
            return Err(CoreError::invalid(
                "knowledge entry ids must be unique and belong to their book",
            ));
        }
    }
    while !pending.is_empty() {
        let before = pending.len();
        let mut index = 0;
        while index < pending.len() {
            let (ordinal, entry) = pending[index];
            if entry
                .parent_id
                .as_ref()
                .is_some_and(|parent| !inserted.contains(parent.as_str()))
            {
                index += 1;
                continue;
            }
            write_knowledge_entry(transaction, revision_id, ordinal, entry)?;
            inserted.insert(entry.id.as_str());
            pending.remove(index);
        }
        if pending.len() == before {
            return Err(CoreError::invalid(
                "knowledge entry parents contain a cycle or missing id",
            ));
        }
    }
    Ok(())
}

#[cfg(test)]
pub(super) fn weight_millionths_for_test(value: f32) -> CoreResult<u32> {
    weight_millionths(value)
}

#[cfg(test)]
pub(super) fn ensure_memory_summary_schema_for_test(
    transaction: &Transaction<'_>,
    id: &lorepia_domain::SummarySchemaId,
    provenance: &Provenance,
) -> CoreResult<String> {
    ensure_memory_summary_schema(transaction, id, provenance)
}

#[cfg(test)]
pub(super) fn write_knowledge_entries_for_test(
    transaction: &Transaction<'_>,
    revision_id: &str,
    book: &KnowledgeBook,
) -> CoreResult<()> {
    write_knowledge_entries(transaction, revision_id, book)
}

fn write_knowledge_entry(
    transaction: &Transaction<'_>,
    revision_id: &str,
    ordinal: usize,
    entry: &lorepia_domain::KnowledgeEntry,
) -> CoreResult<()> {
    if entry.content.is_empty() || entry.name.trim().is_empty() {
        return Err(CoreError::invalid(
            "knowledge entry name and content are required",
        ));
    }
    let (document_json, _) = encode_document("knowledge entry", entry)?;
    let activation_json = serde_json::to_string(&entry.activation).map_err(|error| {
        CoreError::invalid(format!("cannot encode knowledge activation: {error}"))
    })?;
    let activation_value = serde_json::to_value(&entry.activation).map_err(|error| {
        CoreError::invalid(format!("cannot inspect knowledge activation: {error}"))
    })?;
    let activation_kind = activation_value
        .get("kind")
        .and_then(Value::as_str)
        .ok_or_else(|| CoreError::invalid("knowledge activation kind is missing"))?;
    let provenance_json = serde_json::to_string(&entry.provenance).map_err(|error| {
        CoreError::invalid(format!("cannot encode knowledge provenance: {error}"))
    })?;
    transaction
        .execute(
            "INSERT INTO knowledge_entries
             (book_revision_id, entry_id, ordinal, parent_entry_id, name,
              content, enabled, activation_kind, activation_json, priority,
              importance, placement, token_priority, min_tokens, max_tokens,
              reserve_tokens, overflow_policy, activation_probability_basis_points,
              cacheable, provenance_json, document_json)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12,
                     ?13, ?14, ?15, ?16, 'reduce_knowledge_entries', ?17, 0,
                     ?18, ?19)",
            params![
                revision_id,
                entry.id.as_str(),
                usize_to_i64(ordinal, "knowledge entry ordinal")?,
                entry.parent_id.as_ref().map(KnowledgeEntryId::as_str),
                entry.name,
                entry.content,
                entry.enabled,
                activation_kind,
                activation_json,
                entry.priority,
                entry.importance,
                enum_wire(&entry.placement)?,
                entry.token_policy.priority,
                entry.token_policy.min_tokens,
                entry.token_policy.max_tokens,
                entry.token_policy.reserve_tokens,
                entry.activation_probability_basis_points,
                provenance_json,
                document_json,
            ],
        )
        .map_err(storage_db_error)?;
    Ok(())
}
