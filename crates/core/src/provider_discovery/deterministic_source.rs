use std::{error::Error, fmt};

use lorepia_domain::{
    ApiFamily, AuthBinding, CanonicalOrigin, EndpointPath, HeaderName, HttpMethod,
    ProviderNetworkMode, ProviderTemplate, ProviderTemplateId,
};
#[cfg(test)]
use lorepia_providers::{
    BuiltInTemplateId, SecretCurlInput, parse_curl, url_policy::UrlPolicyMode,
};
use lorepia_providers::{
    CurlAuthHint, JsonShape, ParsedCurlEvidence,
    discovery::{DiscoveryFetchBudget, DiscoveryFetchError, DiscoveryFetchPlan},
    url_policy::UrlPolicy,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::execution::{
    canonical_secret_free_document_url_with_policy, invalid_document_url, sha256_bytes,
    validate_curl_endpoint_policy,
};

pub(super) const REDACTION_VERSION: u32 = 1;
pub(super) const DISCOVERED_TEMPLATE_VERSION: u32 = 1;
pub const DETERMINISTIC_DISCOVERY_RESULT_VERSION: u32 = 1;

/// A stable error category which never contains source text or credential data.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DeterministicDiscoveryErrorKind {
    InvalidSource,
    InvalidDocumentUrl,
    InvalidFetchBudget,
    CurlParseRejected,
    KnownProviderNotFound,
    ProviderContractUnavailable,
    EvidenceSerializationFailed,
    UnsafeEvidence,
}

/// A secret-free discovery failure.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DeterministicDiscoveryError {
    kind: DeterministicDiscoveryErrorKind,
}

impl DeterministicDiscoveryError {
    pub(super) const fn new(kind: DeterministicDiscoveryErrorKind) -> Self {
        Self { kind }
    }

    pub const fn kind(self) -> DeterministicDiscoveryErrorKind {
        self.kind
    }
}

impl fmt::Display for DeterministicDiscoveryError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self.kind {
            DeterministicDiscoveryErrorKind::InvalidSource => {
                "deterministic discovery source is invalid"
            }
            DeterministicDiscoveryErrorKind::InvalidDocumentUrl => {
                "discovery document URL was rejected by policy"
            }
            DeterministicDiscoveryErrorKind::InvalidFetchBudget => {
                "discovery fetch budget is invalid"
            }
            DeterministicDiscoveryErrorKind::CurlParseRejected => "pasted cURL input was rejected",
            DeterministicDiscoveryErrorKind::KnownProviderNotFound => {
                "no active provider template matches the selected source"
            }
            DeterministicDiscoveryErrorKind::ProviderContractUnavailable => {
                "the inferred provider contract is not compiled into this build"
            }
            DeterministicDiscoveryErrorKind::EvidenceSerializationFailed => {
                "sanitized discovery evidence could not be encoded"
            }
            DeterministicDiscoveryErrorKind::UnsafeEvidence => {
                "discovery evidence did not satisfy the persistence safety contract"
            }
        })
    }
}

impl Error for DeterministicDiscoveryError {}

#[derive(Clone)]
#[allow(clippy::large_enum_variant)]
pub(super) enum DeterministicDiscoverySourceKind {
    KnownProvider(KnownProviderSelector),
    Site {
        plan: DiscoveryFetchPlan,
    },
    Curl {
        evidence: SanitizedCurlDiscoveryEvidence,
        policy: UrlPolicy,
    },
}

#[derive(Clone, PartialEq, Eq)]
pub(super) enum KnownProviderSelector {
    Template(ProviderTemplateId),
    SiteOrigin {
        origin: CanonicalOrigin,
        policy: UrlPolicy,
    },
}

impl fmt::Debug for KnownProviderSelector {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Template(template_id) => formatter
                .debug_tuple("Template")
                .field(template_id)
                .finish(),
            Self::SiteOrigin { origin, policy } => formatter
                .debug_struct("SiteOrigin")
                .field("origin", origin)
                .field("network_boundary", &policy.network_boundary())
                .finish(),
        }
    }
}

