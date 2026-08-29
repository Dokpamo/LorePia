//! Regression coverage for populated schema-31 knowledge embeddings.

use std::{
    fs,
    path::{Path, PathBuf},
};

use lorepia_domain::{
    ActivationRule, AuxiliaryTaskKind, GenerationPresetId, KnowledgeBook, KnowledgeBookId,
    KnowledgeEntry, KnowledgeEntryId, KnowledgePlacement, ModelRouteId, Provenance, RateLimit,
    SourceKind, TaskProfile, TaskProfileId, TokenBudget, TokenPolicy,
};
use lorepia_storage::{KnowledgeEmbeddingQuery, Storage};
use rusqlite::{Connection, OptionalExtension, Transaction, params};
use sha2::{Digest, Sha256};
use tempfile::tempdir;

const NOW: &str = "2026-08-09T00:00:00Z";
const LEGACY_EMBEDDING_ID: &str = "legacy-knowledge-embedding";
const LEGACY_VECTOR_SPACE_SENTINEL: &str =
    "0000000000000000000000000000000000000000000000000000000000000000";
const EXACT_PROVIDER_VECTOR_SPACE: &str =
    "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const MODEL_ROUTE_ID: &str = "route:legacy-knowledge";
const GENERATION_PRESET_ID: &str = "preset:legacy-knowledge";
const ENTRY_ID: &str = "entry:legacy-knowledge";
const LAST_INVERTED_SCHEMA_VERSION: u32 = 39;
const MIGRATION_0019: &str = include_str!("../migrations/0019_lifecycle_outbox.sql");
const MIGRATION_0024: &str = include_str!("../migrations/0024_generation_attempt_proposals.sql");
const MIGRATION_0027: &str =
    include_str!("../migrations/0027_provider_discovery_native_attestations.sql");
const MIGRATION_0028: &str =
    include_str!("../migrations/0028_generation_attempt_storage_identities.sql");
const MIGRATION_0029: &str =
    include_str!("../migrations/0029_generation_attempt_decision_handshake.sql");
const MIGRATION_0037: &str = include_str!("../migrations/0037_provider_credential_operations.sql");
const MIGRATION_0038: &str = include_str!("../migrations/0038_conversation_speakers.sql");
const MIGRATION_0039: &str = include_str!("../migrations/0039_runtime_model_audit.sql");

