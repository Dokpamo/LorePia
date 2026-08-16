use std::collections::{BTreeMap, BTreeSet};

use chrono::{DateTime, Utc};
use lorepia_domain::{
    CapabilityKey, Character, CharacterContentV1, ConversationBranchId, ConversationId,
    ConversationMode, ConversationPersonaSelection, CoreError, CoreErrorCode, CoreResult,
    GenerationId, GenerationReasoningEffort, GenerationTarget, InteractionEffect, InteractionEvent,
    InteractionState, KnowledgeBook, KnowledgeEntryId, MessageId, PromptPreset, PromptPresetId,
    ProviderConnectionId, Sha256Digest, ValidateOrchestration, VariableMap, VersionedJson,
    prompt_local_user_id_sha256,
};
use lorepia_orchestration::{
    AppliedModuleRuntimePlan, ModuleMergeReview, no_applied_module_runtime_plan_sha256,
};
use rusqlite::{Connection, OptionalExtension, Transaction, TransactionBehavior, params};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::database::{Storage, storage_db_error};
use crate::interaction_repository::{
    InteractionActionResultWrite, InteractionDerivedEventWrite, InteractionKnowledgeBinding,
    InteractionPolicySnapshot, InteractionProposalWrite, interaction_policy_sha256,
};
use crate::orchestration::{PromptPresetBinding, PromptResponseLength, StoredRevision};
use crate::provider_credential_repository::{
    ProviderCredentialAccessAuthority, provider_credential_ownership_authority_is_valid,
};

const GENERATION_ATTEMPT_SCHEMA_VERSION: u32 = 1;
const MAX_ATTEMPT_FAILURE_CODE_BYTES: usize = 128;
const MAX_ATTEMPT_EVIDENCE_BYTES: usize = 1_024 * 1_024;
const MAX_RETRYABLE_ATTEMPT_LIST_LIMIT: u32 = 100;

/// One exact knowledge entry/revision consulted by an interaction evaluation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InteractionEvaluationKnowledgeRevision {
    pub entry_id: KnowledgeEntryId,
    pub book_revision_id: String,
}

/// One deterministic asset-action failure or acceptance diagnostic.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InteractionEvaluationAssetDiagnostic {
    pub rule_id: String,
    pub action_ordinal: u32,
    pub diagnostic: VersionedJson,
}

/// Concrete built-in template values; evaluation never re-reads the clock or
/// mutable character/persona state after this value has been sealed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InteractionEvaluationTemplateValues {
    pub character_name: Option<String>,
    pub user_name: Option<String>,
    pub persona_name: Option<String>,
    pub persona_description: Option<String>,
    pub current_date: Option<String>,
    pub current_time: Option<String>,
}

/// Serializable copy of every hard interaction-engine limit consulted by an
/// evaluation. This remains independent of future process defaults.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InteractionEvaluationLimits {
    pub max_rule_sets: usize,
    pub max_rules: usize,
    pub max_actions_per_event: usize,
    pub max_actions_per_rule: usize,
    pub max_condition_depth: usize,
    pub max_condition_nodes: usize,
    pub max_template_depth: usize,
    pub max_template_parts: usize,
    pub max_variables: usize,
    pub max_proposals: usize,
    pub max_pending_proposals: usize,
    pub max_effects: usize,
    pub max_choices: usize,
    pub max_dice_count: u16,
    pub max_dice_sides: u32,
    pub max_text_chars: usize,
    pub max_identifier_bytes: usize,
}

/// Immutable complete context for one interaction evaluation.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InteractionEvaluationSeal {
    pub schema_version: u32,
    pub engine_contract_version: u32,
    pub policy_sha256: Sha256Digest,
    pub executable_rule_sets_sha256: Sha256Digest,
    pub knowledge_revisions: Vec<InteractionEvaluationKnowledgeRevision>,
    pub asset_action_diagnostics: Vec<InteractionEvaluationAssetDiagnostic>,
    pub approved_import_source_ids: Vec<String>,
    pub policy_variables: VariableMap,
    pub supported_capabilities: Vec<CapabilityKey>,
    pub template_values: InteractionEvaluationTemplateValues,
    pub event_epoch_seconds: i64,
    pub limits: InteractionEvaluationLimits,
    pub seed_contract_version: u32,
}

/// One event transition already evaluated inside an isolated generation
/// attempt. `parent_ordinal = None` identifies the root `BeforeGeneration` event.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GenerationAttemptDerivedTransition {
    pub ordinal: u32,
    pub parent_ordinal: Option<u32>,
    pub depth: u32,
    pub event_id: String,
    pub event: InteractionEvent,
    pub event_sha256: Sha256Digest,
    pub deterministic_seed: u64,
    pub expected_state_revision: u64,
    pub resulting_state_revision: u64,
    pub policy: InteractionPolicySnapshot,
    pub evaluation_seal: InteractionEvaluationSeal,
    pub next_state: InteractionState,
    pub knowledge: Vec<InteractionKnowledgeBinding>,
    pub action_results: Vec<InteractionActionResultWrite>,
    pub effects: Vec<InteractionEffect>,
    pub derived_events: Vec<InteractionDerivedEventWrite>,
    pub proposals: Vec<InteractionProposalWrite>,
    pub commit_sha256: Sha256Digest,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GenerationAttemptDerivedGuardKind {
    Cycle,
    DepthLimit,
    CountLimit,
}

/// Explicit audit for a derived candidate suppressed by a bounded-chain guard.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GenerationAttemptDerivedGuardAudit {
    pub kind: GenerationAttemptDerivedGuardKind,
    pub candidate_event_sha256: Option<Sha256Digest>,
    pub parent_ordinal: u32,
    pub depth: u32,
    pub suppressed_count: u32,
    pub evidence_sha256: Sha256Digest,
}

/// Complete deterministic derived-event closure staged before provider
/// dispatch. Storage materializes this sequence without enqueuing live outbox
/// work, then proves that the live derived queue is empty for the attempt.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GenerationAttemptDerivedClosure {
    pub schema_version: u32,
    pub transitions: Vec<GenerationAttemptDerivedTransition>,
    pub guard_audits: Vec<GenerationAttemptDerivedGuardAudit>,
    pub final_state: InteractionState,
    pub final_knowledge: Vec<InteractionKnowledgeBinding>,
    pub event_count: u32,
    pub guard_count: u32,
    pub chain_sha256: Sha256Digest,
}

/// Exact prompt-preset selection consulted before an attempt crosses an
/// approval pause. Resume and fork paths use this value without re-reading
/// mutable binding, persona-selection, or preset-head state.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GenerationPromptQuickSettingsAuthority {
    pub response_length: PromptResponseLength,
    pub creativity: u8,
    pub reasoning_effort: Option<GenerationReasoningEffort>,
    pub memory_enabled: bool,
    pub knowledge_enabled: bool,
    pub supports_temperature: bool,
    pub resolved_temperature: Option<f64>,
    pub resolved_max_output_tokens: Option<u32>,
}

/// Secret-free identity of the exact provider mapping resolved before an
/// attempt crosses an approval pause.
///
/// Core recomputes this authority from current provider rows immediately
/// before dispatch. Any mismatch fails closed under the original operation
/// identity; credentials are deliberately excluded.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum GenerationProviderTargetAuthority {
    ProviderProfile {
        provider_profile_id: String,
        dispatch_snapshot_sha256: Sha256Digest,
    },
    GenerationTarget {
        target: GenerationTarget,
        resolved_snapshot_sha256: Sha256Digest,
    },
    /// Private injected-provider paths have no durable endpoint row. Their
    /// caller-owned model identity remains explicit for deterministic tests.
    DirectModel { model_sha256: Sha256Digest },
}

/// Exact prompt-preset selection consulted before an attempt crosses an
/// approval pause. Resume and fork paths use this value without re-reading
/// mutable prompt inputs.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GenerationPromptSelectionAuthority {
    pub schema_version: u32,
    pub mode: ConversationMode,
    /// Domain-separated identity of the local profile whose user-facing
    /// values were materialized into this attempt's prompt context.
    pub local_user_id_sha256: String,
    pub character: Character,
    pub character_content: Option<StoredRevision<CharacterContentV1>>,
    pub character_knowledge_book: Option<StoredRevision<KnowledgeBook>>,
    pub supported_capabilities: Vec<CapabilityKey>,
    pub quick_settings: GenerationPromptQuickSettingsAuthority,
    /// Added inside the existing schema-36 authority envelope. Older rows
    /// decode without it and remain hash-compatible, but cannot be resumed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_target_authority: Option<GenerationProviderTargetAuthority>,
    pub explicit_preset_id: Option<PromptPresetId>,
    pub preset: PromptPreset,
    pub preset_revision: u64,
    pub preset_revision_id: String,
    pub binding: Option<StoredRevision<PromptPresetBinding>>,
    pub persona_selection: Option<StoredRevision<ConversationPersonaSelection>>,
}

/// Immutable pre-interaction request identity for one provider dispatch.
///
/// `base_request_fingerprint_sha256` covers the requested user input, target,
/// preset, and other Core-owned inputs before `BeforeGeneration` effects. The
/// final prompt is sealed separately only after all approvals have resolved.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GenerationAttemptInput {
    /// Caller-stable idempotency key retained across restart/response loss.
    pub operation_id: String,
    pub conversation_id: ConversationId,
    pub source_branch_id: ConversationBranchId,
    pub proposed_branch_id: ConversationBranchId,
    /// Optimistic concurrency guard for the current source branch.
    pub expected_head_message_id: Option<MessageId>,
    /// Historical prompt/interaction boundary. A fork may point at an
    /// ancestor of `expected_head_message_id`, but never at later source state.
    pub context_head_message_id: Option<MessageId>,
    pub module_plan_sha256: Sha256Digest,
    pub base_request_fingerprint_sha256: Sha256Digest,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prompt_selection_authority: Option<GenerationPromptSelectionAuthority>,
    /// Exact initial module review captured before any approval pause. Legacy
    /// schema-35 attempts omit it and are not resumable.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub module_runtime_review_authority: Option<ModuleMergeReview>,
    /// Exact applied runtime plan paired with the review. `None` is the sealed
    /// no-module result, not permission to recalculate from live bindings.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub applied_runtime_plan_authority: Option<AppliedModuleRuntimePlan>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GenerationAttemptStatus {
    Prepared,
    BeforeGenerationApplied,
    AwaitingApproval,
    DispatchReady,
    Running,
    FailedBeforeDispatch,
    Completed,
}

impl GenerationAttemptStatus {
    const fn as_str(self) -> &'static str {
        match self {
            Self::Prepared => "prepared",
            Self::BeforeGenerationApplied => "before_generation_applied",
            Self::AwaitingApproval => "awaiting_approval",
            Self::DispatchReady => "dispatch_ready",
            Self::Running => "running",
            Self::FailedBeforeDispatch => "failed_before_dispatch",
            Self::Completed => "completed",
        }
    }

    fn parse(value: &str) -> CoreResult<Self> {
        match value {
            "prepared" => Ok(Self::Prepared),
            "before_generation_applied" => Ok(Self::BeforeGenerationApplied),
            "awaiting_approval" => Ok(Self::AwaitingApproval),
            "dispatch_ready" => Ok(Self::DispatchReady),
            "running" => Ok(Self::Running),
            "failed_before_dispatch" => Ok(Self::FailedBeforeDispatch),
            "completed" => Ok(Self::Completed),
            _ => Err(corrupted("stored generation attempt status is invalid")),
        }
    }
}

/// Exact already-reviewed `BeforeGeneration` occurrence. A pending approval
/// resumes this evidence; it is never re-evaluated against newer state.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GenerationBeforeEventEvidence {
    pub event_id: String,
    pub event_sha256: Sha256Digest,
    pub context_state_revision: u64,
    pub context_state_sha256: Sha256Digest,
    pub proposal_review_sha256s: Vec<Sha256Digest>,
    pub awaiting_approval: bool,
}

/// Exactly-once decisions/UserAction events that resolved a reviewed proposal.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GenerationApprovalEvidence {
    pub before_event_sha256: Sha256Digest,
    pub decision_event_ids: Vec<String>,
    pub decision_event_sha256s: Vec<Sha256Digest>,
    pub resulting_state_revision: u64,
    pub resulting_state_sha256: Sha256Digest,
}

/// Immutable authority checked in the same transaction that starts dispatch.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GenerationDispatchSeal {
    pub final_prompt_plan_sha256: Sha256Digest,
    pub final_prompt_input_fingerprint_sha256: Sha256Digest,
    pub final_interaction_state_revision: u64,
    pub final_interaction_state_sha256: Sha256Digest,
    pub applied_module_plan_sha256: Sha256Digest,
    pub before_generation_evidence_sha256: Sha256Digest,
    pub approval_evidence_sha256: Option<Sha256Digest>,
    /// Cumulative immutable derived-transition chain at dispatch. `None` is
    /// accepted only while decoding a pre-schema-36 legacy seal, which cannot
    /// be resumed or appended.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub derived_chain_sha256: Option<Sha256Digest>,
    /// Number of transitions covered by `derived_chain_sha256`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub derived_event_count: Option<u32>,
    /// Number of suppressed derived edges covered by the same chain.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub derived_guard_count: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StoredGenerationAttempt {
    pub generation_id: GenerationId,
    pub input: GenerationAttemptInput,
    pub attempt_sha256: Sha256Digest,
    pub status: GenerationAttemptStatus,
    pub revision: u64,
    pub before_generation_evidence: Option<GenerationBeforeEventEvidence>,
    pub before_generation_evidence_sha256: Option<Sha256Digest>,
    pub approval_evidence: Option<GenerationApprovalEvidence>,
    pub approval_evidence_sha256: Option<Sha256Digest>,
    pub dispatch_seal: Option<GenerationDispatchSeal>,
    pub dispatch_seal_sha256: Option<Sha256Digest>,
    pub failure_code: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// Non-sensitive restart projection for one attempt that can continue without
/// replaying its already-reviewed `BeforeGeneration` occurrence.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RetryableGenerationAttemptProjection {
    pub generation_id: GenerationId,
    pub status: GenerationAttemptStatus,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Serialize)]
struct GenerationAttemptDigest<'a> {
    schema_version: u32,
    input: &'a GenerationAttemptInput,
}

#[derive(Serialize)]
struct GenerationOperationDigest<'a> {
    schema_version: u32,
    operation_id: &'a str,
    conversation_id: &'a ConversationId,
    source_branch_id: &'a ConversationBranchId,
    context_head_message_id: Option<&'a MessageId>,
}

struct EncodedGenerationAttemptAuthorities {
    prompt_selection_json: String,
    prompt_selection_sha256: Sha256Digest,
    module_runtime_review_json: String,
    module_runtime_review_sha256: Sha256Digest,
    applied_runtime_plan_json: Option<String>,
    applied_runtime_plan_sha256: Option<Sha256Digest>,
}

struct GenerationAttemptRow {
    operation_id: String,
    conversation_id: String,
    source_branch_id: String,
    proposed_branch_id: String,
    expected_head_message_id: Option<String>,
    context_head_message_id: Option<String>,
    module_plan_sha256: String,
    base_input_fingerprint_sha256: String,
    attempt_sha256: String,
    status: String,
    revision: i64,
    before_generation_evidence_json: Option<String>,
    before_generation_evidence_sha256: Option<String>,
    approval_evidence_json: Option<String>,
    approval_evidence_sha256: Option<String>,
    dispatch_seal_json: Option<String>,
    dispatch_seal_sha256: Option<String>,
    failure_code: Option<String>,
    created_at: String,
    updated_at: String,
    prompt_selection_authority_json: Option<String>,
    prompt_selection_authority_sha256: Option<String>,
    prompt_selection_authority_version: i64,
    module_runtime_review_authority_json: Option<String>,
    module_runtime_review_authority_sha256: Option<String>,
    applied_runtime_plan_authority_json: Option<String>,
    applied_runtime_plan_authority_sha256: Option<String>,
    module_runtime_authority_version: i64,
}

impl Storage {
    /// Persists a deterministic intent before any durable `BeforeGeneration`
    /// transition. Exact retries are idempotent.
    ///
    /// Same-branch requests enqueue their durable lifecycle occurrence here.
    /// Fork requests deliberately do not enqueue against the source branch:
    /// their reviewed event and historical checkpoint must be committed to the
    /// proposed child by the atomic action-append path.
    pub fn prepare_generation_attempt(
        &self,
        input: &GenerationAttemptInput,
        prepared_at: DateTime<Utc>,
    ) -> CoreResult<StoredGenerationAttempt> {
        self.prepare_generation_attempt_observed(input, prepared_at, None, false)
    }

    /// Production admission boundary after a native credential read. The
    /// exact read authority is compared in the same immediate transaction
    /// which creates the attempt intent.
    pub fn prepare_generation_attempt_with_credential_authority(
        &self,
        input: &GenerationAttemptInput,
        prepared_at: DateTime<Utc>,
        credential_authority: Option<&ProviderCredentialAccessAuthority>,
    ) -> CoreResult<StoredGenerationAttempt> {
        self.prepare_generation_attempt_observed(input, prepared_at, credential_authority, true)
    }

