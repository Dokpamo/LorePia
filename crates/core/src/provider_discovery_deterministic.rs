//! Credential-free, deterministic provider discovery.
//!
//! This module is intentionally independent from the durable discovery
//! reducer. It accepts only compiled provider identities, policy-validated
//! document fetch plans, or the sanitized output of the bounded cURL parser.
//! Raw cURL, credentials, response bodies, and credential-origin approvals are
//! neither retained nor returned.

use std::{collections::BTreeMap, error::Error, fmt};

use lorepia_domain::{
    ApiFamily, AuthBinding, CanonicalOrigin, ConnectionFieldSpec, EndpointPath, HeaderName,
    HttpMethod, HttpUrl, ManifestSource, ManifestSourceKind, ProviderManifest, ProviderNetworkMode,
    ProviderTemplate, ProviderTemplateId, TemplateSource,
};
use lorepia_providers::{
    AdapterRegistry, BuiltInTemplateId, CurlAuthHint, JsonShape, ParsedCurlEvidence,
    discovery::{
        BoundedDocumentFetcher, DiscoveryDocumentEvidence, DiscoveryEvidenceKind,
        DiscoveryFetchBudget, DiscoveryFetchError, DiscoveryFetchIssue, DiscoveryFetchIssueKind,
        DiscoveryFetchPlan,
    },
    url_policy::{UrlNetworkBoundary, UrlPolicy},
};
#[cfg(test)]
use lorepia_providers::{SecretCurlInput, parse_curl, url_policy::UrlPolicyMode};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

