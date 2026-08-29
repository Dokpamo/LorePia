//! End-to-end invariants for the complete current migration chain.

use std::{
    fs,
    path::{Path, PathBuf},
};

use lorepia_domain::CoreErrorCode;
use lorepia_storage::Storage;
use rusqlite::{Connection, OptionalExtension, params};
use tempfile::tempdir;

const NOW: &str = "2026-08-03T00:00:00Z";
const HASH_A: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const HASH_B: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
const LEGACY_CHAT_SOURCE_BYTES: &[u8] = b"synthetic legacy source bytes";
const LEGACY_CHAT_SOURCE_SHA256: &str =
    "32cfecba8b4ae0eb1e4e6ee98580532df17c27e26f5901c100f538c83d8cf502";

const MIGRATIONS: &[&str] = &[
    include_str!("../migrations/0001_initial.sql"),
    include_str!("../migrations/0002_import_asset_recovery.sql"),
    include_str!("../migrations/0003_conversation_branches.sql"),
    include_str!("../migrations/0004_provider_catalog.sql"),
    include_str!("../migrations/0005_discovery_state_machine.sql"),
    include_str!("../migrations/0006_generation_provider_provenance.sql"),
    include_str!("../migrations/0007_signed_catalog_history.sql"),
    include_str!("../migrations/0008_generation_protocol_state.sql"),
    include_str!("../migrations/0009_model_sync_jobs.sql"),
    include_str!("../migrations/0010_provider_connection_tombstones.sql"),
    include_str!("../migrations/0011_provider_local_network_approvals.sql"),
    include_str!("../migrations/0012_content_package_foundation.sql"),
    include_str!("../migrations/0013_prompt_pipeline.sql"),
    include_str!("../migrations/0014_knowledge.sql"),
    include_str!("../migrations/0015_memory.sql"),
    include_str!("../migrations/0016_transforms.sql"),
    include_str!("../migrations/0017_interactions_modules.sql"),
    include_str!("../migrations/0018_persona_selection.sql"),
    include_str!("../migrations/0019_lifecycle_outbox.sql"),
    include_str!("../migrations/0020_package_cas_promotion_journal.sql"),
    include_str!("../migrations/0021_interaction_checkpoints.sql"),
    include_str!("../migrations/0022_memory_vector_space.sql"),
    include_str!("../migrations/0023_applied_module_runtime_plans.sql"),
    include_str!("../migrations/0024_generation_attempt_proposals.sql"),
    include_str!("../migrations/0025_conversation_greeting_bindings.sql"),
    include_str!("../migrations/0026_provider_discovery_native_no_effect.sql"),
    include_str!("../migrations/0027_provider_discovery_native_attestations.sql"),
    include_str!("../migrations/0028_generation_attempt_storage_identities.sql"),
    include_str!("../migrations/0029_generation_attempt_decision_handshake.sql"),
    include_str!("../migrations/0030_package_document_target_reviews.sql"),
    include_str!("../migrations/0031_message_display_projections.sql"),
    include_str!("../migrations/0032_knowledge_vector_space.sql"),
    include_str!("../migrations/0033_interaction_derived_event_outbox.sql"),
    include_str!("../migrations/0034_generation_attempt_derived_event_authority.sql"),
    include_str!("../migrations/0035_interaction_derived_event_quarantine.sql"),
    include_str!("../migrations/0036_generation_attempt_derived_closure.sql"),
    include_str!("../migrations/0037_provider_credential_operations.sql"),
    include_str!("../migrations/0038_conversation_speakers.sql"),
    include_str!("../migrations/0039_runtime_model_audit.sql"),
];

fn expected_schema_version() -> u32 {
    u32::try_from(MIGRATIONS.len()).expect("migration count fits u32")
}

fn active_database_path(root: &Path) -> PathBuf {
    let cutover = root.join("db/schema-cutover");
    let (_, relative) = fs::read_dir(cutover)
        .expect("read committed database generations")
        .filter_map(Result::ok)
        .filter(|entry| entry.path().join("generation-committed.json").is_file())
        .map(|entry| {
            let manifest = serde_json::from_slice::<serde_json::Value>(
                &fs::read(entry.path().join("generation-manifest.json"))
                    .expect("read generation manifest"),
            )
            .expect("parse generation manifest");
            let sequence = manifest["activation_sequence"]
                .as_u64()
                .expect("generation activation sequence");
            let relative = manifest["active_database_relative_path"]
                .as_str()
                .expect("active database relative path")
                .to_owned();
            (sequence, relative)
        })
        .max_by_key(|(sequence, _)| *sequence)
        .expect("at least one committed database generation");
    root.join(relative)
}

#[test]
fn fresh_database_reaches_current_contiguous_schema_and_reopens_idempotently() {
    let root = tempdir().expect("temporary data root");
    let storage = Storage::open(root.path()).expect("open fresh schema");
    assert_eq!(
        storage
            .schema_version()
            .expect("read durable schema version"),
        expected_schema_version()
    );
    drop(storage);

    let database_path = active_database_path(root.path());
    let before = schema_inventory(&database_path);
    assert_schema_is_complete(&database_path);

    let reopened = Storage::open(root.path()).expect("reopen current schema");
    assert_eq!(
        reopened
            .schema_version()
            .expect("read durable schema version"),
        expected_schema_version()
    );
    drop(reopened);

    let after = schema_inventory(&database_path);
    assert_eq!(after, before, "reopen must not recreate schema objects");
    assert_schema_is_complete(&database_path);
}

#[test]
fn version_twenty_nine_upgrade_adds_package_document_target_review_authority() {
    let root = tempdir().expect("temporary data root");
    let database_dir = root.path().join("db");
    fs::create_dir_all(&database_dir).expect("create database directory");
    let database_path = database_dir.join("lorepia.sqlite3");
    let mut connection = Connection::open(&database_path).expect("open fixture database");
    connection
        .pragma_update(None, "foreign_keys", true)
        .expect("enable foreign keys");
    apply_through(&mut connection, 29);
    assert_eq!(
        connection
            .query_row(
                "SELECT COUNT(*) FROM sqlite_schema
                 WHERE type = 'table'
                   AND name = 'package_import_document_target_reviews'",
                [],
                |row| row.get::<_, u32>(0),
            )
            .expect("pre-upgrade target-review table count"),
        0
    );
    drop(connection);

    let storage = Storage::open(root.path()).expect("upgrade schema twenty-nine");
    assert_eq!(
        storage
            .schema_version()
            .expect("read upgraded schema version"),
        expected_schema_version()
    );
    drop(storage);

    let database_path = active_database_path(root.path());
    let connection = Connection::open(&database_path).expect("inspect upgraded database");
    assert_eq!(
        connection
            .query_row(
                "SELECT COUNT(*) FROM sqlite_schema
                 WHERE type = 'table'
                   AND name = 'package_import_document_target_reviews'",
                [],
                |row| row.get::<_, u32>(0),
            )
            .expect("target-review table count"),
        1
    );
    for trigger in [
        "package_import_document_target_reviews_guard",
        "package_import_document_target_reviews_no_update",
        "package_import_document_target_reviews_no_delete",
        "package_import_component_commits_guard",
    ] {
        assert_eq!(
            connection
                .query_row(
                    "SELECT COUNT(*) FROM sqlite_schema
                     WHERE type = 'trigger' AND name = ?1",
                    [trigger],
                    |row| row.get::<_, u32>(0),
                )
                .expect("target-review trigger count"),
            1,
            "missing schema-30 trigger {trigger}"
        );
    }
    assert_schema_connection_is_complete(&connection);
}

