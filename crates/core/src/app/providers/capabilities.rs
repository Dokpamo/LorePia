use std::collections::HashMap;

use chrono::{DateTime, Utc};
use lorepia_domain::{
    CapabilityKey, CapabilityObservation, CapabilityValue, Confidence, CoreError, CoreErrorCode,
    CoreResult, ModelRoute, ModelRouteId, ObservationId, ObservationSource, ProviderTemplate,
    SupportStatus,
};
use lorepia_providers::parameter_mapping::{OpenRouterReasoningWireStyle, ReasoningWireDialect};
use lorepia_providers::{
    ListedModel, ListedModelCapabilities, ListedModelCapability, ListedModelReasoningCapability,
    OpenRouterReasoningEffortSupport, OpenRouterSupportedParameter,
    OpenRouterSupportedParameterSupport,
};
use uuid::Uuid;

use crate::app::{
    Core, EffectiveCapability, effective_capability_at, effective_route_parameter_specs,
    validate_capability_wire_metadata,
};
use crate::catalog::CatalogRouteProjection;

pub(in crate::app) const PROVIDER_API_CAPABILITY_FRESHNESS: chrono::Duration =
    chrono::Duration::hours(24);

impl Core {
    pub fn upsert_capability_observation(
        &self,
        observation: CapabilityObservation,
    ) -> CoreResult<CapabilityObservation> {
        if observation.source == ObservationSource::SignedLorepiaCatalog {
            return Err(CoreError::invalid(
                "signed catalog observations are derived from the active verified catalog and cannot be stored independently",
            ));
        }
        let route = self
            .inner
            .storage
            .get_model_route(&observation.model_route_id)?;
        let connection = self
            .inner
            .storage
            .get_provider_connection(&route.connection_id)?;
        let template = self
            .inner
            .storage
            .get_provider_template(&connection.template_id, connection.template_version)?;
        validate_capability_wire_metadata(&route, &template, &observation)?;
        self.inner
            .storage
            .upsert_capability_observation(&observation)?;
        Ok(observation)
    }

    /// Stores a capability override explicitly authored by the local user.
    ///
    /// Provider API, signed catalog, probe, documentation, and assistant
    /// observations have dedicated trusted ingestion paths and cannot be
    /// impersonated through a native binding.
    pub fn upsert_user_capability_override(
        &self,
        mut observation: CapabilityObservation,
    ) -> CoreResult<CapabilityObservation> {
        if observation.source != ObservationSource::UserOverride {
            return Err(CoreError::invalid(
                "the user override API only accepts user_override observations",
            ));
        }
        if matches!(observation.value, CapabilityValue::Structured(_)) {
            return Err(CoreError::invalid(
                "structured provider wire metadata cannot be authored as a user override",
            ));
        }
        if !matches!(
            observation.status,
            SupportStatus::Verified
                | SupportStatus::Unsupported
                | SupportStatus::Unknown
                | SupportStatus::Conditional
        ) {
            return Err(CoreError::invalid(
                "user override status must be verified, unsupported, unknown, or conditional",
            ));
        }
        observation.confidence = Confidence::High;
        observation.observed_at = Utc::now();
        observation.evidence_ref = None;
        if observation
            .expires_at
            .is_some_and(|expires_at| expires_at <= observation.observed_at)
        {
            return Err(CoreError::invalid(
                "a user capability override expiry must be in the future",
            ));
        }
        self.upsert_capability_observation(observation)
    }

    pub fn list_capability_observations(
        &self,
        model_route_id: &ModelRouteId,
    ) -> CoreResult<Vec<CapabilityObservation>> {
        let now = Utc::now();
        let route = self.inner.storage.get_model_route(model_route_id)?;
        let catalog = self.catalog_route_projection_at(&route, now)?;
        let mut observations = self
            .inner
            .storage
            .list_capability_observations(model_route_id)?
            .into_iter()
            .filter(|observation| observation.source != ObservationSource::SignedLorepiaCatalog)
            .map(|observation| (observation.id.clone(), observation))
            .collect::<HashMap<_, _>>();
        for observation in catalog.capability_observations {
            observations.insert(observation.id.clone(), observation);
        }
        let mut observations = observations.into_values().collect::<Vec<_>>();
        observations.sort_by(|left, right| {
            capability_key_identity(left.key)
                .cmp(capability_key_identity(right.key))
                .then_with(|| left.id.cmp(&right.id))
        });
        Ok(observations)
    }

    pub fn delete_capability_observation(&self, id: &ObservationId) -> CoreResult<()> {
        self.inner.storage.delete_capability_observation(id)
    }