#[derive(Debug)]
struct FixtureIds {
    source_database_path: PathBuf,
    current_schema_version: u32,
    task_profile_revision_id: String,
    book_revision_id: String,
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

#[derive(Debug, PartialEq, Eq)]
struct MigratedLegacyRow {
    id: String,
    book_revision_id: String,
    entry_id: String,
    task_profile_revision_id: String,
    model_route_id: String,
    dimensions: u32,
    vector_space_sha256: String,
    encoding: String,
    vector_blob: Vec<u8>,
    vector_sha256: String,
    created_at: String,
}

#[test]
fn populated_v31_knowledge_embedding_is_quarantined_and_reopen_is_idempotent() {
    let root = tempdir().expect("temporary storage root");
    let fixture = seed_current_dependencies(root.path());
    let expected_vector = encode_vector(&[1.0, 0.0, 0.0]);
    let expected_vector_sha256 = format!("{:x}", Sha256::digest(&expected_vector));
    downgrade_and_seed_populated_v31(&fixture, &expected_vector, &expected_vector_sha256);

    let storage =
        Storage::open(root.path()).expect("migrate populated schema 31 to current schema");
    assert_eq!(
        storage.schema_version().expect("schema version"),
        fixture.current_schema_version
    );
    assert_exact_provider_space_does_not_match(&storage, &fixture);
    drop(storage);

    let first = read_migrated_legacy_row(&active_database_path(root.path()));
    assert_migrated_legacy_row(&first, &fixture, &expected_vector, &expected_vector_sha256);
    assert_reopen_is_idempotent(root.path(), &fixture, &first);
}

fn assert_migrated_legacy_row(
    row: &MigratedLegacyRow,
    fixture: &FixtureIds,
    expected_vector: &[u8],
    expected_vector_sha256: &str,
) {
    assert_eq!(row.id, LEGACY_EMBEDDING_ID);
    assert_eq!(row.book_revision_id, fixture.book_revision_id);
    assert_eq!(row.entry_id, ENTRY_ID);
    assert_eq!(
        row.task_profile_revision_id,
        fixture.task_profile_revision_id
    );
    assert_eq!(row.model_route_id, MODEL_ROUTE_ID);
    assert_eq!(row.dimensions, 3);
    assert_eq!(row.vector_space_sha256, LEGACY_VECTOR_SPACE_SENTINEL);
    assert_eq!(row.encoding, "f32le");
    assert_eq!(row.vector_blob, expected_vector);
    assert_eq!(row.vector_sha256, expected_vector_sha256);
    assert_eq!(row.created_at, NOW);
}

fn assert_reopen_is_idempotent(root: &Path, fixture: &FixtureIds, first: &MigratedLegacyRow) {
    let reopened = Storage::open(root).expect("reopen migrated current schema");
    assert_eq!(
        reopened.schema_version().expect("reopened version"),
        fixture.current_schema_version
    );
    assert_exact_provider_space_does_not_match(&reopened, fixture);
    drop(reopened);

    let active_database_path = active_database_path(root);
    let reopened_row = read_migrated_legacy_row(&active_database_path);
    assert_eq!(
        &reopened_row, first,
        "a second reopen must not rewrite the quarantined legacy vector"
    );
    let connection = Connection::open(active_database_path).expect("inspect migration registry");
    assert_eq!(
        connection
            .query_row(
                "SELECT COUNT(*) FROM schema_migrations WHERE version = 32",
                [],
                |row| row.get::<_, u32>(0),
            )
            .expect("schema-32 registry count"),
        1,
        "schema 32 must be recorded exactly once"
    );
}

fn seed_current_dependencies(root: &Path) -> FixtureIds {
    let seed_root = tempdir().expect("temporary current-schema seed root");
    let storage = Storage::open(seed_root.path()).expect("create current schema");
    let current_schema_version = storage.schema_version().expect("current schema version");
    drop(storage);
    let seed_database_path = active_database_path(seed_root.path());
    seed_provider_graph(&seed_database_path);

    let storage = Storage::open(seed_root.path()).expect("open dependency storage");
    let task_profile_revision_id = seed_embedding_task(&storage);
    let book_revision_id = seed_knowledge_book(&storage);
    drop(storage);

    checkpoint_database(&seed_database_path);
    let source_database_path = root.join("db/lorepia.sqlite3");
    fs::create_dir_all(
        source_database_path
            .parent()
            .expect("legacy source database parent"),
    )
    .expect("create legacy source database directory");
    fs::copy(&seed_database_path, &source_database_path)
        .expect("copy current dependency database into legacy source root");

    FixtureIds {
        source_database_path,
        current_schema_version,
        task_profile_revision_id,
        book_revision_id,
    }
}

fn checkpoint_database(database_path: &Path) {
    let connection = Connection::open(database_path).expect("open seed database for checkpoint");
    let checkpoint = connection
        .query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |row| {
            Ok((
                row.get::<_, u32>(0)?,
                row.get::<_, u32>(1)?,
                row.get::<_, u32>(2)?,
            ))
        })
        .expect("checkpoint seed database");
    assert_eq!(checkpoint.0, 0, "seed database checkpoint was busy");
    assert_eq!(
        checkpoint.1, checkpoint.2,
        "seed database checkpoint left frames uncommitted"
    );
}

fn seed_provider_graph(database_path: &Path) {
    let connection = Connection::open(database_path).expect("open dependency fixture database");
    connection
        .pragma_update(None, "foreign_keys", true)
        .expect("enable foreign keys");
    let manifest_json = "{}";
    let manifest_sha256 = format!("{:x}", Sha256::digest(manifest_json.as_bytes()));
    connection
        .execute(
            "INSERT INTO provider_templates
             (id, version, display_name, source_kind, manifest_json,
              manifest_sha256, created_at)
             VALUES ('template:legacy-knowledge', 1, 'Legacy knowledge fixture',
                     'built_in', ?1, ?2, ?3)",
            params![manifest_json, manifest_sha256, NOW],
        )
        .expect("provider template");
    connection
        .execute(
            "INSERT INTO provider_connections
             (id, template_id, template_version, display_name, api_origin,
              config_json, credential_ref, credential_scope_json,
              timeout_seconds, status, created_at, updated_at)
             VALUES ('connection:legacy-knowledge', 'template:legacy-knowledge', 1,
                     'Legacy knowledge fixture', 'https://example.invalid', '{}',
                     NULL, NULL, 30, 'connected', ?1, ?1)",
            [NOW],
        )
        .expect("provider connection");
    connection
        .execute(
            "INSERT INTO provider_models
             (id, connection_id, api_family, model_id, display_name,
              route_json, availability, raw_metadata_json,
              first_seen_at, last_seen_at)
             VALUES (?1, 'connection:legacy-knowledge', 'openai_chat_completions',
                     'legacy-embedding-model', 'Legacy embedding model', '{}',
                     'available', NULL, ?2, ?2)",
            params![MODEL_ROUTE_ID, NOW],
        )
        .expect("provider model");
    connection
        .execute(
            "INSERT INTO generation_presets
             (id, model_route_id, display_name, values_json, created_at, updated_at)
             VALUES (?1, ?2, 'Legacy embedding preset', '{}', ?3, ?3)",
            params![GENERATION_PRESET_ID, MODEL_ROUTE_ID, NOW],
        )
        .expect("generation preset");
}

