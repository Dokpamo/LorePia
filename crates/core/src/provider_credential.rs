//! Core-owned coordination for ordinary native provider credentials.
//!
//! Raw credential material never enters this module. Core persists and
//! validates only exact native-vault operation plans and typed slot status
//! attestations supplied by the native host.

use lorepia_domain::{CoreResult, ProviderConnectionId};
use lorepia_storage::{
    ProviderCredentialAccessAuthority, ProviderCredentialObservedStatus,
    ProviderCredentialOperationKind, ProviderCredentialSlotGarbage,
    StoredProviderCredentialOperation,
};

use crate::Core;

impl Core {
    pub fn propose_provider_credential_install_authority(
        &self,
        connection_id: &ProviderConnectionId,
    ) -> CoreResult<ProviderCredentialAccessAuthority> {
        self.storage()
            .propose_provider_credential_install_authority(connection_id)
    }

    pub fn prepare_provider_credential_operation(
        &self,
        connection_id: &ProviderConnectionId,
        kind: ProviderCredentialOperationKind,
        preflight_status: ProviderCredentialObservedStatus,
    ) -> CoreResult<StoredProviderCredentialOperation> {
        if kind == ProviderCredentialOperationKind::Install {
            return Err(lorepia_domain::CoreError::invalid(
                "generic provider credential installation is disabled; propose an authority and use the dedicated install preparation path",
            ));
        }
        self.storage()
            .prepare_provider_credential_operation(connection_id, kind, preflight_status)
    }

    pub fn prepare_provider_credential_install_operation(
        &self,
        connection_id: &ProviderConnectionId,
        authority: &ProviderCredentialAccessAuthority,
        preflight_status: ProviderCredentialObservedStatus,
    ) -> CoreResult<StoredProviderCredentialOperation> {
        self.storage()
            .prepare_provider_credential_operation_with_install_authority(
                connection_id,
                ProviderCredentialOperationKind::Install,
                preflight_status,
                Some(authority),
            )
    }

    pub fn start_provider_credential_operation(
        &self,
        operation_id: &str,
        plan_sha256: &str,
    ) -> CoreResult<StoredProviderCredentialOperation> {
        self.storage()
            .start_provider_credential_operation(operation_id, plan_sha256)
    }

    pub fn attest_provider_credential_predecessor_delete_intent(
        &self,
        operation_id: &str,
        plan_sha256: &str,
        observed_status: ProviderCredentialObservedStatus,
    ) -> CoreResult<StoredProviderCredentialOperation> {
        self.storage()
            .attest_provider_credential_predecessor_delete_intent(
                operation_id,
                plan_sha256,
                observed_status,
            )
    }

    pub fn attest_provider_credential_predecessor_missing(
        &self,
        operation_id: &str,
        plan_sha256: &str,
    ) -> CoreResult<StoredProviderCredentialOperation> {
        self.storage()
            .attest_provider_credential_predecessor_missing(operation_id, plan_sha256)
    }

    pub fn finish_provider_credential_operation(
        &self,
        operation_id: &str,
        plan_sha256: &str,
        observed_status: ProviderCredentialObservedStatus,
    ) -> CoreResult<StoredProviderCredentialOperation> {
        self.storage().finish_provider_credential_operation(
            operation_id,
            plan_sha256,
            observed_status,
        )
    }

    pub fn finish_provider_credential_archive(
        &self,
        operation_id: &str,
        plan_sha256: &str,
        observed_status: ProviderCredentialObservedStatus,
    ) -> CoreResult<StoredProviderCredentialOperation> {
        self.storage().finish_provider_credential_archive(
            operation_id,
            plan_sha256,
            observed_status,
        )
    }

    pub fn reconcile_provider_credential_operation(
        &self,
        operation_id: &str,
        plan_sha256: &str,
        observed_status: ProviderCredentialObservedStatus,
    ) -> CoreResult<StoredProviderCredentialOperation> {
        self.storage().reconcile_provider_credential_operation(
            operation_id,
            plan_sha256,
            observed_status,
        )
    }

