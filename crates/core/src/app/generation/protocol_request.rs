use lorepia_domain::{
    ApiFamily, CoreError, CoreErrorCode, CoreResult, GenerationPresetId,
    GenerationProviderProvenance, GenerationReasoningEffort, GenerationRequest, GenerationStatus,
    GenerationTarget, Message, MessageRole, MessageStatus, ModelRouteId, OpaqueReasoningContext,
    OpaqueReasoningState, validate_opaque_reasoning_states,
};
use lorepia_providers::parameter_mapping::PromptCacheWireDialect;
use lorepia_providers::{DeveloperRoleCapability, Provider};
use lorepia_storage::Storage;
use serde::Serialize;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct PromptRouteWireContract {
    pub(crate) model_route_id: ModelRouteId,
    pub(crate) generation_preset_id: GenerationPresetId,
    pub(crate) model: String,
    pub(crate) api_family: ApiFamily,
    pub(crate) developer_capability: DeveloperRoleCapability,
    pub(crate) cache_dialect: PromptCacheWireDialect,
    pub(crate) request_plan_sha256: String,
    pub(crate) generation_preset_sha256: String,
    pub(crate) configured_max_output_tokens: Option<u32>,
    pub(crate) context_limit_tokens: Option<u32>,
    pub(crate) observed_max_output_tokens: Option<u32>,
    pub(crate) supports_temperature: bool,
    pub(crate) reasoning_effort_applied: Option<GenerationReasoningEffort>,
}

pub(crate) fn configure_generation_protocol_request(
    storage: &Storage,
    request: &mut GenerationRequest,
    generation_target: Option<&GenerationTarget>,
    provider_family: Option<ApiFamily>,
    mut preserve_opaque_reasoning_state: bool,
) -> CoreResult<()> {
    if preserve_opaque_reasoning_state && let Some(target) = generation_target {
        let route = storage.get_model_route(&target.model_route_id)?;
        let connection = storage.get_provider_connection(&route.connection_id)?;
        if connection.credential_ref.is_some() {
            preserve_opaque_reasoning_state = false;
        }
    }
    let (generation_target, provider_family) = match (generation_target, provider_family) {
        (None, None) if !preserve_opaque_reasoning_state => {
            request.provider_provenance = None;
            request.preserve_opaque_reasoning_state = false;
            request.opaque_reasoning_context.clear();
            return Ok(());
        }
        (Some(target), Some(family)) => (target, family),
        _ => {
            return Err(CoreError::internal(
                "generation provider protocol provenance is inconsistent",
            ));
        }
    };

    let opaque_reasoning_context = if preserve_opaque_reasoning_state {
        load_opaque_reasoning_context(
            storage,
            &request.messages,
            provider_family,
            &request.model,
            generation_target,
        )?
    } else {
        Vec::new()
    };
    request.provider_provenance = Some(GenerationProviderProvenance {
        api_family: provider_family,
        model_route_id: generation_target.model_route_id.clone(),
        generation_preset_id: generation_target.generation_preset_id.clone(),
    });
    request.preserve_opaque_reasoning_state = preserve_opaque_reasoning_state;
    request.opaque_reasoning_context = opaque_reasoning_context;
    Ok(())
}

pub(in crate::app) fn snapshot_provider_request(
    provider: &dyn Provider,
    request: &GenerationRequest,
    generation_target: Option<&GenerationTarget>,
) -> CoreResult<serde_json::Value> {
    match provider.snapshot_request(request) {
        Ok(value) => Ok(value),
        Err(error) => {
            #[cfg(test)]
            if generation_target.is_none()
                && error.code == CoreErrorCode::UnsupportedContent
                && !request.preserve_opaque_reasoning_state
                && request.opaque_reasoning_context.is_empty()
            {
                return serde_json::to_value(request).map_err(|encode_error| {
                    CoreError::internal(format!(
                        "cannot encode synthetic provider request snapshot: {encode_error}"
                    ))
                });
            }
            let _ = generation_target;
            Err(error)
        }
    }
}

pub(in crate::app) fn reject_sensitive_provider_preview_fields(
    value: &serde_json::Value,
) -> CoreResult<()> {
    const FORBIDDEN_KEYS: [&str; 12] = [
        "api_key",
        "apikey",
        "authorization",
        "base_url",
        "credential",
        "credentials",
        "endpoint",
        "headers",
        "opaque_reasoning_context",
        "opaque_reasoning_state",
        "token",
        "url",
    ];
    match value {
        serde_json::Value::Object(values) => {
            for (key, value) in values {
                if FORBIDDEN_KEYS.contains(&key.to_ascii_lowercase().as_str()) {
                    return Err(CoreError::new(
                        CoreErrorCode::PermissionDenied,
                        "provider preview contained a security-sensitive field",
                        false,
                    ));
                }
                reject_sensitive_provider_preview_fields(value)?;
            }
        }
        serde_json::Value::Array(values) => {
            for value in values {
                reject_sensitive_provider_preview_fields(value)?;
            }
        }
        _ => {}
    }
    Ok(())
}

pub(in crate::app) fn load_opaque_reasoning_context(
    storage: &Storage,
    history: &[Message],
    provider_family: ApiFamily,
    model: &str,
    generation_target: &GenerationTarget,
) -> CoreResult<Vec<OpaqueReasoningContext>> {
    let mut contexts = Vec::new();
    let mut states = Vec::<OpaqueReasoningState>::new();
    for message in history {
        if message.role != MessageRole::Assistant || message.status != MessageStatus::Complete {
            continue;
        }
        let Some(generation_id) = message.generation_id.as_ref() else {
            continue;
        };
        if generation_id.is_character_greeting() {
            continue;
        }
        let generation = storage.get_generation(generation_id).map_err(|error| {
            if error.code == CoreErrorCode::NotFound {
                CoreError::new(
                    CoreErrorCode::StorageCorrupted,
                    "assistant message references a missing generation",
                    false,
                )
            } else {
                error
            }
        })?;
        if generation.opaque_reasoning_state.is_empty() {
            continue;
        }
        if generation.status != GenerationStatus::Complete
            || generation.conversation_id != message.conversation_id
            || generation.assistant_message_id.as_ref() != Some(&message.id)
        {
            return Err(CoreError::new(
                CoreErrorCode::StorageCorrupted,
                "stored opaque reasoning state has inconsistent message ownership",
                false,
            ));
        }
        if generation.provider_family != Some(provider_family)
            || generation.model != model
            || generation.model_route_id.as_ref() != Some(&generation_target.model_route_id)
        {
            continue;
        }
        let generation_preset_id = generation.generation_preset_id.ok_or_else(|| {
            CoreError::new(
                CoreErrorCode::StorageCorrupted,
                "stored opaque reasoning state is missing preset provenance",
                false,
            )
        })?;
        for state in generation.opaque_reasoning_state {
            states.push(state.clone());
            contexts.push(OpaqueReasoningContext {
                source_message_id: message.id.clone(),
                api_family: provider_family,
                model: model.to_owned(),
                model_route_id: generation_target.model_route_id.clone(),
                generation_preset_id: generation_preset_id.clone(),
                state,
            });
        }
    }
    validate_opaque_reasoning_states(&states).map_err(CoreError::invalid)?;
    Ok(contexts)
}
