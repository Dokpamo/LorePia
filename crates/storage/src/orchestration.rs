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
mod knowledge_memory_profiles;
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
#[cfg(test)]
use knowledge_memory_profiles::{
    ensure_memory_summary_schema_for_test as ensure_memory_summary_schema,
    weight_millionths_for_test as weight_millionths,
    write_knowledge_entries_for_test as write_knowledge_entries,
};
use knowledge_memory_profiles::{write_knowledge_book_projection, write_memory_profile_projection};
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

/// Storage-only counters used to prove orchestration transaction atomicity
/// without exposing raw `SQLite` access through app or FFI surfaces.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OrchestrationDatabaseStats {
    pub generations: u64,
    pub generation_prompt_plans: u64,
    pub knowledge_activation_logs: u64,
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

impl Storage {
    pub fn save_memory_record(
        &self,
        record: &MemoryRecord,
        expected_revision: Option<u64>,
    ) -> CoreResult<StoredRevision<MemoryRecord>> {
        save_memory_record(self, record, expected_revision)
    }

    pub fn get_memory_record(
        &self,
        conversation_id: &ConversationId,
        branch_id: &ConversationBranchId,
        id: &MemoryRecordId,
    ) -> CoreResult<StoredRevision<MemoryRecord>> {
        get_memory_record(self, id, false, Some((conversation_id, branch_id)))
    }

    pub fn list_memory_records(
        &self,
        conversation_id: &ConversationId,
        branch_id: &ConversationBranchId,
        include_invalidated: bool,
    ) -> CoreResult<Vec<StoredRevision<MemoryRecord>>> {
        list_visible_memory_records(self, conversation_id, branch_id, include_invalidated)
    }

    pub fn list_memory_records_at_head(
        &self,
        conversation_id: &ConversationId,
        source_branch_id: &ConversationBranchId,
        context_head_message_id: Option<&MessageId>,
        include_invalidated: bool,
    ) -> CoreResult<MemoryRecordsAtHeadSelection> {
        let connection = self.connection()?;
        memory_records_at_head_in_connection(
            &connection,
            conversation_id,
            source_branch_id,
            context_head_message_id,
            include_invalidated,
        )
    }

    /// Resolves bounded message positions on one exact historical branch
    /// lineage without loading the full conversation into Core.
    ///
    /// Depth zero is `context_head_message_id`; larger depths are older. The
    /// context head must remain visible from `source_branch_id`, but it need
    /// not equal that branch's newer mutable head.
    pub fn message_lineage_depths_at_head(
        &self,
        conversation_id: &ConversationId,
        source_branch_id: &ConversationBranchId,
        context_head_message_id: &MessageId,
        message_ids: &[MessageId],
    ) -> CoreResult<HashMap<MessageId, u64>> {
        validate_identifier("message lineage conversation", &conversation_id.0)?;
        validate_identifier("message lineage source branch", &source_branch_id.0)?;
        validate_identifier("message lineage context head", &context_head_message_id.0)?;
        if message_ids.len() > MAX_PROMPT_BLOCKS {
            return Err(CoreError::invalid(
                "message lineage position request exceeds its bound",
            ));
        }
        let mut requested = message_ids
            .iter()
            .map(|id| id.0.as_str())
            .collect::<Vec<_>>();
        for id in &requested {
            validate_identifier("message lineage member", id)?;
        }
        requested.sort_unstable();
        if requested.windows(2).any(|pair| pair[0] == pair[1]) {
            return Err(CoreError::invalid(
                "message lineage position identifiers must be unique",
            ));
        }
        let requested_json = serde_json::to_string(&requested).map_err(|error| {
            CoreError::internal(format!("cannot encode message lineage request: {error}"))
        })?;
        let connection = self.connection()?;
        let mut statement = connection
            .prepare(
                "WITH RECURSIVE source_lineage(id, parent_id, depth) AS (
                     SELECT message.id, message.parent_id, 0
                     FROM conversation_branches AS branch
                     JOIN messages AS message
                       ON message.conversation_id = branch.conversation_id
                      AND message.id = branch.head_message_id
                     WHERE branch.conversation_id = ?1 AND branch.id = ?2
                     UNION ALL
                     SELECT parent.id, parent.parent_id, child.depth + 1
                     FROM messages AS parent
                     JOIN source_lineage AS child ON child.parent_id = parent.id
                     WHERE parent.conversation_id = ?1 AND child.depth < 100000
                 ),
                 context(id, parent_id) AS (
                     SELECT message.id, message.parent_id
                     FROM messages AS message
                     JOIN source_lineage ON source_lineage.id = message.id
                     WHERE message.conversation_id = ?1 AND message.id = ?3
                 ),
                 lineage(id, parent_id, depth) AS (
                     SELECT id, parent_id, 0 FROM context
                     UNION ALL
                     SELECT parent.id, parent.parent_id, child.depth + 1
                     FROM messages AS parent
                     JOIN lineage AS child ON child.parent_id = parent.id
                     WHERE parent.conversation_id = ?1 AND child.depth < 100000
                 )
                 SELECT lineage.id, lineage.depth
                 FROM json_each(?4) AS requested
                 JOIN lineage ON lineage.id = requested.value
                 ORDER BY lineage.depth DESC, lineage.id",
            )
            .map_err(storage_db_error)?;
        let rows = statement
            .query_map(
                params![
                    conversation_id.0,
                    source_branch_id.0,
                    context_head_message_id.0,
                    requested_json
                ],
                |row| Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?)),
            )
            .map_err(storage_db_error)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(storage_db_error)?;
        if rows.len() != requested.len() {
            return Err(CoreError::invalid(
                "requested message is unavailable at the exact prompt context head",
            ));
        }
        rows.into_iter()
            .map(|(id, depth)| Ok((MessageId(id), u64_revision(depth)?)))
            .collect()
    }

    pub fn invalidate_memory_range(
        &self,
        conversation_id: &ConversationId,
        branch_id: &ConversationBranchId,
        start_message_id: &MessageId,
        end_message_id: &MessageId,
        invalidated_at: DateTime<Utc>,
    ) -> CoreResult<MemoryInvalidationResult> {
        invalidate_memory_range(
            self,
            conversation_id,
            branch_id,
            start_message_id,
            end_message_id,
            invalidated_at,
        )
    }

    pub fn save_memory_job(
        &self,
        job: &MemoryJob,
        expected_revision: Option<u64>,
    ) -> CoreResult<StoredRevision<MemoryJob>> {
        save_memory_job(self, job, expected_revision)
    }

    pub fn get_memory_job(&self, id: &MemoryJobId) -> CoreResult<StoredRevision<MemoryJob>> {
        get_memory_job(self, id)
    }

    pub fn save_memory_embedding(&self, embedding: &MemoryEmbeddingRecord) -> CoreResult<()> {
        save_memory_embedding(self, embedding)
    }

    pub fn get_memory_embedding(&self, id: &str) -> CoreResult<MemoryEmbeddingRecord> {
        get_memory_embedding(self, id)
    }

    pub fn save_module_binding(
        &self,
        binding: &ModuleBinding,
        expected_revision: Option<u64>,
    ) -> CoreResult<StoredRevision<ModuleBinding>> {
        save_module_binding(self, binding, expected_revision)
    }

    /// Atomically revalidates and applies one hash-bound module activation.
    pub fn apply_approved_module_activation(
        &self,
        review: &lorepia_orchestration::ModuleActivationReview,
        approved: &lorepia_orchestration::ApprovedModuleActivationPlan,
    ) -> CoreResult<StoredRevision<ModuleBinding>> {
        apply_approved_module_activation(self, review, approved)
    }

    /// Recovers one already-applied activation after a lost response.
    ///
    /// The lookup is deliberately keyed by both the caller-stable approval id
    /// and the exact plan hash. Reusing either identity for a different
    /// activation is rejected instead of being treated as a new write.
    pub fn recover_applied_module_activation(
        &self,
        binding_id: &ModuleBindingId,
        approval: &lorepia_orchestration::ModuleActivationApproval,
    ) -> CoreResult<Option<RecoveredModuleActivation>> {
        recover_applied_module_activation(self, binding_id, approval)
    }

    /// Recovers one already-applied rollback after a lost response.
    ///
    /// The activation identity and final binding are checked by the ordinary
    /// recovery path first. The rollback-only plan and approval digest must
    /// then be present in, and verify against, the immutable prepared audit.
    pub fn recover_applied_module_rollback(
        &self,
        binding_id: &ModuleBindingId,
        approval: &lorepia_orchestration::ModuleActivationApproval,
    ) -> CoreResult<Option<RecoveredModuleRollback>> {
        recover_applied_module_rollback(self, binding_id, approval)
    }

    /// Loads the one currently applied runtime overlay only after independently
    /// re-deriving and exact-matching the caller's fresh context review.
    pub fn get_applied_module_runtime_plan(
        &self,
        current_review: &lorepia_orchestration::ModuleMergeReview,
    ) -> CoreResult<lorepia_orchestration::AppliedModuleRuntimePlan> {
        get_applied_module_runtime_plan(self, current_review)
    }

    /// Loads an immutable applied runtime plan for a sealed historical event.
    /// A plan made stale by later binding changes remains valid historical
    /// authority, but every canonical payload and exact source revision is
    /// still verified before it is returned.
    pub fn get_historical_applied_module_runtime_plan(
        &self,
        applied_plan_sha256: &lorepia_domain::Sha256Digest,
    ) -> CoreResult<lorepia_orchestration::AppliedModuleRuntimePlan> {
        let mut connection = self.connection()?;
        let transaction = connection.transaction().map_err(storage_db_error)?;
        let runtime = load_historical_applied_module_runtime_plan_transaction(
            &transaction,
            applied_plan_sha256,
        )?;
        transaction.commit().map_err(storage_db_error)?;
        Ok(runtime)
    }

    /// Revalidates and uniquely materializes the active runtime authority
    /// without persisting a context row. Proposed branches use this preview
    /// before the branch exists and promote the exact returned object only in
    /// their atomic append transaction.
    pub fn preview_applied_module_runtime_plan(
        &self,
        current_review: &lorepia_orchestration::ModuleMergeReview,
    ) -> CoreResult<lorepia_orchestration::AppliedModuleRuntimePlan> {
        preview_applied_module_runtime_plan(self, current_review)
    }

    /// Revalidates a target review and derives a context-specific plan without
    /// persisting it. This is safe for a proposed branch that does not exist
    /// yet; the atomic branch append persists the returned plan only after the
    /// branch row and checkpoint have been created.
    pub fn derive_applied_module_runtime_plan(
        &self,
        source: &lorepia_orchestration::AppliedModuleRuntimePlan,
        target_review: &lorepia_orchestration::ModuleMergeReview,
    ) -> CoreResult<lorepia_orchestration::AppliedModuleRuntimePlan> {
        derive_applied_module_runtime_plan(self, source, target_review)
    }

    pub fn list_module_bindings(
        &self,
        module_id: &ContentModuleId,
    ) -> CoreResult<Vec<StoredRevision<ModuleBinding>>> {
        list_module_bindings(self, module_id)
    }

    pub fn soft_delete_module_binding(
        &self,
        id: &ModuleBindingId,
        expected_revision: u64,
    ) -> CoreResult<StoredRevision<ModuleBinding>> {
        soft_delete_module_binding(self, id, expected_revision)
    }

    pub fn list_all_module_bindings(&self) -> CoreResult<Vec<StoredRevision<ModuleBinding>>> {
        let connection = self.connection()?;
        let mut statement = connection
            .prepare(
                "SELECT document_json, revision, created_at, updated_at, deleted_at
                 FROM content_module_bindings
                 WHERE deleted_at IS NULL
                 ORDER BY scope_kind, priority DESC, id",
            )
            .map_err(storage_db_error)?;
        let rows = statement
            .query_map([], module_binding_row)
            .map_err(storage_db_error)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(storage_db_error)?;
        rows.into_iter()
            .map(|row| decode_stored_document("module binding", row))
            .collect()
    }

    /// Applies a previously reviewed rollback only when every hash-bound
    /// revision and the binding CAS version still match the reviewed snapshot.
    pub fn apply_module_rollback_plan(
        &self,
        _plan: &lorepia_orchestration::ModuleRollbackPlan,
    ) -> CoreResult<StoredRevision<ModuleBinding>> {
        Err(CoreError::invalid(
            "module rollback requires an approved target runtime plan",
        ))
    }

    /// Atomically applies a rollback together with its freshly approved target
    /// runtime composition.
    pub fn apply_approved_module_rollback(
        &self,
        approved: &lorepia_orchestration::ApprovedModuleRollbackPlan,
    ) -> CoreResult<StoredRevision<ModuleBinding>> {
        apply_approved_module_rollback(self, approved)
    }

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

    /// Loads the immutable prompt provenance attached to one generation.
    pub fn get_generation_prompt_plan_by_generation(
        &self,
        generation_id: &GenerationId,
    ) -> CoreResult<GenerationPromptPlanRecord> {
        let connection = self.connection()?;
        let raw = connection
            .query_row(
                "SELECT plan.id, generation.id, plan.conversation_id, plan.branch_id,
                        plan.head_message_id, plan.latest_user_message_id,
                        plan.prompt_preset_id, plan.prompt_preset_revision_id,
                        plan.model_route_id, plan.generation_preset_id,
                        plan.task_profile_revision_id, plan.random_seed,
                        plan.tokenizer_id, plan.tokenizer_version,
                        plan.schema_version, plan.canonical_plan_json,
                        plan.plan_sha256, plan.input_fingerprint_sha256,
                        plan.context_limit_tokens, plan.estimated_input_tokens,
                        plan.reserved_output_tokens, plan.final_input_tokens,
                        plan.cacheable_prefix_tokens, plan.created_at,
                        snapshot.id, snapshot.api_family,
                        snapshot.request_schema_version, snapshot.request_json,
                        snapshot.request_sha256,
                        snapshot.mapping_diagnostics_json, snapshot.created_at
                 FROM generations AS generation
                 JOIN generation_prompt_plans AS plan
                   ON plan.id = generation.resolved_prompt_plan_id
                 JOIN generation_prompt_plan_seals AS seal
                   ON seal.plan_id = plan.id
                  AND seal.plan_sha256 = plan.plan_sha256
                 JOIN provider_request_snapshots AS snapshot
                   ON snapshot.id = generation.provider_request_snapshot_id
                  AND snapshot.plan_id = plan.id
                 WHERE generation.id = ?1
                   AND generation.prompt_plan_sha256 = plan.plan_sha256",
                [&generation_id.0],
                raw_generation_prompt_plan,
            )
            .optional()
            .map_err(storage_db_error)?
            .ok_or_else(|| not_found("generation prompt plan"))?;
        decode_generation_prompt_plan_record(raw)
    }

    /// Returns bounded orchestration counters for atomicity assertions.
    pub fn orchestration_stats(&self) -> CoreResult<OrchestrationDatabaseStats> {
        let connection = self.connection()?;
        let count = |table: &str| -> CoreResult<u64> {
            let sql = format!("SELECT COUNT(*) FROM {table}");
            connection
                .query_row(&sql, [], |row| row.get::<_, i64>(0))
                .map_err(storage_db_error)
                .and_then(u64_revision)
        };
        Ok(OrchestrationDatabaseStats {
            generations: count("generations")?,
            generation_prompt_plans: count("generation_prompt_plans")?,
            knowledge_activation_logs: count("knowledge_activation_logs")?,
        })
    }
}

struct RawGenerationPromptPlan {
    plan_id: String,
    generation_id: String,
    conversation_id: String,
    branch_id: String,
    head_message_id: Option<String>,
    latest_user_message_id: String,
    prompt_preset_id: String,
    prompt_preset_revision_id: String,
    model_route_id: Option<String>,
    generation_preset_id: Option<String>,
    task_profile_revision_id: Option<String>,
    random_seed: Option<i64>,
    tokenizer_id: String,
    tokenizer_version: String,
    plan_schema_version: i64,
    canonical_plan_json: String,
    plan_sha256: String,
    input_fingerprint_sha256: String,
    context_limit_tokens: i64,
    estimated_input_tokens: i64,
    reserved_output_tokens: i64,
    final_input_tokens: i64,
    cacheable_prefix_tokens: i64,
    plan_created_at: String,
    request_id: String,
    api_family: String,
    request_schema_version: i64,
    request_json: String,
    request_sha256: String,
    mapping_diagnostics_json: String,
    request_created_at: String,
}

fn raw_generation_prompt_plan(
    row: &rusqlite::Row<'_>,
) -> rusqlite::Result<RawGenerationPromptPlan> {
    Ok(RawGenerationPromptPlan {
        plan_id: row.get(0)?,
        generation_id: row.get(1)?,
        conversation_id: row.get(2)?,
        branch_id: row.get(3)?,
        head_message_id: row.get(4)?,
        latest_user_message_id: row.get(5)?,
        prompt_preset_id: row.get(6)?,
        prompt_preset_revision_id: row.get(7)?,
        model_route_id: row.get(8)?,
        generation_preset_id: row.get(9)?,
        task_profile_revision_id: row.get(10)?,
        random_seed: row.get(11)?,
        tokenizer_id: row.get(12)?,
        tokenizer_version: row.get(13)?,
        plan_schema_version: row.get(14)?,
        canonical_plan_json: row.get(15)?,
        plan_sha256: row.get(16)?,
        input_fingerprint_sha256: row.get(17)?,
        context_limit_tokens: row.get(18)?,
        estimated_input_tokens: row.get(19)?,
        reserved_output_tokens: row.get(20)?,
        final_input_tokens: row.get(21)?,
        cacheable_prefix_tokens: row.get(22)?,
        plan_created_at: row.get(23)?,
        request_id: row.get(24)?,
        api_family: row.get(25)?,
        request_schema_version: row.get(26)?,
        request_json: row.get(27)?,
        request_sha256: row.get(28)?,
        mapping_diagnostics_json: row.get(29)?,
        request_created_at: row.get(30)?,
    })
}

struct DecodedGenerationPromptPayload {
    plan_schema_version: u32,
    request_schema_version: u32,
    plan_value: Value,
    request_value: Value,
    resolved: ResolvedPromptPlan,
}

fn decode_generation_prompt_payload(
    raw: &RawGenerationPromptPlan,
) -> CoreResult<DecodedGenerationPromptPayload> {
    let plan_schema_version = u32::try_from(raw.plan_schema_version)
        .map_err(|_| storage_corrupted("stored prompt plan schema version is invalid"))?;
    let request_schema_version = u32::try_from(raw.request_schema_version)
        .map_err(|_| storage_corrupted("stored request schema version is invalid"))?;
    validate_stored_json("resolved prompt plan", &raw.canonical_plan_json)?;
    validate_stored_json("provider request snapshot", &raw.request_json)?;
    validate_stored_sha256("prompt plan", &raw.plan_sha256)?;
    validate_stored_sha256("request", &raw.request_sha256)?;
    if sha256_hex(raw.request_json.as_bytes()) != raw.request_sha256 {
        return Err(storage_corrupted(
            "stored provider request snapshot hash does not match its canonical JSON",
        ));
    }
    let plan_value = serde_json::from_str::<Value>(&raw.canonical_plan_json)
        .map_err(|error| storage_corrupted(format!("stored prompt plan is invalid: {error}")))?;
    let request_value = serde_json::from_str::<Value>(&raw.request_json).map_err(|error| {
        storage_corrupted(format!(
            "stored provider request snapshot is invalid: {error}"
        ))
    })?;
    let resolved =
        serde_json::from_value::<ResolvedPromptPlan>(plan_value.clone()).map_err(|error| {
            storage_corrupted(format!(
                "stored resolved prompt plan cannot be decoded: {error}"
            ))
        })?;
    if resolved.schema_version != plan_schema_version
        || resolved.plan_hash != raw.plan_sha256
        || resolved_prompt_plan_hash(&resolved).map_err(|error| {
            storage_corrupted(format!(
                "stored resolved prompt plan cannot be rehashed: {}",
                error.message
            ))
        })? != raw.plan_sha256
    {
        return Err(storage_corrupted(
            "stored resolved prompt plan hash or schema version is invalid",
        ));
    }
    Ok(DecodedGenerationPromptPayload {
        plan_schema_version,
        request_schema_version,
        plan_value,
        request_value,
        resolved,
    })
}

fn validate_stored_json(label: &str, value: &str) -> CoreResult<()> {
    validate_json_bounds(&format!("stored {label}"), value).map_err(|error| {
        storage_corrupted(format!(
            "stored {label} violates storage bounds: {}",
            error.message
        ))
    })
}

fn validate_stored_sha256(label: &str, value: &str) -> CoreResult<()> {
    validate_optional_sha256(&format!("stored {label} hash"), Some(value)).map_err(|error| {
        storage_corrupted(format!("stored {label} hash is invalid: {}", error.message))
    })
}

fn decode_generation_prompt_plan_record(
    raw: RawGenerationPromptPlan,
) -> CoreResult<GenerationPromptPlanRecord> {
    let decoded = decode_generation_prompt_payload(&raw)?;
    let record = GenerationPromptPlanRecord {
        id: raw.plan_id,
        generation_id: GenerationId(raw.generation_id),
        conversation_id: ConversationId(raw.conversation_id),
        branch_id: ConversationBranchId(raw.branch_id),
        head_message_id: raw.head_message_id.map(MessageId),
        latest_user_message_id: MessageId(raw.latest_user_message_id),
        prompt_preset_id: PromptPresetId::from(raw.prompt_preset_id),
        prompt_preset_revision_id: raw.prompt_preset_revision_id,
        model_route_id: raw.model_route_id.map(ModelRouteId::from),
        generation_preset_id: raw.generation_preset_id.map(GenerationPresetId::from),
        task_profile_revision_id: raw.task_profile_revision_id,
        random_seed: raw
            .random_seed
            .map(|value| {
                u64::try_from(value)
                    .map_err(|_| storage_corrupted("stored prompt random seed is invalid"))
            })
            .transpose()?,
        tokenizer_id: raw.tokenizer_id,
        tokenizer_version: raw.tokenizer_version,
        plan: VersionedJson {
            schema_version: decoded.plan_schema_version,
            value: decoded.plan_value,
        },
        plan_sha256: raw.plan_sha256,
        input_fingerprint_sha256: raw.input_fingerprint_sha256,
        context_limit_tokens: positive_u32("context limit", raw.context_limit_tokens)?,
        estimated_input_tokens: nonnegative_u32(
            "estimated input tokens",
            raw.estimated_input_tokens,
        )?,
        reserved_output_tokens: nonnegative_u32(
            "reserved output tokens",
            raw.reserved_output_tokens,
        )?,
        final_input_tokens: nonnegative_u32("final input tokens", raw.final_input_tokens)?,
        cacheable_prefix_tokens: nonnegative_u32(
            "cacheable prefix tokens",
            raw.cacheable_prefix_tokens,
        )?,
        provider_request: ProviderRequestSnapshotRecord {
            id: raw.request_id,
            api_family: parse_api_family(&raw.api_family)?,
            request_schema_version: decoded.request_schema_version,
            request: VersionedJson {
                schema_version: decoded.request_schema_version,
                value: decoded.request_value,
            },
            mapping_diagnostics: decode_document(
                "provider mapping diagnostics",
                &raw.mapping_diagnostics_json,
            )?,
            created_at: parse_datetime("provider request created_at", &raw.request_created_at)?,
        },
        created_at: parse_datetime("prompt plan created_at", &raw.plan_created_at)?,
    };
    validate_generation_prompt_plan_metadata(&record, &decoded.resolved)?;
    Ok(record)
}

fn validate_generation_prompt_plan_metadata(
    record: &GenerationPromptPlanRecord,
    resolved: &ResolvedPromptPlan,
) -> CoreResult<()> {
    let latest_user_included = resolved.effective_messages.iter().any(|message| {
        message.effective_role == ProviderMessageRole::User
            && message
                .source_message_ids
                .iter()
                .any(|id| id == &record.latest_user_message_id)
    });
    if resolved.preset_id != record.prompt_preset_id
        || resolved.generation_preset_id != record.generation_preset_id
        || resolved.trace.max_context_tokens != record.context_limit_tokens
        || resolved.trace.reserved_output_tokens != record.reserved_output_tokens
        || resolved.trace.estimated_input_tokens != record.estimated_input_tokens
        || record.final_input_tokens != resolved.trace.estimated_input_tokens
        || !latest_user_included
    {
        return Err(storage_corrupted(
            "stored resolved prompt plan metadata does not match its canonical body",
        ));
    }
    Ok(())
}

#[derive(Debug, Clone)]
pub(crate) struct GenerationPromptPlanLink {
    pub plan_id: String,
    pub plan_sha256: String,
    pub provider_request_snapshot_id: String,
}

struct PreparedGenerationPromptPlan {
    resolved: ResolvedPromptPlan,
    canonical_plan_json: String,
    request_json: String,
    mapping_diagnostics_json: String,
    request_sha256: String,
    random_seed: Option<i64>,
}

fn prepare_generation_prompt_plan(
    transaction: &Transaction<'_>,
    record: &GenerationPromptPlanRecord,
) -> CoreResult<PreparedGenerationPromptPlan> {
    validate_identifier("generation prompt plan", &record.id)?;
    validate_identifier("provider request snapshot", &record.provider_request.id)?;
    validate_optional_sha256("prompt plan hash", Some(&record.plan_sha256))?;
    validate_optional_sha256(
        "prompt input fingerprint",
        Some(&record.input_fingerprint_sha256),
    )?;
    if record.context_limit_tokens == 0
        || record
            .final_input_tokens
            .saturating_add(record.reserved_output_tokens)
            > record.context_limit_tokens
    {
        return Err(CoreError::invalid(
            "resolved prompt plan exceeds its context token limit",
        ));
    }
    let resolved: ResolvedPromptPlan = serde_json::from_value(record.plan.value.clone())
        .map_err(|error| CoreError::invalid(format!("resolved prompt plan is invalid: {error}")))?;
    validate_prepared_generation_prompt_plan_metadata(record, &resolved)?;
    let canonical_plan_json = serde_json::to_string(&record.plan.value).map_err(|error| {
        CoreError::invalid(format!("cannot encode resolved prompt plan: {error}"))
    })?;
    validate_json_bounds("resolved prompt plan", &canonical_plan_json)?;
    let request_json =
        serde_json::to_string(&record.provider_request.request.value).map_err(|error| {
            CoreError::invalid(format!("cannot encode provider request snapshot: {error}"))
        })?;
    validate_json_bounds("provider request snapshot", &request_json)?;
    let mapping_diagnostics_json =
        serde_json::to_string(&record.provider_request.mapping_diagnostics).map_err(|error| {
            CoreError::invalid(format!(
                "cannot encode provider mapping diagnostics: {error}"
            ))
        })?;
    validate_json_bounds("provider mapping diagnostics", &mapping_diagnostics_json)?;
    // Prompt-only and transform-only module overlays must also pass the exact
    // append-time module-plan identity check.
    let _ = load_generation_module_plan_evidence(transaction, record)?;
    let request_sha256 = sha256_hex(request_json.as_bytes());
    let random_seed = record
        .random_seed
        .map(|value| {
            i64::try_from(value)
                .map_err(|_| CoreError::invalid("prompt random seed exceeds SQLite range"))
        })
        .transpose()?;
    Ok(PreparedGenerationPromptPlan {
        resolved,
        canonical_plan_json,
        request_json,
        mapping_diagnostics_json,
        request_sha256,
        random_seed,
    })
}