const REDACTION_VERSION: u32 = 1;
const DISCOVERED_TEMPLATE_VERSION: u32 = 1;
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
    const fn new(kind: DeterministicDiscoveryErrorKind) -> Self {
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
enum DeterministicDiscoverySourceKind {
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
enum KnownProviderSelector {
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
struct SanitizedCurlDiscoveryEvidence {
    method: HttpMethod,
    origin: CanonicalOrigin,
    path: EndpointPath,
    query_parameter_names: Vec<String>,
    header_names: Vec<HeaderName>,
    auth_hints: Vec<CurlAuthHint>,
    body_json_shape: Option<JsonShape>,
    stream_hint: Option<bool>,
    api_family_candidates: Vec<ApiFamily>,
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
    kind: DeterministicDiscoverySourceKind,
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
    fn empty() -> Self {
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

/// Executes only credential-free deterministic discovery.
#[derive(Debug, Default, Clone, Copy)]
pub struct DeterministicDiscoveryExecutor {
    fetcher: BoundedDocumentFetcher,
}

impl DeterministicDiscoveryExecutor {
    pub const fn new() -> Self {
        Self {
            fetcher: BoundedDocumentFetcher::new(),
        }
    }

    pub async fn execute(
        &self,
        source: DeterministicDiscoverySource,
    ) -> Result<DeterministicDiscoveryOutput, DeterministicDiscoveryError> {
        let templates = AdapterRegistry::built_in_templates().map_err(|_| contract_error())?;
        self.execute_with_templates(source, &templates).await
    }

    /// Execute known-provider matching against one already-verified active
    /// template snapshot. Site and cURL discovery remain independent of it.
    pub async fn execute_with_templates(
        &self,
        source: DeterministicDiscoverySource,
        templates: &[ProviderTemplate],
    ) -> Result<DeterministicDiscoveryOutput, DeterministicDiscoveryError> {
        match source.kind {
            DeterministicDiscoverySourceKind::KnownProvider(selector) => {
                Self::execute_known(selector, templates)
            }
            DeterministicDiscoverySourceKind::Site { plan } => self.execute_site(&plan).await,
            DeterministicDiscoverySourceKind::Curl { evidence, policy } => {
                Self::execute_curl(evidence, &policy)
            }
        }
    }

    fn execute_known(
        selector: KnownProviderSelector,
        active_templates: &[ProviderTemplate],
    ) -> Result<DeterministicDiscoveryOutput, DeterministicDiscoveryError> {
        let (templates, selected_site_policy): (Vec<ProviderTemplate>, Option<UrlPolicy>) =
            match selector {
                KnownProviderSelector::Template(id) => (
                    active_templates
                        .iter()
                        .find(|template| template.id == id)
                        .cloned()
                        .into_iter()
                        .collect(),
                    None,
                ),
                KnownProviderSelector::SiteOrigin { origin, policy } => (
                    active_templates
                        .iter()
                        .filter(|template| template_matches_origin(template, &origin, &policy))
                        .cloned()
                        .collect(),
                    Some(policy),
                ),
            };
        if templates.is_empty() {
            return Err(DeterministicDiscoveryError::new(
                DeterministicDiscoveryErrorKind::KnownProviderNotFound,
            ));
        }

        let mut output = DeterministicDiscoveryOutput::empty();
        for template in templates {
            let validated = lorepia_providers::validate_manifest(&template.default_manifest)
                .map_err(|_| contract_error())?;
            let evidence_index =
                if let Some(evidence) = built_in_evidence(&template, validated.sha256()) {
                    let index = output.evidence.len();
                    output.evidence.push(evidence);
                    push_family_candidate(
                        &mut output.family_candidates,
                        template.api_family,
                        DiscoveryCandidateConfidence::ExactCompiledProvider,
                        index,
                    );
                    Some(index)
                } else {
                    push_family_candidate_without_evidence(
                        &mut output.family_candidates,
                        template.api_family,
                        DiscoveryCandidateConfidence::ExactCompiledProvider,
                    );
                    None
                };
            output.manifest_candidates.push(DiscoveryManifestCandidate {
                template: template.clone(),
                manifest_sha256: validated.sha256().to_owned(),
                confidence: DiscoveryCandidateConfidence::ExactCompiledProvider,
                generation_endpoint_evidenced: true,
                model_endpoint_evidenced: template.default_manifest.endpoints.models.is_some(),
                auth_evidenced: true,
                evidence_indices: evidence_index.into_iter().collect(),
            });
            if let Some(origin) = &template.default_manifest.default_api_origin {
                let api_base_path = built_in_id_for_template(&template)
                    .map(|built_in_id| parse_endpoint_path(built_in_id.default_api_base_path()))
                    .transpose()?;
                output.connection_hints.push(connection_hint(
                    template.api_family,
                    origin.clone(),
                    api_base_path,
                    network_mode_for_origin(origin, selected_site_policy.as_ref())?,
                    template.default_manifest.auth.clone(),
                    ConnectionOriginHintSource::CompiledProviderDefault,
                    evidence_index.ok_or_else(contract_error)?,
                ));
            }
        }
        if output.manifest_candidates.len() == 1 {
            output.selected_template = output
                .manifest_candidates
                .first()
                .map(|candidate| candidate.template.clone());
        }
        Ok(output)
    }

    async fn execute_site(
        &self,
        plan: &DiscoveryFetchPlan,
    ) -> Result<DeterministicDiscoveryOutput, DeterministicDiscoveryError> {
        let report = self.fetcher.fetch(plan).await;
        let mut output = DeterministicDiscoveryOutput::empty();
        output.fetch_issues = report
            .issues()
            .iter()
            .map(fetch_issue_record)
            .collect::<Result<Vec<_>, _>>()?;
        output.fetch_stopped_by_budget = report.stopped_by_budget();

        for document in report.evidence() {
            let evidence_index = output.evidence.len();
            output.evidence.push(document_evidence_record(document)?);
            if document.kind() == DiscoveryEvidenceKind::OpenApi {
                Self::add_openapi_candidates(&mut output, document, evidence_index, plan.policy())?;
            }
        }
        select_unambiguous_template(&mut output);
        Ok(output)
    }

    fn execute_curl(
        evidence: SanitizedCurlDiscoveryEvidence,
        policy: &UrlPolicy,
    ) -> Result<DeterministicDiscoveryOutput, DeterministicDiscoveryError> {
        validate_curl_endpoint_policy(&evidence, policy)?;
        let network_mode = provider_network_mode(policy);
        let sanitized = sanitized_curl_json(&evidence)?;
        let content_sha256 = sha256_json(&sanitized)?;
        let mut output = DeterministicDiscoveryOutput::empty();
        output.evidence.push(RedactedDiscoveryEvidenceRecord {
            kind: "sanitized_curl_request".to_owned(),
            source_origin: evidence.origin.clone(),
            content_sha256,
            extracted_json: sanitized,
            redaction_version: REDACTION_VERSION,
        });

        let evidence_index = 0;
        for family in evidence.api_family_candidates.iter().copied() {
            let seed = seed_template(family)?;
            let generation_evidenced = generation_path_matches(family, evidence.path.as_str());
            if !generation_evidenced || evidence.method != HttpMethod::Post {
                continue;
            }
            let base_path = infer_base_path(family, evidence.path.as_str())
                .or_else(|| parse_endpoint_path(seed_default_base_path(family)).ok());
            let (template, manifest_hash) = discovered_template(
                &seed,
                evidence.origin.clone(),
                base_path.as_ref(),
                None,
                output.evidence[evidence_index].content_sha256.clone(),
                policy,
            )?;
            let auth_evidenced =
                curl_auth_matches(&evidence.auth_hints, &template.default_manifest.auth);
            push_family_candidate(
                &mut output.family_candidates,
                family,
                DiscoveryCandidateConfidence::Structural,
                evidence_index,
            );
            output.manifest_candidates.push(DiscoveryManifestCandidate {
                template: template.clone(),
                manifest_sha256: manifest_hash,
                confidence: DiscoveryCandidateConfidence::Structural,
                generation_endpoint_evidenced: true,
                model_endpoint_evidenced: false,
                auth_evidenced,
                evidence_indices: vec![evidence_index],
            });
            output.connection_hints.push(connection_hint(
                family,
                evidence.origin.clone(),
                None,
                network_mode,
                template.default_manifest.auth.clone(),
                ConnectionOriginHintSource::SanitizedCurlRequest,
                evidence_index,
            ));
        }
        deduplicate_candidates_and_hints(&mut output);
        select_unambiguous_template(&mut output);
        Ok(output)
    }

    fn add_openapi_candidates(
        output: &mut DeterministicDiscoveryOutput,
        document: &DiscoveryDocumentEvidence,
        evidence_index: usize,
        policy: &UrlPolicy,
    ) -> Result<(), DeterministicDiscoveryError> {
        let extracted = document.extracted();
        let families = openapi_family_candidates(extracted);
        let servers = openapi_server_candidates(extracted, policy)?;
        let generation_paths = string_array(extracted, "generation_paths");
        let model_paths = string_array(extracted, "model_list_paths");

        for family in families {
            let Some(generation_path) = generation_paths
                .iter()
                .find(|path| generation_path_matches(family, path))
            else {
                continue;
            };
            let seed = seed_template(family)?;
            let descriptor = AdapterRegistry::descriptor(family).map_err(|_| contract_error())?;
            let generation_evidenced = generation_path_matches(family, generation_path);
            let model_evidenced = model_paths
                .iter()
                .any(|path| path.ends_with(descriptor.models_endpoint.as_str()));
            for (origin, base_path) in &servers {
                let (template, manifest_hash) = discovered_template(
                    &seed,
                    origin.clone(),
                    Some(base_path),
                    Some(document.source().origin()),
                    document.content_sha256().to_owned(),
                    policy,
                )?;
                push_family_candidate(
                    &mut output.family_candidates,
                    family,
                    DiscoveryCandidateConfidence::Structural,
                    evidence_index,
                );
                output.manifest_candidates.push(DiscoveryManifestCandidate {
                    template: template.clone(),
                    manifest_sha256: manifest_hash,
                    confidence: DiscoveryCandidateConfidence::Structural,
                    generation_endpoint_evidenced: generation_evidenced,
                    model_endpoint_evidenced: model_evidenced,
                    auth_evidenced: openapi_auth_matches(
                        extracted,
                        &template.default_manifest.auth,
                    ),
                    evidence_indices: vec![evidence_index],
                });
                output.connection_hints.push(connection_hint(
                    family,
                    origin.clone(),
                    None,
                    provider_network_mode(policy),
                    template.default_manifest.auth.clone(),
                    ConnectionOriginHintSource::OpenApiServer,
                    evidence_index,
                ));
            }
        }
        deduplicate_candidates_and_hints(output);
        Ok(())
    }
}

fn canonical_secret_free_document_url_with_policy(
    value: &str,
    policy: &UrlPolicy,
) -> Result<lorepia_providers::url_policy::CanonicalUrl, DeterministicDiscoveryError> {
    let canonical = policy
        .canonicalize(value)
        .map_err(|_| invalid_document_url())?;
    let mut url = canonical.into_url();
    url.set_query(None);
    url.set_fragment(None);
    policy
        .canonicalize(url.as_str())
        .map_err(|_| invalid_document_url())
}

fn validate_curl_endpoint_policy(
    evidence: &SanitizedCurlDiscoveryEvidence,
    policy: &UrlPolicy,
) -> Result<(), DeterministicDiscoveryError> {
    let endpoint = format!("{}{}", evidence.origin.as_str(), evidence.path.as_str());
    let canonical = policy
        .canonicalize(&endpoint)
        .map_err(|_| invalid_document_url())?;
    if canonical.origin().as_string() != evidence.origin.as_str()
        || canonical.url().path() != evidence.path.as_str()
        || canonical.url().query().is_some()
        || canonical.stripped_fragment()
        || canonical.stripped_sensitive_query_parameters() != 0
    {
        return Err(invalid_document_url());
    }
    Ok(())
}

const fn invalid_document_url() -> DeterministicDiscoveryError {
    DeterministicDiscoveryError::new(DeterministicDiscoveryErrorKind::InvalidDocumentUrl)
}

const fn contract_error() -> DeterministicDiscoveryError {
    DeterministicDiscoveryError::new(DeterministicDiscoveryErrorKind::ProviderContractUnavailable)
}

fn parse_endpoint_path(value: &str) -> Result<EndpointPath, DeterministicDiscoveryError> {
    EndpointPath::parse(value).map_err(|_| contract_error())
}

fn provider_network_mode(policy: &UrlPolicy) -> ProviderNetworkMode {
    match policy.network_boundary() {
        UrlNetworkBoundary::Public => ProviderNetworkMode::Public,
        UrlNetworkBoundary::LocalLoopback => ProviderNetworkMode::LocalLoopback,
        UrlNetworkBoundary::ApprovedLocalNetwork => ProviderNetworkMode::ApprovedLocalNetwork,
    }
}

fn network_mode_for_origin(
    origin: &CanonicalOrigin,
    selected_site_policy: Option<&UrlPolicy>,
) -> Result<ProviderNetworkMode, DeterministicDiscoveryError> {
    if let Some(policy) = selected_site_policy {
        let permitted = policy
            .canonicalize(origin.as_str())
            .is_ok_and(|url| url.origin().as_string() == origin.as_str());
        if !permitted {
            return Err(invalid_document_url());
        }
        return Ok(provider_network_mode(policy));
    }
    if UrlPolicy::public().canonicalize(origin.as_str()).is_ok() {
        Ok(ProviderNetworkMode::Public)
    } else if UrlPolicy::local_loopback()
        .canonicalize(origin.as_str())
        .is_ok()
    {
        Ok(ProviderNetworkMode::LocalLoopback)
    } else {
        Err(invalid_document_url())
    }
}

fn built_in_id_for_template(template: &ProviderTemplate) -> Option<BuiltInTemplateId> {
    BuiltInTemplateId::ALL
        .into_iter()
        .find(|id| id.as_str() == template.id.as_str())
}

fn template_matches_origin(
    template: &ProviderTemplate,
    expected: &CanonicalOrigin,
    policy: &UrlPolicy,
) -> bool {
    if template
        .default_manifest
        .default_api_origin
        .as_ref()
        .is_some_and(|origin| origin == expected)
    {
        return true;
    }
    template.default_manifest.sources.iter().any(|source| {
        policy
            .canonicalize(source.url.as_str())
            .is_ok_and(|url| url.origin().as_string() == expected.as_str())
    })
}

fn built_in_evidence(
    template: &ProviderTemplate,
    manifest_sha256: &str,
) -> Option<RedactedDiscoveryEvidenceRecord> {
    let source_origin = template
        .default_manifest
        .sources
        .first()
        .and_then(|source| {
            UrlPolicy::public()
                .canonicalize(source.url.as_str())
                .ok()
                .or_else(|| {
                    UrlPolicy::local_loopback()
                        .canonicalize(source.url.as_str())
                        .ok()
                })
        })
        .and_then(|url| CanonicalOrigin::parse(&url.origin().as_string()).ok())
        .or_else(|| template.default_manifest.default_api_origin.clone());
    let source_origin = source_origin?;
    Some(RedactedDiscoveryEvidenceRecord {
        kind: match template.source {
            TemplateSource::BuiltIn => "built_in_template",
            TemplateSource::SignedCatalog => "signed_catalog_template",
            TemplateSource::UserDiscovered => "user_discovered_template",
        }
        .to_owned(),
        source_origin,
        content_sha256: manifest_sha256.to_owned(),
        extracted_json: json!({
            "template_id": template.id,
            "template_version": template.manifest_version,
            "api_family": template.api_family,
            "manifest_sha256": manifest_sha256,
            "trust": match template.source {
                TemplateSource::BuiltIn => "compiled_in",
                TemplateSource::SignedCatalog => "verified_signed_catalog",
                TemplateSource::UserDiscovered => "locally_reviewed",
            }
        }),
        redaction_version: REDACTION_VERSION,
    })
}

fn document_evidence_record(
    document: &DiscoveryDocumentEvidence,
) -> Result<RedactedDiscoveryEvidenceRecord, DeterministicDiscoveryError> {
    if !document.extracted().is_object() {
        return Err(DeterministicDiscoveryError::new(
            DeterministicDiscoveryErrorKind::UnsafeEvidence,
        ));
    }
    let source_origin = CanonicalOrigin::parse(document.source().origin()).map_err(|_| {
        DeterministicDiscoveryError::new(DeterministicDiscoveryErrorKind::UnsafeEvidence)
    })?;
    let discovered_links = document
        .discovered_links()
        .iter()
        .map(redacted_url_json)
        .collect::<Vec<_>>();
    let redirect_chain = document
        .redirect_chain()
        .iter()
        .map(redacted_url_json)
        .collect::<Vec<_>>();
    Ok(RedactedDiscoveryEvidenceRecord {
        kind: evidence_kind_name(document.kind()).to_owned(),
        source_origin,
        content_sha256: document.content_sha256().to_owned(),
        extracted_json: json!({
            "trust_boundary": "untrusted_external_document",
            "source_path_sha256": document.source().path_sha256(),
            "source_path_is_root": document.source().path_is_root(),
            "media_type": document.media_type(),
            "response_bytes": document.response_bytes(),
            "excerpt": document.excerpt().as_str(),
            "extracted": document.extracted(),
            "discovered_links": discovered_links,
            "redirect_chain": redirect_chain
        }),
        redaction_version: REDACTION_VERSION,
    })
}

fn redacted_url_json(url: &lorepia_providers::discovery::RedactedUrlEvidence) -> Value {
    json!({
        "origin": url.origin(),
        "path_sha256": url.path_sha256(),
        "path_is_root": url.path_is_root()
    })
}

fn fetch_issue_record(
    issue: &DiscoveryFetchIssue,
) -> Result<DeterministicDiscoveryFetchIssue, DeterministicDiscoveryError> {
    let source_origin = CanonicalOrigin::parse(issue.source().origin()).map_err(|_| {
        DeterministicDiscoveryError::new(DeterministicDiscoveryErrorKind::UnsafeEvidence)
    })?;
    let (kind, http_status) = match issue.kind() {
        DiscoveryFetchIssueKind::WallClockLimitReached => ("wall_clock_limit_reached", None),
        DiscoveryFetchIssueKind::PageLimitReached => ("page_limit_reached", None),
        DiscoveryFetchIssueKind::LinkLimitReached => ("link_limit_reached", None),
        DiscoveryFetchIssueKind::RedirectLimitReached => ("redirect_limit_reached", None),
        DiscoveryFetchIssueKind::RedirectLocationMissing => ("redirect_location_missing", None),
        DiscoveryFetchIssueKind::RedirectHostNotAllowed => ("redirect_host_not_allowed", None),
        DiscoveryFetchIssueKind::DnsLookupFailed => ("dns_lookup_failed", None),
        DiscoveryFetchIssueKind::DnsPolicyRejected => ("dns_policy_rejected", None),
        DiscoveryFetchIssueKind::RequestFailed => ("request_failed", None),
        DiscoveryFetchIssueKind::HttpStatus(status) => ("http_status", Some(*status)),
        DiscoveryFetchIssueKind::MediaTypeMissing => ("media_type_missing", None),
        DiscoveryFetchIssueKind::MediaTypeNotAllowed => ("media_type_not_allowed", None),
        DiscoveryFetchIssueKind::CharsetNotSupported => ("charset_not_supported", None),
        DiscoveryFetchIssueKind::ContentEncodingNotAllowed => {
            ("content_encoding_not_allowed", None)
        }
        DiscoveryFetchIssueKind::DocumentTooLarge => ("document_too_large", None),
        DiscoveryFetchIssueKind::TotalByteLimitReached => ("total_byte_limit_reached", None),
        DiscoveryFetchIssueKind::InvalidDocument => ("invalid_document", None),
    };
    Ok(DeterministicDiscoveryFetchIssue {
        source_origin,
        source_path_sha256: issue.source().path_sha256().to_owned(),
        source_path_is_root: issue.source().path_is_root(),
        kind: kind.to_owned(),
        http_status,
    })
}

const fn evidence_kind_name(kind: DiscoveryEvidenceKind) -> &'static str {
    match kind {
        DiscoveryEvidenceKind::HtmlDocument => "html_document",
        DiscoveryEvidenceKind::JsonDocument => "json_document",
        DiscoveryEvidenceKind::YamlDocument => "yaml_document",
        DiscoveryEvidenceKind::XmlDocument => "xml_document",
        DiscoveryEvidenceKind::PlainTextDocument => "plain_text_document",
        DiscoveryEvidenceKind::JsonSchema => "json_schema",
        DiscoveryEvidenceKind::OpenApi => "openapi",
    }
}

fn sanitized_curl_json(
    evidence: &SanitizedCurlDiscoveryEvidence,
) -> Result<Value, DeterministicDiscoveryError> {
    let value = json!({
        "method": evidence.method,
        "origin": evidence.origin,
        "source_path_sha256": sha256_bytes(evidence.path.as_str().as_bytes()),
        "source_path_is_root": evidence.path.as_str() == "/",
        "query_parameter_names": evidence.query_parameter_names,
        "header_names": evidence.header_names,
        "auth_hints": evidence.auth_hints,
        "body_json_shape": evidence.body_json_shape,
        "stream_hint": evidence.stream_hint,
        "api_family_candidates": evidence.api_family_candidates,
        "trust": "sanitized_curl_structure"
    });
    if value.is_object() {
        Ok(value)
    } else {
        Err(DeterministicDiscoveryError::new(
            DeterministicDiscoveryErrorKind::EvidenceSerializationFailed,
        ))
    }
}

fn sha256_json(value: &Value) -> Result<String, DeterministicDiscoveryError> {
    let encoded = serde_json::to_vec(value).map_err(|_| {
        DeterministicDiscoveryError::new(
            DeterministicDiscoveryErrorKind::EvidenceSerializationFailed,
        )
    })?;
    let digest = Sha256::digest(encoded);
    Ok(format!("{digest:x}"))
}

fn sha256_bytes(value: &[u8]) -> String {
    let digest = Sha256::digest(value);
    format!("{digest:x}")
}

fn seed_template(family: ApiFamily) -> Result<ProviderTemplate, DeterministicDiscoveryError> {
    let id = match family {
        ApiFamily::OpenAiResponses => BuiltInTemplateId::OpenAiResponses,
        ApiFamily::OpenAiChatCompletions => BuiltInTemplateId::OpenAiChatCompatible,
        ApiFamily::AnthropicMessages => BuiltInTemplateId::AnthropicMessages,
        ApiFamily::GeminiGenerateContent => BuiltInTemplateId::GeminiGenerateContent,
        ApiFamily::OllamaNative => BuiltInTemplateId::OllamaNative,
    };
    AdapterRegistry::built_in_template(id).map_err(|_| contract_error())
}

const fn seed_default_base_path(family: ApiFamily) -> &'static str {
    match family {
        ApiFamily::OpenAiResponses
        | ApiFamily::OpenAiChatCompletions
        | ApiFamily::AnthropicMessages => "/v1",
        ApiFamily::GeminiGenerateContent => "/v1beta",
        ApiFamily::OllamaNative => "/api",
    }
}

fn discovered_template(
    seed: &ProviderTemplate,
    api_origin: CanonicalOrigin,
    api_base_path: Option<&EndpointPath>,
    evidence_source_origin: Option<&str>,
    evidence_sha256: String,
    policy: &UrlPolicy,
) -> Result<(ProviderTemplate, String), DeterministicDiscoveryError> {
    let mut manifest = seed.default_manifest.clone();
    embed_discovered_api_base_path(&mut manifest, api_base_path)?;
    if policy.network_boundary() == UrlNetworkBoundary::ApprovedLocalNetwork {
        // A LAN grant is connection-specific authority. Do not promote its
        // exact origin into a reusable template default or manifest source;
        // the redacted evidence and connection hint retain that origin while
        // the eventual connection must retain the typed exact-address grant.
        manifest.default_api_origin = None;
        manifest.sources.clear();
    } else {
        manifest.default_api_origin = Some(api_origin.clone());
        let source_origin = evidence_source_origin.unwrap_or(api_origin.as_str());
        manifest.sources = vec![ManifestSource {
            kind: ManifestSourceKind::UserSupplied,
            url: HttpUrl::parse(source_origin).map_err(|_| {
                DeterministicDiscoveryError::new(DeterministicDiscoveryErrorKind::UnsafeEvidence)
            })?,
            content_sha256: Some(evidence_sha256),
        }];
    }
    let validated =
        lorepia_providers::validate_manifest(&manifest).map_err(|_| contract_error())?;
    let manifest_sha256 = validated.sha256().to_owned();
    let template = ProviderTemplate {
        id: ProviderTemplateId::from(format!("discovered-{manifest_sha256}")),
        display_name: discovered_display_name(seed.api_family).to_owned(),
        manifest_version: DISCOVERED_TEMPLATE_VERSION,
        source: TemplateSource::UserDiscovered,
        api_family: seed.api_family,
        connection_fields: discovered_connection_fields(seed),
        default_manifest: manifest,
    };
    lorepia_providers::validate_connection_fields(&template.connection_fields)
        .map_err(|_| contract_error())?;
    Ok((template, manifest_sha256))
}

/// Make a user-discovered manifest self-contained by folding its validated API
/// base path into every endpoint path.
///
/// Connection hints are operation-local and disappear after commit. Keeping
/// the prefix in the manifest therefore prevents a later known-provider setup
/// from silently addressing `/models` when discovery proved `/v1/models` (or
/// another bounded structural prefix). The already-prefixed check also keeps
/// assistant-authored absolute endpoint paths from receiving the prefix twice.
pub(crate) fn embed_discovered_api_base_path(
    manifest: &mut ProviderManifest,
    api_base_path: Option<&EndpointPath>,
) -> Result<(), DeterministicDiscoveryError> {
    let Some(api_base_path) = api_base_path else {
        return Ok(());
    };
    if !base_path_is_persistence_safe(api_base_path.as_str()) {
        return Err(DeterministicDiscoveryError::new(
            DeterministicDiscoveryErrorKind::UnsafeEvidence,
        ));
    }
    if let Some(existing_base_path) = infer_base_path(
        manifest.api_family,
        manifest.endpoints.generate.path.as_str(),
    ) && existing_base_path.as_str() != "/"
        && existing_base_path != *api_base_path
    {
        return Err(DeterministicDiscoveryError::new(
            DeterministicDiscoveryErrorKind::UnsafeEvidence,
        ));
    }
    if let Some(models) = manifest.endpoints.models.as_ref()
        && let Some(existing_base_path) =
            infer_models_base_path(manifest.api_family, models.path.as_str())
        && existing_base_path.as_str() != "/"
        && existing_base_path != *api_base_path
    {
        return Err(DeterministicDiscoveryError::new(
            DeterministicDiscoveryErrorKind::UnsafeEvidence,
        ));
    }

    // Compute every path before mutating the manifest. A malformed or
    // conflicting secondary endpoint must not leave a partially-prefixed
    // template in the caller's review draft.
    let generation_path =
        endpoint_with_embedded_base(api_base_path, &manifest.endpoints.generate.path)?;
    let models_path = manifest
        .endpoints
        .models
        .as_ref()
        .map(|models| endpoint_with_embedded_base(api_base_path, &models.path))
        .transpose()?;
    manifest.endpoints.generate.path = generation_path;
    if let (Some(models), Some(path)) = (manifest.endpoints.models.as_mut(), models_path) {
        models.path = path;
    }
    Ok(())
}

fn endpoint_with_embedded_base(
    api_base_path: &EndpointPath,
    endpoint_path: &EndpointPath,
) -> Result<EndpointPath, DeterministicDiscoveryError> {
    let base = api_base_path.as_str().trim_end_matches('/');
    if base.is_empty() {
        return Ok(endpoint_path.clone());
    }
    let endpoint = endpoint_path.as_str();
    if endpoint == base
        || endpoint
            .strip_prefix(base)
            .is_some_and(|remainder| remainder.starts_with('/'))
    {
        return Ok(endpoint_path.clone());
    }
    EndpointPath::parse(&format!("{base}/{}", endpoint.trim_start_matches('/')))
        .map_err(|_| contract_error())
}

fn discovered_connection_fields(seed: &ProviderTemplate) -> Vec<ConnectionFieldSpec> {
    let mut fields = seed.connection_fields.clone();
    if !fields.iter().any(|field| field.key == "api_base_url") {
        fields.insert(
            0,
            ConnectionFieldSpec {
                key: "api_base_url".to_owned(),
                label_key: "provider.connection.api_base_url".to_owned(),
                description_key: Some("provider.connection.api_base_url.description".to_owned()),
                value_type: lorepia_domain::ConnectionFieldType::Text,
                required: true,
            },
        );
    }
    fields
}

const fn discovered_display_name(family: ApiFamily) -> &'static str {
    match family {
        ApiFamily::OpenAiResponses => "Discovered OpenAI Responses provider",
        ApiFamily::OpenAiChatCompletions => "Discovered OpenAI Chat provider",
        ApiFamily::AnthropicMessages => "Discovered Anthropic Messages provider",
        ApiFamily::GeminiGenerateContent => "Discovered Gemini provider",
        ApiFamily::OllamaNative => "Discovered Ollama provider",
    }
}

fn connection_hint(
    api_family: ApiFamily,
    api_origin: CanonicalOrigin,
    api_base_path: Option<EndpointPath>,
    network_mode: ProviderNetworkMode,
    auth: AuthBinding,
    source: ConnectionOriginHintSource,
    evidence_index: usize,
) -> DiscoveryConnectionDraftHint {
    DiscoveryConnectionDraftHint {
        api_family,
        api_origin,
        api_base_path,
        network_mode,
        requires_credential_origin_approval: auth != AuthBinding::None,
        auth,
        source,
        evidence_indices: vec![evidence_index],
    }
}

fn push_family_candidate(
    candidates: &mut Vec<DiscoveryFamilyCandidate>,
    api_family: ApiFamily,
    confidence: DiscoveryCandidateConfidence,
    evidence_index: usize,
) {
    if let Some(existing) = candidates
        .iter_mut()
        .find(|candidate| candidate.api_family == api_family)
    {
        existing.confidence = existing.confidence.max(confidence);
        if !existing.evidence_indices.contains(&evidence_index) {
            existing.evidence_indices.push(evidence_index);
            existing.evidence_indices.sort_unstable();
        }
    } else {
        candidates.push(DiscoveryFamilyCandidate {
            api_family,
            confidence,
            evidence_indices: vec![evidence_index],
        });
        candidates.sort_by_key(|candidate| family_sort_key(candidate.api_family));
    }
}

fn push_family_candidate_without_evidence(
    candidates: &mut Vec<DiscoveryFamilyCandidate>,
    api_family: ApiFamily,
    confidence: DiscoveryCandidateConfidence,
) {
    if let Some(existing) = candidates
        .iter_mut()
        .find(|candidate| candidate.api_family == api_family)
    {
        existing.confidence = existing.confidence.max(confidence);
    } else {
        candidates.push(DiscoveryFamilyCandidate {
            api_family,
            confidence,
            evidence_indices: Vec::new(),
        });
        candidates.sort_by_key(|candidate| family_sort_key(candidate.api_family));
    }
}

const fn family_sort_key(family: ApiFamily) -> u8 {
    match family {
        ApiFamily::OpenAiResponses => 0,
        ApiFamily::OpenAiChatCompletions => 1,
        ApiFamily::AnthropicMessages => 2,
        ApiFamily::GeminiGenerateContent => 3,
        ApiFamily::OllamaNative => 4,
    }
}

fn select_unambiguous_template(output: &mut DeterministicDiscoveryOutput) {
    if output.manifest_candidates.len() == 1
        && output.manifest_candidates[0].generation_endpoint_evidenced
    {
        output.selected_template = Some(output.manifest_candidates[0].template.clone());
    }
}

fn deduplicate_candidates_and_hints(output: &mut DeterministicDiscoveryOutput) {
    let mut manifests: BTreeMap<String, DiscoveryManifestCandidate> = BTreeMap::new();
    for candidate in output.manifest_candidates.drain(..) {
        manifests
            .entry(candidate.manifest_sha256.clone())
            .and_modify(|existing| {
                merge_evidence_indices(&mut existing.evidence_indices, &candidate.evidence_indices);
            })
            .or_insert(candidate);
    }
    output.manifest_candidates = manifests.into_values().collect();

    let mut hints: Vec<DiscoveryConnectionDraftHint> = Vec::new();
    for hint in output.connection_hints.drain(..) {
        if let Some(existing) = hints.iter_mut().find(|existing| {
            existing.api_family == hint.api_family
                && existing.api_origin == hint.api_origin
                && existing.api_base_path == hint.api_base_path
        }) {
            merge_evidence_indices(&mut existing.evidence_indices, &hint.evidence_indices);
        } else {
            hints.push(hint);
        }
    }
    hints.sort_by(|left, right| {
        (
            family_sort_key(left.api_family),
            left.api_origin.as_str(),
            left.api_base_path.as_ref().map(EndpointPath::as_str),
        )
            .cmp(&(
                family_sort_key(right.api_family),
                right.api_origin.as_str(),
                right.api_base_path.as_ref().map(EndpointPath::as_str),
            ))
    });
    output.connection_hints = hints;
}

fn merge_evidence_indices(destination: &mut Vec<usize>, source: &[usize]) {
    for index in source {
        if !destination.contains(index) {
            destination.push(*index);
        }
    }
    destination.sort_unstable();
}

fn openapi_family_candidates(extracted: &Value) -> Vec<ApiFamily> {
    let Some(values) = extracted.get("api_family_hints").and_then(Value::as_array) else {
        return Vec::new();
    };
    let mut families = values
        .iter()
        .filter_map(Value::as_str)
        .filter_map(|value| match value {
            "open_ai_responses" | "openai_responses" => Some(ApiFamily::OpenAiResponses),
            "open_ai_chat_completions" | "openai_chat_completions" => {
                Some(ApiFamily::OpenAiChatCompletions)
            }
            "anthropic_messages" => Some(ApiFamily::AnthropicMessages),
            "gemini_generate_content" => Some(ApiFamily::GeminiGenerateContent),
            "ollama_native" => Some(ApiFamily::OllamaNative),
            _ => None,
        })
        .collect::<Vec<_>>();
    families.sort_by_key(|family| family_sort_key(*family));
    families.dedup();
    families
}

fn openapi_server_candidates(
    extracted: &Value,
    policy: &UrlPolicy,
) -> Result<Vec<(CanonicalOrigin, EndpointPath)>, DeterministicDiscoveryError> {
    let Some(values) = extracted.get("server_candidates").and_then(Value::as_array) else {
        return Ok(Vec::new());
    };
    let mut servers = Vec::new();
    for value in values {
        let Some(origin) = value.get("origin").and_then(Value::as_str) else {
            continue;
        };
        let Some(base_path) = value.get("base_path").and_then(Value::as_str) else {
            continue;
        };
        let canonical = policy.canonicalize(origin).map_err(|_| {
            DeterministicDiscoveryError::new(DeterministicDiscoveryErrorKind::UnsafeEvidence)
        })?;
        if canonical.url().path() != "/"
            || canonical.url().query().is_some()
            || canonical.stripped_fragment()
            || canonical.stripped_sensitive_query_parameters() != 0
        {
            return Err(DeterministicDiscoveryError::new(
                DeterministicDiscoveryErrorKind::UnsafeEvidence,
            ));
        }
        let origin = CanonicalOrigin::parse(&canonical.origin().as_string()).map_err(|_| {
            DeterministicDiscoveryError::new(DeterministicDiscoveryErrorKind::UnsafeEvidence)
        })?;
        let base_path = EndpointPath::parse(base_path).map_err(|_| {
            DeterministicDiscoveryError::new(DeterministicDiscoveryErrorKind::UnsafeEvidence)
        })?;
        servers.push((origin, base_path));
    }
    servers.sort_by(|left, right| {
        (left.0.as_str(), left.1.as_str()).cmp(&(right.0.as_str(), right.1.as_str()))
    });
    servers.dedup();
    Ok(servers)
}

fn string_array<'a>(value: &'a Value, field: &str) -> Vec<&'a str> {
    value
        .get(field)
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .collect()
}

