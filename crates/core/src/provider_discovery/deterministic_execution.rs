use lorepia_domain::{
    CanonicalOrigin, EndpointPath, HttpMethod, ProviderNetworkMode, ProviderTemplate,
    TemplateSource,
};
use lorepia_providers::{
    AdapterRegistry, BuiltInTemplateId,
    discovery::{
        BoundedDocumentFetcher, DiscoveryDocumentEvidence, DiscoveryEvidenceKind,
        DiscoveryFetchIssue, DiscoveryFetchIssueKind, DiscoveryFetchPlan,
    },
    url_policy::{UrlNetworkBoundary, UrlPolicy},
};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use super::{
    resolution::{
        connection_hint, curl_auth_matches, deduplicate_candidates_and_hints, discovered_template,
        generation_path_matches, infer_base_path, openapi_auth_matches, openapi_family_candidates,
        openapi_server_candidates, push_family_candidate, push_family_candidate_without_evidence,
        seed_default_base_path, seed_template, select_unambiguous_template, string_array,
    },
    source::{
        ConnectionOriginHintSource, DeterministicDiscoveryError, DeterministicDiscoveryErrorKind,
        DeterministicDiscoveryFetchIssue, DeterministicDiscoveryOutput,
        DeterministicDiscoverySource, DeterministicDiscoverySourceKind,
        DiscoveryCandidateConfidence, DiscoveryManifestCandidate, KnownProviderSelector,
        REDACTION_VERSION, RedactedDiscoveryEvidenceRecord, SanitizedCurlDiscoveryEvidence,
    },
};

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

pub(super) fn canonical_secret_free_document_url_with_policy(
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

pub(super) fn validate_curl_endpoint_policy(
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

pub(super) const fn invalid_document_url() -> DeterministicDiscoveryError {
    DeterministicDiscoveryError::new(DeterministicDiscoveryErrorKind::InvalidDocumentUrl)
}

pub(super) const fn contract_error() -> DeterministicDiscoveryError {
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

pub(super) fn sanitized_curl_json(
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

pub(super) fn sha256_bytes(value: &[u8]) -> String {
    let digest = Sha256::digest(value);
    format!("{digest:x}")
}