#[test]
fn version_thirty_upgrade_adds_immutable_message_display_projection_authority() {
    let root = tempdir().expect("temporary data root");
    let database_dir = root.path().join("db");
    fs::create_dir_all(&database_dir).expect("create database directory");
    let database_path = database_dir.join("lorepia.sqlite3");
    let mut connection = Connection::open(&database_path).expect("open fixture database");
    connection
        .pragma_update(None, "foreign_keys", true)
        .expect("enable foreign keys");
    apply_through(&mut connection, 30);
    assert_eq!(
        connection
            .query_row(
                "SELECT COUNT(*) FROM sqlite_schema
                 WHERE type = 'table' AND name = 'message_display_projections'",
                [],
                |row| row.get::<_, u32>(0),
            )
            .expect("pre-upgrade projection table count"),
        0
    );
    drop(connection);

    let storage = Storage::open(root.path()).expect("upgrade schema thirty");
    assert_eq!(
        storage
            .schema_version()
            .expect("read upgraded schema version"),
        expected_schema_version()
    );
    drop(storage);

    let database_path = active_database_path(root.path());
    let before_reopen = schema_inventory(&database_path);
    let connection = Connection::open(&database_path).expect("inspect upgraded database");
    for (kind, name) in [
        ("table", "message_display_projections"),
        ("trigger", "message_display_projections_owner_guard"),
        ("trigger", "message_display_projections_no_update"),
        ("trigger", "message_display_projections_no_delete"),
        ("index", "message_display_projections_generation"),
    ] {
        assert_eq!(
            connection
                .query_row(
                    "SELECT COUNT(*) FROM sqlite_schema
                     WHERE type = ?1 AND name = ?2",
                    params![kind, name],
                    |row| row.get::<_, u32>(0),
                )
                .expect("projection schema object count"),
            1,
            "missing schema-31 object {name}"
        );
    }
    assert_schema_connection_is_complete(&connection);
    drop(connection);

    drop(Storage::open(root.path()).expect("reopen schema thirty-one"));
    assert_eq!(
        schema_inventory(&database_path),
        before_reopen,
        "schema-31 reopen must not recreate schema objects"
    );
}

#[test]
fn version_thirty_projection_migration_rolls_back_every_new_object_on_failure() {
    let root = tempdir().expect("temporary data root");
    let database_dir = root.path().join("db");
    fs::create_dir_all(&database_dir).expect("create database directory");
    let database_path = database_dir.join("lorepia.sqlite3");
    let mut connection = Connection::open(&database_path).expect("open fixture database");
    connection
        .pragma_update(None, "foreign_keys", true)
        .expect("enable foreign keys");
    apply_through(&mut connection, 30);
    connection
        .execute_batch(
            "CREATE TABLE projection_index_collision (id TEXT PRIMARY KEY);
             CREATE INDEX message_display_projections_generation
                 ON projection_index_collision(id);",
        )
        .expect("seed schema-31 index collision");
    drop(connection);

    assert!(
        Storage::open(root.path()).is_err(),
        "schema thirty-one must fail on an existing conflicting index"
    );

    let connection = Connection::open(&database_path).expect("inspect failed migration");
    assert_eq!(
        connection
            .query_row("SELECT MAX(version) FROM schema_migrations", [], |row| {
                row.get::<_, u32>(0)
            })
            .expect("schema version after failed projection migration"),
        30
    );
    for (kind, name) in [
        ("table", "message_display_projections"),
        ("trigger", "message_display_projections_owner_guard"),
        ("trigger", "message_display_projections_no_update"),
        ("trigger", "message_display_projections_no_delete"),
    ] {
        assert_eq!(
            connection
                .query_row(
                    "SELECT COUNT(*) FROM sqlite_schema
                     WHERE type = ?1 AND name = ?2",
                    params![kind, name],
                    |row| row.get::<_, u32>(0),
                )
                .expect("rolled-back projection schema object count"),
            0,
            "failed schema-31 transaction leaked {name}"
        );
    }
}

#[test]
fn version_thirty_three_upgrade_backfills_immutable_generation_derived_event_authority() {
    let root = tempdir().expect("temporary data root");
    let database_dir = root.path().join("db");
    fs::create_dir_all(&database_dir).expect("create database directory");
    let database_path = database_dir.join("lorepia.sqlite3");
    let mut connection = Connection::open(&database_path).expect("open fixture database");
    connection
        .pragma_update(None, "foreign_keys", true)
        .expect("enable foreign keys");
    apply_through(&mut connection, 27);
    seed_legacy_chat(root.path(), &connection);
    seed_legacy_generation_attempt_review(&connection);
    apply_range(&mut connection, 27, 33);
    for column in ["derived_events_json", "derived_events_sha256"] {
        assert_eq!(
            connection
                .query_row(
                    "SELECT COUNT(*)
                     FROM pragma_table_info('generation_attempt_before_event_snapshots')
                     WHERE name = ?1",
                    [column],
                    |row| row.get::<_, u32>(0),
                )
                .expect("pre-upgrade derived-event column count"),
            0,
            "schema 33 unexpectedly contains {column}"
        );
    }
    drop(connection);

    let storage = Storage::open(root.path()).expect("upgrade schema thirty-three");
    assert_eq!(
        storage
            .schema_version()
            .expect("read upgraded schema version"),
        expected_schema_version()
    );
    drop(storage);

    let database_path = active_database_path(root.path());
    let connection = Connection::open(&database_path).expect("inspect upgraded database");
    let (derived_events_json, derived_events_sha256) = connection
        .query_row(
            "SELECT derived_events_json, derived_events_sha256
             FROM generation_attempt_before_event_snapshots
             WHERE generation_id = 'identity-generation'",
            [],
            |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
        )
        .expect("read backfilled derived-event authority");
    assert_eq!(derived_events_json, "[]");
    assert_eq!(
        derived_events_sha256,
        "4f53cda18c2baa0c0354bb5f9a3ecbe5ed12ab4d8e11ba873c2f11161202b945"
    );
    assert!(
        connection
            .execute(
                "UPDATE generation_attempt_before_event_snapshots
                 SET derived_events_json = '[{}]'
                 WHERE generation_id = 'identity-generation'",
                [],
            )
            .is_err(),
        "derived-event authority must inherit snapshot immutability"
    );
    assert_schema_connection_is_complete(&connection);
}