fn validate_prepared_generation_prompt_plan_metadata(
    record: &GenerationPromptPlanRecord,
    resolved: &ResolvedPromptPlan,
) -> CoreResult<()> {
    if record.plan.schema_version != resolved.schema_version
        || resolved.plan_hash != record.plan_sha256
        || resolved_prompt_plan_hash(resolved)? != record.plan_sha256
    {
        return Err(CoreError::invalid(
            "resolved prompt plan hash or schema version does not match",
        ));
    }
    if resolved.preset_id != record.prompt_preset_id
        || resolved.generation_preset_id != record.generation_preset_id
        || resolved.trace.max_context_tokens != record.context_limit_tokens
        || resolved.trace.reserved_output_tokens != record.reserved_output_tokens
        || resolved.trace.estimated_input_tokens != record.estimated_input_tokens
    {
        return Err(CoreError::invalid(
            "resolved prompt plan metadata does not match its canonical body",
        ));
    }
    if record.final_input_tokens != resolved.trace.estimated_input_tokens {
        return Err(CoreError::invalid(
            "final input token count does not match the resolved prompt plan",
        ));
    }
    let latest_user_included = resolved.effective_messages.iter().any(|message| {
        message.effective_role == ProviderMessageRole::User
            && message
                .source_message_ids
                .iter()
                .any(|id| id == &record.latest_user_message_id)
    });
    if !latest_user_included {
        return Err(CoreError::invalid(
            "resolved prompt plan does not include the latest user message",
        ));
    }
    if record.model_route_id.is_some() != record.generation_preset_id.is_some() {
        return Err(CoreError::invalid(
            "prompt plan route and generation preset must be present together",
        ));
    }
    Ok(())
}

fn insert_generation_prompt_plan(
    transaction: &Transaction<'_>,
    record: &GenerationPromptPlanRecord,
    prepared: &PreparedGenerationPromptPlan,
) -> CoreResult<()> {
    transaction
        .execute(
            "INSERT INTO generation_prompt_plans
             (id, schema_version, plan_sha256, input_fingerprint_sha256,
              conversation_id, branch_id, head_message_id,
              latest_user_message_id, latest_user_included, prompt_preset_id,
              prompt_preset_revision_id, generation_preset_id, model_route_id,
              task_profile_revision_id, random_seed, tokenizer_id,
              tokenizer_version, context_limit_tokens, reserved_output_tokens,
              estimated_input_tokens, final_input_tokens, message_count,
              cacheable_prefix_tokens, status, canonical_plan_json, sealed_at,
              created_at)
             VALUES (
                 ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, 1, ?9, ?10, ?11, ?12,
                 ?13, ?14, ?15, ?16, ?17, ?18, ?19, ?20, ?21, ?22,
                 'resolved', ?23, ?24, ?24
             )",
            params![
                record.id,
                record.plan.schema_version,
                record.plan_sha256,
                record.input_fingerprint_sha256,
                record.conversation_id.0,
                record.branch_id.0,
                record.head_message_id.as_ref().map(|id| id.0.as_str()),
                record.latest_user_message_id.0,
                record.prompt_preset_id.as_str(),
                record.prompt_preset_revision_id,
                record
                    .generation_preset_id
                    .as_ref()
                    .map(GenerationPresetId::as_str),
                record.model_route_id.as_ref().map(ModelRouteId::as_str),
                record.task_profile_revision_id,
                prepared.random_seed,
                record.tokenizer_id,
                record.tokenizer_version,
                record.context_limit_tokens,
                record.reserved_output_tokens,
                record.estimated_input_tokens,
                record.final_input_tokens,
                i64::try_from(prepared.resolved.effective_messages.len())
                    .map_err(|_| CoreError::invalid("too many prompt messages"))?,
                record.cacheable_prefix_tokens,
                prepared.canonical_plan_json,
                record.created_at.to_rfc3339(),
            ],
        )
        .map_err(storage_db_error)?;
    Ok(())
}

fn seal_generation_prompt_plan(
    transaction: &Transaction<'_>,
    record: &GenerationPromptPlanRecord,
    prepared: &PreparedGenerationPromptPlan,
) -> CoreResult<()> {
    transaction
        .execute(
            "INSERT INTO generation_prompt_plan_seals
             (plan_id, plan_sha256, sealed_at) VALUES (?1, ?2, ?3)",
            params![
                record.id,
                record.plan_sha256,
                record.created_at.to_rfc3339()
            ],
        )
        .map_err(storage_db_error)?;
    transaction
        .execute(
            "INSERT INTO provider_request_snapshots
             (id, plan_id, api_family, request_schema_version, request_json,
              request_sha256, mapping_diagnostics_json, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![
                record.provider_request.id,
                record.id,
                api_family_str(record.provider_request.api_family),
                record.provider_request.request_schema_version,
                prepared.request_json,
                prepared.request_sha256,
                prepared.mapping_diagnostics_json,
                record.provider_request.created_at.to_rfc3339(),
            ],
        )
        .map_err(storage_db_error)?;
    Ok(())
}

pub(crate) fn write_generation_prompt_plan(
    transaction: &Transaction<'_>,
    record: &GenerationPromptPlanRecord,
    knowledge_logs: &[KnowledgeActivationLog],
) -> CoreResult<GenerationPromptPlanLink> {
    let prepared = prepare_generation_prompt_plan(transaction, record)?;
    insert_generation_prompt_plan(transaction, record, &prepared)?;
    write_resolved_prompt_children(transaction, &record.id, &prepared.resolved)?;
    write_generation_knowledge_logs(transaction, record, knowledge_logs)?;
    seal_generation_prompt_plan(transaction, record, &prepared)?;
    Ok(GenerationPromptPlanLink {
        plan_id: record.id.clone(),
        plan_sha256: record.plan_sha256.clone(),
        provider_request_snapshot_id: record.provider_request.id.clone(),
    })
}

fn resolved_prompt_plan_hash(plan: &ResolvedPromptPlan) -> CoreResult<String> {
    #[derive(Serialize)]
    struct HashMaterial<'a> {
        schema_version: u32,
        preset_id: &'a PromptPresetId,
        generation_preset_id: &'a Option<GenerationPresetId>,
        effective_messages: &'a [lorepia_domain::ResolvedPromptMessage],
        cache_directives: &'a [lorepia_domain::ResolvedCacheDirective],
        trace: &'a lorepia_domain::PromptResolutionTrace,
        preview: &'a lorepia_domain::PromptPreview,
    }
    let material = HashMaterial {
        schema_version: plan.schema_version,
        preset_id: &plan.preset_id,
        generation_preset_id: &plan.generation_preset_id,
        effective_messages: &plan.effective_messages,
        cache_directives: &plan.cache_directives,
        trace: &plan.trace,
        preview: &plan.preview,
    };
    serde_json::to_vec(&material)
        .map(|bytes| sha256_hex(&bytes))
        .map_err(|error| CoreError::invalid(format!("cannot hash resolved prompt plan: {error}")))
}

fn write_resolved_prompt_children(
    transaction: &Transaction<'_>,
    plan_id: &str,
    resolved: &ResolvedPromptPlan,
) -> CoreResult<()> {
    write_resolved_prompt_messages(transaction, plan_id, resolved)?;
    write_resolved_cache_directives(transaction, plan_id, resolved)?;
    write_resolved_prompt_warnings(transaction, plan_id, resolved)
}

fn write_resolved_prompt_messages(
    transaction: &Transaction<'_>,
    plan_id: &str,
    resolved: &ResolvedPromptPlan,
) -> CoreResult<()> {
    for (ordinal, message) in resolved.effective_messages.iter().enumerate() {
        let ordinal = i64::try_from(ordinal)
            .map_err(|_| CoreError::invalid("too many resolved prompt messages"))?;
        let trace = resolved
            .trace
            .blocks
            .iter()
            .find(|trace| trace.block_id == message.block_id);
        let provenance_json = serde_json::to_string(&message.provenance).map_err(|error| {
            CoreError::invalid(format!("cannot encode prompt provenance: {error}"))
        })?;
        let payload_json = serde_json::to_string(message).map_err(|error| {
            CoreError::invalid(format!("cannot encode resolved prompt message: {error}"))
        })?;
        transaction
            .execute(
                "INSERT INTO generation_prompt_plan_blocks
                 (plan_id, ordinal, source_owner_revision_id, source_block_id,
                  kind, placement_zone, requested_role, disposition,
                  reduction_reason_json, content, content_sha256,
                  estimated_tokens, final_tokens, provenance_json, payload_json)
                 VALUES (
                     ?1, ?2, NULL, NULL, ?3, 'resolved', ?4, ?5, NULL,
                     ?6, ?7, ?8, ?8, ?9, ?10
                 )",
                params![
                    plan_id,
                    ordinal,
                    enum_wire(&message.block_kind)?,
                    enum_wire(&message.requested_role)?,
                    trace.map_or("included", |trace| {
                        block_resolution_disposition(trace.status)
                    }),
                    message.content,
                    sha256_hex(message.content.as_bytes()),
                    message.estimated_tokens,
                    provenance_json,
                    payload_json,
                ],
            )
            .map_err(storage_db_error)?;
        transaction
            .execute(
                "INSERT INTO generation_prompt_plan_messages
                 (plan_id, ordinal, role, content, content_sha256,
                  source_block_ordinals_json, source_message_id,
                  estimated_tokens)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
                params![
                    plan_id,
                    ordinal,
                    enum_wire(&message.effective_role)?,
                    message.content,
                    sha256_hex(message.content.as_bytes()),
                    format!("[{ordinal}]"),
                    message.source_message_ids.first().map(|id| id.0.as_str()),
                    message.estimated_tokens,
                ],
            )
            .map_err(storage_db_error)?;
    }
    Ok(())
}

fn write_resolved_cache_directives(
    transaction: &Transaction<'_>,
    plan_id: &str,
    resolved: &ResolvedPromptPlan,
) -> CoreResult<()> {
    for (ordinal, directive) in resolved.cache_directives.iter().enumerate() {
        let directive_json = serde_json::to_string(directive).map_err(|error| {
            CoreError::invalid(format!("cannot encode cache directive: {error}"))
        })?;
        let (disposition, warning_code) = match directive.status {
            lorepia_domain::CacheDirectiveStatus::Applied => ("applied", None),
            lorepia_domain::CacheDirectiveStatus::IgnoredUnsupported => {
                ("ignored", Some("unsupported"))
            }
            lorepia_domain::CacheDirectiveStatus::IgnoredLimit => ("ignored", Some("limit")),
            lorepia_domain::CacheDirectiveStatus::RemovedWithBlock => {
                ("ignored", Some("removed_with_block"))
            }
        };
        transaction
            .execute(
                "INSERT INTO generation_prompt_plan_directives
                 (plan_id, ordinal, directive_kind, source_owner_revision_id,
                  source_boundary_id, directive_json, disposition,
                  provider_mapping_json, warning_code)
                 VALUES (?1, ?2, 'cache', NULL, NULL, ?3, ?4, NULL, ?5)",
                params![
                    plan_id,
                    i64::try_from(ordinal)
                        .map_err(|_| CoreError::invalid("too many cache directives"))?,
                    directive_json,
                    disposition,
                    warning_code,
                ],
            )
            .map_err(storage_db_error)?;
    }
    Ok(())
}

fn write_resolved_prompt_warnings(
    transaction: &Transaction<'_>,
    plan_id: &str,
    resolved: &ResolvedPromptPlan,
) -> CoreResult<()> {
    for (ordinal, warning) in resolved.trace.warnings.iter().enumerate() {
        transaction
            .execute(
                "INSERT INTO generation_prompt_plan_warnings
                 (plan_id, ordinal, code, severity, message_key, details_json)
                 VALUES (?1, ?2, 'resolver_warning', 'warning', ?3, '{}')",
                params![
                    plan_id,
                    i64::try_from(ordinal)
                        .map_err(|_| CoreError::invalid("too many prompt warnings"))?,
                    warning
                ],
            )
            .map_err(storage_db_error)?;
    }
    Ok(())
}

const fn block_resolution_disposition(
    status: lorepia_domain::BlockResolutionStatus,
) -> &'static str {
    match status {
        lorepia_domain::BlockResolutionStatus::Included => "included",
        lorepia_domain::BlockResolutionStatus::TrimmedHead => "trimmed_head",
        lorepia_domain::BlockResolutionStatus::TrimmedTail
        | lorepia_domain::BlockResolutionStatus::ReducedItems => "trimmed_tail",
        lorepia_domain::BlockResolutionStatus::Summarized => "summarized",
        lorepia_domain::BlockResolutionStatus::ConditionFalse
        | lorepia_domain::BlockResolutionStatus::Disabled
        | lorepia_domain::BlockResolutionStatus::Empty
        | lorepia_domain::BlockResolutionStatus::DroppedForBudget => "dropped",
    }
}

fn write_generation_knowledge_logs(
    transaction: &Transaction<'_>,
    record: &GenerationPromptPlanRecord,
    logs: &[KnowledgeActivationLog],
) -> CoreResult<()> {
    for (ordinal, log) in logs.iter().enumerate() {
        require_generation_knowledge_log_authority(transaction, record, log)?;
        let (activation_source, score_millionths) = knowledge_activation_summary(&log.reasons);
        let reason_json = serde_json::to_string(&serde_json::json!({
            "log_id": log.id,
            "reasons": log.reasons,
            "exclusion_reason": log.exclusion_reason,
        }))
        .map_err(|error| {
            CoreError::invalid(format!(
                "cannot encode knowledge activation evidence: {error}"
            ))
        })?;
        let ordinal = i64::try_from(ordinal)
            .map_err(|_| CoreError::invalid("too many knowledge activation logs"))?;
        transaction
            .execute(
                "INSERT INTO knowledge_activation_logs
                 (plan_id, ordinal, book_revision_id, entry_id,
                  activation_source, selected, score_millionths,
                  estimated_tokens, reason_json, created_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
                params![
                    record.id,
                    ordinal,
                    log.book_revision_id,
                    log.entry_id.as_str(),
                    activation_source,
                    log.selected,
                    score_millionths,
                    log.estimated_tokens,
                    reason_json,
                    log.created_at.to_rfc3339(),
                ],
            )
            .map_err(storage_db_error)?;
        transaction
            .execute(
                "INSERT INTO generation_prompt_plan_knowledge_selections
                 (plan_id, ordinal, book_revision_id, entry_id, selected,
                  activation_source, score_millionths, estimated_tokens,
                  reason_json)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
                params![
                    record.id,
                    ordinal,
                    log.book_revision_id,
                    log.entry_id.as_str(),
                    log.selected,
                    activation_source,
                    score_millionths,
                    log.estimated_tokens,
                    reason_json,
                ],
            )
            .map_err(storage_db_error)?;
    }
    Ok(())
}

fn require_generation_knowledge_log_authority(
    transaction: &Transaction<'_>,
    record: &GenerationPromptPlanRecord,
    log: &KnowledgeActivationLog,
) -> CoreResult<()> {
    if log.conversation_id != record.conversation_id || log.branch_id != record.branch_id {
        return Err(CoreError::invalid(
            "knowledge activation log belongs to another conversation branch",
        ));
    }
    let exact_entry_exists = transaction
        .query_row(
            "SELECT EXISTS(
                SELECT 1
                FROM knowledge_book_revisions AS revision
                JOIN knowledge_entries AS entry
                  ON entry.book_revision_id = revision.revision_id
                WHERE revision.knowledge_book_id = ?1
                  AND revision.revision_id = ?2
                  AND entry.entry_id = ?3
             )",
            params![
                log.book_id.as_str(),
                log.book_revision_id,
                log.entry_id.as_str()
            ],
            |row| row.get::<_, bool>(0),
        )
        .map_err(storage_db_error)?;
    if !exact_entry_exists {
        return Err(CoreError::invalid(
            "knowledge activation log does not name an exact immutable book entry",
        ));
    }
    let active_book_revision_id =
        active_content_revision_id(transaction, log.book_id.as_str(), "knowledge_book")?;
    let selected_by_exact_module_plan =
        module_plan_selects_knowledge_revision(transaction, record, log)?;
    let selected_by_exact_prompt_preset =
        prompt_preset_selects_knowledge_revision(transaction, record, log)?;
    let selected_by_generation_attempt =
        generation_attempt_selects_knowledge_revision(transaction, record, log)?;
    if active_book_revision_id != log.book_revision_id
        && !selected_by_exact_module_plan
        && !selected_by_exact_prompt_preset
        && !selected_by_generation_attempt
    {
        return Err(CoreError::invalid(
            "knowledge book changed after prompt resolution; resolve a new prompt plan",
        ));
    }
    Ok(())
}

fn generation_attempt_selects_knowledge_revision(
    transaction: &Transaction<'_>,
    record: &GenerationPromptPlanRecord,
    log: &KnowledgeActivationLog,
) -> CoreResult<bool> {
    let attempt = crate::generation_attempt::read_attempt(transaction, &record.generation_id)?;
    if attempt.input.conversation_id != record.conversation_id
        || attempt.input.proposed_branch_id != record.branch_id
    {
        return Err(storage_corrupted(
            "generation prompt plan is detached from its attempt authority",
        ));
    }
    Ok(attempt
        .input
        .prompt_selection_authority
        .as_ref()
        .and_then(|authority| authority.character_knowledge_book.as_ref())
        .is_some_and(|book| {
            book.value.id == log.book_id
                && book.revision_id.as_deref() == Some(log.book_revision_id.as_str())
        }))
}

fn prompt_preset_selects_knowledge_revision(
    transaction: &Transaction<'_>,
    record: &GenerationPromptPlanRecord,
    log: &KnowledgeActivationLog,
) -> CoreResult<bool> {
    transaction
        .query_row(
            "SELECT EXISTS(
                SELECT 1
                FROM prompt_preset_knowledge_books AS dependency
                JOIN prompt_preset_revisions AS preset
                  ON preset.revision_id = dependency.prompt_preset_revision_id
                JOIN knowledge_book_revisions AS book
                  ON book.revision_id = dependency.knowledge_book_revision_id
                WHERE dependency.prompt_preset_revision_id = ?1
                  AND preset.prompt_preset_id = ?2
                  AND dependency.enabled = 1
                  AND book.knowledge_book_id = ?3
                  AND dependency.knowledge_book_revision_id = ?4
             )",
            params![
                record.prompt_preset_revision_id,
                record.prompt_preset_id.as_str(),
                log.book_id.as_str(),
                log.book_revision_id,
            ],
            |row| row.get::<_, bool>(0),
        )
        .map_err(storage_db_error)
}

fn module_plan_selects_knowledge_revision(
    transaction: &Transaction<'_>,
    record: &GenerationPromptPlanRecord,
    log: &KnowledgeActivationLog,
) -> CoreResult<bool> {
    let Some(runtime) = load_generation_module_plan_evidence(transaction, record)? else {
        return Ok(false);
    };
    for component in &runtime.plan.components {
        let lorepia_domain::ModuleComponentRef::KnowledgeBook { id } = &component.component else {
            continue;
        };
        if id != &log.book_id {
            continue;
        }
        let exact = transaction
            .query_row(
                "SELECT component.knowledge_book_revision_id,
                        component.component_sha256, revision.source_hash,
                        book.knowledge_book_id
                 FROM content_module_revisions AS revision
                 JOIN content_module_components AS component
                   ON component.module_revision_id = revision.revision_id
                  AND component.component_kind = 'knowledge_book'
                 JOIN knowledge_book_revisions AS book
                   ON book.revision_id = component.knowledge_book_revision_id
                 WHERE revision.module_id = ?1
                   AND revision.revision_id = ?2
                   AND book.knowledge_book_id = ?3",
                params![
                    component.selected_source.module_id.as_str(),
                    component.selected_source.revision_id.as_str(),
                    log.book_id.as_str(),
                ],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, String>(3)?,
                    ))
                },
            )
            .optional()
            .map_err(storage_db_error)?;
        if let Some(exact) = exact {
            if exact.1 != component.sha256.as_str()
                || exact.2 != component.selected_source.revision_source_sha256.as_str()
                || exact.3 != log.book_id.as_str()
            {
                return Err(storage_corrupted(
                    "applied module knowledge component diverges from its exact revision",
                ));
            }
            return Ok(exact.0 == log.book_revision_id);
        }
        return Err(storage_corrupted(
            "applied module plan knowledge component cannot be resolved exactly",
        ));
    }
    Ok(false)
}

fn load_generation_module_plan_evidence(
    transaction: &Transaction<'_>,
    record: &GenerationPromptPlanRecord,
) -> CoreResult<Option<lorepia_orchestration::AppliedModuleRuntimePlan>> {
    let module_plan_sha256 = match record
        .provider_request
        .mapping_diagnostics
        .value
        .get("module_plan_sha256")
    {
        None | Some(Value::Null) => return Ok(None),
        Some(Value::String(value)) => value,
        Some(_) => {
            return Err(CoreError::invalid(
                "generation module plan evidence must be a SHA-256 string or null",
            ));
        }
    };
    validate_optional_sha256("generation module plan hash", Some(module_plan_sha256))?;
    let applied_plan_sha256 = lorepia_domain::Sha256Digest::parse(module_plan_sha256.to_owned())
        .map_err(|_| CoreError::invalid("generation module plan hash is not canonical"))?;
    let runtime =
        match load_applied_module_runtime_plan_transaction(transaction, &applied_plan_sha256) {
            Ok(runtime) => runtime,
            Err(error) if error.code == CoreErrorCode::NotFound => {
                return Err(CoreError::invalid(
                    "generation references an unknown applied module runtime plan",
                ));
            }
            Err(error) => return Err(error),
        };
    if runtime.review.context.conversation_id.as_deref() != Some(record.conversation_id.0.as_str())
        || runtime.review.context.branch_id.as_deref() != Some(record.branch_id.0.as_str())
    {
        return Err(CoreError::invalid(
            "generation module plan belongs to another resolution context",
        ));
    }
    Ok(Some(runtime))
}

fn knowledge_activation_summary(
    reasons: &[KnowledgeActivationReason],
) -> (&'static str, Option<u32>) {
    match reasons.first() {
        Some(KnowledgeActivationReason::Always) | None => ("always", None),
        Some(KnowledgeActivationReason::Manual) => ("manual", None),
        Some(KnowledgeActivationReason::Keyword { .. }) => ("keyword", None),
        Some(KnowledgeActivationReason::Regex { .. }) => ("regex", None),
        Some(KnowledgeActivationReason::Semantic { score_millionths }) => {
            ("semantic", Some(*score_millionths))
        }
        Some(KnowledgeActivationReason::Condition) => ("condition", None),
        Some(KnowledgeActivationReason::Recursive { .. }) => ("recursive", None),
    }
}

fn parse_api_family(value: &str) -> CoreResult<ApiFamily> {
    match value {
        "open_ai_responses" | "openai_responses" => Ok(ApiFamily::OpenAiResponses),
        "open_ai_chat_completions" | "openai_chat_completions" => {
            Ok(ApiFamily::OpenAiChatCompletions)
        }
        "anthropic_messages" => Ok(ApiFamily::AnthropicMessages),
        "gemini_generate_content" => Ok(ApiFamily::GeminiGenerateContent),
        "ollama_native" => Ok(ApiFamily::OllamaNative),
        _ => Err(storage_corrupted(format!(
            "stored provider API family is invalid: {value}"
        ))),
    }
}

const fn api_family_str(value: ApiFamily) -> &'static str {
    match value {
        ApiFamily::OpenAiResponses => "openai_responses",
        ApiFamily::OpenAiChatCompletions => "openai_chat_completions",
        ApiFamily::AnthropicMessages => "anthropic_messages",
        ApiFamily::GeminiGenerateContent => "gemini_generate_content",
        ApiFamily::OllamaNative => "ollama_native",
    }
}

fn nonnegative_u32(label: &str, value: i64) -> CoreResult<u32> {
    u32::try_from(value).map_err(|_| storage_corrupted(format!("stored {label} is invalid")))
}

fn positive_u32(label: &str, value: i64) -> CoreResult<u32> {
    let value = nonnegative_u32(label, value)?;
    if value == 0 {
        Err(storage_corrupted(format!("stored {label} is zero")))
    } else {
        Ok(value)
    }
}

#[derive(Debug)]
struct RawMemoryRecord {
    document_json: String,
    state_version: i64,
    active_revision_id: String,
    created_at: String,
    updated_at: String,
    deleted_at: Option<String>,
    pinned: bool,
    invalidated_at: Option<String>,
    excluded_from_conversation_at: Option<String>,
    excluded_from_character_at: Option<String>,
}

fn raw_memory_record(row: &rusqlite::Row<'_>) -> rusqlite::Result<RawMemoryRecord> {
    Ok(RawMemoryRecord {
        document_json: row.get(0)?,
        state_version: row.get(1)?,
        active_revision_id: row.get(2)?,
        created_at: row.get(3)?,
        updated_at: row.get(4)?,
        deleted_at: row.get(5)?,
        pinned: row.get(6)?,
        invalidated_at: row.get(7)?,
        excluded_from_conversation_at: row.get(8)?,
        excluded_from_character_at: row.get(9)?,
    })
}

fn decode_memory_record(raw: RawMemoryRecord) -> CoreResult<StoredRevision<MemoryRecord>> {
    let mut value = decode_document::<MemoryRecord>("memory record", &raw.document_json)?;
    value.created_at = parse_datetime("memory record created_at", &raw.created_at)?;
    value.updated_at = parse_datetime("memory record updated_at", &raw.updated_at)?;
    value.pinned = raw.pinned;
    value.invalidated_at = raw
        .invalidated_at
        .as_deref()
        .map(|value| parse_datetime("memory invalidated_at", value))
        .transpose()?;
    value.excluded_from_conversation = raw.excluded_from_conversation_at.is_some();
    value.excluded_from_character = raw.excluded_from_character_at.is_some();
    Ok(StoredRevision {
        value,
        revision: u64_revision(raw.state_version)?,
        revision_id: Some(raw.active_revision_id),
        created_at: parse_datetime("memory record created_at", &raw.created_at)?,
        updated_at: parse_datetime("memory record updated_at", &raw.updated_at)?,
        deleted_at: raw
            .deleted_at
            .as_deref()
            .map(|value| parse_datetime("memory record deleted_at", value))
            .transpose()?,
    })
}

fn get_memory_record(
    storage: &Storage,
    id: &MemoryRecordId,
    include_deleted: bool,
    owner: Option<(&ConversationId, &ConversationBranchId)>,
) -> CoreResult<StoredRevision<MemoryRecord>> {
    let deleted_clause = if include_deleted {
        ""
    } else {
        " AND state.deleted_at IS NULL"
    };
    let sql = format!(
        "SELECT revision.document_json, state.state_version,
                state.active_revision_id, record.created_at, state.updated_at,
                state.deleted_at, state.pinned, state.invalidated_at,
                state.excluded_from_conversation_at,
                state.excluded_from_character_at
         FROM memory_records AS record
         JOIN memory_record_state AS state ON state.record_id = record.id
         JOIN memory_record_revisions AS revision
           ON revision.record_id = record.id
          AND revision.id = state.active_revision_id
         WHERE record.id = ?1
           AND (?2 IS NULL OR (
               record.conversation_id = ?2 AND record.branch_id = ?3
           )){deleted_clause}"
    );
    let (conversation_id, branch_id) =
        owner.map_or((None, None), |(conversation_id, branch_id)| {
            (Some(conversation_id.0.as_str()), Some(branch_id.0.as_str()))
        });
    let raw = storage
        .connection()?
        .query_row(
            &sql,
            params![id.as_str(), conversation_id, branch_id],
            raw_memory_record,
        )
        .optional()
        .map_err(storage_db_error)?
        .ok_or_else(|| not_found("memory record"))?;
    decode_memory_record(raw)
}

fn normalize_memory_keywords(keywords: &[String]) -> CoreResult<Vec<(String, String)>> {
    let mut seen = BTreeSet::new();
    let mut normalized = Vec::with_capacity(keywords.len());
    for keyword in keywords {
        let trimmed = keyword.trim();
        if trimmed.is_empty() || trimmed.chars().count() > 512 {
            return Err(CoreError::invalid(
                "memory keywords must be non-empty and bounded",
            ));
        }
        let folded = trimmed.to_lowercase();
        if !seen.insert(folded.clone()) {
            return Err(CoreError::invalid(
                "memory keywords must be unique after normalization",
            ));
        }
        normalized.push((trimmed.to_owned(), folded));
    }
    Ok(normalized)
}

