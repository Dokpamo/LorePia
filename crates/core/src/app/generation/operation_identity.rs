use lorepia_domain::{
    ConversationBranchId, ConversationId, CoreError, CoreErrorCode, CoreResult, GenerationId,
    MessageId, Sha256Digest,
};
use lorepia_storage::MessageGenerationAction;
use sha2::{Digest, Sha256};

use super::types::{
    GenerationOperationNonceEnvelope, GenerationSendSemanticSnapshot,
    MAX_GENERATION_OPERATION_NONCE_BYTES, MAX_GENERATION_OPERATION_NONCE_CHARS,
    SameBranchGenerationAttemptIdentity,
};
use crate::app::canonical_value_sha256;

pub(in crate::app) fn same_branch_generation_semantic_fingerprint(
    input: &SameBranchGenerationAttemptIdentity<'_>,
) -> CoreResult<Sha256Digest> {
    let user_text_sha256 = format!("{:x}", Sha256::digest(input.text.as_bytes()));
    Sha256Digest::parse(canonical_value_sha256(
        &GenerationSendSemanticSnapshot {
            schema_version: 1,
            conversation_id: input.conversation_id,
            branch_id: input.branch_id,
            expected_head_message_id: input.expected_head,
            user_text_sha256: &user_text_sha256,
            target: input.target,
            temperature: input.temperature,
            max_output_tokens: input.max_output_tokens,
            prompt_preset_id: input.prompt_preset_id,
            variable_overrides: input.variable_overrides,
        },
        "generation semantic base request",
    )?)
    .map_err(CoreError::invalid)
}

pub(in crate::app) fn new_generation_operation_id(
    domain: &'static str,
    base_request_fingerprint_sha256: &Sha256Digest,
    operation_nonce: &str,
) -> CoreResult<String> {
    let operation_nonce = validate_generation_operation_nonce(operation_nonce)?;
    let operation_sha256 = canonical_value_sha256(
        &GenerationOperationNonceEnvelope {
            schema_version: 1,
            domain,
            semantic_base_fingerprint_sha256: base_request_fingerprint_sha256,
            operation_nonce,
        },
        "generation operation",
    )?;
    Ok(format!("{domain}-{operation_sha256}"))
}

pub(in crate::app) fn validate_same_branch_attempt_semantic_identity(
    attempt: &lorepia_storage::StoredGenerationAttempt,
    conversation_id: &ConversationId,
    branch_id: &ConversationBranchId,
    expected_head: Option<&MessageId>,
    base_request_fingerprint_sha256: &Sha256Digest,
    resume_generation_attempt_id: Option<&GenerationId>,
) -> CoreResult<()> {
    let mismatched = resume_generation_attempt_id
        .is_some_and(|generation_id| generation_id != &attempt.generation_id)
        || attempt.input.conversation_id != *conversation_id
        || attempt.input.source_branch_id != *branch_id
        || attempt.input.proposed_branch_id != *branch_id
        || attempt.input.expected_head_message_id != expected_head.cloned()
        || attempt.input.context_head_message_id != expected_head.cloned()
        || attempt.input.base_request_fingerprint_sha256 != *base_request_fingerprint_sha256
        || attempt.input.prompt_selection_authority.is_none();
    if mismatched {
        return if resume_generation_attempt_id.is_some() {
            Err(CoreError::new(
                CoreErrorCode::InvalidInput,
                "generation resume attempt does not match the caller-owned request; start a new generation operation",
                true,
            ))
        } else {
            Err(CoreError::new(
                CoreErrorCode::StorageCorrupted,
                "stored same-branch generation attempt differs from its immutable request",
                false,
            ))
        };
    }
    Ok(())
}

pub(in crate::app) fn validate_generation_operation_nonce(value: &str) -> CoreResult<&str> {
    if value.is_empty()
        || value.trim() != value
        || value.len() > MAX_GENERATION_OPERATION_NONCE_BYTES
        || value.chars().count() > MAX_GENERATION_OPERATION_NONCE_CHARS
        || value.chars().any(char::is_control)
    {
        return Err(CoreError::invalid(
            "generation operation nonce is empty, unsafe, or exceeds its size limit",
        ));
    }
    Ok(value)
}

pub(in crate::app) const fn generation_action_name(
    action: MessageGenerationAction,
) -> &'static str {
    match action {
        MessageGenerationAction::EditUser => "edit_user",
        MessageGenerationAction::RegenerateAssistant => "regenerate_assistant",
    }
}
