use super::{
    BuiltInTemplateId, CoreError, CoreResult, DiscoverySessionSnapshot, DiscoverySourceIntent,
    ProviderCredentialAccessAuthority, ProviderDiscoverySource, ProviderTemplate,
    ProviderTemplateId, SanitizedDiscoveryInput, Storage, TemplateSource, Utc,
    operational_provider_catalog_projection_for_storage,
};

impl ProviderDiscoverySource {
    pub fn known_provider(template: BuiltInTemplateId) -> Self {
        Self::known_provider_id(lorepia_domain::ProviderTemplateId::from(template.as_str()))
    }

    pub fn known_provider_id(template_id: lorepia_domain::ProviderTemplateId) -> Self {
        Self {
            intent: DiscoverySourceIntent::KnownProvider { template_id },
            transient: None,
            declared_connection_options: None,
            derived_site_url: None,
        }
    }
}

impl crate::app::Core {
    pub fn begin_provider_discovery_known(
        &self,
        input: SanitizedDiscoveryInput,
        template_id: ProviderTemplateId,
    ) -> CoreResult<DiscoverySessionSnapshot> {
        self.begin_provider_discovery_known_with_credential_authority(input, template_id, None)
    }

    pub fn begin_provider_discovery_known_with_credential_authority(
        &self,
        input: SanitizedDiscoveryInput,
        template_id: ProviderTemplateId,
        credential_authority: Option<ProviderCredentialAccessAuthority>,
    ) -> CoreResult<DiscoverySessionSnapshot> {
        self.provider_discovery().begin_with_credential_authority(
            input,
            ProviderDiscoverySource::known_provider_id(template_id),
            credential_authority,
        )
    }
}

pub(super) fn active_discovery_templates(storage: &Storage) -> CoreResult<Vec<ProviderTemplate>> {
    let mut active = std::collections::BTreeMap::<ProviderTemplateId, ProviderTemplate>::new();
    for template in storage.list_provider_templates()? {
        if template.source != TemplateSource::SignedCatalog {
            insert_active_discovery_template(&mut active, template)?;
        }
    }

    let projection = operational_provider_catalog_projection_for_storage(storage, Utc::now())?;
    for template in projection.provider_templates() {
        insert_active_discovery_template(&mut active, template)?;
    }
    Ok(active.into_values().collect())
}

fn insert_active_discovery_template(
    active: &mut std::collections::BTreeMap<ProviderTemplateId, ProviderTemplate>,
    candidate: ProviderTemplate,
) -> CoreResult<()> {
    match active.get(&candidate.id) {
        Some(existing) if existing.manifest_version > candidate.manifest_version => Ok(()),
        Some(existing)
            if existing.manifest_version == candidate.manifest_version
                && existing != &candidate =>
        {
            Err(CoreError::internal(
                "active provider catalog contains conflicting immutable template versions",
            ))
        }
        _ => {
            active.insert(candidate.id.clone(), candidate);
            Ok(())
        }
    }
}
