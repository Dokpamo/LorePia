use std::collections::{BTreeMap, BTreeSet, HashMap};

use chrono::{DateTime, Utc};
use lorepia_domain::{
    ApiFamily, AppSettings, AssetDescriptor, AssetRole, BlockSource, CharacterContentV1,
    CharacterField, ContentModule, ContentModuleId, ContentModuleRevision, ControlSpec,
    ConversationBranchId, ConversationId, GenerationId, GenerationPresetId,
    GenerationReasoningEffort, InteractionRuleSet, InteractionRuleSetId, KnowledgeActivationReason,
    KnowledgeBook, KnowledgeBookId, KnowledgeEntryId, MAX_BLOCK_TEXT_CHARS, MAX_MEMORY_RECORDS,
    MAX_NAME_CHARS, MAX_PROMPT_BLOCKS, MemoryJob, MemoryJobId, MemoryJobStatus, MemoryProfile,
    MemoryProfileId, MemoryRecord, MemoryRecordId, MergePolicy, MessageId, ModelRouteId,
    ModuleBinding, ModuleBindingId, ModuleRevisionId, ModuleScope, OverflowPolicy, PackageId,
    Persona, PersonaId, PlacementZone, PresetMetadata, PromptBlock, PromptBlockId, PromptBlockKind,
    PromptContextBindingEvidence, PromptContextPersonaEvidence, PromptContextSnapshotV1,
    PromptPreset, PromptPresetId, Provenance, ProviderMessageRole, ResolvedPromptPlan, RoleHint,
    SafeTemplate, Sha256Digest, SourceKind, TaskProfile, TaskProfileId, TemplatePart, TemplateSlot,
    TokenPolicy, TransformSet, TransformSetId, ValidateOrchestration, VariableMap, VersionedJson,
    prompt_context_snapshot_sha256,
};
#[cfg(test)]
use lorepia_domain::{LocalUserId, PromptSummarySourceEvidence, prompt_local_user_id_sha256};
use rusqlite::{Connection, OptionalExtension, Transaction, TransactionBehavior, params};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use serde_json::Value;
use sha2::{Digest, Sha256};
use uuid::Uuid;

use crate::database::{Storage, storage_db_error};
use crate::package_repository::VerifiedCompletedPackageAuthorities;
use lorepia_domain::{CoreError, CoreErrorCode, CoreResult};

mod builtins;
mod catalog_documents;
mod content_module_projection;
mod content_module_revisions;
mod document_history;
mod document_store;
mod generation_prompt;
mod knowledge_memory_profiles;
mod knowledge_memory_runtime;
mod memory_context;
mod memory_records;
mod memory_visibility;
mod module_activation;
mod module_bindings;
mod module_runtime;
mod package_commit_bridge;
mod prompt_bindings;
mod prompt_preset_projection;
mod prompt_preset_rollback;
mod transforms_interactions;

