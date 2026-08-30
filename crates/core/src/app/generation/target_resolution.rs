use std::sync::Arc;

use lorepia_domain::{
    ApiFamily, CoreError, CoreErrorCode, CoreResult, GenerationReasoningEffort,
    GenerationReasoningMode, GenerationTarget, ModelAvailability, ModelRoute, ModelRouteId,
    ProviderConnection, ProviderConnectionId, ProviderProfile, ProviderTemplate, Sha256Digest,
};
use lorepia_providers::parameter_mapping::ProviderRequestPlan;
use lorepia_providers::{AdapterRegistry, Provider};
use lorepia_storage::GenerationProviderTargetAuthority;
use serde::Serialize;
#[cfg(test)]
use sha2::{Digest, Sha256};

#[cfg(test)]
use super::GenerationCredential;
use super::{
    ConnectionBoundCredential, GenerationActionTargetIdentity,
    validate_connection_credential_binding,
};
use crate::app::{
    Core, PromptRouteWireContract, canonical_value_sha256, generation_attempt_prompt_authority,
    validate_generation_preset_candidate_plan, validate_provider_template,
};

pub(in crate::app) struct GenerationProviderTemporalContext {
    pub(in crate::app) operation_target: GenerationActionTargetIdentity,
    pub(in crate::app) authority: GenerationProviderTargetAuthority,
}

pub(crate) struct ResolvedGenerationTarget {
    pub(crate) model: String,
    pub(crate) provider: Arc<dyn Provider>,
    pub(crate) api_family: ApiFamily,
    pub(crate) connection_id: ProviderConnectionId,
    pub(in crate::app) preserve_opaque_reasoning_state: bool,
    pub(crate) prompt_wire_contract: PromptRouteWireContract,
}

pub(in crate::app) struct ValidatedGenerationTarget {
    pub(in crate::app) route: ModelRoute,
    pub(in crate::app) connection: ProviderConnection,
    pub(in crate::app) template: ProviderTemplate,
    pub(in crate::app) request_plan: ProviderRequestPlan,
    pub(in crate::app) prompt_wire_contract: PromptRouteWireContract,
}

#[derive(Serialize)]
struct ProviderProfileDispatchAuthoritySnapshot<'a> {
    schema_version: u32,
    provider_profile_id: &'a str,
    base_url: &'a str,
    model: &'a str,
    timeout_seconds: u32,
}

#[derive(Serialize)]
struct GenerationTargetResolutionAuthoritySnapshot<'a> {
    schema_version: u32,
    target: &'a GenerationTarget,
    route: &'a ModelRoute,
    connection: &'a ProviderConnection,
    template: &'a ProviderTemplate,
    request_plan: &'a ProviderRequestPlan,
    prompt_wire_contract: &'a PromptRouteWireContract,
}

pub(in crate::app) fn validate_generation_target_for_attempt(
    core: &Core,
    target: &GenerationTarget,
    attempt: &lorepia_storage::StoredGenerationAttempt,
) -> CoreResult<ValidatedGenerationTarget> {
    let authority = generation_attempt_prompt_authority(attempt)?;
    let validated = validate_generation_target_plan_with_reasoning_effort(
        core,
        target,
        authority.quick_settings.reasoning_effort,
    )?;
    let current = generation_target_provider_authority(target, &validated)?;
    require_generation_provider_target_authority(attempt, &current)?;
    Ok(validated)
}

pub(in crate::app) fn validate_generation_route(
    storage: &lorepia_storage::Storage,
    model_route_id: &ModelRouteId,
) -> CoreResult<(ModelRoute, ProviderConnection, ProviderTemplate)> {
    let route = storage.get_model_route(model_route_id)?;
    if matches!(
        route.status,
        ModelAvailability::MissingTemporarily
            | ModelAvailability::AccessDenied
            | ModelAvailability::Deprecated
            | ModelAvailability::Retired
    ) {
        return Err(CoreError::invalid(
            "selected model route is not currently available for generation",
        ));
    }
    let connection = storage.get_provider_connection(&route.connection_id)?;
    let template =
        storage.get_provider_template(&connection.template_id, connection.template_version)?;
    validate_provider_template(&template)?;
    if route.api_family != template.api_family {
        return Err(CoreError::invalid(
            "model route API family does not match its provider template",
        ));
    }
    Ok((route, connection, template))
}

pub(in crate::app) fn validate_generation_target_plan(
    core: &Core,
    target: &GenerationTarget,
) -> CoreResult<ValidatedGenerationTarget> {
    validate_generation_target_plan_with_reasoning_effort(core, target, None)
}