fn validate_memory_source_range(connection: &Connection, record: &MemoryRecord) -> CoreResult<()> {
    let head = connection
        .query_row(
            "SELECT head_message_id
             FROM conversation_branches
             WHERE conversation_id = ?1 AND id = ?2",
            params![record.conversation_id.0, record.branch_id.0],
            |row| row.get::<_, Option<String>>(0),
        )
        .optional()
        .map_err(storage_db_error)?
        .ok_or_else(|| not_found("memory record branch"))?
        .ok_or_else(|| CoreError::invalid("memory record branch has no messages"))?;
    if !message_is_ancestor(
        connection,
        &record.conversation_id,
        &record.source_end_message_id,
        &MessageId(head),
    )? || !message_is_ancestor(
        connection,
        &record.conversation_id,
        &record.source_start_message_id,
        &record.source_end_message_id,
    )? {
        return Err(CoreError::invalid(
            "memory source range is not an ordered range on its branch lineage",
        ));
    }
    Ok(())
}

fn message_is_ancestor(
    connection: &Connection,
    conversation_id: &ConversationId,
    ancestor_id: &MessageId,
    descendant_id: &MessageId,
) -> CoreResult<bool> {
    connection
        .query_row(
            "WITH RECURSIVE lineage(id, parent_id) AS (
                 SELECT id, parent_id
                 FROM messages
                 WHERE conversation_id = ?1 AND id = ?2
                 UNION
                 SELECT parent.id, parent.parent_id
                 FROM messages AS parent
                 JOIN lineage AS child ON child.parent_id = parent.id
                 WHERE parent.conversation_id = ?1
             )
             SELECT EXISTS(SELECT 1 FROM lineage WHERE id = ?3)",
            params![conversation_id.0, descendant_id.0, ancestor_id.0],
            |row| row.get(0),
        )
        .map_err(storage_db_error)
}

struct CurrentMemoryRecordRevision {
    conversation_id: String,
    branch_id: String,
    source_start_message_id: String,
    source_end_message_id: String,
    kind: String,
    created_at: String,
    state_version: i64,
    active_revision_id: String,
    deleted_at: Option<String>,
    document_json: String,
}

fn current_memory_record_revision(
    transaction: &Transaction<'_>,
    id: &MemoryRecordId,
) -> CoreResult<Option<CurrentMemoryRecordRevision>> {
    transaction
        .query_row(
            "SELECT record.conversation_id, record.branch_id,
                    record.source_start_message_id, record.source_end_message_id,
                    record.kind, record.created_at, state.state_version,
                    state.active_revision_id, state.deleted_at,
                    revision.document_json
             FROM memory_records AS record
             JOIN memory_record_state AS state ON state.record_id = record.id
             JOIN memory_record_revisions AS revision
               ON revision.id = state.active_revision_id
             WHERE record.id = ?1",
            [id.as_str()],
            |row| {
                Ok(CurrentMemoryRecordRevision {
                    conversation_id: row.get(0)?,
                    branch_id: row.get(1)?,
                    source_start_message_id: row.get(2)?,
                    source_end_message_id: row.get(3)?,
                    kind: row.get(4)?,
                    created_at: row.get(5)?,
                    state_version: row.get(6)?,
                    active_revision_id: row.get(7)?,
                    deleted_at: row.get(8)?,
                    document_json: row.get(9)?,
                })
            },
        )
        .optional()
        .map_err(storage_db_error)
}

struct MemoryRevisionContext {
    next_state_version: u64,
    revision_no: u64,
    parent_revision_id: Option<String>,
    created_at: DateTime<Utc>,
    previous: Option<MemoryRecord>,
}

fn resolve_memory_revision_context(
    transaction: &Transaction<'_>,
    record: &MemoryRecord,
    expected_revision: Option<u64>,
    current: Option<CurrentMemoryRecordRevision>,
) -> CoreResult<MemoryRevisionContext> {
    match (expected_revision, current) {
        (None, None) => Ok(MemoryRevisionContext {
            next_state_version: 1,
            revision_no: 1,
            parent_revision_id: None,
            created_at: record.created_at,
            previous: None,
        }),
        (None, Some(current)) => Err(revision_conflict(
            "memory record",
            record.id.as_str(),
            None,
            Some(u64_revision(current.state_version)?),
        )),
        (Some(expected), None) => Err(revision_conflict(
            "memory record",
            record.id.as_str(),
            Some(expected),
            None,
        )),
        (Some(expected), Some(current)) => {
            resolve_existing_memory_revision_context(transaction, record, expected, current)
        }
    }
}

fn resolve_existing_memory_revision_context(
    transaction: &Transaction<'_>,
    record: &MemoryRecord,
    expected: u64,
    current: CurrentMemoryRecordRevision,
) -> CoreResult<MemoryRevisionContext> {
    let actual = u64_revision(current.state_version)?;
    if current.deleted_at.is_some() || actual != expected {
        return Err(revision_conflict(
            "memory record",
            record.id.as_str(),
            Some(expected),
            Some(actual),
        ));
    }
    if current.conversation_id != record.conversation_id.0
        || current.branch_id != record.branch_id.0
        || current.source_start_message_id != record.source_start_message_id.0
        || current.source_end_message_id != record.source_end_message_id.0
        || current.kind != enum_wire(&record.kind)?
    {
        return Err(CoreError::invalid(
            "memory record identity and source range are immutable",
        ));
    }
    let latest_revision_no = transaction
        .query_row(
            "SELECT MAX(revision_no)
             FROM memory_record_revisions WHERE record_id = ?1",
            [record.id.as_str()],
            |row| row.get::<_, i64>(0),
        )
        .map_err(storage_db_error)?;
    Ok(MemoryRevisionContext {
        next_state_version: expected
            .checked_add(1)
            .ok_or_else(|| CoreError::internal("memory state revision overflow"))?,
        revision_no: u64_revision(latest_revision_no)?
            .checked_add(1)
            .ok_or_else(|| CoreError::internal("memory revision overflow"))?,
        parent_revision_id: Some(current.active_revision_id),
        created_at: parse_datetime("memory record created_at", &current.created_at)?,
        previous: Some(decode_document::<MemoryRecord>(
            "memory record",
            &current.document_json,
        )?),
    })
}

fn insert_memory_record_identity(
    transaction: &Transaction<'_>,
    value: &MemoryRecord,
) -> CoreResult<()> {
    transaction
        .execute(
            "INSERT INTO memory_records
             (id, conversation_id, branch_id, source_start_message_id,
              source_end_message_id, kind, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![
                value.id.as_str(),
                value.conversation_id.0,
                value.branch_id.0,
                value.source_start_message_id.0,
                value.source_end_message_id.0,
                enum_wire(&value.kind)?,
                value.created_at.to_rfc3339(),
            ],
        )
        .map_err(storage_db_error)?;
    Ok(())
}

fn insert_memory_record_revision(
    transaction: &Transaction<'_>,
    value: &MemoryRecord,
    context: &MemoryRevisionContext,
    revision_id: &str,
) -> CoreResult<()> {
    let (document_json, content_sha256) = encode_document("memory record", value)?;
    let (structured_data_json, _) =
        encode_document("memory structured data", &value.structured_data)?;
    let (provenance_json, _) = encode_document("memory provenance", &value.provenance)?;
    transaction
        .execute(
            "INSERT INTO memory_record_revisions
             (id, record_id, revision_no, parent_revision_id, title, summary,
              structured_data_json, importance, content_sha256,
              provenance_json, document_json, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
            params![
                revision_id,
                value.id.as_str(),
                i64_revision(context.revision_no)?,
                context.parent_revision_id,
                value.title,
                value.summary,
                structured_data_json,
                value.importance,
                content_sha256,
                provenance_json,
                document_json,
                value.updated_at.to_rfc3339(),
            ],
        )
        .map_err(storage_db_error)?;
    Ok(())
}

fn insert_memory_record_keywords(
    transaction: &Transaction<'_>,
    revision_id: &str,
    keywords: &[(String, String)],
) -> CoreResult<()> {
    for (ordinal, (keyword, normalized_keyword)) in keywords.iter().enumerate() {
        transaction
            .execute(
                "INSERT INTO memory_record_keywords
                 (record_revision_id, ordinal, keyword, normalized_keyword)
                 VALUES (?1, ?2, ?3, ?4)",
                params![
                    revision_id,
                    i64::try_from(ordinal)
                        .map_err(|_| CoreError::invalid("too many memory keywords"))?,
                    keyword,
                    normalized_keyword,
                ],
            )
            .map_err(storage_db_error)?;
    }
    Ok(())
}

fn write_memory_record_state(
    transaction: &Transaction<'_>,
    value: &MemoryRecord,
    context: &MemoryRevisionContext,
    revision_id: &str,
    expected_revision: Option<u64>,
) -> CoreResult<()> {
    let invalidation_reason = value.invalidated_at.map(|_| "record_update");
    let excluded_conversation_at = value
        .excluded_from_conversation
        .then(|| value.updated_at.to_rfc3339());
    let excluded_character_at = value
        .excluded_from_character
        .then(|| value.updated_at.to_rfc3339());
    let Some(expected_revision) = expected_revision else {
        transaction
            .execute(
                "INSERT INTO memory_record_state
                 (record_id, active_revision_id, pinned, invalidated_at,
                  invalidation_reason, excluded_from_conversation_at,
                  excluded_from_character_at, deleted_at, state_version,
                  updated_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, NULL, 1, ?8)",
                params![
                    value.id.as_str(),
                    revision_id,
                    value.pinned,
                    value.invalidated_at.map(|time| time.to_rfc3339()),
                    invalidation_reason,
                    excluded_conversation_at,
                    excluded_character_at,
                    value.updated_at.to_rfc3339(),
                ],
            )
            .map_err(storage_db_error)?;
        return Ok(());
    };
    let changed = transaction
        .execute(
            "UPDATE memory_record_state
             SET active_revision_id = ?2, pinned = ?3, invalidated_at = ?4,
                 invalidation_reason = ?5,
                 excluded_from_conversation_at = ?6,
                 excluded_from_character_at = ?7,
                 state_version = ?8, updated_at = ?9
             WHERE record_id = ?1 AND state_version = ?10
               AND deleted_at IS NULL",
            params![
                value.id.as_str(),
                revision_id,
                value.pinned,
                value.invalidated_at.map(|time| time.to_rfc3339()),
                invalidation_reason,
                excluded_conversation_at,
                excluded_character_at,
                i64_revision(context.next_state_version)?,
                value.updated_at.to_rfc3339(),
                i64_revision(expected_revision)?,
            ],
        )
        .map_err(storage_db_error)?;
    if changed != 1 {
        return Err(revision_conflict(
            "memory record",
            value.id.as_str(),
            Some(expected_revision),
            None,
        ));
    }
    Ok(())
}

fn memory_record_event_kind(previous: Option<&MemoryRecord>, value: &MemoryRecord) -> &'static str {
    match previous {
        None => "created",
        Some(previous) if previous.invalidated_at.is_some() && value.invalidated_at.is_none() => {
            "restored"
        }
        Some(previous) if previous.pinned != value.pinned => {
            if value.pinned {
                "pinned"
            } else {
                "unpinned"
            }
        }
        Some(previous)
            if !previous.excluded_from_conversation && value.excluded_from_conversation =>
        {
            "excluded_conversation"
        }
        Some(previous) if !previous.excluded_from_character && value.excluded_from_character => {
            "excluded_character"
        }
        _ => "edited",
    }
}

fn save_memory_record(
    storage: &Storage,
    record: &MemoryRecord,
    expected_revision: Option<u64>,
) -> CoreResult<StoredRevision<MemoryRecord>> {
    record
        .validate()
        .map_err(|error| CoreError::invalid(error.to_string()))?;
    let keywords = normalize_memory_keywords(&record.keywords)?;
    let mut connection = storage.connection()?;
    let transaction = connection
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(storage_db_error)?;
    validate_memory_source_range(&transaction, record)?;
    let current = current_memory_record_revision(&transaction, &record.id)?;
    let context =
        resolve_memory_revision_context(&transaction, record, expected_revision, current)?;
    let mut value = record.clone();
    value.created_at = context.created_at;
    if value.updated_at < context.created_at {
        return Err(CoreError::invalid(
            "memory record update time predates creation",
        ));
    }
    let revision_id = Uuid::new_v4().to_string();
    if expected_revision.is_none() {
        insert_memory_record_identity(&transaction, &value)?;
    }
    insert_memory_record_revision(&transaction, &value, &context, &revision_id)?;
    insert_memory_record_keywords(&transaction, &revision_id, &keywords)?;
    write_memory_record_state(
        &transaction,
        &value,
        &context,
        &revision_id,
        expected_revision,
    )?;
    let event_kind = memory_record_event_kind(context.previous.as_ref(), &value);
    append_memory_event(
        &transaction,
        value.id.as_str(),
        event_kind,
        context.parent_revision_id.as_deref(),
        Some(&revision_id),
        serde_json::json!({"state_version": context.next_state_version}),
        value.updated_at,
    )?;
    transaction.commit().map_err(storage_db_error)?;
    Ok(StoredRevision {
        value,
        revision: context.next_state_version,
        revision_id: Some(revision_id),
        created_at: context.created_at,
        updated_at: record.updated_at,
        deleted_at: None,
    })
}

fn append_memory_event(
    transaction: &Transaction<'_>,
    record_id: &str,
    event_kind: &str,
    from_revision_id: Option<&str>,
    to_revision_id: Option<&str>,
    payload: Value,
    created_at: DateTime<Utc>,
) -> CoreResult<()> {
    let payload_json = serde_json::to_string(&payload)
        .map_err(|error| CoreError::invalid(format!("cannot encode memory event: {error}")))?;
    transaction
        .execute(
            "INSERT INTO memory_record_events
             (record_id, sequence, event_kind, from_revision_id, to_revision_id,
              payload_json, created_at)
             VALUES (
                 ?1,
                 (SELECT COALESCE(MAX(sequence), 0) + 1
                  FROM memory_record_events WHERE record_id = ?1),
                 ?2, ?3, ?4, ?5, ?6
             )",
            params![
                record_id,
                event_kind,
                from_revision_id,
                to_revision_id,
                payload_json,
                created_at.to_rfc3339(),
            ],
        )
        .map_err(storage_db_error)?;
    Ok(())
}

#[derive(Serialize)]
struct MemoryRecordsAtHeadSnapshotDigest<'a> {
    schema_version: u32,
    conversation_id: &'a ConversationId,
    source_branch_id: &'a ConversationBranchId,
    context_head_message_id: Option<&'a MessageId>,
    include_invalidated: bool,
    records: &'a [MemoryRecordAtHeadEvidence],
}

pub fn memory_records_at_head_snapshot_sha256(
    snapshot: &MemoryRecordsAtHeadSnapshot,
) -> CoreResult<String> {
    if snapshot.schema_version != 1 || snapshot.records.len() > MAX_MEMORY_RECORDS {
        return Err(CoreError::invalid(
            "memory head snapshot schema or record count is invalid",
        ));
    }
    let json = serde_json::to_string(&MemoryRecordsAtHeadSnapshotDigest {
        schema_version: snapshot.schema_version,
        conversation_id: &snapshot.conversation_id,
        source_branch_id: &snapshot.source_branch_id,
        context_head_message_id: snapshot.context_head_message_id.as_ref(),
        include_invalidated: snapshot.include_invalidated,
        records: &snapshot.records,
    })
    .map_err(|error| CoreError::internal(format!("cannot encode memory head snapshot: {error}")))?;
    if json.len() > 8 * 1_024 * 1_024 {
        return Err(CoreError::invalid(
            "memory head snapshot exceeds its byte limit",
        ));
    }
    Ok(sha256_hex(json.as_bytes()))
}

pub(crate) fn require_memory_records_at_head_snapshot_transaction(
    transaction: &Transaction<'_>,
    expected: &MemoryRecordsAtHeadSnapshot,
) -> CoreResult<()> {
    if memory_records_at_head_snapshot_sha256(expected)? != expected.snapshot_sha256 {
        return Err(CoreError::invalid(
            "memory head snapshot fingerprint is invalid",
        ));
    }
    let current = memory_records_at_head_in_connection(
        transaction,
        &expected.conversation_id,
        &expected.source_branch_id,
        expected.context_head_message_id.as_ref(),
        expected.include_invalidated,
    )?;
    if current.snapshot != *expected {
        return Err(CoreError::invalid(
            "memory records changed after generation preparation",
        ));
    }
    Ok(())
}

/// Rechecks every mutable prompt-context authority in the same transaction
/// that makes an attempt-bound generation visible.
///
/// Prompt text remains sealed only in the immutable resolved plan. This gate
/// compares the content-free source identities captured by Core so a room
/// binding, persona selection, summary revision, or local identity cannot
/// drift between preparation and dispatch.
#[cfg(test)]
pub(crate) fn require_generation_prompt_context_snapshot_transaction(
    transaction: &Transaction<'_>,
    record: &GenerationPromptPlanRecord,
    expected_source_branch_id: &ConversationBranchId,
    expected_context_head_message_id: Option<&MessageId>,
    local_user_id: &LocalUserId,
) -> CoreResult<()> {
    let resolved: ResolvedPromptPlan = serde_json::from_value(record.plan.value.clone())
        .map_err(|error| CoreError::invalid(format!("resolved prompt plan is invalid: {error}")))?;
    resolved
        .validate()
        .map_err(|error| CoreError::invalid(format!("resolved prompt plan is invalid: {error}")))?;
    let expected = resolved.trace.context_snapshot.as_ref().ok_or_else(|| {
        CoreError::invalid("attempt-bound prompt plan is missing its context snapshot")
    })?;
    require_prompt_context_snapshot_identity(
        expected,
        record,
        expected_source_branch_id,
        expected_context_head_message_id,
        local_user_id,
    )?;
    let persona_id = require_prompt_context_persona(transaction, expected)?;
    require_prompt_context_binding(transaction, record, expected, persona_id.as_deref())?;
    require_prompt_context_summaries(transaction, expected)
}

/// Validates an attempt-bound prompt against the immutable authority captured
/// before its approval pause. Mutable binding, persona-selection, settings,
/// and memory heads are deliberately not re-read here; their exact identities
/// are carried by the attempt and its `BeforeGeneration` snapshot.
pub(crate) struct SealedGenerationPromptContext<'a> {
    pub(crate) conversation_id: &'a ConversationId,
    pub(crate) target_branch_id: &'a ConversationBranchId,
    pub(crate) source_branch_id: &'a ConversationBranchId,
    pub(crate) context_head_message_id: Option<&'a MessageId>,
    pub(crate) authority: &'a crate::generation_attempt::GenerationPromptSelectionAuthority,
    pub(crate) memory_snapshot: &'a MemoryRecordsAtHeadSnapshot,
}

pub(crate) fn require_sealed_generation_prompt_context_snapshot_transaction(
    transaction: &Transaction<'_>,
    record: &GenerationPromptPlanRecord,
    context: SealedGenerationPromptContext<'_>,
) -> CoreResult<()> {
    let expected = sealed_prompt_context_snapshot(record)?;
    require_sealed_prompt_context_identity(record, &expected, &context)?;
    let sealed_binding = sealed_prompt_binding(context.authority)?;
    if expected.binding != sealed_binding {
        return Err(prompt_context_changed(
            "prompt binding differs from its sealed generation authority",
        ));
    }
    let sealed_persona = sealed_prompt_persona(transaction, context.authority)?;
    if expected.persona != sealed_persona {
        return Err(prompt_context_changed(
            "prompt persona differs from its sealed generation authority",
        ));
    }
    require_sealed_prompt_memory_context(&expected, &context)
}

fn sealed_prompt_context_snapshot(
    record: &GenerationPromptPlanRecord,
) -> CoreResult<PromptContextSnapshotV1> {
    let resolved: ResolvedPromptPlan = serde_json::from_value(record.plan.value.clone())
        .map_err(|error| CoreError::invalid(format!("resolved prompt plan is invalid: {error}")))?;
    resolved
        .validate()
        .map_err(|error| CoreError::invalid(format!("resolved prompt plan is invalid: {error}")))?;
    resolved.trace.context_snapshot.ok_or_else(|| {
        CoreError::invalid("attempt-bound prompt plan is missing its context snapshot")
    })
}

fn require_sealed_prompt_context_identity(
    record: &GenerationPromptPlanRecord,
    expected: &PromptContextSnapshotV1,
    context: &SealedGenerationPromptContext<'_>,
) -> CoreResult<()> {
    if record.conversation_id != *context.conversation_id
        || record.branch_id != *context.target_branch_id
        || record.head_message_id.as_ref() != context.context_head_message_id
        || record.prompt_preset_id != context.authority.preset.id
        || record.prompt_preset_revision_id != context.authority.preset_revision_id
        || expected.schema_version != 1
        || expected.conversation_id != *context.conversation_id
        || expected.source_branch_id != *context.source_branch_id
        || expected.context_head_message_id.as_ref() != context.context_head_message_id
        || expected.local_user_id_sha256 != context.authority.local_user_id_sha256
        || prompt_context_snapshot_sha256(expected).map_err(|error| {
            CoreError::invalid(format!("prompt context snapshot is invalid: {error}"))
        })? != expected.snapshot_sha256
    {
        return Err(prompt_context_changed(
            "sealed prompt context identity differs from its generation attempt",
        ));
    }
    Ok(())
}

fn sealed_prompt_binding(
    authority: &crate::generation_attempt::GenerationPromptSelectionAuthority,
) -> CoreResult<Option<PromptContextBindingEvidence>> {
    authority
        .binding
        .as_ref()
        .map(|binding| {
            Ok(PromptContextBindingEvidence {
                binding_id: binding.value.id.clone(),
                binding_revision: binding.revision,
                document_sha256: binding.value.canonical_document_sha256()?,
            })
        })
        .transpose()
}

fn sealed_prompt_persona(
    transaction: &Transaction<'_>,
    authority: &crate::generation_attempt::GenerationPromptSelectionAuthority,
) -> CoreResult<Option<PromptContextPersonaEvidence>> {
    authority
        .persona_selection
        .as_ref()
        .map(|selection| {
            let revision_id = selection.revision_id.as_deref().ok_or_else(|| {
                storage_corrupted("sealed prompt persona selection has no revision identity")
            })?;
            let document_sha256 = transaction
                .query_row(
                    "SELECT document_sha256
                     FROM content_revisions
                     WHERE object_id = ?1 AND id = ?2 AND object_kind = 'persona'",
                    params![selection.value.persona_id.as_str(), revision_id],
                    |row| row.get::<_, String>(0),
                )
                .optional()
                .map_err(storage_db_error)?
                .ok_or_else(|| storage_corrupted("sealed prompt persona revision is missing"))?;
            Ok(PromptContextPersonaEvidence {
                selection_revision: selection.revision,
                persona_id: selection.value.persona_id.clone(),
                persona_revision_id: revision_id.to_owned(),
                persona_sha256: document_sha256,
            })
        })
        .transpose()
}

fn require_sealed_prompt_memory_context(
    expected: &PromptContextSnapshotV1,
    context: &SealedGenerationPromptContext<'_>,
) -> CoreResult<()> {
    let memory_snapshot = context.memory_snapshot;
    if memory_snapshot.conversation_id != *context.conversation_id
        || memory_snapshot.source_branch_id != *context.source_branch_id
        || memory_snapshot.context_head_message_id.as_ref() != context.context_head_message_id
        || memory_snapshot.include_invalidated
        || memory_records_at_head_snapshot_sha256(memory_snapshot)?
            != memory_snapshot.snapshot_sha256
    {
        return Err(storage_corrupted(
            "generation memory snapshot differs from its sealed prompt boundary",
        ));
    }
    for summary in &expected.summaries {
        let matches = memory_snapshot.records.iter().any(|record| {
            summary.summary_id == record.record_id
                && summary.record_branch_id == record.record_branch_id
                && summary.source_start_message_id == record.source_start_message_id
                && summary.source_end_message_id == record.source_end_message_id
                && summary.state_revision == record.state_revision
                && summary.active_revision_id == record.active_revision_id
                && summary.active_revision_sha256 == record.active_revision_sha256
        });
        if !matches {
            return Err(prompt_context_changed(
                "prompt summary differs from its sealed memory snapshot",
            ));
        }
    }
    if expected.conversation_summary_id.as_ref().is_some_and(|id| {
        !expected
            .summaries
            .iter()
            .any(|summary| &summary.summary_id == id)
    }) {
        return Err(storage_corrupted(
            "sealed prompt conversation summary has no exact revision evidence",
        ));
    }
    Ok(())
}

#[cfg(test)]
fn require_prompt_context_snapshot_identity(
    expected: &PromptContextSnapshotV1,
    record: &GenerationPromptPlanRecord,
    expected_source_branch_id: &ConversationBranchId,
    expected_context_head_message_id: Option<&MessageId>,
    local_user_id: &LocalUserId,
) -> CoreResult<()> {
    if expected.schema_version != 1
        || expected.conversation_id != record.conversation_id
        || expected.source_branch_id != *expected_source_branch_id
        || expected.context_head_message_id.as_ref() != expected_context_head_message_id
        || record.head_message_id.as_ref() != expected_context_head_message_id
    {
        return Err(prompt_context_changed(
            "prompt context boundary changed after generation preparation",
        ));
    }
    let fingerprint = prompt_context_snapshot_sha256(expected).map_err(|error| {
        CoreError::invalid(format!("prompt context snapshot is invalid: {error}"))
    })?;
    if fingerprint != expected.snapshot_sha256
        || expected.local_user_id_sha256 != prompt_local_user_id_sha256(local_user_id)
    {
        return Err(prompt_context_changed(
            "prompt context identity changed after generation preparation",
        ));
    }
    Ok(())
}

#[cfg(test)]
fn require_prompt_context_persona(
    transaction: &Transaction<'_>,
    expected: &PromptContextSnapshotV1,
) -> CoreResult<Option<String>> {
    let current = transaction
        .query_row(
            "SELECT selection.persona_id, selection.persona_revision_id,
                    selection.revision, revision.document_sha256
             FROM conversation_persona_selections AS selection
             JOIN content_objects AS object
               ON object.id = selection.persona_id
              AND object.object_kind = 'persona'
              AND object.deleted_at IS NULL
             JOIN content_revisions AS revision
               ON revision.object_id = selection.persona_id
              AND revision.id = selection.persona_revision_id
              AND revision.object_kind = 'persona'
             WHERE selection.conversation_id = ?1
               AND selection.deleted_at IS NULL",
            [&expected.conversation_id.0],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, i64>(2)?,
                    row.get::<_, String>(3)?,
                ))
            },
        )
        .optional()
        .map_err(storage_db_error)?;
    match (&expected.persona, current) {
        (None, None) => Ok(None),
        (Some(expected), Some((persona_id, revision_id, revision, sha256)))
            if expected.persona_id.as_str() == persona_id
                && expected.persona_revision_id == revision_id
                && expected.selection_revision == u64_revision(revision)?
                && expected.persona_sha256 == sha256 =>
        {
            Ok(Some(persona_id))
        }
        _ => Err(prompt_context_changed(
            "prompt persona selection changed after generation preparation",
        )),
    }
}

#[cfg(test)]
#[derive(Debug)]
struct CurrentPromptBinding {
    evidence: PromptContextBindingEvidence,
    prompt_preset_id: PromptPresetId,
}