#[test]
fn version_thirty_four_upgrade_adds_immutable_derived_event_quarantine_authority() {
    let root = tempdir().expect("temporary data root");
    let database_dir = root.path().join("db");
    fs::create_dir_all(&database_dir).expect("create database directory");
    let database_path = database_dir.join("lorepia.sqlite3");
    let mut connection = Connection::open(&database_path).expect("open fixture database");
    connection
        .pragma_update(None, "foreign_keys", true)
        .expect("enable foreign keys");
    apply_through(&mut connection, 34);
    assert_eq!(
        connection
            .query_row(
                "SELECT COUNT(*) FROM sqlite_schema
                 WHERE type = 'table'
                   AND name = 'interaction_derived_event_quarantines'",
                [],
                |row| row.get::<_, u32>(0),
            )
            .expect("pre-upgrade quarantine table count"),
        0
    );
    drop(connection);

    let storage = Storage::open(root.path()).expect("upgrade schema thirty-four");
    assert_eq!(
        storage
            .schema_version()
            .expect("read upgraded schema version"),
        expected_schema_version()
    );
    drop(storage);

    let database_path = active_database_path(root.path());
    let before_reopen = schema_inventory(&database_path);
    let connection = Connection::open(&database_path).expect("inspect upgraded database");
    for (kind, name) in [
        ("table", "interaction_derived_event_quarantines"),
        (
            "trigger",
            "interaction_derived_event_quarantine_claim_guard",
        ),
        ("trigger", "interaction_derived_event_quarantine_no_update"),
        ("trigger", "interaction_derived_event_quarantine_no_delete"),
    ] {
        assert_eq!(
            connection
                .query_row(
                    "SELECT COUNT(*) FROM sqlite_schema
                     WHERE type = ?1 AND name = ?2",
                    params![kind, name],
                    |row| row.get::<_, u32>(0),
                )
                .expect("quarantine schema object count"),
            1,
            "missing schema-35 object {name}"
        );
    }
    assert_schema_connection_is_complete(&connection);
    drop(connection);

    drop(Storage::open(root.path()).expect("reopen schema thirty-five"));
    assert_eq!(
        schema_inventory(&database_path),
        before_reopen,
        "schema-35 reopen must not recreate schema objects"
    );
}

#[test]
fn message_display_projection_owner_and_immutability_guards_reject_tampering() {
    let root = tempdir().expect("temporary data root");
    let database_dir = root.path().join("db");
    fs::create_dir_all(&database_dir).expect("create database directory");
    let database_path = database_dir.join("lorepia.sqlite3");
    let mut connection = Connection::open(&database_path).expect("open fixture database");
    connection
        .pragma_update(None, "foreign_keys", true)
        .expect("enable foreign keys");
    apply_through(&mut connection, 31);
    seed_legacy_chat(root.path(), &connection);
    connection
        .execute_batch(
            "INSERT INTO messages (
                 id, conversation_id, parent_id, role, content, status,
                 generation_id, created_at
             ) VALUES (
                 'legacy-assistant', 'legacy-conversation', 'legacy-user',
                 'assistant', 'canonical reply', 'complete',
                 'legacy-generation', '2026-08-03T00:00:00Z'
             );
             UPDATE generations
             SET assistant_message_id = 'legacy-assistant', status = 'complete',
                 finished_at = '2026-08-03T00:00:00Z'
             WHERE id = 'legacy-generation';
             UPDATE conversation_branches
             SET head_message_id = 'legacy-assistant'
             WHERE id = 'legacy-branch';",
        )
        .expect("seed terminal assistant owner");

    assert!(
        connection
            .execute(
                "INSERT INTO message_display_projections (
                     message_id, generation_id, canonical_content_sha256,
                     display_content, display_content_sha256,
                     pipeline_diagnostics_json, diagnostics_sha256, created_at
                 ) VALUES (
                     'legacy-user', 'legacy-generation', ?1,
                     'tampered display', ?2, '{}', ?1, ?3
                 )",
                params![HASH_A, HASH_B, NOW],
            )
            .is_err(),
        "a user message cannot own an assistant display projection"
    );

    connection
        .execute(
            "INSERT INTO message_display_projections (
                 message_id, generation_id, canonical_content_sha256,
                 display_content, display_content_sha256,
                 pipeline_diagnostics_json, diagnostics_sha256, created_at
             ) VALUES (
                 'legacy-assistant', 'legacy-generation', ?1,
                 'display reply', ?2,
                 '{\"schema_version\":1,\"failures\":[]}', ?1, ?3
             )",
            params![HASH_A, HASH_B, NOW],
        )
        .expect("insert valid projection owner");
    assert!(
        connection
            .execute(
                "UPDATE message_display_projections
                 SET display_content = 'tampered'
                 WHERE message_id = 'legacy-assistant'",
                [],
            )
            .is_err(),
        "stored display projections must reject updates"
    );
    assert!(
        connection
            .execute(
                "DELETE FROM message_display_projections
                 WHERE message_id = 'legacy-assistant'",
                [],
            )
            .is_err(),
        "stored display projections must reject deletion"
    );
    assert_eq!(
        connection
            .query_row(
                "SELECT content FROM messages WHERE id = 'legacy-assistant'",
                [],
                |row| row.get::<_, String>(0),
            )
            .expect("canonical assistant content"),
        "canonical reply",
        "a display sidecar must never rewrite canonical message content"
    );
}

