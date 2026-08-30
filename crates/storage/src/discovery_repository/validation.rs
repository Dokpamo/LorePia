//! Discovery repository validation boundaries.

use std::collections::BTreeSet;

use lorepia_domain::{
    CoreError, CoreErrorCode, CoreResult, DiscoverySessionId, EvidenceId, ProviderConnectionId,
    discovery::{
        DiscoveryApprovalGrant, DiscoveryApprovalRecord, DiscoveryEffect, DiscoveryProbeBudget,
        DiscoveryReviewDiff, DiscoveryState, PROVIDER_DISCOVERY_EVENT_VERSION,
        ProviderDiscoverySession, SanitizedDiscoveryInput,
    },
};
use rusqlite::{Connection, OptionalExtension, Transaction, params};
use serde_json::Value;

use crate::discovery::DurableOperationOutcome;

use super::{
    contract_codec::{
        decode_redacted_json, encode_redacted_json, validate_initial_discovery_draft,
    },
    errors::{contract_error, database_error},
    map_discovery_effect,
    semantic_view::StoredDiscoveryCandidate,
    types::{DiscoveryEvidenceRecord, DiscoveryJsonUpdate, DiscoveryTransitionWrite},
    validate_prepared_commit, validate_provider_graph,
};

const MAX_DISCOVERY_ROWS: u32 = 1_000;
pub(super) const MAX_DISCOVERY_JSON_BYTES: usize = 1024 * 1024;
pub(super) const MAX_DISCOVERY_JSON_CHARS: usize = 512 * 1024;
const MAX_DISCOVERY_JSON_DEPTH: usize = 24;
const MAX_DISCOVERY_JSON_NODES: usize = 16_384;

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