    fn prepare_generation_attempt_observed(
        &self,
        input: &GenerationAttemptInput,
        prepared_at: DateTime<Utc>,
        credential_authority: Option<&ProviderCredentialAccessAuthority>,
        require_exact_credential_authority: bool,
    ) -> CoreResult<StoredGenerationAttempt> {
        validate_input(input)?;
        if input
            .prompt_selection_authority
            .as_ref()
            .and_then(|authority| authority.provider_target_authority.as_ref())
            .is_none()
        {
            return Err(CoreError::invalid(
                "generation attempt provider target authority is missing",
            ));
        }
        let module_runtime_review =
            input
                .module_runtime_review_authority
                .as_ref()
                .ok_or_else(|| {
                    CoreError::invalid("generation attempt module runtime authority is missing")
                })?;
        let encoded_authorities = encode_generation_attempt_authorities(input)?;
        let attempt_sha256 = generation_attempt_sha256(input)?;
        let generation_id = deterministic_generation_id(&attempt_sha256);
        let mut connection = self.connection()?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(storage_db_error)?;
        ensure_generation_provider_credential_settled(
            &transaction,
            input,
            credential_authority,
            require_exact_credential_authority,
        )?;
        if let Some(existing_generation_id) = transaction
            .query_row(
                "SELECT generation_id
                 FROM generation_attempt_intents
                 WHERE conversation_id = ?1 AND operation_id = ?2",
                params![input.conversation_id.0, input.operation_id],
                |row| row.get::<_, String>(0),
            )
            .optional()
            .map_err(storage_db_error)?
            .map(GenerationId)
        {
            let stored = read_attempt(&transaction, &existing_generation_id)?;
            if stored.generation_id != generation_id
                || stored.input != *input
                || stored.attempt_sha256 != attempt_sha256
            {
                return Err(CoreError::new(
                    CoreErrorCode::InvalidInput,
                    "generation operation id was reused with different input",
                    false,
                ));
            }
            transaction.commit().map_err(storage_db_error)?;
            return Ok(stored);
        }
        validate_source_snapshot(&transaction, input)?;
        validate_generation_prompt_character_row(&transaction, input)?;
        validate_generation_prompt_content_heads(&transaction, input)?;
        crate::orchestration::validate_fresh_module_merge_review(
            self,
            &transaction,
            module_runtime_review,
        )?;
        insert_prepared_generation_attempt(
            &transaction,
            input,
            &generation_id,
            &attempt_sha256,
            prepared_at,
            &encoded_authorities,
        )?;
        let stored = read_attempt(&transaction, &generation_id)?;
        if stored.generation_id != generation_id
            || stored.input != *input
            || stored.attempt_sha256 != attempt_sha256
        {
            return Err(CoreError::new(
                CoreErrorCode::InvalidInput,
                "generation operation id was reused with different input",
                false,
            ));
        }
        transaction.commit().map_err(storage_db_error)?;
        Ok(stored)
    }

    pub fn get_generation_attempt(
        &self,
        generation_id: &GenerationId,
    ) -> CoreResult<StoredGenerationAttempt> {
        validate_id("generation", &generation_id.0)?;
        let connection = self.connection()?;
        read_attempt(&connection, generation_id)
    }

    pub fn get_generation_attempt_by_operation_id(
        &self,
        conversation_id: &ConversationId,
        operation_id: &str,
    ) -> CoreResult<StoredGenerationAttempt> {
        validate_id("conversation", &conversation_id.0)?;
        validate_id("generation operation", operation_id)?;
        let connection = self.connection()?;
        let generation_id = connection
            .query_row(
                "SELECT generation_id
                 FROM generation_attempt_intents
                 WHERE conversation_id = ?1 AND operation_id = ?2",
                params![conversation_id.0, operation_id],
                |row| row.get::<_, String>(0),
            )
            .optional()
            .map_err(storage_db_error)?
            .map(GenerationId)
            .ok_or_else(|| not_found("generation attempt"))?;
        read_attempt(&connection, &generation_id)
    }

    /// Lists restart-retryable attempts for one exact source room without
    /// exposing operation, provider, prompt-plan, or nonce authority.
    pub fn list_retryable_generation_attempts_for_source_room(
        &self,
        conversation_id: &ConversationId,
        source_branch_id: &ConversationBranchId,
        limit: u32,
    ) -> CoreResult<Vec<RetryableGenerationAttemptProjection>> {
        validate_id("retryable generation conversation", &conversation_id.0)?;
        validate_id("retryable generation source branch", &source_branch_id.0)?;
        validate_retryable_attempt_list_limit(limit)?;
        let connection = self.connection()?;
        let generation_ids = {
            let mut statement = connection
                .prepare(
                    "SELECT generation_id
                     FROM generation_attempt_intents
                     WHERE conversation_id = ?1
                       AND source_branch_id = ?2
                       AND status IN ('before_generation_applied', 'dispatch_ready')
                     ORDER BY created_at DESC, generation_id DESC
                     LIMIT ?3",
                )
                .map_err(storage_db_error)?;
            statement
                .query_map(
                    params![conversation_id.0, source_branch_id.0, i64::from(limit)],
                    |row| row.get::<_, String>(0),
                )
                .map_err(storage_db_error)?
                .collect::<Result<Vec<_>, _>>()
                .map_err(storage_db_error)?
        };
        generation_ids
            .into_iter()
            .map(|generation_id| {
                let stored = read_attempt(&connection, &GenerationId(generation_id))?;
                retryable_generation_attempt_projection(&stored, conversation_id, source_branch_id)
            })
            .collect()
    }

    pub fn record_generation_attempt_before_event(
        &self,
        generation_id: &GenerationId,
        expected_revision: u64,
        evidence: &GenerationBeforeEventEvidence,
        recorded_at: DateTime<Utc>,
    ) -> CoreResult<StoredGenerationAttempt> {
        validate_before_evidence(evidence)?;
        let next = if evidence.awaiting_approval {
            GenerationAttemptStatus::AwaitingApproval
        } else {
            GenerationAttemptStatus::BeforeGenerationApplied
        };
        let (json, sha256) = encode_hashed("before-generation evidence", evidence)?;
        let mut connection = self.connection()?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(storage_db_error)?;
        let current = read_attempt(&transaction, generation_id)?;
        if current.revision != expected_revision
            || current.status != GenerationAttemptStatus::Prepared
            || current.before_generation_evidence.is_some()
        {
            return Err(attempt_conflict(generation_id));
        }
        let changed = transaction
            .execute(
                "UPDATE generation_attempt_intents
                 SET status = ?2, revision = revision + 1,
                     before_generation_evidence_json = ?3,
                     before_generation_evidence_sha256 = ?4,
                     updated_at = ?5
                 WHERE generation_id = ?1
                   AND revision = ?6 AND status = 'prepared'
                   AND before_generation_evidence_sha256 IS NULL",
                params![
                    generation_id.0,
                    next.as_str(),
                    json,
                    sha256.as_str(),
                    recorded_at.to_rfc3339(),
                    i64_from_u64(expected_revision)?,
                ],
            )
            .map_err(storage_db_error)?;
        if changed != 1 {
            return Err(attempt_conflict(generation_id));
        }
        let stored = read_attempt(&transaction, generation_id)?;
        transaction.commit().map_err(storage_db_error)?;
        Ok(stored)
    }

    pub fn resolve_generation_attempt_approval(
        &self,
        generation_id: &GenerationId,
        expected_revision: u64,
        evidence: &GenerationApprovalEvidence,
        resolved_at: DateTime<Utc>,
    ) -> CoreResult<StoredGenerationAttempt> {
        validate_approval_evidence(evidence)?;
        let (json, sha256) = encode_hashed("generation approval evidence", evidence)?;
        let mut connection = self.connection()?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(storage_db_error)?;
        let current = read_attempt(&transaction, generation_id)?;
        if current.revision != expected_revision
            || current.status != GenerationAttemptStatus::AwaitingApproval
            || current.approval_evidence.is_some()
            || current.before_generation_evidence_sha256.as_ref()
                != Some(&evidence.before_event_sha256)
        {
            return Err(attempt_conflict(generation_id));
        }
        let changed = transaction
            .execute(
                "UPDATE generation_attempt_intents
                 SET status = 'before_generation_applied',
                     revision = revision + 1,
                     approval_evidence_json = ?2,
                     approval_evidence_sha256 = ?3,
                     updated_at = ?4
                 WHERE generation_id = ?1
                   AND revision = ?5 AND status = 'awaiting_approval'
                   AND approval_evidence_sha256 IS NULL",
                params![
                    generation_id.0,
                    json,
                    sha256.as_str(),
                    resolved_at.to_rfc3339(),
                    i64_from_u64(expected_revision)?,
                ],
            )
            .map_err(storage_db_error)?;
        if changed != 1 {
            return Err(attempt_conflict(generation_id));
        }
        let stored = read_attempt(&transaction, generation_id)?;
        transaction.commit().map_err(storage_db_error)?;
        Ok(stored)
    }

    pub fn seal_generation_attempt_dispatch_ready(
        &self,
        generation_id: &GenerationId,
        expected_revision: u64,
        seal: &GenerationDispatchSeal,
        ready_at: DateTime<Utc>,
    ) -> CoreResult<StoredGenerationAttempt> {
        validate_dispatch_seal(seal)?;
        let (json, sha256) = encode_hashed("generation dispatch seal", seal)?;
        let mut connection = self.connection()?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(storage_db_error)?;
        let current = read_attempt(&transaction, generation_id)?;
        let aggregate =
            crate::interaction_repository::read_generation_attempt_interaction_aggregate(
                &transaction,
                generation_id,
            )?;
        if current.revision != expected_revision
            || current.status != GenerationAttemptStatus::BeforeGenerationApplied
            || current.dispatch_seal.is_some()
            || current.input.module_plan_sha256 != seal.applied_module_plan_sha256
            || current.before_generation_evidence_sha256.as_ref()
                != Some(&seal.before_generation_evidence_sha256)
            || current.approval_evidence_sha256.as_ref() != seal.approval_evidence_sha256.as_ref()
            || seal.derived_chain_sha256.as_ref() != Some(&aggregate.derived_chain_sha256)
            || seal.derived_event_count != Some(aggregate.derived_event_count)
            || seal.derived_guard_count != Some(aggregate.derived_guard_count)
        {
            return Err(attempt_conflict(generation_id));
        }
        let changed = transaction
            .execute(
                "UPDATE generation_attempt_intents
                 SET status = 'dispatch_ready', revision = revision + 1,
                     dispatch_seal_json = ?2,
                     dispatch_seal_sha256 = ?3,
                     updated_at = ?4
                 WHERE generation_id = ?1
                   AND revision = ?5
                   AND status = 'before_generation_applied'
                   AND dispatch_seal_sha256 IS NULL",
                params![
                    generation_id.0,
                    json,
                    sha256.as_str(),
                    ready_at.to_rfc3339(),
                    i64_from_u64(expected_revision)?,
                ],
            )
            .map_err(storage_db_error)?;
        if changed != 1 {
            return Err(attempt_conflict(generation_id));
        }
        let stored = read_attempt(&transaction, generation_id)?;
        transaction.commit().map_err(storage_db_error)?;
        Ok(stored)
    }

    pub fn fail_generation_attempt_before_dispatch(
        &self,
        generation_id: &GenerationId,
        expected_revision: u64,
        failure_code: &str,
        failed_at: DateTime<Utc>,
    ) -> CoreResult<StoredGenerationAttempt> {
        validate_failure_code(failure_code)?;
        self.transition_generation_attempt(
            generation_id,
            expected_revision,
            &[
                GenerationAttemptStatus::Prepared,
                GenerationAttemptStatus::BeforeGenerationApplied,
                GenerationAttemptStatus::AwaitingApproval,
            ],
            GenerationAttemptStatus::FailedBeforeDispatch,
            Some(failure_code),
            failed_at,
        )
    }

    /// Resumes the exact stored phase and evidence after a pre-dispatch
    /// failure. This never clears evidence and therefore never retriggers a
    /// reviewed approval.
    pub fn retry_generation_attempt(
        &self,
        generation_id: &GenerationId,
        expected_revision: u64,
        retried_at: DateTime<Utc>,
    ) -> CoreResult<StoredGenerationAttempt> {
        self.retry_generation_attempt_with_credential_authority(
            generation_id,
            expected_revision,
            retried_at,
            None,
        )
    }

    pub fn retry_generation_attempt_with_credential_authority(
        &self,
        generation_id: &GenerationId,
        expected_revision: u64,
        retried_at: DateTime<Utc>,
        credential_authority: Option<&ProviderCredentialAccessAuthority>,
    ) -> CoreResult<StoredGenerationAttempt> {
        let mut connection = self.connection()?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(storage_db_error)?;
        let current = read_attempt(&transaction, generation_id)?;
        if current.revision != expected_revision
            || current.status != GenerationAttemptStatus::FailedBeforeDispatch
        {
            return Err(attempt_conflict(generation_id));
        }
        if current
            .input
            .prompt_selection_authority
            .as_ref()
            .and_then(|authority| authority.provider_target_authority.as_ref())
            .is_none()
            || current.input.module_runtime_review_authority.is_none()
            || current.failure_code.as_deref() == Some("stale_generation_derived_closure_authority")
        {
            return Err(CoreError::new(
                CoreErrorCode::InvalidInput,
                "legacy generation attempt cannot be retried; start a new regeneration operation",
                false,
            ));
        }
        ensure_generation_provider_credential_settled(
            &transaction,
            &current.input,
            credential_authority,
            true,
        )?;
        let next = match current.before_generation_evidence.as_ref() {
            None => GenerationAttemptStatus::Prepared,
            Some(evidence) if evidence.awaiting_approval && current.approval_evidence.is_none() => {
                GenerationAttemptStatus::AwaitingApproval
            }
            Some(_) => GenerationAttemptStatus::BeforeGenerationApplied,
        };
        transition_attempt_in_transaction(
            &transaction,
            generation_id,
            expected_revision,
            &[GenerationAttemptStatus::FailedBeforeDispatch],
            next,
            None,
            retried_at,
        )?;
        let stored = read_attempt(&transaction, generation_id)?;
        transaction.commit().map_err(storage_db_error)?;
        Ok(stored)
    }

    fn transition_generation_attempt(
        &self,
        generation_id: &GenerationId,
        expected_revision: u64,
        expected_statuses: &[GenerationAttemptStatus],
        next_status: GenerationAttemptStatus,
        failure_code: Option<&str>,
        updated_at: DateTime<Utc>,
    ) -> CoreResult<StoredGenerationAttempt> {
        let mut connection = self.connection()?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(storage_db_error)?;
        transition_attempt_in_transaction(
            &transaction,
            generation_id,
            expected_revision,
            expected_statuses,
            next_status,
            failure_code,
            updated_at,
        )?;
        let stored = read_attempt(&transaction, generation_id)?;
        transaction.commit().map_err(storage_db_error)?;
        Ok(stored)
    }
}

fn ensure_generation_provider_credential_settled(
    transaction: &Transaction<'_>,
    input: &GenerationAttemptInput,
    credential_authority: Option<&ProviderCredentialAccessAuthority>,
    require_exact_credential_authority: bool,
) -> CoreResult<()> {
    let authority = input
        .prompt_selection_authority
        .as_ref()
        .and_then(|selection| selection.provider_target_authority.as_ref())
        .ok_or_else(|| {
            CoreError::invalid("generation attempt provider target authority is missing")
        })?;
    let connection_id = match authority {
        GenerationProviderTargetAuthority::ProviderProfile {
            provider_profile_id,
            ..
        } => Some(ProviderConnectionId::from(provider_profile_id.clone())),
        GenerationProviderTargetAuthority::GenerationTarget { target, .. } => {
            Some(ProviderConnectionId::from(
                transaction
                    .query_row(
                        "SELECT connection_id FROM provider_models WHERE id = ?1",
                        [target.model_route_id.as_str()],
                        |row| row.get::<_, String>(0),
                    )
                    .optional()
                    .map_err(storage_db_error)?
                    .ok_or_else(|| {
                        CoreError::invalid("generation provider model route is unavailable")
                    })?,
            ))
        }
        GenerationProviderTargetAuthority::DirectModel { .. } => None,
    };
    let Some(connection_id) = connection_id else {
        return Ok(());
    };
    ensure_generation_provider_connection_is_active(transaction, &connection_id)?;
    if require_exact_credential_authority {
        return validate_provider_credential_access_authority_in_transaction(
            transaction,
            &connection_id,
            credential_authority,
        );
    }
    let unresolved = transaction
        .query_row(
            "SELECT EXISTS(
               SELECT 1
               FROM provider_credential_operations
               WHERE connection_id = ?1
                 AND status IN (
                   'prepared', 'started', 'cleanup_required', 'outcome_unknown'
                 )
             )",
            [connection_id.as_str()],
            |row| row.get::<_, bool>(0),
        )
        .map_err(storage_db_error)?;
    if unresolved {
        return Err(CoreError::new(
            CoreErrorCode::InvalidInput,
            "generation cannot start while provider credential recovery is unresolved",
            true,
        ));
    }
    Ok(())
}

fn ensure_generation_provider_connection_is_active(
    connection: &Connection,
    connection_id: &ProviderConnectionId,
) -> CoreResult<()> {
    let active = connection
        .query_row(
            "SELECT EXISTS(
               SELECT 1 FROM provider_connections
               WHERE id = ?1 AND archived_at IS NULL
             )",
            [connection_id.as_str()],
            |row| row.get::<_, bool>(0),
        )
        .map_err(storage_db_error)?;
    if active {
        Ok(())
    } else {
        Err(CoreError::new(
            CoreErrorCode::InvalidInput,
            "generation provider connection is unavailable",
            true,
        ))
    }
}

