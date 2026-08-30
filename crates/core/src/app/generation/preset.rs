use chrono::Utc;
use lorepia_domain::{
    ApiFamily, CapabilityKey, CapabilityValue, CoreError, CoreResult, GenerationPreset,
    GenerationPresetId, GenerationTarget, ModelRoute, ModelRouteId, ObservationSource,
    ProviderConnection, ProviderTemplate,
};
use lorepia_providers::parameter_mapping::{
    GEMINI_OPAQUE_REASONING_TOPOLOGY_ERROR, ParameterEngine, PromptCacheControlModel,
    PromptCacheSettings, PromptCacheWireDialect, ProviderRequestPlan, ReasoningControlModel,
    ReasoningSettings, ReasoningWireDialect, render_prompt_cache_control, render_reasoning_control,
    validate_and_build_provider_request_plan,
};
use lorepia_providers::{
    AdapterRegistry, DeveloperRoleCapability, OPAQUE_REASONING_STATE_UNSUPPORTED_ERROR,
    RequestPreview,
};

use super::capabilities::{
    effective_capability_at, effective_prompt_cache_dialect, effective_reasoning_dialect,
    effective_route_parameter_specs, fresh_openrouter_route_metadata,
    is_exact_built_in_openrouter_template,
};
use super::target_resolution::{
    ValidatedGenerationTarget, validate_generation_route, validate_generation_target_plan,
};
use crate::app::{
    Core, PromptRouteWireContract, canonical_value_sha256,
    openrouter_reasoning_dialect_from_capabilities,
};

struct GenerationPresetControlContext {
    route: ModelRoute,
    connection: ProviderConnection,
    template: ProviderTemplate,
    parameter_engine: ParameterEngine,
    reasoning: ReasoningSettings,
    prompt_cache: PromptCacheSettings,
    reasoning_dialect: ReasoningWireDialect,
    cache_dialect: PromptCacheWireDialect,
}

impl Core {
    /// Validates an unsaved preset candidate against the effective route
    /// catalog and capability dialects. Callers may safely use this before
    /// save; [`Self::upsert_generation_preset`] always applies the same gate.
    pub fn validate_generation_preset_candidate(
        &self,
        preset: &GenerationPreset,
    ) -> CoreResult<()> {
        validate_generation_preset_candidate_plan(self, preset).map(|_| ())
    }

    /// Returns the render-ready, model-specific reasoning controls for a
    /// stored or unsaved preset candidate. Native UI must not reconstruct
    /// these rules from an API-family name.
    pub fn render_reasoning_control_for_preset(
        &self,
        preset: &GenerationPreset,
    ) -> CoreResult<ReasoningControlModel> {
        let context = generation_preset_control_context(self, preset)?;
        let mut reasoning = context.reasoning;
        if context.connection.credential_ref.is_some()
            || !AdapterRegistry::template_supports_opaque_reasoning_state(&context.template)
        {
            reasoning.preserve_opaque_state = false;
        }
        Ok(render_reasoning_control(
            context.route.api_family,
            &context.reasoning_dialect,
            &reasoning,
        ))
    }

    /// Returns the render-ready, model-specific prompt-cache controls for a
    /// stored or unsaved preset candidate.
    pub fn render_prompt_cache_control_for_preset(
        &self,
        preset: &GenerationPreset,
    ) -> CoreResult<PromptCacheControlModel> {
        let context = generation_preset_control_context(self, preset)?;
        Ok(render_prompt_cache_control(
            context.route.api_family,
            context.cache_dialect,
            &context.prompt_cache,
        ))
    }

    /// Previews an unsaved preset through the same validation and adapter
    /// contract used by save and generation.
    pub fn preview_provider_request_candidate(
        &self,
        preset: &GenerationPreset,
    ) -> CoreResult<RequestPreview> {
        let validated = validate_generation_preset_candidate_plan(self, preset)?;
        AdapterRegistry::new().preview_provider_request(
            &validated.template,
            &validated.connection,
            &validated.route,
            Some(&validated.request_plan),
        )
    }

    /// Validates the same stored route/preset pair and family-specific request
    /// plan that generation will use, without constructing a provider or
    /// performing network work.
    pub fn validate_generation_preset(
        &self,
        model_route_id: &ModelRouteId,
        generation_preset_id: &GenerationPresetId,
    ) -> CoreResult<()> {
        validate_generation_target_plan(
            self,
            &GenerationTarget {
                model_route_id: model_route_id.clone(),
                generation_preset_id: generation_preset_id.clone(),
            },
        )
        .map(|_| ())
    }

