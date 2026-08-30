//! Deterministic discovery contracts, canonical encoding, and audit persistence.

use chrono::{DateTime, Utc};
use lorepia_domain::{
    ApiFamily, AuthBinding, CanonicalOrigin, CoreError, CoreResult, EndpointPath, HeaderName,
    HttpMethod, ProviderNetworkMode, ProviderTemplate, TemplateSource,
    discovery::{
        DiscoveryApprovalDecision, DiscoveryApprovalGrant, DiscoveryCandidate, DiscoveryCommitPlan,
        DiscoveryOperationKind, DiscoverySideEffectClass, DiscoveryState, SanitizedDiscoveryInput,
    },
};
use rusqlite::{Transaction, params};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};

use super::{
    DISCOVERY_REDACTION_VERSION,
    errors::{contract_error, corrupted, database_error},
    validation::{
        MAX_DISCOVERY_JSON_BYTES, MAX_DISCOVERY_JSON_CHARS, looks_like_secret, validate_identifier,
        validate_opaque_credential_reference, validate_redacted_value, validate_sha256,
    },
};

const DETERMINISTIC_DISCOVERY_SCHEMA_VERSION: u32 = 1;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct InitialDeterministicDiscoveryOutput {
    schema_version: u32,
    selected_template: Option<ProviderTemplate>,
    evidence: Vec<InitialRedactedDiscoveryEvidence>,
    family_candidates: Vec<InitialDiscoveryFamilyCandidate>,
    manifest_candidates: Vec<InitialDiscoveryManifestCandidate>,
    connection_hints: Vec<InitialDiscoveryConnectionHint>,
    fetch_issues: Vec<InitialDiscoveryFetchIssue>,
    fetch_stopped_by_budget: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct InitialRedactedDiscoveryEvidence {
    kind: String,
    source_origin: CanonicalOrigin,
    content_sha256: String,
    extracted_json: Value,
    redaction_version: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum InitialDiscoveryCandidateConfidence {
    Structural,
    ExactCompiledProvider,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct InitialDiscoveryFamilyCandidate {
    api_family: ApiFamily,
    confidence: InitialDiscoveryCandidateConfidence,
    evidence_indices: Vec<usize>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct InitialDiscoveryManifestCandidate {
    template: ProviderTemplate,
    manifest_sha256: String,
    confidence: InitialDiscoveryCandidateConfidence,
    generation_endpoint_evidenced: bool,
    model_endpoint_evidenced: bool,
    auth_evidenced: bool,
    evidence_indices: Vec<usize>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum InitialConnectionOriginHintSource {
    CompiledProviderDefault,
    OpenApiServer,
    SanitizedCurlRequest,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct InitialDiscoveryConnectionHint {
    api_family: ApiFamily,
    api_origin: CanonicalOrigin,
    api_base_path: Option<EndpointPath>,
    network_mode: ProviderNetworkMode,
    auth: AuthBinding,
    requires_credential_origin_approval: bool,
    source: InitialConnectionOriginHintSource,
    evidence_indices: Vec<usize>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct InitialDiscoveryFetchIssue {
    source_origin: CanonicalOrigin,
    source_path_sha256: String,
    source_path_is_root: bool,
    kind: String,
    http_status: Option<u16>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
enum InitialCurlAuthHint {
    BearerHeader,
    AuthorizationHeader,
    ApiKeyHeader { header_name: HeaderName },
    CookieHeader { header_name: HeaderName },
    ApiKeyQuery { parameter_name: String },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
enum InitialJsonShape {
    Null,
    Boolean,
    Number,
    String,
    Array { items: Vec<Self>, truncated: bool },
    Object { fields: Vec<InitialJsonFieldShape> },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct InitialJsonFieldShape {
    name: String,
    shape: InitialJsonShape,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct InitialSanitizedCurlEvidence {
    method: HttpMethod,
    origin: CanonicalOrigin,
    source_path_sha256: String,
    source_path_is_root: bool,
    query_parameter_names: Vec<String>,
    header_names: Vec<HeaderName>,
    auth_hints: Vec<InitialCurlAuthHint>,
    body_json_shape: Option<InitialJsonShape>,
    stream_hint: Option<bool>,
    api_family_candidates: Vec<ApiFamily>,
    trust: String,
}

pub(super) fn validate_initial_discovery_draft(
    value: &Value,
    input: &SanitizedDiscoveryInput,
) -> CoreResult<()> {
    const EXPECTED_KEYS: [&str; 15] = [
        "schema_version",
        "source",
        "deterministic",
        "evidence_ids",
        "extra_evidence_ids",
        "selected_candidate_id",
        "template",
        "connection",
        "routes",
        "observations",
        "presets",
        "credential_approval_id",
        "probe_route_ids",
        "probe_failure_count",
        "assistant",
    ];
    validate_redacted_value(value)?;
    let object = value
        .as_object()
        .ok_or_else(|| CoreError::invalid("initial discovery draft must be a JSON object"))?;
    if object.len() != EXPECTED_KEYS.len()
        || EXPECTED_KEYS.iter().any(|key| !object.contains_key(*key))
        || object.get("schema_version").and_then(Value::as_u64) != Some(1)
        || object.get("probe_failure_count").and_then(Value::as_u64) != Some(0)
        || ["selected_candidate_id", "template", "connection"]
            .into_iter()
            .chain(["credential_approval_id", "assistant"])
            .any(|key| !object[key].is_null())
        || [
            "evidence_ids",
            "extra_evidence_ids",
            "routes",
            "observations",
            "presets",
            "probe_route_ids",
        ]
        .into_iter()
        .any(|key| {
            object[key]
                .as_array()
                .is_none_or(|values| !values.is_empty())
        })
    {
        return Err(CoreError::invalid(
            "initial discovery draft must contain only pristine source intent",
        ));
    }
    let source = object["source"]
        .as_object()
        .ok_or_else(|| CoreError::invalid("initial discovery source intent is invalid"))?;
    match source.get("kind").and_then(Value::as_str) {
        Some("site") if source.len() == 1 && object["deterministic"].is_null() => Ok(()),
        Some("curl") if source.len() == 1 => {
            validate_initial_curl_deterministic_output(&object["deterministic"], input)
        }
        Some("known_provider") if source.len() == 2 => {
            if !object["deterministic"].is_null() {
                return Err(CoreError::invalid(
                    "known-provider discovery cannot begin with transient deterministic output",
                ));
            }
            let template_id = source
                .get("template_id")
                .and_then(Value::as_str)
                .ok_or_else(|| {
                    CoreError::invalid("known-provider source intent has no template identifier")
                })?;
            validate_identifier("known-provider source template id", template_id, 256)?;
            if looks_like_secret(template_id) {
                return Err(CoreError::invalid(
                    "known-provider source template id resembles credential material",
                ));
            }
            Ok(())
        }
        _ => Err(CoreError::invalid(
            "initial discovery source intent is unsupported or contains payload data",
        )),
    }
}

#[allow(clippy::too_many_lines)]
fn validate_initial_curl_deterministic_output(
    value: &Value,
    input: &SanitizedDiscoveryInput,
) -> CoreResult<()> {
    let output = serde_json::from_value::<InitialDeterministicDiscoveryOutput>(value.clone())
        .map_err(|_| {
            CoreError::invalid("initial cURL deterministic output has an invalid schema")
        })?;
    let canonical = serde_json::to_value(&output)
        .map_err(|_| CoreError::internal("cannot canonicalize cURL deterministic output"))?;
    if canonical != *value {
        return Err(CoreError::invalid(
            "initial cURL deterministic output contains non-canonical fields",
        ));
    }
    if output.schema_version != DETERMINISTIC_DISCOVERY_SCHEMA_VERSION
        || output.evidence.len() != 1
        || output.family_candidates.len() > 5
        || output.manifest_candidates.len() > 5
        || output.connection_hints.len() > 5
        || !output.fetch_issues.is_empty()
        || output.fetch_stopped_by_budget
    {
        return Err(CoreError::invalid(
            "initial cURL deterministic output violates its bounded contract",
        ));
    }

    let input_origin = CanonicalOrigin::parse(input.site_url.as_str())
        .map_err(|_| CoreError::invalid("initial cURL input site URL is not an origin"))?;
    let evidence = &output.evidence[0];
    if evidence.kind != "sanitized_curl_request"
        || evidence.source_origin != input_origin
        || evidence.redaction_version != DISCOVERY_REDACTION_VERSION
    {
        return Err(CoreError::invalid(
            "initial cURL evidence is not bound to the sanitized input origin",
        ));
    }
    validate_sha256(
        "initial cURL evidence content hash",
        &evidence.content_sha256,
    )?;
    let extracted =
        serde_json::from_value::<InitialSanitizedCurlEvidence>(evidence.extracted_json.clone())
            .map_err(|_| {
                CoreError::invalid("initial cURL evidence has an invalid sanitized shape")
            })?;
    let canonical_extracted = serde_json::to_value(&extracted)
        .map_err(|_| CoreError::internal("cannot canonicalize sanitized cURL evidence"))?;
    if canonical_extracted != evidence.extracted_json {
        return Err(CoreError::invalid(
            "initial cURL evidence contains non-canonical fields",
        ));
    }
    let extracted_bytes = serde_json::to_vec(&evidence.extracted_json)
        .map_err(|_| CoreError::internal("cannot hash sanitized cURL evidence"))?;
    let extracted_sha256 = format!("{:x}", Sha256::digest(extracted_bytes));
    if evidence.content_sha256 != extracted_sha256
        || extracted.origin != evidence.source_origin
        || extracted.trust != "sanitized_curl_structure"
    {
        return Err(CoreError::invalid(
            "initial cURL evidence content hash or provenance is invalid",
        ));
    }
    validate_sha256(
        "initial cURL source path hash",
        &extracted.source_path_sha256,
    )?;
    if extracted.query_parameter_names.len() > 64
        || extracted.header_names.len() > 64
        || extracted.auth_hints.len() > 64
        || extracted.api_family_candidates.len() > 5
    {
        return Err(CoreError::invalid(
            "initial cURL evidence exceeds parser collection bounds",
        ));
    }
    for name in &extracted.query_parameter_names {
        validate_identifier("initial cURL query parameter name", name, 256)?;
    }
    for hint in &extracted.auth_hints {
        if let InitialCurlAuthHint::ApiKeyQuery { parameter_name } = hint {
            validate_identifier(
                "initial cURL authentication query parameter name",
                parameter_name,
                256,
            )?;
        }
    }

    for (index, candidate) in output.family_candidates.iter().enumerate() {
        if candidate.confidence != InitialDiscoveryCandidateConfidence::Structural
            || candidate.evidence_indices.as_slice() != [0]
            || !extracted
                .api_family_candidates
                .contains(&candidate.api_family)
            || output.family_candidates[..index]
                .iter()
                .any(|previous| previous.api_family == candidate.api_family)
        {
            return Err(CoreError::invalid(
                "initial cURL family candidate has invalid evidence provenance",
            ));
        }
    }
    for (index, candidate) in output.manifest_candidates.iter().enumerate() {
        validate_sha256(
            "initial cURL manifest candidate hash",
            &candidate.manifest_sha256,
        )?;
        let manifest_json = canonical_json_result(
            serde_json::to_value(&candidate.template.default_manifest),
            "initial cURL provider manifest",
        )?;
        let actual_manifest_sha256 = sha256_hex(manifest_json.as_bytes());
        let expected_template_id = format!("discovered-{}", candidate.manifest_sha256);
        if candidate.confidence != InitialDiscoveryCandidateConfidence::Structural
            || !candidate.generation_endpoint_evidenced
            || candidate.model_endpoint_evidenced
            || candidate.evidence_indices.as_slice() != [0]
            || candidate.manifest_sha256 != actual_manifest_sha256
            || candidate.template.id.as_str() != expected_template_id
            || candidate.template.manifest_version != 1
            || candidate.template.source != TemplateSource::UserDiscovered
            || candidate.template.api_family != candidate.template.default_manifest.api_family
            || !output
                .family_candidates
                .iter()
                .any(|family| family.api_family == candidate.template.api_family)
            || output.manifest_candidates[..index]
                .iter()
                .any(|previous| previous.manifest_sha256 == candidate.manifest_sha256)
        {
            return Err(CoreError::invalid(
                "initial cURL manifest candidate has invalid deterministic provenance",
            ));
        }
        let default_origin = candidate
            .template
            .default_manifest
            .default_api_origin
            .as_ref();
        if input.connection_options.network_mode == ProviderNetworkMode::ApprovedLocalNetwork {
            if default_origin.is_some() || !candidate.template.default_manifest.sources.is_empty() {
                return Err(CoreError::invalid(
                    "initial LAN cURL manifest promoted connection-specific authority",
                ));
            }
        } else if default_origin != Some(&evidence.source_origin) {
            return Err(CoreError::invalid(
                "initial cURL manifest origin does not match sanitized evidence",
            ));
        }
    }

    for (index, hint) in output.connection_hints.iter().enumerate() {
        if hint.source != InitialConnectionOriginHintSource::SanitizedCurlRequest
            || hint.api_origin != evidence.source_origin
            || hint.network_mode != input.connection_options.network_mode
            || hint.evidence_indices.as_slice() != [0]
            || hint.requires_credential_origin_approval != (hint.auth != AuthBinding::None)
            || !output
                .manifest_candidates
                .iter()
                .any(|candidate| candidate.template.api_family == hint.api_family)
            || output.connection_hints[..index].iter().any(|previous| {
                previous.api_family == hint.api_family
                    && previous.api_origin == hint.api_origin
                    && previous.api_base_path == hint.api_base_path
            })
        {
            return Err(CoreError::invalid(
                "initial cURL connection hint has invalid deterministic provenance",
            ));
        }
    }

    let expected_selected = if output.manifest_candidates.len() == 1 {
        Some(&output.manifest_candidates[0].template)
    } else {
        None
    };
    if output.selected_template.as_ref() != expected_selected {
        return Err(CoreError::invalid(
            "initial cURL selected template is not bound to the candidate set",
        ));
    }
    Ok(())
}

pub(super) fn encode_redacted_json(value: &Value, label: &str) -> CoreResult<String> {
    if !value.is_object() {
        return Err(CoreError::invalid(format!("{label} must be a JSON object")));
    }
    validate_redacted_value(value)?;
    let json = serde_json::to_string(value)
        .map_err(|_| CoreError::internal(format!("cannot encode {label}")))?;
    if json.len() > MAX_DISCOVERY_JSON_BYTES || json.chars().count() > MAX_DISCOVERY_JSON_CHARS {
        return Err(CoreError::invalid(format!(
            "{label} exceeds the persistence bound"
        )));
    }
    Ok(json)
}

pub(super) fn decode_redacted_json(json: &str, label: &str) -> CoreResult<Value> {
    if json.len() > MAX_DISCOVERY_JSON_BYTES || json.chars().count() > MAX_DISCOVERY_JSON_CHARS {
        return Err(corrupted(format!("{label} exceeds its storage bound")));
    }
    let value =
        serde_json::from_str(json).map_err(|_| corrupted(format!("{label} is invalid JSON")))?;
    validate_redacted_value(&value)
        .map_err(|_| corrupted(format!("{label} contains forbidden data")))?;
    Ok(value)
}

pub(super) fn encode_json_result(
    value: Result<Value, serde_json::Error>,
    label: &str,
) -> CoreResult<String> {
    let value = value.map_err(|_| CoreError::internal(format!("cannot encode {label}")))?;
    validate_redacted_value(&value)?;
    let json = serde_json::to_string(&value)
        .map_err(|_| CoreError::internal(format!("cannot encode {label}")))?;
    if json.len() > MAX_DISCOVERY_JSON_BYTES || json.chars().count() > MAX_DISCOVERY_JSON_CHARS {
        return Err(CoreError::invalid(format!(
            "{label} exceeds the persistence bound"
        )));
    }
    Ok(json)
}

pub(super) fn encode_approval_grant(grant: &DiscoveryApprovalGrant) -> CoreResult<String> {
    let json = serde_json::to_string(grant)
        .map_err(|_| CoreError::internal("cannot encode discovery approval grant"))?;
    let value = serde_json::from_str(&json)
        .map_err(|_| CoreError::internal("cannot inspect discovery approval grant"))?;
    validate_redacted_value(&value)?;
    if json.len() > MAX_DISCOVERY_JSON_BYTES || json.chars().count() > MAX_DISCOVERY_JSON_CHARS {
        return Err(CoreError::invalid(
            "discovery approval grant exceeds the persistence bound",
        ));
    }
    Ok(json)
}

pub(super) fn encode_commit_plan_json(plan: &DiscoveryCommitPlan) -> CoreResult<String> {
    plan.validate().map_err(contract_error)?;
    if let Some(reference) = &plan.credential_ref {
        validate_opaque_credential_reference(reference.as_str())?;
    }
    let json = serde_json::to_string(plan)
        .map_err(|_| CoreError::internal("cannot encode discovery commit plan"))?;
    if json.len() > MAX_DISCOVERY_JSON_BYTES || json.chars().count() > MAX_DISCOVERY_JSON_CHARS {
        return Err(CoreError::invalid(
            "discovery commit plan exceeds the persistence bound",
        ));
    }
    Ok(json)
}

pub(crate) fn canonical_discovery_commit_plan_sha256(plan_json: &str) -> Option<String> {
    let plan = serde_json::from_str::<DiscoveryCommitPlan>(plan_json).ok()?;
    let canonical = encode_commit_plan_json(&plan).ok()?;
    (canonical == plan_json).then(|| sha256_hex(plan_json.as_bytes()))
}

pub(super) fn candidate_kind(candidate: &DiscoveryCandidate) -> &'static str {
    match candidate.summary {
        lorepia_domain::discovery::DiscoveryCandidateSummary::ProviderTemplate { .. } => {
            "provider_template"
        }
        lorepia_domain::discovery::DiscoveryCandidateSummary::ApiOrigin { .. } => "api_origin",
        lorepia_domain::discovery::DiscoveryCandidateSummary::OfficialDocument { .. } => {
            "official_document"
        }
        lorepia_domain::discovery::DiscoveryCandidateSummary::ModelRoute { .. } => "model_route",
        lorepia_domain::discovery::DiscoveryCandidateSummary::ManifestDraft { .. } => {
            "manifest_draft"
        }
    }
}

#[allow(clippy::too_many_arguments)]
pub(super) fn append_audit(
    transaction: &Transaction<'_>,
    session_id: &str,
    revision: u64,
    kind: &str,
    action_id: Option<&str>,
    subject_id: Option<&str>,
    summary_key: &str,
    created_at: DateTime<Utc>,
) -> CoreResult<()> {
    let sequence = transaction
        .query_row(
            "SELECT COALESCE(MAX(audit_sequence), 0) + 1
             FROM provider_discovery_audit_log
             WHERE session_id = ?1",
            [session_id],
            |row| row.get::<_, u64>(0),
        )
        .map_err(database_error)?;
    transaction
        .execute(
            "INSERT INTO provider_discovery_audit_log (
                 session_id, audit_sequence, session_revision, audit_kind,
                 action_id, subject_id, summary_key, created_at
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![
                session_id,
                sequence,
                revision,
                kind,
                action_id,
                subject_id,
                summary_key,
                created_at.to_rfc3339(),
            ],
        )
        .map_err(database_error)?;
    Ok(())
}

pub(super) fn parse_timestamp(value: &str, label: &str) -> CoreResult<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(value)
        .map(|value| value.with_timezone(&Utc))
        .map_err(|_| corrupted(format!("stored {label} is invalid")))
}

pub(super) fn parse_discovery_state(value: &str) -> CoreResult<DiscoveryState> {
    serde_json::from_value(Value::String(value.to_owned()))
        .map_err(|_| corrupted("stored discovery state is invalid"))
}

pub(super) fn parse_operation_kind(value: &str) -> CoreResult<DiscoveryOperationKind> {
    serde_json::from_value(Value::String(value.to_owned()))
        .map_err(|_| corrupted("stored discovery operation kind is invalid"))
}

pub(super) fn parse_side_effect_class(value: &str) -> CoreResult<DiscoverySideEffectClass> {
    serde_json::from_value(Value::String(value.to_owned()))
        .map_err(|_| corrupted("stored discovery side-effect class is invalid"))
}

pub(super) fn parse_approval_decision(value: &str) -> CoreResult<DiscoveryApprovalDecision> {
    serde_json::from_value(Value::String(value.to_owned()))
        .map_err(|_| corrupted("stored discovery approval decision is invalid"))
}

fn json_enum_wire(value: Value, label: &str) -> CoreResult<String> {
    value
        .as_str()
        .map(str::to_owned)
        .ok_or_else(|| CoreError::internal(format!("{label} did not serialize as a string")))
}

pub(super) fn enum_wire_result(
    value: Result<Value, serde_json::Error>,
    label: &str,
) -> CoreResult<String> {
    json_enum_wire(
        value.map_err(|_| CoreError::internal(format!("cannot encode {label}")))?,
        label,
    )
}

pub(super) fn canonical_json_result(
    value: Result<Value, serde_json::Error>,
    label: &str,
) -> CoreResult<String> {
    let value = value.map_err(|_| CoreError::internal(format!("cannot encode {label}")))?;
    validate_redacted_value(&value)?;
    canonical_typed_value(value, label)
}

pub(super) fn canonical_typed_json_result(
    value: Result<Value, serde_json::Error>,
    label: &str,
) -> CoreResult<String> {
    let value = value.map_err(|_| CoreError::internal(format!("cannot encode {label}")))?;
    canonical_typed_value(value, label)
}

fn canonical_typed_value(value: Value, label: &str) -> CoreResult<String> {
    let mut output = String::new();
    write_canonical_json(&value, &mut output)?;
    if output.len() > MAX_DISCOVERY_JSON_BYTES || output.chars().count() > MAX_DISCOVERY_JSON_CHARS
    {
        return Err(CoreError::invalid(format!(
            "{label} exceeds the persistence bound"
        )));
    }
    Ok(output)
}

fn write_canonical_json(value: &Value, output: &mut String) -> CoreResult<()> {
    match value {
        Value::Null => output.push_str("null"),
        Value::Bool(value) => output.push_str(if *value { "true" } else { "false" }),
        Value::Number(value) => {
            if value.as_f64().is_some_and(|value| value == 0.0) {
                output.push('0');
            } else {
                output.push_str(&value.to_string());
            }
        }
        Value::String(value) => output.push_str(
            &serde_json::to_string(value)
                .map_err(|_| CoreError::internal("cannot encode canonical JSON string"))?,
        ),
        Value::Array(values) => {
            output.push('[');
            for (index, value) in values.iter().enumerate() {
                if index != 0 {
                    output.push(',');
                }
                write_canonical_json(value, output)?;
            }
            output.push(']');
        }
        Value::Object(values) => {
            output.push('{');
            let mut keys = values.keys().collect::<Vec<_>>();
            keys.sort_unstable();
            for (index, key) in keys.into_iter().enumerate() {
                if index != 0 {
                    output.push(',');
                }
                output.push_str(
                    &serde_json::to_string(key)
                        .map_err(|_| CoreError::internal("cannot encode canonical JSON key"))?,
                );
                output.push(':');
                write_canonical_json(&values[key], output)?;
            }
            output.push('}');
        }
    }
    Ok(())
}

pub(super) fn sha256_hex(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}
