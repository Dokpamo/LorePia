use std::collections::HashMap;

use chrono::{DateTime, Utc};
use lorepia_domain::{
    ApiFamily, CapabilityKey, CapabilityObservation, CapabilityValue, Confidence, CoreError,
    CoreErrorCode, CoreResult, ModelAvailability, ModelMetadataSource, ModelRoute, ModelRouteId,
    ObservationSource, ParameterDefaultMode, ParameterId, ParameterSpec, ParameterType,
    ProviderParameterMapping, ProviderParameterTarget, ProviderTemplate, SupportStatus,
    TemplateSource, UiParameterLevel,
};
use lorepia_providers::parameter_mapping::{
    PromptCacheWireDialect, ReasoningWireDialect, parse_prompt_cache_wire_dialect_metadata,
    parse_reasoning_wire_dialect_metadata,
};
use lorepia_providers::{
    AdapterRegistry, BuiltInTemplateId, ListedModelCapabilities, OpenRouterSupportedParameter,
    OpenRouterSupportedParameterSupport, merge_capability_observations,
};
use lorepia_storage::{Storage, validate_provider_api_route_metadata};

use crate::app::PROVIDER_API_CAPABILITY_FRESHNESS;

/// Deterministically merged capability state for one route and key.
///
/// Alternatives remain visible so native UI can explain disagreements rather
/// than presenting the selected value as an unqualified fact.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EffectiveCapability {
    pub selected: CapabilityObservation,
    pub alternatives: Vec<CapabilityObservation>,
    pub evaluated_at: DateTime<Utc>,
    pub selected_is_stale: bool,
    pub has_conflict: bool,
}

pub(in crate::app) fn effective_capability_at(
    storage: &Storage,
    catalog_observations: &[CapabilityObservation],
    model_route_id: &ModelRouteId,
    key: CapabilityKey,
    now: DateTime<Utc>,
) -> CoreResult<Option<EffectiveCapability>> {
    let mut observations = storage
        .list_capability_observations_for_key(model_route_id, key)?
        .into_iter()
        .filter(|observation| observation.source != ObservationSource::SignedLorepiaCatalog)
        .map(|observation| (observation.id.clone(), observation))
        .collect::<HashMap<_, _>>();
    for observation in catalog_observations.iter().filter(|observation| {
        observation.model_route_id == *model_route_id && observation.key == key
    }) {
        observations.insert(observation.id.clone(), observation.clone());
    }
    let observations = observations.into_values().collect::<Vec<_>>();
    if observations.is_empty() {
        return Ok(None);
    }
    let merged = merge_capability_observations(&observations, now)?;
    Ok(Some(EffectiveCapability {
        selected: merged.selected().clone(),
        alternatives: merged.alternatives().to_vec(),
        evaluated_at: now,
        selected_is_stale: merged.selected_is_stale(),
        has_conflict: merged.has_conflict(),
    }))
}

pub(in crate::app) fn validate_capability_wire_metadata(
    route: &ModelRoute,
    template: &ProviderTemplate,
    observation: &CapabilityObservation,
) -> CoreResult<()> {
    let CapabilityValue::Structured(value) = &observation.value else {
        return Ok(());
    };
    match observation.key {
        CapabilityKey::Reasoning => {
            let dialect = parse_reasoning_wire_dialect_metadata(route.api_family, value).map_err(
                |error| {
                    CoreError::invalid(format!(
                        "reasoning capability metadata is invalid for this model route: {error}"
                    ))
                },
            )?;
            if matches!(dialect, ReasoningWireDialect::OpenRouter { .. })
                && !is_exact_built_in_openrouter_template(template)?
            {
                return Err(CoreError::invalid(
                    "OpenRouter reasoning metadata requires the exact built-in OpenRouter template",
                ));
            }
            if dialect == ReasoningWireDialect::Unsupported
                && matches!(
                    observation.status,
                    SupportStatus::Verified | SupportStatus::Documented
                )
            {
                return Err(CoreError::invalid(
                    "a supported reasoning observation requires a concrete wire dialect",
                ));
            }
        }
        CapabilityKey::PromptCaching => {
            let dialect = parse_prompt_cache_wire_dialect_metadata(route.api_family, value)
                .map_err(|error| {
                    CoreError::invalid(format!(
                        "prompt-cache capability metadata is invalid for this model route: {error}"
                    ))
                })?;
            if dialect == PromptCacheWireDialect::Unsupported
                && matches!(
                    observation.status,
                    SupportStatus::Verified | SupportStatus::Documented
                )
            {
                return Err(CoreError::invalid(
                    "a supported prompt-cache observation requires a concrete wire dialect",
                ));
            }
        }
        _ => {}
    }
    Ok(())
}

