use std::{
    collections::BTreeMap,
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    path::{Component, Path, PathBuf},
    time::Duration,
};

use lorepia_domain::{CanonicalOrigin, CoreError, CoreErrorCode, CoreResult, HeaderName};
use rusqlite::{
    Connection, OpenFlags, OptionalExtension, backup::Backup, functions::FunctionFlags,
    types::ValueRef,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use uuid::Uuid;

use crate::database::{
    FROZEN_NATIVE_MIGRATIONS, FROZEN_NATIVE_SCHEMA_VERSION, SCHEMA_VERSION, apply_migrations,
    read_current_schema_version, read_pre_migration_schema_version, storage_db_error,
    truncate_sensitive_migration_wal,
};

const MIN_SUPPORTED_MANIFEST_FORMAT_VERSION: u32 = 2;
const ACTIVE_MANIFEST_FORMAT_VERSION: u32 = 3;
const LEGACY_DATABASE_RELATIVE_PATH: &str = "db/lorepia.sqlite3";
const CUTOVER_DIRECTORY_RELATIVE_PATH: &str = "db/schema-cutover";
const GENERATION_MANIFEST_FILENAME: &str = "generation-manifest.json";
const GENERATION_COMMIT_FILENAME: &str = "generation-committed.json";
const MANIFEST_CHECKSUM_DOMAIN_V2: &str = "lorepia.active-database-manifest.v2";
const MANIFEST_CHECKSUM_DOMAIN: &str = "lorepia.active-database-manifest.v3";
const GENERATION_COMMIT_CHECKSUM_DOMAIN: &str = "lorepia.database-generation-commit.v1";
const MAX_MANIFEST_BYTES: u64 = 64 * 1024;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ActiveDatabaseManifest {
    format_version: u32,
    activation_sequence: u64,
    cutover_id: String,
    parent_cutover_id: Option<String>,
    active_database_relative_path: String,
    active_schema_version: u32,
    source_database_relative_path: String,
    baseline_schema_version: u32,
    source_database_size_bytes: u64,
    source_database_fingerprint_sha256: String,
    rollback_cas_pin_count: u64,
    checksum_sha256: String,
}

#[derive(Serialize)]
struct ManifestChecksumPayload<'a> {
    domain: &'static str,
    format_version: u32,
    activation_sequence: u64,
    cutover_id: &'a str,
    parent_cutover_id: Option<&'a str>,
    active_database_relative_path: &'a str,
    active_schema_version: u32,
    source_database_relative_path: &'a str,
    baseline_schema_version: u32,
    source_database_size_bytes: u64,
    source_database_fingerprint_sha256: &'a str,
    rollback_cas_pin_count: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct RollbackCasPin {
    namespace: String,
    sha256: String,
    relative_path: String,
    size_bytes: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct GenerationCommit {
    format_version: u32,
    cutover_id: String,
    manifest_sha256: String,
    checksum_sha256: String,
}

#[derive(Serialize)]
struct GenerationCommitChecksumPayload<'a> {
    domain: &'static str,
    format_version: u32,
    cutover_id: &'a str,
    manifest_sha256: &'a str,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct SchemaObject {
    object_type: String,
    name: String,
    table_name: String,
    sql: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct TableSnapshot {
    table_name: String,
    columns: Vec<String>,
    row_count: u64,
    rows_sha256: [u8; 32],
}

struct CanonicalSourceBinding<'a> {
    database_size_bytes: u64,
    database_fingerprint_sha256: &'a str,
    rollback_cas_pin_count: u64,
    schema_version: u32,
}

pub(crate) fn open_database(root: &Path, canonical_path: &Path) -> CoreResult<Connection> {
    cleanup_manifest_temps(root)?;
    let committed_generations = read_committed_generations(root)?;
    cleanup_uncommitted_candidates(root, &committed_generations)?;
    if let Some(manifest) = committed_generations.last() {
        let source_reservations =
            validate_generation_source_bindings(root, canonical_path, &committed_generations)?;
        let active = open_or_upgrade_committed_generation(root, manifest)?;
        drop(source_reservations);
        return Ok(active);
    }

    if !canonical_path.try_exists().map_err(storage_io_error)? {
        drop(open_configured_and_migrate(canonical_path)?);
    }
    ensure_regular_file(canonical_path, "canonical database")?;

    let source_reservation = reserve_source_writes(canonical_path)?;
    let source = open_cutover_source(canonical_path)?;
    let source_version = read_pre_migration_schema_version(&source)?;
    validate_cutover_source(&source, source_version)?;
    let source_tables = capture_table_snapshots(&source, true)?;
    // Older migration cutpoints can contain intentional repair transforms.
    // Their migration-specific regressions validate those transforms; the
    // exact frozen baseline and a no-op current-schema publication preserve
    // every existing row exactly.
    let exact_snapshot =
        source_version == FROZEN_NATIVE_SCHEMA_VERSION || source_version == SCHEMA_VERSION;
    let semantic_snapshot = if exact_snapshot {
        semantic_snapshot(&source_tables)
    } else {
        Vec::new()
    };
    let source_database_fingerprint_sha256 =
        database_fingerprint_from_snapshots(&source, &source_tables)?;
    let source_database_size_bytes = database_logical_page_span_bytes(&source)?;
    let rollback_cas_pin_count = validate_rollback_cas_snapshot(root, &source)?;
    let source_binding = CanonicalSourceBinding {
        database_size_bytes: source_database_size_bytes,
        database_fingerprint_sha256: &source_database_fingerprint_sha256,
        rollback_cas_pin_count,
        schema_version: source_version,
    };
    create_generation(
        root,
        &source,
        &semantic_snapshot,
        None,
        source_binding,
        source_reservation,
    )
}

fn open_or_upgrade_committed_generation(
    root: &Path,
    manifest: &ActiveDatabaseManifest,
) -> CoreResult<Connection> {
    let active_path = manifest.active_database_path(root)?;
    let source = open_cutover_source(&active_path)?;
    let active_schema_version = read_pre_migration_schema_version(&source)?;
    if active_schema_version != manifest.active_schema_version {
        return Err(storage_corrupted(
            "active database schema does not match its committed generation manifest",
        ));
    }
    validate_database_integrity(&source)?;
    if active_schema_version == SCHEMA_VERSION {
        drop(source);
        return open_configured_current(&active_path);
    }
    if active_schema_version > SCHEMA_VERSION {
        return Err(storage_corrupted(
            "active database schema is newer than this Core; use a compatible rollback build",
        ));
    }

    drop(source);
    let source_reservation = reserve_source_writes(&active_path)?;
    let source = open_cutover_source(&active_path)?;
    let active_schema_version = read_pre_migration_schema_version(&source)?;
    if active_schema_version != manifest.active_schema_version {
        return Err(storage_corrupted(
            "active database schema changed while reserving its generation upgrade",
        ));
    }
    validate_database_integrity(&source)?;
    let source_tables = capture_table_snapshots(&source, true)?;
    let semantic_snapshot = semantic_snapshot(&source_tables);
    let source_database_fingerprint_sha256 =
        database_fingerprint_from_snapshots(&source, &source_tables)?;
    let source_database_size_bytes = database_logical_page_span_bytes(&source)?;
    let rollback_cas_pin_count = validate_rollback_cas_snapshot(root, &source)?;
    create_generation(
        root,
        &source,
        &semantic_snapshot,
        Some(manifest),
        CanonicalSourceBinding {
            database_size_bytes: source_database_size_bytes,
            database_fingerprint_sha256: &source_database_fingerprint_sha256,
            rollback_cas_pin_count,
            schema_version: active_schema_version,
        },
        source_reservation,
    )
}

fn create_generation(
    root: &Path,
    source: &Connection,
    semantic_snapshot: &[TableSnapshot],
    parent: Option<&ActiveDatabaseManifest>,
    source_binding: CanonicalSourceBinding<'_>,
    source_reservation: Connection,
) -> CoreResult<Connection> {
    let (cutover_id, candidate_path) = create_candidate_path(root)?;
    if let Err(error) = backup_database(source, &candidate_path) {
        cleanup_candidate_directory(
            candidate_path
                .parent()
                .ok_or_else(|| storage_corrupted("candidate database has no parent directory"))?,
        )?;
        return Err(error);
    }
    cutover_failpoint("after_backup");

    let candidate_result = (|| {
        let mut candidate = open_configured(&candidate_path)?;
        apply_migrations(&mut candidate)?;
        validate_current_candidate(&candidate, semantic_snapshot)?;
        crate::database::prepare_cutover_candidate_for_open(&mut candidate)?;
        cutover_failpoint("after_migrations");
        truncate_sensitive_migration_wal(&candidate)?;
        sync_file(&candidate_path).map_err(storage_io_error)?;
        sync_directory(
            candidate_path
                .parent()
                .ok_or_else(|| storage_corrupted("candidate database has no parent directory"))?,
        )?;
        cutover_failpoint("after_candidate_sync");
        cutover_pausepoint("before_generation_publication");
        Ok(candidate)
    })();
    let candidate =
        match candidate_result {
            Ok(candidate) => candidate,
            Err(error) => {
                cleanup_candidate_directory(candidate_path.parent().ok_or_else(|| {
                    storage_corrupted("candidate database has no parent directory")
                })?)?;
                return Err(error);
            }
        };

    let manifest = ActiveDatabaseManifest::new(
        &cutover_id,
        parent,
        source_binding.database_size_bytes,
        source_binding.database_fingerprint_sha256,
        source_binding.rollback_cas_pin_count,
        source_binding.schema_version,
    )?;
    publish_committed_generation(root, &manifest)?;
    // Closing the write-reservation connection rolls back its empty
    // `BEGIN IMMEDIATE` transaction only after the commit marker is durable.
    drop(source_reservation);
    Ok(candidate)
}

fn reserve_source_writes(path: &Path) -> CoreResult<Connection> {
    let reservation = Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_WRITE
            | OpenFlags::SQLITE_OPEN_NO_MUTEX
            | OpenFlags::SQLITE_OPEN_PRIVATE_CACHE
            | OpenFlags::SQLITE_OPEN_NOFOLLOW,
    )
    .map_err(storage_db_error)?;
    reservation
        .busy_timeout(Duration::from_secs(5))
        .map_err(storage_db_error)?;
    reservation
        .execute_batch("BEGIN IMMEDIATE")
        .map_err(storage_db_error)?;
    Ok(reservation)
}