pub(crate) fn validate_provider_credential_access_authority_in_transaction(
    connection: &Connection,
    connection_id: &ProviderConnectionId,
    expected: Option<&ProviderCredentialAccessAuthority>,
) -> CoreResult<()> {
    let credential_ref = load_active_provider_credential_ref(connection, connection_id)?;
    let Some(credential_ref) = credential_ref else {
        if expected.is_some() {
            return Err(CoreError::invalid(
                "credential authority was supplied for a credentialless provider connection",
            ));
        }
        return Ok(());
    };
    if credential_ref != connection_id.as_str() {
        return Err(CoreError::new(
            CoreErrorCode::StorageCorrupted,
            "provider credential binding is detached from its connection",
            false,
        ));
    }
    ensure_provider_credential_operation_settled(connection, connection_id)?;
    let projection = load_provider_credential_authority(connection, connection_id)?;
    if !matches!(
        projection.ownership_state.as_str(),
        "ordinary_owned" | "discovery_owned"
    ) {
        return Err(CoreError::new(
            CoreErrorCode::InvalidInput,
            "provider credential has no current durable access authority",
            true,
        ));
    }
    let authority = projection.into_current_authority()?;
    let expected = expected.ok_or_else(|| {
        CoreError::new(
            CoreErrorCode::InvalidInput,
            "provider credential access authority is required",
            true,
        )
    })?;
    let current_binding_sha256 =
        crate::provider_credential_repository::provider_credential_connection_binding_sha256(
            connection,
            connection_id,
        )?;
    if expected.authority_id != authority.authority_id
        || expected.connection_binding_sha256 != authority.binding_sha256
        || current_binding_sha256 != authority.binding_sha256
    {
        return Err(CoreError::new(
            CoreErrorCode::InvalidInput,
            "provider credential access authority is stale",
            true,
        ));
    }
    ensure_provider_credential_authority_has_durable_history(connection, connection_id, &authority)
}

struct ProviderCredentialAuthorityProjection {
    ownership_state: String,
    binding_sha256: Option<String>,
    authority_id: Option<String>,
}

impl ProviderCredentialAuthorityProjection {
    fn into_current_authority(self) -> CoreResult<CurrentProviderCredentialAuthority> {
        let binding_sha256 = self.binding_sha256.ok_or_else(|| {
            CoreError::new(
                CoreErrorCode::StorageCorrupted,
                "provider credential ownership binding is missing",
                false,
            )
        })?;
        let authority_id = self.authority_id.ok_or_else(|| {
            CoreError::new(
                CoreErrorCode::StorageCorrupted,
                "provider credential ownership authority is missing",
                false,
            )
        })?;
        Ok(CurrentProviderCredentialAuthority {
            ownership_state: self.ownership_state,
            binding_sha256,
            authority_id,
        })
    }
}

struct CurrentProviderCredentialAuthority {
    ownership_state: String,
    binding_sha256: String,
    authority_id: String,
}

fn load_active_provider_credential_ref(
    connection: &Connection,
    connection_id: &ProviderConnectionId,
) -> CoreResult<Option<String>> {
    connection
        .query_row(
            "SELECT credential_ref
             FROM provider_connections
             WHERE id = ?1 AND archived_at IS NULL",
            [connection_id.as_str()],
            |row| row.get::<_, Option<String>>(0),
        )
        .optional()
        .map_err(storage_db_error)?
        .ok_or_else(|| CoreError::invalid("provider connection is unavailable"))
}

fn ensure_provider_credential_operation_settled(
    connection: &Connection,
    connection_id: &ProviderConnectionId,
) -> CoreResult<()> {
    let unresolved = connection
        .query_row(
            "SELECT EXISTS(
               SELECT 1
               FROM provider_credential_operations
               WHERE connection_id = ?1
                 AND status IN (
                   'prepared', 'started', 'cleanup_required', 'outcome_unknown'
                 )
             )",
            [connection_id.as_str()],
            |row| row.get::<_, bool>(0),
        )
        .map_err(storage_db_error)?;
    if unresolved {
        return Err(CoreError::new(
            CoreErrorCode::InvalidInput,
            "provider credential recovery is unresolved",
            true,
        ));
    }
    Ok(())
}

fn load_provider_credential_authority(
    connection: &Connection,
    connection_id: &ProviderConnectionId,
) -> CoreResult<ProviderCredentialAuthorityProjection> {
    let (ownership_state, binding_sha256, authority_id) = connection
        .query_row(
            "SELECT ownership_state, connection_binding_sha256, authority_id
             FROM provider_credential_ownership
             WHERE connection_id = ?1 AND credential_ref = ?1",
            [connection_id.as_str()],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, Option<String>>(1)?,
                    row.get::<_, Option<String>>(2)?,
                ))
            },
        )
        .optional()
        .map_err(storage_db_error)?
        .ok_or_else(|| {
            CoreError::new(
                CoreErrorCode::StorageCorrupted,
                "provider credential ownership projection is missing",
                false,
            )
        })?;
    Ok(ProviderCredentialAuthorityProjection {
        ownership_state,
        binding_sha256,
        authority_id,
    })
}

fn ensure_provider_credential_authority_has_durable_history(
    connection: &Connection,
    connection_id: &ProviderConnectionId,
    authority: &CurrentProviderCredentialAuthority,
) -> CoreResult<()> {
    let durable_authority_exists = provider_credential_ownership_authority_is_valid(
        connection,
        connection_id,
        &authority.ownership_state,
        Some(&authority.binding_sha256),
        &authority.authority_id,
    )?;
    if !durable_authority_exists {
        return Err(CoreError::new(
            CoreErrorCode::StorageCorrupted,
            "provider credential authority is not backed by durable history",
            false,
        ));
    }
    Ok(())
}

fn encode_generation_attempt_authorities(
    input: &GenerationAttemptInput,
) -> CoreResult<EncodedGenerationAttemptAuthorities> {
    let prompt_selection = input.prompt_selection_authority.as_ref().ok_or_else(|| {
        CoreError::invalid("generation attempt prompt selection authority is missing")
    })?;
    let (prompt_selection_json, prompt_selection_sha256) =
        encode_hashed("generation prompt selection authority", prompt_selection)?;
    let module_runtime_review =
        input
            .module_runtime_review_authority
            .as_ref()
            .ok_or_else(|| {
                CoreError::invalid("generation attempt module runtime authority is missing")
            })?;
    let (module_runtime_review_json, module_runtime_review_sha256) = encode_hashed(
        "generation attempt module runtime review authority",
        module_runtime_review,
    )?;
    let (applied_runtime_plan_json, applied_runtime_plan_sha256) = input
        .applied_runtime_plan_authority
        .as_ref()
        .map(|plan| encode_hashed("generation attempt applied runtime plan authority", plan))
        .transpose()?
        .map_or((None, None), |(json, sha256)| (Some(json), Some(sha256)));
    Ok(EncodedGenerationAttemptAuthorities {
        prompt_selection_json,
        prompt_selection_sha256,
        module_runtime_review_json,
        module_runtime_review_sha256,
        applied_runtime_plan_json,
        applied_runtime_plan_sha256,
    })
}

fn insert_prepared_generation_attempt(
    transaction: &Transaction<'_>,
    input: &GenerationAttemptInput,
    generation_id: &GenerationId,
    attempt_sha256: &Sha256Digest,
    prepared_at: DateTime<Utc>,
    authorities: &EncodedGenerationAttemptAuthorities,
) -> CoreResult<()> {
    transaction
        .execute(
            "INSERT INTO generation_attempt_intents
             (generation_id, operation_id, conversation_id,
              source_branch_id, proposed_branch_id,
              expected_head_message_id, context_head_message_id,
              module_plan_sha256, base_input_fingerprint_sha256,
              before_generation_evidence_json,
              before_generation_evidence_sha256,
              approval_evidence_json, approval_evidence_sha256,
              dispatch_seal_json, dispatch_seal_sha256,
              attempt_sha256, status, revision, failure_code,
              created_at, updated_at,
              prompt_selection_authority_json,
              prompt_selection_authority_sha256,
              prompt_selection_authority_version,
              module_runtime_review_authority_json,
              module_runtime_review_authority_sha256,
              applied_runtime_plan_authority_json,
              applied_runtime_plan_authority_sha256,
              module_runtime_authority_version)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9,
                     NULL, NULL, NULL, NULL, NULL, NULL, ?10,
                     'prepared', 1, NULL, ?11, ?11, ?12, ?13, 1,
                     ?14, ?15, ?16, ?17, 1)",
            params![
                generation_id.0,
                input.operation_id,
                input.conversation_id.0,
                input.source_branch_id.0,
                input.proposed_branch_id.0,
                optional_message(input.expected_head_message_id.as_ref()),
                optional_message(input.context_head_message_id.as_ref()),
                input.module_plan_sha256.as_str(),
                input.base_request_fingerprint_sha256.as_str(),
                attempt_sha256.as_str(),
                prepared_at.to_rfc3339(),
                authorities.prompt_selection_json,
                authorities.prompt_selection_sha256.as_str(),
                authorities.module_runtime_review_json,
                authorities.module_runtime_review_sha256.as_str(),
                authorities.applied_runtime_plan_json,
                authorities
                    .applied_runtime_plan_sha256
                    .as_ref()
                    .map(Sha256Digest::as_str),
            ],
        )
        .map_err(storage_db_error)?;
    Ok(())
}

pub fn generation_attempt_sha256(input: &GenerationAttemptInput) -> CoreResult<Sha256Digest> {
    validate_input(input)?;
    hash_json(&GenerationAttemptDigest {
        schema_version: GENERATION_ATTEMPT_SCHEMA_VERSION,
        input,
    })
}

pub fn generation_prompt_selection_authority_sha256(
    authority: &GenerationPromptSelectionAuthority,
) -> CoreResult<Sha256Digest> {
    validate_generation_prompt_selection_authority(authority)?;
    hash_json(authority)
}

pub fn deterministic_generation_id(attempt_sha256: &Sha256Digest) -> GenerationId {
    GenerationId(format!("attempt-{}", attempt_sha256.as_str()))
}

pub fn deterministic_proposed_branch_id(
    operation_id: &str,
    conversation_id: &ConversationId,
    source_branch_id: &ConversationBranchId,
    context_head_message_id: Option<&MessageId>,
) -> CoreResult<ConversationBranchId> {
    validate_id("generation operation", operation_id)?;
    validate_id("conversation", &conversation_id.0)?;
    validate_id("source branch", &source_branch_id.0)?;
    let digest = hash_json(&GenerationOperationDigest {
        schema_version: GENERATION_ATTEMPT_SCHEMA_VERSION,
        operation_id,
        conversation_id,
        source_branch_id,
        context_head_message_id,
    })?;
    Ok(ConversationBranchId(format!(
        "attempt-branch-{}",
        digest.as_str()
    )))
}

pub fn generation_before_event_evidence_sha256(
    evidence: &GenerationBeforeEventEvidence,
) -> CoreResult<Sha256Digest> {
    validate_before_evidence(evidence)?;
    hash_json(evidence)
}

pub fn generation_approval_evidence_sha256(
    evidence: &GenerationApprovalEvidence,
) -> CoreResult<Sha256Digest> {
    validate_approval_evidence(evidence)?;
    hash_json(evidence)
}

pub fn generation_dispatch_seal_sha256(seal: &GenerationDispatchSeal) -> CoreResult<Sha256Digest> {
    validate_dispatch_seal(seal)?;
    hash_json(seal)
}

pub fn interaction_evaluation_seal_sha256(
    seal: &InteractionEvaluationSeal,
) -> CoreResult<Sha256Digest> {
    validate_interaction_evaluation_seal(seal)?;
    hash_json(seal)
}

pub fn generation_attempt_derived_transition_sha256(
    transition: &GenerationAttemptDerivedTransition,
) -> CoreResult<Sha256Digest> {
    validate_generation_attempt_derived_transition(transition)?;
    hash_json(transition)
}

pub fn generation_attempt_derived_guard_evidence_sha256(
    audit: &GenerationAttemptDerivedGuardAudit,
) -> CoreResult<Sha256Digest> {
    #[derive(Serialize)]
    #[serde(deny_unknown_fields)]
    struct GuardFingerprint<'a> {
        schema_version: u32,
        kind: GenerationAttemptDerivedGuardKind,
        candidate_event_sha256: Option<&'a Sha256Digest>,
        parent_ordinal: u32,
        depth: u32,
        suppressed_count: u32,
    }

    hash_json(&GuardFingerprint {
        schema_version: 1,
        kind: audit.kind,
        candidate_event_sha256: audit.candidate_event_sha256.as_ref(),
        parent_ordinal: audit.parent_ordinal,
        depth: audit.depth,
        suppressed_count: audit.suppressed_count,
    })
}

pub fn generation_attempt_derived_event_sha256(
    event: &InteractionEvent,
) -> CoreResult<Sha256Digest> {
    hash_json(&("lorepia.generation-attempt-derived-event.v1", event))
}

pub fn generation_attempt_derived_transition_commit_sha256(
    generation_id: &GenerationId,
    transition: &GenerationAttemptDerivedTransition,
) -> CoreResult<Sha256Digest> {
    #[derive(Serialize)]
    #[serde(deny_unknown_fields)]
    struct CommitFingerprint<'a> {
        schema_version: u32,
        generation_id: &'a GenerationId,
        ordinal: u32,
        parent_ordinal: Option<u32>,
        depth: u32,
        event_id: &'a str,
        event: &'a InteractionEvent,
        event_sha256: &'a Sha256Digest,
        deterministic_seed: u64,
        expected_state_revision: u64,
        resulting_state_revision: u64,
        policy: &'a InteractionPolicySnapshot,
        evaluation_seal_sha256: &'a Sha256Digest,
        next_state: &'a InteractionState,
        knowledge: &'a [InteractionKnowledgeBinding],
        action_results: &'a [InteractionActionResultWrite],
        effects: &'a [InteractionEffect],
        derived_events: &'a [InteractionDerivedEventWrite],
        proposals: &'a [InteractionProposalWrite],
    }

    validate_generation_attempt_derived_transition(transition)?;
    let evaluation_seal_sha256 = interaction_evaluation_seal_sha256(&transition.evaluation_seal)?;
    hash_json(&CommitFingerprint {
        schema_version: 1,
        generation_id,
        ordinal: transition.ordinal,
        parent_ordinal: transition.parent_ordinal,
        depth: transition.depth,
        event_id: &transition.event_id,
        event: &transition.event,
        event_sha256: &transition.event_sha256,
        deterministic_seed: transition.deterministic_seed,
        expected_state_revision: transition.expected_state_revision,
        resulting_state_revision: transition.resulting_state_revision,
        policy: &transition.policy,
        evaluation_seal_sha256: &evaluation_seal_sha256,
        next_state: &transition.next_state,
        knowledge: &transition.knowledge,
        action_results: &transition.action_results,
        effects: &transition.effects,
        derived_events: &transition.derived_events,
        proposals: &transition.proposals,
    })
}

pub fn generation_attempt_derived_chain_sha256(
    closure: &GenerationAttemptDerivedClosure,
) -> CoreResult<Sha256Digest> {
    #[derive(Serialize)]
    struct ChainFingerprint<'a> {
        schema_version: u32,
        transitions: &'a [GenerationAttemptDerivedTransition],
        guard_audits: &'a [GenerationAttemptDerivedGuardAudit],
        final_state: &'a InteractionState,
        final_knowledge: &'a [InteractionKnowledgeBinding],
        event_count: u32,
        guard_count: u32,
    }

    hash_json(&ChainFingerprint {
        schema_version: closure.schema_version,
        transitions: &closure.transitions,
        guard_audits: &closure.guard_audits,
        final_state: &closure.final_state,
        final_knowledge: &closure.final_knowledge,
        event_count: closure.event_count,
        guard_count: closure.guard_count,
    })
}

pub fn generation_attempt_derived_closure_sha256(
    closure: &GenerationAttemptDerivedClosure,
) -> CoreResult<Sha256Digest> {
    validate_generation_attempt_derived_closure(closure)?;
    hash_json(closure)
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn require_dispatch_ready_attempt(
    transaction: &Transaction<'_>,
    generation_id: &GenerationId,
    conversation_id: &ConversationId,
    source_branch_id: &ConversationBranchId,
    proposed_branch_id: &ConversationBranchId,
    expected_head_message_id: Option<&MessageId>,
    module_plan_sha256: &Sha256Digest,
    prompt_plan_sha256: &Sha256Digest,
    prompt_input_fingerprint_sha256: &Sha256Digest,
) -> CoreResult<StoredGenerationAttempt> {
    let attempt = read_attempt(transaction, generation_id)?;
    let seal = attempt.dispatch_seal.as_ref();
    if attempt.status != GenerationAttemptStatus::DispatchReady
        || attempt.input.conversation_id != *conversation_id
        || attempt.input.source_branch_id != *source_branch_id
        || attempt.input.proposed_branch_id != *proposed_branch_id
        || attempt.input.expected_head_message_id.as_ref() != expected_head_message_id
        || attempt.input.module_plan_sha256 != *module_plan_sha256
        || seal.map(|seal| &seal.final_prompt_plan_sha256) != Some(prompt_plan_sha256)
        || seal.map(|seal| &seal.final_prompt_input_fingerprint_sha256)
            != Some(prompt_input_fingerprint_sha256)
    {
        return Err(CoreError::new(
            CoreErrorCode::InvalidInput,
            "generation append does not match its dispatch-ready seal",
            true,
        ));
    }
    Ok(attempt)
}

pub(crate) fn mark_attempt_running_in_transaction(
    transaction: &Transaction<'_>,
    attempt: &StoredGenerationAttempt,
    started_at: DateTime<Utc>,
) -> CoreResult<()> {
    transition_attempt_in_transaction(
        transaction,
        &attempt.generation_id,
        attempt.revision,
        &[GenerationAttemptStatus::DispatchReady],
        GenerationAttemptStatus::Running,
        None,
        started_at,
    )
}

pub(crate) fn mark_attempt_completed_if_present_in_transaction(
    transaction: &Transaction<'_>,
    generation_id: &GenerationId,
    completed_at: DateTime<Utc>,
) -> CoreResult<bool> {
    let exists = transaction
        .query_row(
            "SELECT EXISTS(
                 SELECT 1 FROM generation_attempt_intents
                 WHERE generation_id = ?1
             )",
            [generation_id.0.as_str()],
            |row| row.get::<_, bool>(0),
        )
        .map_err(storage_db_error)?;
    if exists {
        let attempt = read_attempt(transaction, generation_id)?;
        // `Completed` covers every durable terminal provider outcome; the
        // linked generations row distinguishes complete/cancelled/failed.
        transition_attempt_in_transaction(
            transaction,
            generation_id,
            attempt.revision,
            &[GenerationAttemptStatus::Running],
            GenerationAttemptStatus::Completed,
            None,
            completed_at,
        )?;
    }
    Ok(exists)
}

