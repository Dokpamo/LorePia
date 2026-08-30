use std::collections::{HashMap, HashSet};

use chrono::Utc;
use lorepia_domain::{
    CoreError, CoreResult, EndpointPath, ProviderNetworkMode, ProviderTemplate, TemplateSource,
};
use lorepia_providers::{
    AdapterRegistry, BuiltInTemplateId, validate_connection_fields, validate_manifest,
};

use crate::app::Core;

/// Native-facing provider-template presentation derived by Rust.
///
/// `default_network_mode` comes from the compiled adapter descriptor rather
/// than from native inference or persisted template JSON. This keeps Ollama's
/// loopback boundary explicit while every other built-in family defaults to
/// the public network policy.
#[derive(Debug, Clone, PartialEq)]
pub struct ProviderTemplateView {
    pub template: ProviderTemplate,
    pub default_network_mode: ProviderNetworkMode,
}

pub(super) fn compiled_built_in_default_api_base_path(
    template: &ProviderTemplate,
) -> CoreResult<Option<EndpointPath>> {
    if template.source != TemplateSource::BuiltIn {
        return Ok(None);
    }
    let Some(id) = BuiltInTemplateId::ALL
        .into_iter()
        .find(|id| id.as_str() == template.id.as_str())
    else {
        return Ok(None);
    };
    let compiled = AdapterRegistry::built_in_template(id)?;
    if template != &compiled {
        return Ok(None);
    }
    EndpointPath::parse(id.default_api_base_path())
        .map(Some)
        .map_err(|error| {
            CoreError::internal(format!(
                "compiled provider API base path is invalid: {error}"
            ))
        })
}

impl Core {
    pub fn list_provider_templates(&self) -> CoreResult<Vec<ProviderTemplate>> {
        let active_catalog = self.operational_provider_catalog_projection_at(Utc::now())?;
        let active_templates = active_catalog.provider_templates();
        let active_ids = active_templates
            .iter()
            .map(|template| template.id.clone())
            .collect::<HashSet<_>>();
        let mut by_id = self
            .inner
            .storage
            .list_provider_templates()?
            .into_iter()
            // Signed template rows are retained only to keep already-created
            // connections pinned. Visibility is controlled by the atomic
            // active catalog pointer, never by these inert support rows.
            .filter(|template| {
                template.source != TemplateSource::SignedCatalog
                    && !active_ids.contains(&template.id)
            })
            .fold(HashMap::new(), |mut latest, template| {
                latest
                    .entry(template.id.clone())
                    .and_modify(|current: &mut ProviderTemplate| {
                        if template.manifest_version > current.manifest_version {
                            *current = template.clone();
                        }
                    })
                    .or_insert(template);
                latest
            });
        for template in active_templates {
            validate_provider_template(&template)?;
            by_id.insert(template.id.clone(), template);
        }
        let mut templates = by_id.into_values().collect::<Vec<_>>();
        templates.sort_by(|left, right| {
            left.display_name
                .to_lowercase()
                .cmp(&right.display_name.to_lowercase())
                .then_with(|| left.id.cmp(&right.id))
                .then_with(|| right.manifest_version.cmp(&left.manifest_version))
        });
        Ok(templates)
    }

    /// Lists provider templates together with Rust-owned presentation defaults.
    pub fn list_provider_template_views(&self) -> CoreResult<Vec<ProviderTemplateView>> {
        self.list_provider_templates()?
            .into_iter()
            .map(|template| {
                validate_provider_template(&template)?;
                let descriptor = AdapterRegistry::descriptor(template.api_family)?;
                Ok(ProviderTemplateView {
                    template,
                    default_network_mode: descriptor.default_network_mode,
                })
            })
            .collect()
    }
}

pub(in crate::app) fn validate_provider_template(template: &ProviderTemplate) -> CoreResult<()> {
    if template.manifest_version == 0 {
        return Err(CoreError::invalid(
            "provider template version must be positive",
        ));
    }
    if template.api_family != template.default_manifest.api_family {
        return Err(CoreError::invalid(
            "provider template API family does not match its manifest",
        ));
    }
    validate_connection_fields(&template.connection_fields)?;
    validate_manifest(&template.default_manifest)?;
    Ok(())
}
