use lorepia_domain::{ConversationBranchId, ConversationId, CoreResult};

use super::{
    GenerationAttemptStatus, RetryableGenerationAttemptProjection, StoredGenerationAttempt,
    corrupted, validate_approval_evidence, validate_before_evidence, validate_dispatch_seal,
};

pub(super) fn retryable_generation_attempt_projection(
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
