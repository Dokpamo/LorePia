mod admission;
mod capabilities;
mod credential;
mod operation_identity;
mod preset;
mod target_resolution;
mod types;

pub(super) use admission::{
    GenerationLaunchPermit, MAX_ACTIVE_GENERATIONS_PER_CONVERSATION,
    MAX_ACTIVE_GENERATIONS_PER_PROCESS, MAX_ACTIVE_GENERATIONS_PER_PROVIDER,
};
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
pub(super) use operation_identity::{
    generation_action_name, new_generation_operation_id,
    same_branch_generation_semantic_fingerprint, validate_same_branch_attempt_semantic_identity,
};
pub(super) use preset::validate_generation_preset_candidate_plan;
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