use content_module_projection::write_content_module_projection;
use content_module_revisions::load_content_module_revision;
use document_history::{
    DocumentTable, RawStoredDocument, RevisionEventKind, RevisionWrite, decode_document,
    decode_stored_document, diff_content_object_revisions, diff_prompt_preset_revision_documents,
    document_provenance, document_schema_version, encode_document, get_object_revision,
    i64_revision, list_object_revisions, load_exact_content_revision, not_found, parse_datetime,
    prompt_preset_diff_from_revisions, revision_conflict, revision_diff_json,
    rollback_content_object, sha256_hex, source_kind_str, storage_corrupted, u64_revision,
    validate_identifier, validate_json_bounds, validate_optional_sha256,
};
use document_store::{
    active_content_revision_id, append_content_revision, content_revision_no,
    content_revision_number, get_document, list_documents, list_documents_page,
    persona_catalog_revision, save_content_object, soft_delete_content_object,
};
pub use generation_prompt::OrchestrationDatabaseStats;
use generation_prompt::nonnegative_u32;
pub(crate) use generation_prompt::{GenerationPromptPlanLink, write_generation_prompt_plan};
#[cfg(test)]
use knowledge_memory_profiles::{
    ensure_memory_summary_schema_for_test as ensure_memory_summary_schema,
    weight_millionths_for_test as weight_millionths,
    write_knowledge_entries_for_test as write_knowledge_entries,
};
use knowledge_memory_profiles::{write_knowledge_book_projection, write_memory_profile_projection};
use knowledge_memory_runtime::{
    load_generation_module_plan_evidence, write_generation_knowledge_logs,
};
pub use memory_context::memory_records_at_head_snapshot_sha256;
#[cfg(test)]
pub(crate) use memory_context::require_generation_prompt_context_snapshot_transaction;
pub(crate) use memory_context::{
    SealedGenerationPromptContext, require_memory_records_at_head_snapshot_transaction,
    require_sealed_generation_prompt_context_snapshot_transaction,
};
use memory_records::{
    RawMemoryRecord, append_memory_event, decode_memory_record, raw_memory_record,
};
pub(crate) use memory_visibility::invalidate_memory_range_in_transaction;
use memory_visibility::{memory_records_at_head_in_connection, prompt_context_changed};
use module_bindings::{
    insert_module_activation_audit, list_all_module_bindings_transaction,
    module_activation_resolution_set, module_binding_row, module_binding_targets,
    module_component_storage_key, resolve_module_binding_revision,
    stale_affected_module_activation_plans, write_module_binding_transaction,
};
use module_runtime::load_applied_module_runtime_plan_transaction;
pub(crate) use module_runtime::persist_applied_module_runtime_plan_transaction;
#[cfg(test)]
use package_commit_bridge::character_content_metadata_json;
pub(crate) use package_commit_bridge::{
    append_package_asset_descriptor, append_package_commit_document,
    write_imported_character_content,
};
use package_commit_bridge::{character_content_object_id, write_character_content_projection};
#[cfg(test)]
use prompt_bindings::{prompt_binding_targets_for_test, validate_prompt_binding_context_for_test};
use prompt_preset_projection::{
    validate_prompt_preset_storage_shape, write_prompt_preset_projection,
};
use prompt_preset_rollback::{apply_prompt_preset_rollback, review_prompt_preset_rollback};
use transforms_interactions::{
    write_interaction_rule_set_projection, write_transform_set_projection,
};
mod module_authority;
#[cfg(test)]
mod tests;

pub use builtins::built_in_prompt_presets;
pub(crate) use builtins::seed_builtin_prompt_presets;
use module_authority::module_activation_snapshots;
pub(crate) use module_authority::{
    validate_fresh_module_merge_review, verify_module_import_authorities,
};
pub use prompt_bindings::{PromptPresetBinding, PromptResponseLength};

/// Largest canonical JSON document accepted by the orchestration repository.
///
/// Large content payloads and assets remain in content-addressed storage. The
/// relational layer stores bounded metadata and declarative configuration.
pub const MAX_ORCHESTRATION_JSON_BYTES: usize = 2 * 1024 * 1024;
pub const MAX_ORCHESTRATION_JSON_CHARS: usize = 1_000_000;
pub const MAX_ORCHESTRATION_JSON_DEPTH: usize = 32;
pub const MAX_ORCHESTRATION_JSON_NODES: usize = 100_000;
const MAX_CHARACTER_CONTENT_JSON_BYTES: usize = 8 * 1024 * 1024;
const MAX_CHARACTER_CONTENT_JSON_CHARS: usize = 4_000_000;
const MAX_CHARACTER_CONTENT_JSON_NODES: usize = 250_000;
pub const MAX_MEMORY_EMBEDDING_DIMENSIONS: usize = 32_768;

const BUILTIN_CHAT_PRESET_ID: &str = "lorepia.builtin.chat-compatible.v1";
const BUILTIN_STORY_PRESET_ID: &str = "lorepia.builtin.story-compatible.v1";

/// A typed object together with its compare-and-swap storage revision.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StoredRevision<T> {
    pub value: T,
    pub revision: u64,
    /// Exact immutable content revision when the value is backed by the
    /// generic content registry. Mutable binding/job records have no immutable
    /// content revision and return `None`.
    #[serde(default)]
    pub revision_id: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub deleted_at: Option<DateTime<Utc>>,
}

/// One read of the mutable persona catalog, guarded by its exact active-state
/// digest. A stale continuation never returns a partial page; callers must
/// restart from the first page under the returned current revision.
#[derive(Debug, Clone, PartialEq)]
pub enum PersonaCatalogPage {
    Page {
        catalog_revision: Sha256Digest,
        items: Vec<StoredRevision<Persona>>,
    },
    RestartRequired {
        current_catalog_revision: Sha256Digest,
    },
}

/// Exact durable authority returned only for response-loss recovery of an
/// already-applied module activation.
#[derive(Debug, Clone, PartialEq)]
pub struct RecoveredModuleActivation {
    pub review: lorepia_orchestration::ModuleActivationReview,
    pub approved: lorepia_orchestration::ApprovedModuleActivationPlan,
    pub binding: StoredRevision<ModuleBinding>,
}

