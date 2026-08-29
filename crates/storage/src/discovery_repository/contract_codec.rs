//! Bounded value validation, canonical encoding, and audit persistence helpers.

use super::*;

pub(super) fn validate_limit(limit: u32) -> CoreResult<()> {
    if limit == 0 || limit > MAX_DISCOVERY_ROWS {
        return Err(CoreError::invalid(
            "discovery list limit must be from 1 to 1000",
        ));
    }
    Ok(())
}

pub(super) fn validate_identifier(label: &str, value: &str, maximum: usize) -> CoreResult<()> {
    if value.is_empty()
        || value.len() > maximum
        || value.trim() != value
        || value.chars().any(char::is_control)
    {
        return Err(CoreError::invalid(format!(
            "{label} must be a bounded trimmed opaque identifier"
        )));
    }
    Ok(())
}

pub(super) fn validate_sha256(label: &str, value: &str) -> CoreResult<()> {
    if value.len() != 64
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err(CoreError::invalid(format!(
            "{label} must be a lowercase SHA-256 digest"
        )));
    }
    Ok(())
}

pub(super) fn validate_discovery_evidence(evidence: &DiscoveryEvidenceRecord) -> CoreResult<()> {
    validate_identifier("discovery evidence id", evidence.id.as_str(), 256)?;
    validate_identifier(
        "discovery evidence session id",
        evidence.session_id.as_str(),
        128,
    )?;
    validate_sha256("discovery evidence content hash", &evidence.content_sha256)?;
    validate_persistable_discovery_url(
        evidence.source_url.as_str(),
        "discovery evidence source URL",
    )?;
    if !evidence.extracted_json.is_object() {
        return Err(CoreError::invalid(
            "discovery evidence extraction must be a JSON object",
        ));
    }
    encode_redacted_json(&evidence.extracted_json, "discovery evidence")?;
    Ok(())
}

pub(super) fn validate_sanitized_input(input: &SanitizedDiscoveryInput) -> CoreResult<()> {
    if looks_like_secret(input.connection_id.as_str()) || looks_like_secret(&input.display_name) {
        return Err(CoreError::invalid(
            "discovery connection identity contains credential-like material",
        ));
    }
    validate_persistable_discovery_url(input.site_url.as_str(), "discovery site URL")?;
    if let Some(docs_url) = &input.docs_url {
        validate_persistable_discovery_url(docs_url.as_str(), "discovery docs URL")?;
    }
    if let Some(reference) = &input.credential_ref {
        validate_opaque_credential_reference(reference.as_str())?;
    }
    Ok(())
}

pub(super) fn validate_persistable_discovery_url(value: &str, label: &str) -> CoreResult<()> {
    let parsed =
        url::Url::parse(value).map_err(|_| CoreError::invalid(format!("{label} is invalid")))?;
    if parsed.query().is_some()
        || parsed.fragment().is_some()
        || !parsed.username().is_empty()
        || parsed.password().is_some()
    {
        return Err(CoreError::invalid(format!(
            "{label} must not contain user information, a query, or a fragment"
        )));
    }
    if parsed.host_str().is_some_and(looks_like_secret) {
        return Err(CoreError::invalid(format!(
            "{label} contains credential-like host material"
        )));
    }
    for segment in parsed.path().split('/') {
        let mut decoded = segment.to_owned();
        for _ in 0..4 {
            if !decoded.as_bytes().contains(&b'%') {
                break;
            }
            let next = percent_decode_path_segment(&decoded).ok_or_else(|| {
                CoreError::invalid(format!("{label} contains invalid path encoding"))
            })?;
            if next == decoded {
                break;
            }
            decoded = next;
        }
        if decoded.as_bytes().contains(&b'%') {
            return Err(CoreError::invalid(format!(
                "{label} contains excessively nested path encoding"
            )));
        }
        if looks_like_secret(&decoded) {
            return Err(CoreError::invalid(format!(
                "{label} contains credential-like path material"
            )));
        }
    }
    Ok(())
}

pub(super) fn percent_decode_path_segment(segment: &str) -> Option<String> {
    let bytes = segment.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut index = 0_usize;
    while index < bytes.len() {
        if bytes[index] == b'%' {
            let high = *bytes.get(index + 1)?;
            let low = *bytes.get(index + 2)?;
            decoded.push(hex_nibble(high)?.checked_mul(16)? + hex_nibble(low)?);
            index += 3;
        } else {
            decoded.push(bytes[index]);
            index += 1;
        }
    }
    String::from_utf8(decoded).ok()
}