pub(crate) fn transition_attempt_in_transaction(
    transaction: &Transaction<'_>,
    generation_id: &GenerationId,
    expected_revision: u64,
    expected_statuses: &[GenerationAttemptStatus],
    next_status: GenerationAttemptStatus,
    failure_code: Option<&str>,
    updated_at: DateTime<Utc>,
) -> CoreResult<()> {
    if expected_revision == 0 || expected_statuses.is_empty() {
        return Err(CoreError::invalid(
            "generation attempt transition requires an expected revision and status",
        ));
    }
    if let Some(code) = failure_code {
        validate_failure_code(code)?;
    }
    if (next_status == GenerationAttemptStatus::FailedBeforeDispatch) != failure_code.is_some() {
        return Err(CoreError::invalid(
            "only failed-before-dispatch attempts carry a failure code",
        ));
    }
    let current = read_attempt(transaction, generation_id)?;
    if current.revision != expected_revision || !expected_statuses.contains(&current.status) {
        return Err(attempt_conflict(generation_id));
    }
    let changed = transaction
        .execute(
            "UPDATE generation_attempt_intents
             SET status = ?2, revision = revision + 1,
                 failure_code = ?3, updated_at = ?4
             WHERE generation_id = ?1
               AND revision = ?5 AND status = ?6",
            params![
                generation_id.0,
                next_status.as_str(),
                failure_code,
                updated_at.to_rfc3339(),
                i64_from_u64(expected_revision)?,
                current.status.as_str(),
            ],
        )
        .map_err(storage_db_error)?;
    if changed == 1 {
        Ok(())
    } else {
        Err(attempt_conflict(generation_id))
    }
}

fn validate_source_snapshot(
    transaction: &Transaction<'_>,
    input: &GenerationAttemptInput,
) -> CoreResult<()> {
    let stored_head = transaction
        .query_row(
            "SELECT head_message_id
             FROM conversation_branches
             WHERE conversation_id = ?1 AND id = ?2",
            params![input.conversation_id.0, input.source_branch_id.0],
            |row| row.get::<_, Option<String>>(0),
        )
        .optional()
        .map_err(storage_db_error)?
        .ok_or_else(|| not_found("generation attempt source branch"))?;
    if stored_head.as_deref() != optional_message(input.expected_head_message_id.as_ref()) {
        return Err(CoreError::new(
            CoreErrorCode::InvalidInput,
            "generation attempt source branch head changed",
            true,
        ));
    }
    validate_context_is_ancestor(transaction, input)?;
    if input.proposed_branch_id != input.source_branch_id {
        let target_exists = transaction
            .query_row(
                "SELECT EXISTS(
                     SELECT 1 FROM conversation_branches WHERE id = ?1
                 )",
                [input.proposed_branch_id.0.as_str()],
                |row| row.get::<_, bool>(0),
            )
            .map_err(storage_db_error)?;
        if target_exists {
            return Err(CoreError::invalid(
                "generation attempt proposed branch already exists",
            ));
        }
    }
    Ok(())
}

fn validate_generation_prompt_character_row(
    transaction: &Transaction<'_>,
    input: &GenerationAttemptInput,
) -> CoreResult<()> {
    let authority = input.prompt_selection_authority.as_ref().ok_or_else(|| {
        CoreError::invalid("generation attempt prompt selection authority is missing")
    })?;
    let raw = transaction
        .query_row(
            "SELECT conversation.character_id,
                    character.id, character.name, character.description,
                    character.source_hash, character.avatar_asset_hash,
                    character.created_at
             FROM conversations AS conversation
             JOIN characters AS character
               ON character.id = conversation.character_id
             WHERE conversation.id = ?1",
            [input.conversation_id.0.as_str()],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, String>(4)?,
                    row.get::<_, Option<String>>(5)?,
                    row.get::<_, String>(6)?,
                ))
            },
        )
        .optional()
        .map_err(storage_db_error)?
        .ok_or_else(|| not_found("generation attempt conversation character"))?;
    let stored = Character {
        id: raw.1,
        name: raw.2,
        description: raw.3,
        source_hash: raw.4,
        avatar_asset_hash: raw.5,
        created_at: parse_time("generation prompt character created_at", &raw.6)?,
    };
    if raw.0 != stored.id || stored != authority.character {
        return Err(CoreError::invalid(
            "generation prompt character differs from its conversation authority",
        ));
    }
    Ok(())
}

fn validate_generation_prompt_content_heads(
    transaction: &Transaction<'_>,
    input: &GenerationAttemptInput,
) -> CoreResult<()> {
    let authority = input.prompt_selection_authority.as_ref().ok_or_else(|| {
        CoreError::invalid("generation attempt prompt selection authority is missing")
    })?;
    validate_active_content_revision(
        transaction,
        "generation prompt character content",
        &format!("character-content:{}", authority.character.id),
        "character_content",
        authority.character_content.as_ref(),
    )?;
    if let Some(book) = authority.character_knowledge_book.as_ref() {
        validate_active_content_revision(
            transaction,
            "generation prompt character knowledge book",
            book.value.id.as_str(),
            "knowledge_book",
            Some(book),
        )?;
    }
    Ok(())
}

fn validate_active_content_revision<T: Serialize>(
    transaction: &Transaction<'_>,
    label: &str,
    object_id: &str,
    object_kind: &str,
    expected: Option<&StoredRevision<T>>,
) -> CoreResult<()> {
    let raw = transaction
        .query_row(
            "SELECT state.state_version, state.active_revision_id,
                    object.created_at, state.updated_at, object.deleted_at,
                    revision.document_json, revision.document_sha256
             FROM content_objects AS object
             JOIN content_object_state AS state
               ON state.object_id = object.id
             JOIN content_revisions AS revision
               ON revision.object_id = object.id
              AND revision.id = state.active_revision_id
              AND revision.object_kind = object.object_kind
             WHERE object.id = ?1
               AND object.object_kind = ?2
               AND object.deleted_at IS NULL",
            params![object_id, object_kind],
            |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, Option<String>>(4)?,
                    row.get::<_, String>(5)?,
                    row.get::<_, String>(6)?,
                ))
            },
        )
        .optional()
        .map_err(storage_db_error)?;
    let Some(expected) = expected else {
        if raw.is_some() {
            return Err(CoreError::invalid(format!(
                "{label} was omitted from its active authority"
            )));
        }
        return Ok(());
    };
    let raw = raw.ok_or_else(|| CoreError::invalid(format!("{label} is not active")))?;
    let document_json = serde_json::to_string(&expected.value)
        .map_err(|error| CoreError::invalid(format!("{label} cannot be canonicalized: {error}")))?;
    let document_sha256 = format!("{:x}", Sha256::digest(document_json.as_bytes()));
    if expected.revision != u64_from_i64(raw.0)?
        || expected.revision_id.as_deref() != Some(raw.1.as_str())
        || expected.created_at != parse_time(&format!("{label} created_at"), &raw.2)?
        || expected.updated_at != parse_time(&format!("{label} updated_at"), &raw.3)?
        || expected.deleted_at.is_some()
        || raw.4.is_some()
        || document_json != raw.5
        || document_sha256 != raw.6
    {
        return Err(CoreError::invalid(format!(
            "{label} differs from its active immutable revision"
        )));
    }
    Ok(())
}

fn validate_context_is_ancestor(
    transaction: &Transaction<'_>,
    input: &GenerationAttemptInput,
) -> CoreResult<()> {
    match (
        input.expected_head_message_id.as_ref(),
        input.context_head_message_id.as_ref(),
    ) {
        (None, None) => Ok(()),
        (Some(expected), Some(context)) => {
            let is_ancestor = transaction
                .query_row(
                    "WITH RECURSIVE lineage(id, parent_id, depth) AS (
                         SELECT id, parent_id, 0
                         FROM messages
                         WHERE conversation_id = ?1 AND id = ?2
                         UNION ALL
                         SELECT parent.id, parent.parent_id, lineage.depth + 1
                         FROM messages AS parent
                         JOIN lineage ON parent.id = lineage.parent_id
                         WHERE parent.conversation_id = ?1
                           AND lineage.depth < 4095
                     )
                     SELECT EXISTS(
                         SELECT 1 FROM lineage WHERE id = ?3
                     )",
                    params![input.conversation_id.0, expected.0, context.0],
                    |row| row.get::<_, bool>(0),
                )
                .map_err(storage_db_error)?;
            if is_ancestor {
                Ok(())
            } else {
                Err(CoreError::invalid(
                    "generation context head is not in the source lineage",
                ))
            }
        }
        (Some(_), None) if input.proposed_branch_id != input.source_branch_id => Ok(()),
        (None, Some(_)) | (Some(_), None) => Err(CoreError::invalid(
            "generation context may be empty below a fork source head only",
        )),
    }
}

pub(crate) fn read_attempt(
    connection: &Connection,
    generation_id: &GenerationId,
) -> CoreResult<StoredGenerationAttempt> {
    let row = query_attempt_row(connection, generation_id)?;
    let before = decode_hashed::<GenerationBeforeEventEvidence>(
        "before-generation evidence",
        row.before_generation_evidence_json.as_deref(),
        row.before_generation_evidence_sha256.as_deref(),
    )?;
    let approval = decode_hashed::<GenerationApprovalEvidence>(
        "generation approval evidence",
        row.approval_evidence_json.as_deref(),
        row.approval_evidence_sha256.as_deref(),
    )?;
    let dispatch = decode_hashed::<GenerationDispatchSeal>(
        "generation dispatch seal",
        row.dispatch_seal_json.as_deref(),
        row.dispatch_seal_sha256.as_deref(),
    )?;
    let prompt_selection_authority = decode_prompt_selection_authority(
        row.prompt_selection_authority_json.as_deref(),
        row.prompt_selection_authority_sha256.as_deref(),
        row.prompt_selection_authority_version,
    )?;
    let (module_runtime_review_authority, applied_runtime_plan_authority) =
        decode_module_runtime_authority(
            row.module_runtime_review_authority_json.as_deref(),
            row.module_runtime_review_authority_sha256.as_deref(),
            row.applied_runtime_plan_authority_json.as_deref(),
            row.applied_runtime_plan_authority_sha256.as_deref(),
            row.module_runtime_authority_version,
        )?;
    let stored = StoredGenerationAttempt {
        generation_id: generation_id.clone(),
        input: GenerationAttemptInput {
            operation_id: row.operation_id,
            conversation_id: ConversationId(row.conversation_id),
            source_branch_id: ConversationBranchId(row.source_branch_id),
            proposed_branch_id: ConversationBranchId(row.proposed_branch_id),
            expected_head_message_id: row.expected_head_message_id.map(MessageId),
            context_head_message_id: row.context_head_message_id.map(MessageId),
            module_plan_sha256: parse_sha("module plan", &row.module_plan_sha256)?,
            base_request_fingerprint_sha256: parse_sha(
                "base request",
                &row.base_input_fingerprint_sha256,
            )?,
            prompt_selection_authority,
            module_runtime_review_authority,
            applied_runtime_plan_authority,
        },
        attempt_sha256: parse_sha("generation attempt", &row.attempt_sha256)?,
        status: GenerationAttemptStatus::parse(&row.status)?,
        revision: u64_from_i64(row.revision)?,
        before_generation_evidence: before.as_ref().map(|value| value.0.clone()),
        before_generation_evidence_sha256: before.map(|value| value.1),
        approval_evidence: approval.as_ref().map(|value| value.0.clone()),
        approval_evidence_sha256: approval.map(|value| value.1),
        dispatch_seal: dispatch.as_ref().map(|value| value.0.clone()),
        dispatch_seal_sha256: dispatch.map(|value| value.1),
        failure_code: row.failure_code,
        created_at: parse_time("generation attempt created_at", &row.created_at)?,
        updated_at: parse_time("generation attempt updated_at", &row.updated_at)?,
    };
    require_valid_stored_attempt_identity(&stored)?;
    Ok(stored)
}

fn query_attempt_row(
    connection: &Connection,
    generation_id: &GenerationId,
) -> CoreResult<GenerationAttemptRow> {
    connection
        .query_row(
            "SELECT operation_id, conversation_id, source_branch_id,
                    proposed_branch_id, expected_head_message_id,
                    context_head_message_id, module_plan_sha256,
                    base_input_fingerprint_sha256, attempt_sha256,
                    status, revision,
                    before_generation_evidence_json,
                    before_generation_evidence_sha256,
                    approval_evidence_json, approval_evidence_sha256,
                    dispatch_seal_json, dispatch_seal_sha256,
                    failure_code, created_at, updated_at,
                    prompt_selection_authority_json,
                    prompt_selection_authority_sha256,
                    prompt_selection_authority_version,
                    module_runtime_review_authority_json,
                    module_runtime_review_authority_sha256,
                    applied_runtime_plan_authority_json,
                    applied_runtime_plan_authority_sha256,
                    module_runtime_authority_version
             FROM generation_attempt_intents
             WHERE generation_id = ?1",
            [generation_id.0.as_str()],
            |row| {
                Ok(GenerationAttemptRow {
                    operation_id: row.get(0)?,
                    conversation_id: row.get(1)?,
                    source_branch_id: row.get(2)?,
                    proposed_branch_id: row.get(3)?,
                    expected_head_message_id: row.get(4)?,
                    context_head_message_id: row.get(5)?,
                    module_plan_sha256: row.get(6)?,
                    base_input_fingerprint_sha256: row.get(7)?,
                    attempt_sha256: row.get(8)?,
                    status: row.get(9)?,
                    revision: row.get(10)?,
                    before_generation_evidence_json: row.get(11)?,
                    before_generation_evidence_sha256: row.get(12)?,
                    approval_evidence_json: row.get(13)?,
                    approval_evidence_sha256: row.get(14)?,
                    dispatch_seal_json: row.get(15)?,
                    dispatch_seal_sha256: row.get(16)?,
                    failure_code: row.get(17)?,
                    created_at: row.get(18)?,
                    updated_at: row.get(19)?,
                    prompt_selection_authority_json: row.get(20)?,
                    prompt_selection_authority_sha256: row.get(21)?,
                    prompt_selection_authority_version: row.get(22)?,
                    module_runtime_review_authority_json: row.get(23)?,
                    module_runtime_review_authority_sha256: row.get(24)?,
                    applied_runtime_plan_authority_json: row.get(25)?,
                    applied_runtime_plan_authority_sha256: row.get(26)?,
                    module_runtime_authority_version: row.get(27)?,
                })
            },
        )
        .optional()
        .map_err(storage_db_error)?
        .ok_or_else(|| not_found("generation attempt"))
}

fn decode_prompt_selection_authority(
    authority_json: Option<&str>,
    authority_sha256: Option<&str>,
    authority_version: i64,
) -> CoreResult<Option<GenerationPromptSelectionAuthority>> {
    Ok(
        match (authority_json, authority_sha256, authority_version) {
            (None, None, 0) => None,
            (Some(json), Some(expected_sha256), 1) => {
                let decoded = decode_hashed::<GenerationPromptSelectionAuthority>(
                    "generation prompt selection authority",
                    Some(json),
                    Some(expected_sha256),
                )?
                .ok_or_else(|| corrupted("generation prompt selection authority is missing"))?;
                validate_generation_prompt_selection_authority(&decoded.0)?;
                let canonical = serde_json::to_string(&decoded.0).map_err(|error| {
                    corrupted(format!(
                        "generation prompt selection authority cannot be canonicalized: {error}"
                    ))
                })?;
                if canonical != json {
                    return Err(corrupted(
                        "generation prompt selection authority JSON is not canonical",
                    ));
                }
                Some(decoded.0)
            }
            _ => {
                return Err(corrupted(
                    "generation prompt selection authority columns are incomplete",
                ));
            }
        },
    )
}

fn decode_module_runtime_authority(
    review_json: Option<&str>,
    review_sha256: Option<&str>,
    plan_json: Option<&str>,
    plan_sha256: Option<&str>,
    authority_version: i64,
) -> CoreResult<(Option<ModuleMergeReview>, Option<AppliedModuleRuntimePlan>)> {
    match (
        review_json,
        review_sha256,
        plan_json,
        plan_sha256,
        authority_version,
    ) {
        (None, None, None, None, 0) => Ok((None, None)),
        (Some(review_json), Some(review_sha256), plan_json, plan_sha256, 1) => {
            let review = decode_hashed::<ModuleMergeReview>(
                "generation module runtime review authority",
                Some(review_json),
                Some(review_sha256),
            )?
            .ok_or_else(|| corrupted("generation module runtime review authority is missing"))?
            .0;
            review.verify().map_err(|error| {
                corrupted(format!(
                    "generation module runtime review authority is invalid: {error}"
                ))
            })?;
            if serde_json::to_string(&review).map_err(|error| {
                corrupted(format!(
                    "generation module runtime review authority cannot be canonicalized: {error}"
                ))
            })? != review_json
            {
                return Err(corrupted(
                    "generation module runtime review authority JSON is not canonical",
                ));
            }
            let plan = match (plan_json, plan_sha256) {
                (None, None) => None,
                (Some(plan_json), Some(plan_sha256)) => {
                    let plan = decode_hashed::<AppliedModuleRuntimePlan>(
                        "generation applied runtime plan authority",
                        Some(plan_json),
                        Some(plan_sha256),
                    )?
                    .ok_or_else(|| {
                        corrupted("generation applied runtime plan authority is missing")
                    })?
                    .0;
                    plan.verify().map_err(|error| {
                        corrupted(format!(
                            "generation applied runtime plan authority is invalid: {error}"
                        ))
                    })?;
                    if serde_json::to_string(&plan).map_err(|error| {
                        corrupted(format!(
                            "generation applied runtime plan authority cannot be canonicalized: {error}"
                        ))
                    })? != plan_json
                    {
                        return Err(corrupted(
                            "generation applied runtime plan authority JSON is not canonical",
                        ));
                    }
                    Some(plan)
                }
                _ => {
                    return Err(corrupted(
                        "generation applied runtime plan authority columns are incomplete",
                    ));
                }
            };
            if plan.as_ref().is_some_and(|plan| plan.review != review) {
                return Err(corrupted(
                    "generation applied runtime plan authority differs from its review",
                ));
            }
            Ok((Some(review), plan))
        }
        _ => Err(corrupted(
            "generation module runtime authority columns are incomplete",
        )),
    }
}

