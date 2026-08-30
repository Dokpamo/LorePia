use super::{
    AssistantEngineSnapshot, BTreeMap, CanonicalOrigin, CapabilityObservation, CredentialRef,
    CurlAuthHint, Deserialize, DeterministicDiscoveryOutput, DeterministicDiscoverySource,
    DiscoveryApprovalBinding, DiscoveryApprovalId, DiscoveryCandidateId,
    DiscoveryCatalogAuthorityBinding, DiscoveryRecoveryOwner, EvidenceClaim, EvidenceId,
    GenerationPreset, Handle, HttpUrl, ModelRoute, ModelRouteId, ParsedCurlEvidence,
    ProviderConnection, ProviderConnectionId, ProviderDiscoveryConnectionOptions, ProviderTemplate,
    SecretBytes, SecretCurlInput, Serialize, Storage, UnresolvedQuestion,
    WORKING_DRAFT_SCHEMA_VERSION,
};

/// Secret-free options for a cURL-only discovery start.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderDiscoveryCurlInput {
    pub connection_id: ProviderConnectionId,
    pub display_name: String,
    pub docs_url: Option<HttpUrl>,
    pub credential_ref: Option<CredentialRef>,
    pub preferred_assistant: Option<ModelRouteId>,
    pub connection_options: ProviderDiscoveryConnectionOptions,
    pub supplied_evidence_ids: Vec<EvidenceId>,
}

/// One-shot cURL inspection result.
///
/// This type is intentionally not serializable. Its manual `Debug` never
/// exposes the extracted credential. Callers should immediately move that
/// credential to the native vault, retain only the opaque credential
/// reference, and pass `redacted_curl()` to discovery.
pub struct ProviderCurlInspection {
    pub(super) site_url: HttpUrl,
    pub(super) origin: CanonicalOrigin,
    pub(super) redacted_curl: String,
    pub(super) auth_hints: Vec<CurlAuthHint>,
    pub(super) evidence: ParsedCurlEvidence,
    pub(super) extracted_credential: Option<SecretBytes>,
}

impl std::fmt::Debug for ProviderCurlInspection {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ProviderCurlInspection")
            .field("site_url", &self.site_url)
            .field("origin", &self.origin)
            .field("redacted_curl", &self.redacted_curl)
            .field("auth_hints", &self.auth_hints)
            .field(
                "extracted_credential_present",
                &self.extracted_credential.is_some(),
            )
            .finish_non_exhaustive()
    }
}

impl ProviderCurlInspection {
    pub fn site_url(&self) -> &HttpUrl {
        &self.site_url
    }

    pub fn origin(&self) -> &CanonicalOrigin {
        &self.origin
    }

    pub fn redacted_curl(&self) -> &str {
        &self.redacted_curl
    }

    pub fn auth_hints(&self) -> &[CurlAuthHint] {
        &self.auth_hints
    }

    pub fn evidence(&self) -> &ParsedCurlEvidence {
        &self.evidence
    }

    pub fn extracted_credential(&self) -> Option<&[u8]> {
        self.extracted_credential
            .as_ref()
            .map(SecretBytes::expose_to_vault)
    }

    pub fn into_parts(self) -> (ParsedCurlEvidence, Option<SecretBytes>) {
        (self.evidence, self.extracted_credential)
    }
}

/// A source selector with no serializable raw cURL representation.
///
/// Site and known-provider sources are reconstructed from the sanitized input.
/// A cURL source is one-shot: if the process stops before it is reduced to a
/// safe deterministic result, the user must explicitly restart with a newly
/// supplied source.
pub struct ProviderDiscoverySource {
    pub(super) intent: DiscoverySourceIntent,
    pub(super) transient: Option<DeterministicDiscoverySource>,
    pub(super) declared_connection_options: Option<ProviderDiscoveryConnectionOptions>,
    pub(super) derived_site_url: Option<HttpUrl>,
}

/// One fresh evidence source accepted only while discovery is waiting for more
/// evidence.
///
/// The document variant is already secret-free. The cURL variant owns a
/// one-shot, zeroizing input and therefore implements neither serialization,
/// cloning, nor debug formatting.
pub enum ProviderDiscoveryAdditionalEvidence {
    DocumentUrl(HttpUrl),
    Curl(SecretCurlInput),
}

impl ProviderDiscoveryAdditionalEvidence {
    pub const fn document_url(url: HttpUrl) -> Self {
        Self::DocumentUrl(url)
    }

    pub const fn curl(input: SecretCurlInput) -> Self {
        Self::Curl(input)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub(super) enum DiscoverySourceIntent {
    KnownProvider {
        template_id: lorepia_domain::ProviderTemplateId,
    },
    Site,
    Curl,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct DiscoveryWorkingDraft {
    pub(super) schema_version: u32,
    pub(super) source: DiscoverySourceIntent,
    pub(super) deterministic: Option<DeterministicDiscoveryOutput>,
    pub(super) evidence_ids: Vec<EvidenceId>,
    pub(super) extra_evidence_ids: Vec<EvidenceId>,
    pub(super) selected_candidate_id: Option<DiscoveryCandidateId>,
    pub(super) template: Option<ProviderTemplate>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) catalog_authority: Option<DiscoveryCatalogAuthorityBinding>,
    pub(super) connection: Option<ProviderConnection>,
    pub(super) routes: Vec<ModelRoute>,
    pub(super) observations: Vec<CapabilityObservation>,
    pub(super) presets: Vec<GenerationPreset>,
    pub(super) credential_approval_id: Option<DiscoveryApprovalId>,
    pub(super) probe_route_ids: Vec<ModelRouteId>,
    pub(super) probe_failure_count: u32,
    pub(super) assistant: Option<AssistantEngineSnapshot>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub(super) assistant_evidence_claims: BTreeMap<EvidenceId, Vec<EvidenceClaim>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) assistant_approval_binding: Option<DiscoveryApprovalBinding>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(super) assistant_more_evidence_questions: Vec<UnresolvedQuestion>,
}

impl DiscoveryWorkingDraft {
    pub(super) fn new(source: DiscoverySourceIntent) -> Self {
        Self {
            schema_version: WORKING_DRAFT_SCHEMA_VERSION,
            source,
            deterministic: None,
            evidence_ids: Vec::new(),
            extra_evidence_ids: Vec::new(),
            selected_candidate_id: None,
            template: None,
            catalog_authority: None,
            connection: None,
            routes: Vec::new(),
            observations: Vec::new(),
            presets: Vec::new(),
            credential_approval_id: None,
            probe_route_ids: Vec::new(),
            probe_failure_count: 0,
            assistant: None,
            assistant_evidence_claims: BTreeMap::new(),
            assistant_approval_binding: None,
            assistant_more_evidence_questions: Vec::new(),
        }
    }
}

/// Coordinates one discovery graph against a Storage and Core runtime.
pub(crate) struct ProviderDiscoveryOrchestrator<'a> {
    pub(super) storage: &'a Storage,
    pub(super) runtime: &'a Handle,
    pub(super) recovery_owner: DiscoveryRecoveryOwner,
}

impl<'a> ProviderDiscoveryOrchestrator<'a> {
    pub const fn new(
        storage: &'a Storage,
        runtime: &'a Handle,
        recovery_owner: DiscoveryRecoveryOwner,
    ) -> Self {
        Self {
            storage,
            runtime,
            recovery_owner,
        }
    }
}
