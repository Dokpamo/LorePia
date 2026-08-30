mod actions;
mod admission;
mod attempt;
mod auxiliary;
mod capabilities;
mod credential;
mod delivery;
mod operation_identity;
mod preset;
mod prompt_preparation;
mod protocol_request;
mod send;
mod send_dispatch;
mod target_resolution;
mod target_send;
mod types;

pub(super) use admission::{
    GenerationLaunchPermit, MAX_ACTIVE_GENERATIONS_PER_CONVERSATION,
    MAX_ACTIVE_GENERATIONS_PER_PROCESS, MAX_ACTIVE_GENERATIONS_PER_PROVIDER,
};
pub(crate) use attempt::generation_attempt_module_authority;
pub(super) use attempt::{
    PreparedSameBranchGenerationAttempt, SameBranchGenerationAttempt,
    generation_attempt_prompt_authority,
};
pub(super) use auxiliary::dispatch_auxiliary_task_provider;
#[cfg(test)]
pub(super) use auxiliary::unknown_task_outcome;
pub(crate) use auxiliary::{TaskDispatchClassification, TaskExecutionOutcome};
pub use capabilities::EffectiveCapability;
#[cfg(test)]
pub(super) use capabilities::{
    compiled_openrouter_parameter_spec, openrouter_safe_signed_parameter_specs,
};
pub(super) use capabilities::{
    effective_capability_at, effective_route_parameter_specs, validate_capability_wire_metadata,
};
pub use credential::{ConnectionBoundCredential, GenerationCredentialAdmissionLease};
pub(super) use credential::{GenerationCredential, validate_connection_credential_binding};
pub(super) use delivery::{
    ActiveGenerationGuard, GenerationCompletionContext, GenerationEventForwardingContext,
    GenerationTask, GenerationTransformContext, TerminalPersistenceContext,
};
pub(super) use operation_identity::{
    generation_action_name, new_generation_operation_id,
    same_branch_generation_semantic_fingerprint, validate_same_branch_attempt_semantic_identity,
};
pub(super) use preset::validate_generation_preset_candidate_plan;
pub(crate) use prompt_preparation::BoundedTaskPrompt;
pub(super) use prompt_preparation::{
    ReviewedPromptSendContext, reviewed_prompt_session_seed,
    validate_reviewed_generation_attempt_id,
};
#[cfg(test)]
pub(super) use protocol_request::load_opaque_reasoning_context;
pub(super) use protocol_request::snapshot_provider_request;
pub(crate) use protocol_request::{PromptRouteWireContract, configure_generation_protocol_request};
pub(super) use target_resolution::{
    GenerationProviderTemporalContext, ValidatedGenerationTarget, build_resolved_generation_target,
    generation_target_provider_authority, preflight_generation_target_connection_credential,
    provider_profile_target_authority, provider_profile_temporal_context,
    require_generation_provider_target_authority, validate_generation_target_for_attempt,
    validate_generation_target_plan, validate_generation_target_plan_with_reasoning_effort,
};
pub(crate) use target_resolution::{
    ResolvedGenerationTarget, prompt_route_supports_temperature, prompt_route_wire_contract,
    prompt_route_wire_contract_with_reasoning_effort, resolve_generation_target,
};
#[cfg(test)]
pub(super) use target_resolution::{
    direct_model_provider_target_authority, direct_model_temporal_context,
    generation_target_temporal_context, resolve_generation_target_with_connection_credential,
};
pub(super) use types::{
    GenerationActionSemanticSnapshot, GenerationActionTargetIdentity,
    MessageGenerationActionIdentityInput, ResolvedGenerationOperationIdentity,
    SameBranchGenerationAttemptIdentity,
};
pub use types::{
    GenerationOperationContext, MAX_GENERATION_OPERATION_NONCE_BYTES,
    MAX_GENERATION_OPERATION_NONCE_CHARS,
};
