use std::time::Duration;

use chrono::Utc;
use lorepia_domain::{
    AuthBinding, ConnectionConfig, ConnectionStatus, CoreError, CoreErrorCode, CoreResult,
    CredentialRedirectPolicy, CredentialRef, CredentialScope, ProviderConnection,
    ProviderConnectionDraft, ProviderConnectionId, ProviderLocalNetworkApproval,
    ProviderNetworkMode, ProviderProfile, TemplateSource,
};
use lorepia_providers::OpenAiCompatibleProvider;
use lorepia_providers::url_policy::{ApprovedLocalNetworkOrigin, UrlPolicy};

use super::templates::{compiled_built_in_default_api_base_path, validate_provider_template};
use crate::app::{Core, normalize_bounded_text};

pub(in crate::app) const MAX_PROVIDER_ID_BYTES: usize = 256;
pub(in crate::app) const MAX_PROVIDER_ID_CHARS: usize = 64;
pub(in crate::app) const MAX_PROVIDER_DISPLAY_NAME_BYTES: usize = 512;
pub(in crate::app) const MAX_PROVIDER_DISPLAY_NAME_CHARS: usize = 128;
pub(in crate::app) const MAX_PROVIDER_BASE_URL_BYTES: usize = 4 * 1024;
pub(in crate::app) const MAX_PROVIDER_BASE_URL_CHARS: usize = 1_024;
pub(in crate::app) const MAX_PROVIDER_MODEL_BYTES: usize = 1_024;
pub(in crate::app) const MAX_PROVIDER_MODEL_CHARS: usize = 256;

impl Core {
    pub fn list_provider_connections(&self) -> CoreResult<Vec<ProviderConnection>> {
        self.inner.storage.list_provider_connections()
    }