fn open_cutover_source(path: &Path) -> CoreResult<Connection> {
    Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_ONLY
            | OpenFlags::SQLITE_OPEN_NO_MUTEX
            | OpenFlags::SQLITE_OPEN_PRIVATE_CACHE
            | OpenFlags::SQLITE_OPEN_NOFOLLOW,
    )
    .map_err(storage_db_error)
}

fn open_configured_and_migrate(path: &Path) -> CoreResult<Connection> {
    let mut connection = open_configured(path)?;
    apply_migrations(&mut connection)?;
    validate_database_integrity(&connection)?;
    read_current_schema_version(&connection)?;
    Ok(connection)
}

fn open_configured_current(path: &Path) -> CoreResult<Connection> {
    let connection = open_configured(path)?;
    validate_database_integrity(&connection)?;
    read_current_schema_version(&connection)?;
    Ok(connection)
}

fn open_configured(path: &Path) -> CoreResult<Connection> {
    let connection = Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_WRITE
            | OpenFlags::SQLITE_OPEN_CREATE
            | OpenFlags::SQLITE_OPEN_NO_MUTEX
            | OpenFlags::SQLITE_OPEN_PRIVATE_CACHE
            | OpenFlags::SQLITE_OPEN_NOFOLLOW,
    )
    .map_err(storage_db_error)?;
    register_integrity_functions(&connection)?;
    connection
        .pragma_update(None, "foreign_keys", true)
        .map_err(storage_db_error)?;
    connection
        .pragma_update(None, "journal_mode", "WAL")
        .map_err(storage_db_error)?;
    connection
        .pragma_update(None, "synchronous", "FULL")
        .map_err(storage_db_error)?;
    Ok(connection)
}

pub(crate) fn register_integrity_functions(connection: &Connection) -> CoreResult<()> {
    let flags = FunctionFlags::SQLITE_UTF8
        | FunctionFlags::SQLITE_DETERMINISTIC
        | FunctionFlags::SQLITE_INNOCUOUS;
    connection
        .create_scalar_function("lorepia_sha256_hex", 1, flags, |context| {
            let value = context.get::<String>(0)?;
            Ok(format!("{:x}", Sha256::digest(value.as_bytes())))
        })
        .map_err(storage_db_error)?;
    connection
        .create_scalar_function(
            "lorepia_discovery_commit_plan_sha256",
            1,
            flags,
            |context| {
                let value = context.get::<String>(0)?;
                Ok(crate::discovery_repository::canonical_discovery_commit_plan_sha256(&value))
            },
        )
        .map_err(storage_db_error)?;
    connection
        .create_scalar_function("lorepia_canonical_origin", 1, flags, |context| {
            let value = context.get::<String>(0)?;
            Ok(CanonicalOrigin::parse(&value)
                .ok()
                .map(|origin| origin.to_string()))
        })
        .map_err(storage_db_error)?;
    connection
        .create_scalar_function("lorepia_header_name", 1, flags, |context| {
            let value = context.get::<String>(0)?;
            Ok(HeaderName::parse(&value)
                .ok()
                .map(|name| name.as_str().to_owned()))
        })
        .map_err(storage_db_error)?;
    connection
        .create_scalar_function(
            "lorepia_native_no_effect_evidence_sha256",
            8,
            flags,
            |context| {
                let evidence = serde_json::json!({
                    "schema_version": context.get::<i64>(0)?,
                    "attestation_kind": context.get::<String>(1)?,
                    "recovery_owner": context.get::<String>(2)?,
                    "operation_id": context.get::<String>(3)?,
                    "session_id": context.get::<String>(4)?,
                    "commit_attempt_id": context.get::<String>(5)?,
                    "commit_plan_sha256": context.get::<String>(6)?,
                    "connection_id": context.get::<String>(7)?,
                });
                Ok(format!(
                    "{:x}",
                    Sha256::digest(evidence.to_string().as_bytes())
                ))
            },
        )
        .map_err(storage_db_error)?;
    Ok(())
}

fn validate_frozen_source(connection: &Connection) -> CoreResult<()> {
    if read_pre_migration_schema_version(connection)? != FROZEN_NATIVE_SCHEMA_VERSION {
        return Err(storage_corrupted(
            "frozen native cutover source is not schema eleven",
        ));
    }
    validate_database_integrity(connection)?;
    let actual = schema_inventory(connection)?;
    let expected = expected_frozen_schema_inventory()?;
    if actual != expected {
        return Err(storage_corrupted(
            "frozen native schema inventory does not match the approved schema-eleven baseline",
        ));
    }
    Ok(())
}

fn validate_cutover_source(connection: &Connection, source_version: u32) -> CoreResult<()> {
    if source_version == FROZEN_NATIVE_SCHEMA_VERSION {
        validate_frozen_source(connection)
    } else {
        validate_database_integrity(connection)
    }
}

fn validate_current_candidate(
    connection: &Connection,
    semantic_snapshot: &[TableSnapshot],
) -> CoreResult<()> {
    read_current_schema_version(connection)?;
    validate_database_integrity(connection)?;
    validate_semantic_snapshot(connection, semantic_snapshot)
}

fn validate_database_integrity(connection: &Connection) -> CoreResult<()> {
    let foreign_key_violation = connection
        .query_row("PRAGMA foreign_key_check", [], |_| Ok(()))
        .optional()
        .map_err(storage_db_error)?
        .is_some();
    if foreign_key_violation {
        return Err(storage_corrupted(
            "database cutover foreign-key validation failed",
        ));
    }
    let integrity = connection
        .query_row("PRAGMA integrity_check", [], |row| row.get::<_, String>(0))
        .map_err(storage_db_error)?;
    if integrity != "ok" {
        return Err(storage_corrupted(format!(
            "database cutover integrity validation failed: {integrity}"
        )));
    }
    Ok(())
}

fn expected_frozen_schema_inventory() -> CoreResult<Vec<SchemaObject>> {
    let mut reference = Connection::open_in_memory().map_err(storage_db_error)?;
    reference
        .pragma_update(None, "foreign_keys", true)
        .map_err(storage_db_error)?;
    for (index, migration) in FROZEN_NATIVE_MIGRATIONS.iter().enumerate() {
        let version = u32::try_from(index + 1)
            .map_err(|_| CoreError::internal("frozen schema version overflowed"))?;
        let transaction = reference.transaction().map_err(storage_db_error)?;
        transaction
            .execute_batch(migration)
            .map_err(storage_db_error)?;
        transaction
            .execute(
                "INSERT OR IGNORE INTO schema_migrations(version, applied_at) VALUES (?1, ?2)",
                (version, "frozen-schema-reference"),
            )
            .map_err(storage_db_error)?;
        transaction.commit().map_err(storage_db_error)?;
    }
    schema_inventory(&reference)
}

fn schema_inventory(connection: &Connection) -> CoreResult<Vec<SchemaObject>> {
    let mut statement = connection
        .prepare(
            "SELECT type, name, tbl_name, COALESCE(sql, '')
             FROM sqlite_schema
             WHERE name NOT LIKE 'sqlite_%'
             ORDER BY type, name, tbl_name",
        )
        .map_err(storage_db_error)?;
    statement
        .query_map([], |row| {
            Ok(SchemaObject {
                object_type: row.get(0)?,
                name: row.get(1)?,
                table_name: row.get(2)?,
                sql: row.get(3)?,
            })
        })
        .map_err(storage_db_error)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(storage_db_error)
}

fn semantic_snapshot(tables: &[TableSnapshot]) -> Vec<TableSnapshot> {
    tables
        .iter()
        .filter(|table| table.table_name != "schema_migrations")
        .cloned()
        .collect()
}

fn capture_table_snapshots(
    connection: &Connection,
    include_schema_registry: bool,
) -> CoreResult<Vec<TableSnapshot>> {
    let mut statement = connection
        .prepare(
            "SELECT name
             FROM sqlite_schema
             WHERE type = 'table'
               AND (name = 'sqlite_sequence' OR name NOT LIKE 'sqlite_%')
               AND (?1 OR name != 'schema_migrations')
             ORDER BY name",
        )
        .map_err(storage_db_error)?;
    let table_names = statement
        .query_map([include_schema_registry], |row| row.get::<_, String>(0))
        .map_err(storage_db_error)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(storage_db_error)?;
    table_names
        .into_iter()
        .map(|table_name| {
            let columns = table_columns(connection, &table_name)?;
            let rows = table_rows(connection, &table_name, &columns)?;
            Ok(TableSnapshot {
                table_name,
                columns,
                row_count: rows.0,
                rows_sha256: rows.1,
            })
        })
        .collect()
}

fn database_fingerprint(connection: &Connection) -> CoreResult<String> {
    let tables = capture_table_snapshots(connection, true)?;
    database_fingerprint_from_snapshots(connection, &tables)
}

fn database_fingerprint_from_snapshots(
    connection: &Connection,
    tables: &[TableSnapshot],
) -> CoreResult<String> {
    let mut digest = Sha256::new();
    digest_field(&mut digest, b"lorepia.frozen-source-fingerprint.v1");
    for object in schema_inventory(connection)? {
        digest_field(&mut digest, object.object_type.as_bytes());
        digest_field(&mut digest, object.name.as_bytes());
        digest_field(&mut digest, object.table_name.as_bytes());
        digest_field(&mut digest, object.sql.as_bytes());
    }
    for table in tables {
        digest_field(&mut digest, table.table_name.as_bytes());
        for column in &table.columns {
            digest_field(&mut digest, column.as_bytes());
        }
        digest_field(&mut digest, &table.row_count.to_be_bytes());
        digest_field(&mut digest, &table.rows_sha256);
    }
    Ok(format!("{:x}", digest.finalize()))
}

fn database_logical_page_span_bytes(connection: &Connection) -> CoreResult<u64> {
    let page_count = connection
        .query_row("PRAGMA page_count", [], |row| row.get::<_, u64>(0))
        .map_err(storage_db_error)?;
    let page_size = connection
        .query_row("PRAGMA page_size", [], |row| row.get::<_, u64>(0))
        .map_err(storage_db_error)?;
    page_count
        .checked_mul(page_size)
        .ok_or_else(|| storage_corrupted("database source logical page span overflowed"))
}