pub(super) const fn hex_nibble(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

pub(super) fn validate_opaque_credential_reference(reference: &str) -> CoreResult<()> {
    let lower = reference.to_ascii_lowercase();
    if reference.is_empty()
        || reference.len() > 256
        || reference.trim() != reference
        || reference.chars().any(char::is_control)
        || reference.contains("://")
        || reference.contains('?')
        || reference.contains('#')
        || reference.contains('=')
        || lower.contains("secret")
        || lower.contains("password")
        || lower.contains("api-key")
        || lower.contains("apikey")
        || lower.contains("token")
        || looks_like_secret(reference)
    {
        return Err(CoreError::invalid(
            "discovery credential_ref must be an opaque broker reference, not credential material",
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

pub(super) fn validate_redacted_value(value: &Value) -> CoreResult<()> {
    let mut nodes = 0_usize;
    validate_redacted_value_inner(value, 0, &mut nodes)
}

pub(super) fn validate_redacted_value_inner(
    value: &Value,
    depth: usize,
    nodes: &mut usize,
) -> CoreResult<()> {
    *nodes = nodes.saturating_add(1);
    if depth > MAX_DISCOVERY_JSON_DEPTH || *nodes > MAX_DISCOVERY_JSON_NODES {
        return Err(CoreError::invalid(
            "redacted discovery JSON exceeds structural bounds",
        ));
    }
    match value {
        Value::Object(object) => {
            for (key, child) in object {
                let normalized = key
                    .bytes()
                    .filter(u8::is_ascii_alphanumeric)
                    .map(|byte| byte.to_ascii_lowercase())
                    .collect::<Vec<_>>();
                if matches!(
                    normalized.as_slice(),
                    b"apikey"
                        | b"apikeyvalue"
                        | b"authorization"
                        | b"authorizationvalue"
                        | b"proxyauthorization"
                        | b"cookie"
                        | b"setcookie"
                        | b"password"
                        | b"secret"
                        | b"clientsecret"
                        | b"clientsecretvalue"
                        | b"token"
                        | b"bearertoken"
                        | b"idtoken"
                        | b"sessiontoken"
                        | b"credential"
                        | b"credentials"
                        | b"accesstoken"
                        | b"refreshtoken"
                        | b"credentialvalue"
                        | b"rawcredential"
                        | b"requestheaders"
                        | b"responseheaders"
                        | b"headers"
                        | b"documentbody"
                        | b"rawdocument"
                        | b"rawbody"
                        | b"rawrequest"
                        | b"rawresponse"
                        | b"rawcurl"
                        | b"pastedcurl"
                ) {
                    return Err(CoreError::invalid(
                        "redacted discovery JSON contains a forbidden sensitive field",
                    ));
                }
                if normalized.as_slice() == b"sourceurl"
                    && let Some(source_url) = child.as_str()
                {
                    validate_persistable_discovery_url(source_url, "source_url")?;
                }
                validate_redacted_value_inner(child, depth + 1, nodes)?;
            }
        }
        Value::Array(values) => {
            for child in values {
                validate_redacted_value_inner(child, depth + 1, nodes)?;
            }
        }
        Value::String(value) => {
            if value.chars().count() > MAX_DISCOVERY_JSON_CHARS {
                return Err(CoreError::invalid(
                    "redacted discovery JSON contains an oversized string",
                ));
            }
            if looks_like_secret(value) {
                return Err(CoreError::invalid(
                    "redacted discovery JSON contains credential-like material",
                ));
            }
            if value.contains("://")
                && let Ok(url) = url::Url::parse(value)
                && (!url.username().is_empty() || url.password().is_some())
            {
                return Err(CoreError::invalid(
                    "redacted discovery JSON contains URL user information",
                ));
            }
        }
        _ => {}
    }
    Ok(())
}

pub(super) fn looks_like_secret(value: &str) -> bool {
    const SECRET_PREFIXES: [&str; 10] = [
        "sk-proj-",
        "sk-ant-",
        "sk-or-",
        "sk-",
        "AIza",
        "xoxb-",
        "xoxp-",
        "ghp_",
        "github_pat_",
        "AKIA",
    ];
    let trimmed = value.trim();
    let lower = trimmed.to_ascii_lowercase();
    if lower.starts_with("bearer ")
        || lower.starts_with("basic ")
        || lower.contains("api_key=")
        || lower.contains("apikey=")
        || lower.contains("access_token=")
        || lower.contains("secret=")
        || lower.contains("password=")
        || lower.contains("-----begin private key-----")
        || lower.contains("-----begin rsa private key-----")
        || lower.contains("sk-proj-")
        || lower.contains("sk-ant-")
        || lower.contains("sk-or-")
        || lower.contains("github_pat_")
    {
        return true;
    }
    if SECRET_PREFIXES
        .iter()
        .any(|prefix| trimmed.starts_with(prefix))
    {
        return true;
    }
    let jwt_parts = trimmed.split('.').collect::<Vec<_>>();
    jwt_parts.len() == 3
        && jwt_parts[0].starts_with("eyJ")
        && jwt_parts[1].starts_with("eyJ")
        && jwt_parts.iter().all(|part| {
            part.len() >= 8
                && part
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
        })
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

pub(super) fn validate_candidate_evidence_references(
    transaction: &Transaction<'_>,
    candidate: &StoredDiscoveryCandidate,
) -> CoreResult<()> {
    let mut unique = BTreeSet::new();
    for evidence_id in &candidate.candidate.evidence_ids {
        if !unique.insert(evidence_id.as_str()) {
            return Err(CoreError::invalid(
                "discovery candidate evidence references must be unique",
            ));
        }
        let belongs = transaction
            .query_row(
                "SELECT EXISTS(
                     SELECT 1
                     FROM provider_discovery_evidence
                     WHERE id = ?1 AND session_id = ?2
                 )",
                params![
                    evidence_id.as_str(),
                    candidate.candidate.session_id.as_str()
                ],
                |row| row.get::<_, bool>(0),
            )
            .map_err(database_error)?;
        if !belongs {
            return Err(CoreError::invalid(
                "candidate evidence must exist in the same discovery session",
            ));
        }
    }
    Ok(())
}

pub(super) fn validate_session_evidence_ids<'a>(
    connection: &Connection,
    session_id: &DiscoverySessionId,
    evidence_ids: impl IntoIterator<Item = &'a EvidenceId>,
    label: &str,
) -> CoreResult<()> {
    let mut unique = BTreeSet::new();
    for evidence_id in evidence_ids {
        if !unique.insert(evidence_id.as_str()) {
            return Err(CoreError::invalid(format!(
                "{label} evidence references must be unique"
            )));
        }
        let belongs = connection
            .query_row(
                "SELECT EXISTS(
                     SELECT 1
                     FROM provider_discovery_evidence
                     WHERE id = ?1 AND session_id = ?2
                 )",
                params![evidence_id.as_str(), session_id.as_str()],
                |row| row.get::<_, bool>(0),
            )
            .map_err(database_error)?;
        if !belongs {
            return Err(CoreError::invalid(format!(
                "{label} evidence must exist in the same discovery session"
            )));
        }
    }
    Ok(())
}

pub(super) fn validate_review_evidence_references(
    connection: &Connection,
    session_id: &DiscoverySessionId,
    review: &DiscoveryReviewDiff,
) -> CoreResult<()> {
    for change in &review.changes {
        validate_session_evidence_ids(
            connection,
            session_id,
            &change.evidence_ids,
            "discovery review change",
        )?;
    }
    Ok(())
}

pub(super) fn validate_approval_references(
    transaction: &Transaction<'_>,
    approval: &DiscoveryApprovalRecord,
) -> CoreResult<()> {
    match &approval.grant {
        DiscoveryApprovalGrant::AssistantConsent {
            assistant_route_id,
            evidence_ids,
            ..
        } => {
            validate_session_evidence_ids(
                transaction,
                &approval.session_id,
                evidence_ids,
                "assistant consent",
            )?;
            let route_exists = transaction
                .query_row(
                    "SELECT EXISTS(SELECT 1 FROM provider_models WHERE id = ?1)",
                    [assistant_route_id.as_str()],
                    |row| row.get::<_, bool>(0),
                )
                .map_err(database_error)?;
            if !route_exists {
                return Err(CoreError::invalid(
                    "assistant consent route must exist before approval",
                ));
            }
        }
        DiscoveryApprovalGrant::CapabilityProbe {
            model_route_ids,
            budget,
        } => {
            let state = transaction
                .query_row(
                    "SELECT state FROM provider_discovery_sessions WHERE id = ?1",
                    [approval.session_id.as_str()],
                    |row| row.get::<_, String>(0),
                )
                .map_err(database_error)?;
            if state != "awaiting_probe_consent" {
                return Err(CoreError::invalid(
                    "capability probe approval requires the consent state",
                ));
            }
            validate_capability_probe_grant(
                transaction,
                &approval.session_id,
                model_route_ids,
                *budget,
            )?;
        }
        _ => {}
    }
    Ok(())
}

pub(super) fn validate_capability_probe_grant(
    connection: &Connection,
    session_id: &DiscoverySessionId,
    model_route_ids: &[lorepia_domain::ModelRouteId],
    budget: DiscoveryProbeBudget,
) -> CoreResult<()> {
    let draft_json = connection
        .query_row(
            "SELECT draft_json
             FROM provider_discovery_sessions
             WHERE id = ?1",
            [session_id.as_str()],
            |row| row.get::<_, Option<String>>(0),
        )
        .optional()
        .map_err(database_error)?
        .flatten()
        .ok_or_else(|| CoreError::invalid("capability probe proposal has no durable draft"))?;
    let draft = decode_redacted_json(&draft_json, "stored discovery draft")?;
    let probe_routes = draft
        .get("probe_route_ids")
        .and_then(Value::as_array)
        .ok_or_else(|| CoreError::invalid("durable probe route proposal is missing"))?;
    let mut expected = probe_routes
        .iter()
        .map(|value| {
            value
                .as_str()
                .map(str::to_owned)
                .ok_or_else(|| CoreError::invalid("durable probe route identifier is invalid"))
        })
        .collect::<CoreResult<Vec<_>>>()?;
    expected.sort();
    expected.dedup();
    if expected.is_empty() {
        return Err(CoreError::invalid(
            "durable probe route proposal must not be empty",
        ));
    }
    let graph_route_ids = draft
        .get("routes")
        .and_then(Value::as_array)
        .ok_or_else(|| CoreError::invalid("durable discovery graph routes are missing"))?
        .iter()
        .map(|route| {
            route
                .get("id")
                .and_then(Value::as_str)
                .ok_or_else(|| CoreError::invalid("durable discovery route is invalid"))
        })
        .collect::<CoreResult<BTreeSet<_>>>()?;
    if expected
        .iter()
        .any(|route_id| !graph_route_ids.contains(route_id.as_str()))
    {
        return Err(CoreError::invalid(
            "durable probe proposal references a route outside its graph",
        ));
    }
    let actual = model_route_ids
        .iter()
        .map(|route_id| route_id.as_str().to_owned())
        .collect::<Vec<_>>();
    let expected_budget =
        DiscoveryProbeBudget::standard_for_route_count(expected.len()).map_err(contract_error)?;
    if actual != expected || budget != expected_budget {
        return Err(CoreError::invalid(
            "capability probe approval differs from its durable proposal",
        ));
    }
    Ok(())
}

pub(super) fn current_session_revision(
    connection: &Connection,
    session_id: &str,
) -> CoreResult<u64> {
    connection
        .query_row(
            "SELECT revision FROM provider_discovery_sessions WHERE id = ?1",
            [session_id],
            |row| row.get(0),
        )
        .optional()
        .map_err(database_error)?
        .ok_or_else(|| {
            CoreError::new(
                CoreErrorCode::NotFound,
                "provider discovery session was not found",
                false,
            )
        })
}

pub(super) fn require_session(connection: &Connection, session_id: &str) -> CoreResult<()> {
    current_session_revision(connection, session_id).map(|_| ())
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

pub(super) fn json_enum_wire(value: Value, label: &str) -> CoreResult<String> {
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

pub(super) fn canonical_typed_value(value: Value, label: &str) -> CoreResult<String> {
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

pub(super) fn write_canonical_json(value: &Value, output: &mut String) -> CoreResult<()> {
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