pub(in crate::app) fn validate_generation_target_plan_with_reasoning_effort(
    core: &Core,
    target: &GenerationTarget,
    requested_reasoning_effort: Option<GenerationReasoningEffort>,
) -> CoreResult<ValidatedGenerationTarget> {
    let mut preset = core
        .inner
        .storage
        .get_generation_preset(&target.generation_preset_id)?;
    if preset.model_route_id != target.model_route_id {
        return Err(CoreError::invalid(
            "generation preset does not belong to the selected model route",
        ));
    }
    let (_, connection, _) =
        validate_generation_route(&core.inner.storage, &preset.model_route_id)?;
    if connection.credential_ref.is_some() {
        preset.reasoning.preserve_opaque_state = false;
    }
    let validated = validate_generation_preset_candidate_plan(core, &preset)?;
    let Some(effort) = requested_reasoning_effort else {
        return Ok(validated);
    };

    let original_mode = preset.reasoning.mode;
    let mut candidate_modes = Vec::with_capacity(3);
    if matches!(
        original_mode,
        GenerationReasoningMode::Enabled | GenerationReasoningMode::Automatic
    ) {
        candidate_modes.push(original_mode);
    }
    for mode in [
        GenerationReasoningMode::Enabled,
        GenerationReasoningMode::Automatic,
    ] {
        if !candidate_modes.contains(&mode) {
            candidate_modes.push(mode);
        }
    }
    for mode in candidate_modes {
        let mut candidate = preset.clone();
        candidate.reasoning.mode = mode;
        candidate.reasoning.effort = Some(effort);
        if let Ok(mut candidate) = validate_generation_preset_candidate_plan(core, &candidate) {
            candidate.prompt_wire_contract.reasoning_effort_applied = Some(effort);
            return Ok(candidate);
        }
    }

    // A quick setting is a bounded overlay, not an unvalidated generic
    // parameter patch. Retain the original exact request plan when this route
    // cannot represent the requested effort; prompt diagnostics report the
    // omission.
    Ok(validated)
}

pub(crate) fn resolve_generation_target(
    core: &Core,
    target: &GenerationTarget,
) -> CoreResult<ResolvedGenerationTarget> {
    let validated = validate_generation_target_plan(core, target)?;
    build_resolved_generation_target(validated)
}

pub(crate) fn prompt_route_wire_contract(
    core: &Core,
    target: &GenerationTarget,
) -> CoreResult<PromptRouteWireContract> {
    let validated = validate_generation_target_plan(core, target)?;
    Ok(validated.prompt_wire_contract)
}

pub(crate) fn prompt_route_wire_contract_with_reasoning_effort(
    core: &Core,
    target: &GenerationTarget,
    requested_reasoning_effort: Option<GenerationReasoningEffort>,
) -> CoreResult<PromptRouteWireContract> {
    let validated = validate_generation_target_plan_with_reasoning_effort(
        core,
        target,
        requested_reasoning_effort,
    )?;
    Ok(validated.prompt_wire_contract)
}

pub(crate) fn prompt_route_supports_temperature(
    core: &Core,
    target: &GenerationTarget,
) -> CoreResult<bool> {
    Ok(validate_generation_target_plan(core, target)?
        .prompt_wire_contract
        .supports_temperature)
}

#[cfg(test)]
pub(in crate::app) fn resolve_generation_target_with_connection_credential(
    core: &Core,
    target: &GenerationTarget,
    credential: ConnectionBoundCredential,
) -> CoreResult<(ResolvedGenerationTarget, GenerationCredential)> {
    let validated = validate_generation_target_plan(core, target)?;
    validate_connection_credential_binding(&validated.connection, &credential)?;
    let resolved = build_resolved_generation_target(validated)?;
    Ok((resolved, credential.into()))
}

pub(in crate::app) fn preflight_generation_target_connection_credential(
    core: &Core,
    target: &GenerationTarget,
    credential: &ConnectionBoundCredential,
) -> CoreResult<()> {
    let validated = validate_generation_target_plan(core, target)?;
    validate_connection_credential_binding(&validated.connection, credential)
}

pub(in crate::app) fn build_resolved_generation_target(
    validated: ValidatedGenerationTarget,
) -> CoreResult<ResolvedGenerationTarget> {
    let preserve_opaque_reasoning_state = validated.connection.credential_ref.is_none()
        && validated.request_plan.preserves_opaque_reasoning_state();
    let prompt_wire_contract = validated.prompt_wire_contract;
    let provider = AdapterRegistry::new().build_provider_for_route_with_plan(
        &validated.template,
        &validated.connection,
        &validated.route,
        Some(validated.request_plan),
    )?;

    Ok(ResolvedGenerationTarget {
        model: validated.route.model_id,
        provider,
        api_family: validated.route.api_family,
        connection_id: validated.connection.id,
        preserve_opaque_reasoning_state,
        prompt_wire_contract,
    })
}

