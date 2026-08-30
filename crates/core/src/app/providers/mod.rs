mod capabilities;
mod connections;
mod model_reconciliation;
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
pub use model_reconciliation::{ProviderModelRefreshProvenance, ProviderModelRefreshResult};
pub(crate) use model_reconciliation::{
    ReconciledModelRoutes, initial_generation_preset, reconcile_input_routes,
    template_accepts_empty_preset,
};
#[cfg(test)]
pub(in crate::app) use model_reconciliation::{
    deterministic_model_route_id, listed_model_metadata,
};
pub(in crate::app) use model_reconciliation::{
    ensure_model_list_does_not_reflect_credential, model_record_source_name,
    record_model_refresh_failure,
};
pub(in crate::app) use routes::validate_settings_generation_target;
pub use templates::ProviderTemplateView;
pub(in crate::app) use templates::validate_provider_template;