fn digest_field(digest: &mut Sha256, value: &[u8]) {
    digest.update(u64::try_from(value.len()).unwrap_or(u64::MAX).to_be_bytes());
    digest.update(value);
}

fn validate_semantic_snapshot(
    connection: &Connection,
    snapshots: &[TableSnapshot],
) -> CoreResult<()> {
    for snapshot in snapshots {
        let current_columns = table_columns(connection, &snapshot.table_name)?;
        if snapshot
            .columns
            .iter()
            .any(|column| !current_columns.contains(column))
        {
            return Err(storage_corrupted(format!(
                "database cutover removed a baseline column from {}",
                snapshot.table_name
            )));
        }
        let current_rows = table_rows(connection, &snapshot.table_name, &snapshot.columns)?;
        if current_rows != (snapshot.row_count, snapshot.rows_sha256) {
            return Err(storage_corrupted(format!(
                "database cutover changed baseline rows in {}",
                snapshot.table_name
            )));
        }
    }
    Ok(())
}

fn table_columns(connection: &Connection, table_name: &str) -> CoreResult<Vec<String>> {
    let sql = format!("PRAGMA table_info({})", quote_identifier(table_name));
    let mut statement = connection.prepare(&sql).map_err(storage_db_error)?;
    let columns = statement
        .query_map([], |row| row.get::<_, String>(1))
        .map_err(storage_db_error)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(storage_db_error)?;
    if columns.is_empty() {
        return Err(storage_corrupted(format!(
            "database cutover is missing baseline table {table_name}"
        )));
    }
    Ok(columns)
}

fn table_rows(
    connection: &Connection,
    table_name: &str,
    columns: &[String],
) -> CoreResult<(u64, [u8; 32])> {
    let projection = columns
        .iter()
        .map(|column| quote_identifier(column))
        .collect::<Vec<_>>()
        .join(", ");
    let ordering = columns
        .iter()
        .flat_map(|column| {
            let quoted = quote_identifier(column);
            [format!("typeof({quoted})"), format!("quote({quoted})")]
        })
        .collect::<Vec<_>>()
        .join(", ");
    let sql = format!(
        "SELECT {projection} FROM {} ORDER BY {ordering}",
        quote_identifier(table_name)
    );
    let mut statement = connection.prepare(&sql).map_err(storage_db_error)?;
    let mut rows = statement.query([]).map_err(storage_db_error)?;
    let mut row_count = 0_u64;
    let mut table_digest = Sha256::new();
    digest_field(&mut table_digest, b"lorepia.cutover-table-rows.v1");
    while let Some(row) = rows.next().map_err(storage_db_error)? {
        let mut row_digest = Sha256::new();
        digest_field(&mut row_digest, b"lorepia.cutover-row.v1");
        for index in 0..columns.len() {
            digest_value(
                row.get_ref(index).map_err(storage_db_error)?,
                &mut row_digest,
            );
        }
        digest_field(&mut table_digest, &row_digest.finalize());
        row_count = row_count
            .checked_add(1)
            .ok_or_else(|| CoreError::internal("database cutover row count overflowed"))?;
    }
    digest_field(&mut table_digest, &row_count.to_be_bytes());
    Ok((row_count, table_digest.finalize().into()))
}

fn digest_value(value: ValueRef<'_>, digest: &mut Sha256) {
    match value {
        ValueRef::Null => digest_field(digest, &[0]),
        ValueRef::Integer(value) => {
            digest_field(digest, &[1]);
            digest_field(digest, &value.to_be_bytes());
        }
        ValueRef::Real(value) => {
            digest_field(digest, &[2]);
            digest_field(digest, &value.to_bits().to_be_bytes());
        }
        ValueRef::Text(value) => {
            digest_field(digest, &[3]);
            digest_field(digest, value);
        }
        ValueRef::Blob(value) => {
            digest_field(digest, &[4]);
            digest_field(digest, value);
        }
    }
}

fn quote_identifier(value: &str) -> String {
    format!("\"{}\"", value.replace('"', "\"\""))
}

fn create_candidate_path(root: &Path) -> CoreResult<(String, PathBuf)> {
    let database_dir = root.join("db");
    ensure_real_directory(&database_dir, "database directory")?;
    let cutover_dir = root.join(CUTOVER_DIRECTORY_RELATIVE_PATH);
    create_real_directory(&cutover_dir, &database_dir)?;

    for _ in 0..8 {
        let cutover_id = Uuid::new_v4().to_string();
        let candidate_dir = cutover_dir.join(&cutover_id);
        match fs::create_dir(&candidate_dir) {
            Ok(()) => {
                sync_directory(&cutover_dir)?;
                return Ok((cutover_id, candidate_dir.join("lorepia.sqlite3")));
            }
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(error) => return Err(storage_io_error(error)),
        }
    }
    Err(CoreError::internal(
        "cannot allocate a unique database cutover generation",
    ))
}

fn create_real_directory(path: &Path, parent: &Path) -> CoreResult<()> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_dir() && !metadata_is_reparse_point(&metadata) => {
            return Ok(());
        }
        Ok(_) => {
            return Err(storage_corrupted(format!(
                "owned cutover path is not a real directory: {}",
                path.display()
            )));
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(storage_io_error(error)),
    }
    fs::create_dir(path).map_err(storage_io_error)?;
    sync_directory(parent)
}

fn backup_database(source: &Connection, candidate_path: &Path) -> CoreResult<()> {
    let mut destination = Connection::open_with_flags(
        candidate_path,
        OpenFlags::SQLITE_OPEN_READ_WRITE
            | OpenFlags::SQLITE_OPEN_CREATE
            | OpenFlags::SQLITE_OPEN_NO_MUTEX
            | OpenFlags::SQLITE_OPEN_PRIVATE_CACHE
            | OpenFlags::SQLITE_OPEN_NOFOLLOW,
    )
    .map_err(storage_db_error)?;
    let backup = Backup::new(source, &mut destination).map_err(storage_db_error)?;
    backup
        .run_to_completion(128, Duration::from_millis(1), None)
        .map_err(storage_db_error)?;
    drop(backup);
    drop(destination);
    ensure_regular_file(candidate_path, "candidate database")?;
    sync_file(candidate_path).map_err(storage_io_error)
}

fn validate_rollback_cas_snapshot(root: &Path, connection: &Connection) -> CoreResult<u64> {
    let pins = rollback_cas_snapshot(connection, true)?;
    for pin in &pins {
        validate_rollback_cas_pin(root, pin)?;
    }
    u64::try_from(pins.len()).map_err(|_| CoreError::internal("rollback CAS pin count overflowed"))
}

fn validate_format_two_rollback_cas_snapshot(
    root: &Path,
    connection: &Connection,
) -> CoreResult<u64> {
    let counted_pins = rollback_cas_snapshot(connection, false)?;
    for pin in rollback_cas_snapshot(connection, true)? {
        validate_rollback_cas_pin(root, &pin)?;
    }
    u64::try_from(counted_pins.len())
        .map_err(|_| CoreError::internal("rollback CAS pin count overflowed"))
}

fn rollback_cas_snapshot(
    connection: &Connection,
    include_promotion_journal: bool,
) -> CoreResult<Vec<RollbackCasPin>> {
    let mut pins = BTreeMap::<(String, String), RollbackCasPin>::new();
    for (namespace, table) in [("sources", "content_sources"), ("assets", "assets")] {
        if !table_exists(connection, table)? {
            continue;
        }
        let sql = format!("SELECT sha256, relative_path, size_bytes FROM {table} ORDER BY sha256");
        let mut statement = connection.prepare(&sql).map_err(storage_db_error)?;
        let mut rows = statement.query([]).map_err(storage_db_error)?;
        while let Some(row) = rows.next().map_err(storage_db_error)? {
            let sha256 = row.get::<_, String>(0).map_err(storage_db_error)?;
            let relative_path = row.get::<_, String>(1).map_err(storage_db_error)?;
            let raw_size = row.get::<_, i64>(2).map_err(storage_db_error)?;
            let size_bytes = u64::try_from(raw_size)
                .map_err(|_| storage_corrupted("rollback CAS pin has a negative size"))?;
            let pin = RollbackCasPin {
                namespace: namespace.to_owned(),
                sha256,
                relative_path,
                size_bytes,
            };
            insert_rollback_cas_pin(&mut pins, pin)?;
        }
    }
    if include_promotion_journal && table_exists(connection, "package_cas_promotion_journal")? {
        let mut statement = connection
            .prepare(
                "SELECT namespace, sha256, relative_path, size_bytes
                 FROM package_cas_promotion_journal
                 WHERE phase IN ('file_durable', 'row_registered')
                 ORDER BY namespace, sha256, import_id",
            )
            .map_err(storage_db_error)?;
        let mut rows = statement.query([]).map_err(storage_db_error)?;
        while let Some(row) = rows.next().map_err(storage_db_error)? {
            let namespace = match row.get::<_, String>(0).map_err(storage_db_error)?.as_str() {
                "source" => "sources",
                "asset" => "assets",
                _ => {
                    return Err(storage_corrupted(
                        "rollback CAS journal namespace is unsupported",
                    ));
                }
            };
            let raw_size = row.get::<_, i64>(3).map_err(storage_db_error)?;
            let pin = RollbackCasPin {
                namespace: namespace.to_owned(),
                sha256: row.get(1).map_err(storage_db_error)?,
                relative_path: row.get(2).map_err(storage_db_error)?,
                size_bytes: u64::try_from(raw_size)
                    .map_err(|_| storage_corrupted("rollback CAS pin has a negative size"))?,
            };
            insert_rollback_cas_pin(&mut pins, pin)?;
        }
    }
    Ok(pins.into_values().collect())
}

fn insert_rollback_cas_pin(
    pins: &mut BTreeMap<(String, String), RollbackCasPin>,
    pin: RollbackCasPin,
) -> CoreResult<()> {
    pin.validate()?;
    let key = (pin.namespace.clone(), pin.sha256.clone());
    if let Some(existing) = pins.get(&key)
        && existing != &pin
    {
        return Err(storage_corrupted(
            "rollback CAS sources disagree about an object's immutable identity",
        ));
    }
    pins.insert(key, pin);
    Ok(())
}