fn require_valid_stored_attempt_identity(stored: &StoredGenerationAttempt) -> CoreResult<()> {
    if generation_attempt_sha256(&stored.input)? != stored.attempt_sha256
        || deterministic_generation_id(&stored.attempt_sha256) != stored.generation_id
    {
        return Err(corrupted(
            "stored generation attempt identity failed deterministic verification",
        ));
    }
    Ok(())
}

fn validate_input(input: &GenerationAttemptInput) -> CoreResult<()> {
    validate_id("generation operation", &input.operation_id)?;
    validate_id("conversation", &input.conversation_id.0)?;
    validate_id("source branch", &input.source_branch_id.0)?;
    validate_id("proposed branch", &input.proposed_branch_id.0)?;
    if let Some(id) = &input.expected_head_message_id {
        validate_id("expected head message", &id.0)?;
    }
    if let Some(id) = &input.context_head_message_id {
        validate_id("context head message", &id.0)?;
    }
    if input.proposed_branch_id == input.source_branch_id {
        if input.context_head_message_id != input.expected_head_message_id {
            return Err(CoreError::invalid(
                "same-branch generation context must equal its expected head",
            ));
        }
    } else if deterministic_proposed_branch_id(
        &input.operation_id,
        &input.conversation_id,
        &input.source_branch_id,
        input.context_head_message_id.as_ref(),
    )? != input.proposed_branch_id
    {
        return Err(CoreError::invalid(
            "fork generation proposed branch id is not deterministic",
        ));
    }
    if let Some(authority) = input.prompt_selection_authority.as_ref() {
        validate_generation_prompt_selection_authority(authority)?;
        if authority
            .persona_selection
            .as_ref()
            .is_some_and(|selection| selection.value.conversation_id != input.conversation_id)
        {
            return Err(CoreError::invalid(
                "generation prompt persona selection belongs to another conversation",
            ));
        }
    }
    validate_generation_module_runtime_authority(input)?;
    Ok(())
}

fn validate_generation_module_runtime_authority(input: &GenerationAttemptInput) -> CoreResult<()> {
    let Some(review) = input.module_runtime_review_authority.as_ref() else {
        if input.prompt_selection_authority.is_some()
            || input.applied_runtime_plan_authority.is_some()
        {
            return Err(CoreError::invalid(
                "generation module runtime authority is missing",
            ));
        }
        return Ok(());
    };
    let prompt = input.prompt_selection_authority.as_ref().ok_or_else(|| {
        CoreError::invalid("generation module runtime authority has no prompt authority")
    })?;
    review.verify().map_err(|error| {
        CoreError::invalid(format!(
            "generation module runtime review authority is invalid: {error}"
        ))
    })?;
    if review.context.conversation_id.as_deref() != Some(input.conversation_id.0.as_str())
        || review.context.branch_id.as_deref() != Some(input.proposed_branch_id.0.as_str())
        || review.context.character_id.as_deref() != Some(prompt.character.id.as_str())
        || review.context.persona_id.as_ref()
            != prompt
                .persona_selection
                .as_ref()
                .map(|selection| &selection.value.persona_id)
        || prompt_local_user_id_sha256(&review.context.local_user_id) != prompt.local_user_id_sha256
    {
        return Err(CoreError::invalid(
            "generation module runtime review context differs from its attempt authority",
        ));
    }
    match input.applied_runtime_plan_authority.as_ref() {
        Some(plan) => {
            plan.verify().map_err(|error| {
                CoreError::invalid(format!(
                    "generation applied runtime plan authority is invalid: {error}"
                ))
            })?;
            if plan.review != *review || plan.applied_plan_sha256 != input.module_plan_sha256 {
                return Err(CoreError::invalid(
                    "generation applied runtime plan differs from its attempt authority",
                ));
            }
        }
        None => {
            if input.module_plan_sha256 != no_applied_module_runtime_plan_sha256()
                || !review.ordered_bindings.is_empty()
            {
                return Err(CoreError::invalid(
                    "generation no-module runtime authority is inconsistent",
                ));
            }
        }
    }
    Ok(())
}

fn validate_generation_prompt_selection_authority(
    authority: &GenerationPromptSelectionAuthority,
) -> CoreResult<()> {
    authority.preset.validate().map_err(|error| {
        CoreError::invalid(format!("generation prompt preset is invalid: {error}"))
    })?;
    if authority.schema_version != 1
        || !is_canonical_sha256(&authority.local_user_id_sha256)
        || authority.preset_revision == 0
        || authority.preset_revision_id.trim() != authority.preset_revision_id
        || authority.preset_revision_id.is_empty()
        || authority
            .explicit_preset_id
            .as_ref()
            .is_some_and(|id| id != &authority.preset.id)
    {
        return Err(CoreError::invalid(
            "generation prompt selection identity is invalid",
        ));
    }
    validate_generation_prompt_character_authority(authority)?;
    validate_generation_prompt_quick_settings(authority)?;
    if let Some(provider_target) = authority.provider_target_authority.as_ref() {
        validate_generation_provider_target_authority(provider_target)?;
    }
    if let Some(binding) = authority.binding.as_ref()
        && (binding.revision == 0
            || binding.deleted_at.is_some()
            || !binding.value.enabled
            || binding.value.prompt_preset_id != authority.preset.id
            || binding
                .value
                .pinned_revision_id
                .as_deref()
                .is_some_and(|revision| revision != authority.preset_revision_id))
    {
        return Err(CoreError::invalid(
            "generation prompt binding authority is invalid",
        ));
    }
    if authority
        .persona_selection
        .as_ref()
        .is_some_and(|selection| selection.revision == 0 || selection.deleted_at.is_some())
    {
        return Err(CoreError::invalid(
            "generation prompt persona selection authority is invalid",
        ));
    }
    Ok(())
}

fn validate_generation_provider_target_authority(
    authority: &GenerationProviderTargetAuthority,
) -> CoreResult<()> {
    let valid = match authority {
        GenerationProviderTargetAuthority::ProviderProfile {
            provider_profile_id,
            dispatch_snapshot_sha256,
        } => {
            validate_id("generation provider profile", provider_profile_id)?;
            is_canonical_sha256(dispatch_snapshot_sha256.as_str())
        }
        GenerationProviderTargetAuthority::GenerationTarget {
            target,
            resolved_snapshot_sha256,
        } => {
            validate_id("generation model route", target.model_route_id.as_str())?;
            validate_id(
                "generation provider preset",
                target.generation_preset_id.as_str(),
            )?;
            is_canonical_sha256(resolved_snapshot_sha256.as_str())
        }
        GenerationProviderTargetAuthority::DirectModel { model_sha256 } => {
            is_canonical_sha256(model_sha256.as_str())
        }
    };
    if valid {
        Ok(())
    } else {
        Err(CoreError::invalid(
            "generation provider target authority is invalid",
        ))
    }
}

fn validate_generation_prompt_character_authority(
    authority: &GenerationPromptSelectionAuthority,
) -> CoreResult<()> {
    let capability_keys = authority
        .supported_capabilities
        .iter()
        .map(|capability| {
            serde_json::to_string(capability).map_err(|error| {
                CoreError::internal(format!(
                    "generation prompt capability cannot be encoded: {error}"
                ))
            })
        })
        .collect::<CoreResult<Vec<_>>>()?;
    validate_id("generation prompt character", &authority.character.id)?;
    if authority.character.name.trim() != authority.character.name
        || authority.character.name.is_empty()
        || !is_canonical_sha256(&authority.character.source_hash)
        || !capability_keys.windows(2).all(|pair| pair[0] < pair[1])
    {
        return Err(CoreError::invalid(
            "generation prompt character authority is invalid",
        ));
    }
    if let Some(content) = authority.character_content.as_ref()
        && (content.revision == 0
            || content.revision_id.as_deref().is_none_or(str::is_empty)
            || content.deleted_at.is_some()
            || content.value.schema_version != 1)
    {
        return Err(CoreError::invalid(
            "generation prompt character content authority is invalid",
        ));
    }
    match (
        authority
            .character_content
            .as_ref()
            .and_then(|content| content.value.knowledge_book.as_ref())
            .and_then(|reference| reference.id.as_ref()),
        authority.character_knowledge_book.as_ref(),
    ) {
        (None, None) => {}
        (Some(reference_id), Some(book))
            if book.revision > 0
                && book.revision_id.as_deref().is_some_and(|id| !id.is_empty())
                && book.deleted_at.is_none()
                && &book.value.id == reference_id =>
        {
            book.value.validate().map_err(|error| {
                CoreError::invalid(format!(
                    "generation prompt character knowledge authority is invalid: {error}"
                ))
            })?;
        }
        _ => {
            return Err(CoreError::invalid(
                "generation prompt character knowledge authority is inconsistent",
            ));
        }
    }
    Ok(())
}

fn validate_generation_prompt_quick_settings(
    authority: &GenerationPromptSelectionAuthority,
) -> CoreResult<()> {
    let quick = &authority.quick_settings;
    if quick.creativity > 100
        || quick
            .resolved_temperature
            .is_some_and(|temperature| !temperature.is_finite())
        || (!quick.supports_temperature && quick.resolved_temperature.is_some())
        || quick
            .resolved_max_output_tokens
            .is_some_and(|tokens| tokens == 0 || tokens > 10_000_000)
    {
        return Err(CoreError::invalid(
            "generation prompt quick-settings authority is invalid",
        ));
    }
    let binding = authority.binding.as_ref().map(|binding| &binding.value);
    if quick.response_length
        != binding.map_or(PromptResponseLength::Balanced, |value| {
            value.response_length
        })
        || quick.creativity != binding.map_or(50, |value| value.creativity)
        || quick.reasoning_effort != binding.and_then(|value| value.reasoning_effort)
        || quick.memory_enabled != binding.is_none_or(|value| value.memory_enabled)
        || quick.knowledge_enabled != binding.is_none_or(|value| value.knowledge_enabled)
    {
        return Err(CoreError::invalid(
            "generation prompt quick settings differ from their binding authority",
        ));
    }
    Ok(())
}

