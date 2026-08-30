use super::{
    ApprovedLocalNetworkOrigin, CanonicalOrigin, CoreError, CoreErrorCode, CoreResult,
    DeterministicDiscoveryExecutor, DeterministicDiscoverySource, DiscoveryActionId,
    DiscoveryJsonUpdate, DiscoveryOperationId, DiscoverySessionId, DiscoverySessionSnapshot,
    DiscoverySourceIntent, DiscoveryTransitionWrite, DiscoveryWorkingDraft, HttpUrl,
    ParsedCurlEvidence, ProviderCredentialAccessAuthority, ProviderCurlInspection,
    ProviderDiscoveryAction, ProviderDiscoveryConnectionOptions, ProviderDiscoveryCurlInput,
    ProviderDiscoveryOrchestrator, ProviderDiscoverySession, ProviderDiscoverySource,
    ProviderNetworkMode, SanitizedDiscoveryInput, SecretCurlInput, UrlPolicy, Utc, Uuid,
    deterministic_error, inspect_curl, origin_from_http_url, provider_discovery_action_envelope,
    require_active_discovery_network_authority, transition_error, watch, working_draft_value,
};

impl ProviderDiscoverySource {
    pub fn site() -> Self {
        Self {
            intent: DiscoverySourceIntent::Site,
            transient: None,
            declared_connection_options: None,
            derived_site_url: None,
        }
    }

    pub fn curl(
        input: SecretCurlInput,
        connection_options: ProviderDiscoveryConnectionOptions,
    ) -> CoreResult<Self> {
        let policy = unissued_discovery_url_policy(&connection_options)?;
        let inspection = inspect_curl(input)
            .map_err(|_| CoreError::invalid("pasted cURL input was rejected"))?;
        let (evidence, extracted_credential) = inspection.into_parts();
        if extracted_credential.is_some() {
            drop(extracted_credential);
            return Err(credential_bearing_curl_requires_handoff());
        }
        Self::sanitized_curl(evidence, policy, connection_options)
    }

    fn sanitized_curl(
        evidence: ParsedCurlEvidence,
        policy: UrlPolicy,
        connection_options: ProviderDiscoveryConnectionOptions,
    ) -> CoreResult<Self> {
        let derived_site_url = HttpUrl::parse(evidence.origin.as_str())
            .map_err(|error| CoreError::invalid(format!("invalid cURL origin: {error}")))?;
        let transient = DeterministicDiscoverySource::sanitized_curl_with_policy(evidence, policy)
            .map_err(deterministic_error)?;
        Ok(Self {
            intent: DiscoverySourceIntent::Curl,
            transient: Some(transient),
            declared_connection_options: Some(connection_options),
            derived_site_url: Some(derived_site_url),
        })
    }
}