fn seed_embedding_task(storage: &Storage) -> String {
    let task_profile = TaskProfile {
        id: TaskProfileId::from("task:legacy-knowledge-embedding"),
        kind: AuxiliaryTaskKind::MemoryEmbedding,
        route_id: ModelRouteId::from(MODEL_ROUTE_ID),
        generation_preset_id: GenerationPresetId::from(GENERATION_PRESET_ID),
        fallback_route_ids: Vec::new(),
        embedding_dimensions: Some(3),
        timeout_ms: 30_000,
        rate_limit: RateLimit {
            requests: 2,
            per_seconds: 60,
        },
        concurrency_limit: 1,
    };
    storage
        .save_task_profile(&task_profile, None)
        .expect("embedding task profile")
        .revision_id
        .expect("embedding task revision id")
}

fn seed_knowledge_book(storage: &Storage) -> String {
    let book_id = KnowledgeBookId::from("book:legacy-knowledge");
    let provenance = fixture_provenance();
    let book = KnowledgeBook {
        id: book_id.clone(),
        name: "Legacy knowledge fixture".to_owned(),
        schema_version: 1,
        entries: vec![KnowledgeEntry {
            id: KnowledgeEntryId::from(ENTRY_ID),
            book_id,
            name: "Legacy entry".to_owned(),
            content: "Project-owned synthetic legacy knowledge.".to_owned(),
            enabled: true,
            activation: ActivationRule::Always,
            priority: 1,
            importance: 50,
            placement: KnowledgePlacement::RetrievedContext,
            token_policy: TokenPolicy {
                priority: 100,
                min_tokens: None,
                max_tokens: Some(64),
                reserve_tokens: None,
            },
            parent_id: None,
            activation_probability_basis_points: 10_000,
            provenance: provenance.clone(),
        }],
        scan_depth: 8,
        token_budget: TokenBudget { max_tokens: 256 },
        recursive: false,
        max_recursion_depth: 0,
        provenance,
    };
    storage
        .save_knowledge_book(&book, None)
        .expect("knowledge book")
        .revision_id
        .expect("knowledge book revision id")
}

fn downgrade_and_seed_populated_v31(fixture: &FixtureIds, vector_blob: &[u8], vector_sha256: &str) {
    assert_eq!(
        fixture.current_schema_version, LAST_INVERTED_SCHEMA_VERSION,
        "extend the synthetic schema-31 inverse for every new migration"
    );
    let mut connection =
        Connection::open(&fixture.source_database_path).expect("open schema downgrade fixture");
    connection
        .pragma_update(None, "foreign_keys", true)
        .expect("enable foreign keys");
    let transaction = connection
        .transaction()
        .expect("schema downgrade transaction");
    remove_schema39_objects(&transaction);
    remove_schema38_objects(&transaction);
    remove_schema37_objects(&transaction);
    remove_schema36_objects(&transaction);
    remove_post_v32_schema(&transaction);
    restore_v31_knowledge_embedding_table(&transaction);
    insert_populated_v31_embedding(&transaction, fixture, vector_blob, vector_sha256);
    assert_schema31_fixture(&transaction);
    transaction.commit().expect("commit populated schema 31");
}

fn remove_schema39_objects(transaction: &Transaction<'_>) {
    for (object_type, name) in created_objects(MIGRATION_0039) {
        transaction
            .execute(&format!("DROP {object_type} \"{name}\""), [])
            .unwrap_or_else(|error| panic!("remove schema-39 {object_type} {name}: {error}"));
    }
}