fn validate_rollback_cas_pin(root: &Path, pin: &RollbackCasPin) -> CoreResult<()> {
    pin.validate()?;
    let path = root.join(&pin.relative_path);
    ensure_cas_pin_ancestors(root, pin)?;
    ensure_regular_file(&path, "rollback CAS object")?;
    let metadata = fs::metadata(&path).map_err(storage_io_error)?;
    if metadata.len() != pin.size_bytes || sha256_file(&path)? != pin.sha256 {
        return Err(storage_corrupted(
            "database cutover rollback CAS object does not match its pin",
        ));
    }
    Ok(())
}

pub(crate) fn is_rollback_cas_pinned(
    root: &Path,
    namespace: &str,
    sha256: &str,
) -> CoreResult<bool> {
    if !matches!(namespace, "sources" | "assets") {
        return Err(CoreError::internal(
            "unsupported database cutover rollback CAS namespace",
        ));
    }
    let committed_generations = read_committed_generations(root)?;
    if committed_generations.is_empty() {
        return Ok(false);
    }
    if !valid_sha256(sha256) {
        return Err(storage_corrupted("rollback CAS lookup hash is invalid"));
    }
    let table = match namespace {
        "sources" => "content_sources",
        "assets" => "assets",
        _ => unreachable!("validated rollback CAS namespace"),
    };
    let canonical_path = root.join(LEGACY_DATABASE_RELATIVE_PATH);
    for index in 0..committed_generations.len() {
        let source_path =
            generation_source_database_path(root, &canonical_path, &committed_generations, index)?;
        let source = open_cutover_source(&source_path)?;
        let table_pin = table_exists(&source, table)?
            && source
                .query_row(
                    &format!("SELECT EXISTS(SELECT 1 FROM {table} WHERE sha256 = ?1)"),
                    [sha256],
                    |row| row.get::<_, bool>(0),
                )
                .map_err(storage_db_error)?;
        let journal_namespace = match namespace {
            "sources" => "source",
            "assets" => "asset",
            _ => unreachable!("validated rollback CAS namespace"),
        };
        let journal_pin = table_exists(&source, "package_cas_promotion_journal")?
            && source
                .query_row(
                    "SELECT EXISTS(
                         SELECT 1 FROM package_cas_promotion_journal
                         WHERE namespace = ?1 AND sha256 = ?2
                           AND phase IN ('file_durable', 'row_registered')
                     )",
                    [journal_namespace, sha256],
                    |row| row.get::<_, bool>(0),
                )
                .map_err(storage_db_error)?;
        if table_pin || journal_pin {
            return Ok(true);
        }
    }
    Ok(false)
}

fn table_exists(connection: &Connection, table: &str) -> CoreResult<bool> {
    connection
        .query_row(
            "SELECT EXISTS(
                 SELECT 1 FROM sqlite_schema WHERE type = 'table' AND name = ?1
             )",
            [table],
            |row| row.get::<_, bool>(0),
        )
        .map_err(storage_db_error)
}

fn ensure_cas_pin_ancestors(root: &Path, pin: &RollbackCasPin) -> CoreResult<()> {
    let namespace = root.join(&pin.namespace);
    let sha_root = namespace.join("sha256");
    let prefix = sha_root.join(&pin.sha256[..2]);
    ensure_real_directory(&namespace, "rollback CAS namespace")?;
    ensure_real_directory(&sha_root, "rollback CAS SHA root")?;
    ensure_real_directory(&prefix, "rollback CAS hash prefix")
}

fn sha256_file(path: &Path) -> CoreResult<String> {
    let mut reader = File::open(path).map_err(storage_io_error)?;
    let mut digest = Sha256::new();
    let mut buffer = [0_u8; 16 * 1024];
    loop {
        let count = reader.read(&mut buffer).map_err(storage_io_error)?;
        if count == 0 {
            break;
        }
        digest.update(&buffer[..count]);
    }
    Ok(format!("{:x}", digest.finalize()))
}

fn generation_source_database_path(
    root: &Path,
    canonical_path: &Path,
    manifests: &[ActiveDatabaseManifest],
    index: usize,
) -> CoreResult<PathBuf> {
    let manifest = manifests
        .get(index)
        .ok_or_else(|| CoreError::internal("database generation index is out of range"))?;
    let (expected_relative_path, source_path) = match index.checked_sub(1) {
        None => (LEGACY_DATABASE_RELATIVE_PATH, canonical_path.to_path_buf()),
        Some(parent_index) => {
            let parent = &manifests[parent_index];
            (
                parent.active_database_relative_path.as_str(),
                parent.active_database_path(root)?,
            )
        }
    };
    if manifest.source_database_relative_path != expected_relative_path {
        return Err(storage_corrupted(
            "database generation source does not match its parent chain",
        ));
    }
    ensure_regular_file(&source_path, "database generation source")?;
    Ok(source_path)
}

fn validate_generation_source_bindings(
    root: &Path,
    canonical_path: &Path,
    manifests: &[ActiveDatabaseManifest],
) -> CoreResult<Vec<Connection>> {
    let mut reservations = Vec::with_capacity(manifests.len());
    for (index, manifest) in manifests.iter().enumerate() {
        let source_path = generation_source_database_path(root, canonical_path, manifests, index)?;
        let reservation = reserve_source_writes(&source_path)?;
        let source = open_cutover_source(&source_path)?;
        validate_generation_source_binding(root, &source, manifest)?;
        reservations.push(reservation);
    }
    Ok(reservations)
}

fn validate_generation_source_binding(
    root: &Path,
    source: &Connection,
    manifest: &ActiveDatabaseManifest,
) -> CoreResult<()> {
    let source_version = read_pre_migration_schema_version(source)?;
    if source_version != manifest.baseline_schema_version {
        return Err(storage_corrupted(
            "database generation source schema diverged from its manifest",
        ));
    }
    validate_cutover_source(source, source_version)?;
    if database_fingerprint(source)? != manifest.source_database_fingerprint_sha256 {
        return Err(storage_corrupted(
            "database generation source diverged from its committed rollback snapshot; recovery must preserve both canonical and active generations, including every sealed parent source",
        ));
    }
    cutover_pausepoint("after_sealed_source_fingerprint");
    if manifest.format_version >= 3
        && database_logical_page_span_bytes(source)? != manifest.source_database_size_bytes
    {
        return Err(storage_corrupted(
            "database generation source size diverged from its committed logical page span",
        ));
    }
    let rollback_cas_pin_count = if manifest.format_version == 2 {
        validate_format_two_rollback_cas_snapshot(root, source)?
    } else {
        validate_rollback_cas_snapshot(root, source)?
    };
    if rollback_cas_pin_count != manifest.rollback_cas_pin_count {
        return Err(storage_corrupted(
            "database generation rollback CAS inventory changed after cutover",
        ));
    }
    Ok(())
}

fn cleanup_manifest_temps(root: &Path) -> CoreResult<()> {
    let database_dir = root.join("db");
    ensure_real_directory(&database_dir, "database directory")?;
    let mut removed = false;
    for entry in fs::read_dir(&database_dir).map_err(storage_io_error)? {
        let entry = entry.map_err(storage_io_error)?;
        let name = entry.file_name();
        let Some(name) = name.to_str() else {
            continue;
        };
        let Some(uuid) = name
            .strip_prefix(".active-database.")
            .and_then(|value| value.strip_suffix(".tmp"))
        else {
            continue;
        };
        let Ok(uuid) = Uuid::parse_str(uuid) else {
            continue;
        };
        if format!(".active-database.{uuid}.tmp") != name {
            continue;
        }
        let metadata = fs::symlink_metadata(entry.path()).map_err(storage_io_error)?;
        if !metadata.file_type().is_file() || metadata_is_reparse_point(&metadata) {
            return Err(storage_corrupted(
                "active database manifest temporary path is not a regular file",
            ));
        }
        fs::remove_file(entry.path()).map_err(storage_io_error)?;
        removed = true;
    }
    if removed {
        sync_directory(&database_dir)?;
    }
    Ok(())
}