#[test]
#[allow(clippy::too_many_lines)] // The fixture proves both source binding and the insert window.
fn package_target_review_insert_guard_binds_source_and_selection_window() {
    let root = tempdir().expect("temporary data root");
    let storage = Storage::open(root.path()).expect("open current schema");
    drop(storage);

    let database_path = active_database_path(root.path());
    let connection = Connection::open(&database_path).expect("open fixture database");
    connection
        .pragma_update(None, "foreign_keys", true)
        .expect("enable foreign keys");
    connection
        .execute(
            "INSERT INTO content_sources (
                sha256, relative_path, size_bytes, created_at
             ) VALUES (?1, 'bb/package-source', 1, ?2)",
            params![HASH_B, NOW],
        )
        .expect("source fixture");
    connection
        .execute(
            "INSERT INTO package_sources (
                id, source_hash, format, format_version, package_id, name,
                version, author, manifest_json, manifest_sha256,
                license_expression, license_status, redistribution_status,
                required_app_version, signature_json, signature_status,
                created_at
             ) VALUES (
                'target-review-source', ?1, 'lorepia_content_package', 1,
                'target-review-package', 'Target review package', '1.0.0',
                NULL, '{}', ?2, NULL, 'unknown', 'unknown', NULL, NULL,
                'unsigned', ?3
             )",
            params![HASH_B, HASH_A, NOW],
        )
        .expect("package source fixture");
    connection
        .execute(
            "INSERT INTO package_imports (
                id, package_source_id, inspection_schema_version, state,
                revision, inspection_json, inspection_sha256,
                selection_json, selection_sha256,
                capability_review_sha256, approved_selection_sha256,
                approved_at, failure_json, created_at, updated_at,
                completed_at
             ) VALUES (
                'target-review-import', 'target-review-source', 1,
                'inspected', 1, '{}', ?1, NULL, NULL, ?2, NULL, NULL,
                NULL, ?3, ?3, NULL
             )",
            params![HASH_A, HASH_B, NOW],
        )
        .expect("package import fixture");

    for (ordinal, component_id) in [(0_u32, "component-a"), (1_u32, "component-b")] {
        let review_json = format!(
            "{{\"id\":\"{component_id}\",\"kind\":\"transform_set\",\"logical_path\":\"transforms/{component_id}.json\",\"sha256\":\"{HASH_A}\",\"dependencies\":[],\"conflicts_with\":[],\"required_capabilities\":[],\"asset_ids\":[],\"disposition\":\"importable\"}}"
        );
        connection
            .execute(
                "INSERT INTO package_import_components (
                    import_id, ordinal, source_component_key, component_kind,
                    disposition, selected, target_object_id,
                    target_revision_id, review_json, review_sha256
                 ) VALUES (
                    'target-review-import', ?1, ?2, 'transform_set',
                    'create', 1, NULL, NULL, ?3, ?4
                 )",
                params![i64::from(ordinal), component_id, review_json, HASH_B],
            )
            .expect("selected component fixture");
    }

    let insert_target_review = |component_ordinal: u32,
                                document_index: u32,
                                component_id: &str,
                                source_component_sha256: &str| {
        connection.execute(
            "INSERT INTO package_import_document_target_reviews (
                import_id, component_ordinal, document_ordinal,
                document_index, document_kind, target_object_id,
                disposition, expected_target_revision_id,
                expected_target_state_revision, source_component_sha256,
                document_sha256
             ) VALUES (
                'target-review-import', ?1, 0, ?2, 'transform_set', ?3,
                'create', NULL, NULL, ?4, ?5
             )",
            params![
                i64::from(component_ordinal),
                i64::from(document_index),
                component_id,
                source_component_sha256,
                HASH_B,
            ],
        )
    };

    assert!(
        insert_target_review(0, 0, "target-a", HASH_B).is_err(),
        "a target row must bind the immutable source component digest"
    );
    insert_target_review(0, 0, "target-a", HASH_A).expect("exact source component target review");

    connection
        .execute(
            "UPDATE package_imports
             SET state = 'awaiting_review', revision = 2,
                 selection_json = '{}', selection_sha256 = ?1,
                 updated_at = ?2
             WHERE id = 'target-review-import'",
            params![HASH_A, NOW],
        )
        .expect("close target-review selection window");
    assert!(
        insert_target_review(1, 1, "target-b", HASH_A).is_err(),
        "target rows cannot be appended after the sealed selection transition"
    );
    assert_eq!(
        connection
            .query_row(
                "SELECT COUNT(*)
                 FROM package_import_document_target_reviews
                 WHERE import_id = 'target-review-import'",
                [],
                |row| row.get::<_, u32>(0),
            )
            .expect("target-review row count"),
        1
    );
}

#[test]
fn version_twenty_seven_upgrade_backfills_populated_generation_attempt_identities() {
    let root = tempdir().expect("temporary data root");
    let database_dir = root.path().join("db");
    fs::create_dir_all(&database_dir).expect("create database directory");
    let database_path = database_dir.join("lorepia.sqlite3");
    let mut connection = Connection::open(&database_path).expect("open fixture database");
    connection
        .pragma_update(None, "foreign_keys", true)
        .expect("enable foreign keys");
    apply_through(&mut connection, 27);
    seed_legacy_chat(root.path(), &connection);
    seed_legacy_generation_attempt_review(&connection);
    drop(connection);

    let storage = Storage::open(root.path()).expect("upgrade populated schema twenty-seven");
    assert_eq!(
        storage
            .schema_version()
            .expect("read upgraded schema version"),
        expected_schema_version()
    );
    drop(storage);

    let database_path = active_database_path(root.path());
    let connection = Connection::open(&database_path).expect("inspect upgraded database");
    assert_eq!(
        connection
            .query_row(
                "SELECT review_sha256, domain_review_sha256,
                        storage_identity_version
                 FROM generation_attempt_before_event_snapshots
                 WHERE generation_id = 'identity-generation'",
                [],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, u32>(2)?,
                    ))
                },
            )
            .expect("backfilled generation review identity"),
        (HASH_B.to_owned(), HASH_B.to_owned(), 1)
    );
    for column in [
        "domain_proposal_record_id",
        "domain_proposal_review_sha256",
        "storage_identity_version",
    ] {
        assert_eq!(
            connection
                .query_row(
                    "SELECT COUNT(*) FROM pragma_table_info('generation_attempt_proposals')
                     WHERE name = ?1",
                    [column],
                    |row| row.get::<_, u32>(0),
                )
                .expect("generation proposal identity column count"),
            1,
            "missing schema-28 proposal identity column {column}"
        );
    }
    assert_schema_connection_is_complete(&connection);
}

#[test]
fn malformed_legacy_generation_identity_rolls_back_version_twenty_eight() {
    let root = tempdir().expect("temporary data root");
    let database_dir = root.path().join("db");
    fs::create_dir_all(&database_dir).expect("create database directory");
    let database_path = database_dir.join("lorepia.sqlite3");
    let mut connection = Connection::open(&database_path).expect("open fixture database");
    connection
        .pragma_update(None, "foreign_keys", true)
        .expect("enable foreign keys");
    apply_through(&mut connection, 27);
    seed_legacy_chat(root.path(), &connection);
    seed_legacy_generation_attempt_review(&connection);
    connection
        .pragma_update(None, "foreign_keys", false)
        .expect("disable foreign keys for corruption fixture");
    connection
        .pragma_update(None, "ignore_check_constraints", true)
        .expect("disable checks for corruption fixture");
    connection
        .execute_batch(
            "DROP TRIGGER generation_attempt_before_snapshot_no_update;
             DROP TRIGGER generation_attempt_aggregate_update_guard;
             UPDATE generation_attempt_before_event_snapshots
             SET review_sha256 = 'malformed'
             WHERE generation_id = 'identity-generation';
             UPDATE generation_attempt_interaction_aggregates
             SET before_review_sha256 = 'malformed'
             WHERE generation_id = 'identity-generation';",
        )
        .expect("inject malformed legacy generation identity");
    drop(connection);

    let error = Storage::open(root.path())
        .map(drop)
        .expect_err("schema twenty-eight must reject malformed legacy identities");
    assert_eq!(error.code, CoreErrorCode::StorageCorrupted);
    assert_eq!(
        error.message,
        "legacy generation review identity is malformed"
    );

    let connection = Connection::open(&database_path).expect("inspect failed migration");
    assert_eq!(
        connection
            .query_row("SELECT MAX(version) FROM schema_migrations", [], |row| {
                row.get::<_, u32>(0)
            })
            .expect("schema version after failed identity migration"),
        27
    );
    assert_eq!(
        connection
            .query_row(
                "SELECT COUNT(*)
                 FROM pragma_table_info('generation_attempt_before_event_snapshots')
                 WHERE name IN ('domain_review_sha256', 'storage_identity_version')",
                [],
                |row| row.get::<_, u32>(0),
            )
            .expect("rolled-back generation review identity columns"),
        0,
        "schema twenty-eight DDL must roll back with its registry write"
    );
}