fn is_canonical_sha256(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn validate_before_evidence(evidence: &GenerationBeforeEventEvidence) -> CoreResult<()> {
    validate_id("before-generation event", &evidence.event_id)?;
    if evidence.context_state_revision == 0 {
        return Err(CoreError::invalid(
            "before-generation evidence requires a durable state revision",
        ));
    }
    if evidence.proposal_review_sha256s.is_empty() && evidence.awaiting_approval {
        return Err(CoreError::invalid(
            "awaiting-approval evidence requires a reviewed proposal",
        ));
    }
    Ok(())
}

fn validate_approval_evidence(evidence: &GenerationApprovalEvidence) -> CoreResult<()> {
    if evidence.decision_event_ids.len() != evidence.decision_event_sha256s.len()
        || evidence.resulting_state_revision == 0
    {
        return Err(CoreError::invalid(
            "generation approval evidence is incomplete",
        ));
    }
    for id in &evidence.decision_event_ids {
        validate_id("approval decision event", id)?;
    }
    Ok(())
}

fn validate_dispatch_seal(seal: &GenerationDispatchSeal) -> CoreResult<()> {
    if seal.final_interaction_state_revision == 0
        || seal.derived_chain_sha256.is_none()
        || !matches!(seal.derived_event_count, Some(1..=256))
        || !matches!(seal.derived_guard_count, Some(0..=1_024))
    {
        Err(CoreError::invalid(
            "generation dispatch seal requires durable interaction and derived closure authority",
        ))
    } else {
        Ok(())
    }
}

fn validate_interaction_evaluation_seal(seal: &InteractionEvaluationSeal) -> CoreResult<()> {
    if seal.schema_version != 1
        || seal.engine_contract_version != 1
        || seal.seed_contract_version != 1
        || seal.limits.max_rule_sets == 0
        || seal.limits.max_rules == 0
        || seal.limits.max_actions_per_event == 0
        || seal.limits.max_actions_per_rule == 0
        || seal.limits.max_condition_depth == 0
        || seal.limits.max_condition_nodes == 0
        || seal.limits.max_template_depth == 0
        || seal.limits.max_template_parts == 0
        || seal.limits.max_variables == 0
        || seal.limits.max_proposals == 0
        || seal.limits.max_pending_proposals == 0
        || seal.limits.max_pending_proposals > seal.limits.max_proposals
        || seal.limits.max_effects == 0
        || seal.limits.max_choices == 0
        || seal.limits.max_dice_count == 0
        || seal.limits.max_dice_sides < 2
        || seal.limits.max_text_chars == 0
        || seal.limits.max_identifier_bytes == 0
    {
        return Err(CoreError::invalid(
            "interaction evaluation seal version or limits are invalid",
        ));
    }
    if !seal
        .knowledge_revisions
        .windows(2)
        .all(|pair| pair[0].entry_id < pair[1].entry_id)
        || !seal.asset_action_diagnostics.windows(2).all(|pair| {
            (&pair[0].rule_id, pair[0].action_ordinal) < (&pair[1].rule_id, pair[1].action_ordinal)
        })
        || !seal
            .approved_import_source_ids
            .windows(2)
            .all(|pair| pair[0] < pair[1])
    {
        return Err(CoreError::invalid(
            "interaction evaluation seal collections are not canonical",
        ));
    }
    Ok(())
}

fn validate_generation_attempt_derived_transition(
    transition: &GenerationAttemptDerivedTransition,
) -> CoreResult<()> {
    validate_id("generation derived transition event", &transition.event_id)?;
    validate_interaction_evaluation_seal(&transition.evaluation_seal)?;
    let policy_sha256 = Sha256Digest::parse(interaction_policy_sha256(&transition.policy)?)
        .map_err(CoreError::invalid)?;
    if transition.depth > 16
        || transition.ordinal > 256
        || transition.resulting_state_revision
            != transition
                .expected_state_revision
                .checked_add(1)
                .ok_or_else(|| CoreError::invalid("derived transition revision overflowed"))?
        || transition.next_state.revision != transition.resulting_state_revision
        || transition.parent_ordinal.is_none() != (transition.ordinal == 0)
        || transition
            .parent_ordinal
            .is_some_and(|parent| parent >= transition.ordinal)
        || transition.event_sha256 != generation_attempt_derived_event_sha256(&transition.event)?
        || transition.evaluation_seal.policy_sha256 != policy_sha256
    {
        return Err(CoreError::invalid(
            "generation derived transition authority is invalid",
        ));
    }
    Ok(())
}

fn validate_generation_attempt_derived_closure(
    closure: &GenerationAttemptDerivedClosure,
) -> CoreResult<()> {
    let Some(root) = closure.transitions.first() else {
        return Err(CoreError::invalid(
            "generation attempt derived closure is invalid",
        ));
    };
    if closure.schema_version != 1
        || closure.event_count == 0
        || closure.event_count > 256
        || closure.guard_count > 1_024
        || usize::try_from(closure.event_count).ok() != Some(closure.transitions.len())
        || usize::try_from(closure.guard_count).ok() != Some(closure.guard_audits.len())
        || root.ordinal != 0
        || root.parent_ordinal.is_some()
        || root.depth != 0
    {
        return Err(CoreError::invalid(
            "generation attempt derived closure is invalid",
        ));
    }
    let mut remaining_edges = BTreeMap::<(u32, Sha256Digest, u64), u32>::new();
    for transition in &closure.transitions {
        for derived in &transition.derived_events {
            let event_sha256 = generation_attempt_derived_event_sha256(&derived.event)?;
            let count = remaining_edges
                .entry((transition.ordinal, event_sha256, derived.deterministic_seed))
                .or_default();
            *count = count
                .checked_add(1)
                .ok_or_else(|| CoreError::invalid("derived edge count overflowed"))?;
        }
    }
    for (ordinal, transition) in closure.transitions.iter().enumerate() {
        if u32::try_from(ordinal).ok() != Some(transition.ordinal)
            || validate_generation_attempt_derived_transition(transition).is_err()
            || transition.evaluation_seal != root.evaluation_seal
            || transition.policy != root.policy
        {
            return Err(CoreError::invalid(
                "generation attempt derived closure is invalid",
            ));
        }
        if ordinal == 0 {
            continue;
        }
        let parent_ordinal = transition
            .parent_ordinal
            .ok_or_else(|| CoreError::invalid("generation derived child has no parent ordinal"))?;
        let parent =
            closure
                .transitions
                .get(usize::try_from(parent_ordinal).map_err(|_| {
                    CoreError::invalid("generation derived parent ordinal overflowed")
                })?)
                .ok_or_else(|| CoreError::invalid("generation derived parent is missing"))?;
        if transition.depth != parent.depth.saturating_add(1)
            || transition.expected_state_revision
                != closure.transitions[ordinal - 1].resulting_state_revision
            || generation_attempt_derived_ancestry_contains(
                &closure.transitions,
                parent_ordinal,
                &transition.event_sha256,
            )?
        {
            return Err(CoreError::invalid(
                "generation attempt derived closure is invalid",
            ));
        }
        let edge = remaining_edges
            .get_mut(&(
                parent_ordinal,
                transition.event_sha256.clone(),
                transition.deterministic_seed,
            ))
            .filter(|count| **count > 0)
            .ok_or_else(|| {
                CoreError::invalid("generation derived child has no exact parent edge")
            })?;
        *edge -= 1;
    }
    let last = closure.transitions.last().ok_or_else(|| {
        CoreError::invalid("generation attempt derived closure has no final transition")
    })?;
    if last.next_state != closure.final_state
        || last.knowledge != closure.final_knowledge
        || generation_attempt_derived_chain_sha256(closure)? != closure.chain_sha256
    {
        return Err(CoreError::invalid(
            "generation attempt derived closure is invalid",
        ));
    }
    validate_generation_attempt_derived_guards(closure, &mut remaining_edges)?;
    Ok(())
}

fn generation_attempt_derived_ancestry_contains(
    transitions: &[GenerationAttemptDerivedTransition],
    mut ordinal: u32,
    candidate: &Sha256Digest,
) -> CoreResult<bool> {
    loop {
        let transition = transitions
            .get(
                usize::try_from(ordinal)
                    .map_err(|_| CoreError::invalid("derived ancestry ordinal overflowed"))?,
            )
            .ok_or_else(|| CoreError::invalid("derived ancestry transition is missing"))?;
        if &transition.event_sha256 == candidate {
            return Ok(true);
        }
        let Some(parent) = transition.parent_ordinal else {
            return Ok(false);
        };
        if parent >= ordinal {
            return Err(CoreError::invalid("derived ancestry is not acyclic"));
        }
        ordinal = parent;
    }
}

fn validate_generation_attempt_derived_guards(
    closure: &GenerationAttemptDerivedClosure,
    remaining_edges: &mut BTreeMap<(u32, Sha256Digest, u64), u32>,
) -> CoreResult<()> {
    let mut identities = BTreeSet::new();
    let mut count_guards = Vec::new();
    for audit in &closure.guard_audits {
        let parent = closure
            .transitions
            .get(
                usize::try_from(audit.parent_ordinal)
                    .map_err(|_| CoreError::invalid("derived guard parent overflowed"))?,
            )
            .ok_or_else(|| CoreError::invalid("derived guard parent is missing"))?;
        if audit.suppressed_count == 0
            || audit.depth != parent.depth.saturating_add(1)
            || generation_attempt_derived_guard_evidence_sha256(audit)? != audit.evidence_sha256
            || !identities.insert((
                audit.kind,
                audit.candidate_event_sha256.clone(),
                audit.parent_ordinal,
                audit.depth,
            ))
        {
            return Err(CoreError::invalid(
                "generation attempt derived guard authority is invalid",
            ));
        }
        match audit.kind {
            GenerationAttemptDerivedGuardKind::Cycle => {
                let candidate = audit.candidate_event_sha256.as_ref().ok_or_else(|| {
                    CoreError::invalid("cycle guard is missing its candidate event")
                })?;
                if !generation_attempt_derived_ancestry_contains(
                    &closure.transitions,
                    audit.parent_ordinal,
                    candidate,
                )? {
                    return Err(CoreError::invalid(
                        "cycle guard candidate is absent from its ancestry",
                    ));
                }
                consume_generation_attempt_guard_edges(
                    remaining_edges,
                    audit.parent_ordinal,
                    candidate,
                    audit.suppressed_count,
                )?;
            }
            GenerationAttemptDerivedGuardKind::DepthLimit => {
                let candidate = audit.candidate_event_sha256.as_ref().ok_or_else(|| {
                    CoreError::invalid("depth guard is missing its candidate event")
                })?;
                if audit.depth <= 16
                    || generation_attempt_derived_ancestry_contains(
                        &closure.transitions,
                        audit.parent_ordinal,
                        candidate,
                    )?
                {
                    return Err(CoreError::invalid(
                        "depth guard candidate or boundary is invalid",
                    ));
                }
                consume_generation_attempt_guard_edges(
                    remaining_edges,
                    audit.parent_ordinal,
                    candidate,
                    audit.suppressed_count,
                )?;
            }
            GenerationAttemptDerivedGuardKind::CountLimit => {
                if audit.candidate_event_sha256.is_some() || closure.event_count != 256 {
                    return Err(CoreError::invalid(
                        "count guard candidate or boundary is invalid",
                    ));
                }
                count_guards.push(audit);
            }
        }
    }
    for audit in count_guards {
        let mut remaining = audit.suppressed_count;
        for ((parent, _, _), count) in remaining_edges.iter_mut() {
            if *parent == audit.parent_ordinal && remaining > 0 {
                let consumed = (*count).min(remaining);
                *count -= consumed;
                remaining -= consumed;
            }
        }
        if remaining != 0 {
            return Err(CoreError::invalid(
                "count guard has no exact suppressed parent edges",
            ));
        }
    }
    if remaining_edges.values().any(|count| *count != 0) {
        return Err(CoreError::invalid(
            "generation derived closure omitted a parent edge or guard audit",
        ));
    }
    Ok(())
}

fn consume_generation_attempt_guard_edges(
    remaining_edges: &mut BTreeMap<(u32, Sha256Digest, u64), u32>,
    parent_ordinal: u32,
    candidate: &Sha256Digest,
    mut remaining: u32,
) -> CoreResult<()> {
    for ((parent, event_sha256, _), count) in remaining_edges.iter_mut() {
        if *parent == parent_ordinal && event_sha256 == candidate && remaining > 0 {
            let consumed = (*count).min(remaining);
            *count -= consumed;
            remaining -= consumed;
        }
    }
    if remaining != 0 {
        return Err(CoreError::invalid(
            "derived guard has no exact suppressed parent edges",
        ));
    }
    Ok(())
}

fn encode_hashed<T: Serialize>(label: &str, value: &T) -> CoreResult<(String, Sha256Digest)> {
    let json = serde_json::to_string(value).map_err(|error| {
        CoreError::new(
            CoreErrorCode::Internal,
            format!("{label} could not be encoded: {error}"),
            false,
        )
    })?;
    if json.len() > MAX_ATTEMPT_EVIDENCE_BYTES {
        return Err(CoreError::invalid(format!(
            "{label} exceeds its byte limit"
        )));
    }
    let sha256 = Sha256Digest::parse(format!("{:x}", Sha256::digest(json.as_bytes())))
        .map_err(CoreError::invalid)?;
    Ok((json, sha256))
}

fn decode_hashed<T>(
    label: &str,
    json: Option<&str>,
    expected_sha256: Option<&str>,
) -> CoreResult<Option<(T, Sha256Digest)>>
where
    T: for<'de> Deserialize<'de> + Serialize,
{
    match (json, expected_sha256) {
        (None, None) => Ok(None),
        (Some(json), Some(expected)) => {
            let actual = Sha256Digest::parse(format!("{:x}", Sha256::digest(json.as_bytes())))
                .map_err(|error| corrupted(format!("{label} hash is invalid: {error}")))?;
            let expected = parse_sha(label, expected)?;
            if actual != expected {
                return Err(corrupted(format!("{label} hash diverges from its JSON")));
            }
            let value = serde_json::from_str(json)
                .map_err(|error| corrupted(format!("{label} is invalid: {error}")))?;
            Ok(Some((value, expected)))
        }
        _ => Err(corrupted(format!("{label} hash pair is incomplete"))),
    }
}

fn hash_json<T: Serialize>(value: &T) -> CoreResult<Sha256Digest> {
    let bytes = serde_json::to_vec(value).map_err(|error| {
        CoreError::new(
            CoreErrorCode::Internal,
            format!("generation attempt value could not be encoded: {error}"),
            false,
        )
    })?;
    Sha256Digest::parse(format!("{:x}", Sha256::digest(bytes))).map_err(CoreError::invalid)
}

fn parse_sha(label: &str, value: &str) -> CoreResult<Sha256Digest> {
    Sha256Digest::parse(value.to_owned())
        .map_err(|error| corrupted(format!("stored {label} SHA-256 is invalid: {error}")))
}

fn optional_message(value: Option<&MessageId>) -> Option<&str> {
    value.map(|id| id.0.as_str())
}

fn retryable_generation_attempt_projection(
    stored: &StoredGenerationAttempt,
    conversation_id: &ConversationId,
    source_branch_id: &ConversationBranchId,
) -> CoreResult<RetryableGenerationAttemptProjection> {
    if stored.input.conversation_id != *conversation_id
        || stored.input.source_branch_id != *source_branch_id
        || !matches!(
            stored.status,
            GenerationAttemptStatus::BeforeGenerationApplied
                | GenerationAttemptStatus::DispatchReady
        )
    {
        return Err(corrupted(
            "retryable generation attempt escaped its source-room or status boundary",
        ));
    }
    let before = stored.before_generation_evidence.as_ref().ok_or_else(|| {
        corrupted("retryable generation attempt is missing before-generation evidence")
    })?;
    if validate_before_evidence(before).is_err()
        || before.awaiting_approval != stored.approval_evidence.is_some()
    {
        return Err(corrupted(
            "retryable generation attempt has invalid approval authority",
        ));
    }
    if let Some(approval) = stored.approval_evidence.as_ref()
        && (validate_approval_evidence(approval).is_err()
            || stored.before_generation_evidence_sha256.as_ref()
                != Some(&approval.before_event_sha256))
    {
        return Err(corrupted(
            "retryable generation attempt has invalid approval evidence",
        ));
    }
    match (stored.status, stored.dispatch_seal.as_ref()) {
        (GenerationAttemptStatus::BeforeGenerationApplied, None) => {}
        (GenerationAttemptStatus::DispatchReady, Some(seal))
            if validate_dispatch_seal(seal).is_ok()
                && stored.before_generation_evidence_sha256.as_ref()
                    == Some(&seal.before_generation_evidence_sha256)
                && stored.approval_evidence_sha256.as_ref()
                    == seal.approval_evidence_sha256.as_ref()
                && stored.input.module_plan_sha256 == seal.applied_module_plan_sha256 => {}
        _ => {
            return Err(corrupted(
                "retryable generation attempt has invalid dispatch authority",
            ));
        }
    }
    Ok(RetryableGenerationAttemptProjection {
        generation_id: stored.generation_id.clone(),
        status: stored.status,
        created_at: stored.created_at,
        updated_at: stored.updated_at,
    })
}

fn validate_retryable_attempt_list_limit(limit: u32) -> CoreResult<()> {
    if (1..=MAX_RETRYABLE_ATTEMPT_LIST_LIMIT).contains(&limit) {
        Ok(())
    } else {
        Err(CoreError::invalid(
            "retryable generation attempt list limit must be between 1 and 100",
        ))
    }
}

fn validate_failure_code(value: &str) -> CoreResult<()> {
    if value.is_empty()
        || value.trim() != value
        || value.len() > MAX_ATTEMPT_FAILURE_CODE_BYTES
        || value.chars().any(char::is_control)
    {
        Err(CoreError::invalid(
            "generation attempt failure code is invalid",
        ))
    } else {
        Ok(())
    }
}

fn validate_id(label: &str, value: &str) -> CoreResult<()> {
    if value.is_empty()
        || value.len() > 256
        || value.trim() != value
        || value.chars().any(char::is_control)
    {
        Err(CoreError::invalid(format!("{label} id is invalid")))
    } else {
        Ok(())
    }
}

fn parse_time(label: &str, value: &str) -> CoreResult<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(value)
        .map(|value| value.with_timezone(&Utc))
        .map_err(|error| corrupted(format!("{label} is invalid: {error}")))
}

fn i64_from_u64(value: u64) -> CoreResult<i64> {
    i64::try_from(value).map_err(|_| CoreError::invalid("generation attempt revision overflowed"))
}

fn u64_from_i64(value: i64) -> CoreResult<u64> {
    u64::try_from(value).map_err(|_| corrupted("stored generation attempt revision is negative"))
}

fn attempt_conflict(generation_id: &GenerationId) -> CoreError {
    CoreError::new(
        CoreErrorCode::InvalidInput,
        format!(
            "generation attempt revision or status conflict for {}",
            generation_id.0
        ),
        true,
    )
}

fn not_found(label: &str) -> CoreError {
    CoreError::new(
        CoreErrorCode::NotFound,
        format!("{label} was not found"),
        false,
    )
}

fn corrupted(message: impl Into<String>) -> CoreError {
    CoreError::new(CoreErrorCode::StorageCorrupted, message, false)
}

#[cfg(test)]
mod tests {
    use lorepia_domain::{GenerationPresetId, ModelRouteId, ProviderConnectionId, ProviderProfile};
    use rusqlite::params;
    use tempfile::{TempDir, tempdir};

    use crate::{ProviderCredentialObservedStatus, ProviderCredentialOperationKind};

    use super::*;

    const SOURCE_SHA256: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
    const INPUT_SHA256: &str = "cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc";

    struct AttemptFixture {
        _root: TempDir,
        storage: Storage,
        conversation_id: ConversationId,
        source_branch_id: ConversationBranchId,
        original_head: MessageId,
    }

    fn prompt_selection_authority(fixture: &AttemptFixture) -> GenerationPromptSelectionAuthority {
        let character = fixture
            .storage
            .connection()
            .expect("open character authority connection")
            .query_row(
                "SELECT character.id, character.name, character.description,
                        character.source_hash, character.avatar_asset_hash,
                        character.created_at
                 FROM conversations AS conversation
                 JOIN characters AS character
                   ON character.id = conversation.character_id
                 WHERE conversation.id = ?1",
                [fixture.conversation_id.0.as_str()],
                |row| {
                    Ok(Character {
                        id: row.get(0)?,
                        name: row.get(1)?,
                        description: row.get(2)?,
                        source_hash: row.get(3)?,
                        avatar_asset_hash: row.get(4)?,
                        created_at: DateTime::parse_from_rfc3339(&row.get::<_, String>(5)?)
                            .expect("parse character authority time")
                            .with_timezone(&Utc),
                    })
                },
            )
            .expect("load character authority");
        GenerationPromptSelectionAuthority {
            schema_version: 1,
            mode: ConversationMode::Chat,
            local_user_id_sha256: prompt_local_user_id_sha256(
                &fixture
                    .storage
                    .load_settings()
                    .expect("load prompt local user")
                    .local_user_id,
            ),
            character,
            character_content: None,
            character_knowledge_book: None,
            supported_capabilities: Vec::new(),
            quick_settings: GenerationPromptQuickSettingsAuthority {
                response_length: PromptResponseLength::Balanced,
                creativity: 50,
                reasoning_effort: None,
                memory_enabled: true,
                knowledge_enabled: true,
                supports_temperature: false,
                resolved_temperature: None,
                resolved_max_output_tokens: None,
            },
            provider_target_authority: Some(GenerationProviderTargetAuthority::DirectModel {
                model_sha256: Sha256Digest::parse(INPUT_SHA256.to_owned())
                    .expect("synthetic direct model SHA-256"),
            }),
            explicit_preset_id: None,
            preset: crate::built_in_prompt_presets()
                .into_iter()
                .next()
                .expect("built-in chat preset"),
            preset_revision: 1,
            preset_revision_id: "synthetic-prompt-revision".to_owned(),
            binding: None,
            persona_selection: None,
        }
    }

    fn fixture() -> AttemptFixture {
        let root = tempdir().expect("temporary storage");
        let storage = Storage::open(root.path()).expect("open storage");
        let conversation_id = ConversationId("attempt-conversation".to_owned());
        let source_branch_id = ConversationBranchId("attempt-source-branch".to_owned());
        let original_head = MessageId("attempt-original-head".to_owned());
        let now = Utc::now().to_rfc3339();
        let connection = storage.connection().expect("storage connection");
        connection
            .execute(
                "INSERT INTO content_sources
                 (sha256, relative_path, size_bytes, created_at)
                 VALUES (?1, 'sources/test', 1, ?2)",
                params![SOURCE_SHA256, now],
            )
            .expect("insert source");
        connection
            .execute(
                "INSERT INTO characters
                 (id, name, description, source_hash, avatar_asset_hash, created_at)
                 VALUES ('attempt-character', 'Attempt', '', ?1, NULL, ?2)",
                params![SOURCE_SHA256, now],
            )
            .expect("insert character");
        connection
            .execute(
                "INSERT INTO conversations
                 (id, character_id, title, created_at, updated_at)
                 VALUES (?1, 'attempt-character', 'Attempt', ?2, ?2)",
                params![conversation_id.0, now],
            )
            .expect("insert conversation");
        connection
            .execute(
                "INSERT INTO messages
                 (id, conversation_id, parent_id, role, content, status,
                  generation_id, created_at)
                 VALUES (?1, ?2, NULL, 'user', 'first', 'complete', NULL, ?3)",
                params![original_head.0, conversation_id.0, now],
            )
            .expect("insert original head");
        connection
            .execute(
                "INSERT INTO conversation_branches
                 (id, conversation_id, title, fork_message_id, head_message_id,
                  created_at, updated_at)
                 VALUES (?1, ?2, NULL, NULL, ?3, ?4, ?4)",
                params![source_branch_id.0, conversation_id.0, original_head.0, now],
            )
            .expect("insert source branch");
        connection
            .execute(
                "INSERT INTO conversation_state
                 (conversation_id, active_branch_id, selected_mode, updated_at)
                 VALUES (?1, ?2, 'chat', ?3)",
                params![conversation_id.0, source_branch_id.0, now],
            )
            .expect("insert conversation state");
        drop(connection);
        AttemptFixture {
            _root: root,
            storage,
            conversation_id,
            source_branch_id,
            original_head,
        }
    }