fn read_committed_generations(root: &Path) -> CoreResult<Vec<ActiveDatabaseManifest>> {
    let cutover_dir = root.join(CUTOVER_DIRECTORY_RELATIVE_PATH);
    match fs::symlink_metadata(&cutover_dir) {
        Ok(metadata) if metadata.file_type().is_dir() && !metadata_is_reparse_point(&metadata) => {}
        Ok(_) => {
            return Err(storage_corrupted(
                "database cutover path is not a real directory",
            ));
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(storage_io_error(error)),
    }

    let mut generations = Vec::new();
    for entry in fs::read_dir(&cutover_dir).map_err(storage_io_error)? {
        let entry = entry.map_err(storage_io_error)?;
        let name = entry
            .file_name()
            .to_str()
            .ok_or_else(|| storage_corrupted("database cutover ID is not valid UTF-8"))?
            .to_owned();
        let cutover_id = Uuid::parse_str(&name)
            .map_err(|_| storage_corrupted("database cutover directory has an invalid ID"))?;
        if cutover_id.to_string() != name {
            return Err(storage_corrupted(
                "database cutover directory ID is not canonical",
            ));
        }
        ensure_real_directory(&entry.path(), "database cutover generation directory")?;
        let commit_path = entry.path().join(GENERATION_COMMIT_FILENAME);
        match fs::symlink_metadata(&commit_path) {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(error) => return Err(storage_io_error(error)),
            Ok(metadata)
                if metadata.file_type().is_file() && !metadata_is_reparse_point(&metadata) => {}
            Ok(_) => {
                return Err(storage_corrupted(
                    "database generation commit marker is not a regular file",
                ));
            }
        }
        let commit = read_generation_commit(&commit_path)?;
        if commit.cutover_id != name {
            return Err(storage_corrupted(
                "database generation commit marker names a different generation",
            ));
        }
        let manifest_path = entry.path().join(GENERATION_MANIFEST_FILENAME);
        let manifest_bytes = read_bounded_file(&manifest_path, "database generation manifest")?;
        if sha256_bytes(&manifest_bytes) != commit.manifest_sha256 {
            return Err(storage_corrupted(
                "database generation manifest does not match its commit marker",
            ));
        }
        let manifest =
            serde_json::from_slice::<ActiveDatabaseManifest>(&manifest_bytes).map_err(|error| {
                storage_corrupted(format!("database generation manifest is invalid: {error}"))
            })?;
        manifest.validate()?;
        if manifest.format_version != commit.format_version {
            return Err(storage_corrupted(
                "database generation manifest and commit formats do not match",
            ));
        }
        if manifest.cutover_id != name {
            return Err(storage_corrupted(
                "database generation manifest names a different directory",
            ));
        }
        generations.push(manifest);
    }
    generations.sort_by_key(|manifest| manifest.activation_sequence);
    for (index, manifest) in generations.iter().enumerate() {
        let expected_sequence = u64::try_from(index + 1)
            .map_err(|_| CoreError::internal("database generation sequence overflowed"))?;
        if manifest.activation_sequence != expected_sequence {
            return Err(storage_corrupted(
                "committed database generation sequence has a gap or duplicate",
            ));
        }
        let expected_parent = index
            .checked_sub(1)
            .map(|parent_index| generations[parent_index].cutover_id.as_str());
        if manifest.parent_cutover_id.as_deref() != expected_parent {
            return Err(storage_corrupted(
                "committed database generation parent chain is invalid",
            ));
        }
    }
    Ok(generations)
}

fn cleanup_uncommitted_candidates(
    root: &Path,
    committed: &[ActiveDatabaseManifest],
) -> CoreResult<()> {
    let cutover_dir = root.join(CUTOVER_DIRECTORY_RELATIVE_PATH);
    match fs::symlink_metadata(&cutover_dir) {
        Ok(metadata) if metadata.file_type().is_dir() && !metadata_is_reparse_point(&metadata) => {}
        Ok(_) => {
            return Err(storage_corrupted(
                "database cutover path is not a real directory",
            ));
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(storage_io_error(error)),
    }

    let mut removed = false;
    for entry in fs::read_dir(&cutover_dir).map_err(storage_io_error)? {
        let entry = entry.map_err(storage_io_error)?;
        let name = entry
            .file_name()
            .to_str()
            .ok_or_else(|| storage_corrupted("database cutover ID is not valid UTF-8"))?
            .to_owned();
        let cutover_id = Uuid::parse_str(&name)
            .map_err(|_| storage_corrupted("database cutover directory has an invalid ID"))?;
        if cutover_id.to_string() != name {
            return Err(storage_corrupted(
                "database cutover directory ID is not canonical",
            ));
        }
        ensure_real_directory(&entry.path(), "database cutover generation directory")?;
        if committed.iter().any(|manifest| manifest.cutover_id == name) {
            continue;
        }
        cleanup_candidate_directory(&entry.path())?;
        removed = true;
    }
    if removed {
        sync_directory(&cutover_dir)?;
    }
    Ok(())
}

fn cleanup_candidate_directory(candidate_dir: &Path) -> CoreResult<()> {
    ensure_real_directory(candidate_dir, "candidate database directory")?;
    let parent = candidate_dir
        .parent()
        .ok_or_else(|| storage_corrupted("candidate database directory has no parent"))?;
    for entry in fs::read_dir(candidate_dir).map_err(storage_io_error)? {
        let entry = entry.map_err(storage_io_error)?;
        let name = entry
            .file_name()
            .to_str()
            .ok_or_else(|| storage_corrupted("candidate database file name is not valid UTF-8"))?
            .to_owned();
        if !matches!(
            name.as_str(),
            "lorepia.sqlite3"
                | "lorepia.sqlite3-wal"
                | "lorepia.sqlite3-shm"
                | "lorepia.sqlite3-journal"
                | GENERATION_MANIFEST_FILENAME
                | GENERATION_COMMIT_FILENAME
        ) {
            let is_owned_temp = (name.starts_with(".generation-manifest.")
                || name.starts_with(".generation-commit."))
                && name.as_bytes().ends_with(b".tmp");
            if is_owned_temp {
                let metadata = fs::symlink_metadata(entry.path()).map_err(storage_io_error)?;
                if !metadata.file_type().is_file() || metadata_is_reparse_point(&metadata) {
                    return Err(storage_corrupted(
                        "candidate database temporary entry is not a regular file",
                    ));
                }
                fs::remove_file(entry.path()).map_err(storage_io_error)?;
                continue;
            }
            return Err(storage_corrupted(format!(
                "candidate database directory contains an unexpected entry: {name}"
            )));
        }
        let metadata = fs::symlink_metadata(entry.path()).map_err(storage_io_error)?;
        if !metadata.file_type().is_file() || metadata_is_reparse_point(&metadata) {
            return Err(storage_corrupted(
                "candidate database entry is not a regular file",
            ));
        }
        fs::remove_file(entry.path()).map_err(storage_io_error)?;
    }
    fs::remove_dir(candidate_dir).map_err(storage_io_error)?;
    sync_directory(parent)
}

fn publish_committed_generation(root: &Path, manifest: &ActiveDatabaseManifest) -> CoreResult<()> {
    let generation_dir = root
        .join(CUTOVER_DIRECTORY_RELATIVE_PATH)
        .join(&manifest.cutover_id);
    ensure_real_directory(&generation_dir, "database cutover generation directory")?;
    let manifest_path = generation_dir.join(GENERATION_MANIFEST_FILENAME);
    let bytes = serde_json::to_vec_pretty(manifest)
        .map_err(|error| CoreError::internal(format!("cannot encode cutover manifest: {error}")))?;
    let manifest_bytes = write_immutable_json(
        &generation_dir,
        ".generation-manifest",
        &manifest_path,
        &bytes,
    )?;
    sync_directory(&generation_dir)?;
    cutover_failpoint("after_generation_manifest");

    let commit = GenerationCommit::new(&manifest.cutover_id, &sha256_bytes(&manifest_bytes));
    let commit_bytes = serde_json::to_vec_pretty(&commit).map_err(|error| {
        CoreError::internal(format!("cannot encode generation commit marker: {error}"))
    })?;
    write_immutable_json(
        &generation_dir,
        ".generation-commit",
        &generation_dir.join(GENERATION_COMMIT_FILENAME),
        &commit_bytes,
    )?;
    sync_directory(&generation_dir)?;
    sync_directory(
        generation_dir
            .parent()
            .ok_or_else(|| storage_corrupted("database generation has no parent directory"))?,
    )?;
    cutover_failpoint("after_generation_commit");
    Ok(())
}

fn write_immutable_json(
    directory: &Path,
    temp_prefix: &str,
    final_path: &Path,
    bytes: &[u8],
) -> CoreResult<Vec<u8>> {
    if final_path.try_exists().map_err(storage_io_error)? {
        return Err(storage_corrupted(
            "immutable database generation metadata already exists",
        ));
    }
    let temp_path = directory.join(format!("{temp_prefix}.{}.tmp", Uuid::new_v4()));
    let mut published_bytes = Vec::with_capacity(bytes.len().saturating_add(1));
    published_bytes.extend_from_slice(bytes);
    published_bytes.push(b'\n');
    let mut writer = OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(&temp_path)
        .map_err(storage_io_error)?;
    writer
        .write_all(&published_bytes)
        .map_err(storage_io_error)?;
    writer.flush().map_err(storage_io_error)?;
    writer.sync_all().map_err(storage_io_error)?;
    drop(writer);
    if let Err(error) = publish_noclobber(&temp_path, final_path) {
        let _ = fs::remove_file(&temp_path);
        return Err(storage_io_error(error));
    }
    Ok(published_bytes)
}

fn read_generation_commit(path: &Path) -> CoreResult<GenerationCommit> {
    let bytes = read_bounded_file(path, "database generation commit marker")?;
    let commit = serde_json::from_slice::<GenerationCommit>(&bytes).map_err(|error| {
        storage_corrupted(format!(
            "database generation commit marker is invalid: {error}"
        ))
    })?;
    commit.validate()?;
    Ok(commit)
}

fn read_bounded_file(path: &Path, label: &str) -> CoreResult<Vec<u8>> {
    ensure_regular_file(path, label)?;
    let metadata = fs::metadata(path).map_err(storage_io_error)?;
    if metadata.len() > MAX_MANIFEST_BYTES {
        return Err(storage_corrupted(format!("{label} is too large")));
    }
    let mut bytes = Vec::with_capacity(usize::try_from(metadata.len()).unwrap_or(0));
    File::open(path)
        .and_then(|mut file| file.read_to_end(&mut bytes))
        .map_err(storage_io_error)?;
    Ok(bytes)
}

fn sha256_bytes(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

fn validate_owned_relative_path(path: &Path) -> CoreResult<()> {
    if path.is_absolute()
        || path
            .components()
            .any(|component| !matches!(component, Component::Normal(_)))
    {
        return Err(storage_corrupted(
            "active database manifest contains an unsafe path",
        ));
    }
    Ok(())
}

impl ActiveDatabaseManifest {
    fn new(
        cutover_id: &str,
        parent: Option<&Self>,
        source_database_size_bytes: u64,
        source_database_fingerprint_sha256: &str,
        rollback_cas_pin_count: u64,
        baseline_schema_version: u32,
    ) -> CoreResult<Self> {
        let active_database_relative_path =
            format!("{CUTOVER_DIRECTORY_RELATIVE_PATH}/{cutover_id}/lorepia.sqlite3");
        let source_database_relative_path = parent.map_or_else(
            || LEGACY_DATABASE_RELATIVE_PATH.to_owned(),
            |manifest| manifest.active_database_relative_path.clone(),
        );
        let activation_sequence = match parent {
            Some(manifest) => manifest
                .activation_sequence
                .checked_add(1)
                .ok_or_else(|| CoreError::internal("database generation sequence overflowed"))?,
            None => 1,
        };
        let mut manifest = Self {
            format_version: ACTIVE_MANIFEST_FORMAT_VERSION,
            activation_sequence,
            cutover_id: cutover_id.to_owned(),
            parent_cutover_id: parent.map(|manifest| manifest.cutover_id.clone()),
            active_database_relative_path,
            active_schema_version: SCHEMA_VERSION,
            source_database_relative_path,
            baseline_schema_version,
            source_database_size_bytes,
            source_database_fingerprint_sha256: source_database_fingerprint_sha256.to_owned(),
            rollback_cas_pin_count,
            checksum_sha256: String::new(),
        };
        manifest.checksum_sha256 = manifest.expected_checksum();
        Ok(manifest)
    }

    fn validate(&self) -> CoreResult<()> {
        if !(MIN_SUPPORTED_MANIFEST_FORMAT_VERSION..=ACTIVE_MANIFEST_FORMAT_VERSION)
            .contains(&self.format_version)
            || self.activation_sequence == 0
            || self.baseline_schema_version > self.active_schema_version
        {
            return Err(storage_corrupted(
                "active database manifest metadata is unsupported",
            ));
        }
        let cutover_id = Uuid::parse_str(&self.cutover_id)
            .map_err(|_| storage_corrupted("active database manifest cutover ID is invalid"))?;
        if cutover_id.to_string() != self.cutover_id {
            return Err(storage_corrupted(
                "active database manifest cutover ID is not canonical",
            ));
        }
        match (&self.parent_cutover_id, self.activation_sequence) {
            (None, 1) => {}
            (Some(parent), sequence) if sequence > 1 => {
                let parent_id = Uuid::parse_str(parent).map_err(|_| {
                    storage_corrupted("active database manifest parent ID is invalid")
                })?;
                if parent_id.to_string() != *parent || parent == &self.cutover_id {
                    return Err(storage_corrupted(
                        "active database manifest parent ID is not canonical",
                    ));
                }
            }
            _ => {
                return Err(storage_corrupted(
                    "active database manifest sequence and parent do not agree",
                ));
            }
        }
        let expected_path = format!(
            "{CUTOVER_DIRECTORY_RELATIVE_PATH}/{}/lorepia.sqlite3",
            self.cutover_id
        );
        if self.active_database_relative_path != expected_path {
            return Err(storage_corrupted(
                "active database manifest candidate path is invalid",
            ));
        }
        validate_owned_relative_path(Path::new(&self.source_database_relative_path))?;
        if self.source_database_relative_path == self.active_database_relative_path {
            return Err(storage_corrupted(
                "active database manifest source and candidate paths are identical",
            ));
        }
        if self.source_database_fingerprint_sha256.len() != 64
            || self
                .source_database_fingerprint_sha256
                .bytes()
                .any(|byte| !byte.is_ascii_hexdigit() || byte.is_ascii_uppercase())
        {
            return Err(storage_corrupted(
                "active database manifest source fingerprint is invalid",
            ));
        }
        if self.checksum_sha256 != self.expected_checksum() {
            return Err(storage_corrupted(
                "active database manifest checksum does not match",
            ));
        }
        Ok(())
    }

    fn active_database_path(&self, root: &Path) -> CoreResult<PathBuf> {
        let relative = Path::new(&self.active_database_relative_path);
        validate_owned_relative_path(relative)?;
        let database_dir = root.join("db");
        let cutover_dir = root.join(CUTOVER_DIRECTORY_RELATIVE_PATH);
        let generation_dir = cutover_dir.join(&self.cutover_id);
        ensure_real_directory(&database_dir, "database directory")?;
        ensure_real_directory(&cutover_dir, "database cutover directory")?;
        ensure_real_directory(&generation_dir, "database generation directory")?;
        let active_path = root.join(relative);
        ensure_regular_file(&active_path, "active candidate database")?;
        Ok(active_path)
    }

    fn expected_checksum(&self) -> String {
        let payload = ManifestChecksumPayload {
            domain: if self.format_version == MIN_SUPPORTED_MANIFEST_FORMAT_VERSION {
                MANIFEST_CHECKSUM_DOMAIN_V2
            } else {
                MANIFEST_CHECKSUM_DOMAIN
            },
            format_version: self.format_version,
            activation_sequence: self.activation_sequence,
            cutover_id: &self.cutover_id,
            parent_cutover_id: self.parent_cutover_id.as_deref(),
            active_database_relative_path: &self.active_database_relative_path,
            active_schema_version: self.active_schema_version,
            source_database_relative_path: &self.source_database_relative_path,
            baseline_schema_version: self.baseline_schema_version,
            source_database_size_bytes: self.source_database_size_bytes,
            source_database_fingerprint_sha256: &self.source_database_fingerprint_sha256,
            rollback_cas_pin_count: self.rollback_cas_pin_count,
        };
        let bytes = serde_json::to_vec(&payload).expect("manifest checksum payload serializes");
        format!("{:x}", Sha256::digest(bytes))
    }
}

impl RollbackCasPin {
    fn validate(&self) -> CoreResult<()> {
        if !matches!(self.namespace.as_str(), "sources" | "assets")
            || self.sha256.len() != 64
            || self
                .sha256
                .bytes()
                .any(|byte| !byte.is_ascii_hexdigit() || byte.is_ascii_uppercase())
        {
            return Err(storage_corrupted(
                "database cutover rollback CAS pin is invalid",
            ));
        }
        let expected_path = format!(
            "{}/sha256/{}/{}",
            self.namespace,
            &self.sha256[..2],
            &self.sha256[2..]
        );
        if self.relative_path != expected_path {
            return Err(storage_corrupted(
                "database cutover rollback CAS pin path is invalid",
            ));
        }
        validate_owned_relative_path(Path::new(&self.relative_path))
    }
}

impl GenerationCommit {
    fn new(cutover_id: &str, manifest_sha256: &str) -> Self {
        let mut commit = Self {
            format_version: ACTIVE_MANIFEST_FORMAT_VERSION,
            cutover_id: cutover_id.to_owned(),
            manifest_sha256: manifest_sha256.to_owned(),
            checksum_sha256: String::new(),
        };
        commit.checksum_sha256 = commit.expected_checksum();
        commit
    }

    fn validate(&self) -> CoreResult<()> {
        let cutover_id = Uuid::parse_str(&self.cutover_id)
            .map_err(|_| storage_corrupted("database generation commit ID is invalid"))?;
        if !(MIN_SUPPORTED_MANIFEST_FORMAT_VERSION..=ACTIVE_MANIFEST_FORMAT_VERSION)
            .contains(&self.format_version)
            || cutover_id.to_string() != self.cutover_id
            || !valid_sha256(&self.manifest_sha256)
            || self.checksum_sha256 != self.expected_checksum()
        {
            return Err(storage_corrupted(
                "database generation commit marker is invalid",
            ));
        }
        Ok(())
    }

    fn expected_checksum(&self) -> String {
        let payload = GenerationCommitChecksumPayload {
            domain: GENERATION_COMMIT_CHECKSUM_DOMAIN,
            format_version: self.format_version,
            cutover_id: &self.cutover_id,
            manifest_sha256: &self.manifest_sha256,
        };
        let bytes = serde_json::to_vec(&payload).expect("generation commit payload serializes");
        sha256_bytes(&bytes)
    }
}

fn valid_sha256(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
}

#[cfg(debug_assertions)]
fn cutover_failpoint(name: &str) {
    if std::env::var("LOREPIA_TEST_CUTOVER_FAILPOINT")
        .ok()
        .as_deref()
        == Some(name)
    {
        std::process::exit(86);
    }
}

#[cfg(not(debug_assertions))]
fn cutover_failpoint(_name: &str) {}

#[cfg(debug_assertions)]
fn cutover_pausepoint(name: &str) {
    if std::env::var("LOREPIA_TEST_CUTOVER_PAUSEPOINT")
        .ok()
        .as_deref()
        != Some(name)
    {
        return;
    }
    let ready = std::env::var_os("LOREPIA_TEST_CUTOVER_PAUSE_READY")
        .map(PathBuf::from)
        .expect("cutover pausepoint ready path");
    let release = std::env::var_os("LOREPIA_TEST_CUTOVER_PAUSE_RELEASE")
        .map(PathBuf::from)
        .expect("cutover pausepoint release path");
    fs::write(&ready, b"ready").expect("publish cutover pausepoint readiness");
    let deadline = std::time::Instant::now() + Duration::from_secs(30);
    while !release.exists() && std::time::Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(release.exists(), "cutover pausepoint was not released");
}

#[cfg(not(debug_assertions))]
fn cutover_pausepoint(_name: &str) {}

fn ensure_regular_file(path: &Path, label: &str) -> CoreResult<()> {
    let metadata = fs::symlink_metadata(path).map_err(storage_io_error)?;
    if !metadata.file_type().is_file() || metadata_is_reparse_point(&metadata) {
        return Err(storage_corrupted(format!(
            "{label} is not a regular file: {}",
            path.display()
        )));
    }
    Ok(())
}

fn ensure_real_directory(path: &Path, label: &str) -> CoreResult<()> {
    let metadata = fs::symlink_metadata(path).map_err(storage_io_error)?;
    if !metadata.file_type().is_dir() || metadata_is_reparse_point(&metadata) {
        return Err(storage_corrupted(format!(
            "{label} is not a real directory: {}",
            path.display()
        )));
    }
    Ok(())
}

#[cfg(windows)]
fn metadata_is_reparse_point(metadata: &fs::Metadata) -> bool {
    use std::os::windows::fs::MetadataExt;

    const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x0000_0400;
    metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0
}

#[cfg(not(windows))]
fn metadata_is_reparse_point(_metadata: &fs::Metadata) -> bool {
    false
}

#[cfg(any(target_os = "android", target_os = "linux", target_vendor = "apple"))]
fn publish_noclobber(temp_path: &Path, final_path: &Path) -> std::io::Result<()> {
    use rustix::fs::{CWD, RenameFlags, renameat_with};

    renameat_with(CWD, temp_path, CWD, final_path, RenameFlags::NOREPLACE)
        .map_err(|error| std::io::Error::from_raw_os_error(error.raw_os_error()))
}

#[cfg(windows)]
const WINDOWS_MOVE_FILE_FLAGS: u32 =
    windows_sys::Win32::Storage::FileSystem::MOVEFILE_WRITE_THROUGH;

#[cfg(windows)]
const WINDOWS_DIRECTORY_OPEN_FLAGS: u32 =
    windows_sys::Win32::Storage::FileSystem::FILE_FLAG_BACKUP_SEMANTICS
        | windows_sys::Win32::Storage::FileSystem::FILE_FLAG_OPEN_REPARSE_POINT
        | windows_sys::Win32::Storage::FileSystem::FILE_FLAG_WRITE_THROUGH;

#[cfg(windows)]
fn null_terminated_windows_path(path: &Path) -> std::io::Result<Vec<u16>> {
    use std::os::windows::ffi::OsStrExt;

    let mut encoded = path.as_os_str().encode_wide().collect::<Vec<_>>();
    if encoded.contains(&0) {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "database cutover path contains an embedded NUL",
        ));
    }
    encoded.push(0);
    Ok(encoded)
}

#[cfg(windows)]
#[allow(unsafe_code)]
fn publish_noclobber(temp_path: &Path, final_path: &Path) -> std::io::Result<()> {
    use windows_sys::Win32::Storage::FileSystem::MoveFileExW;

    let temp_path = null_terminated_windows_path(temp_path)?;
    let final_path = null_terminated_windows_path(final_path)?;
    // SAFETY: both buffers are immutable, NUL-terminated UTF-16 paths and remain
    // alive for the complete call. Omitting MOVEFILE_REPLACE_EXISTING preserves
    // immutable publication when another process wins the destination race.
    let moved = unsafe {
        MoveFileExW(
            temp_path.as_ptr(),
            final_path.as_ptr(),
            WINDOWS_MOVE_FILE_FLAGS,
        )
    };
    if moved == 0 {
        Err(std::io::Error::last_os_error())
    } else {
        Ok(())
    }
}

#[cfg(not(any(
    target_os = "android",
    target_os = "linux",
    target_vendor = "apple",
    windows
)))]
fn publish_noclobber(temp_path: &Path, final_path: &Path) -> std::io::Result<()> {
    if final_path.try_exists()? {
        return Err(std::io::Error::new(
            std::io::ErrorKind::AlreadyExists,
            "immutable database generation metadata already exists",
        ));
    }
    fs::rename(temp_path, final_path)
}