    pub fn delete_user_capability_override(
        &self,
        model_route_id: &ModelRouteId,
        id: &ObservationId,
    ) -> CoreResult<()> {
        let observation = self
            .inner
            .storage
            .list_capability_observations(model_route_id)?
            .into_iter()
            .find(|observation| observation.id == *id)
            .ok_or_else(|| {
                CoreError::new(
                    CoreErrorCode::NotFound,
                    "capability observation was not found",
                    false,
                )
            })?;
        if observation.source != ObservationSource::UserOverride {
            return Err(CoreError::invalid(
                "only user_override observations can be deleted through this API",
            ));
        }
        self.inner.storage.delete_capability_observation(id)
    }

    pub fn effective_capability(
        &self,
        model_route_id: &ModelRouteId,
        key: CapabilityKey,
    ) -> CoreResult<Option<EffectiveCapability>> {
        let now = Utc::now();
        let route = self.inner.storage.get_model_route(model_route_id)?;
        let catalog = self.catalog_route_projection_at(&route, now)?;
        effective_capability_at(
            &self.inner.storage,
            &catalog.capability_observations,
            model_route_id,
            key,
            now,
        )
    }

    /// Return the fresh model-specific parameter contract in effect now.
    ///
    /// Signed exact/glob entries override the family fallback by stable
    /// parameter ID. Stale signed mappings are not allowed to alter a request;
    /// expired layers have already been removed from the active projection.
    pub fn effective_parameter_specs(
        &self,
        model_route_id: &ModelRouteId,
    ) -> CoreResult<Vec<lorepia_domain::ParameterSpec>> {
        let now = Utc::now();
        let route = self.inner.storage.get_model_route(model_route_id)?;
        let connection = self
            .inner
            .storage
            .get_provider_connection(&route.connection_id)?;
        let template = self
            .inner
            .storage
            .get_provider_template(&connection.template_id, connection.template_version)?;
        let catalog = self
            .operational_provider_catalog_projection_at(now)?
            .route_projection(&route, &connection.template_id);
        let base = if catalog.matched {
            catalog.parameters
        } else {
            template.default_manifest.parameters.clone()
        };
        effective_route_parameter_specs(&route, &template, &base, &catalog.signed_parameters, now)
    }

    fn catalog_route_projection_at(
        &self,
        route: &ModelRoute,
        now: DateTime<Utc>,
    ) -> CoreResult<CatalogRouteProjection> {
        let connection = self
            .inner
            .storage
            .get_provider_connection(&route.connection_id)?;
        Ok(self
            .operational_provider_catalog_projection_at(now)?
            .route_projection(route, &connection.template_id))
    }

    /// Atomic ingestion point for direct provider model metadata.
    pub fn record_provider_api_capability_observations(
        &self,
        observations: Vec<CapabilityObservation>,
    ) -> CoreResult<Vec<CapabilityObservation>> {
        self.record_capability_observations_from_source(
            observations,
            ObservationSource::ProviderApi,
        )
    }

    /// Atomic ingestion point for one-shot probe results.
    pub fn record_probe_capability_observations(
        &self,
        observations: Vec<CapabilityObservation>,
    ) -> CoreResult<Vec<CapabilityObservation>> {
        self.record_capability_observations_from_source(
            observations,
            ObservationSource::CapabilityProbe,
        )
    }

    fn record_capability_observations_from_source(
        &self,
        observations: Vec<CapabilityObservation>,
        expected_source: ObservationSource,
    ) -> CoreResult<Vec<CapabilityObservation>> {
        let mut routes = HashMap::<ModelRouteId, (ModelRoute, ProviderTemplate)>::new();
        for observation in &observations {
            if observation.source != expected_source {
                return Err(CoreError::invalid(
                    "capability observation source does not match the ingestion path",
                ));
            }
            let (route, template) = if let Some(route) = routes.get(&observation.model_route_id) {
                route
            } else {
                let route = self
                    .inner
                    .storage
                    .get_model_route(&observation.model_route_id)?;
                let connection = self
                    .inner
                    .storage
                    .get_provider_connection(&route.connection_id)?;
                let template = self
                    .inner
                    .storage
                    .get_provider_template(&connection.template_id, connection.template_version)?;
                routes.insert(observation.model_route_id.clone(), (route, template));
                routes
                    .get(&observation.model_route_id)
                    .expect("inserted capability route")
            };
            validate_capability_wire_metadata(route, template, observation)?;
        }
        self.inner
            .storage
            .upsert_capability_observations(&observations)?;
        Ok(observations)
    }
}