fn remove_schema38_objects(transaction: &Transaction<'_>) {
    for (object_type, name) in created_objects(MIGRATION_0038) {
        transaction
            .execute(&format!("DROP {object_type} \"{name}\""), [])
            .unwrap_or_else(|error| panic!("remove schema-38 {object_type} {name}: {error}"));
    }
}

/// Additive objects a migration creates, innermost first, for inverse fixtures.
fn created_objects(migration: &str) -> Vec<(&str, &str)> {
    let mut objects = migration
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
            Some((object_type, name.trim_end_matches(';')))
        })
        .collect::<Vec<_>>();
    objects.sort_by_key(|(object_type, _)| match *object_type {
        "VIEW" => 0,
        "TRIGGER" => 1,
        "INDEX" => 2,
        _ => 3,
    });
    objects
}

fn remove_schema37_objects(transaction: &Transaction<'_>) {
    restore_schema35_trigger(
        transaction,
        MIGRATION_0027,
        "provider_discovery_native_no_effect_attestation_binding",
    );
    restore_schema35_trigger(
        transaction,
        MIGRATION_0027,
        "provider_discovery_operation_legal_transition",
    );
    let replaced_objects = MIGRATION_0037
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
    let created_objects = MIGRATION_0037
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
        created_objects.contains(&("TABLE", "provider_credential_ownership_events")),
        "schema-37 inverse must track every additive credential-journal object"
    );
    for object_type in ["VIEW", "TRIGGER", "INDEX", "TABLE"] {
        for (_, name) in created_objects
            .iter()
            .rev()
            .filter(|(candidate_type, _)| *candidate_type == object_type)
        {
            transaction
                .execute(&format!("DROP {object_type} \"{name}\""), [])
                .unwrap_or_else(|error| panic!("remove schema-37 {object_type} {name}: {error}"));
        }
    }
}

fn remove_schema36_objects(transaction: &Transaction<'_>) {
    restore_schema35_trigger(
        transaction,
        MIGRATION_0019,
        "generation_attempt_intents_transition_guard",
    );
    restore_schema35_trigger(
        transaction,
        MIGRATION_0029,
        "generation_attempt_aggregate_insert_guard_v2",
    );
    restore_schema35_trigger(
        transaction,
        MIGRATION_0028,
        "generation_attempt_proposals_transition_guard",
    );
    restore_schema35_trigger(
        transaction,
        MIGRATION_0024,
        "generation_attempt_aggregate_update_guard",
    );
    restore_schema35_trigger(
        transaction,
        MIGRATION_0029,
        "generation_attempt_decision_binding_insert_guard",
    );
    restore_schema35_trigger(
        transaction,
        MIGRATION_0029,
        "generation_attempt_decision_commit_insert_guard",
    );
    restore_schema35_trigger(
        transaction,
        MIGRATION_0029,
        "generation_attempt_proposal_decision_commit",
    );
    restore_schema35_trigger(
        transaction,
        MIGRATION_0029,
        "generation_attempt_aggregate_decision_bind",
    );
    drop_schema36_only_objects(transaction);
    drop_schema36_columns(transaction);
}

fn restore_schema35_trigger(transaction: &Transaction<'_>, migration: &str, trigger_name: &str) {
    transaction
        .execute_batch(&format!("DROP TRIGGER {trigger_name};"))
        .unwrap_or_else(|error| panic!("drop schema-36 trigger {trigger_name}: {error}"));
    let marker = format!("CREATE TRIGGER {trigger_name}\n");
    let start = migration
        .find(&marker)
        .unwrap_or_else(|| panic!("find schema-35 trigger {trigger_name}"));
    let tail = &migration[start..];
    let end = tail.find("\nEND;").map_or_else(
        || panic!("find end of schema-35 trigger {trigger_name}"),
        |offset| offset + "\nEND;".len(),
    );
    transaction
        .execute_batch(&tail[..end])
        .unwrap_or_else(|error| panic!("restore schema-35 trigger {trigger_name}: {error}"));
}