#[cfg(not(windows))]
fn sync_file(path: &Path) -> std::io::Result<()> {
    File::open(path).and_then(|file| file.sync_all())
}

#[cfg(windows)]
fn sync_file(path: &Path) -> std::io::Result<()> {
    OpenOptions::new()
        .write(true)
        .open(path)
        .and_then(|file| file.sync_all())
}

#[cfg(unix)]
fn sync_directory(path: &Path) -> CoreResult<()> {
    File::open(path)
        .and_then(|directory| directory.sync_all())
        .map_err(storage_io_error)
}

#[cfg(windows)]
fn sync_directory(path: &Path) -> CoreResult<()> {
    use std::os::windows::fs::{MetadataExt, OpenOptionsExt};

    let directory = OpenOptions::new()
        .read(true)
        .write(true)
        .custom_flags(WINDOWS_DIRECTORY_OPEN_FLAGS)
        .open(path)
        .map_err(storage_io_error)?;
    let metadata = directory.metadata().map_err(storage_io_error)?;
    if metadata.file_attributes()
        & windows_sys::Win32::Storage::FileSystem::FILE_ATTRIBUTE_REPARSE_POINT
        != 0
    {
        return Err(storage_corrupted(
            "database cutover directory handle resolved to a reparse point",
        ));
    }
    directory.sync_all().map_err(storage_io_error)
}

