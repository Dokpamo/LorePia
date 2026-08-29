//! Core-owned coordination for ordinary native provider credentials.
//!
//! Raw credential material never enters this module. Core persists and
//! validates only exact native-vault operation plans and typed slot status
//! attestations supplied by the native host.

use chrono::{DateTime, Utc};
use lorepia_domain::{CoreResult, CredentialScope, ProviderConnectionId};
use lorepia_storage::{
    ProviderCredentialAccessAuthority, ProviderCredentialObservedStatus,
    ProviderCredentialOperationKind, ProviderCredentialOperationStatus,
    ProviderCredentialOutcomeCode, ProviderCredentialSlotGarbage,
    StoredProviderCredentialOperation,
};

use crate::Core;

/// Core-owned projection of an immutable native-vault operation plan.
///
/// Storage journal records stay behind the Core boundary; callers receive
/// only the secret-free authority and recovery fields needed to carry out the
/// already-authorized platform operation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderCredentialOperationPlanView {
    pub schema_version: u32,
    pub redaction_version: u32,
    pub operation_id: String,
    pub operation_sequence: u64,
    pub operation_kind: ProviderCredentialOperationKind,
    pub connection_id: ProviderConnectionId,
    pub credential_ref: String,
    pub connection_binding_sha256: String,
    pub credential_authority_id: Option<String>,
    pub credential_authority_binding_sha256: Option<String>,
    pub predecessor_authority_id: Option<String>,
    pub predecessor_authority_binding_sha256: Option<String>,
    pub credential_scope: CredentialScope,
}

/// Core-owned, secret-free view of a provider credential operation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderCredentialOperationView {
    pub plan: ProviderCredentialOperationPlanView,
    pub plan_sha256: String,
    pub preflight_evidence_sha256: String,
    pub preflight_attested_at: DateTime<Utc>,
    pub preflight_status: ProviderCredentialObservedStatus,
    pub status: ProviderCredentialOperationStatus,
    pub outcome_code: Option<ProviderCredentialOutcomeCode>,
    pub outcome_attestation_sequence: Option<u64>,
    pub cleanup_archives_connection: bool,
    pub operation_slot_recovery_required: bool,
    pub predecessor_slot_recovery_required: bool,
    pub created_at: DateTime<Utc>,
    pub started_at: Option<DateTime<Utc>>,
    pub finished_at: Option<DateTime<Utc>>,
    pub updated_at: DateTime<Utc>,
}