#[test]
fn version_eleven_upgrade_preserves_legacy_rows_and_nullable_plan_provenance() {
    let root = tempdir().expect("temporary data root");
    let database_dir = root.path().join("db");
    fs::create_dir_all(&database_dir).expect("create database directory");
    let database_path = database_dir.join("lorepia.sqlite3");
    let mut connection = Connection::open(&database_path).expect("open fixture database");
    connection
        .pragma_update(None, "foreign_keys", true)
        .expect("enable foreign keys");
    apply_through(&mut connection, 11);
    seed_legacy_chat(root.path(), &connection);
    drop(connection);

    let storage = Storage::open(root.path()).expect("upgrade schema eleven");
    assert_eq!(
        storage
            .schema_version()
            .expect("read durable schema version"),
        expected_schema_version()
    );
    drop(storage);

    let database_path = active_database_path(root.path());
    let connection = Connection::open(&database_path).expect("inspect upgraded database");
    connection
        .pragma_update(None, "foreign_keys", true)
        .expect("enable foreign keys");
    assert_eq!(
        connection
            .query_row(
                "SELECT description FROM characters WHERE id = 'legacy-character'",
                [],
                |row| row.get::<_, String>(0),
            )
            .expect("legacy character"),
        "legacy bytes stay unchanged"
    );
    assert!(
        connection
            .query_row(
                "SELECT (
                    resolved_prompt_plan_id IS NULL
                    AND prompt_plan_sha256 IS NULL
                    AND provider_request_snapshot_id IS NULL
                 )
                 FROM generations WHERE id = 'legacy-generation'",
                [],
                |row| row.get::<_, bool>(0),
            )
            .expect("legacy generation provenance")
    );
    assert_schema_connection_is_complete(&connection);
}

#[test]
fn migration_thirteen_is_atomic_and_can_resume_after_object_conflict() {
    let root = tempdir().expect("temporary data root");
    let database_dir = root.path().join("db");
    fs::create_dir_all(&database_dir).expect("create database directory");
    let database_path = database_dir.join("lorepia.sqlite3");
    {
        let mut connection = Connection::open(&database_path).expect("open fixture database");
        connection
            .pragma_update(None, "foreign_keys", true)
            .expect("enable foreign keys");
        apply_through(&mut connection, 12);
        connection
            .execute_batch("CREATE TABLE prompt_blocks (conflict INTEGER);")
            .expect("create migration conflict");
    }

    assert!(
        Storage::open(root.path()).is_err(),
        "a conflicting schema object must fail migration thirteen"
    );
    {
        let connection = Connection::open(&database_path).expect("inspect failed migration");
        assert_eq!(
            connection
                .query_row("SELECT MAX(version) FROM schema_migrations", [], |row| {
                    row.get::<_, u32>(0)
                })
                .expect("schema version"),
            12
        );
        assert_eq!(
            connection
                .query_row(
                    "SELECT COUNT(*) FROM sqlite_schema
                     WHERE type = 'table' AND name = 'prompt_presets'",
                    [],
                    |row| row.get::<_, u32>(0),
                )
                .expect("rolled-back prompt table count"),
            0,
            "all earlier DDL in migration thirteen must roll back"
        );
        connection
            .execute("DROP TABLE prompt_blocks", [])
            .expect("remove deliberate conflict");
    }

    let reopened = Storage::open(root.path()).expect("resume repaired migration");
    assert_eq!(
        reopened
            .schema_version()
            .expect("read durable schema version"),
        expected_schema_version()
    );
}

#[test]
fn version_twenty_five_upgrade_requires_typed_atomic_commit_attestation() {
    let root = tempdir().expect("temporary data root");
    let database_dir = root.path().join("db");
    fs::create_dir_all(&database_dir).expect("create database directory");
    let database_path = database_dir.join("lorepia.sqlite3");
    let mut connection = Connection::open(&database_path).expect("open fixture database");
    connection
        .pragma_update(None, "foreign_keys", true)
        .expect("enable foreign keys");
    apply_through(&mut connection, 25);
    connection
        .execute(
            "INSERT INTO provider_discovery_sessions (
                 id, state, sanitized_input_json, created_at, updated_at
             ) VALUES ('native-recovery', 'draft', '{}', ?1, ?1)",
            [NOW],
        )
        .expect("seed provider discovery session");
    for (id, operation_kind) in [
        ("credential-install", "atomic_commit"),
        ("compensation", "compensation"),
    ] {
        connection
            .execute(
                "INSERT INTO provider_discovery_operations (
                     id, session_id, operation_kind, side_effect_class, status,
                     action_id, expected_revision, request_sha256, started_at,
                     created_at, updated_at
                 ) VALUES (
                     ?1, 'native-recovery', ?2, 'persistent', 'started',
                     ?1, 0, ?3, ?4, ?4, ?4
                 )",
                params![id, operation_kind, HASH_A, NOW],
            )
            .expect("seed started persistent operation");
    }
    assert!(
        connection
            .execute(
                "UPDATE provider_discovery_operations
                 SET status = 'interrupted', finished_at = ?2, updated_at = ?2
                 WHERE id = ?1",
                params!["credential-install", NOW],
            )
            .is_err(),
        "schema twenty-five must retain its conservative persistent-effect rule"
    );
    drop(connection);

    let storage = Storage::open_with_deferred_discovery_recovery(root.path())
        .expect("upgrade schema without classifying the recovery fixture");
    assert_eq!(
        storage
            .schema_version()
            .expect("read durable schema version"),
        expected_schema_version()
    );
    drop(storage);

    let database_path = active_database_path(root.path());
    let connection = Connection::open(&database_path).expect("inspect upgraded database");
    connection
        .pragma_update(None, "foreign_keys", true)
        .expect("enable foreign keys");
    assert!(
        connection
            .execute(
                "UPDATE provider_discovery_operations
                 SET status = 'interrupted', finished_at = ?2, updated_at = ?2
                 WHERE id = ?1",
                params!["credential-install", NOW],
            )
            .is_err(),
        "schema twenty-seven must require an exact typed native attestation row"
    );
    assert!(
        connection
            .execute(
                "UPDATE provider_discovery_operations
                 SET status = 'interrupted', finished_at = ?2, updated_at = ?2
                 WHERE id = ?1",
                params!["compensation", NOW],
            )
            .is_err(),
        "the migration must not weaken other persistent operation recovery"
    );
}

#[test]
fn memory_job_current_revision_is_monotonic_and_supports_compare_and_swap() {
    let root = tempdir().expect("temporary data root");
    let storage = Storage::open(root.path()).expect("open fresh schema");
    drop(storage);
    let database_path = active_database_path(root.path());
    let connection = Connection::open(&database_path).expect("open schema database");
    connection
        .pragma_update(None, "foreign_keys", true)
        .expect("enable foreign keys");
    seed_legacy_chat(root.path(), &connection);

    assert_eq!(
        connection
            .query_row(
                "SELECT dflt_value
                 FROM pragma_table_info('memory_jobs')
                 WHERE name = 'revision'",
                [],
                |row| row.get::<_, String>(0),
            )
            .expect("memory job revision default"),
        "1"
    );
    assert!(
        connection
            .execute(
                "INSERT INTO memory_jobs (
                    id, idempotency_key, job_kind, conversation_id, branch_id,
                    source_start_message_id, source_end_message_id,
                    input_fingerprint_sha256, state, revision, attempts,
                    available_at, payload_json, created_at, updated_at
                 ) VALUES (
                    'invalid-memory-job', 'invalid-memory-job-key', 'summary',
                    'legacy-conversation', 'legacy-branch', 'legacy-user',
                    'legacy-user', ?1, 'queued', 2, 0, ?2, '{}', ?2, ?2
                 )",
                params![HASH_A, NOW],
            )
            .is_err(),
        "a memory job must begin at current revision one"
    );
    connection
        .execute(
            "INSERT INTO memory_jobs (
                id, idempotency_key, job_kind, conversation_id, branch_id,
                source_start_message_id, source_end_message_id,
                input_fingerprint_sha256, state, attempts, available_at,
                payload_json, created_at, updated_at
             ) VALUES (
                'memory-job', 'memory-job-key', 'summary',
                'legacy-conversation', 'legacy-branch', 'legacy-user',
                'legacy-user', ?1, 'queued', 0, ?2, '{}', ?2, ?2
             )",
            params![HASH_A, NOW],
        )
        .expect("insert memory job at the default current revision");
    assert_eq!(
        connection
            .query_row(
                "SELECT revision FROM memory_jobs WHERE id = 'memory-job'",
                [],
                |row| row.get::<_, u64>(0),
            )
            .expect("created memory job revision"),
        1
    );

    assert_memory_job_compare_and_swap(&connection);
}

