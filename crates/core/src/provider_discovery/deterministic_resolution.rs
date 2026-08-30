use std::collections::BTreeMap;

use lorepia_domain::{
    ApiFamily, AuthBinding, CanonicalOrigin, ConnectionFieldSpec, EndpointPath, HttpUrl,
    ManifestSource, ManifestSourceKind, ProviderManifest, ProviderNetworkMode, ProviderTemplate,
    ProviderTemplateId, TemplateSource,
};
use lorepia_providers::{
    AdapterRegistry, BuiltInTemplateId, CurlAuthHint,
    url_policy::{UrlNetworkBoundary, UrlPolicy},
};
use serde_json::Value;

use super::{
    execution::contract_error,
    source::{
        ConnectionOriginHintSource, DISCOVERED_TEMPLATE_VERSION, DeterministicDiscoveryError,
        DeterministicDiscoveryErrorKind, DeterministicDiscoveryOutput,
        DiscoveryCandidateConfidence, DiscoveryConnectionDraftHint, DiscoveryFamilyCandidate,
        DiscoveryManifestCandidate,
    },
};

pub(super) fn seed_template(
    family: ApiFamily,
) -> Result<ProviderTemplate, DeterministicDiscoveryError> {
    let id = match family {
        ApiFamily::OpenAiResponses => BuiltInTemplateId::OpenAiResponses,
        ApiFamily::OpenAiChatCompletions => BuiltInTemplateId::OpenAiChatCompatible,
        ApiFamily::AnthropicMessages => BuiltInTemplateId::AnthropicMessages,
        ApiFamily::GeminiGenerateContent => BuiltInTemplateId::GeminiGenerateContent,
        ApiFamily::OllamaNative => BuiltInTemplateId::OllamaNative,
    };
    AdapterRegistry::built_in_template(id).map_err(|_| contract_error())
}

pub(super) const fn seed_default_base_path(family: ApiFamily) -> &'static str {
    match family {
        ApiFamily::OpenAiResponses
        | ApiFamily::OpenAiChatCompletions
        | ApiFamily::AnthropicMessages => "/v1",
        ApiFamily::GeminiGenerateContent => "/v1beta",
        ApiFamily::OllamaNative => "/api",
    }
}

pub(super) fn discovered_template(
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

pub(super) fn connection_hint(
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

pub(super) fn push_family_candidate(
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

pub(super) fn push_family_candidate_without_evidence(
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

pub(super) fn select_unambiguous_template(output: &mut DeterministicDiscoveryOutput) {
    if output.manifest_candidates.len() == 1
        && output.manifest_candidates[0].generation_endpoint_evidenced
    {
        output.selected_template = Some(output.manifest_candidates[0].template.clone());
    }
}

pub(super) fn deduplicate_candidates_and_hints(output: &mut DeterministicDiscoveryOutput) {
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

pub(super) fn openapi_family_candidates(extracted: &Value) -> Vec<ApiFamily> {
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

pub(super) fn openapi_server_candidates(
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

pub(super) fn string_array<'a>(value: &'a Value, field: &str) -> Vec<&'a str> {
    value
        .get(field)
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .collect()
}

pub(super) fn generation_path_matches(family: ApiFamily, path: &str) -> bool {
    let path = path.to_ascii_lowercase();
    match family {
        ApiFamily::OpenAiResponses => path.ends_with("/responses"),
        ApiFamily::OpenAiChatCompletions => path.ends_with("/chat/completions"),
        ApiFamily::AnthropicMessages => path.ends_with("/messages"),
        ApiFamily::GeminiGenerateContent => path.contains("generatecontent"),
        ApiFamily::OllamaNative => path.ends_with("/api/chat") || path.ends_with("/chat"),
    }
}

pub(super) fn infer_base_path(family: ApiFamily, full_path: &str) -> Option<EndpointPath> {
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

pub(super) fn curl_auth_matches(hints: &[CurlAuthHint], auth: &AuthBinding) -> bool {
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

pub(super) fn openapi_auth_matches(extracted: &Value, auth: &AuthBinding) -> bool {
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