impl ProviderDiscoveryOrchestrator<'_> {
    #[allow(clippy::unused_self)]
    pub fn inspect_curl(
        &self,
        input: SecretCurlInput,
        connection_options: &ProviderDiscoveryConnectionOptions,
    ) -> CoreResult<ProviderCurlInspection> {
        let policy = unissued_discovery_url_policy(connection_options)?;
        let inspection = inspect_curl(input)
            .map_err(|_| CoreError::invalid("pasted cURL input was rejected"))?;
        let (evidence, extracted_credential) = inspection.into_parts();
        DeterministicDiscoverySource::sanitized_curl_with_policy(evidence.clone(), policy)
            .map_err(deterministic_error)?;
        let site_url = HttpUrl::parse(evidence.origin.as_str())
            .map_err(|error| CoreError::invalid(format!("invalid cURL origin: {error}")))?;
        Ok(ProviderCurlInspection {
            site_url,
            origin: evidence.origin.clone(),
            redacted_curl: evidence.redacted_curl.clone(),
            auth_hints: evidence.auth_hints.clone(),
            evidence,
            extracted_credential,
        })
    }

    /// Starts discovery directly from a cURL command. The cURL origin becomes
    /// the sanitized site URL, so no separate site/docs URL is required.
    ///
    /// If the command contains a credential, callers must first use
    /// [`Self::inspect_curl`], move the returned secret into the native vault,
    /// and call this method with the inspection's redacted cURL plus the opaque
    /// credential reference.
    pub fn begin_curl_with_credential_authority(
        &self,
        input: ProviderDiscoveryCurlInput,
        curl: SecretCurlInput,
        credential_authority: Option<ProviderCredentialAccessAuthority>,
    ) -> CoreResult<DiscoverySessionSnapshot> {
        let source = ProviderDiscoverySource::curl(curl, input.connection_options.clone())?;
        let site_url = source
            .derived_site_url
            .clone()
            .ok_or_else(|| CoreError::internal("sanitized cURL lost its derived origin"))?;
        self.begin_with_credential_authority(
            SanitizedDiscoveryInput {
                connection_id: input.connection_id,
                display_name: input.display_name,
                site_url,
                docs_url: input.docs_url,
                credential_ref: input.credential_ref,
                preferred_assistant: input.preferred_assistant,
                connection_options: input.connection_options,
                supplied_evidence_ids: input.supplied_evidence_ids,
            },
            source,
            credential_authority,
        )
    }

    /// Starts a durable discovery and immediately executes only its prepared
    /// non-persistent effects. A raw cURL value is consumed and reduced to a
    /// secret-free deterministic result before any draft is serialized.
    pub fn begin_with_credential_authority(
        &self,
        mut input: SanitizedDiscoveryInput,
        mut source: ProviderDiscoverySource,
        credential_authority: Option<ProviderCredentialAccessAuthority>,
    ) -> CoreResult<DiscoverySessionSnapshot> {
        let occurred_at = Utc::now();
        input
            .connection_options
            .issue_local_network_approval_at(occurred_at)
            .map_err(|error| CoreError::invalid(format!("invalid discovery input: {error}")))?;
        if let Some(declared) = source.declared_connection_options.as_mut() {
            declared
                .issue_local_network_approval_at(occurred_at)
                .map_err(|error| {
                    CoreError::invalid(format!("invalid cURL connection options: {error}"))
                })?;
        }
        input
            .validate()
            .map_err(|error| CoreError::invalid(format!("invalid discovery input: {error}")))?;
        if input.connection_options.network_mode == ProviderNetworkMode::ApprovedLocalNetwork
            && matches!(source.intent, DiscoverySourceIntent::Site)
        {
            return Err(approved_lan_web_discovery_disabled());
        }
        if input
            .credential_ref
            .as_ref()
            .is_some_and(|reference| reference.as_str() != input.connection_id.as_str())
        {
            return Err(CoreError::invalid(
                "discovery credential reference must equal the intended connection identifier",
            ));
        }
        if source
            .declared_connection_options
            .as_ref()
            .is_some_and(|declared| declared != &input.connection_options)
        {
            return Err(CoreError::invalid(
                "cURL connection options do not match the sanitized discovery input",
            ));
        }
        let mut draft = DiscoveryWorkingDraft::new(source.intent.clone());
        if let Some(transient) = source.transient.take() {
            draft.deterministic = Some(
                self.runtime
                    .block_on(DeterministicDiscoveryExecutor::new().execute(transient))
                    .map_err(deterministic_error)?,
            );
        }
        let session_id = DiscoverySessionId::from(Uuid::new_v4().to_string());
        let initial = ProviderDiscoverySession::new(session_id.clone(), input)
            .map_err(|error| CoreError::invalid(format!("invalid discovery input: {error}")))?;
        let envelope = provider_discovery_action_envelope(
            DiscoveryActionId::new(),
            0,
            ProviderDiscoveryAction::Begin,
        )?;
        let transition = initial.apply(&envelope).map_err(transition_error)?;
        let write = DiscoveryTransitionWrite {
            transition,
            draft: DiscoveryJsonUpdate::Replace(working_draft_value(&draft)?),
            review: DiscoveryJsonUpdate::Clear,
            new_evidence: Vec::new(),
            new_candidates: Vec::new(),
            approval: None,
            new_operation_id: Some(DiscoveryOperationId::new()),
            completed_operation: None,
            prepared_commit: None,
            provider_graph: None,
            occurred_at,
        };
        self.storage
            .begin_discovery_session_with_credential_authority(
                &initial,
                &write,
                credential_authority.as_ref(),
            )?;
        let (_cancel, cancelled) = watch::channel(false);
        self.drive_nonpersistent(&session_id, None, cancelled)
    }
}

impl crate::app::Core {
    pub fn inspect_provider_curl(
        &self,
        input: SecretCurlInput,
        connection_options: ProviderDiscoveryConnectionOptions,
    ) -> CoreResult<ProviderCurlInspection> {
        self.provider_discovery()
            .inspect_curl(input, &connection_options)
    }

    pub fn begin_provider_discovery(
        &self,
        input: SanitizedDiscoveryInput,
        source: ProviderDiscoverySource,
    ) -> CoreResult<DiscoverySessionSnapshot> {
        self.begin_provider_discovery_with_credential_authority(input, source, None)
    }

    pub fn begin_provider_discovery_with_credential_authority(
        &self,
        input: SanitizedDiscoveryInput,
        source: ProviderDiscoverySource,
        credential_authority: Option<ProviderCredentialAccessAuthority>,
    ) -> CoreResult<DiscoverySessionSnapshot> {
        self.provider_discovery().begin_with_credential_authority(
            input,
            source,
            credential_authority,
        )
    }

    pub fn begin_provider_discovery_site(
        &self,
        input: SanitizedDiscoveryInput,
    ) -> CoreResult<DiscoverySessionSnapshot> {
        self.begin_provider_discovery_site_with_credential_authority(input, None)
    }