fn assert_memory_job_compare_and_swap(connection: &Connection) {
    assert!(
        connection
            .execute(
                "UPDATE memory_jobs
                 SET state = 'running', started_at = ?1, updated_at = ?1
                 WHERE id = 'memory-job'",
                [NOW],
            )
            .is_err(),
        "a state write must advance the current revision"
    );
    assert_eq!(
        connection
            .execute(
                "UPDATE memory_jobs
                 SET state = 'running', revision = revision + 1,
                     started_at = ?1, updated_at = ?1
                 WHERE id = 'memory-job' AND revision = 1",
                [NOW],
            )
            .expect("compare-and-swap queued memory job"),
        1
    );
    assert_eq!(
        connection
            .execute(
                "UPDATE memory_jobs
                 SET state = 'cancelled', revision = revision + 1,
                     finished_at = ?1, updated_at = ?1
                 WHERE id = 'memory-job' AND revision = 1",
                [NOW],
            )
            .expect("stale compare-and-swap statement"),
        0,
        "a stale expected revision must not overwrite current state"
    );
    assert_eq!(
        connection
            .query_row(
                "SELECT state, revision FROM memory_jobs WHERE id = 'memory-job'",
                [],
                |row| Ok((row.get::<_, String>(0)?, row.get::<_, u64>(1)?)),
            )
            .expect("memory job after stale compare-and-swap"),
        ("running".to_owned(), 2)
    );
    assert_eq!(
        connection
            .execute(
                "UPDATE memory_jobs
                 SET state = 'cancelled', revision = revision + 1,
                     finished_at = ?1, updated_at = ?1
                 WHERE id = 'memory-job' AND revision = 2",
                [NOW],
            )
            .expect("compare-and-swap running memory job"),
        1
    );
    assert_eq!(
        connection
            .query_row(
                "SELECT state, revision FROM memory_jobs WHERE id = 'memory-job'",
                [],
                |row| Ok((row.get::<_, String>(0)?, row.get::<_, u64>(1)?)),
            )
            .expect("cancelled memory job"),
        ("cancelled".to_owned(), 3)
    );
}

#[test]
#[allow(clippy::too_many_lines)]
fn governance_checks_fail_closed_and_revision_evidence_is_immutable() {
    let root = tempdir().expect("temporary data root");
    let storage = Storage::open(root.path()).expect("open fresh schema");
    drop(storage);
    let database_path = active_database_path(root.path());
    let connection = Connection::open(&database_path).expect("open schema database");
    connection
        .pragma_update(None, "foreign_keys", true)
        .expect("enable foreign keys");

    let built_in_revision = connection
        .query_row(
            "SELECT id FROM content_revisions
             WHERE object_kind = 'prompt_preset'
             ORDER BY object_id, revision_no LIMIT 1",
            [],
            |row| row.get::<_, String>(0),
        )
        .expect("built-in prompt revision");
    assert!(
        connection
            .execute(
                "UPDATE content_revisions SET document_json = '{}'
                 WHERE id = ?1",
                [&built_in_revision],
            )
            .is_err(),
        "sealed content revision must be immutable"
    );
    assert!(
        connection
            .execute(
                "INSERT INTO share_eligibility_reviews (
                    id, content_revision_id, share_scope, policy_version,
                    decision, license_status, redistribution_status,
                    blockers_json, evidence_json, evaluated_at
                 ) VALUES (
                    'unsafe-share', ?1, 'public_share', 1, 'eligible',
                    'unknown', 'unknown', '[]', '{}', ?2
                 )",
                params![built_in_revision, NOW],
            )
            .is_err(),
        "unknown licensing must never be publicly share-eligible"
    );

    seed_legacy_chat(root.path(), &connection);
    let built_in_preset = connection
        .query_row(
            "SELECT object_id FROM content_revisions WHERE id = ?1",
            [&built_in_revision],
            |row| row.get::<_, String>(0),
        )
        .expect("built-in prompt preset id");
    connection
        .execute(
            "INSERT INTO generation_prompt_plans (
                id, schema_version, plan_sha256, input_fingerprint_sha256,
                conversation_id, branch_id, head_message_id,
                latest_user_message_id, latest_user_included,
                prompt_preset_id, prompt_preset_revision_id,
                generation_preset_id, model_route_id,
                task_profile_revision_id, random_seed, tokenizer_id,
                tokenizer_version, context_limit_tokens,
                reserved_output_tokens, estimated_input_tokens,
                final_input_tokens, message_count, cacheable_prefix_tokens,
                status, canonical_plan_json, sealed_at, created_at
             ) VALUES (
                'sealed-plan', 1, ?1, ?2, 'legacy-conversation',
                'legacy-branch', 'legacy-user', 'legacy-user', 1, ?3, ?4,
                NULL, NULL, NULL, 7, 'test-tokenizer', '1', 4096, 512, 1,
                1, 1, 0, 'resolved', '{}', ?5, ?5
             )",
            params![HASH_A, HASH_B, built_in_preset, built_in_revision, NOW],
        )
        .expect("resolved prompt plan fixture");
    connection
        .execute(
            "INSERT INTO generation_prompt_plan_messages (
                plan_id, ordinal, role, content, content_sha256,
                source_block_ordinals_json, source_message_id,
                estimated_tokens
             ) VALUES (
                'sealed-plan', 0, 'user', 'unchanged user message', ?1,
                '[]', 'legacy-user', 1
             )",
            [HASH_A],
        )
        .expect("resolved prompt message fixture");
    connection
        .execute(
            "INSERT INTO generation_prompt_plan_seals (
                plan_id, plan_sha256, sealed_at
             ) VALUES ('sealed-plan', ?1, ?2)",
            params![HASH_A, NOW],
        )
        .expect("seal resolved prompt plan");
    assert!(
        connection
            .execute(
                "INSERT INTO generation_prompt_plan_warnings (
                    plan_id, ordinal, code, severity, message_key,
                    details_json
                 ) VALUES (
                    'sealed-plan', 0, 'late-warning', 'warning',
                    'prompt.warning.late', '{}'
                 )",
                [],
            )
            .is_err(),
        "normalized plan evidence must reject inserts after the seal"
    );

    connection
        .execute(
            "INSERT INTO content_sources (
                sha256, relative_path, size_bytes, created_at
             ) VALUES (?1, 'bb/source', 1, ?2)",
            params![HASH_B, NOW],
        )
        .expect("source fixture");
    connection
        .execute(
            "INSERT INTO package_sources (
                id, source_hash, format, format_version, package_id, name,
                version, author, manifest_json, manifest_sha256,
                license_expression, license_status, redistribution_status,
                required_app_version, signature_json, signature_status,
                created_at
             ) VALUES (
                'package-source', ?1, 'lorepia_content_package', 1,
                'package-id', 'Package', '1.0.0', NULL, '{}', ?2,
                NULL, 'unknown', 'unknown', NULL, NULL, 'unsigned', ?3
             )",
            params![HASH_B, HASH_A, NOW],
        )
        .expect("package source fixture");
    assert!(
        connection
            .execute(
                "INSERT INTO package_imports (
                    id, package_source_id, inspection_schema_version, state,
                    revision, inspection_json, inspection_sha256,
                    selection_json, selection_sha256,
                    capability_review_sha256, approved_selection_sha256,
                    approved_at, failure_json, created_at, updated_at,
                    completed_at
                 ) VALUES (
                    'invalid-approved-import', 'package-source', 1, 'approved',
                    1, '{}', ?1, NULL, NULL, ?2, ?2, ?3, NULL, ?3, ?3, NULL
                 )",
                params![HASH_A, HASH_B, NOW],
            )
            .is_err(),
        "an approval hash must not bypass a missing reviewed selection"
    );

    connection
        .execute(
            "INSERT INTO content_objects (
                id, object_kind, created_at, deleted_at
             ) VALUES (
                'imported-transform', 'transform_set', ?1, NULL
             )",
            [NOW],
        )
        .expect("transform identity fixture");
    assert!(
        connection
            .execute(
                "INSERT INTO transform_sets (
                    id, name, schema_version, revision, enabled,
                    max_rules_per_phase, max_output_chars, document_json,
                    provenance_json, source_kind, source_hash,
                    created_at, updated_at, deleted_at
                 ) VALUES (
                    'imported-transform', 'Imported', 1, 1, 1,
                    16, 4096, '{}', '{}', 'imported_package', ?1,
                    ?2, ?2, NULL
                 )",
                params![HASH_A, NOW],
            )
            .is_err(),
        "imported transform sets must remain inactive before local review"
    );
}