#[cfg(test)]
fn require_prompt_context_binding(
    transaction: &Transaction<'_>,
    record: &GenerationPromptPlanRecord,
    expected: &PromptContextSnapshotV1,
    persona_id: Option<&str>,
) -> CoreResult<()> {
    let character_id = transaction
        .query_row(
            "SELECT character_id FROM conversations WHERE id = ?1",
            [&record.conversation_id.0],
            |row| row.get::<_, String>(0),
        )
        .optional()
        .map_err(storage_db_error)?
        .ok_or_else(|| not_found("prompt context conversation"))?;
    let scopes = [
        ("branch", Some(record.branch_id.0.as_str())),
        ("conversation", Some(record.conversation_id.0.as_str())),
        ("character", Some(character_id.as_str())),
        ("persona", persona_id),
        ("user", None),
        ("app", None),
    ];
    let mut current = None;
    for (scope_kind, target_id) in scopes {
        if scope_kind == "persona" && target_id.is_none() {
            continue;
        }
        if let Some(binding) = prompt_context_binding_at_scope(
            transaction,
            scope_kind,
            target_id,
            &record.conversation_id,
        )? {
            current = (binding.prompt_preset_id == record.prompt_preset_id).then_some(binding);
            break;
        }
    }
    if current.as_ref().map(|binding| &binding.evidence) != expected.binding.as_ref() {
        return Err(prompt_context_changed(
            "prompt binding changed after generation preparation",
        ));
    }
    Ok(())
}

#[cfg(test)]
fn prompt_context_binding_at_scope(
    transaction: &Transaction<'_>,
    scope_kind: &str,
    target_id: Option<&str>,
    conversation_id: &ConversationId,
) -> CoreResult<Option<CurrentPromptBinding>> {
    let target_clause = match scope_kind {
        "branch" => "branch_id = ?2",
        "conversation" => "conversation_id = ?2",
        "character" => "character_id = ?2",
        "persona" => "persona_id = ?2",
        "user" | "app" => "1 = 1",
        _ => return Err(CoreError::internal("unsupported prompt binding scope")),
    };
    let sql = format!(
        "SELECT id, revision, prompt_preset_id, document_json
         FROM prompt_preset_bindings
         WHERE scope_kind = ?1 AND {target_clause}
           AND enabled = 1 AND deleted_at IS NULL
         ORDER BY priority DESC, id"
    );
    let mut statement = transaction.prepare(&sql).map_err(storage_db_error)?;
    let rows = if let Some(target_id) = target_id {
        statement
            .query_map(params![scope_kind, target_id], prompt_context_binding_row)
            .map_err(storage_db_error)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(storage_db_error)?
    } else {
        statement
            .query_map([scope_kind], prompt_context_binding_row)
            .map_err(storage_db_error)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(storage_db_error)?
    };
    if rows.len() > 1 {
        return Err(prompt_context_changed(
            "multiple enabled prompt bindings now apply at one scope",
        ));
    }
    rows.into_iter()
        .next()
        .map(|row| decode_current_prompt_binding(row, scope_kind, target_id, conversation_id))
        .transpose()
}

#[cfg(test)]
type CurrentPromptBindingRow = (String, i64, String, String);

#[cfg(test)]
fn prompt_context_binding_row(
    row: &rusqlite::Row<'_>,
) -> rusqlite::Result<CurrentPromptBindingRow> {
    Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?))
}

#[cfg(test)]
fn decode_current_prompt_binding(
    row: CurrentPromptBindingRow,
    scope_kind: &str,
    target_id: Option<&str>,
    conversation_id: &ConversationId,
) -> CoreResult<CurrentPromptBinding> {
    let value: PromptPresetBinding = serde_json::from_str(&row.3)
        .map_err(|error| storage_corrupted(format!("stored prompt binding is invalid: {error}")))?;
    validate_prompt_binding_context_for_test(&value).map_err(|error| {
        storage_corrupted(format!("stored prompt binding context is invalid: {error}"))
    })?;
    let targets = prompt_binding_targets_for_test(&value)?;
    let document_target = match scope_kind {
        "branch" => targets.branch_id,
        "conversation" => targets.conversation_id,
        "character" => targets.character_id,
        "persona" => targets.persona_id,
        _ => None,
    };
    if value.id != row.0
        || value.prompt_preset_id.as_str() != row.2
        || targets.scope_kind != scope_kind
        || document_target != target_id
        || (scope_kind == "branch" && targets.conversation_id != Some(conversation_id.0.as_str()))
    {
        return Err(storage_corrupted(
            "stored prompt binding document differs from its projection",
        ));
    }
    Ok(CurrentPromptBinding {
        evidence: PromptContextBindingEvidence {
            binding_id: row.0,
            binding_revision: u64_revision(row.1)?,
            document_sha256: sha256_hex(row.3.as_bytes()),
        },
        prompt_preset_id: value.prompt_preset_id,
    })
}

#[cfg(test)]
fn require_prompt_context_summaries(
    transaction: &Transaction<'_>,
    expected: &PromptContextSnapshotV1,
) -> CoreResult<()> {
    if expected.summaries.is_empty() {
        if expected.conversation_summary_id.is_some() {
            return Err(CoreError::invalid(
                "prompt context conversation summary is missing its evidence",
            ));
        }
        return Ok(());
    }
    let current = memory_records_at_head_in_connection(
        transaction,
        &expected.conversation_id,
        &expected.source_branch_id,
        expected.context_head_message_id.as_ref(),
        false,
    )?;
    if current.records.len() != current.snapshot.records.len() {
        return Err(storage_corrupted(
            "memory records differ from their exact-head evidence",
        ));
    }
    let visible_summaries = current
        .records
        .into_iter()
        .zip(current.snapshot.records)
        .filter(|(record, _)| {
            record.value.kind == lorepia_domain::MemoryKind::ConversationSummary
                && record.value.invalidated_at.is_none()
                && !record.value.excluded_from_conversation
                && !record.value.excluded_from_character
                && record.deleted_at.is_none()
        })
        .collect::<Vec<_>>();
    for expected_summary in &expected.summaries {
        let unchanged = visible_summaries.iter().any(|(record, evidence)| {
            prompt_summary_evidence_matches(expected_summary, record, evidence)
        });
        if !unchanged {
            return Err(prompt_context_changed(
                "prompt summary changed after generation preparation",
            ));
        }
    }
    if let Some(expected_summary_id) = &expected.conversation_summary_id {
        let Some(context_head) = expected.context_head_message_id.as_ref() else {
            return Err(CoreError::invalid(
                "prompt context summary cannot exist before the first message",
            ));
        };
        let latest = latest_visible_prompt_summary_id(
            transaction,
            expected,
            context_head,
            &visible_summaries,
        )?;
        if latest.as_ref() != Some(expected_summary_id) {
            return Err(prompt_context_changed(
                "latest conversation summary changed after generation preparation",
            ));
        }
    }
    Ok(())
}

#[cfg(test)]
fn prompt_summary_evidence_matches(
    expected: &PromptSummarySourceEvidence,
    record: &StoredRevision<MemoryRecord>,
    evidence: &MemoryRecordAtHeadEvidence,
) -> bool {
    expected.summary_id == record.value.id
        && expected.summary_id == evidence.record_id
        && expected.record_branch_id == record.value.branch_id
        && expected.record_branch_id == evidence.record_branch_id
        && expected.source_start_message_id == record.value.source_start_message_id
        && expected.source_start_message_id == evidence.source_start_message_id
        && expected.source_end_message_id == record.value.source_end_message_id
        && expected.source_end_message_id == evidence.source_end_message_id
        && expected.state_revision == record.revision
        && expected.state_revision == evidence.state_revision
        && record.revision_id.as_deref() == Some(expected.active_revision_id.as_str())
        && expected.active_revision_id == evidence.active_revision_id
        && expected.active_revision_sha256 == evidence.active_revision_sha256
}

#[cfg(test)]
fn latest_visible_prompt_summary_id(
    transaction: &Transaction<'_>,
    expected: &PromptContextSnapshotV1,
    context_head: &MessageId,
    summaries: &[(StoredRevision<MemoryRecord>, MemoryRecordAtHeadEvidence)],
) -> CoreResult<Option<MemoryRecordId>> {
    let mut endpoints = summaries
        .iter()
        .map(|(record, _)| record.value.source_end_message_id.clone())
        .collect::<Vec<_>>();
    endpoints.sort_by(|left, right| left.0.cmp(&right.0));
    endpoints.dedup_by(|left, right| left.0 == right.0);
    let depths = prompt_context_lineage_depths(
        transaction,
        &expected.conversation_id,
        &expected.source_branch_id,
        context_head,
        &endpoints,
    )?;
    summaries
        .iter()
        .map(|(record, _)| {
            depths
                .get(&record.value.source_end_message_id)
                .copied()
                .map(|depth| (depth, record.value.id.clone()))
                .ok_or_else(|| storage_corrupted("visible summary has no lineage depth"))
        })
        .collect::<CoreResult<Vec<_>>>()
        .map(|mut ordered| {
            ordered.sort_by(|left, right| right.0.cmp(&left.0).then_with(|| left.1.cmp(&right.1)));
            ordered.pop().map(|(_, id)| id)
        })
}

#[cfg(test)]
fn prompt_context_lineage_depths(
    transaction: &Transaction<'_>,
    conversation_id: &ConversationId,
    source_branch_id: &ConversationBranchId,
    context_head: &MessageId,
    message_ids: &[MessageId],
) -> CoreResult<HashMap<MessageId, u64>> {
    let requested = message_ids
        .iter()
        .map(|id| id.0.as_str())
        .collect::<Vec<_>>();
    let requested_json = serde_json::to_string(&requested).map_err(|error| {
        CoreError::internal(format!(
            "cannot encode prompt context lineage request: {error}"
        ))
    })?;
    let mut statement = transaction
        .prepare(
            "WITH RECURSIVE source_lineage(id, parent_id, depth) AS (
                 SELECT message.id, message.parent_id, 0
                 FROM conversation_branches AS branch
                 JOIN messages AS message
                   ON message.conversation_id = branch.conversation_id
                  AND message.id = branch.head_message_id
                 WHERE branch.conversation_id = ?1 AND branch.id = ?2
                 UNION ALL
                 SELECT parent.id, parent.parent_id, child.depth + 1
                 FROM messages AS parent
                 JOIN source_lineage AS child ON child.parent_id = parent.id
                 WHERE parent.conversation_id = ?1 AND child.depth < 100000
             ),
             context(id, parent_id) AS (
                 SELECT message.id, message.parent_id
                 FROM messages AS message
                 JOIN source_lineage ON source_lineage.id = message.id
                 WHERE message.conversation_id = ?1 AND message.id = ?3
             ),
             lineage(id, parent_id, depth) AS (
                 SELECT id, parent_id, 0 FROM context
                 UNION ALL
                 SELECT parent.id, parent.parent_id, child.depth + 1
                 FROM messages AS parent
                 JOIN lineage AS child ON child.parent_id = parent.id
                 WHERE parent.conversation_id = ?1 AND child.depth < 100000
             )
             SELECT lineage.id, lineage.depth
             FROM json_each(?4) AS requested
             JOIN lineage ON lineage.id = requested.value",
        )
        .map_err(storage_db_error)?;
    let rows = statement
        .query_map(
            params![
                conversation_id.0,
                source_branch_id.0,
                context_head.0,
                requested_json
            ],
            |row| Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?)),
        )
        .map_err(storage_db_error)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(storage_db_error)?;
    rows.into_iter()
        .map(|(id, depth)| Ok((MessageId(id), u64_revision(depth)?)))
        .collect()
}

fn prompt_context_changed(message: impl Into<String>) -> CoreError {
    CoreError::new(CoreErrorCode::InvalidInput, message, true)
}

fn memory_records_at_head_in_connection(
    connection: &Connection,
    conversation_id: &ConversationId,
    source_branch_id: &ConversationBranchId,
    context_head_message_id: Option<&MessageId>,
    include_invalidated: bool,
) -> CoreResult<MemoryRecordsAtHeadSelection> {
    validate_identifier("memory conversation", &conversation_id.0)?;
    validate_identifier("memory source branch", &source_branch_id.0)?;
    require_memory_context_head_visible(
        connection,
        conversation_id,
        source_branch_id,
        context_head_message_id,
    )?;
    let (records, evidence) = context_head_message_id.map_or_else(
        || Ok((Vec::new(), Vec::new())),
        |context_head| {
            load_memory_records_at_head(
                connection,
                conversation_id,
                context_head,
                include_invalidated,
            )
        },
    )?;
    let mut snapshot = MemoryRecordsAtHeadSnapshot {
        schema_version: 1,
        conversation_id: conversation_id.clone(),
        source_branch_id: source_branch_id.clone(),
        context_head_message_id: context_head_message_id.cloned(),
        include_invalidated,
        records: evidence,
        snapshot_sha256: String::new(),
    };
    snapshot.snapshot_sha256 = memory_records_at_head_snapshot_sha256(&snapshot)?;
    Ok(MemoryRecordsAtHeadSelection { snapshot, records })
}

fn require_memory_context_head_visible(
    connection: &Connection,
    conversation_id: &ConversationId,
    source_branch_id: &ConversationBranchId,
    context_head_message_id: Option<&MessageId>,
) -> CoreResult<()> {
    let branch_head = connection
        .query_row(
            "SELECT head_message_id
             FROM conversation_branches
             WHERE conversation_id = ?1 AND id = ?2",
            params![conversation_id.0, source_branch_id.0],
            |row| row.get::<_, Option<String>>(0),
        )
        .optional()
        .map_err(storage_db_error)?
        .ok_or_else(|| not_found("memory record branch"))?;
    match (branch_head.as_deref(), context_head_message_id) {
        // `None` is the exact pre-first-message boundary. It remains a valid
        // historical fork point after the source branch has advanced and, by
        // definition, has no visible message-backed memory records.
        (_, None) => {}
        (Some(branch_head), Some(context_head)) => {
            let visible = connection
                .query_row(
                    "WITH RECURSIVE lineage(id, parent_id, depth) AS (
                         SELECT id, parent_id, 0
                         FROM messages
                         WHERE conversation_id = ?1 AND id = ?2
                         UNION ALL
                         SELECT parent.id, parent.parent_id, child.depth + 1
                         FROM messages AS parent
                         JOIN lineage AS child ON child.parent_id = parent.id
                         WHERE parent.conversation_id = ?1
                           AND child.depth < 100000
                     )
                     SELECT EXISTS(SELECT 1 FROM lineage WHERE id = ?3)",
                    params![conversation_id.0, branch_head, context_head.0],
                    |row| row.get::<_, bool>(0),
                )
                .map_err(storage_db_error)?;
            if !visible {
                return Err(CoreError::invalid(
                    "memory context head is not on the selected source branch",
                ));
            }
        }
        (None, Some(_)) => {
            return Err(CoreError::invalid(
                "memory context head does not match the source branch boundary",
            ));
        }
    }
    Ok(())
}

struct RawMemoryRecordAtHead {
    record: RawMemoryRecord,
    record_id: String,
    record_branch_id: String,
    source_start_message_id: String,
    source_end_message_id: String,
    active_revision_sha256: String,
}

fn load_memory_records_at_head(
    connection: &Connection,
    conversation_id: &ConversationId,
    context_head: &MessageId,
    include_invalidated: bool,
) -> CoreResult<(
    Vec<StoredRevision<MemoryRecord>>,
    Vec<MemoryRecordAtHeadEvidence>,
)> {
    let mut statement = connection
        .prepare(
            "WITH RECURSIVE lineage(id, parent_id) AS (
                 SELECT id, parent_id
                 FROM messages
                 WHERE conversation_id = ?1 AND id = ?2
                 UNION
                 SELECT parent.id, parent.parent_id
                 FROM messages AS parent
                 JOIN lineage AS child ON child.parent_id = parent.id
                 WHERE parent.conversation_id = ?1
             )
             SELECT revision.document_json, state.state_version,
                    state.active_revision_id, record.created_at,
                    state.updated_at, state.deleted_at, state.pinned,
                    state.invalidated_at,
                    state.excluded_from_conversation_at,
                    state.excluded_from_character_at,
                    record.id, record.branch_id,
                    record.source_start_message_id,
                    record.source_end_message_id,
                    revision.content_sha256
             FROM memory_records AS record
             JOIN lineage AS source_start
               ON source_start.id = record.source_start_message_id
             JOIN lineage AS source_end
               ON source_end.id = record.source_end_message_id
             JOIN memory_record_state AS state ON state.record_id = record.id
             JOIN memory_record_revisions AS revision
               ON revision.id = state.active_revision_id
             WHERE record.conversation_id = ?1
               AND state.deleted_at IS NULL
               AND (?3 OR state.invalidated_at IS NULL)
             ORDER BY record.created_at, record.id",
        )
        .map_err(storage_db_error)?;
    let rows = statement
        .query_map(
            params![conversation_id.0, context_head.0, include_invalidated],
            raw_memory_record_at_head,
        )
        .map_err(storage_db_error)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(storage_db_error)?;
    if rows.len() > MAX_MEMORY_RECORDS {
        return Err(CoreError::invalid(format!(
            "memory head snapshot exceeds {MAX_MEMORY_RECORDS} records"
        )));
    }
    rows.into_iter().map(decode_memory_record_at_head).collect()
}

fn raw_memory_record_at_head(row: &rusqlite::Row<'_>) -> rusqlite::Result<RawMemoryRecordAtHead> {
    Ok(RawMemoryRecordAtHead {
        record: raw_memory_record(row)?,
        record_id: row.get(10)?,
        record_branch_id: row.get(11)?,
        source_start_message_id: row.get(12)?,
        source_end_message_id: row.get(13)?,
        active_revision_sha256: row.get(14)?,
    })
}

fn decode_memory_record_at_head(
    raw: RawMemoryRecordAtHead,
) -> CoreResult<(StoredRevision<MemoryRecord>, MemoryRecordAtHeadEvidence)> {
    let state_revision = u64_revision(raw.record.state_version)?;
    let active_revision_id = raw.record.active_revision_id.clone();
    let evidence = MemoryRecordAtHeadEvidence {
        record_id: MemoryRecordId::from(raw.record_id),
        record_branch_id: ConversationBranchId(raw.record_branch_id),
        source_start_message_id: MessageId(raw.source_start_message_id),
        source_end_message_id: MessageId(raw.source_end_message_id),
        state_revision,
        active_revision_id,
        active_revision_sha256: raw.active_revision_sha256,
    };
    Ok((decode_memory_record(raw.record)?, evidence))
}

fn list_visible_memory_records(
    storage: &Storage,
    conversation_id: &ConversationId,
    branch_id: &ConversationBranchId,
    include_invalidated: bool,
) -> CoreResult<Vec<StoredRevision<MemoryRecord>>> {
    let connection = storage.connection()?;
    let branch_exists = connection
        .query_row(
            "SELECT 1 FROM conversation_branches
             WHERE conversation_id = ?1 AND id = ?2",
            params![conversation_id.0, branch_id.0],
            |_| Ok(()),
        )
        .optional()
        .map_err(storage_db_error)?
        .is_some();
    if !branch_exists {
        return Err(not_found("memory record branch"));
    }
    let mut statement = connection
        .prepare(
            "WITH RECURSIVE lineage(id, parent_id) AS (
                 SELECT message.id, message.parent_id
                 FROM conversation_branches AS branch
                 JOIN messages AS message
                   ON message.conversation_id = branch.conversation_id
                  AND message.id = branch.head_message_id
                 WHERE branch.conversation_id = ?1 AND branch.id = ?2
                 UNION
                 SELECT parent.id, parent.parent_id
                 FROM messages AS parent
                 JOIN lineage AS child ON child.parent_id = parent.id
                 WHERE parent.conversation_id = ?1
             )
             SELECT revision.document_json, state.state_version,
                    state.active_revision_id, record.created_at,
                    state.updated_at, state.deleted_at, state.pinned,
                    state.invalidated_at,
                    state.excluded_from_conversation_at,
                    state.excluded_from_character_at
             FROM memory_records AS record
             JOIN lineage AS source_start
               ON source_start.id = record.source_start_message_id
             JOIN lineage AS source_end
               ON source_end.id = record.source_end_message_id
             JOIN memory_record_state AS state ON state.record_id = record.id
             JOIN memory_record_revisions AS revision
               ON revision.id = state.active_revision_id
             WHERE record.conversation_id = ?1
               AND state.deleted_at IS NULL
               AND (?3 OR state.invalidated_at IS NULL)
             ORDER BY record.created_at, record.id",
        )
        .map_err(storage_db_error)?;
    let rows = statement
        .query_map(
            params![conversation_id.0, branch_id.0, include_invalidated],
            raw_memory_record,
        )
        .map_err(storage_db_error)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(storage_db_error)?;
    rows.into_iter().map(decode_memory_record).collect()
}

fn invalidate_memory_range(
    storage: &Storage,
    conversation_id: &ConversationId,
    branch_id: &ConversationBranchId,
    start_message_id: &MessageId,
    end_message_id: &MessageId,
    invalidated_at: DateTime<Utc>,
) -> CoreResult<MemoryInvalidationResult> {
    let mut connection = storage.connection()?;
    let transaction = connection
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(storage_db_error)?;
    let result = invalidate_memory_range_in_transaction(
        &transaction,
        conversation_id,
        branch_id,
        start_message_id,
        end_message_id,
        invalidated_at,
    )?;
    transaction.commit().map_err(storage_db_error)?;
    Ok(result)
}

/// Invalidates memory derived from a lineage range inside an existing write
/// transaction. Callers must invoke this before moving the branch head because
/// the exact removed lineage is resolved from the current head.
pub(crate) fn invalidate_memory_range_in_transaction(
    transaction: &Transaction<'_>,
    conversation_id: &ConversationId,
    branch_id: &ConversationBranchId,
    start_message_id: &MessageId,
    end_message_id: &MessageId,
    invalidated_at: DateTime<Utc>,
) -> CoreResult<MemoryInvalidationResult> {
    let (start_depth, end_depth) = memory_invalidation_depths(
        transaction,
        conversation_id,
        branch_id,
        start_message_id,
        end_message_id,
    )?;
    let records = memory_records_in_invalidation_range(
        transaction,
        conversation_id,
        branch_id,
        start_depth,
        end_depth,
    )?;
    invalidate_memory_records(
        transaction,
        &records,
        start_message_id,
        end_message_id,
        invalidated_at,
    )?;
    let invalidated_jobs = cancel_memory_jobs_in_range(
        transaction,
        conversation_id,
        branch_id,
        start_depth,
        end_depth,
        invalidated_at,
    )?;
    Ok(MemoryInvalidationResult {
        invalidated_records: u64::try_from(records.len())
            .map_err(|_| CoreError::internal("memory invalidation count overflow"))?,
        invalidated_jobs: u64::try_from(invalidated_jobs)
            .map_err(|_| CoreError::internal("memory job invalidation count overflow"))?,
    })
}

fn memory_invalidation_depths(
    transaction: &Transaction<'_>,
    conversation_id: &ConversationId,
    branch_id: &ConversationBranchId,
    start_message_id: &MessageId,
    end_message_id: &MessageId,
) -> CoreResult<(i64, i64)> {
    let bounds = transaction
        .query_row(
            "WITH RECURSIVE lineage(id, parent_id, depth) AS (
                 SELECT message.id, message.parent_id, 0
                 FROM conversation_branches AS branch
                 JOIN messages AS message
                   ON message.conversation_id = branch.conversation_id
                  AND message.id = branch.head_message_id
                 WHERE branch.conversation_id = ?1 AND branch.id = ?2
                 UNION ALL
                 SELECT parent.id, parent.parent_id, child.depth + 1
                 FROM messages AS parent
                 JOIN lineage AS child ON child.parent_id = parent.id
                 WHERE parent.conversation_id = ?1 AND child.depth < 100000
             )
             SELECT MAX(CASE WHEN id = ?3 THEN depth END),
                    MAX(CASE WHEN id = ?4 THEN depth END)
             FROM lineage",
            params![
                conversation_id.0,
                branch_id.0,
                start_message_id.0,
                end_message_id.0
            ],
            |row| Ok((row.get::<_, Option<i64>>(0)?, row.get::<_, Option<i64>>(1)?)),
        )
        .map_err(storage_db_error)?;
    let (Some(start_depth), Some(end_depth)) = bounds else {
        return Err(CoreError::invalid(
            "memory invalidation range is not on the selected branch",
        ));
    };
    if start_depth < end_depth {
        Err(CoreError::invalid(
            "memory invalidation start must not follow its end",
        ))
    } else {
        Ok((start_depth, end_depth))
    }
}

fn memory_records_in_invalidation_range(
    transaction: &Transaction<'_>,
    conversation_id: &ConversationId,
    branch_id: &ConversationBranchId,
    start_depth: i64,
    end_depth: i64,
) -> CoreResult<Vec<(String, i64, String)>> {
    let mut statement = transaction
        .prepare(
            "WITH RECURSIVE lineage(id, parent_id, depth) AS (
                 SELECT message.id, message.parent_id, 0
                 FROM conversation_branches AS branch
                 JOIN messages AS message
                   ON message.conversation_id = branch.conversation_id
                  AND message.id = branch.head_message_id
                 WHERE branch.conversation_id = ?1 AND branch.id = ?2
                 UNION ALL
                 SELECT parent.id, parent.parent_id, child.depth + 1
                 FROM messages AS parent
                 JOIN lineage AS child ON child.parent_id = parent.id
                 WHERE parent.conversation_id = ?1 AND child.depth < 100000
             )
             SELECT record.id, state.state_version, state.active_revision_id
             FROM memory_records AS record
             JOIN memory_record_state AS state ON state.record_id = record.id
             JOIN lineage AS source_start ON source_start.id = record.source_start_message_id
             JOIN lineage AS source_end ON source_end.id = record.source_end_message_id
             WHERE record.conversation_id = ?1 AND record.branch_id = ?2
               AND state.deleted_at IS NULL AND state.invalidated_at IS NULL
               AND source_start.depth >= ?3 AND ?4 >= source_end.depth
             ORDER BY record.id",
        )
        .map_err(storage_db_error)?;
    statement
        .query_map(
            params![conversation_id.0, branch_id.0, end_depth, start_depth],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, String>(2)?,
                ))
            },
        )
        .map_err(storage_db_error)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(storage_db_error)
}

fn invalidate_memory_records(
    transaction: &Transaction<'_>,
    records: &[(String, i64, String)],
    start_message_id: &MessageId,
    end_message_id: &MessageId,
    invalidated_at: DateTime<Utc>,
) -> CoreResult<()> {
    for (record_id, state_version, active_revision_id) in records {
        let next = state_version
            .checked_add(1)
            .ok_or_else(|| CoreError::internal("memory state revision overflow"))?;
        transaction
            .execute(
                "UPDATE memory_record_state
                 SET invalidated_at = ?2, invalidation_reason = 'source_range_changed',
                     state_version = ?3, updated_at = ?2
                 WHERE record_id = ?1 AND state_version = ?4
                   AND invalidated_at IS NULL AND deleted_at IS NULL",
                params![record_id, invalidated_at.to_rfc3339(), next, state_version],
            )
            .map_err(storage_db_error)?;
        append_memory_event(
            transaction,
            record_id,
            "invalidated",
            Some(active_revision_id),
            Some(active_revision_id),
            serde_json::json!({
                "start_message_id": start_message_id.0,
                "end_message_id": end_message_id.0,
            }),
            invalidated_at,
        )?;
    }
    Ok(())
}