    pub fn reconcile_provider_credential_archive(
        &self,
        operation_id: &str,
        plan_sha256: &str,
        observed_status: ProviderCredentialObservedStatus,
    ) -> CoreResult<StoredProviderCredentialOperation> {
        self.storage().reconcile_provider_credential_archive(
            operation_id,
            plan_sha256,
            observed_status,
        )
    }

    pub fn mark_provider_credential_cleanup_required(
        &self,
        operation_id: &str,
        plan_sha256: &str,
        observed_status: ProviderCredentialObservedStatus,
        archive_connection: bool,
    ) -> CoreResult<StoredProviderCredentialOperation> {
        self.storage().mark_provider_credential_cleanup_required(
            operation_id,
            plan_sha256,
            observed_status,
            archive_connection,
        )
    }

    pub fn mark_provider_credential_durability_recovery_required(
        &self,
        operation_id: &str,
        plan_sha256: &str,
        archive_connection: bool,
    ) -> CoreResult<StoredProviderCredentialOperation> {
        self.storage()
            .mark_provider_credential_durability_recovery_required(
                operation_id,
                plan_sha256,
                archive_connection,
            )
    }

    pub fn fence_started_provider_credential_operation_for_recovery(
        &self,
        operation_id: &str,
        plan_sha256: &str,
    ) -> CoreResult<StoredProviderCredentialOperation> {
        self.storage()
            .fence_started_provider_credential_operation_for_recovery(operation_id, plan_sha256)
    }

    pub fn mark_provider_credential_predecessor_durability_recovery_required(
        &self,
        operation_id: &str,
        plan_sha256: &str,
        archive_connection: bool,
    ) -> CoreResult<StoredProviderCredentialOperation> {
        self.storage()
            .mark_provider_credential_predecessor_durability_recovery_required(
                operation_id,
                plan_sha256,
                archive_connection,
            )
    }

    pub fn attest_provider_credential_durability_repaired(
        &self,
        operation_id: &str,
        plan_sha256: &str,
    ) -> CoreResult<StoredProviderCredentialOperation> {
        self.storage()
            .attest_provider_credential_durability_repaired(operation_id, plan_sha256)
    }

    pub fn attest_provider_credential_predecessor_durability_repaired(
        &self,
        operation_id: &str,
        plan_sha256: &str,
    ) -> CoreResult<StoredProviderCredentialOperation> {
        self.storage()
            .attest_provider_credential_predecessor_durability_repaired(operation_id, plan_sha256)
    }

    pub fn list_unresolved_provider_credential_operations(
        &self,
    ) -> CoreResult<Vec<StoredProviderCredentialOperation>> {
        self.storage()
            .list_unresolved_provider_credential_operations()
    }

    pub fn list_provider_credential_slot_garbage(
        &self,
    ) -> CoreResult<Vec<ProviderCredentialSlotGarbage>> {
        self.storage().list_provider_credential_slot_garbage()
    }

    pub fn observe_provider_credential_slot_garbage(
        &self,
        connection_id: &ProviderConnectionId,
        authority_sequence: u64,
        observed_status: ProviderCredentialObservedStatus,
    ) -> CoreResult<ProviderCredentialSlotGarbage> {
        self.storage().observe_provider_credential_slot_garbage(
            connection_id,
            authority_sequence,
            observed_status,
        )
    }

    pub fn ensure_provider_credential_access_settled(
        &self,
        connection_id: &ProviderConnectionId,
    ) -> CoreResult<ProviderCredentialAccessAuthority> {
        self.storage()
            .ensure_provider_credential_access_settled(connection_id)
    }

    pub fn ensure_legacy_profile_raw_credential_access(
        &self,
        provider_profile_id: &str,
    ) -> CoreResult<()> {
        self.storage()
            .ensure_legacy_profile_raw_credential_access(provider_profile_id)
    }

