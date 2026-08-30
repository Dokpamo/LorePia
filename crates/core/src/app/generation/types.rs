use lorepia_domain::{
    ConversationBranchId, ConversationId, GenerationId, GenerationPresetId, MessageId,
    ModelRouteId, Sha256Digest, VariableMap,
};
use lorepia_storage::MessageGenerationAction;
use serde::Serialize;

pub const MAX_GENERATION_OPERATION_NONCE_BYTES: usize = 128;
pub const MAX_GENERATION_OPERATION_NONCE_CHARS: usize = 64;

/// Caller-owned boundary for a new generation operation or an exact durable
/// attempt selected for restart-safe resume. The variants are intentionally
/// exclusive so callers cannot ambiguously rotate and resume at once.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GenerationOperationContext<'a> {
    New {
        operation_nonce: &'a str,
    },
    Resume {
        generation_attempt_id: &'a GenerationId,
    },
}
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub(in crate::app) enum GenerationActionTargetIdentity {
    GenerationTarget {
        model_route_id: ModelRouteId,
        generation_preset_id: GenerationPresetId,
    },
    ProviderProfile {
        provider_profile_id: String,
    },
    #[cfg(test)]
    DirectModel {
        model_sha256: String,
    },
}
#[derive(Serialize)]
pub(in crate::app) struct GenerationActionSemanticSnapshot<'a> {
    pub(in crate::app) schema_version: u32,
    pub(in crate::app) action: &'static str,
    pub(in crate::app) conversation_id: &'a ConversationId,
    pub(in crate::app) source_branch_id: &'a ConversationBranchId,
    pub(in crate::app) expected_source_head_message_id: Option<&'a MessageId>,
    pub(in crate::app) target_message_id: &'a MessageId,
    pub(in crate::app) context_head_message_id: Option<&'a MessageId>,
    pub(in crate::app) replacement_text_sha256: &'a str,
    pub(in crate::app) target: &'a GenerationActionTargetIdentity,
}

pub(in crate::app) struct MessageGenerationActionIdentityInput<'a> {
    pub(in crate::app) conversation_id: &'a ConversationId,
    pub(in crate::app) source_branch_id: &'a ConversationBranchId,
    pub(in crate::app) expected_source_head_message_id: Option<&'a MessageId>,
    pub(in crate::app) target_message_id: &'a MessageId,
    pub(in crate::app) action: MessageGenerationAction,
    pub(in crate::app) replacement_text: Option<&'a str>,
    pub(in crate::app) operation_context: GenerationOperationContext<'a>,
    pub(in crate::app) target: GenerationActionTargetIdentity,
}

#[derive(Serialize)]
pub(in crate::app) struct GenerationSendSemanticSnapshot<'a> {
    /// This includes only caller-owned semantic request identity. Conversation
    /// mode, provider mapping, effective quick settings, and the operation
    /// nonce are sealed or scoped separately so none can alter prompt
    /// semantics after an approval pause.
    pub(in crate::app) schema_version: u32,
    pub(in crate::app) conversation_id: &'a ConversationId,
    pub(in crate::app) branch_id: &'a ConversationBranchId,
    pub(in crate::app) expected_head_message_id: Option<&'a MessageId>,
    pub(in crate::app) user_text_sha256: &'a str,
    pub(in crate::app) target: &'a GenerationActionTargetIdentity,
    pub(in crate::app) temperature: Option<f64>,
    pub(in crate::app) max_output_tokens: Option<u32>,
    pub(in crate::app) prompt_preset_id: Option<&'a lorepia_domain::PromptPresetId>,
    pub(in crate::app) variable_overrides: &'a VariableMap,
}

#[derive(Serialize)]
pub(in crate::app) struct GenerationOperationNonceEnvelope<'a> {
    pub(in crate::app) schema_version: u32,
    pub(in crate::app) domain: &'static str,
    pub(in crate::app) semantic_base_fingerprint_sha256: &'a Sha256Digest,
    pub(in crate::app) operation_nonce: &'a str,
}

pub(in crate::app) struct SameBranchGenerationAttemptIdentity<'a> {
    pub(in crate::app) conversation_id: &'a ConversationId,
    pub(in crate::app) branch_id: &'a ConversationBranchId,
    pub(in crate::app) expected_head: Option<&'a MessageId>,
    pub(in crate::app) text: &'a str,
    pub(in crate::app) operation_context: GenerationOperationContext<'a>,
    pub(in crate::app) target: &'a GenerationActionTargetIdentity,
    pub(in crate::app) temperature: Option<f64>,
    pub(in crate::app) max_output_tokens: Option<u32>,
    pub(in crate::app) prompt_preset_id: Option<&'a lorepia_domain::PromptPresetId>,
    pub(in crate::app) variable_overrides: &'a VariableMap,
}

pub(in crate::app) struct ResolvedGenerationOperationIdentity {
    pub(in crate::app) operation_id: String,
    pub(in crate::app) base_request_fingerprint_sha256: Sha256Digest,
    pub(in crate::app) resume_generation_attempt_id: Option<GenerationId>,
}
