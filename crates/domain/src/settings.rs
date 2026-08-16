use serde::{Deserialize, Serialize};

use crate::{GenerationPresetId, LocalUserId, ModelRouteId};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AppSettings {
    /// Stable installation-local singleton identity.
    ///
    /// Storage creates and persists this value on first load and preserves it
    /// across all caller-authored settings updates.
    #[serde(default)]
    pub local_user_id: LocalUserId,
    pub preserve_partial_generations: bool,
    #[serde(default)]
    pub selected_provider_profile_id: Option<String>,
    #[serde(default)]
    pub selected_model_route_id: Option<ModelRouteId>,
    #[serde(default)]
    pub selected_generation_preset_id: Option<GenerationPresetId>,
}

impl Default for AppSettings {
    fn default() -> Self {
        Self {
            local_user_id: LocalUserId::new(),
            preserve_partial_generations: true,
            selected_provider_profile_id: None,
            selected_model_route_id: None,
            selected_generation_preset_id: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::AppSettings;

    #[test]
    fn legacy_settings_deserialize_without_new_selections() {
        let settings: AppSettings = serde_json::from_str(
            r#"{
                "preserve_partial_generations": false,
                "selected_provider_profile_id": "legacy-profile"
            }"#,
        )
        .unwrap();

        assert!(!settings.preserve_partial_generations);
        assert!(!settings.local_user_id.as_str().is_empty());
        assert_eq!(
            settings.selected_provider_profile_id.as_deref(),
            Some("legacy-profile")
        );
        assert!(settings.selected_model_route_id.is_none());
        assert!(settings.selected_generation_preset_id.is_none());
    }
}