#[cfg(test)]
pub(in crate::app) fn direct_model_provider_target_authority(
    model: &str,
) -> CoreResult<GenerationProviderTargetAuthority> {
    let digest = format!("{:x}", Sha256::digest(model.as_bytes()));
    Ok(GenerationProviderTargetAuthority::DirectModel {
        model_sha256: Sha256Digest::parse(digest).map_err(CoreError::invalid)?,
    })
}

#[cfg(test)]
pub(in crate::app) fn direct_model_temporal_context(
    model: &str,
) -> CoreResult<GenerationProviderTemporalContext> {
    let authority = direct_model_provider_target_authority(model)?;
    let GenerationProviderTargetAuthority::DirectModel { model_sha256 } = &authority else {
        unreachable!("direct-model authority constructor returned another variant");
    };
    Ok(GenerationProviderTemporalContext {
        operation_target: GenerationActionTargetIdentity::DirectModel {
            model_sha256: model_sha256.as_str().to_owned(),
        },
        authority,
    })
}

pub(in crate::app) fn provider_profile_target_authority(
    profile: &ProviderProfile,
) -> CoreResult<GenerationProviderTargetAuthority> {
    let digest = canonical_value_sha256(
        &ProviderProfileDispatchAuthoritySnapshot {
            schema_version: 1,
            provider_profile_id: &profile.id,
            base_url: &profile.base_url,
            model: &profile.model,
            timeout_seconds: profile.timeout_seconds,
        },
        "provider profile dispatch authority",
    )?;
    Ok(GenerationProviderTargetAuthority::ProviderProfile {
        provider_profile_id: profile.id.clone(),
        dispatch_snapshot_sha256: Sha256Digest::parse(digest).map_err(CoreError::invalid)?,
    })
}

pub(in crate::app) fn provider_profile_temporal_context(
    profile: &ProviderProfile,
) -> CoreResult<GenerationProviderTemporalContext> {
    Ok(GenerationProviderTemporalContext {
        operation_target: GenerationActionTargetIdentity::ProviderProfile {
            provider_profile_id: profile.id.clone(),
        },
        authority: provider_profile_target_authority(profile)?,
    })
}

pub(in crate::app) fn generation_target_provider_authority(
    target: &GenerationTarget,
    validated: &ValidatedGenerationTarget,
) -> CoreResult<GenerationProviderTargetAuthority> {
    let digest = canonical_value_sha256(
        &GenerationTargetResolutionAuthoritySnapshot {
            schema_version: 1,
            target,
            route: &validated.route,
            connection: &validated.connection,
            template: &validated.template,
            request_plan: &validated.request_plan,
            prompt_wire_contract: &validated.prompt_wire_contract,
        },
        "generation target resolution authority",
    )?;
    Ok(GenerationProviderTargetAuthority::GenerationTarget {
        target: target.clone(),
        resolved_snapshot_sha256: Sha256Digest::parse(digest).map_err(CoreError::invalid)?,
    })
}

#[cfg(test)]
pub(in crate::app) fn generation_target_temporal_context(
    target: &GenerationTarget,
    validated: &ValidatedGenerationTarget,
) -> CoreResult<GenerationProviderTemporalContext> {
    Ok(GenerationProviderTemporalContext {
        operation_target: GenerationActionTargetIdentity::GenerationTarget {
            model_route_id: target.model_route_id.clone(),
            generation_preset_id: target.generation_preset_id.clone(),
        },
        authority: generation_target_provider_authority(target, validated)?,
    })
}

pub(in crate::app) fn require_generation_provider_target_authority(
    attempt: &lorepia_storage::StoredGenerationAttempt,
    current: &GenerationProviderTargetAuthority,
) -> CoreResult<()> {
    let sealed = generation_attempt_prompt_authority(attempt)?
        .provider_target_authority
        .as_ref()
        .ok_or_else(|| {
            CoreError::new(
                CoreErrorCode::InvalidInput,
                "legacy generation attempt has no provider target authority; start a new generation operation",
                true,
            )
        })?;
    if sealed != current {
        return Err(CoreError::new(
            CoreErrorCode::InvalidInput,
            "provider configuration changed after generation review; start a new generation operation",
            true,
        ));
    }
    Ok(())
}