fn drop_schema36_only_objects(transaction: &Transaction<'_>) {
    transaction
        .execute_batch(
            "DROP TRIGGER generation_attempt_before_closure_insert_guard;
             DROP TRIGGER generation_attempt_prompt_selection_insert_guard;
             DROP TRIGGER generation_attempt_prompt_selection_update_guard;
             DROP TRIGGER generation_attempt_module_runtime_authority_insert_guard;
             DROP TRIGGER generation_attempt_module_runtime_authority_update_guard;
             DROP TRIGGER generation_attempt_proposal_origin_insert_guard;
             DROP TRIGGER interaction_event_evaluation_seal_insert_guard;
             DROP TRIGGER interaction_derived_event_outbox_seal_insert_guard;
             DROP TRIGGER generation_attempt_legacy_closure_interruption_no_update;
             DROP TRIGGER generation_attempt_legacy_closure_interruption_no_delete;
             DROP TABLE generation_attempt_legacy_closure_interruptions;",
        )
        .expect("remove schema-36-only tables and triggers");
}

fn drop_schema36_columns(transaction: &Transaction<'_>) {
    transaction
        .execute_batch(
            "ALTER TABLE generation_attempt_intents
                 DROP COLUMN module_runtime_authority_version;
             ALTER TABLE generation_attempt_intents
                 DROP COLUMN applied_runtime_plan_authority_sha256;
             ALTER TABLE generation_attempt_intents
                 DROP COLUMN applied_runtime_plan_authority_json;
             ALTER TABLE generation_attempt_intents
                 DROP COLUMN module_runtime_review_authority_sha256;
             ALTER TABLE generation_attempt_intents
                 DROP COLUMN module_runtime_review_authority_json;
             ALTER TABLE generation_attempt_intents
                 DROP COLUMN prompt_selection_authority_version;
             ALTER TABLE generation_attempt_intents
                 DROP COLUMN prompt_selection_authority_sha256;
             ALTER TABLE generation_attempt_intents
                 DROP COLUMN prompt_selection_authority_json;
             ALTER TABLE generation_attempt_before_event_snapshots
                 DROP COLUMN closure_authority_version;
             ALTER TABLE generation_attempt_before_event_snapshots
                 DROP COLUMN derived_closure_sha256;
             ALTER TABLE generation_attempt_before_event_snapshots
                 DROP COLUMN derived_closure_json;
             ALTER TABLE generation_attempt_before_event_snapshots
                 DROP COLUMN evaluation_seal_sha256;
             ALTER TABLE generation_attempt_before_event_snapshots
                 DROP COLUMN evaluation_seal_json;
             ALTER TABLE generation_attempt_interaction_aggregates
                 DROP COLUMN closure_authority_version;
             ALTER TABLE generation_attempt_interaction_aggregates
                 DROP COLUMN derived_guard_count;
             ALTER TABLE generation_attempt_interaction_aggregates
                 DROP COLUMN derived_event_count;
             ALTER TABLE generation_attempt_interaction_aggregates
                 DROP COLUMN evaluation_seal_sha256;
             ALTER TABLE generation_attempt_interaction_aggregates
                 DROP COLUMN derived_chain_sha256;
             ALTER TABLE generation_attempt_proposals
                 DROP COLUMN resulting_pending_proposal_count;
             ALTER TABLE generation_attempt_proposals
                 DROP COLUMN resulting_derived_guard_count;
             ALTER TABLE generation_attempt_proposals
                 DROP COLUMN resulting_derived_event_count;
             ALTER TABLE generation_attempt_proposals
                 DROP COLUMN resulting_derived_chain_sha256;
             ALTER TABLE generation_attempt_proposals
                 DROP COLUMN origin_evaluation_seal_sha256;
             ALTER TABLE generation_attempt_proposals
                 DROP COLUMN origin_evaluation_seal_json;
             ALTER TABLE generation_attempt_proposals
                 DROP COLUMN origin_aggregate_revision;
             ALTER TABLE generation_attempt_proposals
                 DROP COLUMN origin_chain_ordinal;
             ALTER TABLE generation_attempt_proposals
                 DROP COLUMN origin_event_id;
             ALTER TABLE generation_attempt_aggregate_decision_bindings
                 DROP COLUMN resulting_pending_proposal_count;
             ALTER TABLE generation_attempt_aggregate_decision_bindings
                 DROP COLUMN resulting_derived_guard_count;
             ALTER TABLE generation_attempt_aggregate_decision_bindings
                 DROP COLUMN resulting_derived_event_count;
             ALTER TABLE generation_attempt_aggregate_decision_bindings
                 DROP COLUMN resulting_derived_chain_sha256;
             ALTER TABLE generation_attempt_proposal_decision_commits
                 DROP COLUMN resulting_pending_proposal_count;
             ALTER TABLE generation_attempt_proposal_decision_commits
                 DROP COLUMN resulting_derived_guard_count;
             ALTER TABLE generation_attempt_proposal_decision_commits
                 DROP COLUMN resulting_derived_event_count;
             ALTER TABLE generation_attempt_proposal_decision_commits
                 DROP COLUMN resulting_derived_chain_sha256;
             ALTER TABLE interaction_events DROP COLUMN evaluation_seal_version;
             ALTER TABLE interaction_events DROP COLUMN evaluation_seal_sha256;
             ALTER TABLE interaction_events DROP COLUMN evaluation_seal_json;",
        )
        .expect("remove schema-36 columns");
}