/// In-memory cURL structure after the one-shot secret boundary.
///
/// The providers parser deliberately exposes a model hint and a redacted
/// command for immediate UI/assistant use. Deterministic Core discovery needs
/// neither, so both are dropped before the source can be cloned or retained.
/// The endpoint path remains available only in this private, non-serializable
/// value for family/base-path inference; diagnostics and durable evidence use
/// its hash.
#[derive(Clone, PartialEq, Eq)]
pub(super) struct SanitizedCurlDiscoveryEvidence {
    pub(super) method: HttpMethod,
    pub(super) origin: CanonicalOrigin,
    pub(super) path: EndpointPath,
    pub(super) query_parameter_names: Vec<String>,
    pub(super) header_names: Vec<HeaderName>,
    pub(super) auth_hints: Vec<CurlAuthHint>,
    pub(super) body_json_shape: Option<JsonShape>,
    pub(super) stream_hint: Option<bool>,
    pub(super) api_family_candidates: Vec<ApiFamily>,
}

impl From<ParsedCurlEvidence> for SanitizedCurlDiscoveryEvidence {
    fn from(evidence: ParsedCurlEvidence) -> Self {
        let ParsedCurlEvidence {
            method,
            origin,
            path,
            query_parameter_names,
            header_names,
            auth_hints,
            body_json_shape,
            model_hint: _,
            stream_hint,
            api_family_candidates,
            redacted_curl: _,
        } = evidence;
        Self {
            method,
            origin,
            path,
            query_parameter_names,
            header_names,
            auth_hints,
            body_json_shape,
            stream_hint,
            api_family_candidates,
        }
    }
}

/// Secret-free input for one deterministic discovery execution.
///
/// The fields are private so callers cannot accidentally construct a source
/// containing a raw URL query, fragment, pasted command, or credential value.
#[derive(Clone)]
pub struct DeterministicDiscoverySource {
    pub(super) kind: DeterministicDiscoverySourceKind,
}

impl fmt::Debug for DeterministicDiscoverySource {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.kind {
            DeterministicDiscoverySourceKind::KnownProvider(selector) => formatter
                .debug_tuple("KnownProvider")
                .field(selector)
                .finish(),
            DeterministicDiscoverySourceKind::Site { plan } => {
                formatter.debug_tuple("Site").field(plan).finish()
            }
            DeterministicDiscoverySourceKind::Curl { evidence, policy } => formatter
                .debug_struct("Curl")
                .field("network_boundary", &policy.network_boundary())
                .field("method", &evidence.method)
                .field("origin", &evidence.origin)
                .field(
                    "source_path_sha256",
                    &sha256_bytes(evidence.path.as_str().as_bytes()),
                )
                .field("source_path_is_root", &(evidence.path.as_str() == "/"))
                .field("query_parameter_names", &evidence.query_parameter_names)
                .field("header_names", &evidence.header_names)
                .field("auth_hints", &evidence.auth_hints)
                .field("body_json_shape", &evidence.body_json_shape)
                .field("stream_hint", &evidence.stream_hint)
                .field("api_family_candidates", &evidence.api_family_candidates)
                .finish_non_exhaustive(),
        }
    }
}

impl DeterministicDiscoverySource {
    #[cfg(test)]
    pub fn known_provider(template: BuiltInTemplateId) -> Self {
        Self::known_provider_id(ProviderTemplateId::from(template.as_str()))
    }

    /// Select any template present in Core's state-consistent active catalog.
    pub fn known_provider_id(template_id: ProviderTemplateId) -> Self {
        Self {
            kind: DeterministicDiscoverySourceKind::KnownProvider(KnownProviderSelector::Template(
                template_id,
            )),
        }
    }

    /// Select a compiled provider by the canonical origin of its API or one of
    /// its official documentation sources.
    ///
    /// The supplied path is discarded after policy validation. Query strings
    /// and fragments are stripped; credential-bearing authority components are
    /// rejected.
    #[cfg(test)]
    pub fn known_provider_site(
        site_url: &str,
        mode: UrlPolicyMode,
    ) -> Result<Self, DeterministicDiscoveryError> {
        Self::known_provider_site_with_policy(site_url, UrlPolicy::new(mode))
    }