    /// Returns a scalar-free, credential-free preview produced by the same
    /// family adapter and validated request plan used for generation.
    pub fn preview_provider_request(
        &self,
        model_route_id: &ModelRouteId,
        generation_preset_id: &GenerationPresetId,
    ) -> CoreResult<RequestPreview> {
        let validated = validate_generation_target_plan(
            self,
            &GenerationTarget {
                model_route_id: model_route_id.clone(),
                generation_preset_id: generation_preset_id.clone(),
            },
        )?;
        AdapterRegistry::new().preview_provider_request(
            &validated.template,
            &validated.connection,
            &validated.route,
            Some(&validated.request_plan),
        )
    }
}

fn generation_preset_control_context(
    core: &Core,
    preset: &GenerationPreset,
) -> CoreResult<GenerationPresetControlContext> {
    let storage = &core.inner.storage;
    let (route, connection, template) = validate_generation_route(storage, &preset.model_route_id)?;
    let evaluated_at = Utc::now();
    let catalog = core
        .operational_provider_catalog_projection_at(evaluated_at)?
        .route_projection(&route, &connection.template_id);
    let base_parameter_specs = if catalog.matched {
        catalog.parameters.clone()
    } else {
        template.default_manifest.parameters.clone()
    };
    let parameter_specs = effective_route_parameter_specs(
        &route,
        &template,
        &base_parameter_specs,
        &catalog.signed_parameters,
        evaluated_at,
    )?;
    let parameter_engine =
        ParameterEngine::from_manifest_specs_for_family(route.api_family, &parameter_specs)
            .map_err(|error| {
                CoreError::invalid(format!(
                    "provider parameter manifest is invalid for this model route: {error}"
                ))
            })?;
    let reasoning = ReasoningSettings::from(&preset.reasoning);
    let prompt_cache = PromptCacheSettings::from(&preset.prompt_cache);
    let reasoning_capability = effective_capability_at(
        storage,
        &catalog.capability_observations,
        &route.id,
        CapabilityKey::Reasoning,
        evaluated_at,
    )?;
    let cache_capability = effective_capability_at(
        storage,
        &catalog.capability_observations,
        &route.id,
        CapabilityKey::PromptCaching,
        evaluated_at,
    )?;
    let mut reasoning_dialect =
        effective_reasoning_dialect(route.api_family, reasoning_capability.as_ref());
    if matches!(reasoning_dialect, ReasoningWireDialect::OpenRouter { .. }) {
        let exact_template = is_exact_built_in_openrouter_template(&template)?;
        let metadata_matches_route =
            fresh_openrouter_route_metadata(&route, &template, evaluated_at)?.is_some_and(
                |metadata| {
                    let observation_time_matches =
                        reasoning_capability.as_ref().is_some_and(|capability| {
                            capability.selected.source != ObservationSource::ProviderApi
                                || capability.selected.observed_at == metadata.observed_at
                        });
                    observation_time_matches
                        && openrouter_reasoning_dialect_from_capabilities(&metadata.capabilities)
                            .is_some_and(|dialect| dialect == reasoning_dialect)
                },
            );
        if !exact_template || !metadata_matches_route {
            reasoning_dialect = ReasoningWireDialect::Unsupported;
        }
    }
    let cache_dialect = effective_prompt_cache_dialect(route.api_family, cache_capability.as_ref());

    Ok(GenerationPresetControlContext {
        route,
        connection,
        template,
        parameter_engine,
        reasoning,
        prompt_cache,
        reasoning_dialect,
        cache_dialect,
    })
}