#[cfg(not(any(unix, windows)))]
fn sync_directory(path: &Path) -> CoreResult<()> {
    Err(storage_corrupted(format!(
        "database cutover cannot durably sync directories on this platform: {}",
        path.display()
    )))
}

fn storage_io_error(error: std::io::Error) -> CoreError {
    CoreError::new(
        CoreErrorCode::StorageUnavailable,
        format!("local storage cutover failed: {error}"),
        true,
    )
}

fn storage_corrupted(message: impl Into<String>) -> CoreError {
    CoreError::new(CoreErrorCode::StorageCorrupted, message, false)
}

#[cfg(test)]
mod tests {
    use super::*;

    use tempfile::tempdir;

    const RECOVERY_COMPATIBILITY_VECTORS: &str =
        include_str!("../../../testdata/tauri-upgrade/recovery-compatibility-v1-vectors.json");

    #[test]
    fn recovery_compatibility_v1_known_vectors() {
        let vectors: serde_json::Value = serde_json::from_str(RECOVERY_COMPATIBILITY_VECTORS)
            .expect("recovery compatibility vectors must be JSON");
        let cutover = &vectors["cutover_manifest"];
        assert_eq!(
            cutover["manifest_filename"].as_str(),
            Some(GENERATION_MANIFEST_FILENAME)
        );
        assert_eq!(
            cutover["commit_filename"].as_str(),
            Some(GENERATION_COMMIT_FILENAME)
        );

        let manifests = cutover["manifests"]
            .as_array()
            .expect("manifest vectors must be an array");
        assert_eq!(manifests.len(), 2);
        for value in manifests {
            let manifest: ActiveDatabaseManifest =
                serde_json::from_value(value.clone()).expect("known manifest vector must decode");
            assert_eq!(manifest.expected_checksum(), manifest.checksum_sha256);
            manifest
                .validate()
                .expect("known manifest vector must remain readable");
        }

        let commits = cutover["commits"]
            .as_array()
            .expect("commit vectors must be an array");
        assert_eq!(commits.len(), 2);
        for value in commits {
            let commit: GenerationCommit =
                serde_json::from_value(value.clone()).expect("known commit vector must decode");
            assert_eq!(commit.expected_checksum(), commit.checksum_sha256);
            commit
                .validate()
                .expect("known commit vector must remain readable");
        }
    }

    #[cfg(windows)]
    #[test]
    fn windows_publication_uses_write_through_and_directory_durability_flags() {
        assert_eq!(WINDOWS_MOVE_FILE_FLAGS, 0x0000_0008);
        assert_eq!(WINDOWS_DIRECTORY_OPEN_FLAGS, 0x8220_0000);
    }

    #[cfg(windows)]
    #[test]
    fn windows_publication_is_noclobber_and_directory_syncable() {
        let root = tempdir().expect("temporary Windows publication root");
        let source = root.path().join("source.tmp");
        let destination = root.path().join("committed.json");
        fs::write(&source, b"new").expect("write publication source");
        fs::write(&destination, b"existing").expect("write existing destination");

        let error = publish_noclobber(&source, &destination)
            .expect_err("Windows publication must not replace committed metadata");
        assert_eq!(error.kind(), std::io::ErrorKind::AlreadyExists);
        assert_eq!(fs::read(&source).expect("read retained source"), b"new");
        assert_eq!(
            fs::read(&destination).expect("read committed destination"),
            b"existing"
        );

        fs::remove_file(&destination).expect("remove test destination");
        publish_noclobber(&source, &destination).expect("publish committed metadata");
        assert!(!source.exists());
        assert_eq!(
            fs::read(&destination).expect("read published destination"),
            b"new"
        );
        sync_directory(root.path()).expect("sync Windows publication directory");
    }

    #[test]
    fn ordered_streaming_digest_is_insertion_order_and_duplicate_safe() {
        let first = Connection::open_in_memory().expect("first streaming digest database");
        let second = Connection::open_in_memory().expect("second streaming digest database");
        for connection in [&first, &second] {
            connection
                .execute_batch("CREATE TABLE sample (value, text_value, blob_value);")
                .expect("create digest table");
        }
        first
            .execute_batch(
                "INSERT INTO sample VALUES (1, 'alpha', X'00ff');
                 INSERT INTO sample VALUES (1.0, 'alpha', X'00ff');
                 INSERT INTO sample VALUES (NULL, 'beta', X'');
                 INSERT INTO sample VALUES (1, 'alpha', X'00ff');",
            )
            .expect("insert first row order");
        second
            .execute_batch(
                "INSERT INTO sample VALUES (1, 'alpha', X'00ff');
                 INSERT INTO sample VALUES (NULL, 'beta', X'');
                 INSERT INTO sample VALUES (1, 'alpha', X'00ff');
                 INSERT INTO sample VALUES (1.0, 'alpha', X'00ff');",
            )
            .expect("insert second row order");
        let columns = table_columns(&first, "sample").expect("read digest columns");
        assert_eq!(
            table_rows(&first, "sample", &columns).expect("digest first table"),
            table_rows(&second, "sample", &columns).expect("digest second table")
        );
    }

