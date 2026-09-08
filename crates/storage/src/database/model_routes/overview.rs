use super::super::generation_presets::{decode_generation_preset_row, generation_preset_columns};
use super::{CoreResult, GenerationPreset, ModelRoute, Storage, storage_db_error};
use super::{decode_model_route_row, model_route_columns};

impl Storage {
    /// Reads the active providers' route/preset catalog under one connection guard.
    /// Ordering matches the existing connection -> route -> preset list traversal.
    pub fn provider_generation_catalog(
        &self,
    ) -> CoreResult<(Vec<ModelRoute>, Vec<GenerationPreset>)> {
        let connection = self.connection()?;
        let routes = connection.prepare(
            "SELECT m.id, m.connection_id, m.api_family, m.model_id, m.display_name, m.route_json,
                    m.availability, m.raw_metadata_json, m.miss_count, m.metadata_source_kind,
                    m.metadata_observed_at, m.last_reconciled_sync_job_id,
                    m.metadata_sync_job_id, m.first_seen_at, m.last_seen_at
             FROM provider_models m JOIN provider_connections c ON c.id = m.connection_id
             WHERE c.archived_at IS NULL
             ORDER BY c.display_name COLLATE NOCASE, c.id, m.model_id COLLATE NOCASE, m.id"
        ).map_err(storage_db_error)?
            .query_map([], model_route_columns).map_err(storage_db_error)?
            .collect::<Result<Vec<_>, _>>().map_err(storage_db_error)?
            .into_iter().map(decode_model_route_row).collect::<CoreResult<Vec<_>>>()?;
        let presets = connection.prepare(
            "SELECT p.id, p.model_route_id, p.display_name, p.values_json, p.created_at, p.updated_at
             FROM generation_presets p JOIN provider_models m ON m.id = p.model_route_id
             JOIN provider_connections c ON c.id = m.connection_id
             WHERE c.archived_at IS NULL
             ORDER BY c.display_name COLLATE NOCASE, c.id, m.model_id COLLATE NOCASE, m.id,
                      p.display_name COLLATE NOCASE, p.id"
        ).map_err(storage_db_error)?
            .query_map([], generation_preset_columns).map_err(storage_db_error)?
            .collect::<Result<Vec<_>, _>>().map_err(storage_db_error)?
            .into_iter().map(decode_generation_preset_row).collect::<CoreResult<Vec<_>>>()?;
        Ok((routes, presets))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use lorepia_domain::ProviderProfile;

    #[test]
    fn catalog_matches_individual_lists_with_one_connection_acquisition() {
        let root = tempfile::tempdir().expect("temporary root");
        let storage = Storage::open(root.path()).expect("storage");
        for (id, name, model) in [
            ("z", "same", "Zebra"),
            ("a", "Same", "alpha"),
            ("m", "Middle", "model"),
        ] {
            storage
                .save_provider_profile(&ProviderProfile {
                    id: id.to_owned(),
                    display_name: name.to_owned(),
                    base_url: "https://example.test/v1".to_owned(),
                    model: model.to_owned(),
                    timeout_seconds: 30,
                })
                .expect("profile");
        }
        let mut expected_routes = Vec::new();
        let mut expected_presets = Vec::new();
        for connection in storage.list_provider_connections().expect("connections") {
            let routes = storage.list_model_routes(&connection.id).expect("routes");
            for route in &routes {
                expected_presets
                    .extend(storage.list_generation_presets(&route.id).expect("presets"));
            }
            expected_routes.extend(routes);
        }
        let before = storage.database_connection_metrics().acquisitions;
        let (routes, presets) = storage.provider_generation_catalog().expect("catalog");
        assert_eq!(
            storage.database_connection_metrics().acquisitions - before,
            1
        );
        assert_eq!(routes, expected_routes);
        assert_eq!(presets, expected_presets);
        storage.connection().expect("connection").execute("UPDATE provider_connections SET archived_at = '2026-09-08T00:00:00Z' WHERE id = 'm'", []).expect("archive fixture");
        let (routes, presets) = storage
            .provider_generation_catalog()
            .expect("active catalog");
        assert!(
            !routes
                .iter()
                .any(|route| route.connection_id.as_str() == "m")
        );
        assert!(
            !presets
                .iter()
                .any(|preset| preset.model_route_id.as_str() == "m")
        );
    }
}