pub(in crate::app) fn validate_generation_preset_candidate_plan(
    core: &Core,
    preset: &GenerationPreset,
) -> CoreResult<ValidatedGenerationTarget> {
    let context = generation_preset_control_context(core, preset)?;
    validate_opaque_reasoning_state_support(
        &context.template,
        &context.connection,
        &context.reasoning,
    )?;
    // A family name alone is not evidence that a particular model supports a
    // reasoning or cache control. Only a fresh, non-conflicting, sufficiently
    // confident observation with an exact structured dialect can enable those
    // controls. Provider-default remains the only lossless fallback.
    let request_plan = validate_and_build_provider_request_plan(
        &context.parameter_engine,
        context.route.api_family,
        &preset.values,
        &context.reasoning,
        &context.reasoning_dialect,
        &context.prompt_cache,
        context.cache_dialect,
    )
    .map_err(|error| {
        CoreError::invalid(format!(
            "generation preset cannot be represented by this model route: {error}"
        ))
    })?;
    let developer_capability = match context.route.api_family {
        ApiFamily::OpenAiResponses => DeveloperRoleCapability::Supported,
        ApiFamily::OpenAiChatCompletions => DeveloperRoleCapability::Unknown,
        ApiFamily::AnthropicMessages
        | ApiFamily::GeminiGenerateContent
        | ApiFamily::OllamaNative => DeveloperRoleCapability::Unsupported,
    };
    let parameter_evaluation = context.parameter_engine.evaluate(&preset.values);
    let supports_temperature = parameter_evaluation
        .editor
        .basic
        .iter()
        .chain(&parameter_evaluation.editor.advanced)
        .chain(&parameter_evaluation.editor.expert)
        .any(|control| {
            control.id.as_str().eq_ignore_ascii_case("temperature")
                && control.visible
                && control.enabled
        });
    let prompt_wire_contract = PromptRouteWireContract {
        model_route_id: context.route.id.clone(),
        generation_preset_id: preset.id.clone(),
        model: context.route.model_id.clone(),
        api_family: context.route.api_family,
        developer_capability,
        cache_dialect: context.cache_dialect,
        request_plan_sha256: canonical_value_sha256(&request_plan, "provider request plan")?,
        generation_preset_sha256: canonical_value_sha256(preset, "generation preset")?,
        configured_max_output_tokens: configured_max_output_tokens(&request_plan),
        context_limit_tokens: observed_positive_integer_capability(
            core,
            &context.route.id,
            CapabilityKey::ContextWindow,
        )?,
        observed_max_output_tokens: observed_positive_integer_capability(
            core,
            &context.route.id,
            CapabilityKey::MaxOutputTokens,
        )?,
        supports_temperature,
        reasoning_effort_applied: None,
    };

    Ok(ValidatedGenerationTarget {
        route: context.route,
        connection: context.connection,
        template: context.template,
        request_plan,
        prompt_wire_contract,
    })
}

fn validate_opaque_reasoning_state_support(
    template: &ProviderTemplate,
    connection: &ProviderConnection,
    reasoning: &ReasoningSettings,
) -> CoreResult<()> {
    if !reasoning.preserve_opaque_state {
        return Ok(());
    }
    if !AdapterRegistry::template_supports_opaque_reasoning_state(template) {
        let message = if template.api_family == ApiFamily::GeminiGenerateContent {
            GEMINI_OPAQUE_REASONING_TOPOLOGY_ERROR
        } else {
            OPAQUE_REASONING_STATE_UNSUPPORTED_ERROR
        };
        return Err(CoreError::invalid(message));
    }
    if connection.credential_ref.is_some() {
        return Err(CoreError::invalid(OPAQUE_REASONING_STATE_UNSUPPORTED_ERROR));
    }
    Ok(())
}

fn configured_max_output_tokens(plan: &ProviderRequestPlan) -> Option<u32> {
    const OUTPUT_TOKEN_PATHS: [&str; 5] = [
        "max_output_tokens",
        "max_tokens",
        "max_completion_tokens",
        "generationConfig.maxOutputTokens",
        "options.num_predict",
    ];
    plan.body_patches()
        .iter()
        .find(|patch| OUTPUT_TOKEN_PATHS.contains(&patch.path()))
        .and_then(|patch| patch.value().as_u64())
        .and_then(|tokens| u32::try_from(tokens).ok())
        .filter(|tokens| *tokens > 0)
}

fn observed_positive_integer_capability(
    core: &Core,
    model_route_id: &ModelRouteId,
    key: CapabilityKey,
) -> CoreResult<Option<u32>> {
    Ok(core
        .effective_capability(model_route_id, key)?
        .filter(|capability| !capability.has_conflict && !capability.selected_is_stale)
        .and_then(|capability| match capability.selected.value {
            CapabilityValue::Integer(value) => u32::try_from(value).ok(),
            _ => None,
        })
        .filter(|value| *value > 0))
}
