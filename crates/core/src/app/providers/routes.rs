use lorepia_domain::{
    ApiFamily, AppSettings, CoreError, CoreErrorCode, CoreResult, GenerationPreset,
    GenerationPresetId, GenerationTarget, ModelMetadataSource, ModelRoute, ModelRouteConfig,
    ModelRouteId, ProviderConnectionId, ProviderProfile,
};

use crate::app::{Core, validate_generation_target_plan};

enum MigratedLegacyTargetClassification {
    Ordinary,
    Current { profile_id: String },
    Alias,
}

impl Core {
    pub fn list_model_routes(
        &self,
        connection_id: &ProviderConnectionId,
    ) -> CoreResult<Vec<ModelRoute>> {
        self.inner.storage.list_model_routes(connection_id)
    }

    pub fn upsert_model_route(&self, mut route: ModelRoute) -> CoreResult<ModelRoute> {
        if self
            .retained_legacy_profile_for_connection(&route.connection_id)?
            .is_some()
        {
            return Err(CoreError::invalid(
                "migrated legacy model routes are managed through their retained provider profile",
            ));
        }
        match self.inner.storage.get_model_route(&route.id) {
            Ok(existing) => {
                if route.connection_id != existing.connection_id
                    || route.api_family != existing.api_family
                    || route.model_id != existing.model_id
                    || route.route_config != existing.route_config
                    || route.first_seen_at != existing.first_seen_at
                {
                    return Err(CoreError::invalid(
                        "an existing model route cannot be rebound to another provider, model, or route discriminator",
                    ));
                }
                // Refresh/catalog provenance is owned by trusted Rust
                // ingestion paths. A native edit may change only the
                // user-facing label and availability.
                route.miss_count = existing.miss_count;
                route.raw_metadata = existing.raw_metadata;
                route.metadata_source = existing.metadata_source;
                route.metadata_observed_at = existing.metadata_observed_at;
                route.last_reconciled_sync_job_id = existing.last_reconciled_sync_job_id;
                route.metadata_sync_job_id = existing.metadata_sync_job_id;
                route.last_seen_at = existing.last_seen_at;
            }
            Err(error) if error.code == CoreErrorCode::NotFound => {
                let connection = self
                    .inner
                    .storage
                    .get_provider_connection(&route.connection_id)?;
                let template = self
                    .inner
                    .storage
                    .get_provider_template(&connection.template_id, connection.template_version)?;
                if route.api_family != template.api_family {
                    return Err(CoreError::invalid(
                        "model route API family does not match its provider template",
                    ));
                }
                if route.miss_count != 0
                    || route.raw_metadata.is_some()
                    || !matches!(
                        route.metadata_source,
                        ModelMetadataSource::Legacy | ModelMetadataSource::UserOverride
                    )
                    || route.metadata_observed_at.is_some()
                    || route.last_reconciled_sync_job_id.is_some()
                    || route.metadata_sync_job_id.is_some()
                {
                    return Err(CoreError::invalid(
                        "a native-created model route cannot claim provider, catalog, probe, or synchronization provenance",
                    ));
                }
            }
            Err(error) => return Err(error),
        }
        self.inner.storage.save_model_route(&route)?;
        Ok(route)
    }

    pub fn delete_model_route(&self, id: &ModelRouteId) -> CoreResult<()> {
        let route = self.inner.storage.get_model_route(id)?;
        if self.is_current_migrated_legacy_route(&route)? {
            return Err(CoreError::invalid(
                "the migrated legacy profile's current model route cannot be deleted independently",
            ));
        }
        self.inner.storage.delete_model_route(id)
    }

    pub fn list_generation_presets(
        &self,
        model_route_id: &ModelRouteId,
    ) -> CoreResult<Vec<GenerationPreset>> {
        self.inner.storage.list_generation_presets(model_route_id)
    }

    pub fn upsert_generation_preset(
        &self,
        preset: GenerationPreset,
    ) -> CoreResult<GenerationPreset> {
        let route = self.inner.storage.get_model_route(&preset.model_route_id)?;
        if self
            .retained_legacy_profile_for_connection(&route.connection_id)?
            .is_some()
        {
            return Err(CoreError::invalid(
                "migrated legacy generation presets are managed through their retained provider profile",
            ));
        }
        self.validate_generation_preset_candidate(&preset)?;
        self.inner.storage.save_generation_preset(&preset)?;
        Ok(preset)
    }