fn remove_post_v32_schema(transaction: &Transaction<'_>) {
    transaction
        .execute_batch(
            "DROP TRIGGER interaction_derived_event_quarantine_claim_guard;
             DROP TRIGGER interaction_derived_event_quarantine_no_update;
             DROP TRIGGER interaction_derived_event_quarantine_no_delete;
             DROP TABLE interaction_derived_event_quarantines;
             DROP TRIGGER interaction_derived_event_outbox_identity_guard;
             DROP TRIGGER interaction_derived_event_outbox_transition_guard;
             DROP TRIGGER interaction_derived_event_guard_audit_no_update;
             DROP TRIGGER interaction_derived_event_guard_audit_no_delete;
             DROP INDEX interaction_derived_event_outbox_causal_delivery;
             DROP TABLE interaction_derived_event_guard_audit;
             DROP TABLE interaction_derived_event_outbox;
             ALTER TABLE generation_attempt_before_event_snapshots
                 DROP COLUMN derived_events_sha256;
             ALTER TABLE generation_attempt_before_event_snapshots
                 DROP COLUMN derived_events_json;",
        )
        .expect("remove schema-33 through schema-35 objects");
}

fn restore_v31_knowledge_embedding_table(transaction: &Transaction<'_>) {
    assert_eq!(
        transaction
            .query_row("SELECT COUNT(*) FROM knowledge_embeddings", [], |row| {
                row.get::<_, u32>(0)
            })
            .expect("current knowledge embedding count"),
        0,
        "the synthetic downgrade fixture must not discard an embedding"
    );
    transaction
        .execute_batch(
            "DROP TRIGGER knowledge_embeddings_no_update;
             DROP TRIGGER knowledge_embeddings_no_delete;
             DROP INDEX knowledge_embeddings_entry;
             DROP INDEX knowledge_embeddings_exact_space;
             DROP TABLE knowledge_embeddings;

             CREATE TABLE knowledge_embeddings (
                 id TEXT PRIMARY KEY NOT NULL CHECK (length(trim(id)) > 0),
                 book_revision_id TEXT NOT NULL,
                 entry_id TEXT NOT NULL,
                 task_profile_revision_id TEXT
                     REFERENCES task_profile_revisions(revision_id)
                     ON UPDATE RESTRICT ON DELETE RESTRICT,
                 model_route_id TEXT NOT NULL
                     REFERENCES provider_models(id)
                     ON UPDATE RESTRICT ON DELETE RESTRICT,
                 dimensions INTEGER NOT NULL CHECK (dimensions BETWEEN 1 AND 1048576),
                 encoding TEXT NOT NULL CHECK (encoding = 'f32le'),
                 vector_blob BLOB NOT NULL CHECK (length(vector_blob) = dimensions * 4),
                 vector_sha256 TEXT NOT NULL CHECK (
                     length(vector_sha256) = 64
                     AND vector_sha256 NOT GLOB '*[^0-9a-f]*'
                 ),
                 created_at TEXT NOT NULL CHECK (length(trim(created_at)) > 0),
                 FOREIGN KEY (book_revision_id, entry_id)
                     REFERENCES knowledge_entries(book_revision_id, entry_id)
                     ON UPDATE RESTRICT ON DELETE RESTRICT,
                 UNIQUE (book_revision_id, entry_id, model_route_id, dimensions, vector_sha256)
             );
             CREATE INDEX knowledge_embeddings_entry
                 ON knowledge_embeddings(book_revision_id, entry_id, model_route_id, id);
             CREATE TRIGGER knowledge_embeddings_no_update
             BEFORE UPDATE ON knowledge_embeddings
             BEGIN
                 SELECT RAISE(ABORT, 'knowledge embeddings are immutable');
             END;
             CREATE TRIGGER knowledge_embeddings_no_delete
             BEFORE DELETE ON knowledge_embeddings
             BEGIN
                 SELECT RAISE(ABORT, 'knowledge embeddings are immutable');
             END;
             DELETE FROM schema_migrations WHERE version > 31;",
        )
        .expect("restore schema-31 knowledge embedding table");
}

