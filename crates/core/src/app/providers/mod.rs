mod capabilities;
mod connections;
mod routes;
mod templates;

pub(crate) use capabilities::provider_api_capability_observations;
pub(in crate::app) use capabilities::{
    PROVIDER_API_CAPABILITY_FRESHNESS, openrouter_reasoning_dialect_from_capabilities,
};
#[cfg(test)]
pub(in crate::app) use connections::{
    MAX_PROVIDER_BASE_URL_BYTES, MAX_PROVIDER_BASE_URL_CHARS, MAX_PROVIDER_DISPLAY_NAME_BYTES,
    MAX_PROVIDER_DISPLAY_NAME_CHARS, MAX_PROVIDER_ID_BYTES, MAX_PROVIDER_ID_CHARS,
    MAX_PROVIDER_MODEL_BYTES, MAX_PROVIDER_MODEL_CHARS,
};
pub(in crate::app) use routes::validate_settings_generation_target;
pub use templates::ProviderTemplateView;
pub(in crate::app) use templates::validate_provider_template;