pub(crate) fn provider_api_capability_observations(
    routes: &[ModelRoute],
    listed_models: &[ListedModel],
    observed_at: DateTime<Utc>,
) -> CoreResult<Vec<CapabilityObservation>> {
    let routes_by_model = routes
        .iter()
        .map(|route| (route.model_id.as_str(), route))
        .collect::<HashMap<_, _>>();
    let expires_at = observed_at.checked_add_signed(PROVIDER_API_CAPABILITY_FRESHNESS);
    let mut observations = Vec::new();
    for model in listed_models {
        let route = routes_by_model
            .get(model.model_id.as_str())
            .ok_or_else(|| {
                CoreError::internal("reconciled model route is missing from capability ingestion")
            })?;
        for (key, value) in [
            (CapabilityKey::ContextWindow, model.max_input_tokens),
            (CapabilityKey::MaxOutputTokens, model.max_output_tokens),
        ] {
            let Some(value) = value else {
                continue;
            };
            if value == 0 {
                return Err(CoreError::new(
                    CoreErrorCode::ProviderUnavailable,
                    "provider model metadata contains a zero token limit",
                    false,
                ));
            }
            observations.push(CapabilityObservation {
                id: deterministic_capability_observation_id(
                    &route.id,
                    key,
                    ObservationSource::ProviderApi,
                ),
                model_route_id: route.id.clone(),
                key,
                value: CapabilityValue::Integer(value),
                status: SupportStatus::Verified,
                source: ObservationSource::ProviderApi,
                confidence: Confidence::High,
                observed_at,
                expires_at,
                evidence_ref: None,
            });
        }
        append_listed_model_capability_observations(
            model,
            &route.id,
            observed_at,
            expires_at,
            &mut observations,
        )?;
    }
    observations.sort_by(|left, right| left.id.cmp(&right.id));
    Ok(observations)
}

fn append_listed_model_capability_observations(
    model: &ListedModel,
    route_id: &ModelRouteId,
    observed_at: DateTime<Utc>,
    expires_at: Option<DateTime<Utc>>,
    observations: &mut Vec<CapabilityObservation>,
) -> CoreResult<()> {
    let mut supported = model.capabilities.supported.clone();
    supported.sort();
    supported.dedup();
    let authoritative = matches!(
        model.capabilities.parameters,
        OpenRouterSupportedParameterSupport::Exact(_)
    );
    let capabilities = if authoritative {
        vec![
            ListedModelCapability::Reasoning,
            ListedModelCapability::ToolCalling,
            ListedModelCapability::ParallelToolCalling,
            ListedModelCapability::StructuredOutput,
            ListedModelCapability::JsonMode,
            ListedModelCapability::Logprobs,
            ListedModelCapability::Seed,
        ]
    } else {
        supported.clone()
    };
    for capability in capabilities {
        let key = match capability {
            ListedModelCapability::Reasoning => CapabilityKey::Reasoning,
            ListedModelCapability::ToolCalling => CapabilityKey::ToolCalling,
            ListedModelCapability::ParallelToolCalling => CapabilityKey::ParallelToolCalling,
            ListedModelCapability::StructuredOutput => CapabilityKey::StructuredOutput,
            ListedModelCapability::JsonMode => CapabilityKey::JsonMode,
            ListedModelCapability::Logprobs => CapabilityKey::Logprobs,
            ListedModelCapability::Seed => CapabilityKey::Seed,
        };
        let is_supported = supported.contains(&capability);
        let value = if !is_supported {
            CapabilityValue::Boolean(false)
        } else if capability == ListedModelCapability::Reasoning {
            openrouter_reasoning_capability_value(model)?
        } else {
            CapabilityValue::Boolean(true)
        };
        observations.push(CapabilityObservation {
            id: deterministic_capability_observation_id(
                route_id,
                key,
                ObservationSource::ProviderApi,
            ),
            model_route_id: route_id.clone(),
            key,
            value,
            status: if is_supported {
                SupportStatus::Verified
            } else {
                SupportStatus::Unsupported
            },
            source: ObservationSource::ProviderApi,
            confidence: Confidence::High,
            observed_at,
            expires_at,
            evidence_ref: None,
        });
    }
    Ok(())
}

fn openrouter_reasoning_capability_value(model: &ListedModel) -> CoreResult<CapabilityValue> {
    let Some(dialect) = openrouter_reasoning_dialect_from_capabilities(&model.capabilities) else {
        return Ok(CapabilityValue::Boolean(true));
    };
    serialize_reasoning_capability(dialect)
}

