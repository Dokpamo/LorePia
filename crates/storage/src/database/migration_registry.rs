use super::schema::FROZEN_NATIVE_SCHEMA_VERSION;

pub(super) const MIGRATION_0001: &str = include_str!("../../migrations/0001_initial.sql");
pub(super) const MIGRATION_0002: &str =
    include_str!("../../migrations/0002_import_asset_recovery.sql");
pub(super) const MIGRATION_0003: &str =
    include_str!("../../migrations/0003_conversation_branches.sql");
pub(super) const MIGRATION_0004: &str = include_str!("../../migrations/0004_provider_catalog.sql");
pub(super) const MIGRATION_0005: &str = crate::discovery::DISCOVERY_STATE_MACHINE_MIGRATION;
pub(super) const MIGRATION_0006: &str =
    include_str!("../../migrations/0006_generation_provider_provenance.sql");
pub(super) const MIGRATION_0007: &str = crate::catalog::SIGNED_CATALOG_HISTORY_MIGRATION;
pub(super) const MIGRATION_0008: &str =
    include_str!("../../migrations/0008_generation_protocol_state.sql");
pub(super) const MIGRATION_0009: &str = include_str!("../../migrations/0009_model_sync_jobs.sql");
pub(super) const MIGRATION_0010: &str =
    include_str!("../../migrations/0010_provider_connection_tombstones.sql");
pub(super) const MIGRATION_0011: &str =
    include_str!("../../migrations/0011_provider_local_network_approvals.sql");
pub(crate) const FROZEN_NATIVE_MIGRATIONS: [&str; FROZEN_NATIVE_SCHEMA_VERSION as usize] = [
    MIGRATION_0001,
    MIGRATION_0002,
    MIGRATION_0003,
    MIGRATION_0004,
    MIGRATION_0005,
    MIGRATION_0006,
    MIGRATION_0007,
    MIGRATION_0008,
    MIGRATION_0009,
    MIGRATION_0010,
    MIGRATION_0011,
];
pub(super) const MIGRATION_0012: &str =
    include_str!("../../migrations/0012_content_package_foundation.sql");
pub(super) const MIGRATION_0013: &str = include_str!("../../migrations/0013_prompt_pipeline.sql");
pub(super) const MIGRATION_0014: &str = include_str!("../../migrations/0014_knowledge.sql");
pub(super) const MIGRATION_0015: &str = include_str!("../../migrations/0015_memory.sql");
pub(super) const MIGRATION_0016: &str = include_str!("../../migrations/0016_transforms.sql");
pub(super) const MIGRATION_0017: &str =
    include_str!("../../migrations/0017_interactions_modules.sql");
pub(super) const MIGRATION_0018: &str = include_str!("../../migrations/0018_persona_selection.sql");
pub(super) const MIGRATION_0019: &str = include_str!("../../migrations/0019_lifecycle_outbox.sql");
pub(super) const MIGRATION_0020: &str =
    include_str!("../../migrations/0020_package_cas_promotion_journal.sql");
pub(super) const MIGRATION_0021: &str =
    include_str!("../../migrations/0021_interaction_checkpoints.sql");
pub(super) const MIGRATION_0022: &str =
    include_str!("../../migrations/0022_memory_vector_space.sql");
pub(super) const MIGRATION_0023: &str =
    include_str!("../../migrations/0023_applied_module_runtime_plans.sql");
pub(super) const MIGRATION_0024: &str =
    include_str!("../../migrations/0024_generation_attempt_proposals.sql");
pub(super) const MIGRATION_0025: &str =
    include_str!("../../migrations/0025_conversation_greeting_bindings.sql");
pub(super) const MIGRATION_0026: &str =
    include_str!("../../migrations/0026_provider_discovery_native_no_effect.sql");
pub(super) const MIGRATION_0027: &str =
    include_str!("../../migrations/0027_provider_discovery_native_attestations.sql");
pub(super) const MIGRATION_0028: &str =
    include_str!("../../migrations/0028_generation_attempt_storage_identities.sql");
pub(super) const MIGRATION_0029: &str =
    include_str!("../../migrations/0029_generation_attempt_decision_handshake.sql");
pub(super) const MIGRATION_0030: &str =
    include_str!("../../migrations/0030_package_document_target_reviews.sql");
pub(super) const MIGRATION_0031: &str =
    include_str!("../../migrations/0031_message_display_projections.sql");
pub(super) const MIGRATION_0032: &str =
    include_str!("../../migrations/0032_knowledge_vector_space.sql");
pub(super) const MIGRATION_0033: &str =
    include_str!("../../migrations/0033_interaction_derived_event_outbox.sql");
pub(super) const MIGRATION_0034: &str =
    include_str!("../../migrations/0034_generation_attempt_derived_event_authority.sql");
pub(super) const MIGRATION_0035: &str =
    include_str!("../../migrations/0035_interaction_derived_event_quarantine.sql");
pub(super) const MIGRATION_0036: &str =
    include_str!("../../migrations/0036_generation_attempt_derived_closure.sql");
pub(super) const MIGRATION_0037: &str =
    include_str!("../../migrations/0037_provider_credential_operations.sql");
pub(super) const MIGRATION_0038: &str =
    include_str!("../../migrations/0038_conversation_speakers.sql");
pub(super) const MIGRATION_0039: &str =
    include_str!("../../migrations/0039_runtime_model_audit.sql");
pub(super) const MIGRATION_0040: &str =
    include_str!("../../migrations/0040_portable_runtime_state.sql");