/// Exact durable authority returned only for response-loss recovery of an
/// already-applied module rollback.
///
/// Unlike an ordinary activation recovery, this includes the rollback-only
/// plan and approval digest recovered from the append-only activation audit.
/// That distinction prevents an otherwise similar pinned activation from
/// being accepted as a successful rollback retry.
#[derive(Debug, Clone, PartialEq)]
pub struct RecoveredModuleRollback {
    pub approved: lorepia_orchestration::ApprovedModuleRollbackPlan,
    pub binding: StoredRevision<ModuleBinding>,
}

/// Auditable evidence for one knowledge-selection decision.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct KnowledgeActivationLog {
    pub id: String,
    pub book_id: KnowledgeBookId,
    /// Immutable revision selected while the prompt was prepared. Ordinary
    /// selections must still match the active revision at atomic append time.
    /// A historical revision is accepted only when the sealed generation
    /// evidence names an applied module plan whose exact component selects it.
    pub book_revision_id: String,
    pub entry_id: KnowledgeEntryId,
    pub conversation_id: ConversationId,
    pub branch_id: ConversationBranchId,
    pub selected: bool,
    pub reasons: Vec<KnowledgeActivationReason>,
    pub estimated_tokens: u32,
    pub exclusion_reason: Option<String>,
    pub created_at: DateTime<Utc>,
}

/// Bounded embedding payload associated with exactly one memory record.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MemoryEmbeddingRecord {
    pub id: String,
    pub memory_record_id: MemoryRecordId,
    pub model_route_id: Option<ModelRouteId>,
    pub dimensions: u32,
    pub values: Vec<f32>,
    pub created_at: DateTime<Utc>,
}

/// One immutable memory-record identity visible at an exact message head.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MemoryRecordAtHeadEvidence {
    pub record_id: MemoryRecordId,
    pub record_branch_id: ConversationBranchId,
    pub source_start_message_id: MessageId,
    pub source_end_message_id: MessageId,
    pub state_revision: u64,
    pub active_revision_id: String,
    pub active_revision_sha256: String,
}

/// Canonical, bounded memory visibility evidence for prompt preparation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MemoryRecordsAtHeadSnapshot {
    pub schema_version: u32,
    pub conversation_id: ConversationId,
    pub source_branch_id: ConversationBranchId,
    pub context_head_message_id: Option<MessageId>,
    pub include_invalidated: bool,
    pub records: Vec<MemoryRecordAtHeadEvidence>,
    pub snapshot_sha256: String,
}

/// Exact-at-head memory records plus the compact evidence sealed by a
/// generation attempt and rechecked in its atomic append transaction.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MemoryRecordsAtHeadSelection {
    pub snapshot: MemoryRecordsAtHeadSnapshot,
    pub records: Vec<StoredRevision<MemoryRecord>>,
}

/// One provider-neutral message persisted as generation-plan provenance.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StoredPromptMessage {
    pub role: RoleHint,
    pub content: String,
}

/// Credential-free provider request evidence bound to a sealed prompt plan.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProviderRequestSnapshotRecord {
    pub id: String,
    pub api_family: ApiFamily,
    pub request_schema_version: u32,
    /// Provider-neutral request material. It must contain no credential,
    /// authorization header, unrestricted URL, or opaque reasoning state.
    pub request: VersionedJson,
    pub mapping_diagnostics: VersionedJson,
    pub created_at: DateTime<Utc>,
}

/// Immutable, reproducible prompt-plan provenance for one generation.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GenerationPromptPlanRecord {
    pub id: String,
    pub generation_id: GenerationId,
    pub conversation_id: ConversationId,
    pub branch_id: ConversationBranchId,
    pub head_message_id: Option<MessageId>,
    pub latest_user_message_id: MessageId,
    pub prompt_preset_id: PromptPresetId,
    pub prompt_preset_revision_id: String,
    pub model_route_id: Option<ModelRouteId>,
    pub generation_preset_id: Option<GenerationPresetId>,
    pub task_profile_revision_id: Option<String>,
    pub random_seed: Option<u64>,
    pub tokenizer_id: String,
    pub tokenizer_version: String,
    pub plan: VersionedJson,
    /// Exact `ResolvedPromptPlan.plan_hash`. This hash intentionally excludes
    /// the `plan_hash` field itself and is also copied onto `generations`.
    pub plan_sha256: String,
    pub input_fingerprint_sha256: String,
    pub context_limit_tokens: u32,
    pub estimated_input_tokens: u32,
    pub reserved_output_tokens: u32,
    pub final_input_tokens: u32,
    pub cacheable_prefix_tokens: u32,
    pub provider_request: ProviderRequestSnapshotRecord,
    pub created_at: DateTime<Utc>,
}