fn generation_path_matches(family: ApiFamily, path: &str) -> bool {
    let path = path.to_ascii_lowercase();
    match family {
        ApiFamily::OpenAiResponses => path.ends_with("/responses"),
        ApiFamily::OpenAiChatCompletions => path.ends_with("/chat/completions"),
        ApiFamily::AnthropicMessages => path.ends_with("/messages"),
        ApiFamily::GeminiGenerateContent => path.contains("generatecontent"),
        ApiFamily::OllamaNative => path.ends_with("/api/chat") || path.ends_with("/chat"),
    }
}

fn infer_base_path(family: ApiFamily, full_path: &str) -> Option<EndpointPath> {
    let lower = full_path.to_ascii_lowercase();
    let suffix_start = match family {
        ApiFamily::OpenAiResponses => lower.rfind("/responses"),
        ApiFamily::OpenAiChatCompletions => lower.rfind("/chat/completions"),
        ApiFamily::AnthropicMessages => lower.rfind("/messages"),
        ApiFamily::GeminiGenerateContent => lower.find("/models/"),
        ApiFamily::OllamaNative => lower.rfind("/chat"),
    }?;
    let prefix = &full_path[..suffix_start];
    let base = if prefix.is_empty() { "/" } else { prefix };
    if !base_path_is_persistence_safe(base) {
        return None;
    }
    EndpointPath::parse(base).ok()
}