fn percent_decode_path_segment(segment: &str) -> Option<String> {
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

const fn hex_nibble(byte: u8) -> Option<u8> {
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

pub(super) fn validate_redacted_value(value: &Value) -> CoreResult<()> {
    let mut nodes = 0_usize;
    validate_redacted_value_inner(value, 0, &mut nodes)
}

fn validate_redacted_value_inner(value: &Value, depth: usize, nodes: &mut usize) -> CoreResult<()> {
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

fn current_session_revision(connection: &Connection, session_id: &str) -> CoreResult<u64> {
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

pub(super) fn is_pristine_discovery_session(session: &ProviderDiscoverySession) -> bool {
    session.state == DiscoveryState::Draft
        && session.revision == 0
        && session.next_event_sequence == 1
        && session.recovery.is_none()
        && session.unknown_operation.is_none()
        && session.manifest_sha256.is_none()
        && session.commit_plan_sha256.is_none()
        && session.commit_attempt_id.is_none()
        && session.committed_connection_id.is_none()
        && !session.cancellation_pending
        && session.active_effect_approval.is_none()
        && session.failure.is_none()
}

pub(super) fn validate_atomic_discovery_begin(
    initial_session: &ProviderDiscoverySession,
    write: &DiscoveryTransitionWrite,
) -> CoreResult<()> {
    initial_session.validate().map_err(contract_error)?;
    validate_sanitized_input(&initial_session.input)?;
    if !is_pristine_discovery_session(initial_session)
        || write.transition.previous_revision != 0
        || write.transition.session.id != initial_session.id
        || write.transition.receipt.action_kind != "begin"
    {
        return Err(CoreError::invalid(
            "atomic discovery begin requires a pristine matching draft and Begin transition",
        ));
    }
    validate_identifier("discovery session id", initial_session.id.as_str(), 128)?;
    let begun = &write.transition.session;
    if begun.input != initial_session.input
        || begun.state != DiscoveryState::ResolvingKnownProvider
        || begun.revision != 1
        || begun.next_event_sequence != 2
        || begun.recovery.is_some()
        || begun.unknown_operation.is_some()
        || begun.manifest_sha256.is_some()
        || begun.commit_plan_sha256.is_some()
        || begun.commit_attempt_id.is_some()
        || begun.committed_connection_id.is_some()
        || begun.cancellation_pending
        || begun.active_effect_approval.is_some()
        || begun.failure.is_some()
        || write.transition.effect != DiscoveryEffect::ResolveKnownProvider
        || write.transition.event.progress.is_some()
        || write.transition.event.action_required.is_some()
        || write.transition.event.warning.is_some()
        || write.transition.event.failure.is_some()
    {
        return Err(CoreError::invalid(
            "atomic discovery begin transition contains non-begin state",
        ));
    }
    if !write.new_evidence.is_empty()
        || !write.new_candidates.is_empty()
        || write.approval.is_some()
        || write.prepared_commit.is_some()
        || write.provider_graph.is_some()
        || write.completed_operation.is_some()
        || matches!(write.draft, DiscoveryJsonUpdate::Clear)
        || matches!(write.review, DiscoveryJsonUpdate::Replace(_))
    {
        return Err(CoreError::invalid(
            "atomic discovery begin cannot publish later-stage artifacts",
        ));
    }
    if let DiscoveryJsonUpdate::Replace(draft) = &write.draft {
        validate_initial_discovery_draft(draft, &initial_session.input)?;
    }
    validate_transition_write(write)
}

pub(super) fn ensure_provider_credential_operation_settled_for_discovery(
    connection: &Connection,
    connection_id: &ProviderConnectionId,
) -> CoreResult<()> {
    let unresolved_exists = connection
        .query_row(
            "SELECT EXISTS(
                SELECT 1 FROM provider_credential_operations
                WHERE connection_id = ?1
                  AND status IN (
                    'prepared', 'started', 'cleanup_required', 'outcome_unknown'
                  )
             )",
            [connection_id.as_str()],
            |row| row.get::<_, bool>(0),
        )
        .map_err(database_error)?;
    if unresolved_exists {
        return Err(CoreError::new(
            CoreErrorCode::InvalidInput,
            "provider connection cannot begin discovery while its credential operation is unresolved",
            true,
        ));
    }
    Ok(())
}

pub(super) fn validate_transition_write(write: &DiscoveryTransitionWrite) -> CoreResult<()> {
    let transition = &write.transition;
    transition.session.validate().map_err(contract_error)?;
    if transition.event.version != PROVIDER_DISCOVERY_EVENT_VERSION
        || transition.receipt.outcome
            != lorepia_domain::discovery::DiscoveryActionReceiptOutcome::Applied
        || transition.session.revision != transition.previous_revision.saturating_add(1)
        || transition.event.session_id != transition.session.id
        || transition.event.session_revision != transition.session.revision
        || transition.event.state != transition.session.state
        || transition.event.failure != transition.session.failure
        || transition.event.sequence.saturating_add(1) != transition.session.next_event_sequence
        || transition.event.action_id != transition.receipt.action_id
        || transition.receipt.session_id != transition.session.id
        || transition.receipt.expected_revision != transition.previous_revision
        || transition.receipt.resulting_revision != transition.session.revision
        || transition.receipt.event_sequence != transition.event.sequence
    {
        return Err(CoreError::invalid(
            "discovery transition aggregate fields do not agree",
        ));
    }
    let (_, operation_kind, _) = map_discovery_effect(&transition.effect);
    if operation_kind.is_some() != write.new_operation_id.is_some() {
        return Err(CoreError::invalid(
            "discovery external effects require exactly one prepared operation id",
        ));
    }
    if let Some(approval) = &write.approval {
        approval.validate().map_err(contract_error)?;
        if approval.session_id != transition.session.id
            || approval.session_revision != transition.previous_revision
            || approval.created_at != write.occurred_at
        {
            return Err(CoreError::invalid(
                "discovery approval must match the transition session, revision, and time",
            ));
        }
    }
    validate_prepared_commit(write)?;
    validate_provider_graph_publication(write)?;
    for evidence in &write.new_evidence {
        if evidence.session_id != transition.session.id {
            return Err(CoreError::invalid(
                "transition evidence must belong to the discovery session",
            ));
        }
        validate_discovery_evidence(evidence)?;
    }
    for candidate in &write.new_candidates {
        if candidate.candidate.session_id != transition.session.id {
            return Err(CoreError::invalid(
                "transition candidates must belong to the discovery session",
            ));
        }
        candidate.candidate.validate().map_err(contract_error)?;
    }
    Ok(())
}

fn validate_provider_graph_publication(write: &DiscoveryTransitionWrite) -> CoreResult<()> {
    let transition = &write.transition;
    let Some(graph) = &write.provider_graph else {
        if transition.receipt.action_kind == "commit_succeeded" {
            return Err(CoreError::invalid(
                "a successful discovery commit must publish its exact provider graph atomically",
            ));
        }
        return Ok(());
    };
    validate_provider_graph(graph)?;
    if transition.receipt.action_kind != "commit_succeeded"
        || transition.session.state != DiscoveryState::Ready
        || transition.effect != DiscoveryEffect::None
        || write.new_operation_id.is_some()
        || write.prepared_commit.is_some()
        || write.approval.is_some()
        || !write.new_evidence.is_empty()
        || !write.new_candidates.is_empty()
        || write
            .completed_operation
            .as_ref()
            .is_none_or(|completed| completed.outcome != DurableOperationOutcome::Succeeded)
        || transition.session.commit_attempt_id.as_ref() != Some(&graph.plan.attempt_id)
        || transition.session.commit_plan_sha256.as_deref() != Some(graph.plan_sha256.as_str())
        || transition.session.committed_connection_id.as_ref() != Some(&graph.plan.connection_id)
        || graph.plan.session_id != transition.session.id
        || graph.plan.expected_revision >= transition.previous_revision
    {
        return Err(CoreError::invalid(
            "provider graph publication must be the exact atomic Ready transition",
        ));
    }
    Ok(())
}