fn cancel_memory_jobs_in_range(
    transaction: &Transaction<'_>,
    conversation_id: &ConversationId,
    branch_id: &ConversationBranchId,
    start_depth: i64,
    end_depth: i64,
    invalidated_at: DateTime<Utc>,
) -> CoreResult<usize> {
    transaction
        .execute(
            "WITH RECURSIVE lineage(id, parent_id, depth) AS (
                 SELECT message.id, message.parent_id, 0
                 FROM conversation_branches AS branch
                 JOIN messages AS message
                   ON message.conversation_id = branch.conversation_id
                  AND message.id = branch.head_message_id
                 WHERE branch.conversation_id = ?1 AND branch.id = ?2
                 UNION ALL
                 SELECT parent.id, parent.parent_id, child.depth + 1
                 FROM messages AS parent
                 JOIN lineage AS child ON child.parent_id = parent.id
                 WHERE parent.conversation_id = ?1 AND child.depth < 100000
             ), affected AS (
                 SELECT job.id FROM memory_jobs AS job
                 JOIN lineage AS source_start ON source_start.id = job.source_start_message_id
                 JOIN lineage AS source_end ON source_end.id = job.source_end_message_id
                 WHERE job.conversation_id = ?1 AND job.branch_id = ?2
                   AND job.state IN ('queued', 'running', 'interrupted')
                   AND source_start.depth >= ?3 AND ?4 >= source_end.depth
             )
             UPDATE memory_jobs
             SET state = 'cancelled', revision = revision + 1,
                 finished_at = ?5, failure_json = NULL, updated_at = ?5
             WHERE id IN (SELECT id FROM affected)",
            params![
                conversation_id.0,
                branch_id.0,
                end_depth,
                start_depth,
                invalidated_at.to_rfc3339()
            ],
        )
        .map_err(storage_db_error)
}

fn memory_job_fingerprint(job: &MemoryJob) -> CoreResult<String> {
    let value = serde_json::json!({
        "schema_version": 1,
        "idempotency_key": job.idempotency_key,
        "kind": job.kind,
        "conversation_id": job.conversation_id,
        "branch_id": job.branch_id,
        "source_start_message_id": job.source_start_message_id,
        "source_end_message_id": job.source_end_message_id,
    });
    serde_json::to_vec(&value)
        .map(|bytes| sha256_hex(&bytes))
        .map_err(|error| CoreError::invalid(format!("cannot fingerprint memory job: {error}")))
}

fn memory_job_state(status: MemoryJobStatus) -> &'static str {
    match status {
        MemoryJobStatus::Queued => "queued",
        MemoryJobStatus::Running => "running",
        MemoryJobStatus::Interrupted => "interrupted",
        MemoryJobStatus::Succeeded => "succeeded",
        MemoryJobStatus::Failed => "failed",
        MemoryJobStatus::Cancelled => "cancelled",
    }
}

struct MemoryJobSaveContext {
    revision: u64,
    created_at: DateTime<Utc>,
    input_fingerprint: String,
    payload_json: String,
    started_at: Option<String>,
    finished_at: Option<String>,
    failure_json: Option<String>,
}

fn save_memory_job(
    storage: &Storage,
    job: &MemoryJob,
    expected_revision: Option<u64>,
) -> CoreResult<StoredRevision<MemoryJob>> {
    validate_memory_job_for_save(job)?;
    let payload_json = encode_document("memory job", job)?.0;
    let input_fingerprint = memory_job_fingerprint(job)?;
    let mut connection = storage.connection()?;
    let transaction = connection
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(storage_db_error)?;
    let context = prepare_memory_job_save(
        &transaction,
        job,
        expected_revision,
        payload_json,
        input_fingerprint,
    )?;
    write_memory_job_row(&transaction, job, expected_revision, &context)?;
    transaction.commit().map_err(storage_db_error)?;
    let mut value = job.clone();
    value.created_at = context.created_at;
    Ok(StoredRevision {
        value,
        revision: context.revision,
        revision_id: None,
        created_at: context.created_at,
        updated_at: job.updated_at,
        deleted_at: None,
    })
}

fn validate_memory_job_for_save(job: &MemoryJob) -> CoreResult<()> {
    validate_identifier("memory job", job.id.as_str())?;
    validate_identifier("memory job idempotency key", &job.idempotency_key)?;
    if job.updated_at < job.created_at {
        return Err(CoreError::invalid(
            "memory job update time predates creation",
        ));
    }
    if job.status == MemoryJobStatus::Failed && job.error_code.is_none() {
        return Err(CoreError::invalid(
            "failed memory job requires an error code",
        ));
    }
    if job.status != MemoryJobStatus::Failed && job.error_code.is_some() {
        return Err(CoreError::invalid(
            "only a failed memory job may carry an error code",
        ));
    }
    Ok(())
}

fn prepare_memory_job_save(
    transaction: &Transaction<'_>,
    job: &MemoryJob,
    expected_revision: Option<u64>,
    payload_json: String,
    input_fingerprint: String,
) -> CoreResult<MemoryJobSaveContext> {
    let current = transaction
        .query_row(
            "SELECT revision, created_at, state FROM memory_jobs WHERE id = ?1",
            [job.id.as_str()],
            |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                ))
            },
        )
        .optional()
        .map_err(storage_db_error)?;
    let (revision, created_at) = match (expected_revision, current) {
        (None, None) if job.status == MemoryJobStatus::Queued => (1_u64, job.created_at),
        (None, None) => {
            return Err(CoreError::invalid("memory job must begin queued"));
        }
        (None, Some((actual, _, _))) => {
            return Err(revision_conflict(
                "memory job",
                job.id.as_str(),
                None,
                Some(u64_revision(actual)?),
            ));
        }
        (Some(expected), Some((actual, created_at, _))) if u64_revision(actual)? == expected => (
            expected
                .checked_add(1)
                .ok_or_else(|| CoreError::internal("memory job revision overflow"))?,
            parse_datetime("memory job created_at", &created_at)?,
        ),
        (Some(expected), Some((actual, _, _))) => {
            return Err(revision_conflict(
                "memory job",
                job.id.as_str(),
                Some(expected),
                Some(u64_revision(actual)?),
            ));
        }
        (Some(expected), None) => {
            return Err(revision_conflict(
                "memory job",
                job.id.as_str(),
                Some(expected),
                None,
            ));
        }
    };
    let started_at = match job.status {
        MemoryJobStatus::Running | MemoryJobStatus::Interrupted => {
            Some(job.updated_at.to_rfc3339())
        }
        MemoryJobStatus::Succeeded | MemoryJobStatus::Failed | MemoryJobStatus::Cancelled => {
            Some(job.created_at.to_rfc3339())
        }
        MemoryJobStatus::Queued => None,
    };
    let finished_at = matches!(
        job.status,
        MemoryJobStatus::Succeeded | MemoryJobStatus::Failed | MemoryJobStatus::Cancelled
    )
    .then(|| job.updated_at.to_rfc3339());
    let failure_json = job
        .error_code
        .as_ref()
        .map(|code| serde_json::to_string(&serde_json::json!({"error_code": code})))
        .transpose()
        .map_err(|error| {
            CoreError::invalid(format!("cannot encode memory job failure: {error}"))
        })?;
    Ok(MemoryJobSaveContext {
        revision,
        created_at,
        input_fingerprint,
        payload_json,
        started_at,
        finished_at,
        failure_json,
    })
}

fn write_memory_job_row(
    transaction: &Transaction<'_>,
    job: &MemoryJob,
    expected_revision: Option<u64>,
    context: &MemoryJobSaveContext,
) -> CoreResult<()> {
    let changed = transaction
        .execute(
            "INSERT INTO memory_jobs
             (id, idempotency_key, job_kind, memory_profile_revision_id,
              task_profile_revision_id, conversation_id, branch_id,
              source_start_message_id, source_end_message_id,
              input_fingerprint_sha256, state, revision, attempts, available_at,
              started_at, finished_at, result_record_id, failure_json,
              payload_json, created_at, updated_at)
             VALUES (
                 ?1, ?2, ?3, NULL, NULL, ?4, ?5, ?6, ?7, ?8, ?9, ?10,
                 ?11, ?12, ?13, ?14, NULL, ?15, ?16, ?17, ?18
             )
             ON CONFLICT(id) DO UPDATE SET
                 state = excluded.state,
                 revision = excluded.revision,
                 attempts = excluded.attempts,
                 started_at = excluded.started_at,
                 finished_at = excluded.finished_at,
                 failure_json = excluded.failure_json,
                 payload_json = excluded.payload_json,
                 updated_at = excluded.updated_at
             WHERE memory_jobs.revision = ?19",
            params![
                job.id.as_str(),
                job.idempotency_key,
                enum_wire(&job.kind)?,
                job.conversation_id.0,
                job.branch_id.0,
                job.source_start_message_id.0,
                job.source_end_message_id.0,
                context.input_fingerprint.as_str(),
                memory_job_state(job.status),
                i64_revision(context.revision)?,
                job.attempt,
                context.created_at.to_rfc3339(),
                context.started_at.as_deref(),
                context.finished_at.as_deref(),
                context.failure_json.as_deref(),
                context.payload_json.as_str(),
                context.created_at.to_rfc3339(),
                job.updated_at.to_rfc3339(),
                expected_revision
                    .map(i64_revision)
                    .transpose()?
                    .unwrap_or(0),
            ],
        )
        .map_err(storage_db_error)?;
    if changed != 1 {
        return Err(revision_conflict(
            "memory job",
            job.id.as_str(),
            expected_revision,
            None,
        ));
    }
    Ok(())
}

fn get_memory_job(storage: &Storage, id: &MemoryJobId) -> CoreResult<StoredRevision<MemoryJob>> {
    let entry = storage.get_memory_job_queue_entry(id)?;
    let value = entry.job;
    Ok(StoredRevision {
        created_at: value.created_at,
        updated_at: value.updated_at,
        value,
        revision: entry.revision,
        revision_id: None,
        deleted_at: None,
    })
}

fn save_memory_embedding(storage: &Storage, embedding: &MemoryEmbeddingRecord) -> CoreResult<()> {
    validate_identifier("memory embedding", &embedding.id)?;
    if embedding.values.is_empty()
        || embedding.values.len() > MAX_MEMORY_EMBEDDING_DIMENSIONS
        || embedding.values.len()
            != usize::try_from(embedding.dimensions)
                .map_err(|_| CoreError::invalid("memory embedding dimensions are invalid"))?
        || embedding.values.iter().any(|value| !value.is_finite())
    {
        return Err(CoreError::invalid(
            "memory embedding dimensions or values are invalid",
        ));
    }
    let model_route_id = embedding
        .model_route_id
        .as_ref()
        .ok_or_else(|| CoreError::invalid("memory embedding requires a model route"))?;
    let connection = storage.connection()?;
    let record_revision_id = connection
        .query_row(
            "SELECT active_revision_id FROM memory_record_state
             WHERE record_id = ?1 AND deleted_at IS NULL",
            [embedding.memory_record_id.as_str()],
            |row| row.get::<_, String>(0),
        )
        .optional()
        .map_err(storage_db_error)?
        .ok_or_else(|| not_found("memory embedding record"))?;
    let mut bytes = Vec::with_capacity(embedding.values.len().saturating_mul(4));
    for value in &embedding.values {
        bytes.extend_from_slice(&value.to_le_bytes());
    }
    let vector_sha256 = sha256_hex(&bytes);
    connection
        .execute(
            "INSERT INTO memory_embeddings
             (id, record_revision_id, task_profile_revision_id, model_route_id,
              dimensions, encoding, vector_blob, vector_sha256, created_at)
             VALUES (?1, ?2, NULL, ?3, ?4, 'f32le', ?5, ?6, ?7)",
            params![
                embedding.id,
                record_revision_id,
                model_route_id.as_str(),
                embedding.dimensions,
                bytes,
                vector_sha256,
                embedding.created_at.to_rfc3339(),
            ],
        )
        .map_err(storage_db_error)?;
    Ok(())
}

fn get_memory_embedding(storage: &Storage, id: &str) -> CoreResult<MemoryEmbeddingRecord> {
    let row = storage
        .connection()?
        .query_row(
            "SELECT embedding.id, revision.record_id, embedding.model_route_id,
                    embedding.dimensions, embedding.vector_blob,
                    embedding.created_at
             FROM memory_embeddings AS embedding
             JOIN memory_record_revisions AS revision
               ON revision.id = embedding.record_revision_id
             WHERE embedding.id = ?1 AND embedding.encoding = 'f32le'",
            [id],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, i64>(3)?,
                    row.get::<_, Vec<u8>>(4)?,
                    row.get::<_, String>(5)?,
                ))
            },
        )
        .optional()
        .map_err(storage_db_error)?
        .ok_or_else(|| not_found("memory embedding"))?;
    let dimensions = nonnegative_u32("memory embedding dimensions", row.3)?;
    let expected_bytes = usize::try_from(dimensions)
        .map_err(|_| storage_corrupted("stored memory embedding dimensions are invalid"))?
        .checked_mul(4)
        .ok_or_else(|| storage_corrupted("stored memory embedding size overflow"))?;
    if row.4.len() != expected_bytes {
        return Err(storage_corrupted(
            "stored memory embedding byte length is invalid",
        ));
    }
    let values = row
        .4
        .chunks_exact(4)
        .map(|chunk| f32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]))
        .collect::<Vec<_>>();
    if values.iter().any(|value| !value.is_finite()) {
        return Err(storage_corrupted(
            "stored memory embedding contains a non-finite value",
        ));
    }
    Ok(MemoryEmbeddingRecord {
        id: row.0,
        memory_record_id: MemoryRecordId::from(row.1),
        model_route_id: Some(ModelRouteId::from(row.2)),
        dimensions,
        values,
        created_at: parse_datetime("memory embedding created_at", &row.5)?,
    })
}

struct ModuleBindingTargets<'a> {
    scope_kind: &'static str,
    persona_id: Option<&'a str>,
    character_id: Option<&'a str>,
    conversation_id: Option<&'a str>,
    branch_id: Option<&'a str>,
}

fn module_binding_targets(binding: &ModuleBinding) -> CoreResult<ModuleBindingTargets<'_>> {
    let target = binding.target_id.as_deref();
    match binding.scope {
        ModuleScope::App if target.is_none() && binding.conversation_id.is_none() => {
            Ok(ModuleBindingTargets {
                scope_kind: "app",
                persona_id: None,
                character_id: None,
                conversation_id: None,
                branch_id: None,
            })
        }
        ModuleScope::User if target.is_none() && binding.conversation_id.is_none() => {
            Ok(ModuleBindingTargets {
                scope_kind: "user",
                persona_id: None,
                character_id: None,
                conversation_id: None,
                branch_id: None,
            })
        }
        ModuleScope::Persona if target.is_some() && binding.conversation_id.is_none() => {
            Ok(ModuleBindingTargets {
                scope_kind: "persona",
                persona_id: target,
                character_id: None,
                conversation_id: None,
                branch_id: None,
            })
        }
        ModuleScope::Character if target.is_some() && binding.conversation_id.is_none() => {
            Ok(ModuleBindingTargets {
                scope_kind: "character",
                persona_id: None,
                character_id: target,
                conversation_id: None,
                branch_id: None,
            })
        }
        ModuleScope::Conversation if target.is_some() && binding.conversation_id.is_none() => {
            Ok(ModuleBindingTargets {
                scope_kind: "conversation",
                persona_id: None,
                character_id: None,
                conversation_id: target,
                branch_id: None,
            })
        }
        ModuleScope::Branch if target.is_some() && binding.conversation_id.is_some() => {
            Ok(ModuleBindingTargets {
                scope_kind: "branch",
                persona_id: None,
                character_id: None,
                conversation_id: binding.conversation_id.as_ref().map(|id| id.0.as_str()),
                branch_id: target,
            })
        }
        _ => Err(CoreError::invalid(
            "module binding scope and target are inconsistent",
        )),
    }
}

fn validate_module_binding_revision(
    transaction: &Transaction<'_>,
    binding: &ModuleBinding,
) -> CoreResult<()> {
    let exists = transaction
        .query_row(
            "SELECT 1 FROM content_module_revisions
             WHERE module_id = ?1 AND revision_id = ?2",
            params![binding.module_id.as_str(), binding.revision_id.as_str()],
            |_| Ok(()),
        )
        .optional()
        .map_err(storage_db_error)?
        .is_some();
    if !exists {
        return Err(not_found("module binding revision"));
    }
    match binding.resolution_mode {
        lorepia_domain::ModuleRevisionResolutionMode::Pinned => {
            if binding.pinned_revision_id.as_ref() != Some(&binding.revision_id) {
                return Err(CoreError::invalid(
                    "pinned module binding revision is inconsistent",
                ));
            }
        }
        lorepia_domain::ModuleRevisionResolutionMode::Active => {
            let active = transaction
                .query_row(
                    "SELECT active_revision_id
                     FROM content_object_state WHERE object_id = ?1",
                    [binding.module_id.as_str()],
                    |row| row.get::<_, String>(0),
                )
                .map_err(storage_db_error)?;
            if active != binding.revision_id.as_str() {
                return Err(CoreError::invalid(
                    "active module binding does not resolve to the current revision",
                ));
            }
        }
    }
    Ok(())
}

fn save_module_binding(
    storage: &Storage,
    binding: &ModuleBinding,
    expected_revision: Option<u64>,
) -> CoreResult<StoredRevision<ModuleBinding>> {
    let mut connection = storage.connection()?;
    let transaction = connection
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(storage_db_error)?;
    let stored = write_module_binding_transaction(&transaction, binding, expected_revision)?;
    transaction.commit().map_err(storage_db_error)?;
    Ok(stored)
}

type RawModuleBindingState = (i64, String, Option<String>);

fn read_module_binding_state(
    transaction: &Transaction<'_>,
    binding_id: &ModuleBindingId,
) -> CoreResult<Option<RawModuleBindingState>> {
    transaction
        .query_row(
            "SELECT revision, created_at, deleted_at
             FROM content_module_bindings WHERE id = ?1",
            [binding_id.as_str()],
            |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, Option<String>>(2)?,
                ))
            },
        )
        .optional()
        .map_err(storage_db_error)
}

fn resolve_module_binding_write_revision(
    binding: &ModuleBinding,
    expected_revision: Option<u64>,
    current: Option<RawModuleBindingState>,
) -> CoreResult<(u64, DateTime<Utc>)> {
    match (expected_revision, current) {
        (None, None) => Ok((1, binding.created_at)),
        (None, Some((actual, _, _))) => Err(revision_conflict(
            "module binding",
            binding.id.as_str(),
            None,
            Some(u64_revision(actual)?),
        )),
        (Some(expected), Some((actual, created_at, deleted_at))) => {
            let actual = u64_revision(actual)?;
            if actual != expected || deleted_at.is_some() {
                return Err(revision_conflict(
                    "module binding",
                    binding.id.as_str(),
                    Some(expected),
                    Some(actual),
                ));
            }
            Ok((
                expected
                    .checked_add(1)
                    .ok_or_else(|| CoreError::internal("module binding revision overflow"))?,
                parse_datetime("module binding created_at", &created_at)?,
            ))
        }
        (Some(expected), None) => Err(revision_conflict(
            "module binding",
            binding.id.as_str(),
            Some(expected),
            None,
        )),
    }
}

struct ModuleBindingWriteMetadata<'a> {
    next_revision: u64,
    expected_revision: Option<u64>,
    variable_overrides_json: &'a str,
    document_json: &'a str,
    created_at: &'a DateTime<Utc>,
    now: &'a DateTime<Utc>,
}

fn execute_module_binding_upsert(
    transaction: &Transaction<'_>,
    value: &ModuleBinding,
    targets: &ModuleBindingTargets<'_>,
    metadata: &ModuleBindingWriteMetadata<'_>,
) -> CoreResult<()> {
    let changed = transaction
        .execute(
            "INSERT INTO content_module_bindings
             (id, revision, module_id, resolution_mode, pinned_revision_id,
              scope_kind, persona_id, character_id, conversation_id, branch_id,
              priority, enabled, approved, activation_approval_id,
              activation_review_sha256, activation_plan_sha256,
              package_import_approval_id, variable_overrides_json,
              document_json, created_at, updated_at, deleted_at)
             VALUES (
                 ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12,
                 ?13, ?14, ?15, ?16, ?17, ?18, ?19, ?20, ?21, NULL
             )
             ON CONFLICT(id) DO UPDATE SET
                 revision = excluded.revision,
                 module_id = excluded.module_id,
                 resolution_mode = excluded.resolution_mode,
                 pinned_revision_id = excluded.pinned_revision_id,
                 scope_kind = excluded.scope_kind,
                 persona_id = excluded.persona_id,
                 character_id = excluded.character_id,
                 conversation_id = excluded.conversation_id,
                 branch_id = excluded.branch_id,
                 priority = excluded.priority,
                 enabled = excluded.enabled,
                 approved = excluded.approved,
                 activation_approval_id = excluded.activation_approval_id,
                 activation_review_sha256 = excluded.activation_review_sha256,
                 activation_plan_sha256 = excluded.activation_plan_sha256,
                 package_import_approval_id =
                     excluded.package_import_approval_id,
                 variable_overrides_json = excluded.variable_overrides_json,
                 document_json = excluded.document_json,
                 updated_at = excluded.updated_at
             WHERE content_module_bindings.revision = ?22
               AND content_module_bindings.deleted_at IS NULL",
            params![
                value.id.as_str(),
                i64_revision(metadata.next_revision)?,
                value.module_id.as_str(),
                enum_wire(&value.resolution_mode)?,
                value
                    .pinned_revision_id
                    .as_ref()
                    .map(ModuleRevisionId::as_str),
                targets.scope_kind,
                targets.persona_id,
                targets.character_id,
                targets.conversation_id,
                targets.branch_id,
                value.priority,
                value.enabled,
                value.approved,
                value.activation_approval_id,
                value
                    .activation_review_sha256
                    .as_ref()
                    .map(lorepia_domain::Sha256Digest::as_str),
                value
                    .activation_plan_sha256
                    .as_ref()
                    .map(lorepia_domain::Sha256Digest::as_str),
                value.package_import_approval_id,
                metadata.variable_overrides_json,
                metadata.document_json,
                metadata.created_at.to_rfc3339(),
                metadata.now.to_rfc3339(),
                metadata
                    .expected_revision
                    .map(i64_revision)
                    .transpose()?
                    .unwrap_or(0),
            ],
        )
        .map_err(storage_db_error)?;
    if changed != 1 {
        return Err(revision_conflict(
            "module binding",
            value.id.as_str(),
            metadata.expected_revision,
            None,
        ));
    }
    Ok(())
}

fn write_module_binding_transaction(
    transaction: &Transaction<'_>,
    binding: &ModuleBinding,
    expected_revision: Option<u64>,
) -> CoreResult<StoredRevision<ModuleBinding>> {
    binding
        .validate()
        .map_err(|error| CoreError::invalid(error.to_string()))?;
    if binding.enabled && !binding.approved {
        return Err(CoreError::invalid(
            "enabled module binding requires explicit approval",
        ));
    }
    let targets = module_binding_targets(binding)?;
    validate_module_binding_revision(transaction, binding)?;
    let current = read_module_binding_state(transaction, &binding.id)?;
    let now = Utc::now();
    let (next_revision, created_at) =
        resolve_module_binding_write_revision(binding, expected_revision, current)?;
    let mut value = binding.clone();
    value.created_at = created_at;
    let (document_json, _) = encode_document("module binding", &value)?;
    let variable_overrides_json =
        serde_json::to_string(&value.variable_overrides).map_err(|error| {
            CoreError::invalid(format!("cannot encode module binding variables: {error}"))
        })?;
    let metadata = ModuleBindingWriteMetadata {
        next_revision,
        expected_revision,
        variable_overrides_json: &variable_overrides_json,
        document_json: &document_json,
        created_at: &created_at,
        now: &now,
    };
    execute_module_binding_upsert(transaction, &value, &targets, &metadata)?;
    Ok(StoredRevision {
        value,
        revision: next_revision,
        revision_id: None,
        created_at,
        updated_at: now,
        deleted_at: None,
    })
}

fn module_binding_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<RawStoredDocument> {
    Ok((
        row.get::<_, String>(0)?,
        row.get::<_, i64>(1)?,
        None,
        row.get::<_, String>(2)?,
        row.get::<_, String>(3)?,
        row.get::<_, Option<String>>(4)?,
    ))
}

fn list_module_bindings(
    storage: &Storage,
    module_id: &ContentModuleId,
) -> CoreResult<Vec<StoredRevision<ModuleBinding>>> {
    let connection = storage.connection()?;
    let mut statement = connection
        .prepare(
            "SELECT document_json, revision, created_at, updated_at, deleted_at
             FROM content_module_bindings
             WHERE module_id = ?1 AND deleted_at IS NULL
             ORDER BY priority DESC, id",
        )
        .map_err(storage_db_error)?;
    let rows = statement
        .query_map([module_id.as_str()], module_binding_row)
        .map_err(storage_db_error)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(storage_db_error)?;
    rows.into_iter()
        .map(|row| decode_stored_document("module binding", row))
        .collect()
}

fn list_all_module_bindings_transaction(
    transaction: &Transaction<'_>,
) -> CoreResult<Vec<StoredRevision<ModuleBinding>>> {
    let mut statement = transaction
        .prepare(
            "SELECT document_json, revision, created_at, updated_at, deleted_at
             FROM content_module_bindings
             WHERE deleted_at IS NULL
             ORDER BY scope_kind, priority DESC, id",
        )
        .map_err(storage_db_error)?;
    let rows = statement
        .query_map([], module_binding_row)
        .map_err(storage_db_error)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(storage_db_error)?;
    rows.into_iter()
        .map(|row| decode_stored_document("module binding", row))
        .collect()
}

fn resolve_module_binding_revision(
    transaction: &Transaction<'_>,
    binding: &ModuleBinding,
) -> CoreResult<ModuleBinding> {
    let mut resolved = binding.clone();
    resolved.revision_id = match binding.resolution_mode {
        lorepia_domain::ModuleRevisionResolutionMode::Active => ModuleRevisionId::from(
            transaction
                .query_row(
                    "SELECT state.active_revision_id
                         FROM content_objects AS object
                         JOIN content_object_state AS state
                           ON state.object_id = object.id
                         WHERE object.id = ?1
                           AND object.object_kind = 'content_module'
                           AND object.deleted_at IS NULL",
                    [binding.module_id.as_str()],
                    |row| row.get::<_, String>(0),
                )
                .optional()
                .map_err(storage_db_error)?
                .ok_or_else(|| not_found("active module revision"))?,
        ),
        lorepia_domain::ModuleRevisionResolutionMode::Pinned => binding
            .pinned_revision_id
            .clone()
            .ok_or_else(|| CoreError::invalid("pinned module binding requires a revision"))?,
    };
    validate_module_binding_revision(transaction, &resolved)?;
    Ok(resolved)
}

fn module_activation_resolution_set(
    review: &lorepia_orchestration::ModuleActivationReview,
    plan: &lorepia_orchestration::ModuleActivationPlan,
) -> CoreResult<lorepia_orchestration::ModuleMergeResolutionSet> {
    let mut resolutions = Vec::with_capacity(review.conflicts.len());
    for conflict in &review.conflicts {
        let selected = plan
            .components
            .iter()
            .find(|component| component.component == conflict.component)
            .map(|component| lorepia_domain::ModuleConflictCandidate {
                module_id: component.selected_source.module_id.clone(),
                revision_id: component.selected_source.revision_id.clone(),
                component_hash: component.sha256.clone(),
            });
        if selected.is_none()
            && !plan
                .omitted_components
                .iter()
                .any(|component| component == &conflict.component)
        {
            return Err(CoreError::invalid(
                "module activation plan omits a reviewed conflict decision",
            ));
        }
        resolutions.push(lorepia_domain::ModuleConflictResolution {
            component: conflict.component.clone(),
            expected_candidates: conflict.candidates.clone(),
            selected,
        });
    }
    Ok(lorepia_orchestration::ModuleMergeResolutionSet {
        expected_review_sha256: review.review_sha256.clone(),
        resolutions,
    })
}