fn infer_models_base_path(family: ApiFamily, full_path: &str) -> Option<EndpointPath> {
    let lower = full_path.to_ascii_lowercase();
    let suffix = if family == ApiFamily::OllamaNative && lower.ends_with("/tags") {
        "/tags"
    } else {
        "/models"
    };
    if !lower.ends_with(suffix) {
        return None;
    }
    let prefix = &full_path[..full_path.len().saturating_sub(suffix.len())];
    let base = if prefix.is_empty() { "/" } else { prefix };
    if !base_path_is_persistence_safe(base) {
        return None;
    }
    EndpointPath::parse(base).ok()
}

/// Restrict cURL-derived base paths to structural API/version segments.
///
/// Arbitrary path prefixes are not durable evidence: signed URLs and proxy
/// paths can embed credentials. Unknown prefixes therefore fall back to the
/// compiled adapter's default and remain available only in the transient
/// parser value for a later explicit user review.
fn base_path_is_persistence_safe(path: &str) -> bool {
    path == "/"
        || path.split('/').skip(1).all(|segment| {
            let normalized = segment.to_ascii_lowercase();
            matches!(
                normalized.as_str(),
                "api"
                    | "apis"
                    | "openai"
                    | "anthropic"
                    | "gemini"
                    | "ollama"
                    | "public"
                    | "beta"
                    | "preview"
            ) || version_path_segment(&normalized)
        })
}