    /// Select a compiled provider using a complete typed URL policy.
    ///
    /// Unlike the test-only compatibility constructor, this preserves an exact
    /// approved local-network origin and address set rather than projecting it
    /// to the loopback compatibility mode.
    pub fn known_provider_site_with_policy(
        site_url: &str,
        policy: UrlPolicy,
    ) -> Result<Self, DeterministicDiscoveryError> {
        let canonical = canonical_secret_free_document_url_with_policy(site_url, &policy)?;
        let origin = CanonicalOrigin::parse(&canonical.origin().as_string())
            .map_err(|_| invalid_document_url())?;
        Ok(Self {
            kind: DeterministicDiscoverySourceKind::KnownProvider(
                KnownProviderSelector::SiteOrigin { origin, policy },
            ),
        })
    }

    /// Create a bounded site-discovery source.
    #[cfg(test)]
    pub fn site(
        start_url: &str,
        mode: UrlPolicyMode,
        budget: DiscoveryFetchBudget,
    ) -> Result<Self, DeterministicDiscoveryError> {
        Self::site_with_policy(start_url, UrlPolicy::new(mode), budget)
    }

    /// Create a bounded site-discovery source with a complete typed URL policy.
    ///
    /// The policy remains attached to the fetch plan and therefore governs
    /// every allowlisted document, redirect, DNS answer, and connected peer.
    pub fn site_with_policy(
        start_url: &str,
        policy: UrlPolicy,
        budget: DiscoveryFetchBudget,
    ) -> Result<Self, DeterministicDiscoveryError> {
        let canonical = canonical_secret_free_document_url_with_policy(start_url, &policy)?;
        let plan = DiscoveryFetchPlan::new_with_policy(canonical.as_str(), policy, budget)
            .map_err(|error| {
                if matches!(error, DiscoveryFetchError::InvalidBudget) {
                    DeterministicDiscoveryError::new(
                        DeterministicDiscoveryErrorKind::InvalidFetchBudget,
                    )
                } else {
                    invalid_document_url()
                }
            })?;
        Ok(Self {
            kind: DeterministicDiscoverySourceKind::Site { plan },
        })
    }

    /// Add an exact document origin to the site crawl allowlist.
    ///
    /// This is document-fetch authority only. It never becomes credential
    /// authority and no approval bit is present in this type or its output.
    pub fn allow_document_url(
        &mut self,
        document_url: &str,
    ) -> Result<(), DeterministicDiscoveryError> {
        let DeterministicDiscoverySourceKind::Site { plan } = &mut self.kind else {
            return Err(DeterministicDiscoveryError::new(
                DeterministicDiscoveryErrorKind::InvalidSource,
            ));
        };
        let canonical =
            canonical_secret_free_document_url_with_policy(document_url, plan.policy())?;
        plan.allow_document_url(canonical.as_str())
            .map_err(|_| invalid_document_url())
    }

    /// Construct a source from cURL evidence which has already crossed the
    /// one-shot sanitization boundary.
    ///
    /// The parser's optional model scalar and redacted command are discarded
    /// immediately. The returned source implements neither serialization nor
    /// any accessor for the private endpoint path.
    /// Construct a sanitized cURL source with a complete typed URL policy.
    ///
    /// The full policy is retained for execution so an approved local-network
    /// origin cannot be widened into generic loopback or private-network
    /// access.
    pub fn sanitized_curl_with_policy(
        evidence: ParsedCurlEvidence,
        policy: UrlPolicy,
    ) -> Result<Self, DeterministicDiscoveryError> {
        let evidence = SanitizedCurlDiscoveryEvidence::from(evidence);
        validate_curl_endpoint_policy(&evidence, &policy)?;
        Ok(Self {
            kind: DeterministicDiscoverySourceKind::Curl { evidence, policy },
        })
    }
}

/// Consume raw cURL exactly once and return only its sanitized source form.
///
/// `SecretCurlInput` implements neither `Debug`, `Clone`, nor serialization.
/// The parser error is collapsed to a stable category so input fragments can
/// never reach logs, persistence, or UI error details. `mode` is mandatory so
/// loopback cURL input cannot implicitly grant itself local-network authority.
#[cfg(test)]
pub fn sanitize_curl_source(
    input: SecretCurlInput,
    mode: UrlPolicyMode,
) -> Result<DeterministicDiscoverySource, DeterministicDiscoveryError> {
    sanitize_curl_source_with_policy(input, UrlPolicy::new(mode))
}