fn module_component_storage_key(
    component: &lorepia_domain::ModuleComponentRef,
) -> (&'static str, &str) {
    match component {
        lorepia_domain::ModuleComponentRef::PromptBlock { id } => ("prompt_block", id.as_str()),
        lorepia_domain::ModuleComponentRef::Control { id } => ("control", id.as_str()),
        lorepia_domain::ModuleComponentRef::KnowledgeBook { id } => ("knowledge_book", id.as_str()),
        lorepia_domain::ModuleComponentRef::TransformSet { id } => ("transform_set", id.as_str()),
        lorepia_domain::ModuleComponentRef::InteractionRuleSet { id } => {
            ("interaction_rule_set", id.as_str())
        }
        lorepia_domain::ModuleComponentRef::Asset { id } => ("asset", id.as_str()),
    }
}

fn insert_module_activation_audit(
    transaction: &Transaction<'_>,
    activation_plan_id: &str,
    sequence: u64,
    plan_revision: u64,
    event_kind: &str,
    payload: &Value,
    created_at: &str,
) -> CoreResult<()> {
    let payload_json = serde_json::to_string(payload).map_err(|error| {
        CoreError::invalid(format!("cannot encode module activation audit: {error}"))
    })?;
    validate_json_bounds("module activation audit", &payload_json)?;
    transaction
        .execute(
            "INSERT INTO module_activation_audit
             (activation_plan_id, sequence, plan_revision, event_kind,
              payload_json, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                activation_plan_id,
                i64_revision(sequence)?,
                i64_revision(plan_revision)?,
                event_kind,
                payload_json,
                created_at,
            ],
        )
        .map_err(storage_db_error)?;
    Ok(())
}

fn module_binding_affects_context(
    binding: &ModuleBinding,
    context: &lorepia_orchestration::ModuleResolutionContext,
) -> bool {
    match binding.scope {
        ModuleScope::App | ModuleScope::User => true,
        ModuleScope::Persona => {
            binding.target_id.as_deref()
                == context
                    .persona_id
                    .as_ref()
                    .map(lorepia_domain::PersonaId::as_str)
        }
        ModuleScope::Character => binding.target_id.as_deref() == context.character_id.as_deref(),
        ModuleScope::Conversation => {
            binding.target_id.as_deref() == context.conversation_id.as_deref()
        }
        ModuleScope::Branch => {
            binding.target_id.as_deref() == context.branch_id.as_deref()
                && binding.conversation_id.as_ref().map(|id| id.0.as_str())
                    == context.conversation_id.as_deref()
        }
    }
}

struct AppliedModuleActivationPlanRow {
    id: String,
    revision: i64,
    activation_binding_id: String,
    plan_sha256: String,
}

fn read_applied_module_activation_plans(
    transaction: &Transaction<'_>,
) -> CoreResult<Vec<AppliedModuleActivationPlanRow>> {
    let mut statement = transaction
        .prepare(
            "SELECT id, revision, activation_binding_id, plan_sha256
             FROM module_activation_plans
             WHERE state = 'applied'
             ORDER BY applied_at, id",
        )
        .map_err(storage_db_error)?;
    statement
        .query_map([], |row| {
            Ok(AppliedModuleActivationPlanRow {
                id: row.get(0)?,
                revision: row.get(1)?,
                activation_binding_id: row.get(2)?,
                plan_sha256: row.get(3)?,
            })
        })
        .map_err(storage_db_error)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(storage_db_error)
}

fn stale_applied_module_activation_plans(
    transaction: &Transaction<'_>,
    binding_ids: &BTreeSet<String>,
    reason: &str,
    replacement_plan_sha256: Option<&lorepia_domain::Sha256Digest>,
    created_at: &str,
) -> CoreResult<()> {
    for plan in read_applied_module_activation_plans(transaction)? {
        if !binding_ids.contains(&plan.activation_binding_id)
            || replacement_plan_sha256
                .is_some_and(|replacement| replacement.as_str() == plan.plan_sha256)
        {
            continue;
        }
        let plan_revision = u64_revision(plan.revision)?;
        let stale_revision = plan_revision
            .checked_add(1)
            .ok_or_else(|| CoreError::internal("module activation revision overflow"))?;
        let changed = transaction
            .execute(
                "UPDATE module_activation_plans
                 SET state = 'stale', revision = ?2
                 WHERE id = ?1 AND state = 'applied' AND revision = ?3",
                params![
                    plan.id,
                    i64_revision(stale_revision)?,
                    i64_revision(plan_revision)?,
                ],
            )
            .map_err(storage_db_error)?;
        if changed != 1 {
            return Err(CoreError::invalid(
                "concurrent module activation invalidated the reviewed state",
            ));
        }
        insert_module_activation_audit(
            transaction,
            &plan.id,
            stale_revision,
            stale_revision,
            "stale",
            &serde_json::json!({
                "reason": reason,
                "replacement_plan_sha256": replacement_plan_sha256,
            }),
            created_at,
        )?;
    }
    Ok(())
}

struct AppliedModuleRuntimeContextRow {
    applied_plan_sha256: String,
    context_json: String,
}

fn read_applied_module_runtime_contexts(
    transaction: &Transaction<'_>,
) -> CoreResult<Vec<AppliedModuleRuntimeContextRow>> {
    let mut statement = transaction
        .prepare(
            "SELECT applied_plan_sha256, context_json
             FROM applied_module_runtime_plans
             WHERE state = 'applied'
             ORDER BY created_at, applied_plan_sha256",
        )
        .map_err(storage_db_error)?;
    statement
        .query_map([], |row| {
            Ok(AppliedModuleRuntimeContextRow {
                applied_plan_sha256: row.get(0)?,
                context_json: row.get(1)?,
            })
        })
        .map_err(storage_db_error)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(storage_db_error)
}

fn stale_applied_module_runtime_plans(
    transaction: &Transaction<'_>,
    old_binding: Option<&ModuleBinding>,
    new_binding: Option<&ModuleBinding>,
    created_at: &str,
) -> CoreResult<()> {
    for row in read_applied_module_runtime_contexts(transaction)? {
        let context: lorepia_orchestration::ModuleResolutionContext =
            decode_document("applied module runtime context", &row.context_json)?;
        let affected = old_binding
            .is_some_and(|binding| module_binding_affects_context(binding, &context))
            || new_binding.is_some_and(|binding| module_binding_affects_context(binding, &context));
        if affected {
            transaction
                .execute(
                    "UPDATE applied_module_runtime_plans
                     SET state = 'stale', stale_at = ?2
                     WHERE applied_plan_sha256 = ?1 AND state = 'applied'",
                    params![row.applied_plan_sha256, created_at],
                )
                .map_err(storage_db_error)?;
        }
    }
    Ok(())
}

fn stale_affected_module_activation_plans(
    transaction: &Transaction<'_>,
    old_binding: Option<&ModuleBinding>,
    new_binding: Option<&ModuleBinding>,
    reason: &str,
    replacement_plan_sha256: Option<&lorepia_domain::Sha256Digest>,
    created_at: &str,
) -> CoreResult<()> {
    let binding_ids = old_binding
        .into_iter()
        .chain(new_binding)
        .map(|binding| binding.id.as_str().to_owned())
        .collect::<BTreeSet<_>>();
    stale_applied_module_activation_plans(
        transaction,
        &binding_ids,
        reason,
        replacement_plan_sha256,
        created_at,
    )?;
    stale_applied_module_runtime_plans(transaction, old_binding, new_binding, created_at)
}

#[allow(clippy::too_many_lines)]
fn apply_approved_module_activation(
    storage: &Storage,
    review: &lorepia_orchestration::ModuleActivationReview,
    approved: &lorepia_orchestration::ApprovedModuleActivationPlan,
) -> CoreResult<StoredRevision<ModuleBinding>> {
    apply_approved_module_activation_internal(storage, review, approved, None, None)
}

#[allow(
    clippy::too_many_lines,
    reason = "one bounded recovery pass validates the persisted review, approval, and binding together"
)]
fn recover_applied_module_activation(
    storage: &Storage,
    binding_id: &ModuleBindingId,
    approval: &lorepia_orchestration::ModuleActivationApproval,
) -> CoreResult<Option<RecoveredModuleActivation>> {
    validate_identifier("module activation binding", binding_id.as_str())?;
    if approval.approval_id.trim().is_empty()
        || approval.approval_id.len()
            > lorepia_orchestration::MAX_MODULE_ACTIVATION_APPROVAL_ID_BYTES
        || approval.approval_id.chars().any(char::is_control)
    {
        return Err(CoreError::invalid(
            "module activation approval id is invalid",
        ));
    }
    let connection = storage.connection()?;
    let mut statement = connection
        .prepare(
            "SELECT review_json, approved_plan_json, state,
                    activation_binding_id, approval_id, approval_sha256,
                    plan_sha256, expected_bindings_revision_sha256
             FROM module_activation_plans
             WHERE plan_sha256 = ?1 OR approval_id = ?2
             ORDER BY id",
        )
        .map_err(storage_db_error)?;
    let rows = statement
        .query_map(
            params![approval.expected_plan_sha256.as_str(), approval.approval_id],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, String>(4)?,
                    row.get::<_, String>(5)?,
                    row.get::<_, String>(6)?,
                    row.get::<_, String>(7)?,
                ))
            },
        )
        .map_err(storage_db_error)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(storage_db_error)?;
    drop(statement);
    let Some(row) = rows.first() else {
        return Ok(None);
    };
    if rows.len() != 1 {
        return Err(CoreError::invalid(
            "module approval id and plan hash belong to different activations",
        ));
    }
    if row.2 != "applied"
        || row.3 != binding_id.as_str()
        || row.4 != approval.approval_id
        || row.6 != approval.expected_plan_sha256.as_str()
        || row.7 != approval.expected_review_sha256.as_str()
    {
        return Err(CoreError::invalid(
            "module activation approval identity is already bound to another request",
        ));
    }
    let review: lorepia_orchestration::ModuleActivationReview =
        decode_document("applied module activation review", &row.0)?;
    let approved: lorepia_orchestration::ApprovedModuleActivationPlan =
        decode_document("applied module activation plan", &row.1)?;
    review.verify().map_err(|error| {
        storage_corrupted(format!(
            "applied module activation review is invalid: {error}"
        ))
    })?;
    approved.verify().map_err(|error| {
        storage_corrupted(format!(
            "applied module activation plan is invalid: {error}"
        ))
    })?;
    if approved.approval_id != approval.approval_id
        || approved.approval_sha256.as_str() != row.5
        || approved.plan.plan_sha256 != approval.expected_plan_sha256
        || approved.plan.review_sha256 != approval.expected_review_sha256
        || review.review_sha256 != approval.expected_review_sha256
        || approved.plan.expected_state_revision != review.state_revision
        || approved.plan.activation_binding_ids != review.activation_binding_ids
        || review.activation_binding_ids.len() != 1
        || review.activation_binding_ids.first() != Some(binding_id)
    {
        return Err(storage_corrupted(
            "applied module activation authority is internally inconsistent",
        ));
    }
    let binding_row = connection
        .query_row(
            "SELECT document_json, revision, created_at, updated_at, deleted_at
             FROM content_module_bindings
             WHERE id = ?1",
            [binding_id.as_str()],
            module_binding_row,
        )
        .optional()
        .map_err(storage_db_error)?
        .ok_or_else(|| storage_corrupted("applied module activation binding is missing"))?;
    let binding =
        decode_stored_document::<ModuleBinding>("applied module activation binding", binding_row)?;
    if binding.deleted_at.is_some()
        || !binding.value.enabled
        || !binding.value.approved
        || binding.value.activation_approval_id.as_deref() != Some(approved.approval_id.as_str())
        || binding.value.activation_review_sha256.as_ref() != Some(&review.review_sha256)
        || binding.value.activation_plan_sha256.as_ref() != Some(&approved.plan.plan_sha256)
    {
        return Err(storage_corrupted(
            "applied module activation binding no longer matches its authority",
        ));
    }
    Ok(Some(RecoveredModuleActivation {
        review,
        approved,
        binding,
    }))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ModuleActivationPreparedAuditPayload {
    review_sha256: lorepia_domain::Sha256Digest,
    plan_sha256: lorepia_domain::Sha256Digest,
    binding_id: ModuleBindingId,
    rollback: Option<lorepia_orchestration::ModuleRollbackPlan>,
    rollback_approval_sha256: Option<lorepia_domain::Sha256Digest>,
}

fn recover_applied_module_rollback(
    storage: &Storage,
    binding_id: &ModuleBindingId,
    approval: &lorepia_orchestration::ModuleActivationApproval,
) -> CoreResult<Option<RecoveredModuleRollback>> {
    let Some(recovered) = recover_applied_module_activation(storage, binding_id, approval)? else {
        return Ok(None);
    };
    let connection = storage.connection()?;
    let payload_json = connection
        .query_row(
            "SELECT audit.payload_json
             FROM module_activation_plans AS plan
             JOIN module_activation_audit AS audit
               ON audit.activation_plan_id = plan.id
             WHERE plan.plan_sha256 = ?1
               AND plan.approval_id = ?2
               AND plan.activation_binding_id = ?3
               AND audit.sequence = 1
               AND audit.plan_revision = 1
               AND audit.event_kind = 'prepared'",
            params![
                recovered.approved.plan.plan_sha256.as_str(),
                recovered.approved.approval_id.as_str(),
                binding_id.as_str(),
            ],
            |row| row.get::<_, String>(0),
        )
        .optional()
        .map_err(storage_db_error)?
        .ok_or_else(|| {
            storage_corrupted("applied module activation has no prepared audit authority")
        })?;
    let payload: ModuleActivationPreparedAuditPayload =
        decode_document("applied module rollback audit", &payload_json)?;
    if payload.review_sha256 != recovered.review.review_sha256
        || payload.plan_sha256 != recovered.approved.plan.plan_sha256
        || payload.binding_id != *binding_id
    {
        return Err(storage_corrupted(
            "applied module rollback audit disagrees with its activation authority",
        ));
    }
    let (rollback, approval_sha256) = match (payload.rollback, payload.rollback_approval_sha256) {
        (Some(rollback), Some(approval_sha256)) => (rollback, approval_sha256),
        (None, None) => {
            return Err(CoreError::invalid(
                "module activation approval identity belongs to a non-rollback activation",
            ));
        }
        _ => {
            return Err(storage_corrupted(
                "applied module rollback audit has incomplete rollback authority",
            ));
        }
    };
    let approved = lorepia_orchestration::ApprovedModuleRollbackPlan {
        approval_sha256,
        rollback,
        activation_review: recovered.review,
        activation: recovered.approved,
    };
    approved.verify().map_err(|error| {
        storage_corrupted(format!(
            "applied module rollback authority is invalid: {error}"
        ))
    })?;
    Ok(Some(RecoveredModuleRollback {
        approved,
        binding: recovered.binding,
    }))
}

fn apply_approved_module_rollback(
    storage: &Storage,
    approved: &lorepia_orchestration::ApprovedModuleRollbackPlan,
) -> CoreResult<StoredRevision<ModuleBinding>> {
    approved.verify().map_err(|error| {
        CoreError::invalid(format!("invalid approved module rollback: {error}"))
    })?;
    apply_approved_module_activation_internal(
        storage,
        &approved.activation_review,
        &approved.activation,
        Some(&approved.rollback),
        Some(&approved.approval_sha256),
    )
}

#[allow(clippy::too_many_lines)]
fn apply_approved_module_activation_internal(
    storage: &Storage,
    review: &lorepia_orchestration::ModuleActivationReview,
    approved: &lorepia_orchestration::ApprovedModuleActivationPlan,
    rollback: Option<&lorepia_orchestration::ModuleRollbackPlan>,
    rollback_approval_sha256: Option<&lorepia_domain::Sha256Digest>,
) -> CoreResult<StoredRevision<ModuleBinding>> {
    review.verify().map_err(|error| {
        CoreError::invalid(format!("invalid module activation review: {error}"))
    })?;
    approved
        .verify()
        .map_err(|error| CoreError::invalid(format!("invalid module activation plan: {error}")))?;
    if approved.plan.review_sha256 != review.review_sha256
        || approved.plan.expected_state_revision != review.state_revision
        || approved.plan.activation_binding_ids != review.activation_binding_ids
    {
        return Err(CoreError::invalid(
            "module activation approval does not match the reviewed state",
        ));
    }
    let activation_id = review
        .activation_binding_ids
        .as_slice()
        .first()
        .ok_or_else(|| CoreError::invalid("module activation requires one binding"))?;
    if review.activation_binding_ids.len() != 1 {
        return Err(CoreError::invalid(
            "module activation requires exactly one binding",
        ));
    }
    let proposed = review
        .ordered_bindings
        .iter()
        .find(|binding| &binding.id == activation_id)
        .cloned()
        .ok_or_else(|| {
            CoreError::invalid("activation binding is not effective in the reviewed context")
        })?;

    let resolution_set = module_activation_resolution_set(review, &approved.plan)?;
    let reconstructed = lorepia_orchestration::resolve_module_merge(review, &resolution_set)
        .map_err(|error| {
            CoreError::invalid(format!(
                "module activation plan is not review-derived: {error}"
            ))
        })?;
    if reconstructed != approved.plan {
        return Err(CoreError::invalid(
            "module activation plan differs from the reviewed resolution",
        ));
    }

    let verified_authorities = verify_module_import_authorities(storage, &review.ordered_bindings)?;

    let mut connection = storage.connection()?;
    let transaction = connection
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(storage_db_error)?;

    if let Some(state) = transaction
        .query_row(
            "SELECT state FROM module_activation_plans WHERE plan_sha256 = ?1",
            [approved.plan.plan_sha256.as_str()],
            |row| row.get::<_, String>(0),
        )
        .optional()
        .map_err(storage_db_error)?
    {
        if state != "applied" {
            return Err(CoreError::invalid(
                "module activation plan already exists in a nonterminal state",
            ));
        }
        let row = transaction
            .query_row(
                "SELECT document_json, revision, created_at, updated_at, deleted_at
                 FROM content_module_bindings WHERE id = ?1",
                [activation_id.as_str()],
                module_binding_row,
            )
            .optional()
            .map_err(storage_db_error)?
            .ok_or_else(|| storage_corrupted("applied activation binding is missing"))?;
        let stored = decode_stored_document::<ModuleBinding>("module binding", row)?;
        if stored.deleted_at.is_some()
            || !stored.value.enabled
            || !stored.value.approved
            || stored.value.activation_approval_id.as_deref() != Some(approved.approval_id.as_str())
            || stored.value.activation_review_sha256.as_ref() != Some(&review.review_sha256)
            || stored.value.activation_plan_sha256.as_ref() != Some(&approved.plan.plan_sha256)
        {
            return Err(storage_corrupted(
                "applied module activation does not match its durable binding",
            ));
        }
        persist_initial_applied_module_runtime_plan(
            storage,
            &transaction,
            approved,
            &review.context,
            stored.updated_at,
            &verified_authorities,
        )?;
        transaction.commit().map_err(storage_db_error)?;
        return Ok(stored);
    }

    let current_rows = list_all_module_bindings_transaction(&transaction)?;
    let current_target = current_rows
        .iter()
        .find(|stored| stored.value.id == *activation_id);
    let old_binding = current_target.map(|stored| stored.value.clone());
    if let Some(rollback) = rollback {
        let current = current_target.ok_or_else(|| not_found("module rollback binding"))?;
        if current.revision != rollback.expected_state_revision
            || current.value.id != rollback.binding_id
            || current.value.revision_id != rollback.expected_current_revision_id
        {
            return Err(revision_conflict(
                "module rollback binding",
                rollback.binding_id.as_str(),
                Some(rollback.expected_state_revision),
                Some(current.revision),
            ));
        }
        let current_snapshot = load_content_module_revision(
            &transaction,
            &current.value.module_id,
            rollback.expected_current_revision_id.as_str(),
        )?;
        let target_snapshot = load_content_module_revision(
            &transaction,
            &current.value.module_id,
            rollback.target_revision_id.as_str(),
        )?;
        if current_snapshot.module_revision.source_hash != rollback.expected_current_source_sha256
            || target_snapshot.module_revision.source_hash != rollback.target_source_sha256
        {
            return Err(CoreError::invalid(
                "module rollback source hash changed after review",
            ));
        }
        let diff = lorepia_orchestration::diff_module_revisions(
            &lorepia_orchestration::ModuleRevisionSnapshot {
                module: current_snapshot.object.value.clone(),
                revision: current_snapshot.module_revision.clone(),
                import_approval: None,
            },
            &lorepia_orchestration::ModuleRevisionSnapshot {
                module: target_snapshot.object.value.clone(),
                revision: target_snapshot.module_revision.clone(),
                import_approval: None,
            },
        )
        .map_err(|error| {
            CoreError::invalid(format!(
                "cannot revalidate approved module rollback: {error}"
            ))
        })?;
        if diff.diff_sha256 != rollback.diff_sha256 {
            return Err(CoreError::invalid(
                "module rollback diff changed after approval",
            ));
        }
        let target_is_ancestor = transaction
            .query_row(
                "WITH RECURSIVE ancestors(revision_id, previous_revision_id) AS (
                     SELECT revision_id, previous_revision_id
                     FROM content_module_revisions
                     WHERE module_id = ?1 AND revision_id = ?2
                     UNION
                     SELECT parent.revision_id, parent.previous_revision_id
                     FROM content_module_revisions AS parent
                     JOIN ancestors AS child
                       ON child.previous_revision_id = parent.revision_id
                     WHERE parent.module_id = ?1
                 )
                 SELECT EXISTS(
                     SELECT 1 FROM ancestors WHERE revision_id = ?3
                 )",
                params![
                    current.value.module_id.as_str(),
                    rollback.expected_current_revision_id.as_str(),
                    rollback.target_revision_id.as_str(),
                ],
                |row| row.get::<_, bool>(0),
            )
            .map_err(storage_db_error)?;
        if !target_is_ancestor {
            return Err(CoreError::invalid(
                "approved module rollback target is not an ancestor",
            ));
        }
    }
    let expected_revision = if review.state_revision == 0 {
        if current_target.is_some() {
            return Err(revision_conflict(
                "module activation binding",
                activation_id.as_str(),
                None,
                current_target.map(|stored| stored.revision),
            ));
        }
        None
    } else {
        let current = current_target.ok_or_else(|| {
            revision_conflict(
                "module activation binding",
                activation_id.as_str(),
                Some(review.state_revision),
                None,
            )
        })?;
        if current.revision != review.state_revision
            || current.value.created_at != proposed.created_at
        {
            return Err(revision_conflict(
                "module activation binding",
                activation_id.as_str(),
                Some(review.state_revision),
                Some(current.revision),
            ));
        }
        Some(review.state_revision)
    };

    let mut current_bindings = current_rows
        .iter()
        .map(|stored| resolve_module_binding_revision(&transaction, &stored.value))
        .collect::<CoreResult<Vec<_>>>()?;
    let resolved_proposed = resolve_module_binding_revision(&transaction, &proposed)?;
    if resolved_proposed != proposed {
        return Err(CoreError::invalid(
            "module activation revision changed after review",
        ));
    }
    let mut snapshot_bindings = current_bindings.clone();
    if let Some(position) = snapshot_bindings
        .iter()
        .position(|binding| binding.id == proposed.id)
    {
        snapshot_bindings[position] = proposed.clone();
    } else {
        snapshot_bindings.push(proposed.clone());
    }
    let snapshots = module_activation_snapshots(
        storage,
        &transaction,
        &snapshot_bindings,
        &verified_authorities,
    )?;
    let rereview = lorepia_orchestration::review_module_activation(
        expected_revision,
        &review.context,
        &current_bindings,
        &proposed,
        &snapshots,
    )
    .map_err(|error| CoreError::invalid(format!("module activation review is stale: {error}")))?;
    if &rereview != review {
        return Err(CoreError::invalid(
            "module activation candidates changed after review",
        ));
    }

    let now = Utc::now();
    let now_text = now.to_rfc3339();
    let activation_plan_id = Uuid::new_v4().to_string();
    let targets = module_binding_targets(&proposed)?;
    let input_module_revisions = snapshots
        .iter()
        .map(|snapshot| {
            serde_json::json!({
                "module_id": snapshot.revision.module_id,
                "revision_id": snapshot.revision.id,
                "source_sha256": snapshot.revision.source_hash,
            })
        })
        .collect::<Vec<_>>();
    let input_module_revisions_json =
        serde_json::to_string(&input_module_revisions).map_err(|error| {
            CoreError::invalid(format!("cannot encode activation revisions: {error}"))
        })?;
    let conflicts_json = serde_json::to_string(&review.conflicts).map_err(|error| {
        CoreError::invalid(format!("cannot encode activation conflicts: {error}"))
    })?;
    let resolutions_json = serde_json::to_string(&resolution_set.resolutions).map_err(|error| {
        CoreError::invalid(format!("cannot encode activation resolutions: {error}"))
    })?;
    let review_json = serde_json::to_string(review).map_err(|error| {
        CoreError::invalid(format!("cannot encode module activation review: {error}"))
    })?;
    let approved_plan_json = serde_json::to_string(approved).map_err(|error| {
        CoreError::invalid(format!("cannot encode approved module activation: {error}"))
    })?;
    validate_json_bounds("module activation revisions", &input_module_revisions_json)?;
    validate_json_bounds("module activation conflicts", &conflicts_json)?;
    validate_json_bounds("module activation resolutions", &resolutions_json)?;
    validate_json_bounds("module activation review", &review_json)?;
    validate_json_bounds("approved module activation", &approved_plan_json)?;
    let merge_sha256 = sha256_hex(resolutions_json.as_bytes());

    stale_affected_module_activation_plans(
        &transaction,
        old_binding.as_ref(),
        Some(&proposed),
        if rollback.is_some() {
            "binding_rollback"
        } else {
            "binding_activation"
        },
        Some(&approved.plan.plan_sha256),
        &now_text,
    )?;
    transaction
        .execute(
            "INSERT INTO module_activation_plans
             (id, scope_kind, expected_bindings_revision_sha256,
              input_module_revisions_json, conflicts_json, resolutions_json,
              merge_sha256, plan_sha256, activation_binding_id, review_json,
              approved_plan_json, approval_id, approval_sha256, state,
              revision, prepared_at, approved_at, applied_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12,
                     ?13, 'prepared', 1, ?14, NULL, NULL)",
            params![
                activation_plan_id,
                targets.scope_kind,
                review.review_sha256.as_str(),
                input_module_revisions_json,
                conflicts_json,
                resolutions_json,
                merge_sha256,
                approved.plan.plan_sha256.as_str(),
                activation_id.as_str(),
                review_json,
                approved_plan_json,
                approved.approval_id,
                approved.approval_sha256.as_str(),
                now_text,
            ],
        )
        .map_err(storage_db_error)?;
    for (ordinal, conflict) in review.conflicts.iter().enumerate() {
        let (component_kind, component_key) = module_component_storage_key(&conflict.component);
        let selected = resolution_set
            .resolutions
            .iter()
            .find(|resolution| resolution.component == conflict.component)
            .and_then(|resolution| resolution.selected.as_ref());
        let expected_json = serde_json::to_string(&conflict.candidates).map_err(|error| {
            CoreError::invalid(format!("cannot encode activation candidates: {error}"))
        })?;
        let selected_json = selected
            .map(serde_json::to_string)
            .transpose()
            .map_err(|error| {
                CoreError::invalid(format!("cannot encode selected module candidate: {error}"))
            })?;
        let resolution_sha256 = sha256_hex(
            serde_json::json!({
                "expected": conflict.candidates,
                "selected": selected,
            })
            .to_string()
            .as_bytes(),
        );
        transaction
            .execute(
                "INSERT INTO module_conflict_resolutions
                 (activation_plan_id, ordinal, component_kind, component_key,
                  expected_candidates_json, selected_candidate_json,
                  resolution_sha256)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                params![
                    activation_plan_id,
                    usize_to_i64(ordinal, "module conflict ordinal")?,
                    component_kind,
                    component_key,
                    expected_json,
                    selected_json,
                    resolution_sha256,
                ],
            )
            .map_err(storage_db_error)?;
    }
    insert_module_activation_audit(
        &transaction,
        &activation_plan_id,
        1,
        1,
        "prepared",
        &serde_json::json!({
            "review_sha256": review.review_sha256,
            "plan_sha256": approved.plan.plan_sha256,
            "binding_id": activation_id,
            "rollback": rollback,
            "rollback_approval_sha256": rollback_approval_sha256,
        }),
        &now_text,
    )?;
    transaction
        .execute(
            "UPDATE module_activation_plans
             SET state = 'approved', revision = 2, approved_at = ?2
             WHERE id = ?1 AND state = 'prepared' AND revision = 1",
            params![activation_plan_id, now_text],
        )
        .map_err(storage_db_error)?;
    insert_module_activation_audit(
        &transaction,
        &activation_plan_id,
        2,
        2,
        "approved",
        &serde_json::json!({
            "approval_id": approved.approval_id,
            "approval_sha256": approved.approval_sha256,
        }),
        &now_text,
    )?;

    let mut activated = proposed;
    activated.enabled = true;
    activated.approved = true;
    activated.activation_approval_id = Some(approved.approval_id.clone());
    activated.activation_review_sha256 = Some(review.review_sha256.clone());
    activated.activation_plan_sha256 = Some(approved.plan.plan_sha256.clone());
    let stored = write_module_binding_transaction(&transaction, &activated, expected_revision)?;

    let changed = transaction
        .execute(
            "UPDATE module_activation_plans
             SET state = 'applied', revision = 3, applied_at = ?2
             WHERE id = ?1 AND state = 'approved' AND revision = 2",
            params![activation_plan_id, now_text],
        )
        .map_err(storage_db_error)?;
    if changed != 1 {
        return Err(storage_corrupted(
            "module activation plan could not enter applied state",
        ));
    }
    insert_module_activation_audit(
        &transaction,
        &activation_plan_id,
        3,
        3,
        "applied",
        &serde_json::json!({
            "binding_id": activation_id,
            "binding_revision": stored.revision,
            "module_revision_id": stored.value.revision_id,
        }),
        &now_text,
    )?;
    persist_initial_applied_module_runtime_plan(
        storage,
        &transaction,
        approved,
        &review.context,
        now,
        &verified_authorities,
    )?;
    transaction.commit().map_err(storage_db_error)?;
    current_bindings.clear();
    Ok(stored)
}