fn observation_can_drive_wire_mapping(effective: &EffectiveCapability) -> bool {
    !effective.selected_is_stale
        && !effective.has_conflict
        && effective.selected.confidence != Confidence::Low
        && effective.selected.source != ObservationSource::LlmInference
        && matches!(
            effective.selected.status,
            SupportStatus::Verified | SupportStatus::Documented
        )
}

pub(in crate::app) fn effective_reasoning_dialect(
    family: ApiFamily,
    effective: Option<&EffectiveCapability>,
) -> ReasoningWireDialect {
    let Some(effective) = effective.filter(|value| observation_can_drive_wire_mapping(value))
    else {
        return ReasoningWireDialect::Unsupported;
    };
    let CapabilityValue::Structured(value) = &effective.selected.value else {
        return ReasoningWireDialect::Unsupported;
    };
    parse_reasoning_wire_dialect_metadata(family, value)
        .ok()
        .filter(|dialect| *dialect != ReasoningWireDialect::Unsupported)
        .unwrap_or(ReasoningWireDialect::Unsupported)
}

pub(in crate::app) fn effective_prompt_cache_dialect(
    family: ApiFamily,
    effective: Option<&EffectiveCapability>,
) -> PromptCacheWireDialect {
    let Some(effective) = effective.filter(|value| observation_can_drive_wire_mapping(value))
    else {
        return PromptCacheWireDialect::Unsupported;
    };
    let CapabilityValue::Structured(value) = &effective.selected.value else {
        return PromptCacheWireDialect::Unsupported;
    };
    parse_prompt_cache_wire_dialect_metadata(family, value)
        .ok()
        .filter(|dialect| *dialect != PromptCacheWireDialect::Unsupported)
        .unwrap_or(PromptCacheWireDialect::Unsupported)
}

pub(in crate::app) fn is_exact_built_in_openrouter_template(
    template: &ProviderTemplate,
) -> CoreResult<bool> {
    let canonical = AdapterRegistry::built_in_template(BuiltInTemplateId::OpenRouter)?;
    Ok(template.source == TemplateSource::BuiltIn
        && template.id == canonical.id
        && template.manifest_version == canonical.manifest_version)
}

pub(in crate::app) fn effective_route_parameter_specs(
    route: &ModelRoute,
    template: &ProviderTemplate,
    base_specs: &[ParameterSpec],
    signed_model_specs: &[ParameterSpec],
    evaluated_at: DateTime<Utc>,
) -> CoreResult<Vec<ParameterSpec>> {
    if !is_exact_built_in_openrouter_template(template)? {
        return Ok(base_specs.to_vec());
    }
    if route.status != ModelAvailability::Available {
        return Ok(Vec::new());
    }
    let Some(metadata) = fresh_openrouter_route_metadata(route, template, evaluated_at)? else {
        return Ok(openrouter_safe_signed_parameter_specs(signed_model_specs));
    };
    let OpenRouterSupportedParameterSupport::Exact(supported) = metadata.capabilities.parameters
    else {
        return Err(CoreError::new(
            CoreErrorCode::StorageCorrupted,
            "fresh OpenRouter provider metadata lacks exact supported parameters",
            false,
        ));
    };
    Ok(intersect_openrouter_parameter_specs(
        base_specs,
        &supported,
        metadata.max_output_tokens,
    ))
}

pub(in crate::app) struct FreshOpenRouterRouteMetadata {
    pub(in crate::app) capabilities: ListedModelCapabilities,
    pub(in crate::app) max_output_tokens: Option<u64>,
    pub(in crate::app) observed_at: DateTime<Utc>,
}