    fn input(fixture: &AttemptFixture, fork: bool) -> GenerationAttemptInput {
        let operation_id = if fork {
            "fork-response-loss"
        } else {
            "same-branch-response-loss"
        };
        let proposed_branch_id = if fork {
            deterministic_proposed_branch_id(
                operation_id,
                &fixture.conversation_id,
                &fixture.source_branch_id,
                Some(&fixture.original_head),
            )
            .expect("deterministic proposed branch")
        } else {
            fixture.source_branch_id.clone()
        };
        let prompt_selection_authority = prompt_selection_authority(fixture);
        let module_runtime_review_authority = lorepia_orchestration::review_module_merge(
            0,
            &lorepia_orchestration::ModuleResolutionContext {
                local_user_id: fixture
                    .storage
                    .load_settings()
                    .expect("load module local user")
                    .local_user_id,
                persona_id: None,
                character_id: Some(prompt_selection_authority.character.id.clone()),
                conversation_id: Some(fixture.conversation_id.0.clone()),
                branch_id: Some(proposed_branch_id.0.clone()),
                supported_capabilities: Vec::new(),
            },
            &[],
            &[],
        )
        .expect("review synthetic module runtime authority");
        GenerationAttemptInput {
            operation_id: operation_id.to_owned(),
            conversation_id: fixture.conversation_id.clone(),
            source_branch_id: fixture.source_branch_id.clone(),
            proposed_branch_id,
            expected_head_message_id: Some(fixture.original_head.clone()),
            context_head_message_id: Some(fixture.original_head.clone()),
            module_plan_sha256: no_applied_module_runtime_plan_sha256(),
            base_request_fingerprint_sha256: Sha256Digest::parse(INPUT_SHA256.to_owned())
                .expect("input hash"),
            prompt_selection_authority: Some(prompt_selection_authority),
            module_runtime_review_authority: Some(module_runtime_review_authority),
            applied_runtime_plan_authority: None,
        }
    }

    fn install_generation_provider(
        fixture: &AttemptFixture,
        id: &str,
    ) -> (ProviderConnectionId, ModelRouteId, GenerationPresetId) {
        fixture
            .storage
            .save_provider_profile(&ProviderProfile {
                id: id.to_owned(),
                display_name: format!("Generation provider {id}"),
                base_url: "https://api.example.test/v1".to_owned(),
                model: "synthetic-model".to_owned(),
                timeout_seconds: 30,
            })
            .expect("save generation provider");
        (
            ProviderConnectionId::from(id),
            ModelRouteId::from(id),
            GenerationPresetId::from(id),
        )
    }

    fn install_generation_provider_credential_authority(
        fixture: &AttemptFixture,
        id: &str,
    ) -> (
        ProviderConnectionId,
        ModelRouteId,
        GenerationPresetId,
        ProviderCredentialAccessAuthority,
    ) {
        let (connection_id, route_id, preset_id) = install_generation_provider(fixture, id);
        let install_authority = fixture
            .storage
            .propose_provider_credential_install_authority(&connection_id)
            .expect("propose generation provider credential install authority");
        let install = fixture
            .storage
            .prepare_provider_credential_operation_with_install_authority(
                &connection_id,
                ProviderCredentialOperationKind::Install,
                ProviderCredentialObservedStatus::Missing,
                Some(&install_authority),
            )
            .expect("prepare generation provider credential install");
        fixture
            .storage
            .start_provider_credential_operation(&install.plan.operation_id, &install.plan_sha256)
            .expect("start generation provider credential install");
        fixture
            .storage
            .finish_provider_credential_operation(
                &install.plan.operation_id,
                &install.plan_sha256,
                ProviderCredentialObservedStatus::Available,
            )
            .expect("finish generation provider credential install");
        let authority = fixture
            .storage
            .ensure_provider_credential_access_settled(&connection_id)
            .expect("read generation provider credential authority");
        (connection_id, route_id, preset_id, authority)
    }

    fn terminally_remove_generation_provider_credential(
        fixture: &AttemptFixture,
        connection_id: &ProviderConnectionId,
    ) {
        let removal = fixture
            .storage
            .prepare_provider_credential_operation(
                connection_id,
                ProviderCredentialOperationKind::RemoveCredential,
                ProviderCredentialObservedStatus::Available,
            )
            .expect("prepare generation provider credential removal");
        fixture
            .storage
            .start_provider_credential_operation(&removal.plan.operation_id, &removal.plan_sha256)
            .expect("start generation provider credential removal");
        fixture
            .storage
            .finish_provider_credential_operation(
                &removal.plan.operation_id,
                &removal.plan_sha256,
                ProviderCredentialObservedStatus::Missing,
            )
            .expect("finish generation provider credential removal");
    }

    fn set_provider_authority(
        input: &mut GenerationAttemptInput,
        authority: GenerationProviderTargetAuthority,
    ) {
        input
            .prompt_selection_authority
            .as_mut()
            .expect("prompt selection authority")
            .provider_target_authority = Some(authority);
    }

    fn advance_source_head(fixture: &AttemptFixture) {
        let next_head = MessageId("attempt-advanced-head".to_owned());
        let now = Utc::now().to_rfc3339();
        let connection = fixture.storage.connection().expect("storage connection");
        connection
            .execute(
                "INSERT INTO messages
                 (id, conversation_id, parent_id, role, content, status,
                  generation_id, created_at)
                 VALUES (?1, ?2, ?3, 'user', 'advanced', 'complete', NULL, ?4)",
                params![
                    next_head.0,
                    fixture.conversation_id.0,
                    fixture.original_head.0,
                    now
                ],
            )
            .expect("insert advanced head");
        connection
            .execute(
                "UPDATE conversation_branches
                 SET head_message_id = ?3, updated_at = ?4
                 WHERE conversation_id = ?1 AND id = ?2",
                params![
                    fixture.conversation_id.0,
                    fixture.source_branch_id.0,
                    next_head.0,
                    now
                ],
            )
            .expect("advance source head");
    }

    fn attempt_time(offset_seconds: i64) -> DateTime<Utc> {
        "2026-08-10T12:00:00Z"
            .parse::<DateTime<Utc>>()
            .expect("parse attempt test time")
            + chrono::Duration::seconds(offset_seconds)
    }

    fn prepare_named_attempt(
        fixture: &AttemptFixture,
        operation_id: &str,
        prepared_at: DateTime<Utc>,
    ) -> StoredGenerationAttempt {
        let mut attempt_input = input(fixture, false);
        attempt_input.operation_id = operation_id.to_owned();
        fixture
            .storage
            .prepare_generation_attempt(&attempt_input, prepared_at)
            .expect("prepare named generation attempt")
    }

    fn record_before_for_projection(
        fixture: &AttemptFixture,
        attempt: &StoredGenerationAttempt,
        awaiting_approval: bool,
        recorded_at: DateTime<Utc>,
    ) -> StoredGenerationAttempt {
        let digest = Sha256Digest::parse(INPUT_SHA256.to_owned()).expect("projection test digest");
        fixture
            .storage
            .record_generation_attempt_before_event(
                &attempt.generation_id,
                attempt.revision,
                &GenerationBeforeEventEvidence {
                    event_id: format!("before-{}", attempt.generation_id.0),
                    event_sha256: digest.clone(),
                    context_state_revision: 1,
                    context_state_sha256: digest.clone(),
                    proposal_review_sha256s: awaiting_approval
                        .then_some(vec![digest])
                        .unwrap_or_default(),
                    awaiting_approval,
                },
                recorded_at,
            )
            .expect("record projection before-generation evidence")
    }

    fn make_dispatch_ready_for_projection(
        fixture: &AttemptFixture,
        prepared: &StoredGenerationAttempt,
        ready_at: DateTime<Utc>,
    ) -> StoredGenerationAttempt {
        let before = record_before_for_projection(fixture, prepared, false, ready_at);
        let digest = Sha256Digest::parse(INPUT_SHA256.to_owned()).expect("projection test digest");
        let seal = GenerationDispatchSeal {
            final_prompt_plan_sha256: digest.clone(),
            final_prompt_input_fingerprint_sha256: digest.clone(),
            final_interaction_state_revision: 1,
            final_interaction_state_sha256: digest.clone(),
            applied_module_plan_sha256: before.input.module_plan_sha256.clone(),
            before_generation_evidence_sha256: before
                .before_generation_evidence_sha256
                .clone()
                .expect("before-generation evidence hash"),
            approval_evidence_sha256: None,
            derived_chain_sha256: Some(digest),
            derived_event_count: Some(1),
            derived_guard_count: Some(0),
        };
        let (seal_json, seal_sha256) =
            encode_hashed("projection dispatch seal", &seal).expect("encode projection seal");
        let connection = fixture.storage.connection().expect("projection connection");
        connection
            .execute(
                "UPDATE generation_attempt_intents
                 SET status = 'dispatch_ready', revision = revision + 1,
                     dispatch_seal_json = ?2, dispatch_seal_sha256 = ?3,
                     updated_at = ?4
                 WHERE generation_id = ?1",
                params![
                    before.generation_id.0,
                    seal_json,
                    seal_sha256.as_str(),
                    ready_at.to_rfc3339()
                ],
            )
            .expect("advance projection attempt to dispatch-ready");
        drop(connection);
        fixture
            .storage
            .get_generation_attempt(&before.generation_id)
            .expect("read dispatch-ready projection attempt")
    }

    fn transition_projection_attempt(
        fixture: &AttemptFixture,
        attempt: &StoredGenerationAttempt,
        next_status: GenerationAttemptStatus,
        updated_at: DateTime<Utc>,
    ) -> StoredGenerationAttempt {
        let connection = fixture.storage.connection().expect("projection connection");
        connection
            .execute(
                "UPDATE generation_attempt_intents
                 SET status = ?2, revision = revision + 1, updated_at = ?3
                 WHERE generation_id = ?1",
                params![
                    attempt.generation_id.0,
                    next_status.as_str(),
                    updated_at.to_rfc3339()
                ],
            )
            .expect("advance excluded projection attempt status");
        drop(connection);
        fixture
            .storage
            .get_generation_attempt(&attempt.generation_id)
            .expect("read excluded projection attempt")
    }

    fn projection_of(attempt: &StoredGenerationAttempt) -> RetryableGenerationAttemptProjection {
        RetryableGenerationAttemptProjection {
            generation_id: attempt.generation_id.clone(),
            status: attempt.status,
            created_at: attempt.created_at,
            updated_at: attempt.updated_at,
        }
    }

    fn seed_excluded_projection_statuses(fixture: &AttemptFixture) {
        let prepared = prepare_named_attempt(fixture, "projection-prepared", attempt_time(3));
        assert_eq!(prepared.status, GenerationAttemptStatus::Prepared);

        let awaiting = prepare_named_attempt(fixture, "projection-awaiting", attempt_time(4));
        let awaiting = record_before_for_projection(fixture, &awaiting, true, attempt_time(14));
        assert_eq!(awaiting.status, GenerationAttemptStatus::AwaitingApproval);

        let failed = prepare_named_attempt(fixture, "projection-failed", attempt_time(5));
        let failed = fixture
            .storage
            .fail_generation_attempt_before_dispatch(
                &failed.generation_id,
                failed.revision,
                "projection_test_failure",
                attempt_time(15),
            )
            .expect("fail excluded projection attempt");
        assert_eq!(failed.status, GenerationAttemptStatus::FailedBeforeDispatch);

        let running = prepare_named_attempt(fixture, "projection-running", attempt_time(6));
        let running = make_dispatch_ready_for_projection(fixture, &running, attempt_time(16));
        let running = transition_projection_attempt(
            fixture,
            &running,
            GenerationAttemptStatus::Running,
            attempt_time(17),
        );
        assert_eq!(running.status, GenerationAttemptStatus::Running);

        let completed = prepare_named_attempt(fixture, "projection-completed", attempt_time(7));
        let completed = make_dispatch_ready_for_projection(fixture, &completed, attempt_time(18));
        let completed = transition_projection_attempt(
            fixture,
            &completed,
            GenerationAttemptStatus::Running,
            attempt_time(19),
        );
        let completed = transition_projection_attempt(
            fixture,
            &completed,
            GenerationAttemptStatus::Completed,
            attempt_time(20),
        );
        assert_eq!(completed.status, GenerationAttemptStatus::Completed);
    }

    fn assert_projection_scope_and_limits(
        fixture: &AttemptFixture,
        expected: &[RetryableGenerationAttemptProjection],
    ) {
        let listed = fixture
            .storage
            .list_retryable_generation_attempts_for_source_room(
                &fixture.conversation_id,
                &fixture.source_branch_id,
                100,
            )
            .expect("list retryable generation attempts");
        assert_eq!(listed, expected);
        assert_eq!(
            fixture
                .storage
                .list_retryable_generation_attempts_for_source_room(
                    &fixture.conversation_id,
                    &fixture.source_branch_id,
                    2,
                )
                .expect("limit retryable attempts"),
            expected[..2]
        );
        assert!(
            fixture
                .storage
                .list_retryable_generation_attempts_for_source_room(
                    &ConversationId("another-conversation".to_owned()),
                    &fixture.source_branch_id,
                    100,
                )
                .expect("scope retryable attempts by conversation")
                .is_empty()
        );
        assert!(
            fixture
                .storage
                .list_retryable_generation_attempts_for_source_room(
                    &fixture.conversation_id,
                    &ConversationBranchId("another-branch".to_owned()),
                    100,
                )
                .expect("scope retryable attempts by branch")
                .is_empty()
        );
        for invalid_limit in [0, 101] {
            let error = fixture
                .storage
                .list_retryable_generation_attempts_for_source_room(
                    &fixture.conversation_id,
                    &fixture.source_branch_id,
                    invalid_limit,
                )
                .expect_err("reject invalid retryable attempt limit");
            assert_eq!(error.code, CoreErrorCode::InvalidInput);
        }
    }

    fn assert_projection_reopens(
        fixture: AttemptFixture,
        expected: &[RetryableGenerationAttemptProjection],
    ) {
        let AttemptFixture {
            _root: root,
            storage,
            conversation_id,
            source_branch_id,
            ..
        } = fixture;
        drop(storage);
        let reopened = Storage::open(root.path()).expect("reopen projection storage");
        assert_eq!(
            reopened
                .list_retryable_generation_attempts_for_source_room(
                    &conversation_id,
                    &source_branch_id,
                    100,
                )
                .expect("list retryable attempts after reopen"),
            expected
        );
    }

    #[test]
    fn prepare_rejects_forged_prompt_character_authority() {
        let fixture = fixture();
        let mut input = input(&fixture, false);
        input
            .prompt_selection_authority
            .as_mut()
            .expect("prompt authority")
            .character
            .name = "Forged Character".to_owned();
        let error = fixture
            .storage
            .prepare_generation_attempt(&input, Utc::now())
            .expect_err("forged prompt character must be rejected");
        assert_eq!(error.code, CoreErrorCode::InvalidInput);
        assert_eq!(
            error.message,
            "generation prompt character differs from its conversation authority"
        );
    }

    #[test]
    fn module_runtime_authority_is_immutable_and_hash_checked_on_read() {
        let fixture = fixture();
        let input = input(&fixture, false);
        let stored = fixture
            .storage
            .prepare_generation_attempt(&input, Utc::now())
            .expect("prepare module-authority attempt");
        let connection = fixture
            .storage
            .connection()
            .expect("open tamper connection");
        connection
            .execute(
                "UPDATE generation_attempt_intents
                 SET module_runtime_review_authority_json = '{}',
                     revision = revision + 1
                 WHERE generation_id = ?1",
                [stored.generation_id.0.as_str()],
            )
            .expect_err("module runtime authority trigger must reject mutation");
        connection
            .execute_batch(
                "DROP TRIGGER generation_attempt_module_runtime_authority_update_guard;
                 DROP TRIGGER generation_attempt_intents_transition_guard;",
            )
            .expect("simulate trigger bypass");
        connection
            .execute(
                "UPDATE generation_attempt_intents
                 SET module_runtime_review_authority_json = '{}',
                     revision = revision + 1
                 WHERE generation_id = ?1",
                [stored.generation_id.0.as_str()],
            )
            .expect("tamper module runtime authority after trigger bypass");
        drop(connection);
        let error = fixture
            .storage
            .get_generation_attempt(&stored.generation_id)
            .expect_err("tampered module authority must fail read verification");
        assert_eq!(error.code, CoreErrorCode::StorageCorrupted);
    }

    #[test]
    fn retryable_projection_is_safe_scoped_bounded_ordered_and_restart_durable() {
        let fixture = fixture();
        let old_before = prepare_named_attempt(&fixture, "projection-old-before", attempt_time(0));
        let old_before =
            record_before_for_projection(&fixture, &old_before, false, attempt_time(10));
        let dispatch = prepare_named_attempt(&fixture, "projection-dispatch", attempt_time(1));
        let dispatch = make_dispatch_ready_for_projection(&fixture, &dispatch, attempt_time(11));
        let tie_one = prepare_named_attempt(&fixture, "projection-tie-one", attempt_time(2));
        let tie_one = record_before_for_projection(&fixture, &tie_one, false, attempt_time(12));
        let tie_two = prepare_named_attempt(&fixture, "projection-tie-two", attempt_time(2));
        let tie_two = record_before_for_projection(&fixture, &tie_two, false, attempt_time(13));
        seed_excluded_projection_statuses(&fixture);

        let mut tied = vec![projection_of(&tie_one), projection_of(&tie_two)];
        tied.sort_by(|left, right| right.generation_id.0.cmp(&left.generation_id.0));
        let mut expected = tied;
        expected.push(projection_of(&dispatch));
        expected.push(projection_of(&old_before));
        assert_projection_scope_and_limits(&fixture, &expected);
        assert_projection_reopens(fixture, &expected);
    }