fn insert_populated_v31_embedding(
    transaction: &Transaction<'_>,
    fixture: &FixtureIds,
    vector_blob: &[u8],
    vector_sha256: &str,
) {
    transaction
        .execute(
            "INSERT INTO knowledge_embeddings
             (id, book_revision_id, entry_id, task_profile_revision_id,
              model_route_id, dimensions, encoding, vector_blob,
              vector_sha256, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, 3, 'f32le', ?6, ?7, ?8)",
            params![
                LEGACY_EMBEDDING_ID,
                fixture.book_revision_id,
                ENTRY_ID,
                fixture.task_profile_revision_id,
                MODEL_ROUTE_ID,
                vector_blob,
                vector_sha256,
                NOW,
            ],
        )
        .expect("insert populated legacy knowledge embedding");
}

fn assert_schema31_fixture(transaction: &Transaction<'_>) {
    assert_eq!(
        transaction
            .query_row("SELECT MAX(version) FROM schema_migrations", [], |row| {
                row.get::<_, u32>(0)
            })
            .expect("schema-31 registry version"),
        31
    );
    assert_eq!(
        transaction
            .query_row(
                "SELECT COUNT(*) FROM pragma_table_info('knowledge_embeddings')
                 WHERE name = 'vector_space_sha256'",
                [],
                |row| row.get::<_, u32>(0),
            )
            .expect("legacy vector-space column count"),
        0
    );
    assert!(
        transaction
            .query_row("PRAGMA foreign_key_check", [], |_| Ok(()))
            .optional()
            .expect("legacy foreign-key check")
            .is_none()
    );
}

fn assert_exact_provider_space_does_not_match(storage: &Storage, fixture: &FixtureIds) {
    let matches = storage
        .query_knowledge_embeddings_cosine(&KnowledgeEmbeddingQuery {
            book_revision_id: fixture.book_revision_id.clone(),
            task_profile_revision_id: fixture.task_profile_revision_id.clone(),
            model_route_id: ModelRouteId::from(MODEL_ROUTE_ID),
            dimensions: 3,
            vector_space_sha256: EXACT_PROVIDER_VECTOR_SPACE.to_owned(),
            values: vec![1.0, 0.0, 0.0],
        })
        .expect("query exact provider vector space");
    assert!(
        matches.is_empty(),
        "a migrated legacy vector must not match an exact provider space"
    );
}

fn read_migrated_legacy_row(database_path: &Path) -> MigratedLegacyRow {
    let connection = Connection::open(database_path).expect("inspect migrated legacy row");
    connection
        .query_row(
            "SELECT id, book_revision_id, entry_id, task_profile_revision_id,
                    model_route_id, dimensions, vector_space_sha256, encoding,
                    vector_blob, vector_sha256, created_at
             FROM knowledge_embeddings
             WHERE id = ?1",
            [LEGACY_EMBEDDING_ID],
            |row| {
                Ok(MigratedLegacyRow {
                    id: row.get(0)?,
                    book_revision_id: row.get(1)?,
                    entry_id: row.get(2)?,
                    task_profile_revision_id: row.get(3)?,
                    model_route_id: row.get(4)?,
                    dimensions: row.get(5)?,
                    vector_space_sha256: row.get(6)?,
                    encoding: row.get(7)?,
                    vector_blob: row.get(8)?,
                    vector_sha256: row.get(9)?,
                    created_at: row.get(10)?,
                })
            },
        )
        .expect("migrated legacy knowledge embedding")
}

fn encode_vector(values: &[f32]) -> Vec<u8> {
    values
        .iter()
        .flat_map(|value| value.to_le_bytes())
        .collect()
}

fn fixture_provenance() -> Provenance {
    Provenance {
        source_kind: SourceKind::UserCreated,
        source_id: Some("synthetic.knowledge-embedding-migration".to_owned()),
        source_hash: None,
        author: None,
        license: None,
        imported_at: None,
    }
}