fn version_path_segment(segment: &str) -> bool {
    let Some(version) = segment.strip_prefix('v') else {
        return false;
    };
    let (digits, suffix) = version
        .find(|character: char| !character.is_ascii_digit())
        .map_or((version, ""), |index| version.split_at(index));
    !digits.is_empty()
        && digits.bytes().all(|byte| byte.is_ascii_digit())
        && (suffix.is_empty()
            || suffix == "beta"
            || suffix == "preview"
            || suffix.strip_prefix("beta").is_some_and(|value| {
                !value.is_empty() && value.bytes().all(|byte| byte.is_ascii_digit())
            })
            || suffix.strip_prefix("preview").is_some_and(|value| {
                !value.is_empty() && value.bytes().all(|byte| byte.is_ascii_digit())
            }))
}

fn curl_auth_matches(hints: &[CurlAuthHint], auth: &AuthBinding) -> bool {
    match auth {
        AuthBinding::None => hints.is_empty(),
        AuthBinding::BearerHeader => hints.iter().any(|hint| {
            matches!(
                hint,
                CurlAuthHint::BearerHeader | CurlAuthHint::AuthorizationHeader
            )
        }),
        AuthBinding::HeaderApiKey { header_name } => hints.iter().any(|hint| {
            matches!(
                hint,
                CurlAuthHint::ApiKeyHeader { header_name: candidate }
                    if candidate == header_name
            )
        }),
    }
}