pub(in crate::app) fn fresh_openrouter_route_metadata(
    route: &ModelRoute,
    template: &ProviderTemplate,
    evaluated_at: DateTime<Utc>,
) -> CoreResult<Option<FreshOpenRouterRouteMetadata>> {
    if !is_exact_built_in_openrouter_template(template)?
        || route.status != ModelAvailability::Available
        || route.metadata_source != ModelMetadataSource::ProviderApi
    {
        return Ok(None);
    }
    let Some(observed_at) = route.metadata_observed_at else {
        return Err(CoreError::new(
            CoreErrorCode::StorageCorrupted,
            "available ProviderApi route lacks a metadata observation time",
            false,
        ));
    };
    if observed_at > evaluated_at {
        return Err(CoreError::new(
            CoreErrorCode::StorageCorrupted,
            "provider model metadata has a future observation time",
            false,
        ));
    }
    match (
        route.last_reconciled_sync_job_id.as_ref(),
        route.metadata_sync_job_id.as_ref(),
    ) {
        (None, None) => {}
        (Some(reconciled), Some(metadata)) if reconciled == metadata => {}
        _ => {
            return Err(CoreError::new(
                CoreErrorCode::StorageCorrupted,
                "provider model metadata synchronization provenance is inconsistent",
                false,
            ));
        }
    }
    let Some(metadata) = route.raw_metadata.as_ref() else {
        return Err(CoreError::new(
            CoreErrorCode::StorageCorrupted,
            "available ProviderApi route lacks normalized model metadata",
            false,
        ));
    };
    validate_provider_api_route_metadata(Some(metadata)).map_err(|error| {
        CoreError::new(
            CoreErrorCode::StorageCorrupted,
            format!(
                "provider model metadata is not canonical: {}",
                error.message
            ),
            false,
        )
    })?;
    let value = serde_json::from_str::<serde_json::Value>(metadata.as_str()).map_err(|_| {
        CoreError::new(
            CoreErrorCode::StorageCorrupted,
            "provider model metadata is invalid JSON",
            false,
        )
    })?;
    let capabilities = serde_json::from_value::<ListedModelCapabilities>(
        value.get("capabilities").cloned().ok_or_else(|| {
            CoreError::new(
                CoreErrorCode::StorageCorrupted,
                "provider model metadata lacks capabilities",
                false,
            )
        })?,
    )
    .map_err(|_| {
        CoreError::new(
            CoreErrorCode::StorageCorrupted,
            "provider model capability metadata is invalid",
            false,
        )
    })?;
    if !matches!(
        capabilities.parameters,
        OpenRouterSupportedParameterSupport::Exact(_)
    ) {
        return Err(CoreError::new(
            CoreErrorCode::StorageCorrupted,
            "ProviderApi OpenRouter route lacks exact supported parameters",
            false,
        ));
    }
    let max_output_tokens = value
        .get("max_output_tokens")
        .and_then(serde_json::Value::as_u64);
    if observed_at
        .checked_add_signed(PROVIDER_API_CAPABILITY_FRESHNESS)
        .is_none_or(|expires_at| expires_at <= evaluated_at)
    {
        return Ok(None);
    }
    Ok(Some(FreshOpenRouterRouteMetadata {
        capabilities,
        max_output_tokens,
        observed_at,
    }))
}

