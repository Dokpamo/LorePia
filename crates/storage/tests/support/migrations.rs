use std::{
    fs,
    path::{Path, PathBuf},
};

use rusqlite::{Connection, OptionalExtension, params};

const APPLIED_AT: &str = "2026-08-03T00:00:00Z";

const MIGRATIONS: &[&str] = &[
    include_str!("../../migrations/0001_initial.sql"),
    include_str!("../../migrations/0002_import_asset_recovery.sql"),
    include_str!("../../migrations/0003_conversation_branches.sql"),
    include_str!("../../migrations/0004_provider_catalog.sql"),
    include_str!("../../migrations/0005_discovery_state_machine.sql"),
    include_str!("../../migrations/0006_generation_provider_provenance.sql"),
    include_str!("../../migrations/0007_signed_catalog_history.sql"),
    include_str!("../../migrations/0008_generation_protocol_state.sql"),
    include_str!("../../migrations/0009_model_sync_jobs.sql"),
    include_str!("../../migrations/0010_provider_connection_tombstones.sql"),
    include_str!("../../migrations/0011_provider_local_network_approvals.sql"),
    include_str!("../../migrations/0012_content_package_foundation.sql"),
    include_str!("../../migrations/0013_prompt_pipeline.sql"),
    include_str!("../../migrations/0014_knowledge.sql"),
    include_str!("../../migrations/0015_memory.sql"),
    include_str!("../../migrations/0016_transforms.sql"),
    include_str!("../../migrations/0017_interactions_modules.sql"),
    include_str!("../../migrations/0018_persona_selection.sql"),
    include_str!("../../migrations/0019_lifecycle_outbox.sql"),
    include_str!("../../migrations/0020_package_cas_promotion_journal.sql"),
    include_str!("../../migrations/0021_interaction_checkpoints.sql"),
    include_str!("../../migrations/0022_memory_vector_space.sql"),
    include_str!("../../migrations/0023_applied_module_runtime_plans.sql"),
    include_str!("../../migrations/0024_generation_attempt_proposals.sql"),
    include_str!("../../migrations/0025_conversation_greeting_bindings.sql"),
    include_str!("../../migrations/0026_provider_discovery_native_no_effect.sql"),
    include_str!("../../migrations/0027_provider_discovery_native_attestations.sql"),
    include_str!("../../migrations/0028_generation_attempt_storage_identities.sql"),
    include_str!("../../migrations/0029_generation_attempt_decision_handshake.sql"),
    include_str!("../../migrations/0030_package_document_target_reviews.sql"),
    include_str!("../../migrations/0031_message_display_projections.sql"),
    include_str!("../../migrations/0032_knowledge_vector_space.sql"),
    include_str!("../../migrations/0033_interaction_derived_event_outbox.sql"),
    include_str!("../../migrations/0034_generation_attempt_derived_event_authority.sql"),
    include_str!("../../migrations/0035_interaction_derived_event_quarantine.sql"),
    include_str!("../../migrations/0036_generation_attempt_derived_closure.sql"),
    include_str!("../../migrations/0037_provider_credential_operations.sql"),
    include_str!("../../migrations/0038_conversation_speakers.sql"),
    include_str!("../../migrations/0039_runtime_model_audit.sql"),
    include_str!("../../migrations/0040_portable_runtime_state.sql"),
    include_str!("../../migrations/0041_portable_runtime_package_capability.sql"),
    include_str!("../../migrations/0042_module_plan_documents.sql"),
];

pub(crate) fn expected_schema_version() -> u32 {
    u32::try_from(MIGRATIONS.len()).expect("migration count fits u32")
}

pub(crate) fn active_database_path(root: &Path) -> PathBuf {
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

pub(crate) fn apply_through(connection: &mut Connection, target: usize) {
    apply_range(connection, 0, target);
}

pub(crate) fn apply_range(connection: &mut Connection, start_exclusive: usize, target: usize) {
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
                params![version, APPLIED_AT],
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