fn openapi_auth_matches(extracted: &Value, auth: &AuthBinding) -> bool {
    let Some(schemes) = extracted.get("auth_schemes").and_then(Value::as_array) else {
        return matches!(auth, AuthBinding::None);
    };
    match auth {
        AuthBinding::None => schemes.is_empty(),
        AuthBinding::BearerHeader => schemes.iter().any(|scheme| {
            scheme
                .get("scheme")
                .and_then(Value::as_str)
                .is_some_and(|value| value.eq_ignore_ascii_case("bearer"))
        }),
        AuthBinding::HeaderApiKey { header_name } => schemes.iter().any(|scheme| {
            scheme
                .get("location")
                .and_then(Value::as_str)
                .is_some_and(|value| value.eq_ignore_ascii_case("header"))
                && scheme
                    .get("parameter_name")
                    .and_then(Value::as_str)
                    .is_some_and(|value| value.eq_ignore_ascii_case(header_name.as_str()))
        }),
    }
}

#[cfg(test)]
mod tests {
    use std::io::{Read, Write};
    use std::net::{IpAddr, TcpListener};
    use std::thread;
    use std::time::Duration;

    use lorepia_providers::url_policy::ApprovedLocalNetworkOrigin;

    use super::*;

    fn assert_output_round_trip(output: &DeterministicDiscoveryOutput) {
        let encoded = serde_json::to_string(output).expect("serialize deterministic result");
        let hydrated: DeterministicDiscoveryResult =
            serde_json::from_str(&encoded).expect("hydrate deterministic result");
        assert_eq!(&hydrated, output);
    }

    fn approved_lan_policy(origin: &str, address: &str) -> UrlPolicy {
        let address = address.parse::<IpAddr>().expect("private IP address");
        let approval =
            ApprovedLocalNetworkOrigin::new(origin, &[address]).expect("exact LAN approval");
        UrlPolicy::approved_local_network(approval)
    }