pub(in crate::app) fn openrouter_safe_signed_parameter_specs(
    specs: &[ParameterSpec],
) -> Vec<ParameterSpec> {
    let mut safe_specs = Vec::new();
    let mut output_spec = None::<ParameterSpec>;
    let mut output_uses_completion_alias = false;
    for spec in specs {
        if spec.provider_mapping.target != ProviderParameterTarget::RequestBody {
            continue;
        }
        match spec.provider_mapping.field_name.as_str() {
            "max_tokens" | "max_completion_tokens" => {
                let uses_completion_alias =
                    spec.provider_mapping.field_name == "max_completion_tokens";
                let replace = output_spec.as_ref().is_none_or(|current| {
                    (uses_completion_alias && !output_uses_completion_alias)
                        || (uses_completion_alias == output_uses_completion_alias
                            && current.id.as_str() != "max_output_tokens"
                            && spec.id.as_str() == "max_output_tokens")
                });
                if replace {
                    output_spec = Some(spec.clone());
                    output_uses_completion_alias = uses_completion_alias;
                }
            }
            "temperature" | "top_p" | "frequency_penalty" | "presence_penalty" | "stop"
            | "seed"
                if !safe_specs.iter().any(|existing: &ParameterSpec| {
                    existing.id == spec.id || existing.provider_mapping == spec.provider_mapping
                }) =>
            {
                safe_specs.push(spec.clone());
            }
            _ => {}
        }
    }
    if let Some(mut output) = output_spec {
        let safe_maximum = f64::from(u32::MAX);
        output.id = ParameterId::from("max_output_tokens");
        output.label_key.clear();
        output
            .label_key
            .push_str("provider.parameter.max_output_tokens");
        output.description_key =
            Some("provider.parameter.max_output_tokens.description".to_owned());
        let output_field = if output_uses_completion_alias {
            "max_completion_tokens"
        } else {
            "max_tokens"
        };
        output.provider_mapping.field_name.clear();
        output.provider_mapping.field_name.push_str(output_field);
        output.maximum = Some(
            output
                .maximum
                .map_or(safe_maximum, |maximum| maximum.min(safe_maximum)),
        );
        if output.minimum.is_none_or(|minimum| minimum <= safe_maximum) {
            safe_specs.push(output);
        }
    }
    safe_specs
}

fn intersect_openrouter_parameter_specs(
    base_specs: &[ParameterSpec],
    supported: &[OpenRouterSupportedParameter],
    max_output_tokens: Option<u64>,
) -> Vec<ParameterSpec> {
    let mut specs = Vec::new();
    for spec in base_specs
        .iter()
        .filter(|spec| {
            !matches!(
                spec.provider_mapping.field_name.as_str(),
                "max_tokens" | "max_completion_tokens"
            )
        })
        .filter_map(|spec| openrouter_supported_parameter_spec(spec, supported))
    {
        if let Some(existing) = specs.iter_mut().find(|existing: &&mut ParameterSpec| {
            existing.provider_mapping == spec.provider_mapping
        }) {
            if spec.id.as_str() == "max_output_tokens"
                && existing.id.as_str() != "max_output_tokens"
            {
                *existing = spec;
            }
        } else {
            specs.push(spec);
        }
    }
    if let Some(spec) =
        select_openrouter_output_token_spec(base_specs, supported, max_output_tokens)
    {
        specs.push(spec);
    }
    for spec in openrouter_compiled_parameter_specs(supported) {
        if !specs.iter().any(|existing| {
            existing.id == spec.id || existing.provider_mapping == spec.provider_mapping
        }) {
            specs.push(spec);
        }
    }
    specs
}

fn select_openrouter_output_token_spec(
    base_specs: &[ParameterSpec],
    supported: &[OpenRouterSupportedParameter],
    max_output_tokens: Option<u64>,
) -> Option<ParameterSpec> {
    let preferred_field = if supported.contains(&OpenRouterSupportedParameter::MaxCompletionTokens)
    {
        "max_completion_tokens"
    } else if supported.contains(&OpenRouterSupportedParameter::MaxTokens) {
        "max_tokens"
    } else {
        return None;
    };
    let candidates = base_specs.iter().filter(|spec| {
        spec.provider_mapping.target == ProviderParameterTarget::RequestBody
            && matches!(
                spec.provider_mapping.field_name.as_str(),
                "max_tokens" | "max_completion_tokens"
            )
    });
    let selected = candidates
        .clone()
        .filter(|spec| spec.provider_mapping.field_name == preferred_field)
        .min_by_key(|spec| spec.id.as_str() != "max_output_tokens")
        .or_else(|| candidates.min_by_key(|spec| spec.id.as_str() != "max_output_tokens"))?;
    openrouter_output_token_spec(selected, supported, max_output_tokens)
}

fn openrouter_supported_parameter_spec(
    spec: &ParameterSpec,
    supported: &[OpenRouterSupportedParameter],
) -> Option<ParameterSpec> {
    if spec.provider_mapping.target != ProviderParameterTarget::RequestBody {
        return None;
    }
    let field = spec.provider_mapping.field_name.as_str();
    let parameter = match field {
        "temperature" => OpenRouterSupportedParameter::Temperature,
        "top_p" => OpenRouterSupportedParameter::TopP,
        "frequency_penalty" => OpenRouterSupportedParameter::FrequencyPenalty,
        "presence_penalty" => OpenRouterSupportedParameter::PresencePenalty,
        "stop" => OpenRouterSupportedParameter::Stop,
        "seed" => OpenRouterSupportedParameter::Seed,
        _ => return None,
    };
    supported.contains(&parameter).then(|| spec.clone())
}