fn apply_through(connection: &mut Connection, target: usize) {
    apply_range(connection, 0, target);
}

fn apply_range(connection: &mut Connection, start_exclusive: usize, target: usize) {
    for (index, migration) in MIGRATIONS
        .iter()
        .enumerate()
        .take(target)
        .skip(start_exclusive)
    {
        let version = u32::try_from(index + 1).expect("schema version");
        let transaction = connection.transaction().expect("migration transaction");
        transaction
            .execute_batch(migration)
            .unwrap_or_else(|error| panic!("apply migration {version}: {error}"));
        transaction
            .execute(
                "INSERT OR IGNORE INTO schema_migrations(version, applied_at)
                 VALUES (?1, ?2)",
                params![version, NOW],
            )
            .unwrap_or_else(|error| panic!("record migration {version}: {error}"));
        let violation = transaction
            .query_row("PRAGMA foreign_key_check", [], |_| Ok(()))
            .optional()
            .expect("foreign key check");
        assert!(
            violation.is_none(),
            "migration {version} produced a foreign-key violation"
        );
        transaction.commit().expect("commit migration");
    }
}

fn seed_legacy_generation_attempt_review(connection: &Connection) {
    connection
        .execute_batch(
            "INSERT INTO generation_attempt_intents (
                 generation_id, operation_id, conversation_id,
                 source_branch_id, proposed_branch_id,
                 expected_head_message_id, context_head_message_id,
                 module_plan_sha256, base_input_fingerprint_sha256,
                 attempt_sha256, status, revision, failure_code,
                 created_at, updated_at
             ) VALUES (
                 'identity-generation', 'identity-operation',
                 'legacy-conversation', 'legacy-branch', 'legacy-branch',
                 'legacy-user', 'legacy-user',
                 'aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa',
                 'aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa',
                 'aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa',
                 'prepared', 1, NULL,
                 '2026-08-03T00:00:00Z', '2026-08-03T00:00:00Z'
             );
             INSERT INTO generation_attempt_before_event_snapshots (
                 generation_id, event_id, event_kind, event_json, event_sha256,
                 occurred_at, context_head_message_id, context_checkpoint_sha256,
                 previous_state_revision, previous_state_json,
                 previous_state_document_sha256, previous_state_snapshot_sha256,
                 previous_knowledge_json, previous_knowledge_sha256,
                 applied_runtime_plan_sha256, module_runtime_review_json,
                 module_runtime_review_sha256, memory_head_snapshot_json,
                 memory_head_snapshot_sha256, source_runtime_plan_sha256,
                 source_activation_plan_sha256, applied_runtime_plan_json,
                 policy_json, policy_sha256, reviewed_next_state_json,
                 reviewed_next_state_document_sha256,
                 reviewed_next_state_snapshot_sha256, knowledge_json,
                 knowledge_sha256, action_results_json, action_results_sha256,
                 effects_json, effects_sha256, proposal_writes_json,
                 proposal_writes_sha256, review_sha256, created_at
             ) VALUES (
                 'identity-generation', 'identity-before-event',
                 'before_generation', '{\"kind\":\"before_generation\"}',
                 'aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa',
                 '2026-08-03T00:00:00Z', 'legacy-user',
                 'aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa',
                 0, '{}',
                 'aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa',
                 'aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa',
                 '[]',
                 'aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa',
                 'aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa',
                 '{}',
                 'aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa',
                 '{}',
                 'aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa',
                 NULL, NULL, NULL, '{}',
                 'aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa',
                 '{}',
                 'aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa',
                 'aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa',
                 '[]',
                 'aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa',
                 '[]',
                 'aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa',
                 '[]',
                 'aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa',
                 '[]',
                 'aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa',
                 'bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb',
                 '2026-08-03T00:00:00Z'
             );
             INSERT INTO generation_attempt_interaction_aggregates (
                 generation_id, before_review_sha256, aggregate_revision,
                 interaction_state_revision, state_json,
                 state_document_sha256, state_snapshot_sha256,
                 knowledge_json, knowledge_sha256, pending_proposal_count,
                 terminal_decision_count, decision_event_ids_json,
                 decision_event_ids_sha256, decision_event_sha256s_json,
                 decision_event_sha256s_sha256, created_at, updated_at
             ) VALUES (
                 'identity-generation',
                 'bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb',
                 1, 1, '{}',
                 'aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa',
                 'aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa',
                 '[]',
                 'aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa',
                 0, 0, '[]',
                 'aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa',
                 '[]',
                 'aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa',
                 '2026-08-03T00:00:00Z', '2026-08-03T00:00:00Z'
             );",
        )
        .expect("seed populated schema-twenty-seven generation review");
}