pub(in crate::app) fn openrouter_reasoning_dialect_from_capabilities(
    capabilities: &ListedModelCapabilities,
) -> Option<ReasoningWireDialect> {
    let parameters = match &capabilities.parameters {
        OpenRouterSupportedParameterSupport::Exact(parameters) => parameters,
        OpenRouterSupportedParameterSupport::NotExposed => return None,
    };
    if parameters.contains(&OpenRouterSupportedParameter::Reasoning) {
        let reasoning = capabilities
            .reasoning
            .clone()
            .unwrap_or(ListedModelReasoningCapability {
                supported_efforts: OpenRouterReasoningEffortSupport::NotExposed,
                default_effort: None,
                default_enabled: None,
                supports_max_tokens: None,
                mandatory: None,
            });
        return Some(ReasoningWireDialect::OpenRouter {
            style: OpenRouterReasoningWireStyle::Unified,
            supported_efforts: reasoning.supported_efforts,
            default_effort: reasoning.default_effort,
            default_enabled: reasoning.default_enabled,
            supports_max_tokens: reasoning.supports_max_tokens,
            mandatory: reasoning.mandatory,
        });
    }
    if !parameters.contains(&OpenRouterSupportedParameter::ReasoningEffort) {
        return None;
    }
    let reasoning = capabilities.reasoning.as_ref()?;
    if matches!(
        reasoning.supported_efforts,
        OpenRouterReasoningEffortSupport::NotExposed
    ) || matches!(
        &reasoning.supported_efforts,
        OpenRouterReasoningEffortSupport::Exact(efforts) if efforts.is_empty()
    ) {
        return None;
    }
    Some(ReasoningWireDialect::OpenRouter {
        style: OpenRouterReasoningWireStyle::LegacyReasoningEffort,
        supported_efforts: reasoning.supported_efforts.clone(),
        default_effort: reasoning.default_effort,
        default_enabled: reasoning.default_enabled,
        supports_max_tokens: reasoning.supports_max_tokens,
        mandatory: reasoning.mandatory,
    })
}

fn serialize_reasoning_capability(dialect: ReasoningWireDialect) -> CoreResult<CapabilityValue> {
    serde_json::to_value(dialect)
        .map(CapabilityValue::Structured)
        .map_err(|error| {
            CoreError::new(
                CoreErrorCode::ProviderUnavailable,
                format!("OpenRouter reasoning metadata could not be normalized: {error}"),
                false,
            )
        })
}

fn deterministic_capability_observation_id(
    model_route_id: &ModelRouteId,
    key: CapabilityKey,
    source: ObservationSource,
) -> ObservationId {
    let identity = format!(
        "lorepia:capability-observation:v1\u{0}{}\u{0}{}\u{0}{}",
        model_route_id.as_str(),
        capability_key_identity(key),
        observation_source_identity(source),
    );
    ObservationId::from(Uuid::new_v5(&Uuid::NAMESPACE_URL, identity.as_bytes()).to_string())
}

const fn capability_key_identity(key: CapabilityKey) -> &'static str {
    match key {
        CapabilityKey::Streaming => "streaming",
        CapabilityKey::Reasoning => "reasoning",
        CapabilityKey::PromptCaching => "prompt_caching",
        CapabilityKey::ToolCalling => "tool_calling",
        CapabilityKey::ParallelToolCalling => "parallel_tool_calling",
        CapabilityKey::StructuredOutput => "structured_output",
        CapabilityKey::JsonMode => "json_mode",
        CapabilityKey::ImageInput => "image_input",
        CapabilityKey::AudioInput => "audio_input",
        CapabilityKey::AudioOutput => "audio_output",
        CapabilityKey::Logprobs => "logprobs",
        CapabilityKey::Seed => "seed",
        CapabilityKey::Batch => "batch",
        CapabilityKey::Background => "background",
        CapabilityKey::ContextWindow => "context_window",
        CapabilityKey::MaxOutputTokens => "max_output_tokens",
    }
}

const fn observation_source_identity(source: ObservationSource) -> &'static str {
    match source {
        ObservationSource::ProviderApi => "provider_api",
        ObservationSource::OfficialDocumentation => "official_documentation",
        ObservationSource::SignedLorepiaCatalog => "signed_lorepia_catalog",
        ObservationSource::CapabilityProbe => "capability_probe",
        ObservationSource::UserOverride => "user_override",
        ObservationSource::LlmInference => "llm_inference",
    }
}