fn openrouter_output_token_spec(
    spec: &ParameterSpec,
    supported: &[OpenRouterSupportedParameter],
    max_output_tokens: Option<u64>,
) -> Option<ParameterSpec> {
    let supports_max_tokens = supported.contains(&OpenRouterSupportedParameter::MaxTokens);
    let supports_max_completion =
        supported.contains(&OpenRouterSupportedParameter::MaxCompletionTokens);
    let field = match (supports_max_tokens, supports_max_completion) {
        (_, true) => "max_completion_tokens",
        (true, false) => "max_tokens",
        (false, false) => return None,
    };
    let mut normalized = spec.clone();
    normalized.id = ParameterId::from("max_output_tokens");
    normalized.label_key.clear();
    normalized
        .label_key
        .push_str("provider.parameter.max_output_tokens");
    normalized.description_key =
        Some("provider.parameter.max_output_tokens.description".to_owned());
    normalized.provider_mapping.field_name.clear();
    normalized.provider_mapping.field_name.push_str(field);
    let provider_maximum = f64::from(
        max_output_tokens
            .and_then(|maximum| u32::try_from(maximum).ok())
            .unwrap_or(u32::MAX),
    );
    normalized.maximum = Some(
        normalized
            .maximum
            .map_or(provider_maximum, |maximum| maximum.min(provider_maximum)),
    );
    if normalized
        .minimum
        .is_some_and(|minimum| minimum > provider_maximum)
    {
        return None;
    }
    Some(normalized)
}

fn openrouter_compiled_parameter_specs(
    supported: &[OpenRouterSupportedParameter],
) -> Vec<ParameterSpec> {
    [
        (
            OpenRouterSupportedParameter::FrequencyPenalty,
            compiled_openrouter_parameter_spec(
                "frequency_penalty",
                "frequency_penalty",
                ParameterType::Number,
                Some(-2.0),
                Some(2.0),
                None,
                UiParameterLevel::Advanced,
            ),
        ),
        (
            OpenRouterSupportedParameter::PresencePenalty,
            compiled_openrouter_parameter_spec(
                "presence_penalty",
                "presence_penalty",
                ParameterType::Number,
                Some(-2.0),
                Some(2.0),
                None,
                UiParameterLevel::Advanced,
            ),
        ),
        (
            OpenRouterSupportedParameter::Stop,
            compiled_openrouter_parameter_spec(
                "stop",
                "stop",
                ParameterType::StopSequenceList,
                None,
                None,
                None,
                UiParameterLevel::Advanced,
            ),
        ),
        (
            OpenRouterSupportedParameter::Seed,
            compiled_openrouter_parameter_spec(
                "seed",
                "seed",
                ParameterType::Integer,
                None,
                None,
                Some(1.0),
                UiParameterLevel::Advanced,
            ),
        ),
    ]
    .into_iter()
    .filter_map(|(parameter, spec)| supported.contains(&parameter).then_some(spec))
    .collect()
}

#[allow(clippy::too_many_arguments)]
pub(in crate::app) fn compiled_openrouter_parameter_spec(
    id: &str,
    field_name: &str,
    value_type: ParameterType,
    minimum: Option<f64>,
    maximum: Option<f64>,
    step: Option<f64>,
    level: UiParameterLevel,
) -> ParameterSpec {
    ParameterSpec {
        id: ParameterId::from(id),
        label_key: format!("provider.parameter.{id}"),
        description_key: Some(format!("provider.parameter.{id}.description")),
        value_type,
        allowed_values: Vec::new(),
        minimum,
        maximum,
        step,
        default_mode: ParameterDefaultMode::ProviderDefault,
        visibility: None,
        conflicts: Vec::new(),
        provider_mapping: ProviderParameterMapping {
            target: ProviderParameterTarget::RequestBody,
            field_name: field_name.to_owned(),
        },
        level,
    }
}