    fn local_openapi_server(extra_response_padding: usize) -> (String, thread::JoinHandle<()>) {
        let listener = TcpListener::bind(("127.0.0.1", 0)).expect("bind fixture");
        let address = listener.local_addr().expect("fixture address");
        let document = format!(
            r#"{{
                "openapi":"3.1.0",
                "servers":[{{"url":"http://{address}/v1"}}],
                "paths":{{
                    "/models":{{"get":{{"operationId":"listModels"}}}},
                    "/responses":{{
                        "post":{{
                            "operationId":"createResponse",
                            "requestBody":{{
                                "content":{{
                                    "application/json":{{
                                        "schema":{{
                                            "type":"object",
                                            "properties":{{"model":{{"type":"string"}}}}
                                        }}
                                    }}
                                }}
                            }}
                        }}
                    }}
                }},
                "x-padding":"{}"
            }}"#,
            "x".repeat(extra_response_padding)
        );
        let start_url = format!("http://{address}/openapi.json");
        let handle = thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("accept fixture request");
            stream
                .set_read_timeout(Some(Duration::from_secs(3)))
                .expect("set fixture timeout");
            let mut request = [0_u8; 8 * 1024];
            let _ = stream.read(&mut request).expect("read fixture request");
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\n\
                 Content-Length: {}\r\nConnection: close\r\n\r\n{document}",
                document.len()
            );
            stream
                .write_all(response.as_bytes())
                .expect("write fixture response");
        });
        (start_url, handle)
    }

    #[tokio::test]
    async fn known_template_returns_selected_template_and_non_approval_hint() {
        let output = DeterministicDiscoveryExecutor::new()
            .execute(DeterministicDiscoverySource::known_provider(
                BuiltInTemplateId::OpenRouter,
            ))
            .await
            .expect("known provider");

        assert_eq!(
            output
                .selected_template
                .as_ref()
                .map(|template| template.id.as_str()),
            Some("openrouter-v1")
        );
        assert_eq!(output.connection_hints.len(), 1);
        assert_eq!(
            output.connection_hints[0].api_origin.as_str(),
            "https://openrouter.ai"
        );
        assert_eq!(
            output.connection_hints[0]
                .api_base_path
                .as_ref()
                .map(EndpointPath::as_str),
            Some("/api/v1")
        );
        assert!(
            output.connection_hints[0].requires_credential_origin_approval,
            "the hint must not be mistaken for approval"
        );
    }

    #[tokio::test]
    async fn active_signed_catalog_template_participates_in_known_provider_matching() {
        let mut template =
            AdapterRegistry::built_in_template(BuiltInTemplateId::OpenRouter).expect("template");
        template.id = ProviderTemplateId::from("signed-openrouter-compatible-v2");
        template.manifest_version = 2;
        template.source = TemplateSource::SignedCatalog;
        let source = DeterministicDiscoverySource::known_provider_id(template.id.clone());
        let output = DeterministicDiscoveryExecutor::new()
            .execute_with_templates(source, std::slice::from_ref(&template))
            .await
            .expect("signed catalog match");

        assert_eq!(output.selected_template, Some(template));
        assert_eq!(output.connection_hints.len(), 1);
        assert!(
            output.connection_hints[0].api_base_path.is_none(),
            "a signed manifest endpoint must not inherit a built-in base path by family"
        );
        assert_eq!(output.evidence[0].kind, "signed_catalog_template");
        assert_eq!(
            output.evidence[0].extracted_json["trust"],
            "verified_signed_catalog"
        );
    }

    #[tokio::test]
    async fn official_site_origin_matches_compiled_template() {
        let source = DeterministicDiscoverySource::known_provider_site(
            "https://ai.google.dev/gemini-api/docs",
            UrlPolicyMode::Public,
        )
        .expect("safe site");
        let output = DeterministicDiscoveryExecutor::new()
            .execute(source)
            .await
            .expect("known site");

        assert_eq!(
            output
                .selected_template
                .as_ref()
                .map(|template| template.api_family),
            Some(ApiFamily::GeminiGenerateContent)
        );
        assert_eq!(
            output.connection_hints[0].api_origin.as_str(),
            "https://generativelanguage.googleapis.com"
        );
    }

    #[tokio::test]
    async fn known_provider_site_preserves_approved_lan_network_hint() {
        let origin = "https://192.168.42.9:11434";
        let mut template =
            AdapterRegistry::built_in_template(BuiltInTemplateId::OllamaNative).expect("template");
        template.default_manifest.default_api_origin =
            Some(CanonicalOrigin::parse(origin).expect("domain origin"));
        let source = DeterministicDiscoverySource::known_provider_site_with_policy(
            &format!("{origin}/docs"),
            approved_lan_policy(origin, "192.168.42.9"),
        )
        .expect("approved LAN site");

        let output = DeterministicDiscoveryExecutor::new()
            .execute_with_templates(source, std::slice::from_ref(&template))
            .await
            .expect("known LAN provider");

        assert_eq!(output.selected_template, Some(template));
        assert_eq!(output.connection_hints.len(), 1);
        assert_eq!(
            output.connection_hints[0].network_mode,
            ProviderNetworkMode::ApprovedLocalNetwork
        );
    }

    #[tokio::test]
    async fn originless_custom_template_remains_selectable_without_fake_evidence() {
        let output = DeterministicDiscoveryExecutor::new()
            .execute(DeterministicDiscoverySource::known_provider(
                BuiltInTemplateId::OpenAiChatCompatible,
            ))
            .await
            .expect("custom template");

        assert_eq!(
            output
                .selected_template
                .as_ref()
                .map(|template| template.id.as_str()),
            Some("openai-chat-compatible-v1")
        );
        assert!(output.evidence.is_empty());
        assert!(output.connection_hints.is_empty());
        assert_eq!(output.family_candidates.len(), 1);
        assert!(output.family_candidates[0].evidence_indices.is_empty());
    }

    #[tokio::test]
    async fn deterministic_result_round_trips_through_draft_json() {
        let output = DeterministicDiscoveryExecutor::new()
            .execute(DeterministicDiscoverySource::known_provider(
                BuiltInTemplateId::AnthropicMessages,
            ))
            .await
            .expect("known provider");
        assert_output_round_trip(&output);
    }

    #[tokio::test]
    async fn site_result_round_trips_and_uses_the_bounded_credential_free_fetcher() {
        let (start_url, server) = local_openapi_server(0);
        let budget = DiscoveryFetchBudget {
            max_pages: 1,
            max_wall_clock: Duration::from_secs(3),
            max_request_duration: Duration::from_secs(3),
            ..DiscoveryFetchBudget::default()
        };
        let source =
            DeterministicDiscoverySource::site(&start_url, UrlPolicyMode::LocalLoopback, budget)
                .expect("policy-valid local fixture");
        let output = DeterministicDiscoveryExecutor::new()
            .execute(source)
            .await
            .expect("site discovery");
        server.join().expect("fixture server");

        assert_eq!(output.evidence.len(), 1);
        assert_eq!(output.manifest_candidates.len(), 1);
        assert_eq!(
            output
                .selected_template
                .as_ref()
                .map(|template| template.api_family),
            Some(ApiFamily::OpenAiResponses)
        );
        assert_eq!(
            output.connection_hints[0].network_mode,
            ProviderNetworkMode::LocalLoopback
        );
        assert!(
            output.connection_hints[0].api_base_path.is_none(),
            "the discovered template must own its API prefix"
        );
        let manifest = &output
            .selected_template
            .as_ref()
            .expect("selected discovered template")
            .default_manifest;
        assert_eq!(manifest.endpoints.generate.path.as_str(), "/v1/responses");
        assert_eq!(
            manifest
                .endpoints
                .models
                .as_ref()
                .expect("models endpoint")
                .path
                .as_str(),
            "/v1/models"
        );
        assert_output_round_trip(&output);
    }

    #[tokio::test]
    async fn site_fetch_enforces_the_caller_budget() {
        let (start_url, server) = local_openapi_server(4 * 1024);
        let budget = DiscoveryFetchBudget {
            max_pages: 1,
            max_response_bytes_per_document: 256,
            max_decompressed_bytes_per_document: 256,
            max_total_response_bytes: 256,
            max_wall_clock: Duration::from_secs(3),
            max_request_duration: Duration::from_secs(3),
            ..DiscoveryFetchBudget::default()
        };
        let source =
            DeterministicDiscoverySource::site(&start_url, UrlPolicyMode::LocalLoopback, budget)
                .expect("valid bounded plan");
        let output = DeterministicDiscoveryExecutor::new()
            .execute(source)
            .await
            .expect("bounded failure is a discovery result");
        server.join().expect("fixture server");

        assert!(output.evidence.is_empty());
        assert_eq!(output.fetch_issues.len(), 1);
        assert_eq!(output.fetch_issues[0].kind, "document_too_large");
        assert_output_round_trip(&output);
    }

    #[tokio::test]
    async fn c_url_is_consumed_once_and_no_scalar_or_credential_is_returned() {
        let secret = "sk-test-secret-never-return";
        let signed_path_secret = "signed-path-secret-never-return";
        let model_scalar = "credentiallookingmodelscalar";
        let raw = format!(
            "curl 'https://example.com/{signed_path_secret}/v1/chat/completions?api_key={secret}' \
             -H 'Authorization: Bearer {secret}' \
             -H 'Content-Type: application/json' \
             --data '{{\"model\":\"{model_scalar}\",\"messages\":[],\"stream\":true}}'"
        );
        let source = sanitize_curl_source(SecretCurlInput::new(raw), UrlPolicyMode::Public)
            .expect("sanitized source");
        let debug = format!("{source:?}");
        assert!(!debug.contains(secret));
        assert!(!debug.contains(model_scalar));
        assert!(!debug.contains(signed_path_secret));

        let output = DeterministicDiscoveryExecutor::new()
            .execute(source)
            .await
            .expect("curl discovery");
        let serialized = serde_json::to_string(&output).expect("serialize output");
        assert!(!serialized.contains(secret));
        assert!(!serialized.contains(model_scalar));
        assert!(!serialized.contains(signed_path_secret));
        assert!(!serialized.contains("redacted_curl"));
        assert!(!serialized.contains("model_hint"));
        assert!(serialized.contains("sanitized_curl_request"));
        assert!(
            output.evidence[0].extracted_json.get("path").is_none(),
            "cURL source paths must be represented only by their digest"
        );
        assert_eq!(output.family_candidates.len(), 1);
        assert_eq!(
            output.family_candidates[0].api_family,
            ApiFamily::OpenAiChatCompletions
        );
        assert_eq!(
            output.connection_hints[0]
                .api_base_path
                .as_ref()
                .map(EndpointPath::as_str),
            None
        );
        let manifest = &output
            .selected_template
            .as_ref()
            .expect("selected discovered template")
            .default_manifest;
        assert_eq!(
            manifest.endpoints.generate.path.as_str(),
            "/v1/chat/completions"
        );
        assert_eq!(
            manifest
                .endpoints
                .models
                .as_ref()
                .expect("models endpoint")
                .path
                .as_str(),
            "/v1/models"
        );
        assert_output_round_trip(&output);
    }

    #[test]
    fn embedding_a_safe_custom_base_is_idempotent_and_changes_the_manifest_hash() {
        let mut manifest =
            AdapterRegistry::built_in_template(BuiltInTemplateId::OpenAiChatCompatible)
                .expect("seed template")
                .default_manifest;
        let original_hash = lorepia_providers::validate_manifest(&manifest)
            .expect("validate seed manifest")
            .sha256()
            .to_owned();
        let base_path = EndpointPath::parse("/api/v2").expect("safe custom base");

        embed_discovered_api_base_path(&mut manifest, Some(&base_path)).expect("embed custom base");
        embed_discovered_api_base_path(&mut manifest, Some(&base_path))
            .expect("embedding is idempotent");

        assert_eq!(
            manifest.endpoints.generate.path.as_str(),
            "/api/v2/chat/completions"
        );
        assert_eq!(
            manifest
                .endpoints
                .models
                .as_ref()
                .expect("models endpoint")
                .path
                .as_str(),
            "/api/v2/models"
        );
        assert_ne!(
            lorepia_providers::validate_manifest(&manifest)
                .expect("validate embedded manifest")
                .sha256(),
            original_hash
        );

        let conflicting_base = EndpointPath::parse("/v3").expect("conflicting safe base");
        let error = embed_discovered_api_base_path(&mut manifest, Some(&conflicting_base))
            .expect_err("an already-evidenced base must not be nested under another base");
        assert_eq!(
            error.kind(),
            DeterministicDiscoveryErrorKind::UnsafeEvidence
        );
        assert_eq!(
            manifest.endpoints.generate.path.as_str(),
            "/api/v2/chat/completions"
        );

        let mut inconsistent =
            AdapterRegistry::built_in_template(BuiltInTemplateId::OpenAiChatCompatible)
                .expect("seed inconsistent template")
                .default_manifest;
        inconsistent.endpoints.generate.path =
            EndpointPath::parse("/chat/completions").expect("root generation endpoint");
        inconsistent
            .endpoints
            .models
            .as_mut()
            .expect("models endpoint")
            .path = EndpointPath::parse("/v3/models").expect("conflicting models endpoint");
        let unchanged = inconsistent.clone();
        let error = embed_discovered_api_base_path(&mut inconsistent, Some(&base_path))
            .expect_err("a secondary endpoint with another safe base must fail closed");
        assert_eq!(
            error.kind(),
            DeterministicDiscoveryErrorKind::UnsafeEvidence
        );
        assert_eq!(
            inconsistent, unchanged,
            "failed embedding must be atomic across every endpoint"
        );
    }

    #[test]
    fn site_source_strips_queries_and_fragments_and_rejects_invalid_budget() {
        let source = DeterministicDiscoverySource::site(
            "https://example.com/docs?token=secret&view=reference#credential",
            UrlPolicyMode::Public,
            DiscoveryFetchBudget::default(),
        )
        .expect("query and fragment are discarded");
        let debug = format!("{source:?}");
        assert!(!debug.contains("secret"));
        assert!(!debug.contains("reference"));
        assert!(!debug.contains("credential"));
        let DeterministicDiscoverySourceKind::Site { plan } = source.kind else {
            panic!("site source");
        };
        assert!(plan.start_url().url().query().is_none());
        assert!(plan.start_url().url().fragment().is_none());

        let budget = DiscoveryFetchBudget {
            max_pages: 0,
            ..DiscoveryFetchBudget::default()
        };
        let error = DeterministicDiscoverySource::site(
            "https://example.com/docs",
            UrlPolicyMode::Public,
            budget,
        )
        .expect_err("invalid budget");
        assert_eq!(
            error.kind(),
            DeterministicDiscoveryErrorKind::InvalidFetchBudget
        );
    }

    #[test]
    fn document_allowlist_does_not_create_credential_approval() {
        let mut source = DeterministicDiscoverySource::site(
            "https://docs.example.com",
            UrlPolicyMode::Public,
            DiscoveryFetchBudget {
                max_wall_clock: Duration::from_secs(1),
                max_request_duration: Duration::from_secs(1),
                ..DiscoveryFetchBudget::default()
            },
        )
        .expect("site");
        source
            .allow_document_url("https://api.example.com/openapi.json")
            .expect("allow document host");
        let debug = format!("{source:?}");
        assert!(!debug.contains("approved"));
        assert!(!debug.contains("credential"));
    }

    #[test]
    fn approved_lan_site_allowlist_retains_the_exact_policy() {
        let origin = "http://192.168.42.9:11434";
        let mut source = DeterministicDiscoverySource::site_with_policy(
            &format!("{origin}/docs"),
            approved_lan_policy(origin, "192.168.42.9"),
            DiscoveryFetchBudget::default(),
        )
        .expect("approved LAN site");

        source
            .allow_document_url(&format!("{origin}/openapi.json"))
            .expect("same exact approved origin");
        let error = source
            .allow_document_url("http://192.168.42.10:11434/openapi.json")
            .expect_err("another private origin must not inherit approval");
        assert_eq!(
            error.kind(),
            DeterministicDiscoveryErrorKind::InvalidDocumentUrl
        );
    }

    #[test]
    fn public_http_curl_origin_fails_closed_with_safe_error() {
        let error = sanitize_curl_source(
            SecretCurlInput::from(
                "curl -X POST http://api.example.com/v1/responses \
             -H 'Authorization: Bearer should-not-leak' \
             --data '{\"model\":\"safe-model\"}'",
            ),
            UrlPolicyMode::Public,
        )
        .expect_err("network policy rejects public HTTP");
        assert_eq!(
            error.kind(),
            DeterministicDiscoveryErrorKind::InvalidDocumentUrl
        );
        assert!(!error.to_string().contains("should-not-leak"));
        assert!(!error.to_string().contains("api.example.com"));
    }

    #[tokio::test]
    async fn loopback_curl_requires_explicit_local_mode() {
        let command = "curl -X POST http://127.0.0.1:11434/api/chat \
                       --data '{\"model\":\"llama3\",\"messages\":[]}'";
        let public_error =
            sanitize_curl_source(SecretCurlInput::from(command), UrlPolicyMode::Public)
                .expect_err("public mode must not authorize loopback");
        assert_eq!(
            public_error.kind(),
            DeterministicDiscoveryErrorKind::InvalidDocumentUrl
        );

        let local =
            sanitize_curl_source(SecretCurlInput::from(command), UrlPolicyMode::LocalLoopback)
                .expect("explicit local mode");
        let output = DeterministicDiscoveryExecutor::new()
            .execute(local)
            .await
            .expect("local cURL discovery");
        assert_eq!(
            output.connection_hints[0].network_mode,
            ProviderNetworkMode::LocalLoopback
        );
        assert_output_round_trip(&output);
    }

    #[tokio::test]
    async fn approved_lan_curl_retains_exact_policy_through_execution() {
        let origin = "http://192.168.42.9:11434";
        let parsed = parse_curl(SecretCurlInput::from(format!(
            "curl -X POST {origin}/api/chat --data '{{\"model\":\"llama3\",\"messages\":[]}}'"
        )))
        .expect("parse LAN cURL");
        let source = DeterministicDiscoverySource::sanitized_curl_with_policy(
            parsed,
            approved_lan_policy(origin, "192.168.42.9"),
        )
        .expect("approved LAN source");

        let output = DeterministicDiscoveryExecutor::new()
            .execute(source)
            .await
            .expect("approved LAN cURL discovery");

        assert_eq!(output.connection_hints.len(), 1);
        assert_eq!(
            output.connection_hints[0].network_mode,
            ProviderNetworkMode::ApprovedLocalNetwork
        );
        assert!(
            output
                .selected_template
                .as_ref()
                .is_some_and(|template| template.default_manifest.default_api_origin.is_none()),
            "an exact LAN grant must remain connection-specific"
        );
        assert_output_round_trip(&output);
    }

    #[test]
    fn sanitized_curl_json_omits_model_hint_and_redacted_command() {
        let parsed = parse_curl(SecretCurlInput::from(
            "curl -X POST https://example.com/v1/responses \
             -H 'Authorization: Bearer top-secret' \
             --data '{\"model\":\"secret-shaped-model\",\"input\":\"private\"}'",
        ))
        .expect("parse");
        let sanitized = SanitizedCurlDiscoveryEvidence::from(parsed);
        let value = sanitized_curl_json(&sanitized).expect("sanitized JSON");
        let encoded = serde_json::to_string(&value).expect("encode");
        assert!(!encoded.contains("secret-shaped-model"));
        assert!(!encoded.contains("top-secret"));
        assert!(!encoded.contains("private"));
        assert!(!encoded.contains("model_hint"));
        assert!(!encoded.contains("redacted_curl"));
    }

    #[test]
    fn extracted_curl_body_is_only_a_scalar_free_shape() {
        let parsed = parse_curl(SecretCurlInput::from(
            "curl -X POST https://example.com/v1/responses \
             --data '{\"input\":\"do-not-store\",\"nested\":{\"token\":\"also-secret\"}}'",
        ))
        .expect("parse");
        let shape = parsed.body_json_shape.as_ref().expect("shape");
        assert!(matches!(shape, lorepia_providers::JsonShape::Object { .. }));
        let sanitized = SanitizedCurlDiscoveryEvidence::from(parsed);
        let encoded =
            serde_json::to_string(&sanitized_curl_json(&sanitized).expect("safe")).expect("encode");
        assert!(!encoded.contains("do-not-store"));
        assert!(!encoded.contains("also-secret"));
    }
}