    #[test]
    fn fresh_install_generation_is_copied_forward_on_the_next_schema() {
        let root = tempdir().expect("temporary generation-upgrade root");
        let root_path = fs::canonicalize(root.path()).expect("canonical temporary root");
        let (canonical_path, previous_schema) = simulated_previous_release(&root_path);
        let previous_path =
            publish_simulated_generation(&root_path, &canonical_path, previous_schema);
        let (source_sha256, asset_sha256) =
            seed_previous_generation_cas(&root_path, &previous_path);
        let canonical_sha256 = sha256_file(&canonical_path).expect("hash canonical database");
        let previous_sha256 = sha256_file(&previous_path).expect("hash previous generation");

        let current = open_database(&root_path, &canonical_path)
            .expect("copy previous generation forward to current schema");
        assert_eq!(
            read_current_schema_version(&current).expect("read current generation schema"),
            SCHEMA_VERSION
        );
        drop(current);

        let generations =
            read_committed_generations(&root_path).expect("read committed generations");
        assert_eq!(generations.len(), 2);
        assert_eq!(generations[1].activation_sequence, 2);
        assert_eq!(
            generations[1].parent_cutover_id.as_deref(),
            Some(generations[0].cutover_id.as_str())
        );
        assert_eq!(
            generations[1].source_database_relative_path,
            generations[0].active_database_relative_path,
            "the child generation must bind the previous active database it sealed"
        );
        assert!(
            is_rollback_cas_pinned(&root_path, "sources", &source_sha256)
                .expect("lookup previous-generation source pin")
        );
        assert!(
            is_rollback_cas_pinned(&root_path, "assets", &asset_sha256)
                .expect("lookup previous-generation asset pin")
        );
        assert_eq!(
            sha256_file(&previous_path).expect("rehash previous generation"),
            previous_sha256,
            "a future schema upgrade must preserve its previous committed generation"
        );
        assert_eq!(
            sha256_file(&canonical_path).expect("rehash canonical database"),
            canonical_sha256,
            "a future schema upgrade must not migrate its canonical lineage anchor"
        );

        let previous = Connection::open(&previous_path).expect("reopen sealed parent generation");
        previous
            .execute(
                "UPDATE content_sources SET created_at = 'tampered' WHERE sha256 = ?1",
                [&source_sha256],
            )
            .expect("tamper sealed parent generation");
        previous
            .query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |_| Ok(()))
            .expect("checkpoint sealed-parent tamper");
        drop(previous);
        let Err(error) = open_database(&root_path, &canonical_path) else {
            panic!("a sealed parent generation must fail closed after drift");
        };
        assert_eq!(error.code, CoreErrorCode::StorageCorrupted);
        assert!(error.message.contains("generation source diverged"));
    }

    #[test]
    fn committed_generation_rejects_a_self_consistent_wrong_source_size_binding() {
        let root = tempdir().expect("temporary source-size binding root");
        let root_path = fs::canonicalize(root.path()).expect("canonical temporary root");
        let canonical_path = root_path.join(LEGACY_DATABASE_RELATIVE_PATH);
        fs::create_dir_all(canonical_path.parent().expect("database parent"))
            .expect("create database directory");
        drop(open_database(&root_path, &canonical_path).expect("publish fresh generation"));

        let mut manifest = read_committed_generations(&root_path)
            .expect("read generation")
            .pop()
            .expect("committed generation");
        manifest.source_database_size_bytes = manifest
            .source_database_size_bytes
            .checked_add(1)
            .expect("source size increment");
        manifest.checksum_sha256 = manifest.expected_checksum();
        let generation = root_path
            .join(CUTOVER_DIRECTORY_RELATIVE_PATH)
            .join(&manifest.cutover_id);
        let mut manifest_bytes = serde_json::to_vec_pretty(&manifest).expect("encode manifest");
        manifest_bytes.push(b'\n');
        fs::write(
            generation.join(GENERATION_MANIFEST_FILENAME),
            &manifest_bytes,
        )
        .expect("replace test manifest");
        let commit = GenerationCommit::new(&manifest.cutover_id, &sha256_bytes(&manifest_bytes));
        let mut commit_bytes = serde_json::to_vec_pretty(&commit).expect("encode commit");
        commit_bytes.push(b'\n');
        fs::write(generation.join(GENERATION_COMMIT_FILENAME), commit_bytes)
            .expect("replace test commit");

        let Err(error) = open_database(&root_path, &canonical_path) else {
            panic!("wrong source-size binding must fail closed");
        };
        assert_eq!(error.code, CoreErrorCode::StorageCorrupted);
        assert!(error.message.contains("source size"));
    }

    #[test]
    fn format_two_generation_manifest_remains_read_compatible() {
        let root = tempdir().expect("temporary format-two compatibility root");
        let root_path = fs::canonicalize(root.path()).expect("canonical temporary root");
        let canonical_path = root_path.join(LEGACY_DATABASE_RELATIVE_PATH);
        fs::create_dir_all(canonical_path.parent().expect("database parent"))
            .expect("create database directory");
        drop(open_database(&root_path, &canonical_path).expect("publish fresh generation"));

        let mut manifest = read_committed_generations(&root_path)
            .expect("read generation")
            .pop()
            .expect("committed generation");
        manifest.format_version = MIN_SUPPORTED_MANIFEST_FORMAT_VERSION;
        manifest.checksum_sha256 = manifest.expected_checksum();
        let generation = root_path
            .join(CUTOVER_DIRECTORY_RELATIVE_PATH)
            .join(&manifest.cutover_id);
        let mut manifest_bytes = serde_json::to_vec_pretty(&manifest).expect("encode v2 manifest");
        manifest_bytes.push(b'\n');
        fs::write(
            generation.join(GENERATION_MANIFEST_FILENAME),
            &manifest_bytes,
        )
        .expect("replace test manifest");
        let mut commit =
            GenerationCommit::new(&manifest.cutover_id, &sha256_bytes(&manifest_bytes));
        commit.format_version = MIN_SUPPORTED_MANIFEST_FORMAT_VERSION;
        commit.checksum_sha256 = commit.expected_checksum();
        let mut commit_bytes = serde_json::to_vec_pretty(&commit).expect("encode v2 commit");
        commit_bytes.push(b'\n');
        fs::write(generation.join(GENERATION_COMMIT_FILENAME), commit_bytes)
            .expect("replace test commit");

        drop(open_database(&root_path, &canonical_path).expect("open compatible v2 generation"));
    }

    fn simulated_previous_release(root: &Path) -> (PathBuf, u32) {
        assert_eq!(
            SCHEMA_VERSION, 39,
            "update previous-release fixture for the latest migration"
        );
        let canonical_path = root.join(LEGACY_DATABASE_RELATIVE_PATH);
        fs::create_dir_all(canonical_path.parent().expect("database parent"))
            .expect("create database directory");
        drop(
            open_configured_and_migrate(&canonical_path)
                .expect("initialize a fresh current database"),
        );
        let previous = Connection::open(&canonical_path).expect("open previous-release fixture");
        reverse_latest_additive_migration(&previous);
        drop(previous);
        let previous = Connection::open(&canonical_path).expect("reopen previous-release fixture");
        let previous_schema = read_pre_migration_schema_version(&previous)
            .expect("read simulated previous-release schema");
        drop(previous);
        assert_eq!(previous_schema, SCHEMA_VERSION - 1);
        (canonical_path, previous_schema)
    }

    fn reverse_latest_additive_migration(connection: &Connection) {
        const LATEST_ADDITIVE_MIGRATION: &str =
            include_str!("../migrations/0039_runtime_model_audit.sql");

        let replaced_objects = LATEST_ADDITIVE_MIGRATION
            .lines()
            .filter_map(|line| {
                let mut tokens = line.split_ascii_whitespace();
                if tokens.next() != Some("DROP") {
                    return None;
                }
                let object_type = tokens.next()?;
                let name = tokens.next()?.trim_end_matches(';');
                Some((object_type, name))
            })
            .collect::<Vec<_>>();
        let created_objects = LATEST_ADDITIVE_MIGRATION
            .lines()
            .filter_map(|line| {
                let mut tokens = line.split_ascii_whitespace();
                if tokens.next() != Some("CREATE") {
                    return None;
                }
                let object_type = tokens.next()?;
                let (object_type, name) = if object_type == "UNIQUE" {
                    (tokens.next()?, tokens.next()?)
                } else {
                    (object_type, tokens.next()?)
                };
                let name = name.trim_end_matches(';');
                (!replaced_objects.contains(&(object_type, name))).then_some((object_type, name))
            })
            .collect::<Vec<_>>();

        assert!(
            created_objects.contains(&("TABLE", "portable_runtime_model_audit")),
            "the simulated downgrade must track every object in the latest additive migration"
        );
        connection
            .execute_batch("PRAGMA foreign_keys = OFF;")
            .expect("disable foreign keys for the simulated downgrade");
        for object_type in ["VIEW", "TRIGGER", "INDEX", "TABLE"] {
            for (_, name) in created_objects
                .iter()
                .rev()
                .filter(|(candidate_type, _)| *candidate_type == object_type)
            {
                connection
                    .execute(&format!("DROP {object_type} \"{name}\""), [])
                    .unwrap_or_else(|error| panic!("drop schema-39 {object_type} {name}: {error}"));
            }
        }
        connection
            .execute("DELETE FROM schema_migrations WHERE version = 39", [])
            .expect("remove the simulated latest migration registry row");
        connection
            .execute_batch("PRAGMA foreign_keys = ON; PRAGMA wal_checkpoint(TRUNCATE);")
            .expect("finish the simulated previous-release database");
    }

    fn publish_simulated_generation(
        root: &Path,
        canonical_path: &Path,
        previous_schema: u32,
    ) -> PathBuf {
        let source = Connection::open_with_flags(
            canonical_path,
            OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )
        .expect("open canonical source");
        validate_database_integrity(&source).expect("validate previous-release source");
        let tables = capture_table_snapshots(&source, true).expect("snapshot canonical source");
        let fingerprint = database_fingerprint_from_snapshots(&source, &tables)
            .expect("fingerprint canonical source");
        let pin_count =
            validate_rollback_cas_snapshot(root, &source).expect("validate rollback CAS snapshot");
        let (cutover_id, previous_path) =
            create_candidate_path(root).expect("create previous generation");
        backup_database(&source, &previous_path).expect("backup previous generation");
        let mut manifest = ActiveDatabaseManifest::new(
            &cutover_id,
            None,
            fs::metadata(canonical_path)
                .expect("canonical metadata")
                .len(),
            &fingerprint,
            pin_count,
            previous_schema,
        )
        .expect("build previous generation manifest");
        manifest.active_schema_version = previous_schema;
        manifest.checksum_sha256 = manifest.expected_checksum();
        publish_committed_generation(root, &manifest).expect("publish previous generation");
        previous_path
    }

    fn seed_previous_generation_cas(root: &Path, previous_path: &Path) -> (String, String) {
        let source_bytes = b"previous-generation-source";
        let source_sha256 = sha256_bytes(source_bytes);
        let source_relative_path = write_test_cas(root, "sources", &source_sha256, source_bytes);
        let asset_bytes = b"previous-generation-avatar";
        let asset_sha256 = sha256_bytes(asset_bytes);
        let asset_relative_path = write_test_cas(root, "assets", &asset_sha256, asset_bytes);
        let previous = Connection::open(previous_path).expect("open previous active generation");
        previous
            .execute(
                "INSERT INTO content_sources(sha256, relative_path, size_bytes, created_at)
                 VALUES (?1, ?2, ?3, 'previous-release')",
                (
                    &source_sha256,
                    source_relative_path,
                    i64::try_from(source_bytes.len()).expect("source size"),
                ),
            )
            .expect("insert previous-generation source pin");
        previous
            .execute(
                "INSERT INTO assets(sha256, relative_path, media_type, size_bytes, created_at)
                 VALUES (?1, ?2, 'image/png', ?3, 'previous-release')",
                (
                    &asset_sha256,
                    asset_relative_path,
                    i64::try_from(asset_bytes.len()).expect("asset size"),
                ),
            )
            .expect("insert previous-generation asset pin");
        previous
            .query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |_| Ok(()))
            .expect("checkpoint previous-generation writes");
        (source_sha256, asset_sha256)
    }

    fn write_test_cas(root: &Path, namespace: &str, sha256: &str, bytes: &[u8]) -> String {
        let relative_path = format!("{namespace}/sha256/{}/{}", &sha256[..2], &sha256[2..]);
        let path = root.join(&relative_path);
        fs::create_dir_all(path.parent().expect("rollback CAS parent"))
            .expect("create rollback CAS parent");
        fs::write(path, bytes).expect("write rollback CAS object");
        relative_path
    }
}