    pub fn ensure_legacy_profile_credential_mutation_settled(
        &self,
        provider_profile_id: &str,
    ) -> CoreResult<()> {
        self.storage()
            .ensure_legacy_profile_credential_mutation_settled(provider_profile_id)
    }

    pub fn provider_connection_uses_legacy_raw_credential(
        &self,
        connection_id: &ProviderConnectionId,
    ) -> CoreResult<bool> {
        self.storage()
            .provider_connection_uses_legacy_raw_credential(connection_id)
    }

    pub fn provider_credential_recovery_authority(
        &self,
        connection_id: &ProviderConnectionId,
    ) -> CoreResult<Option<ProviderCredentialAccessAuthority>> {
        self.storage()
            .provider_credential_recovery_authority(connection_id)
    }
}

#[cfg(test)]
mod tests {
    use chrono::Utc;
    use lorepia_domain::{
        AuthBinding, CanonicalOrigin, ConnectionConfig, ConnectionConfigEntry,
        ConnectionConfigValue, ConnectionStatus, CoreErrorCode, CredentialRedirectPolicy,
        CredentialRef, CredentialScope, EndpointPath, ProviderConnection, ProviderConnectionId,
        ProviderNetworkMode, ProviderTemplateId,
    };
    use lorepia_storage::{ProviderCredentialObservedStatus, ProviderCredentialOperationKind};
    use tempfile::tempdir;

    use crate::{Core, CoreConfig};

    #[test]
    fn generic_install_fails_closed_before_journaling() {
        let root = tempdir().expect("temporary data root");
        let core = Core::open(CoreConfig::new(root.path())).expect("open Core");
        insert_credential_connection(&core, "core-generic-install");
        let connection_id = ProviderConnectionId::from("core-generic-install");

        let error = core
            .prepare_provider_credential_operation(
                &connection_id,
                ProviderCredentialOperationKind::Install,
                ProviderCredentialObservedStatus::Missing,
            )
            .expect_err("generic Core install must require physical-slot authority");
        assert_eq!(error.code, CoreErrorCode::InvalidInput);
        assert!(
            core.list_unresolved_provider_credential_operations()
                .expect("list credential journal")
                .is_empty()
        );

        let authority = core
            .propose_provider_credential_install_authority(&connection_id)
            .expect("propose install authority");
        let prepared = core
            .prepare_provider_credential_install_operation(
                &connection_id,
                &authority,
                ProviderCredentialObservedStatus::Missing,
            )
            .expect("dedicated authority-bound install remains available");
        assert_eq!(prepared.plan.operation_id, authority.authority_id);
    }

    fn insert_credential_connection(core: &Core, id: &str) {
        let api_origin = CanonicalOrigin::parse("https://api.example.test").expect("origin");
        let now = Utc::now();
        core.storage()
            .insert_provider_connection(&ProviderConnection {
                id: ProviderConnectionId::from(id),
                template_id: ProviderTemplateId::from("custom-openai-chat-v1"),
                template_version: 1,
                display_name: "Core credential test".to_owned(),
                api_origin: api_origin.clone(),
                config: ConnectionConfig {
                    api_base_path: Some(EndpointPath::parse("/v1").expect("base path")),
                    network_mode: ProviderNetworkMode::Public,
                    local_network_approval: None,
                    values: vec![ConnectionConfigEntry {
                        key: "api_base_url".to_owned(),
                        value: ConnectionConfigValue::Text(
                            "https://api.example.test/v1".to_owned(),
                        ),
                    }],
                },
                credential_ref: Some(CredentialRef(id.to_owned())),
                credential_scope: Some(CredentialScope {
                    allowed_origins: vec![api_origin],
                    auth_binding: AuthBinding::BearerHeader,
                    redirect_policy: CredentialRedirectPolicy::Deny,
                }),
                timeout_seconds: 30,
                status: ConnectionStatus::Untested,
                created_at: now,
                updated_at: now,
            })
            .expect("insert credential-bound connection");
    }
}