fn project_provider_credential_operation(
    value: StoredProviderCredentialOperation,
) -> ProviderCredentialOperationView {
    ProviderCredentialOperationView {
        plan: ProviderCredentialOperationPlanView {
            schema_version: value.plan.schema_version,
            redaction_version: value.plan.redaction_version,
            operation_id: value.plan.operation_id,
            operation_sequence: value.plan.operation_sequence,
            operation_kind: value.plan.operation_kind,
            connection_id: value.plan.connection_id,
            credential_ref: value.plan.credential_ref,
            connection_binding_sha256: value.plan.connection_binding_sha256,
            credential_authority_id: value.plan.credential_authority_id,
            credential_authority_binding_sha256: value.plan.credential_authority_binding_sha256,
            predecessor_authority_id: value.plan.predecessor_authority_id,
            predecessor_authority_binding_sha256: value.plan.predecessor_authority_binding_sha256,
            credential_scope: value.plan.credential_scope,
        },
        plan_sha256: value.plan_sha256,
        preflight_evidence_sha256: value.preflight_evidence_sha256,
        preflight_attested_at: value.preflight_attested_at,
        preflight_status: value.preflight_status,
        status: value.status,
        outcome_code: value.outcome_code,
        outcome_attestation_sequence: value.outcome_attestation_sequence,
        cleanup_archives_connection: value.cleanup_archives_connection,
        operation_slot_recovery_required: value.operation_slot_recovery_required,
        predecessor_slot_recovery_required: value.predecessor_slot_recovery_required,
        created_at: value.created_at,
        started_at: value.started_at,
        finished_at: value.finished_at,
        updated_at: value.updated_at,
    }
}

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
    ) -> CoreResult<ProviderCredentialOperationView> {
        if kind == ProviderCredentialOperationKind::Install {
            return Err(lorepia_domain::CoreError::invalid(
                "generic provider credential installation is disabled; propose an authority and use the dedicated install preparation path",
            ));
        }
        self.storage()
            .prepare_provider_credential_operation(connection_id, kind, preflight_status)
            .map(project_provider_credential_operation)
    }

    pub fn prepare_provider_credential_install_operation(
        &self,
        connection_id: &ProviderConnectionId,
        authority: &ProviderCredentialAccessAuthority,
        preflight_status: ProviderCredentialObservedStatus,
    ) -> CoreResult<ProviderCredentialOperationView> {
        self.storage()
            .prepare_provider_credential_operation_with_install_authority(
                connection_id,
                ProviderCredentialOperationKind::Install,
                preflight_status,
                Some(authority),
            )
            .map(project_provider_credential_operation)
    }

    pub fn start_provider_credential_operation(
        &self,
        operation_id: &str,
        plan_sha256: &str,
    ) -> CoreResult<ProviderCredentialOperationView> {
        self.storage()
            .start_provider_credential_operation(operation_id, plan_sha256)
            .map(project_provider_credential_operation)
    }

    pub fn attest_provider_credential_predecessor_delete_intent(
        &self,
        operation_id: &str,
        plan_sha256: &str,
        observed_status: ProviderCredentialObservedStatus,
    ) -> CoreResult<ProviderCredentialOperationView> {
        self.storage()
            .attest_provider_credential_predecessor_delete_intent(
                operation_id,
                plan_sha256,
                observed_status,
            )
            .map(project_provider_credential_operation)
    }

    pub fn attest_provider_credential_predecessor_missing(
        &self,
        operation_id: &str,
        plan_sha256: &str,
    ) -> CoreResult<ProviderCredentialOperationView> {
        self.storage()
            .attest_provider_credential_predecessor_missing(operation_id, plan_sha256)
            .map(project_provider_credential_operation)
    }

    pub fn finish_provider_credential_operation(
        &self,
        operation_id: &str,
        plan_sha256: &str,
        observed_status: ProviderCredentialObservedStatus,
    ) -> CoreResult<ProviderCredentialOperationView> {
        self.storage()
            .finish_provider_credential_operation(operation_id, plan_sha256, observed_status)
            .map(project_provider_credential_operation)
    }

    pub fn finish_provider_credential_archive(
        &self,
        operation_id: &str,
        plan_sha256: &str,
        observed_status: ProviderCredentialObservedStatus,
    ) -> CoreResult<ProviderCredentialOperationView> {
        self.storage()
            .finish_provider_credential_archive(operation_id, plan_sha256, observed_status)
            .map(project_provider_credential_operation)
    }

    pub fn reconcile_provider_credential_operation(
        &self,
        operation_id: &str,
        plan_sha256: &str,
        observed_status: ProviderCredentialObservedStatus,
    ) -> CoreResult<ProviderCredentialOperationView> {
        self.storage()
            .reconcile_provider_credential_operation(operation_id, plan_sha256, observed_status)
            .map(project_provider_credential_operation)
    }

    pub fn reconcile_provider_credential_archive(
        &self,
        operation_id: &str,
        plan_sha256: &str,
        observed_status: ProviderCredentialObservedStatus,
    ) -> CoreResult<ProviderCredentialOperationView> {
        self.storage()
            .reconcile_provider_credential_archive(operation_id, plan_sha256, observed_status)
            .map(project_provider_credential_operation)
    }

    pub fn mark_provider_credential_cleanup_required(
        &self,
        operation_id: &str,
        plan_sha256: &str,
        observed_status: ProviderCredentialObservedStatus,
        archive_connection: bool,
    ) -> CoreResult<ProviderCredentialOperationView> {
        self.storage()
            .mark_provider_credential_cleanup_required(
                operation_id,
                plan_sha256,
                observed_status,
                archive_connection,
            )
            .map(project_provider_credential_operation)
    }

    pub fn mark_provider_credential_durability_recovery_required(
        &self,
        operation_id: &str,
        plan_sha256: &str,
        archive_connection: bool,
    ) -> CoreResult<ProviderCredentialOperationView> {
        self.storage()
            .mark_provider_credential_durability_recovery_required(
                operation_id,
                plan_sha256,
                archive_connection,
            )
            .map(project_provider_credential_operation)
    }

    pub fn fence_started_provider_credential_operation_for_recovery(
        &self,
        operation_id: &str,
        plan_sha256: &str,
    ) -> CoreResult<ProviderCredentialOperationView> {
        self.storage()
            .fence_started_provider_credential_operation_for_recovery(operation_id, plan_sha256)
            .map(project_provider_credential_operation)
    }

    pub fn mark_provider_credential_predecessor_durability_recovery_required(
        &self,
        operation_id: &str,
        plan_sha256: &str,
        archive_connection: bool,
    ) -> CoreResult<ProviderCredentialOperationView> {
        self.storage()
            .mark_provider_credential_predecessor_durability_recovery_required(
                operation_id,
                plan_sha256,
                archive_connection,
            )
            .map(project_provider_credential_operation)
    }

    pub fn attest_provider_credential_durability_repaired(
        &self,
        operation_id: &str,
        plan_sha256: &str,
    ) -> CoreResult<ProviderCredentialOperationView> {
        self.storage()
            .attest_provider_credential_durability_repaired(operation_id, plan_sha256)
            .map(project_provider_credential_operation)
    }

    pub fn attest_provider_credential_predecessor_durability_repaired(
        &self,
        operation_id: &str,
        plan_sha256: &str,
    ) -> CoreResult<ProviderCredentialOperationView> {
        self.storage()
            .attest_provider_credential_predecessor_durability_repaired(operation_id, plan_sha256)
            .map(project_provider_credential_operation)
    }

    pub fn list_unresolved_provider_credential_operations(
        &self,
    ) -> CoreResult<Vec<ProviderCredentialOperationView>> {
        self.storage()
            .list_unresolved_provider_credential_operations()
            .map(|operations| {
                operations
                    .into_iter()
                    .map(project_provider_credential_operation)
                    .collect()
            })
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