    #[allow(
        clippy::too_many_lines,
        reason = "connection creation is one fail-closed validation and persistence boundary"
    )]
    pub fn create_provider_connection(
        &self,
        mut draft: ProviderConnectionDraft,
    ) -> CoreResult<ProviderConnection> {
        match self.inner.storage.get_provider_connection(&draft.id) {
            Ok(_) => {
                return Err(CoreError::invalid(
                    "provider connection identifier already exists; create a new connection identifier",
                ));
            }
            Err(error) if error.code == CoreErrorCode::NotFound => {}
            Err(error) => return Err(error),
        }
        let (network_policy, local_network_approval) = match (
            draft.network_mode,
            draft.local_network_approval.as_ref(),
        ) {
            (ProviderNetworkMode::Public, None) => (UrlPolicy::public(), None),
            (ProviderNetworkMode::LocalLoopback, None) => (UrlPolicy::local_loopback(), None),
            (ProviderNetworkMode::ApprovedLocalNetwork, Some(approval)) => {
                if approval.origin != draft.api_origin {
                    return Err(CoreError::invalid(
                        "local-network approval origin must exactly match the provider API origin",
                    ));
                }
                let approval =
                    ApprovedLocalNetworkOrigin::new(approval.origin.as_str(), &approval.addresses)
                        .map_err(|error| {
                            CoreError::invalid(format!(
                                "provider local-network approval is invalid: {error}"
                            ))
                        })?;
                let normalized = ProviderLocalNetworkApproval {
                    origin: draft.api_origin.clone(),
                    addresses: approval.addresses().to_vec(),
                };
                (
                    UrlPolicy::approved_local_network(approval),
                    Some(normalized),
                )
            }
            (ProviderNetworkMode::ApprovedLocalNetwork, None) => {
                return Err(CoreError::invalid(
                    "approved local-network mode requires an exact origin and address approval",
                ));
            }
            (ProviderNetworkMode::Public | ProviderNetworkMode::LocalLoopback, Some(_)) => {
                return Err(CoreError::invalid(
                    "local-network approval is only valid in approved local-network mode",
                ));
            }
        };
        let policy_url = network_policy
            .canonicalize(&format!(
                "{}/",
                draft.api_origin.as_str().trim_end_matches('/')
            ))
            .map_err(|error| {
                CoreError::invalid(format!("provider API origin is not allowed: {error}"))
            })?;
        if policy_url.origin().as_string() != draft.api_origin.as_str() {
            return Err(CoreError::invalid(
                "provider API origin is not in canonical form",
            ));
        }
        draft.local_network_approval = local_network_approval;
        let active_catalog = self.operational_provider_catalog_projection_at(Utc::now())?;
        let expected_catalog_state_version = active_catalog.state_version;
        let template = if let Some(template) =
            active_catalog.provider_template(&draft.template_id, draft.template_version)
        {
            template
        } else {
            let template = self
                .inner
                .storage
                .get_provider_template(&draft.template_id, draft.template_version)?;
            if template.source == TemplateSource::SignedCatalog {
                return Err(CoreError::new(
                    CoreErrorCode::NotFound,
                    "provider template is not active in the signed catalog",
                    false,
                ));
            }
            template
        };
        validate_provider_template(&template)?;
        if draft.api_base_path.is_none() {
            draft.api_base_path = compiled_built_in_default_api_base_path(&template)?;
        }
        let credential_scope = match &template.default_manifest.auth {
            AuthBinding::None => {
                if draft.approved_credential_origin.is_some() {
                    return Err(CoreError::invalid(
                        "credential-free provider must not declare a credential origin",
                    ));
                }
                None
            }
            auth_binding => {
                let approved_origin =
                    draft.approved_credential_origin.as_ref().ok_or_else(|| {
                        CoreError::invalid(
                            "credential origin approval is required before saving this connection",
                        )
                    })?;
                if approved_origin != &draft.api_origin {
                    return Err(CoreError::invalid(
                        "approved credential origin must exactly match the provider API origin",
                    ));
                }
                Some(CredentialScope {
                    allowed_origins: vec![approved_origin.clone()],
                    auth_binding: auth_binding.clone(),
                    redirect_policy: CredentialRedirectPolicy::Deny,
                })
            }
        };
        let now = Utc::now();
        let connection = ProviderConnection {
            credential_ref: credential_scope
                .as_ref()
                .map(|_| CredentialRef(draft.id.as_str().to_owned())),
            credential_scope,
            id: draft.id,
            template_id: draft.template_id,
            template_version: draft.template_version,
            display_name: draft.display_name,
            api_origin: draft.api_origin,
            config: ConnectionConfig {
                api_base_path: draft.api_base_path,
                network_mode: draft.network_mode,
                local_network_approval: draft.local_network_approval,
                values: draft.values,
            },
            timeout_seconds: draft.timeout_seconds,
            status: ConnectionStatus::Untested,
            created_at: now,
            updated_at: now,
        };
        if template.source == TemplateSource::SignedCatalog {
            self.inner
                .storage
                .insert_provider_connection_for_catalog_state(
                    &connection,
                    &template,
                    expected_catalog_state_version,
                )?;
        } else {
            self.inner.storage.insert_provider_connection(&connection)?;
        }
        Ok(connection)
    }

    pub fn upsert_provider_connection(
        &self,
        connection: ProviderConnection,
    ) -> CoreResult<ProviderConnection> {
        let template = self
            .inner
            .storage
            .get_provider_template(&connection.template_id, connection.template_version)?;
        validate_provider_template(&template)?;
        let current = self.inner.storage.get_provider_connection(&connection.id)?;
        if connection.template_id != current.template_id
            || connection.template_version != current.template_version
            || connection.api_origin != current.api_origin
            || connection.config != current.config
            || connection.credential_ref != current.credential_ref
            || connection.credential_scope != current.credential_scope
        {
            return Err(CoreError::invalid(
                "provider template, endpoint configuration, network approval, and credential binding are immutable; create a newly approved connection instead",
            ));
        }
        let updated = ProviderConnection {
            display_name: connection.display_name,
            timeout_seconds: connection.timeout_seconds,
            updated_at: Utc::now(),
            ..current
        };
        self.inner.storage.save_provider_connection(&updated)?;
        Ok(updated)
    }

    pub fn delete_provider_connection(&self, id: &ProviderConnectionId) -> CoreResult<()> {
        self.inner.storage.delete_provider_connection(id)
    }

    pub fn list_provider_profiles(&self) -> CoreResult<Vec<ProviderProfile>> {
        self.inner.storage.list_provider_profiles()
    }

    pub fn upsert_provider_profile(
        &self,
        mut profile: ProviderProfile,
    ) -> CoreResult<ProviderProfile> {
        profile.id = normalize_bounded_text(
            "provider profile id",
            std::mem::take(&mut profile.id),
            MAX_PROVIDER_ID_BYTES,
            MAX_PROVIDER_ID_CHARS,
        )?;
        profile.display_name = normalize_bounded_text(
            "provider display name",
            std::mem::take(&mut profile.display_name),
            MAX_PROVIDER_DISPLAY_NAME_BYTES,
            MAX_PROVIDER_DISPLAY_NAME_CHARS,
        )?;
        profile.base_url = normalize_bounded_text(
            "provider base URL",
            std::mem::take(&mut profile.base_url),
            MAX_PROVIDER_BASE_URL_BYTES,
            MAX_PROVIDER_BASE_URL_CHARS,
        )?;
        profile.model = normalize_bounded_text(
            "provider model",
            std::mem::take(&mut profile.model),
            MAX_PROVIDER_MODEL_BYTES,
            MAX_PROVIDER_MODEL_CHARS,
        )?;
        if profile.timeout_seconds == 0 || profile.timeout_seconds > 600 {
            return Err(CoreError::invalid(
                "provider profile requires an id, display name, model, and a timeout from 1 to 600 seconds",
            ));
        }
        OpenAiCompatibleProvider::new(
            &profile.base_url,
            Duration::from_secs(u64::from(profile.timeout_seconds)),
        )?;
        match self.inner.storage.get_provider_profile(&profile.id) {
            Ok(existing) if existing.base_url != profile.base_url => {
                return Err(CoreError::invalid(
                    "provider endpoint configuration is immutable; create a new provider connection instead",
                ));
            }
            Ok(_) => {}
            Err(error) if error.code == CoreErrorCode::NotFound => {}
            Err(error) => return Err(error),
        }
        self.inner.storage.save_provider_profile(&profile)?;
        Ok(profile)
    }

    pub fn delete_provider_profile(&self, id: &str) -> CoreResult<()> {
        self.inner.storage.delete_provider_profile(id)
    }
}