/// An immutable revision snapshot returned for audit and rollback.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ObjectRevision<T> {
    pub revision_id: String,
    pub object_kind: String,
    pub object_id: String,
    pub revision: u64,
    pub value: T,
    pub sha256: String,
    pub created_at: DateTime<Utc>,
}

/// Deterministic JSON-level diff between two content-module revisions.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ContentModuleRevisionDiff {
    pub module_id: ContentModuleId,
    pub from_revision: u64,
    pub to_revision: u64,
    pub from_sha256: String,
    pub to_sha256: String,
    pub changed_paths: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PromptPresetRevisionDiff {
    pub preset_id: PromptPresetId,
    pub from_revision_id: String,
    pub from_revision: u64,
    pub from_sha256: String,
    pub to_revision_id: String,
    pub to_revision: u64,
    pub to_sha256: String,
    pub changed_paths: Vec<String>,
    pub diff_sha256: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PromptPresetRollbackReview {
    pub review_sha256: String,
    pub preset_id: PromptPresetId,
    pub expected_current_state_revision: u64,
    pub expected_current_revision_id: String,
    pub expected_current_sha256: String,
    pub target_revision_id: String,
    pub target_revision: u64,
    pub target_sha256: String,
    pub target_document_sha256: String,
    pub target_dependency_sha256: String,
    pub binding_snapshot_sha256: String,
    pub diff: PromptPresetRevisionDiff,
    pub reviewed_at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PromptPresetRollbackApproval {
    pub approval_id: String,
    pub expected_review_sha256: String,
    pub approval_sha256: String,
    pub approved_at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PromptPresetRollbackCommit {
    pub review: PromptPresetRollbackReview,
    pub approval: PromptPresetRollbackApproval,
    pub canonical_target: PromptPreset,
}

/// Active immutable module revision, including component hashes used by
/// composition and binding validation.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ActiveContentModuleRevision {
    pub object: ObjectRevision<ContentModule>,
    pub module_revision: ContentModuleRevision,
}

/// Exact immutable component material selected by an approved module plan.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "value", rename_all = "snake_case")]
pub enum ModuleRevisionComponentSnapshot {
    PromptBlock(PromptBlock),
    Control(ControlSpec),
    KnowledgeBook(ObjectRevision<KnowledgeBook>),
    TransformSet(ObjectRevision<TransformSet>),
    InteractionRuleSet(ObjectRevision<InteractionRuleSet>),
    Asset(AssetDescriptor),
}

/// Exact immutable content-module dependency captured by one prompt-preset
/// revision. This evidence grants no activation authority.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PromptPresetModuleDependency {
    pub ordinal: u32,
    pub prompt_preset_revision_id: String,
    pub module_id: ContentModuleId,
    pub module_revision_id: ModuleRevisionId,
    pub source_sha256: lorepia_domain::Sha256Digest,
}

/// State of a reviewed local content-package import.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PackageImportStatus {
    Inspected,
    AwaitingReview,
    Approved,
    Committing,
    Completed,
    Failed,
    Discarded,
    RolledBack,
}

/// Immutable package-source provenance retained independently of an import.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PackageSourceRecord {
    pub id: String,
    pub package_id: PackageId,
    pub format: String,
    pub format_version: u32,
    pub name: String,
    pub version: String,
    pub source_sha256: String,
    pub source_size_bytes: u64,
    pub author: Option<String>,
    pub license: String,
    pub redistribution_allowed: bool,
    pub manifest: VersionedJson,
    pub created_at: DateTime<Utc>,
}

/// Crash-recoverable commit record for a selected package import.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PackageImportRecord {
    pub id: String,
    pub package_id: PackageId,
    pub status: PackageImportStatus,
    pub revision: u64,
    pub inspection: VersionedJson,
    pub selection: Option<VersionedJson>,
    pub selected_component_ids: Vec<String>,
    pub failure_code: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// Domain-only content document accepted by an atomic package commit.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "document", rename_all = "snake_case")]
