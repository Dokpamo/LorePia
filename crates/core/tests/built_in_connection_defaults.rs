use chrono::Utc;
use lorepia_core::{Core, CoreConfig};
use lorepia_domain::{
    ApiFamily, AuthBinding, CanonicalOrigin, ConnectionConfigEntry, ConnectionConfigValue,
    EndpointPath, ModelAvailability, ModelMetadataSource, ModelRoute, ModelRouteConfig,
    ModelRouteId, ProviderConnection, ProviderConnectionDraft, ProviderConnectionId,
    ProviderTemplate, ProviderTemplateId, TemplateSource,
};
use lorepia_providers::{AdapterRegistry, BuiltInTemplateId};
use lorepia_storage::Storage;
use tempfile::tempdir;

fn template(core: &Core, id: BuiltInTemplateId) -> ProviderTemplate {
    core.list_provider_templates()
        .expect("list provider templates")
        .into_iter()
        .find(|template| template.id.as_str() == id.as_str())
        .expect("compiled built-in template")
}

fn api_origin(template: &ProviderTemplate) -> CanonicalOrigin {
    template
        .default_manifest
        .default_api_origin
        .clone()
        .unwrap_or_else(|| {
            CanonicalOrigin::parse("https://api.openai.com")
                .expect("custom OpenAI-compatible origin")
        })
}

fn connection_values(
    id: BuiltInTemplateId,
    origin: &CanonicalOrigin,
) -> Vec<ConnectionConfigEntry> {
    if id != BuiltInTemplateId::OpenAiChatCompatible {
        return Vec::new();
    }
    vec![ConnectionConfigEntry {
        key: "api_base_url".to_owned(),
        value: ConnectionConfigValue::Text(format!(
            "{}{}",
            origin.as_str(),
            id.default_api_base_path()
        )),
    }]
}

fn create_draft(
    template: &ProviderTemplate,
    id: BuiltInTemplateId,
    api_base_path: Option<EndpointPath>,
) -> ProviderConnectionDraft {
    let origin = api_origin(template);
    let descriptor =
        AdapterRegistry::descriptor(template.api_family).expect("compiled adapter descriptor");
    ProviderConnectionDraft {
        id: ProviderConnectionId::from(format!("{}-connection", id.as_str())),
        template_id: template.id.clone(),
        template_version: template.manifest_version,
        display_name: format!("{} connection", template.display_name),
        api_origin: origin.clone(),
        api_base_path,
        network_mode: descriptor.default_network_mode,
        local_network_approval: None,
        values: connection_values(id, &origin),
        approved_credential_origin: match template.default_manifest.auth {
            AuthBinding::None => None,
            AuthBinding::BearerHeader | AuthBinding::HeaderApiKey { .. } => Some(origin),
        },
        timeout_seconds: 30,
    }
}

fn synthetic_route(connection: &ProviderConnection, api_family: ApiFamily) -> ModelRoute {
    let now = Utc::now();
    ModelRoute {
        id: ModelRouteId::from(format!("{}-route", connection.id.as_str())),
        connection_id: connection.id.clone(),
        api_family,
        model_id: "synthetic-model".to_owned(),
        display_name: Some("Synthetic model".to_owned()),
        route_config: ModelRouteConfig::default(),
        status: ModelAvailability::Available,
        miss_count: 0,
        raw_metadata: None,
        metadata_source: ModelMetadataSource::UserOverride,
        metadata_observed_at: None,
        last_reconciled_sync_job_id: None,
        metadata_sync_job_id: None,
        first_seen_at: now,
        last_seen_at: Some(now),
    }
}

#[test]
fn missing_api_base_path_uses_every_exact_compiled_built_in_default() {
    let root = tempdir().expect("temporary Core root");
    let core = Core::open(CoreConfig::new(root.path())).expect("open Core");

    for id in BuiltInTemplateId::ALL {
        let template = template(&core, id);
        let connection = core
            .create_provider_connection(create_draft(&template, id, None))
            .expect("create built-in provider connection");
        assert_eq!(
            connection
                .config
                .api_base_path
                .as_ref()
                .map(EndpointPath::as_str),
            Some(id.default_api_base_path()),
            "{} must use its identity-specific compiled default",
            id.as_str()
        );
    }
}

#[test]
fn explicit_built_in_override_survives_create_preview_and_upsert() {
    let root = tempdir().expect("temporary Core root");
    let core = Core::open(CoreConfig::new(root.path())).expect("open Core");
    let id = BuiltInTemplateId::OpenRouter;
    let template = template(&core, id);
    let explicit = EndpointPath::parse("/tenant/v9").expect("explicit API base path");
    let connection = core
        .create_provider_connection(create_draft(&template, id, Some(explicit.clone())))
        .expect("create explicitly overridden OpenRouter connection");
    assert_eq!(connection.config.api_base_path, Some(explicit));

    let preview = AdapterRegistry::new()
        .preview_provider_request(
            &template,
            &connection,
            &synthetic_route(&connection, template.api_family),
            None,
        )
        .expect("preview explicit OpenRouter route");
    assert_eq!(preview.path().as_str(), "/tenant/v9/chat/completions");

    let updated = core
        .upsert_provider_connection(ProviderConnection {
            display_name: "Renamed OpenRouter".to_owned(),
            timeout_seconds: 45,
            ..connection
        })
        .expect("upsert normalized connection");
    assert_eq!(
        updated
            .config
            .api_base_path
            .as_ref()
            .map(EndpointPath::as_str),
        Some("/tenant/v9")
    );
}

#[test]
fn user_discovered_openai_template_keeps_missing_base_path_and_unprefixed_route() {
    let root = tempdir().expect("temporary Core root");
    let mut custom = AdapterRegistry::built_in_template(BuiltInTemplateId::OpenAiChatCompatible)
        .expect("compiled OpenAI-compatible template");
    custom.id = ProviderTemplateId::from("user-discovered-openai-chat");
    custom.display_name = "User-discovered OpenAI chat".to_owned();
    custom.source = TemplateSource::UserDiscovered;
    let storage = Storage::open(root.path()).expect("open storage");
    storage
        .save_provider_template(&custom)
        .expect("save user-discovered template");
    drop(storage);

    let core = Core::open(CoreConfig::new(root.path())).expect("open Core");
    let origin = CanonicalOrigin::parse("https://api.openai.com").expect("custom API origin");
    let connection = core
        .create_provider_connection(ProviderConnectionDraft {
            id: ProviderConnectionId::from("user-discovered-connection"),
            template_id: custom.id.clone(),
            template_version: custom.manifest_version,
            display_name: "User-discovered connection".to_owned(),
            api_origin: origin.clone(),
            api_base_path: None,
            network_mode: lorepia_domain::ProviderNetworkMode::Public,
            local_network_approval: None,
            values: vec![ConnectionConfigEntry {
                key: "api_base_url".to_owned(),
                value: ConnectionConfigValue::Text(origin.as_str().to_owned()),
            }],
            approved_credential_origin: Some(origin),
            timeout_seconds: 30,
        })
        .expect("create user-discovered connection");
    assert!(connection.config.api_base_path.is_none());

    let preview = AdapterRegistry::new()
        .preview_provider_request(
            &custom,
            &connection,
            &synthetic_route(&connection, custom.api_family),
            None,
        )
        .expect("preview user-discovered route");
    assert_eq!(preview.path().as_str(), "/chat/completions");
}