fn persist_initial_applied_module_runtime_plan(
    storage: &Storage,
    transaction: &Transaction<'_>,
    approved: &lorepia_orchestration::ApprovedModuleActivationPlan,
    context: &lorepia_orchestration::ModuleResolutionContext,
    created_at: DateTime<Utc>,
    verified_authorities: &VerifiedCompletedPackageAuthorities,
) -> CoreResult<lorepia_orchestration::AppliedModuleRuntimePlan> {
    let current_rows = list_all_module_bindings_transaction(transaction)?;
    let current_bindings = current_rows
        .iter()
        .map(|stored| resolve_module_binding_revision(transaction, &stored.value))
        .collect::<CoreResult<Vec<_>>>()?;
    let snapshots = module_activation_snapshots(
        storage,
        transaction,
        &current_bindings,
        verified_authorities,
    )?;
    let current_review =
        lorepia_orchestration::review_module_merge(0, context, &current_bindings, &snapshots)
            .map_err(|error| {
                CoreError::invalid(format!(
                    "cannot review newly applied module runtime plan: {error}"
                ))
            })?;
    let runtime =
        lorepia_orchestration::materialize_approved_module_runtime_plan(approved, &current_review)
            .map_err(|error| {
                CoreError::invalid(format!(
                    "cannot materialize newly applied module runtime plan: {error}"
                ))
            })?;
    persist_applied_module_runtime_plan_transaction(transaction, &runtime, created_at)?;
    Ok(runtime)
}

fn verify_exact_applied_runtime_source(
    transaction: &Transaction<'_>,
    source: &lorepia_orchestration::ApprovedModuleActivationPlan,
) -> CoreResult<lorepia_orchestration::ModuleMergeReview> {
    verify_exact_applied_runtime_source_with_stale_authority(transaction, source, false)
}

fn verify_exact_applied_runtime_source_with_stale_authority(
    transaction: &Transaction<'_>,
    source: &lorepia_orchestration::ApprovedModuleActivationPlan,
    allow_stale: bool,
) -> CoreResult<lorepia_orchestration::ModuleMergeReview> {
    source.verify().map_err(|error| {
        storage_corrupted(format!(
            "applied runtime source approval is invalid: {error}"
        ))
    })?;
    let row = transaction
        .query_row(
            "SELECT review_json, approved_plan_json, state,
                    approval_id, approval_sha256, plan_sha256,
                    expected_bindings_revision_sha256
             FROM module_activation_plans
             WHERE plan_sha256 = ?1 AND approval_sha256 = ?2",
            params![
                source.plan.plan_sha256.as_str(),
                source.approval_sha256.as_str(),
            ],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, String>(4)?,
                    row.get::<_, String>(5)?,
                    row.get::<_, String>(6)?,
                ))
            },
        )
        .optional()
        .map_err(storage_db_error)?
        .ok_or_else(|| {
            CoreError::invalid("applied runtime plan source activation is not currently applied")
        })?;
    if row.2 != "applied" && !(allow_stale && row.2 == "stale") {
        return Err(CoreError::invalid(
            "applied runtime plan source activation is stale",
        ));
    }
    let review: lorepia_orchestration::ModuleMergeReview =
        decode_document("applied runtime source activation review", &row.0)?;
    let approved: lorepia_orchestration::ApprovedModuleActivationPlan =
        decode_document("applied runtime source activation approval", &row.1)?;
    review.verify().map_err(|error| {
        storage_corrupted(format!(
            "applied runtime source activation review is invalid: {error}"
        ))
    })?;
    approved.verify().map_err(|error| {
        storage_corrupted(format!(
            "applied runtime source activation approval is invalid: {error}"
        ))
    })?;
    if approved != *source
        || row.3 != source.approval_id
        || row.4 != source.approval_sha256.as_str()
        || row.5 != source.plan.plan_sha256.as_str()
        || row.6 != source.plan.review_sha256.as_str()
        || review.review_sha256 != source.plan.review_sha256
        || review.state_revision != source.plan.expected_state_revision
        || review.activation_binding_ids != source.plan.activation_binding_ids
    {
        return Err(storage_corrupted(
            "applied runtime source authority differs from its immutable activation row",
        ));
    }
    Ok(review)
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ModuleActivationRevisionEvidence {
    module_id: String,
    revision_id: String,
    source_sha256: String,
}

fn same_module_runtime_binding(left: &ModuleBinding, right: &ModuleBinding) -> bool {
    left.id == right.id
        && left.module_id == right.module_id
        && left.scope == right.scope
        && left.target_id == right.target_id
        && left.conversation_id == right.conversation_id
        && left.priority == right.priority
        && left.resolution_mode == right.resolution_mode
        && left.pinned_revision_id == right.pinned_revision_id
        && left.package_import_approval_id == right.package_import_approval_id
        && left.variable_overrides == right.variable_overrides
        && left.revision_id == right.revision_id
        && left.created_at == right.created_at
}

#[allow(clippy::too_many_lines)]
#[allow(dead_code)]
fn get_applied_module_runtime_plan_legacy(
    storage: &Storage,
    current_review: &lorepia_orchestration::ModuleMergeReview,
) -> CoreResult<lorepia_orchestration::ApprovedModuleActivationPlan> {
    current_review.verify().map_err(|error| {
        CoreError::invalid(format!("invalid current module runtime review: {error}"))
    })?;
    if !current_review.activation_binding_ids.is_empty() {
        return Err(CoreError::invalid(
            "runtime module review must not contain a pending activation",
        ));
    }
    let verified_authorities =
        verify_module_import_authorities(storage, &current_review.ordered_bindings)?;
    let mut connection = storage.connection()?;
    let transaction = connection.transaction().map_err(storage_db_error)?;
    let current_rows = list_all_module_bindings_transaction(&transaction)?;
    let current_bindings = current_rows
        .iter()
        .map(|stored| resolve_module_binding_revision(&transaction, &stored.value))
        .collect::<CoreResult<Vec<_>>>()?;
    let snapshots = module_activation_snapshots(
        storage,
        &transaction,
        &current_bindings,
        &verified_authorities,
    )?;
    let rereview = lorepia_orchestration::review_module_merge(
        current_review.state_revision,
        &current_review.context,
        &current_bindings,
        &snapshots,
    )
    .map_err(|error| CoreError::invalid(format!("current module review is stale: {error}")))?;
    if &rereview != current_review {
        return Err(CoreError::invalid(
            "current module review does not match durable bindings",
        ));
    }
    let matching_plan_ids = {
        let mut statement = transaction
            .prepare(
                "SELECT id, review_json
                 FROM module_activation_plans
                 WHERE state = 'applied'
                 ORDER BY applied_at, id",
            )
            .map_err(storage_db_error)?;
        let rows = statement
            .query_map([], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })
            .map_err(storage_db_error)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(storage_db_error)?;
        rows.into_iter()
            .filter_map(|(id, review_json)| {
                let review = serde_json::from_str::<lorepia_orchestration::ModuleActivationReview>(
                    &review_json,
                )
                .ok()?;
                (review.context == current_review.context).then_some(id)
            })
            .collect::<Vec<_>>()
    };
    let matching_plan_id = match matching_plan_ids.as_slice() {
        [] => return Err(not_found("applied module runtime plan for context")),
        [id] => id,
        _ => {
            return Err(storage_corrupted(
                "multiple module runtime plans are applied to the same context",
            ));
        }
    };
    let row = transaction
        .query_row(
            "SELECT binding.document_json, binding.revision,
                    binding.created_at, binding.updated_at, binding.deleted_at,
                    plan.review_json, plan.approved_plan_json,
                    plan.input_module_revisions_json, plan.plan_sha256,
                    plan.approval_id, plan.approval_sha256,
                    plan.expected_bindings_revision_sha256,
                    plan.activation_binding_id
             FROM content_module_bindings AS binding
             JOIN module_activation_plans AS plan
              ON plan.activation_binding_id = binding.id
              AND plan.plan_sha256 = binding.activation_plan_sha256
             WHERE binding.deleted_at IS NULL
               AND binding.enabled = 1
               AND binding.approved = 1
               AND plan.state = 'applied'
               AND plan.id = ?1",
            [matching_plan_id],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, Option<String>>(4)?,
                    row.get::<_, String>(5)?,
                    row.get::<_, String>(6)?,
                    row.get::<_, String>(7)?,
                    row.get::<_, String>(8)?,
                    row.get::<_, String>(9)?,
                    row.get::<_, String>(10)?,
                    row.get::<_, String>(11)?,
                    row.get::<_, String>(12)?,
                ))
            },
        )
        .optional()
        .map_err(storage_db_error)?
        .ok_or_else(|| not_found("applied module activation plan"))?;
    let binding_id = ModuleBindingId::from(row.12.clone());
    let binding = decode_stored_document::<ModuleBinding>(
        "module binding",
        (row.0, row.1, None, row.2, row.3, row.4),
    )?;
    validate_optional_sha256("stored module activation plan hash", Some(&row.8)).map_err(
        |error| {
            storage_corrupted(format!(
                "stored module activation plan hash is invalid: {}",
                error.message
            ))
        },
    )?;
    validate_optional_sha256("stored module activation approval hash", Some(&row.10)).map_err(
        |error| {
            storage_corrupted(format!(
                "stored module activation approval hash is invalid: {}",
                error.message
            ))
        },
    )?;
    validate_json_bounds("stored module activation review", &row.5).map_err(|error| {
        storage_corrupted(format!(
            "stored module activation review violates bounds: {}",
            error.message
        ))
    })?;
    validate_json_bounds("stored approved module activation", &row.6).map_err(|error| {
        storage_corrupted(format!(
            "stored approved module activation violates bounds: {}",
            error.message
        ))
    })?;
    validate_json_bounds("stored module activation revisions", &row.7).map_err(|error| {
        storage_corrupted(format!(
            "stored module activation revisions violate bounds: {}",
            error.message
        ))
    })?;
    let review: lorepia_orchestration::ModuleActivationReview = serde_json::from_str(&row.5)
        .map_err(|error| {
            storage_corrupted(format!(
                "stored module activation review is invalid: {error}"
            ))
        })?;
    let approved: lorepia_orchestration::ApprovedModuleActivationPlan =
        serde_json::from_str(&row.6).map_err(|error| {
            storage_corrupted(format!(
                "stored approved module activation is invalid: {error}"
            ))
        })?;
    let revision_evidence: Vec<ModuleActivationRevisionEvidence> = serde_json::from_str(&row.7)
        .map_err(|error| {
            storage_corrupted(format!(
                "stored module activation revision evidence is invalid: {error}"
            ))
        })?;
    review.verify().map_err(|error| {
        storage_corrupted(format!(
            "stored module activation review failed verification: {error}"
        ))
    })?;
    approved.verify().map_err(|error| {
        storage_corrupted(format!(
            "stored approved module activation failed verification: {error}"
        ))
    })?;
    if approved.plan.review_sha256 != review.review_sha256
        || approved.plan.plan_sha256.as_str() != row.8
        || approved.approval_id != row.9
        || approved.approval_sha256.as_str() != row.10
        || review.review_sha256.as_str() != row.11
        || binding_id.as_str() != row.12
        || review.activation_binding_ids.as_slice() != [binding_id.clone()]
        || binding.value.activation_approval_id.as_deref() != Some(row.9.as_str())
        || binding.value.activation_review_sha256.as_ref() != Some(&review.review_sha256)
        || binding.value.activation_plan_sha256.as_ref() != Some(&approved.plan.plan_sha256)
    {
        return Err(storage_corrupted(
            "stored module activation plan, approval, and binding disagree",
        ));
    }
    let resolution_set = module_activation_resolution_set(&review, &approved.plan)?;
    let reconstructed = lorepia_orchestration::resolve_module_merge(&review, &resolution_set)
        .map_err(|error| {
            storage_corrupted(format!(
                "stored module activation plan is not review-derived: {error}"
            ))
        })?;
    if reconstructed != approved.plan {
        return Err(storage_corrupted(
            "stored module activation plan differs from its reviewed resolution",
        ));
    }
    if review.context != current_review.context
        || review.ignored_bindings != current_review.ignored_bindings
        || review.ordered_bindings.len() != current_review.ordered_bindings.len()
        || !review
            .ordered_bindings
            .iter()
            .zip(&current_review.ordered_bindings)
            .all(|(reviewed, current)| same_module_runtime_binding(reviewed, current))
    {
        return Err(CoreError::invalid(
            "applied module plan does not match the current context and binding set",
        ));
    }
    let current_resolution_set = module_activation_resolution_set(current_review, &approved.plan)?;
    let current_plan =
        lorepia_orchestration::resolve_module_merge(current_review, &current_resolution_set)
            .map_err(|error| {
                CoreError::invalid(format!(
                    "applied module selection is stale for the current context: {error}"
                ))
            })?;
    if current_plan.ordered_binding_ids != approved.plan.ordered_binding_ids
        || current_plan.components != approved.plan.components
        || current_plan.omitted_components != approved.plan.omitted_components
        || current_plan.effective_variable_overrides != approved.plan.effective_variable_overrides
    {
        return Err(CoreError::invalid(
            "applied module components are stale for the current context",
        ));
    }

    let mut persisted_snapshots = BTreeMap::new();
    for evidence in revision_evidence {
        validate_identifier("module activation module", &evidence.module_id)?;
        validate_identifier("module activation revision", &evidence.revision_id)?;
        validate_optional_sha256(
            "module activation revision source hash",
            Some(&evidence.source_sha256),
        )
        .map_err(|error| {
            storage_corrupted(format!(
                "stored activation revision source hash is invalid: {}",
                error.message
            ))
        })?;
        let key = (evidence.module_id.clone(), evidence.revision_id.clone());
        if persisted_snapshots.contains_key(&key) {
            return Err(storage_corrupted(
                "stored module activation revision evidence is duplicated",
            ));
        }
        let snapshot = load_content_module_revision(
            &transaction,
            &ContentModuleId::from(evidence.module_id),
            &evidence.revision_id,
        )?;
        if snapshot.module_revision.source_hash.as_str() != evidence.source_sha256 {
            return Err(storage_corrupted(
                "module revision source changed after activation approval",
            ));
        }
        persisted_snapshots.insert(key, snapshot.module_revision);
    }
    for component in &approved.plan.components {
        for source in
            std::iter::once(&component.selected_source).chain(component.coalesced_sources.iter())
        {
            let revision = persisted_snapshots
                .get(&(
                    source.module_id.as_str().to_owned(),
                    source.revision_id.as_str().to_owned(),
                ))
                .ok_or_else(|| {
                    storage_corrupted("approved module component lacks exact revision evidence")
                })?;
            if revision.source_hash != source.revision_source_sha256 {
                return Err(storage_corrupted(
                    "approved module component source hash is stale",
                ));
            }
            let component_hash = revision
                .component_hashes
                .iter()
                .find(|hash| hash.component == component.component)
                .ok_or_else(|| {
                    storage_corrupted(
                        "approved module component is missing from its immutable revision",
                    )
                })?;
            if component_hash.sha256 != component.sha256 {
                return Err(storage_corrupted("approved module component hash is stale"));
            }
        }
    }
    transaction.commit().map_err(storage_db_error)?;
    Ok(approved)
}

fn get_applied_module_runtime_plan(
    storage: &Storage,
    current_review: &lorepia_orchestration::ModuleMergeReview,
) -> CoreResult<lorepia_orchestration::AppliedModuleRuntimePlan> {
    let runtime = preview_applied_module_runtime_plan(storage, current_review)?;
    let verified_authorities =
        verify_module_import_authorities(storage, &current_review.ordered_bindings)?;
    let mut connection = storage.connection()?;
    let transaction = connection
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(storage_db_error)?;
    validate_fresh_module_merge_review(
        storage,
        &transaction,
        current_review,
        &verified_authorities,
    )?;
    persist_applied_module_runtime_plan_transaction(&transaction, &runtime, Utc::now())?;
    transaction.commit().map_err(storage_db_error)?;
    Ok(runtime)
}

fn preview_applied_module_runtime_plan(
    storage: &Storage,
    current_review: &lorepia_orchestration::ModuleMergeReview,
) -> CoreResult<lorepia_orchestration::AppliedModuleRuntimePlan> {
    current_review.verify().map_err(|error| {
        CoreError::invalid(format!("invalid current module runtime review: {error}"))
    })?;
    if !current_review.activation_binding_ids.is_empty() {
        return Err(CoreError::invalid(
            "runtime module review must not contain a pending activation",
        ));
    }
    let verified_authorities =
        verify_module_import_authorities(storage, &current_review.ordered_bindings)?;
    let mut connection = storage.connection()?;
    let transaction = connection
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(storage_db_error)?;
    validate_fresh_module_merge_review(
        storage,
        &transaction,
        current_review,
        &verified_authorities,
    )?;

    let candidates = {
        let mut statement = transaction
            .prepare(
                "SELECT plan_sha256, approval_sha256, approved_plan_json
                 FROM module_activation_plans
                 WHERE state = 'applied'
                 ORDER BY applied_at, id",
            )
            .map_err(storage_db_error)?;
        statement
            .query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                ))
            })
            .map_err(storage_db_error)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(storage_db_error)?
    };
    let mut applicable = Vec::new();
    for (plan_sha256, approval_sha256, approved_json) in candidates {
        let approved: lorepia_orchestration::ApprovedModuleActivationPlan =
            decode_document("applied module activation", &approved_json)?;
        approved.verify().map_err(|error| {
            storage_corrupted(format!(
                "stored applied module activation is invalid: {error}"
            ))
        })?;
        if approved.plan.plan_sha256.as_str() != plan_sha256
            || approved.approval_sha256.as_str() != approval_sha256
        {
            return Err(storage_corrupted(
                "stored applied module activation identity diverges",
            ));
        }
        match lorepia_orchestration::materialize_approved_module_runtime_plan(
            &approved,
            current_review,
        ) {
            Ok(runtime) => applicable.push(runtime),
            Err(
                lorepia_orchestration::ModuleMergeError::RuntimeDerivationChanged
                | lorepia_orchestration::ModuleMergeError::InvalidRuntimeMaterialization(_),
            ) => {}
            Err(error) => {
                return Err(CoreError::invalid(format!(
                    "cannot materialize applied module runtime plan: {error}"
                )));
            }
        }
    }
    let runtime = match applicable.as_slice() {
        [] => return Err(not_found("applied module runtime plan for context")),
        [runtime] => runtime.clone(),
        _ => {
            return Err(storage_corrupted(
                "multiple applied module activations select the same runtime context",
            ));
        }
    };
    transaction.commit().map_err(storage_db_error)?;
    Ok(runtime)
}

fn derive_applied_module_runtime_plan(
    storage: &Storage,
    source: &lorepia_orchestration::AppliedModuleRuntimePlan,
    target_review: &lorepia_orchestration::ModuleMergeReview,
) -> CoreResult<lorepia_orchestration::AppliedModuleRuntimePlan> {
    source.verify().map_err(|error| {
        CoreError::invalid(format!("invalid source applied module plan: {error}"))
    })?;
    target_review.verify().map_err(|error| {
        CoreError::invalid(format!("invalid target module runtime review: {error}"))
    })?;
    let verified_authorities =
        verify_module_import_authorities(storage, &target_review.ordered_bindings)?;
    let mut connection = storage.connection()?;
    let transaction = connection.transaction().map_err(storage_db_error)?;
    let stored_source =
        load_applied_module_runtime_plan_transaction(&transaction, &source.applied_plan_sha256)?;
    if &stored_source != source {
        return Err(CoreError::invalid(
            "source applied module runtime plan differs from durable authority",
        ));
    }
    validate_fresh_module_merge_review(
        storage,
        &transaction,
        target_review,
        &verified_authorities,
    )?;
    let derived = lorepia_orchestration::derive_applied_module_runtime_plan(source, target_review)
        .map_err(|error| {
            CoreError::invalid(format!("cannot derive module runtime plan: {error}"))
        })?;
    transaction.commit().map_err(storage_db_error)?;
    Ok(derived)
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

pub(crate) fn persist_applied_module_runtime_plan_transaction(
    transaction: &Transaction<'_>,
    runtime: &lorepia_orchestration::AppliedModuleRuntimePlan,
    created_at: DateTime<Utc>,
) -> CoreResult<()> {
    runtime.verify().map_err(|error| {
        CoreError::invalid(format!("invalid applied module runtime plan: {error}"))
    })?;
    verify_exact_applied_runtime_source(transaction, &runtime.source_approval)?;
    if let Some(parent) = runtime.derived_from_plan_sha256.as_ref() {
        let parent_valid = transaction
            .query_row(
                "SELECT EXISTS(
                     SELECT 1 FROM applied_module_runtime_plans
                     WHERE applied_plan_sha256 = ?1 AND state = 'applied'
                 )",
                [parent.as_str()],
                |row| row.get::<_, bool>(0),
            )
            .map_err(storage_db_error)?;
        if !parent_valid {
            return Err(CoreError::invalid(
                "derived runtime plan parent is not currently applied",
            ));
        }
    }
    let conversation_id = runtime.review.context.conversation_id.as_deref();
    let branch_id = runtime.review.context.branch_id.as_deref();
    if conversation_id.is_some() != branch_id.is_some() {
        return Err(CoreError::invalid(
            "applied runtime plan conversation and branch context are incomplete",
        ));
    }
    let context_json = serde_json::to_string(&runtime.review.context).map_err(|error| {
        CoreError::internal(format!("cannot encode module runtime context: {error}"))
    })?;
    let runtime_json = serde_json::to_string(runtime).map_err(|error| {
        CoreError::internal(format!(
            "cannot encode applied module runtime plan: {error}"
        ))
    })?;
    validate_json_bounds("module runtime context", &context_json)?;
    validate_json_bounds("applied module runtime plan", &runtime_json)?;
    let inserted = transaction
        .execute(
            "INSERT OR IGNORE INTO applied_module_runtime_plans
             (applied_plan_sha256, source_activation_plan_sha256,
              source_approval_sha256, derived_from_plan_sha256,
              conversation_id, branch_id, review_sha256, context_json,
              runtime_plan_json, state, created_at, stale_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9,
                     'applied', ?10, NULL)",
            params![
                runtime.applied_plan_sha256.as_str(),
                runtime.source_approval.plan.plan_sha256.as_str(),
                runtime.source_approval.approval_sha256.as_str(),
                runtime
                    .derived_from_plan_sha256
                    .as_ref()
                    .map(lorepia_domain::Sha256Digest::as_str),
                conversation_id,
                branch_id,
                runtime.review.review_sha256.as_str(),
                context_json,
                runtime_json,
                created_at.to_rfc3339(),
            ],
        )
        .map_err(storage_db_error)?
        == 1;
    if !inserted {
        let stored = load_applied_module_runtime_plan_transaction(
            transaction,
            &runtime.applied_plan_sha256,
        )?;
        if &stored != runtime {
            return Err(storage_corrupted(
                "applied module runtime hash was reused with different material",
            ));
        }
    }
    Ok(())
}

fn load_applied_module_runtime_plan_transaction(
    transaction: &Transaction<'_>,
    applied_plan_sha256: &lorepia_domain::Sha256Digest,
) -> CoreResult<lorepia_orchestration::AppliedModuleRuntimePlan> {
    load_applied_module_runtime_plan_with_stale_authority(transaction, applied_plan_sha256, false)
}

fn load_historical_applied_module_runtime_plan_transaction(
    transaction: &Transaction<'_>,
    applied_plan_sha256: &lorepia_domain::Sha256Digest,
) -> CoreResult<lorepia_orchestration::AppliedModuleRuntimePlan> {
    load_applied_module_runtime_plan_with_stale_authority(transaction, applied_plan_sha256, true)
}

fn load_applied_module_runtime_plan_with_stale_authority(
    transaction: &Transaction<'_>,
    applied_plan_sha256: &lorepia_domain::Sha256Digest,
    allow_stale: bool,
) -> CoreResult<lorepia_orchestration::AppliedModuleRuntimePlan> {
    let row = transaction
        .query_row(
            "SELECT source_activation_plan_sha256, source_approval_sha256,
                    derived_from_plan_sha256, conversation_id, branch_id,
                    review_sha256, context_json, runtime_plan_json, state
             FROM applied_module_runtime_plans
             WHERE applied_plan_sha256 = ?1",
            [applied_plan_sha256.as_str()],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, Option<String>>(2)?,
                    row.get::<_, Option<String>>(3)?,
                    row.get::<_, Option<String>>(4)?,
                    row.get::<_, String>(5)?,
                    row.get::<_, String>(6)?,
                    row.get::<_, String>(7)?,
                    row.get::<_, String>(8)?,
                ))
            },
        )
        .optional()
        .map_err(storage_db_error)?
        .ok_or_else(|| not_found("applied module runtime plan"))?;
    if row.8 != "applied" && !(allow_stale && row.8 == "stale") {
        return Err(CoreError::invalid("applied module runtime plan is stale"));
    }
    let runtime: lorepia_orchestration::AppliedModuleRuntimePlan =
        decode_document("applied module runtime plan", &row.7)?;
    let context: lorepia_orchestration::ModuleResolutionContext =
        decode_document("applied module runtime context", &row.6)?;
    runtime.verify().map_err(|error| {
        storage_corrupted(format!(
            "stored applied module runtime plan is invalid: {error}"
        ))
    })?;
    let canonical_runtime_json = serde_json::to_string(&runtime).map_err(|error| {
        CoreError::internal(format!(
            "cannot re-encode applied module runtime plan: {error}"
        ))
    })?;
    let canonical_context_json = serde_json::to_string(&context).map_err(|error| {
        CoreError::internal(format!(
            "cannot re-encode applied module runtime context: {error}"
        ))
    })?;
    if &runtime.applied_plan_sha256 != applied_plan_sha256
        || runtime.source_approval.plan.plan_sha256.as_str() != row.0
        || runtime.source_approval.approval_sha256.as_str() != row.1
        || runtime
            .derived_from_plan_sha256
            .as_ref()
            .map(lorepia_domain::Sha256Digest::as_str)
            != row.2.as_deref()
        || runtime.review.context != context
        || runtime.review.context.conversation_id.as_deref() != row.3.as_deref()
        || runtime.review.context.branch_id.as_deref() != row.4.as_deref()
        || runtime.review.review_sha256.as_str() != row.5
        || canonical_context_json != row.6
        || canonical_runtime_json != row.7
    {
        return Err(storage_corrupted(
            "applied module runtime plan authority columns diverge from its canonical payload",
        ));
    }
    verify_exact_applied_runtime_source_with_stale_authority(
        transaction,
        &runtime.source_approval,
        allow_stale,
    )?;
    if let Some(parent) = row.2.as_deref() {
        let parent_is_applied = transaction
            .query_row(
                "SELECT EXISTS(
                     SELECT 1 FROM applied_module_runtime_plans
                     WHERE applied_plan_sha256 = ?1
                       AND (state = 'applied' OR (?2 AND state = 'stale'))
                 )",
                params![parent, allow_stale],
                |row| row.get::<_, bool>(0),
            )
            .map_err(storage_db_error)?;
        if !parent_is_applied {
            return Err(CoreError::invalid("derived module runtime parent is stale"));
        }
    }
    Ok(runtime)
}