pub enum PackageCommitDocument {
    PromptPreset(PromptPreset),
    KnowledgeBook(KnowledgeBook),
    MemoryProfile(MemoryProfile),
    TransformSet(TransformSet),
    InteractionRuleSet(InteractionRuleSet),
    ContentModule(ContentModule),
    CharacterContent {
        character_id: String,
        content: Box<CharacterContentV1>,
    },
}

/// Storage-owned, parser-independent input for an approved package commit.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PackageCommitInput {
    pub source: PackageSourceRecord,
    pub import: PackageImportRecord,
    pub documents: Vec<PackageCommitDocument>,
    pub assets: Vec<AssetDescriptor>,
}

/// Internal result used by the package repository to bind an approved
/// component to the exact immutable revision created by its commit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PackageDocumentWrite {
    pub object_id: String,
    pub revision_id: String,
    pub state_revision: u64,
}

/// Result of invalidating branch-scoped memory records.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MemoryInvalidationResult {
    pub invalidated_records: u64,
    pub invalidated_jobs: u64,
}

/// Computes a full-wrapper JSON integrity digest.
///
/// This is deliberately *not* the resolver's prompt-plan identity, because a
/// [`ResolvedPromptPlan`] identity excludes its own `plan_hash` field. Durable
/// prompt plans use `ResolvedPromptPlan.plan_hash` instead.
pub fn versioned_json_sha256(value: &VersionedJson) -> CoreResult<String> {
    let (json, sha256) = encode_document("versioned JSON", value)?;
    if json == "null" {
        return Err(CoreError::invalid("versioned JSON must not be null"));
    }
    Ok(sha256)
}

pub fn prompt_preset_rollback_approval_sha256(
    approval_id: &str,
    expected_review_sha256: &str,
) -> CoreResult<String> {
    validate_identifier("prompt preset rollback approval", approval_id)?;
    validate_optional_sha256(
        "prompt preset rollback review hash",
        Some(expected_review_sha256),
    )?;
    serde_json::to_string(&serde_json::json!({
        "schema_version": 1,
        "approval_id": approval_id,
        "expected_review_sha256": expected_review_sha256,
    }))
    .map(|json| sha256_hex(json.as_bytes()))
    .map_err(|error| {
        CoreError::internal(format!(
            "cannot encode prompt preset rollback approval: {error}"
        ))
    })
}

fn variable_storage_key(variable: &lorepia_domain::VariableRef) -> String {
    variable.namespace.as_ref().map_or_else(
        || variable.id.as_str().to_owned(),
        |namespace| format!("{}.{}", namespace.as_str(), variable.id.as_str()),
    )
}

const fn variable_value_type(value: &lorepia_domain::VariableValue) -> &'static str {
    match value {
        lorepia_domain::VariableValue::Bool(_) => "bool",
        lorepia_domain::VariableValue::Integer(_) => "integer",
        lorepia_domain::VariableValue::Decimal(_) => "decimal",
        lorepia_domain::VariableValue::Text(_) => "text",
        lorepia_domain::VariableValue::Enum(_) => "enum",
        lorepia_domain::VariableValue::StringList(_) => "string_list",
    }
}

fn enum_wire<T: Serialize>(value: &T) -> CoreResult<String> {
    let encoded = serde_json::to_string(value)
        .map_err(|error| CoreError::internal(format!("cannot encode enum value: {error}")))?;
    serde_json::from_str::<String>(&encoded)
        .map_err(|_| CoreError::invalid("expected a unit enum wire value"))
}

fn usize_to_i64(value: usize, label: &str) -> CoreResult<i64> {
    i64::try_from(value).map_err(|_| CoreError::invalid(format!("{label} exceeds SQLite range")))
}

impl Storage {
    pub fn save_character_content(
        &self,
        character_id: &str,
        content: &CharacterContentV1,
        expected_revision: Option<u64>,
    ) -> CoreResult<StoredRevision<CharacterContentV1>> {
        validate_identifier("character", character_id)?;
        let object_id = character_content_object_id(character_id);
        let provenance = Provenance {
            source_kind: SourceKind::UserCreated,
            source_id: Some(character_id.to_owned()),
            source_hash: content
                .unknown_extensions
                .raw_source_sha256
                .as_ref()
                .map(ToString::to_string),
            author: None,
            license: None,
            imported_at: None,
        };
        save_content_object(
            self,
            DocumentTable::CharacterContent,
            &object_id,
            content.schema_version,
            content,
            &provenance,
            expected_revision,
            if expected_revision.is_some() {
                RevisionEventKind::Update
            } else {
                RevisionEventKind::Create
            },
            |transaction, revision_id, document_json| {
                write_character_content_projection(
                    transaction,
                    &object_id,
                    character_id,
                    revision_id,
                    content,
                    document_json,
                    None,
                )
            },
            false,
        )
    }