    pub fn begin_provider_discovery_site_with_credential_authority(
        &self,
        input: SanitizedDiscoveryInput,
        credential_authority: Option<ProviderCredentialAccessAuthority>,
    ) -> CoreResult<DiscoverySessionSnapshot> {
        self.provider_discovery().begin_with_credential_authority(
            input,
            ProviderDiscoverySource::site(),
            credential_authority,
        )
    }

    pub fn begin_provider_discovery_curl(
        &self,
        input: ProviderDiscoveryCurlInput,
        curl: SecretCurlInput,
    ) -> CoreResult<DiscoverySessionSnapshot> {
        self.begin_provider_discovery_curl_with_credential_authority(input, curl, None)
    }

    pub fn begin_provider_discovery_curl_with_credential_authority(
        &self,
        input: ProviderDiscoveryCurlInput,
        curl: SecretCurlInput,
        credential_authority: Option<ProviderCredentialAccessAuthority>,
    ) -> CoreResult<DiscoverySessionSnapshot> {
        self.provider_discovery()
            .begin_curl_with_credential_authority(input, curl, credential_authority)
    }
}

pub(super) fn discovery_url_policy(
    options: &ProviderDiscoveryConnectionOptions,
) -> CoreResult<UrlPolicy> {
    require_active_discovery_network_authority(options, Utc::now())?;
    unissued_discovery_url_policy(options)
}

/// Builds a policy only for pre-session cURL parsing. It never authorizes a
/// network effect; durable sessions receive their server-issued timestamp at
/// `begin_with_credential_authority` before any effect is driven.
fn unissued_discovery_url_policy(
    options: &ProviderDiscoveryConnectionOptions,
) -> CoreResult<UrlPolicy> {
    options
        .validate()
        .map_err(|error| CoreError::invalid(format!("invalid connection options: {error}")))?;
    match (
        options.network_mode,
        options.local_network_approval.as_ref(),
    ) {
        (ProviderNetworkMode::Public, None) => Ok(UrlPolicy::public()),
        (ProviderNetworkMode::LocalLoopback, None) => Ok(UrlPolicy::local_loopback()),
        (ProviderNetworkMode::ApprovedLocalNetwork, Some(approval)) => {
            let approval =
                ApprovedLocalNetworkOrigin::new(approval.origin.as_str(), &approval.addresses)
                    .map_err(|_| {
                        CoreError::invalid("approved local-network policy was rejected")
                    })?;
            Ok(UrlPolicy::approved_local_network(approval))
        }
        _ => Err(CoreError::invalid(
            "connection network mode and local-network approval do not match",
        )),
    }
}

pub(super) fn additional_document_url_policy(
    input: &SanitizedDiscoveryInput,
    source_origin: &CanonicalOrigin,
) -> CoreResult<UrlPolicy> {
    match input.connection_options.network_mode {
        ProviderNetworkMode::Public => discovery_url_policy(&input.connection_options),
        ProviderNetworkMode::LocalLoopback => {
            require_discovery_site_origin(input, source_origin)?;
            discovery_url_policy(&input.connection_options)
        }
        ProviderNetworkMode::ApprovedLocalNetwork => Err(approved_lan_web_discovery_disabled()),
    }
}

pub(super) fn additional_curl_url_policy(
    input: &SanitizedDiscoveryInput,
    source_origin: &CanonicalOrigin,
) -> CoreResult<UrlPolicy> {
    match input.connection_options.network_mode {
        ProviderNetworkMode::Public => discovery_url_policy(&input.connection_options),
        ProviderNetworkMode::LocalLoopback => {
            require_discovery_site_origin(input, source_origin)?;
            discovery_url_policy(&input.connection_options)
        }
        ProviderNetworkMode::ApprovedLocalNetwork => {
            let approved_origin = input
                .connection_options
                .local_network_approval
                .as_ref()
                .map(|approval| &approval.origin)
                .ok_or_else(|| CoreError::invalid("local-network approval is missing"))?;
            if source_origin != approved_origin {
                return Err(CoreError::new(
                    CoreErrorCode::PermissionDenied,
                    "cURL origin is outside the approved local-network origin",
                    false,
                ));
            }
            discovery_url_policy(&input.connection_options)
        }
    }
}

fn require_discovery_site_origin(
    input: &SanitizedDiscoveryInput,
    source_origin: &CanonicalOrigin,
) -> CoreResult<()> {
    let site_origin = origin_from_http_url(&input.site_url)?;
    if source_origin == &site_origin {
        Ok(())
    } else {
        Err(CoreError::new(
            CoreErrorCode::PermissionDenied,
            "local discovery evidence must use the exact discovery site origin",
            false,
        ))
    }
}

pub(super) fn approved_lan_web_discovery_disabled() -> CoreError {
    CoreError::new(
        CoreErrorCode::PermissionDenied,
        "approved local-network web discovery is disabled without a separate network-read approval",
        false,
    )
}

pub(super) fn credential_bearing_curl_requires_handoff() -> CoreError {
    CoreError::invalid(
        "credential-bearing cURL must be inspected first and only its redacted cURL submitted after native-vault handoff",
    )
}