fn soft_delete_module_binding(
    storage: &Storage,
    id: &ModuleBindingId,
    expected_revision: u64,
) -> CoreResult<StoredRevision<ModuleBinding>> {
    let mut connection = storage.connection()?;
    let transaction = connection
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(storage_db_error)?;
    let row = transaction
        .query_row(
            "SELECT document_json, revision, created_at, updated_at, deleted_at
             FROM content_module_bindings WHERE id = ?1",
            [id.as_str()],
            module_binding_row,
        )
        .optional()
        .map_err(storage_db_error)?
        .ok_or_else(|| not_found("module binding"))?;
    let current = decode_stored_document::<ModuleBinding>("module binding", row)?;
    if current.deleted_at.is_some() || current.revision != expected_revision {
        return Err(revision_conflict(
            "module binding",
            id.as_str(),
            Some(expected_revision),
            Some(current.revision),
        ));
    }
    let next_revision = expected_revision
        .checked_add(1)
        .ok_or_else(|| CoreError::internal("module binding revision overflow"))?;
    let now = Utc::now();
    let now_text = now.to_rfc3339();
    stale_affected_module_activation_plans(
        &transaction,
        Some(&current.value),
        None,
        "binding_deleted",
        None,
        &now_text,
    )?;
    let changed = transaction
        .execute(
            "UPDATE content_module_bindings
             SET revision = ?2, enabled = 0, updated_at = ?3, deleted_at = ?3
             WHERE id = ?1 AND revision = ?4 AND deleted_at IS NULL",
            params![
                id.as_str(),
                i64_revision(next_revision)?,
                now_text,
                i64_revision(expected_revision)?,
            ],
        )
        .map_err(storage_db_error)?;
    if changed != 1 {
        return Err(revision_conflict(
            "module binding",
            id.as_str(),
            Some(expected_revision),
            None,
        ));
    }
    transaction.commit().map_err(storage_db_error)?;
    Ok(StoredRevision {
        value: current.value,
        revision: next_revision,
        revision_id: None,
        created_at: current.created_at,
        updated_at: now,
        deleted_at: Some(now),
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

fn character_content_object_id(character_id: &str) -> String {
    format!("character-content:{character_id}")
}

fn character_content_metadata_json(
    content: &CharacterContentV1,
    inspection_plan_sha256: Option<&str>,
) -> CoreResult<String> {
    // Asset descriptors are already projected into `asset_descriptors` and
    // `asset_links`, while the immutable canonical document retains the full
    // list. Keep this bounded metadata projection compact instead of storing a
    // third copy that grows linearly with archive size.
    let knowledge_book = content.knowledge_book.as_ref().map(|book| {
        serde_json::json!({
            "id": book.id,
            "name": book.name,
            "source_sha256": book.source_sha256,
        })
    });
    serde_json::to_string(&serde_json::json!({
        "schema_version": content.schema_version,
        "knowledge_book": knowledge_book,
        "asset_count": content.assets.len(),
        "inspection_plan_sha256": inspection_plan_sha256,
    }))
    .map_err(|error| CoreError::invalid(format!("cannot encode character metadata: {error}")))
}

fn write_character_content_header(
    transaction: &Transaction<'_>,
    object_id: &str,
    character_id: &str,
    revision_id: &str,
    content: &CharacterContentV1,
    document_json: &str,
    inspection_plan_sha256: Option<&str>,
) -> CoreResult<()> {
    transaction
        .execute(
            "INSERT OR IGNORE INTO character_content (object_id, character_id)
             VALUES (?1, ?2)",
            params![object_id, character_id],
        )
        .map_err(storage_db_error)?;
    let unknown_extensions_json =
        serde_json::to_string(&content.unknown_extensions).map_err(|error| {
            CoreError::invalid(format!("cannot encode character extensions: {error}"))
        })?;
    let metadata_json = character_content_metadata_json(content, inspection_plan_sha256)?;
    transaction
        .execute(
            "INSERT INTO character_content_revisions
             (revision_id, object_id, personality, scenario, first_message,
              system_instruction, post_history_instruction, creator_notes,
              unknown_extensions_json, metadata_json, payload_json)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, '', ?8, ?9, ?10)",
            params![
                revision_id,
                object_id,
                content.personality,
                content.scenario,
                content.first_message,
                content.system_instruction,
                content.post_history_instruction,
                unknown_extensions_json,
                metadata_json,
                document_json,
            ],
        )
        .map_err(storage_db_error)?;
    Ok(())
}

fn write_character_greetings(
    transaction: &Transaction<'_>,
    revision_id: &str,
    content: &CharacterContentV1,
) -> CoreResult<()> {
    let mut greeting_ordinal = 0_i64;
    if !content.first_message.is_empty() {
        transaction
            .execute(
                "INSERT INTO character_greetings
                 (character_content_revision_id, ordinal, greeting_id, kind,
                  content, enabled, payload_json)
                 VALUES (?1, ?2, 'default', 'default', ?3, 1, ?4)",
                params![
                    revision_id,
                    greeting_ordinal,
                    content.first_message,
                    serde_json::json!({"content": content.first_message}).to_string(),
                ],
            )
            .map_err(storage_db_error)?;
        greeting_ordinal += 1;
    }
    for (index, greeting) in content.alternate_greetings.iter().enumerate() {
        if greeting.is_empty() {
            continue;
        }
        transaction
            .execute(
                "INSERT INTO character_greetings
                 (character_content_revision_id, ordinal, greeting_id, kind,
                  content, enabled, payload_json)
                 VALUES (?1, ?2, ?3, 'alternate', ?4, 1, ?5)",
                params![
                    revision_id,
                    greeting_ordinal,
                    format!("alternate-{index}"),
                    greeting,
                    serde_json::json!({"content": greeting}).to_string(),
                ],
            )
            .map_err(storage_db_error)?;
        greeting_ordinal += 1;
    }
    Ok(())
}

fn write_character_dialogue_examples(
    transaction: &Transaction<'_>,
    revision_id: &str,
    content: &CharacterContentV1,
) -> CoreResult<()> {
    for (index, example) in content.example_dialogs.iter().enumerate() {
        if example.is_empty() {
            continue;
        }
        let example_id = format!("example-{index}");
        transaction
            .execute(
                "INSERT INTO character_dialogue_examples
                 (character_content_revision_id, example_id, ordinal, name, payload_json)
                 VALUES (?1, ?2, ?3, '', ?4)",
                params![
                    revision_id,
                    example_id,
                    i64::try_from(index)
                        .map_err(|_| CoreError::invalid("too many dialogue examples"))?,
                    serde_json::json!({"content": example}).to_string(),
                ],
            )
            .map_err(storage_db_error)?;
        transaction
            .execute(
                "INSERT INTO character_dialogue_example_messages
                 (character_content_revision_id, example_id, ordinal, role, content)
                 VALUES (?1, ?2, 0, 'assistant', ?3)",
                params![revision_id, example_id, example],
            )
            .map_err(storage_db_error)?;
    }
    Ok(())
}

fn write_character_assets(
    transaction: &Transaction<'_>,
    revision_id: &str,
    content: &CharacterContentV1,
) -> CoreResult<()> {
    for (ordinal, descriptor) in content.assets.iter().enumerate() {
        write_asset_descriptor(transaction, descriptor, Some(revision_id))?;
        transaction
            .execute(
                "INSERT INTO asset_links
                 (owner_revision_id, asset_descriptor_id, role, ordinal, payload_json)
                 VALUES (?1, ?2, ?3, ?4, '{}')",
                params![
                    revision_id,
                    descriptor.id.as_str(),
                    asset_role_str(descriptor.role),
                    i64::try_from(ordinal)
                        .map_err(|_| CoreError::invalid("too many character assets"))?,
                ],
            )
            .map_err(storage_db_error)?;
    }
    Ok(())
}

fn write_character_content_projection(
    transaction: &Transaction<'_>,
    object_id: &str,
    character_id: &str,
    revision_id: &str,
    content: &CharacterContentV1,
    document_json: &str,
    inspection_plan_sha256: Option<&str>,
) -> CoreResult<()> {
    write_character_content_header(
        transaction,
        object_id,
        character_id,
        revision_id,
        content,
        document_json,
        inspection_plan_sha256,
    )?;
    write_character_greetings(transaction, revision_id, content)?;
    write_character_dialogue_examples(transaction, revision_id, content)?;
    write_character_assets(transaction, revision_id, content)
}

pub(crate) fn write_imported_character_content(
    transaction: &Transaction<'_>,
    character_id: &str,
    source_hash: &str,
    content: &CharacterContentV1,
    inspection_plan_sha256: &str,
) -> CoreResult<()> {
    validate_optional_sha256("character source hash", Some(source_hash))?;
    validate_optional_sha256(
        "character inspection plan hash",
        Some(inspection_plan_sha256),
    )?;
    if content
        .unknown_extensions
        .raw_source_sha256
        .as_ref()
        .is_some_and(|digest| digest.as_str() != source_hash)
    {
        return Err(CoreError::invalid(
            "character extension index is not bound to the imported source",
        ));
    }
    let object_id = character_content_object_id(character_id);
    let provenance = Provenance {
        source_kind: SourceKind::ImportedStandard,
        source_id: Some(character_id.to_owned()),
        source_hash: Some(source_hash.to_owned()),
        author: None,
        license: None,
        imported_at: Some(Utc::now()),
    };
    if let Some(embedded) = content
        .knowledge_book
        .as_ref()
        .and_then(|reference| reference.embedded.as_ref())
    {
        let book = embedded.materialize(provenance.clone());
        // An empty inline reference is only a future link. Creating an empty
        // native object here would claim that ID and prevent the creator from
        // supplying its actual contents later.
        if !book.entries.is_empty() {
            ensure_imported_character_knowledge_book(transaction, &book)?;
        }
    }
    if let Some(transform_set) = content.runtime.materialize_transform_set(Provenance {
        // The immutable imported profile remains attached to the character.
        // This executable set is a native projection generated from that
        // profile, so it is not an unreviewed imported transform document.
        source_kind: SourceKind::Generated,
        source_id: Some(format!("character-import:{character_id}")),
        ..provenance.clone()
    }) {
        ensure_imported_character_transform_set(transaction, &transform_set)?;
    }
    let written = append_content_revision(
        transaction,
        DocumentTable::CharacterContent,
        &object_id,
        content.schema_version,
        content,
        &provenance,
        None,
        RevisionEventKind::Create,
    )?;
    let (document_json, _) = encode_document("character content", content)?;
    write_character_content_projection(
        transaction,
        &object_id,
        character_id,
        &written.revision_id,
        content,
        &document_json,
        Some(inspection_plan_sha256),
    )
}

fn ensure_imported_character_knowledge_book(
    transaction: &Transaction<'_>,
    book: &KnowledgeBook,
) -> CoreResult<()> {
    book.validate()
        .map_err(|error| CoreError::invalid(format!("knowledge book is invalid: {error}")))?;
    let existing = transaction
        .query_row(
            "SELECT revision.document_json
             FROM content_objects AS object
             JOIN content_object_state AS state ON state.object_id = object.id
             JOIN content_revisions AS revision
               ON revision.id = state.active_revision_id
              AND revision.object_id = object.id
             WHERE object.id = ?1
               AND object.object_kind = 'knowledge_book'
               AND object.deleted_at IS NULL",
            [book.id.as_str()],
            |row| row.get::<_, String>(0),
        )
        .optional()
        .map_err(storage_db_error)?;
    if let Some(document_json) = existing {
        let stored: KnowledgeBook = decode_document("knowledge book", &document_json)?;
        if stored == *book {
            return Ok(());
        }
        return Err(CoreError::invalid(format!(
            "embedded knowledge book {} conflicts with existing content",
            book.id.as_str()
        )));
    }
    let written = append_content_revision(
        transaction,
        DocumentTable::KnowledgeBooks,
        book.id.as_str(),
        book.schema_version,
        book,
        &book.provenance,
        None,
        RevisionEventKind::Import,
    )?;
    let (document_json, _) = encode_document("knowledge book", book)?;
    write_knowledge_book_projection(
        transaction,
        &written.revision_id,
        book,
        &document_json,
        None,
    )
}

fn ensure_imported_character_transform_set(
    transaction: &Transaction<'_>,
    transform_set: &TransformSet,
) -> CoreResult<()> {
    transform_set.validate().map_err(|error| {
        CoreError::invalid(format!("character transform set is invalid: {error}"))
    })?;
    let existing = transaction
        .query_row(
            "SELECT revision.document_json
             FROM content_objects AS object
             JOIN content_object_state AS state ON state.object_id = object.id
             JOIN content_revisions AS revision
               ON revision.id = state.active_revision_id
              AND revision.object_id = object.id
             WHERE object.id = ?1
               AND object.object_kind = 'transform_set'
               AND object.deleted_at IS NULL",
            [transform_set.id.as_str()],
            |row| row.get::<_, String>(0),
        )
        .optional()
        .map_err(storage_db_error)?;
    if let Some(document_json) = existing {
        let stored: TransformSet = decode_document("transform set", &document_json)?;
        if stored == *transform_set {
            return Ok(());
        }
        return Err(CoreError::invalid(format!(
            "character transform set {} conflicts with existing content",
            transform_set.id.as_str()
        )));
    }
    let written = append_content_revision(
        transaction,
        DocumentTable::TransformSets,
        transform_set.id.as_str(),
        transform_set.schema_version,
        transform_set,
        &transform_set.provenance,
        None,
        RevisionEventKind::Import,
    )?;
    let (document_json, _) = encode_document("transform set", transform_set)?;
    write_transform_set_projection(
        transaction,
        &written.revision_id,
        transform_set,
        &document_json,
        None,
    )
}

fn write_asset_descriptor(
    transaction: &Transaction<'_>,
    descriptor: &AssetDescriptor,
    source_revision_id: Option<&str>,
) -> CoreResult<()> {
    let (payload_json, _) = encode_document("asset descriptor", descriptor)?;
    let existing = transaction
        .query_row(
            "SELECT asset_hash, payload_json
             FROM asset_descriptors WHERE id = ?1",
            [descriptor.id.as_str()],
            |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
        )
        .optional()
        .map_err(storage_db_error)?;
    if let Some((asset_hash, stored_payload)) = existing {
        if asset_hash != descriptor.sha256.as_str() || stored_payload != payload_json {
            return Err(CoreError::invalid(format!(
                "asset descriptor {} conflicts with an existing immutable descriptor",
                descriptor.id.as_str()
            )));
        }
        return Ok(());
    }
    transaction
        .execute(
            "INSERT INTO asset_descriptors
             (id, asset_hash, name, role, media_type, size_bytes, width, height,
              duration_ms, risk_class, source_revision_id, source_kind,
              source_hash, logical_path, payload_json, created_at)
             VALUES (
                 ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, 'normal', ?10, ?11,
                 ?12, ?13, ?14, ?15
             )",
            params![
                descriptor.id.as_str(),
                descriptor.sha256.as_str(),
                descriptor.name,
                asset_role_str(descriptor.role),
                descriptor.media_type,
                descriptor.size_bytes,
                descriptor.width,
                descriptor.height,
                descriptor.duration_ms,
                source_revision_id,
                asset_source_kind_str(descriptor.source.kind),
                descriptor
                    .source
                    .source_sha256
                    .as_ref()
                    .map(lorepia_domain::Sha256Digest::as_str),
                descriptor.source.logical_path,
                payload_json,
                Utc::now().to_rfc3339(),
            ],
        )
        .map_err(storage_db_error)?;
    Ok(())
}

fn validate_package_provenance(
    label: &str,
    provenance: &Provenance,
    source_sha256: &str,
) -> CoreResult<()> {
    validate_optional_sha256("package source hash", Some(source_sha256))?;
    if provenance.source_kind != SourceKind::ImportedPackage
        || provenance.source_hash.as_deref() != Some(source_sha256)
    {
        return Err(CoreError::invalid(format!(
            "{label} provenance must identify the exact imported package source"
        )));
    }
    Ok(())
}

struct ImportedPackageDocument<'a, T> {
    label: &'static str,
    table: DocumentTable,
    object_id: &'a str,
    schema_version: u32,
    document: &'a T,
    provenance: &'a Provenance,
}

fn append_imported_package_document<T, F>(
    transaction: &Transaction<'_>,
    input: ImportedPackageDocument<'_, T>,
    expected_revision: Option<u64>,
    source_sha256: &str,
    write_projection: F,
) -> CoreResult<PackageDocumentWrite>
where
    T: Serialize + DeserializeOwned,
    F: FnOnce(&Transaction<'_>, &str, &T, &str, Option<u64>) -> CoreResult<()>,
{
    validate_package_provenance(input.label, input.provenance, source_sha256)?;
    let written = append_content_revision(
        transaction,
        input.table,
        input.object_id,
        input.schema_version,
        input.document,
        input.provenance,
        expected_revision,
        RevisionEventKind::Import,
    )?;
    let (document_json, _) = encode_document(input.label, input.document)?;
    write_projection(
        transaction,
        &written.revision_id,
        input.document,
        &document_json,
        expected_revision,
    )?;
    Ok(PackageDocumentWrite {
        object_id: input.object_id.to_owned(),
        revision_id: written.revision_id,
        state_revision: written.state_version,
    })
}

fn append_imported_prompt_preset(
    transaction: &Transaction<'_>,
    preset: &PromptPreset,
    expected_revision: Option<u64>,
    source_sha256: &str,
) -> CoreResult<PackageDocumentWrite> {
    append_imported_package_document(
        transaction,
        ImportedPackageDocument {
            label: "prompt preset",
            table: DocumentTable::PromptPresets,
            object_id: preset.id.as_str(),
            schema_version: preset.schema_version,
            document: preset,
            provenance: &preset.metadata.provenance,
        },
        expected_revision,
        source_sha256,
        write_prompt_preset_projection,
    )
}

fn append_imported_knowledge_book(
    transaction: &Transaction<'_>,
    book: &KnowledgeBook,
    expected_revision: Option<u64>,
    source_sha256: &str,
) -> CoreResult<PackageDocumentWrite> {
    append_imported_package_document(
        transaction,
        ImportedPackageDocument {
            label: "knowledge book",
            table: DocumentTable::KnowledgeBooks,
            object_id: book.id.as_str(),
            schema_version: book.schema_version,
            document: book,
            provenance: &book.provenance,
        },
        expected_revision,
        source_sha256,
        write_knowledge_book_projection,
    )
}

fn append_imported_memory_profile(
    transaction: &Transaction<'_>,
    profile: &MemoryProfile,
    expected_revision: Option<u64>,
    source_sha256: &str,
) -> CoreResult<PackageDocumentWrite> {
    append_imported_package_document(
        transaction,
        ImportedPackageDocument {
            label: "memory profile",
            table: DocumentTable::MemoryProfiles,
            object_id: profile.id.as_str(),
            schema_version: profile.schema_version,
            document: profile,
            provenance: &profile.provenance,
        },
        expected_revision,
        source_sha256,
        write_memory_profile_projection,
    )
}

fn append_imported_transform_set(
    transaction: &Transaction<'_>,
    transform_set: &TransformSet,
    expected_revision: Option<u64>,
    source_sha256: &str,
) -> CoreResult<PackageDocumentWrite> {
    append_imported_package_document(
        transaction,
        ImportedPackageDocument {
            label: "transform set",
            table: DocumentTable::TransformSets,
            object_id: transform_set.id.as_str(),
            schema_version: transform_set.schema_version,
            document: transform_set,
            provenance: &transform_set.provenance,
        },
        expected_revision,
        source_sha256,
        write_transform_set_projection,
    )
}

fn append_imported_interaction_rule_set(
    transaction: &Transaction<'_>,
    rule_set: &InteractionRuleSet,
    expected_revision: Option<u64>,
    source_sha256: &str,
) -> CoreResult<PackageDocumentWrite> {
    append_imported_package_document(
        transaction,
        ImportedPackageDocument {
            label: "interaction rule set",
            table: DocumentTable::InteractionRuleSets,
            object_id: rule_set.id.as_str(),
            schema_version: rule_set.schema_version,
            document: rule_set,
            provenance: &rule_set.provenance,
        },
        expected_revision,
        source_sha256,
        write_interaction_rule_set_projection,
    )
}

fn append_imported_content_module(
    transaction: &Transaction<'_>,
    module: &ContentModule,
    expected_revision: Option<u64>,
    source_sha256: &str,
) -> CoreResult<PackageDocumentWrite> {
    append_imported_package_document(
        transaction,
        ImportedPackageDocument {
            label: "content module",
            table: DocumentTable::ContentModules,
            object_id: module.id.as_str(),
            schema_version: module.schema_version,
            document: module,
            provenance: &module.metadata.provenance,
        },
        expected_revision,
        source_sha256,
        write_content_module_projection,
    )
}

fn append_imported_character_package_content(
    transaction: &Transaction<'_>,
    character_id: &str,
    content: &CharacterContentV1,
    expected_revision: Option<u64>,
    source_sha256: &str,
) -> CoreResult<PackageDocumentWrite> {
    validate_identifier("character", character_id)?;
    if content
        .unknown_extensions
        .raw_source_sha256
        .as_ref()
        .is_some_and(|digest| digest.as_str() != source_sha256)
    {
        return Err(CoreError::invalid(
            "character extension index is not bound to the imported package source",
        ));
    }
    for asset in &content.assets {
        validate_package_asset_source(asset, source_sha256)?;
    }
    let object_id = character_content_object_id(character_id);
    let provenance = Provenance {
        source_kind: SourceKind::ImportedPackage,
        source_id: Some(character_id.to_owned()),
        source_hash: Some(source_sha256.to_owned()),
        author: None,
        license: None,
        imported_at: Some(Utc::now()),
    };
    let written = append_content_revision(
        transaction,
        DocumentTable::CharacterContent,
        &object_id,
        content.schema_version,
        content,
        &provenance,
        expected_revision,
        RevisionEventKind::Import,
    )?;
    let (document_json, _) = encode_document("character content", content)?;
    write_character_content_projection(
        transaction,
        &object_id,
        character_id,
        &written.revision_id,
        content,
        &document_json,
        None,
    )?;
    Ok(PackageDocumentWrite {
        object_id,
        revision_id: written.revision_id,
        state_revision: written.state_version,
    })
}

/// Appends one approved package document and all of its normalized
/// projections inside the caller-owned transaction.
///
/// Keeping this helper in the orchestration repository prevents the package
/// state machine from bypassing typed validation or creating a content
/// revision without its corresponding query projection.
pub(crate) fn append_package_commit_document(
    transaction: &Transaction<'_>,
    document: &PackageCommitDocument,
    expected_revision: Option<u64>,
    source_sha256: &str,
) -> CoreResult<PackageDocumentWrite> {
    match document {
        PackageCommitDocument::PromptPreset(preset) => {
            append_imported_prompt_preset(transaction, preset, expected_revision, source_sha256)
        }
        PackageCommitDocument::KnowledgeBook(book) => {
            append_imported_knowledge_book(transaction, book, expected_revision, source_sha256)
        }
        PackageCommitDocument::MemoryProfile(profile) => {
            append_imported_memory_profile(transaction, profile, expected_revision, source_sha256)
        }
        PackageCommitDocument::TransformSet(transform_set) => append_imported_transform_set(
            transaction,
            transform_set,
            expected_revision,
            source_sha256,
        ),
        PackageCommitDocument::InteractionRuleSet(rule_set) => {
            append_imported_interaction_rule_set(
                transaction,
                rule_set,
                expected_revision,
                source_sha256,
            )
        }
        PackageCommitDocument::ContentModule(module) => {
            append_imported_content_module(transaction, module, expected_revision, source_sha256)
        }
        PackageCommitDocument::CharacterContent {
            character_id,
            content,
        } => append_imported_character_package_content(
            transaction,
            character_id,
            content,
            expected_revision,
            source_sha256,
        ),
    }
}

fn validate_package_asset_source(
    descriptor: &AssetDescriptor,
    source_sha256: &str,
) -> CoreResult<()> {
    validate_optional_sha256("package source hash", Some(source_sha256))?;
    if descriptor.source.kind != lorepia_domain::AssetSourceKind::LorepiaPackage
        || descriptor
            .source
            .source_sha256
            .as_ref()
            .map(lorepia_domain::Sha256Digest::as_str)
            != Some(source_sha256)
    {
        return Err(CoreError::invalid(format!(
            "asset descriptor {} is not bound to the exact imported package source",
            descriptor.id.as_str()
        )));
    }
    Ok(())
}

/// Appends a package asset descriptor without allowing the package state
/// machine to bypass immutable-descriptor conflict checks.
pub(crate) fn append_package_asset_descriptor(
    transaction: &Transaction<'_>,
    descriptor: &AssetDescriptor,
    source_sha256: &str,
) -> CoreResult<()> {
    validate_package_asset_source(descriptor, source_sha256)?;
    write_asset_descriptor(transaction, descriptor, None)
}

const fn asset_role_str(role: AssetRole) -> &'static str {
    match role {
        AssetRole::Avatar => "avatar",
        AssetRole::Icon => "icon",
        AssetRole::Background => "background",
        AssetRole::UserIcon => "user_icon",
        AssetRole::Emotion => "emotion",
        AssetRole::Expression => "expression",
        AssetRole::Illustration => "illustration",
        AssetRole::Audio => "audio",
        AssetRole::Voice => "voice",
        AssetRole::Video => "video",
        AssetRole::StatusPanel => "status_panel",
        AssetRole::Attachment => "attachment",
        AssetRole::Other => "other",
    }
}

const fn asset_source_kind_str(kind: lorepia_domain::AssetSourceKind) -> &'static str {
    match kind {
        lorepia_domain::AssetSourceKind::CharacterCard => "character_card",
        lorepia_domain::AssetSourceKind::CharxPackage => "charx_package",
        lorepia_domain::AssetSourceKind::LorepiaPackage => "lorepia_package",
        lorepia_domain::AssetSourceKind::ContentModule => "content_module",
        lorepia_domain::AssetSourceKind::UserSelected => "user_selected",
        lorepia_domain::AssetSourceKind::Generated => "generated",
        lorepia_domain::AssetSourceKind::Unknown => "unknown",
    }
}