    #[test]
    fn retryable_projection_fails_closed_on_full_attempt_authority_tampering() {
        let fixture = fixture();
        let prepared = prepare_named_attempt(&fixture, "projection-tamper", attempt_time(0));
        let stored = record_before_for_projection(&fixture, &prepared, false, attempt_time(1));
        let connection = fixture.storage.connection().expect("tamper connection");
        connection
            .execute_batch(
                "DROP TRIGGER generation_attempt_prompt_selection_update_guard;
                 DROP TRIGGER generation_attempt_intents_transition_guard;",
            )
            .expect("simulate projection authority trigger bypass");
        connection
            .execute(
                "UPDATE generation_attempt_intents
                 SET prompt_selection_authority_json = '{}', revision = revision + 1
                 WHERE generation_id = ?1",
                [stored.generation_id.0.as_str()],
            )
            .expect("tamper projection attempt authority");
        drop(connection);
        let error = fixture
            .storage
            .list_retryable_generation_attempts_for_source_room(
                &fixture.conversation_id,
                &fixture.source_branch_id,
                100,
            )
            .expect_err("tampered full attempt authority must fail projection read");
        assert_eq!(error.code, CoreErrorCode::StorageCorrupted);
    }

    #[test]
    fn exact_same_branch_prepare_retry_survives_advanced_source_head() {
        let fixture = fixture();
        let input = input(&fixture, false);
        let first = fixture
            .storage
            .prepare_generation_attempt(&input, Utc::now())
            .expect("prepare attempt");
        advance_source_head(&fixture);

        let replay = fixture
            .storage
            .prepare_generation_attempt(&input, Utc::now())
            .expect("replay exact attempt after response loss");
        assert_eq!(replay, first);
    }

    #[test]
    fn credential_archive_prepare_blocks_generation_attempt_without_rows() {
        for provider_kind in ["provider-profile", "generation-target"] {
            let fixture = fixture();
            let provider_id = format!("attempt-archive-{provider_kind}");
            let (connection_id, route_id, preset_id) =
                install_generation_provider(&fixture, &provider_id);
            fixture
                .storage
                .prepare_provider_credential_operation(
                    &connection_id,
                    ProviderCredentialOperationKind::RemoveForArchive,
                    ProviderCredentialObservedStatus::Missing,
                )
                .expect("prepare credential archive");

            let mut attempt_input = input(&fixture, false);
            attempt_input.operation_id = format!("blocked-{provider_kind}");
            let authority = if provider_kind == "provider-profile" {
                GenerationProviderTargetAuthority::ProviderProfile {
                    provider_profile_id: provider_id,
                    dispatch_snapshot_sha256: Sha256Digest::parse(INPUT_SHA256.to_owned())
                        .expect("provider profile authority hash"),
                }
            } else {
                GenerationProviderTargetAuthority::GenerationTarget {
                    target: GenerationTarget {
                        model_route_id: route_id,
                        generation_preset_id: preset_id,
                    },
                    resolved_snapshot_sha256: Sha256Digest::parse(INPUT_SHA256.to_owned())
                        .expect("generation target authority hash"),
                }
            };
            set_provider_authority(&mut attempt_input, authority);

            let error = fixture
                .storage
                .prepare_generation_attempt(&attempt_input, Utc::now())
                .expect_err("credential archive must block generation attempt prepare");
            assert_eq!(error.code, CoreErrorCode::InvalidInput);
            assert!(error.recoverable);
            assert_eq!(
                fixture
                    .storage
                    .connection()
                    .expect("attempt row count connection")
                    .query_row(
                        "SELECT COUNT(*) FROM generation_attempt_intents WHERE operation_id = ?1",
                        [attempt_input.operation_id.as_str()],
                        |row| row.get::<_, u64>(0),
                    )
                    .expect("attempt row count"),
                0
            );
        }
    }

    #[test]
    fn prepared_credential_removal_blocks_generation_attempt_without_rows() {
        let fixture = fixture();
        let provider_id = "attempt-credential-removal";
        let (connection_id, route_id, preset_id) =
            install_generation_provider(&fixture, provider_id);
        let removal = fixture
            .storage
            .prepare_provider_credential_operation(
                &connection_id,
                ProviderCredentialOperationKind::RemoveCredential,
                ProviderCredentialObservedStatus::Missing,
            )
            .expect("prepare credential removal before generation");
        let mut attempt_input = input(&fixture, false);
        attempt_input.operation_id = "blocked-by-credential-removal".to_owned();
        set_provider_authority(
            &mut attempt_input,
            GenerationProviderTargetAuthority::GenerationTarget {
                target: GenerationTarget {
                    model_route_id: route_id,
                    generation_preset_id: preset_id,
                },
                resolved_snapshot_sha256: Sha256Digest::parse(INPUT_SHA256.to_owned())
                    .expect("generation target authority hash"),
            },
        );

        let error = fixture
            .storage
            .prepare_generation_attempt(&attempt_input, Utc::now())
            .expect_err("prepared credential removal must reserve the connection");
        assert_eq!(error.code, CoreErrorCode::InvalidInput);
        assert!(error.recoverable);
        assert_eq!(
            fixture
                .storage
                .connection()
                .expect("attempt count connection")
                .query_row(
                    "SELECT COUNT(*) FROM generation_attempt_intents WHERE operation_id = ?1",
                    [attempt_input.operation_id.as_str()],
                    |row| row.get::<_, u64>(0),
                )
                .expect("attempt row count"),
            0
        );
        assert_eq!(
            fixture
                .storage
                .list_unresolved_provider_credential_operations()
                .expect("list credential operations")
                .iter()
                .map(|operation| operation.plan.operation_id.as_str())
                .collect::<Vec<_>>(),
            vec![removal.plan.operation_id.as_str()]
        );
    }

    #[test]
    fn terminal_credential_removal_rejects_cached_generation_authority_without_an_attempt() {
        let fixture = fixture();
        let (connection_id, route_id, preset_id, cached_authority) =
            install_generation_provider_credential_authority(
                &fixture,
                "terminal-removal-generation-authority",
            );
        terminally_remove_generation_provider_credential(&fixture, &connection_id);

        let mut attempt_input = input(&fixture, false);
        attempt_input.operation_id = "stale-terminal-removal-generation".to_owned();
        set_provider_authority(
            &mut attempt_input,
            GenerationProviderTargetAuthority::GenerationTarget {
                target: GenerationTarget {
                    model_route_id: route_id,
                    generation_preset_id: preset_id,
                },
                resolved_snapshot_sha256: Sha256Digest::parse(INPUT_SHA256.to_owned())
                    .expect("generation target authority hash"),
            },
        );

        let error = fixture
            .storage
            .prepare_generation_attempt_with_credential_authority(
                &attempt_input,
                Utc::now(),
                Some(&cached_authority),
            )
            .expect_err("terminal credential removal must invalidate a cached authority");
        assert_eq!(error.code, CoreErrorCode::InvalidInput);
        assert!(error.recoverable);
        assert_eq!(
            fixture
                .storage
                .connection()
                .expect("attempt count connection")
                .query_row(
                    "SELECT COUNT(*) FROM generation_attempt_intents WHERE operation_id = ?1",
                    [attempt_input.operation_id.as_str()],
                    |row| row.get::<_, u64>(0),
                )
                .expect("attempt row count"),
            0
        );
    }

    #[test]
    fn prepared_generation_attempt_blocks_credential_archive() {
        for provider_kind in ["provider-profile", "generation-target"] {
            let fixture = fixture();
            let provider_id = format!("attempt-running-{provider_kind}");
            let (connection_id, route_id, preset_id) =
                install_generation_provider(&fixture, &provider_id);
            let mut attempt_input = input(&fixture, false);
            attempt_input.operation_id = format!("prepared-{provider_kind}");
            let authority = if provider_kind == "provider-profile" {
                GenerationProviderTargetAuthority::ProviderProfile {
                    provider_profile_id: provider_id,
                    dispatch_snapshot_sha256: Sha256Digest::parse(INPUT_SHA256.to_owned())
                        .expect("provider profile authority hash"),
                }
            } else {
                GenerationProviderTargetAuthority::GenerationTarget {
                    target: GenerationTarget {
                        model_route_id: route_id,
                        generation_preset_id: preset_id,
                    },
                    resolved_snapshot_sha256: Sha256Digest::parse(INPUT_SHA256.to_owned())
                        .expect("generation target authority hash"),
                }
            };
            set_provider_authority(&mut attempt_input, authority);
            fixture
                .storage
                .prepare_generation_attempt(&attempt_input, Utc::now())
                .expect("prepare provider generation attempt");

            let error = fixture
                .storage
                .prepare_provider_credential_operation(
                    &connection_id,
                    ProviderCredentialOperationKind::RemoveForArchive,
                    ProviderCredentialObservedStatus::Missing,
                )
                .expect_err("unfinished generation attempt must block credential archive");
            assert_eq!(error.code, CoreErrorCode::InvalidInput);
            assert!(error.recoverable);
            assert!(
                fixture
                    .storage
                    .list_unresolved_provider_credential_operations()
                    .expect("list credential operations")
                    .is_empty()
            );
        }
    }

    #[test]
    fn prepared_generation_attempt_blocks_credential_removal_prepare() {
        let fixture = fixture();
        let provider_id = "attempt-before-credential-removal";
        let (connection_id, route_id, preset_id) =
            install_generation_provider(&fixture, provider_id);
        let mut attempt_input = input(&fixture, false);
        attempt_input.operation_id = "prepared-before-credential-removal".to_owned();
        set_provider_authority(
            &mut attempt_input,
            GenerationProviderTargetAuthority::GenerationTarget {
                target: GenerationTarget {
                    model_route_id: route_id,
                    generation_preset_id: preset_id,
                },
                resolved_snapshot_sha256: Sha256Digest::parse(INPUT_SHA256.to_owned())
                    .expect("generation target authority hash"),
            },
        );
        fixture
            .storage
            .prepare_generation_attempt(&attempt_input, Utc::now())
            .expect("prepare provider generation attempt");

        let error = fixture
            .storage
            .prepare_provider_credential_operation(
                &connection_id,
                ProviderCredentialOperationKind::RemoveCredential,
                ProviderCredentialObservedStatus::Missing,
            )
            .expect_err("unfinished generation attempt must block credential removal");
        assert_eq!(error.code, CoreErrorCode::InvalidInput);
        assert!(error.recoverable);
        assert!(
            fixture
                .storage
                .list_unresolved_provider_credential_operations()
                .expect("list credential operations")
                .is_empty()
        );
    }

    #[test]
    fn admitted_legacy_generation_blocks_raw_credential_mutation() {
        let fixture = fixture();
        let provider_id = "legacy-admission-mutation-guard";
        install_generation_provider(&fixture, provider_id);
        let legacy_connection = fixture
            .storage
            .connection()
            .expect("legacy ownership connection");
        legacy_connection
            .execute_batch("DROP TRIGGER provider_credential_ownership_authority_guard")
            .expect("simulate schema-36 cutover ownership projection");
        legacy_connection
            .execute(
                "UPDATE provider_credential_ownership
                 SET ownership_state = 'legacy_pending',
                     connection_binding_sha256 = NULL,
                     authority_id = 'schema-36-cutover'
                 WHERE connection_id = ?1 AND credential_ref = ?1",
                [provider_id],
            )
            .expect("project synthetic schema-36 legacy ownership");
        drop(legacy_connection);
        fixture
            .storage
            .ensure_legacy_profile_credential_mutation_settled(provider_id)
            .expect("idle legacy credential permits mutation");
        let mut attempt_input = input(&fixture, false);
        attempt_input.operation_id = "legacy-admission-mutation-attempt".to_owned();
        set_provider_authority(
            &mut attempt_input,
            GenerationProviderTargetAuthority::ProviderProfile {
                provider_profile_id: provider_id.to_owned(),
                dispatch_snapshot_sha256: Sha256Digest::parse(INPUT_SHA256.to_owned())
                    .expect("legacy provider authority hash"),
            },
        );
        fixture
            .storage
            .prepare_generation_attempt(&attempt_input, Utc::now())
            .expect("admit legacy generation attempt");

        let error = fixture
            .storage
            .ensure_legacy_profile_credential_mutation_settled(provider_id)
            .expect_err("admitted legacy generation must reserve raw credential mutation");
        assert_eq!(error.code, CoreErrorCode::InvalidInput);
        assert!(error.recoverable);
    }

    #[test]
    fn direct_model_attempts_do_not_block_provider_credential_boundaries() {
        let fixture = fixture();
        let (connection_id, _, _) = install_generation_provider(&fixture, "direct-unrelated");
        fixture
            .storage
            .prepare_generation_attempt(&input(&fixture, false), Utc::now())
            .expect("prepare unrelated direct-model attempt");
        fixture
            .storage
            .prepare_provider_credential_operation(
                &connection_id,
                ProviderCredentialOperationKind::RemoveForArchive,
                ProviderCredentialObservedStatus::Missing,
            )
            .expect("direct-model attempt must not block provider archive");
    }

    #[test]
    fn failed_before_dispatch_attempt_does_not_block_credential_archive() {
        let fixture = fixture();
        let (connection_id, route_id, preset_id) =
            install_generation_provider(&fixture, "failed-before-archive");
        let mut attempt_input = input(&fixture, false);
        attempt_input.operation_id = "failed-before-archive-attempt".to_owned();
        set_provider_authority(
            &mut attempt_input,
            GenerationProviderTargetAuthority::GenerationTarget {
                target: GenerationTarget {
                    model_route_id: route_id,
                    generation_preset_id: preset_id,
                },
                resolved_snapshot_sha256: Sha256Digest::parse(INPUT_SHA256.to_owned())
                    .expect("generation target authority hash"),
            },
        );
        let prepared = fixture
            .storage
            .prepare_generation_attempt(&attempt_input, Utc::now())
            .expect("prepare provider generation attempt");
        let failed = fixture
            .storage
            .fail_generation_attempt_before_dispatch(
                &prepared.generation_id,
                prepared.revision,
                "provider_preflight_failed",
                Utc::now(),
            )
            .expect("terminalize attempt before dispatch");
        assert_eq!(failed.status, GenerationAttemptStatus::FailedBeforeDispatch);

        let archive = fixture
            .storage
            .prepare_provider_credential_operation(
                &connection_id,
                ProviderCredentialOperationKind::RemoveForArchive,
                ProviderCredentialObservedStatus::Missing,
            )
            .expect("failed-before-dispatch attempt must not block provider archive");
        let retry_error = fixture
            .storage
            .retry_generation_attempt(&failed.generation_id, failed.revision, Utc::now())
            .expect_err("prepared credential archive must block failed-attempt retry");
        assert_eq!(retry_error.code, CoreErrorCode::InvalidInput);
        assert!(retry_error.recoverable);
        assert_eq!(
            fixture
                .storage
                .get_generation_attempt(&failed.generation_id)
                .expect("failed attempt after rejected retry")
                .status,
            GenerationAttemptStatus::FailedBeforeDispatch
        );
        assert_eq!(
            fixture
                .storage
                .get_provider_credential_operation(&archive.plan.operation_id)
                .expect("prepared archive after rejected retry")
                .status,
            crate::ProviderCredentialOperationStatus::Prepared
        );
    }

    #[test]
    fn exact_fork_prepare_retry_survives_advanced_head_and_created_target() {
        let fixture = fixture();
        let input = input(&fixture, true);
        let first = fixture
            .storage
            .prepare_generation_attempt(&input, Utc::now())
            .expect("prepare fork attempt");
        advance_source_head(&fixture);
        let now = Utc::now().to_rfc3339();
        fixture
            .storage
            .connection()
            .expect("storage connection")
            .execute(
                "INSERT INTO conversation_branches
                 (id, conversation_id, title, fork_message_id, head_message_id,
                  created_at, updated_at)
                 VALUES (?1, ?2, NULL, ?3, ?3, ?4, ?4)",
                params![
                    input.proposed_branch_id.0,
                    fixture.conversation_id.0,
                    fixture.original_head.0,
                    now
                ],
            )
            .expect("simulate committed target branch");

        let replay = fixture
            .storage
            .prepare_generation_attempt(&input, Utc::now())
            .expect("replay exact fork attempt after response loss");
        assert_eq!(replay, first);
    }

    #[test]
    fn fork_prepare_accepts_the_historical_root_below_a_nonempty_source_head() {
        let fixture = fixture();
        let operation_id = "fork-from-historical-root";
        let proposed_branch_id = deterministic_proposed_branch_id(
            operation_id,
            &fixture.conversation_id,
            &fixture.source_branch_id,
            None,
        )
        .expect("derive root fork branch");
        let prompt_selection_authority = prompt_selection_authority(&fixture);
        let module_runtime_review_authority = lorepia_orchestration::review_module_merge(
            0,
            &lorepia_orchestration::ModuleResolutionContext {
                local_user_id: fixture
                    .storage
                    .load_settings()
                    .expect("load module local user")
                    .local_user_id,
                persona_id: None,
                character_id: Some(prompt_selection_authority.character.id.clone()),
                conversation_id: Some(fixture.conversation_id.0.clone()),
                branch_id: Some(proposed_branch_id.0.clone()),
                supported_capabilities: Vec::new(),
            },
            &[],
            &[],
        )
        .expect("review historical module runtime authority");
        let input = GenerationAttemptInput {
            operation_id: operation_id.to_owned(),
            conversation_id: fixture.conversation_id.clone(),
            source_branch_id: fixture.source_branch_id.clone(),
            proposed_branch_id,
            expected_head_message_id: Some(fixture.original_head.clone()),
            context_head_message_id: None,
            module_plan_sha256: no_applied_module_runtime_plan_sha256(),
            prompt_selection_authority: Some(prompt_selection_authority),
            module_runtime_review_authority: Some(module_runtime_review_authority),
            applied_runtime_plan_authority: None,
            base_request_fingerprint_sha256: Sha256Digest::parse(INPUT_SHA256.to_owned())
                .expect("input hash"),
        };
        let prepared = fixture
            .storage
            .prepare_generation_attempt(&input, Utc::now())
            .expect("prepare historical-root fork");
        assert_eq!(prepared.input.context_head_message_id, None);
        assert_eq!(
            prepared.input.expected_head_message_id,
            Some(fixture.original_head)
        );
    }
}