    pub fn get_character_content(
        &self,
        character_id: &str,
    ) -> CoreResult<StoredRevision<CharacterContentV1>> {
        get_document(
            self,
            DocumentTable::CharacterContent,
            &character_content_object_id(character_id),
            false,
        )
    }
}

fn validate_module_resolution_context_authority(
    transaction: &Transaction<'_>,
    context: &lorepia_orchestration::ModuleResolutionContext,
) -> CoreResult<()> {
    validate_module_context_local_user(transaction, context)?;
    match context.conversation_id.as_deref() {
        Some(conversation_id) => {
            validate_conversation_module_context(transaction, context, conversation_id)
        }
        None => validate_global_module_context(transaction, context),
    }
}

fn validate_module_context_local_user(
    transaction: &Transaction<'_>,
    context: &lorepia_orchestration::ModuleResolutionContext,
) -> CoreResult<()> {
    let settings_json = transaction
        .query_row(
            "SELECT value_json FROM app_settings WHERE key = 'application'",
            [],
            |row| row.get::<_, String>(0),
        )
        .optional()
        .map_err(storage_db_error)?
        .ok_or_else(|| storage_corrupted("application settings are missing"))?;
    let settings: AppSettings = decode_document("application settings", &settings_json)?;
    if settings.local_user_id != context.local_user_id {
        return Err(CoreError::invalid(
            "module runtime context local user is stale",
        ));
    }
    Ok(())
}

fn validate_global_module_context(
    transaction: &Transaction<'_>,
    context: &lorepia_orchestration::ModuleResolutionContext,
) -> CoreResult<()> {
    if context.branch_id.is_some() {
        return Err(CoreError::invalid(
            "module runtime context has a branch without a conversation",
        ));
    }
    if let Some(character_id) = context.character_id.as_deref() {
        let exists = transaction
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM characters WHERE id = ?1)",
                [character_id],
                |row| row.get::<_, bool>(0),
            )
            .map_err(storage_db_error)?;
        if !exists {
            return Err(not_found("module runtime character"));
        }
    }
    if let Some(persona_id) = context.persona_id.as_ref() {
        let exists = transaction
            .query_row(
                "SELECT EXISTS(
                     SELECT 1 FROM personas
                     WHERE object_id = ?1 AND deleted_at IS NULL
                 )",
                [persona_id.as_str()],
                |row| row.get::<_, bool>(0),
            )
            .map_err(storage_db_error)?;
        if !exists {
            return Err(not_found("module runtime persona"));
        }
    }
    Ok(())
}

fn validate_conversation_module_context(
    transaction: &Transaction<'_>,
    context: &lorepia_orchestration::ModuleResolutionContext,
    conversation_id: &str,
) -> CoreResult<()> {
    let character_id = transaction
        .query_row(
            "SELECT character_id FROM conversations WHERE id = ?1",
            [conversation_id],
            |row| row.get::<_, String>(0),
        )
        .optional()
        .map_err(storage_db_error)?
        .ok_or_else(|| not_found("module runtime conversation"))?;
    if context.character_id.as_deref() != Some(character_id.as_str()) {
        return Err(CoreError::invalid(
            "module runtime context character is stale",
        ));
    }
    let persona_id = transaction
        .query_row(
            "SELECT persona_id
             FROM conversation_persona_selections
             WHERE conversation_id = ?1 AND deleted_at IS NULL",
            [conversation_id],
            |row| row.get::<_, String>(0),
        )
        .optional()
        .map_err(storage_db_error)?;
    if context
        .persona_id
        .as_ref()
        .map(lorepia_domain::PersonaId::as_str)
        != persona_id.as_deref()
    {
        return Err(CoreError::invalid(
            "module runtime context persona is stale",
        ));
    }
    if let Some(branch_id) = context.branch_id.as_deref() {
        let branch_conversation = transaction
            .query_row(
                "SELECT conversation_id FROM conversation_branches WHERE id = ?1",
                [branch_id],
                |row| row.get::<_, String>(0),
            )
            .optional()
            .map_err(storage_db_error)?;
        if branch_conversation
            .as_deref()
            .is_some_and(|owner| owner != conversation_id)
        {
            return Err(CoreError::invalid(
                "module runtime context branch belongs to another conversation",
            ));
        }
    }
    Ok(())
}