#[allow(clippy::too_many_lines)]
fn seed_legacy_chat(root: &Path, connection: &Connection) {
    let source_relative_path = format!(
        "sources/sha256/{}/{}",
        &LEGACY_CHAT_SOURCE_SHA256[..2],
        &LEGACY_CHAT_SOURCE_SHA256[2..]
    );
    let source_path = root.join(&source_relative_path);
    fs::create_dir_all(source_path.parent().expect("legacy chat source parent"))
        .expect("create legacy chat source CAS directory");
    fs::write(&source_path, LEGACY_CHAT_SOURCE_BYTES).expect("write legacy chat source CAS bytes");
    connection
        .execute_batch(&format!(
            "INSERT INTO content_sources (
                sha256, relative_path, size_bytes, created_at
             ) VALUES (
                '{LEGACY_CHAT_SOURCE_SHA256}',
                '{source_relative_path}', {}, '2026-08-03T00:00:00Z'
             );
             INSERT INTO characters (
                id, name, description, source_hash, avatar_asset_hash, created_at
             ) VALUES (
                'legacy-character', 'Legacy', 'legacy bytes stay unchanged',
                '{LEGACY_CHAT_SOURCE_SHA256}',
                NULL, '2026-08-03T00:00:00Z'
             );
             INSERT INTO conversations (
                id, character_id, title, created_at, updated_at
             ) VALUES (
                'legacy-conversation', 'legacy-character', 'Legacy room',
                '2026-08-03T00:00:00Z', '2026-08-03T00:00:00Z'
             );
             INSERT INTO messages (
                id, conversation_id, parent_id, role, content, status,
                generation_id, created_at
             ) VALUES (
                'legacy-user', 'legacy-conversation', NULL, 'user',
                'unchanged user message', 'complete', NULL,
                '2026-08-03T00:00:00Z'
             );
             INSERT INTO conversation_branches (
                id, conversation_id, title, fork_message_id, head_message_id,
                created_at, updated_at
             ) VALUES (
                'legacy-branch', 'legacy-conversation', NULL, NULL,
                'legacy-user', '2026-08-03T00:00:00Z',
                '2026-08-03T00:00:00Z'
             );
             INSERT INTO conversation_state (
                conversation_id, active_branch_id, selected_mode, updated_at
             ) VALUES (
                'legacy-conversation', 'legacy-branch', 'chat',
                '2026-08-03T00:00:00Z'
             );
             INSERT INTO generations (
                id, conversation_id, branch_id, user_message_id,
                assistant_message_id, mode, model, status, input_tokens,
                output_tokens, error_code, started_at, finished_at,
                model_route_id, generation_preset_id, provider_family,
                cached_read_tokens, cached_write_tokens, reasoning_tokens,
                tool_tokens, provider_raw_summary_json,
                opaque_reasoning_state_json
             ) VALUES (
                'legacy-generation', 'legacy-conversation', 'legacy-branch',
                'legacy-user', NULL, 'chat', 'legacy-model', 'running',
                NULL, NULL, NULL, '2026-08-03T00:00:00Z', NULL,
                NULL, NULL, NULL, NULL, NULL, NULL, NULL, NULL, NULL
             );",
            LEGACY_CHAT_SOURCE_BYTES.len()
        ))
        .expect("seed schema-eleven legacy rows");
}

fn schema_inventory(database_path: &Path) -> (u32, u32, u32) {
    let connection = Connection::open(database_path).expect("open database inventory");
    (
        connection
            .query_row("SELECT COUNT(*) FROM schema_migrations", [], |row| {
                row.get(0)
            })
            .expect("migration count"),
        connection
            .query_row(
                "SELECT COUNT(*) FROM sqlite_schema WHERE type = 'table'",
                [],
                |row| row.get(0),
            )
            .expect("table count"),
        connection
            .query_row(
                "SELECT COUNT(*) FROM sqlite_schema WHERE type = 'trigger'",
                [],
                |row| row.get(0),
            )
            .expect("trigger count"),
    )
}

fn assert_schema_is_complete(database_path: &Path) {
    let connection = Connection::open(database_path).expect("open schema database");
    assert_schema_connection_is_complete(&connection);
}

fn assert_schema_connection_is_complete(connection: &Connection) {
    let versions = connection
        .prepare("SELECT version FROM schema_migrations ORDER BY version")
        .expect("prepare migration versions")
        .query_map([], |row| row.get::<_, u32>(0))
        .expect("query migration versions")
        .collect::<Result<Vec<_>, _>>()
        .expect("collect migration versions");
    assert_eq!(
        versions,
        (1..=expected_schema_version()).collect::<Vec<_>>()
    );

    for table in [
        "character_content",
        "package_sources",
        "package_imports",
        "package_import_component_commits",
        "package_import_document_target_reviews",
        "package_import_audit_events",
        "content_revisions",
        "content_rollback_plans",
        "generation_attempt_intents",
        "generation_attempt_before_event_snapshots",
        "generation_attempt_interaction_aggregates",
        "generation_attempt_proposals",
        "generation_attempt_aggregate_decision_bindings",
        "generation_attempt_proposal_decision_commits",
        "message_display_projections",
        "conversation_greeting_bindings",
        "provider_discovery_native_no_effect_attestations",
        "provider_selection_state",
        "provider_discovery_selection_restore_authorities",
        "core_lifecycle_outbox",
        "interaction_state_checkpoints",
        "prompt_preset_rollback_reviews",
        "prompt_preset_rollback_approvals",
        "memory_query_embeddings",
        "applied_module_runtime_plans",
        "share_eligibility_reviews",
        "prompt_presets",
        "prompt_blocks",
        "prompt_controls",
        "prompt_preset_bindings",
        "prompt_cache_boundaries",
        "task_profiles",
        "generation_prompt_plans",
        "generation_prompt_plan_seals",
        "generation_prompt_plan_messages",
        "provider_request_snapshots",
        "knowledge_books",
        "knowledge_entries",
        "knowledge_activation_logs",
        "memory_profiles",
        "memory_records",
        "memory_jobs",
        "memory_embeddings",
        "transform_sets",
        "transform_rules",
        "interaction_rule_sets",
        "interaction_rules",
        "interaction_actions",
        "interaction_state",
        "interaction_proposals",
        "interaction_derived_event_outbox",
        "interaction_derived_event_quarantines",
        "interaction_derived_event_guard_audit",
        "content_modules",
        "content_module_components",
        "content_module_bindings",
    ] {
        assert_eq!(
            connection
                .query_row(
                    "SELECT COUNT(*) FROM sqlite_schema
                     WHERE type = 'table' AND name = ?1",
                    [table],
                    |row| row.get::<_, u32>(0),
                )
                .expect("table existence"),
            1,
            "missing required normalized table {table}"
        );
    }

    let foreign_key_violation = connection
        .query_row("PRAGMA foreign_key_check", [], |_| Ok(()))
        .optional()
        .expect("foreign-key check");
    assert!(foreign_key_violation.is_none());
    assert_eq!(
        connection
            .query_row("PRAGMA quick_check", [], |row| row.get::<_, String>(0))
            .expect("quick check"),
        "ok"
    );
}