/// Consume raw cURL once using a complete typed URL policy.
#[cfg(test)]
pub fn sanitize_curl_source_with_policy(
    input: SecretCurlInput,
    policy: UrlPolicy,
) -> Result<DeterministicDiscoverySource, DeterministicDiscoveryError> {
    let evidence = parse_curl(input).map_err(|_| {
        DeterministicDiscoveryError::new(DeterministicDiscoveryErrorKind::CurlParseRejected)
    })?;
    DeterministicDiscoverySource::sanitized_curl_with_policy(evidence, policy)
}

/// Confidence assigned only from compiled identity or structural evidence.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DiscoveryCandidateConfidence {
    Structural,
    ExactCompiledProvider,
}

/// One adapter-family possibility and the evidence records supporting it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DiscoveryFamilyCandidate {
    pub api_family: ApiFamily,
    pub confidence: DiscoveryCandidateConfidence,
    pub evidence_indices: Vec<usize>,
}

/// Why a candidate connection origin was proposed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConnectionOriginHintSource {
    CompiledProviderDefault,
    OpenApiServer,
    SanitizedCurlRequest,
}

/// Secret-free values from which Core may later construct a connection draft.
///
/// `requires_credential_origin_approval` is informational and always true when
/// the adapter authenticates. This record never represents the approval
/// itself.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DiscoveryConnectionDraftHint {
    pub api_family: ApiFamily,
    pub api_origin: CanonicalOrigin,
    pub api_base_path: Option<EndpointPath>,
    pub network_mode: ProviderNetworkMode,
    pub auth: AuthBinding,
    pub requires_credential_origin_approval: bool,
    pub source: ConnectionOriginHintSource,
    pub evidence_indices: Vec<usize>,
}

/// A validated provider-template candidate and its evidence coverage.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DiscoveryManifestCandidate {
    pub template: ProviderTemplate,
    pub manifest_sha256: String,
    pub confidence: DiscoveryCandidateConfidence,
    pub generation_endpoint_evidenced: bool,
    pub model_endpoint_evidenced: bool,
    pub auth_evidenced: bool,
    pub evidence_indices: Vec<usize>,
}

/// Evidence layout accepted by the discovery storage repository once Core adds
/// `id`, `session_id`, and `fetched_at`.
///
/// `source_origin` is deliberately an origin rather than a resource URL, so
/// signed paths and query credentials cannot enter durable state.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RedactedDiscoveryEvidenceRecord {
    pub kind: String,
    pub source_origin: CanonicalOrigin,
    pub content_sha256: String,
    pub extracted_json: Value,
    pub redaction_version: u32,
}

/// Owned, persistence-safe form of a non-fatal document fetch issue.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DeterministicDiscoveryFetchIssue {
    pub source_origin: CanonicalOrigin,
    pub source_path_sha256: String,
    pub source_path_is_root: bool,
    pub kind: String,
    pub http_status: Option<u16>,
}

/// Complete deterministic result. Every field is safe to serialize.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DeterministicDiscoveryOutput {
    pub schema_version: u32,
    pub selected_template: Option<ProviderTemplate>,
    pub evidence: Vec<RedactedDiscoveryEvidenceRecord>,
    pub family_candidates: Vec<DiscoveryFamilyCandidate>,
    pub manifest_candidates: Vec<DiscoveryManifestCandidate>,
    pub connection_hints: Vec<DiscoveryConnectionDraftHint>,
    pub fetch_issues: Vec<DeterministicDiscoveryFetchIssue>,
    pub fetch_stopped_by_budget: bool,
}

/// Durable name used by Core when storing and hydrating `draft_json`.
#[cfg(test)]
pub type DeterministicDiscoveryResult = DeterministicDiscoveryOutput;

impl DeterministicDiscoveryOutput {
    pub(super) fn empty() -> Self {
        Self {
            schema_version: DETERMINISTIC_DISCOVERY_RESULT_VERSION,
            selected_template: None,
            evidence: Vec::new(),
            family_candidates: Vec::new(),
            manifest_candidates: Vec::new(),
            connection_hints: Vec::new(),
            fetch_issues: Vec::new(),
            fetch_stopped_by_budget: false,
        }
    }
}