    pub fn delete_generation_preset(&self, id: &GenerationPresetId) -> CoreResult<()> {
        let preset = self.inner.storage.get_generation_preset(id)?;
        let route = self.inner.storage.get_model_route(&preset.model_route_id)?;
        if preset.id.as_str() == route.id.as_str()
            && self.is_current_migrated_legacy_route(&route)?
        {
            return Err(CoreError::invalid(
                "the migrated legacy profile's current generation preset cannot be deleted independently",
            ));
        }
        self.inner.storage.delete_generation_preset(id)
    }

    pub fn select_generation_target(
        &self,
        target: Option<GenerationTarget>,
    ) -> CoreResult<AppSettings> {
        let (selected_provider_profile_id, selected_model_route_id, selected_generation_preset_id) =
            if let Some(target) = target {
                validate_generation_target_plan(self, &target)?;
                let selected_provider_profile_id = match self
                    .classify_migrated_legacy_target(&target)?
                {
                    MigratedLegacyTargetClassification::Ordinary => None,
                    MigratedLegacyTargetClassification::Current { profile_id } => Some(profile_id),
                    MigratedLegacyTargetClassification::Alias => {
                        return Err(CoreError::invalid(
                            "select the retained legacy provider profile instead of a custom target from its migrated connection",
                        ));
                    }
                };
                (
                    selected_provider_profile_id,
                    Some(target.model_route_id),
                    Some(target.generation_preset_id),
                )
            } else {
                (None, None, None)
            };
        self.inner.storage.save_generation_target_selection(
            selected_provider_profile_id,
            selected_model_route_id,
            selected_generation_preset_id,
        )
    }

    fn retained_legacy_profile_for_connection(
        &self,
        connection_id: &ProviderConnectionId,
    ) -> CoreResult<Option<ProviderProfile>> {
        match self
            .inner
            .storage
            .get_provider_profile(connection_id.as_str())
        {
            Ok(profile) => Ok(Some(profile)),
            Err(error) if error.code == CoreErrorCode::NotFound => Ok(None),
            Err(error) => Err(error),
        }
    }

    fn is_current_migrated_legacy_route(&self, route: &ModelRoute) -> CoreResult<bool> {
        Ok(self
            .retained_legacy_profile_for_connection(&route.connection_id)?
            .is_some_and(|profile| {
                route.api_family == ApiFamily::OpenAiChatCompletions
                    && route.model_id == profile.model
                    && route.route_config == ModelRouteConfig::default()
                    && route.metadata_source == ModelMetadataSource::Legacy
            }))
    }

    fn classify_migrated_legacy_target(
        &self,
        target: &GenerationTarget,
    ) -> CoreResult<MigratedLegacyTargetClassification> {
        let route = self.inner.storage.get_model_route(&target.model_route_id)?;
        let Some(profile) = self.retained_legacy_profile_for_connection(&route.connection_id)?
        else {
            return Ok(MigratedLegacyTargetClassification::Ordinary);
        };
        if route.api_family == ApiFamily::OpenAiChatCompletions
            && route.model_id == profile.model
            && route.route_config == ModelRouteConfig::default()
            && route.metadata_source == ModelMetadataSource::Legacy
            && target.generation_preset_id.as_str() == route.id.as_str()
        {
            return Ok(MigratedLegacyTargetClassification::Current {
                profile_id: profile.id,
            });
        }
        Ok(MigratedLegacyTargetClassification::Alias)
    }
}

pub(in crate::app) fn validate_settings_generation_target(
    core: &Core,
    settings: &AppSettings,
) -> CoreResult<()> {
    match (
        settings.selected_model_route_id.as_ref(),
        settings.selected_generation_preset_id.as_ref(),
    ) {
        (None, None) => Ok(()),
        (Some(model_route_id), Some(generation_preset_id)) => {
            validate_generation_target_plan(
                core,
                &GenerationTarget {
                    model_route_id: model_route_id.clone(),
                    generation_preset_id: generation_preset_id.clone(),
                },
            )?;
            Ok(())
        }
        _ => Err(CoreError::invalid(
            "model route and generation preset must be selected together",
        )),
    }
}
