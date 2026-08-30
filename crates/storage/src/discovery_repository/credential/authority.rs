//! Secret-free credential ownership projection and authority validation.

use super::history::{
    DiscoveryAuthorityReceiptRecord, load_discovery_authority_operation_for_start,
    load_discovery_authority_receipt_by_action, load_discovery_authority_receipt_by_revision,
    load_restart_discovery_authority_receipt, validate_atomic_commit_start_receipt,
    validate_discovery_operation_interrupted_audit, validate_discovery_operation_start_audit,
    validate_discovery_operation_terminal_audit_order,
    validate_discovery_operation_terminal_audit_order_for_receipt,
    validate_discovery_receipt_follows, validate_interrupted_discovery_authority_receipt,
    validate_interrupted_discovery_operation_evidence, validate_ready_discovery_authority_receipt,
    validate_unknown_discovery_credential_receipt,
};
use crate::discovery_repository::{
    BTreeSet, Connection, CoreError, CoreErrorCode, CoreResult, CredentialRef, DateTime,
    DiscoveredProviderGraph, DiscoveryActionId, DiscoveryCommitAttemptRecord, DiscoveryCommitPhase,
    DiscoveryOperationId, DiscoveryOperationKind, DiscoveryOperationRecord,
    DiscoveryOperationStatus, DiscoveryReviewDiff, DiscoverySessionId, DiscoverySessionSnapshot,
    DiscoverySideEffectClass, DiscoveryState, DiscoveryTransition,
    DiscoveryUnknownOutcomeResolution, OptionalExtension, ProviderConnectionId, Transaction, Utc,
    canonical_json_result, corrupted, database_error, decode_evidence_row, decode_redacted_json,
    encode_redacted_json, graph_ownership_audit_hash, graph_template_was_created,
    load_commit_attempt, load_discovered_provider_graph_rows,
    load_discovery_native_credential_execution, load_operation_by_id, load_session_snapshot,
    params, parse_timestamp, sha256_hex, validate_credential_approval,
    validate_discovery_authority_approval_rows, validate_discovery_authority_graph_audits,
    validate_discovery_native_physical_authority_id, validate_discovery_unknown_outcome_resolution,
    validate_graph_component, validate_review_approval, validate_sha256,
};

pub(in crate::discovery_repository) fn project_reconciled_discovery_credential_ownership(
    transaction: &Transaction<'_>,
    transition: &DiscoveryTransition,
    occurred_at: DateTime<Utc>,
) -> CoreResult<()> {
    if transition.receipt.action_kind != "resolve_unknown_outcome" {
        return Ok(());
    }
    let attempt_id = transition
        .session
        .commit_attempt_id
        .as_ref()
        .ok_or_else(|| corrupted("reconciled credential commit has no attempt"))?;
    let attempt = load_commit_attempt(transaction, attempt_id)?;
    if attempt.plan.credential_ref.is_none() {
        return Ok(());
    }
    let snapshot = load_session_snapshot(transaction, transition.session.id.as_str())?
        .ok_or_else(|| corrupted("reconciled credential session disappeared"))?;
    if snapshot.session != transition.session || snapshot.active_operation_id.is_some() {
        return Err(corrupted(
            "reconciled credential session differs from its ready transition",
        ));
    }
    let authority_operation_id =
        validate_discovery_credential_completion_evidence(transaction, &attempt, &snapshot)?;
    let connection_binding_sha256 =
        crate::provider_credential_repository::provider_credential_connection_binding_sha256(
            transaction,
            &attempt.plan.connection_id,
        )?;
    let authority_execution =
        load_discovery_native_credential_execution(transaction, &authority_operation_id)?
            .ok_or_else(|| {
                corrupted("reconciled credential commit has no physical execution authority")
            })?;
    validate_discovery_credential_ownership_authority_inner(
        transaction,
        &attempt.plan.connection_id,
        &authority_execution.physical_authority_id,
        authority_operation_id.as_str(),
        &connection_binding_sha256,
        DiscoveryCredentialBindingAuthority::Active,
    )?;
    let authority_sequence = insert_discovery_credential_ownership_event(
        transaction,
        &attempt.plan.connection_id,
        &connection_binding_sha256,
        &authority_execution.physical_authority_id,
        &authority_operation_id,
        occurred_at,
    )?;
    let changed = transaction
        .execute(
            "UPDATE provider_credential_ownership
             SET ownership_state = 'discovery_owned',
                 connection_binding_sha256 = ?2,
                 authority_id = ?3,
                 authority_sequence = ?4,
                 updated_at = ?5
             WHERE connection_id = ?1 AND credential_ref = ?1",
            params![
                attempt.plan.connection_id.as_str(),
                connection_binding_sha256,
                authority_execution.physical_authority_id,
                authority_sequence,
                occurred_at.to_rfc3339(),
            ],
        )
        .map_err(database_error)?;
    if changed != 1 {
        return Err(corrupted(
            "reconciled discovery credential lost its ownership projection",
        ));
    }
    Ok(())
}

pub(in crate::discovery_repository) fn insert_discovery_credential_ownership_event(
    transaction: &Transaction<'_>,
    connection_id: &ProviderConnectionId,
    connection_binding_sha256: &str,
    physical_authority_id: &str,
    source_operation_id: &DiscoveryOperationId,
    created_at: DateTime<Utc>,
) -> CoreResult<u64> {
    validate_discovery_native_physical_authority_id(physical_authority_id)?;
    validate_sha256(
        "discovery ownership connection binding",
        connection_binding_sha256,
    )?;
    let authority_sequence = transaction
        .query_row(
            "SELECT COALESCE(MAX(authority_sequence), 0) + 1
             FROM provider_credential_ownership_events
             WHERE connection_id = ?1",
            [connection_id.as_str()],
            |row| row.get::<_, u64>(0),
        )
        .map_err(database_error)?;
    transaction
        .execute(
            "INSERT INTO provider_credential_ownership_events (
                 connection_id, authority_sequence, ownership_state,
                 connection_binding_sha256, authority_id, source_kind,
                 source_id, created_at
             ) VALUES (?1, ?2, 'discovery_owned', ?3, ?4,
                       'discovery_commit', ?5, ?6)",
            params![
                connection_id.as_str(),
                authority_sequence,
                connection_binding_sha256,
                physical_authority_id,
                source_operation_id.as_str(),
                created_at.to_rfc3339(),
            ],
        )
        .map_err(database_error)?;
    Ok(authority_sequence)
}

/// Revalidates the complete durable authority behind an active
/// discovery-owned credential projection. `physical_authority_id` is the
/// exact native execution while `source_operation_id` is its immutable
/// semantic atomic-commit source. Archived bindings are rejected here.
pub(crate) fn validate_discovery_credential_ownership_authority(
    connection: &Connection,
    connection_id: &ProviderConnectionId,
    physical_authority_id: &str,
    source_operation_id: &str,
    expected_binding_sha256: &str,
) -> CoreResult<()> {
    validate_discovery_credential_ownership_authority_inner(
        connection,
        connection_id,
        physical_authority_id,
        source_operation_id,
        expected_binding_sha256,
        DiscoveryCredentialBindingAuthority::Active,
    )
    .map_err(normalize_discovery_credential_authority_error)
}

/// Revalidates a superseded discovery-owned physical slot after its provider
/// connection was archived. This is intentionally separate from current
/// access admission: it exists only so slot-GC can delete an exact historical
/// authority-derived native slot without reopening archived credentials for
/// product use.
pub(crate) fn validate_archived_discovery_credential_ownership_authority_for_slot_gc(
    connection: &Connection,
    connection_id: &ProviderConnectionId,
    physical_authority_id: &str,
    source_operation_id: &str,
    expected_binding_sha256: &str,
) -> CoreResult<()> {
    validate_discovery_credential_ownership_authority_inner(
        connection,
        connection_id,
        physical_authority_id,
        source_operation_id,
        expected_binding_sha256,
        DiscoveryCredentialBindingAuthority::ArchivedSlotGarbage,
    )
    .map_err(normalize_discovery_credential_authority_error)
}

fn normalize_discovery_credential_authority_error(error: CoreError) -> CoreError {
    match error.code {
        CoreErrorCode::StorageUnavailable | CoreErrorCode::StorageCorrupted => error,
        _ => corrupted(format!(
            "discovery credential ownership authority is inconsistent: {}",
            error.message
        )),
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DiscoveryCredentialBindingAuthority {
    Active,
    ArchivedSlotGarbage,
}

#[allow(clippy::too_many_lines)]
fn validate_discovery_credential_ownership_authority_inner(
    connection: &Connection,
    connection_id: &ProviderConnectionId,
    physical_authority_id: &str,
    source_operation_id: &str,
    expected_binding_sha256: &str,
    binding_authority: DiscoveryCredentialBindingAuthority,
) -> CoreResult<()> {
    validate_sha256(
        "discovery credential ownership binding",
        expected_binding_sha256,
    )
    .map_err(|_| corrupted("discovery credential ownership binding is invalid"))?;
    validate_discovery_native_physical_authority_id(physical_authority_id)
        .map_err(|_| corrupted("discovery credential physical authority id is invalid"))?;
    let authority_operation_id = DiscoveryOperationId::parse(source_operation_id)
        .map_err(|_| corrupted("discovery credential ownership operation id is invalid"))?;
    let authority_operation = load_operation_by_id(connection, &authority_operation_id)?;
    let authority_execution =
        load_discovery_native_credential_execution(connection, &authority_operation_id)?
            .ok_or_else(|| {
                corrupted("discovery credential ownership execution authority is missing")
            })?;
    if authority_operation.kind != DiscoveryOperationKind::AtomicCommit
        || authority_operation.side_effect_class != DiscoverySideEffectClass::Persistent
        || !matches!(
            authority_operation.status,
            DiscoveryOperationStatus::Succeeded | DiscoveryOperationStatus::OutcomeUnknown
        )
        || authority_operation.started_at.is_none()
        || authority_operation.finished_at.is_none()
        || authority_execution.physical_authority_id != physical_authority_id
        || authority_execution.operation_id != authority_operation_id
        || authority_execution.connection_id != *connection_id
        || authority_execution.connection_binding_sha256 != expected_binding_sha256
        || authority_execution.store_started_at != authority_operation.started_at
    {
        return Err(corrupted(
            "discovery credential ownership operation is not an exact completed native commit",
        ));
    }

    let snapshot = load_session_snapshot(connection, authority_operation.session_id.as_str())?
        .ok_or_else(|| corrupted("discovery credential ownership session is missing"))?;
    let attempt_id =
        snapshot.session.commit_attempt_id.as_ref().ok_or_else(|| {
            corrupted("discovery credential ownership session has no commit attempt")
        })?;
    let attempt = load_commit_attempt(connection, attempt_id).map_err(|error| {
        if error.code == CoreErrorCode::NotFound {
            corrupted("discovery credential ownership attempt is missing")
        } else {
            error
        }
    })?;
    let attempt_completed_at = attempt.completed_at.ok_or_else(|| {
        corrupted("discovery credential ownership attempt has no completion time")
    })?;
    let operation_finished_at = authority_operation
        .finished_at
        .ok_or_else(|| corrupted("discovery credential ownership operation has no finish time"))?;
    let terminal_chronology_matches = match authority_operation.status {
        DiscoveryOperationStatus::Succeeded => operation_finished_at == attempt_completed_at,
        DiscoveryOperationStatus::OutcomeUnknown => operation_finished_at <= attempt_completed_at,
        _ => false,
    };
    if attempt.phase != DiscoveryCommitPhase::Completed
        || attempt.plan.connection_id != *connection_id
        || attempt
            .plan
            .credential_ref
            .as_ref()
            .map(CredentialRef::as_str)
            != Some(connection_id.as_str())
        || attempt.plan.credential_approval_id.is_none()
        || authority_operation.session_id != attempt.session_id
        || !terminal_chronology_matches
    {
        return Err(corrupted(
            "discovery credential ownership attempt is not an exact completed credential commit",
        ));
    }

    if snapshot.session.state != DiscoveryState::Ready
        || snapshot.session.commit_attempt_id.as_ref() != Some(&attempt.id)
        || snapshot.session.commit_plan_sha256.as_deref() != Some(attempt.plan_sha256.as_str())
        || snapshot.session.committed_connection_id.as_ref() != Some(connection_id)
        || snapshot.session.manifest_sha256.as_deref()
            != Some(attempt.plan.manifest_sha256.as_str())
        || snapshot.active_operation_id.is_some()
        || snapshot.session.input.connection_id != *connection_id
        || snapshot
            .session
            .input
            .credential_ref
            .as_ref()
            .map(CredentialRef::as_str)
            != Some(connection_id.as_str())
        || snapshot.session.revision <= attempt.expected_revision
    {
        return Err(corrupted(
            "discovery credential ownership session is detached from its completed commit",
        ));
    }

    let graph_rows = load_discovered_provider_graph_rows(
        connection,
        &attempt.plan.template_id,
        attempt.plan.template_version,
        connection_id,
    )?
    .ok_or_else(|| corrupted("discovery credential ownership graph is missing"))?;
    let current_manifest_json = canonical_json_result(
        serde_json::to_value(&graph_rows.template.default_manifest),
        "discovery credential ownership provider manifest",
    )
    .map_err(|_| corrupted("discovery credential ownership manifest is invalid"))?;
    if graph_rows.template.id != attempt.plan.template_id
        || graph_rows.template.manifest_version != attempt.plan.template_version
        || graph_rows.connection.id != *connection_id
        || graph_rows.connection.template_id != attempt.plan.template_id
        || graph_rows.connection.template_version != attempt.plan.template_version
        || graph_rows.connection.credential_ref != attempt.plan.credential_ref
        || sha256_hex(current_manifest_json.as_bytes()) != attempt.plan.manifest_sha256
    {
        return Err(corrupted(
            "discovery credential ownership connection differs from its immutable manifest identity",
        ));
    }
    let graph = DiscoveredProviderGraph {
        plan: attempt.plan.clone(),
        plan_sha256: attempt.plan_sha256.clone(),
        template: graph_rows.template,
        connection: graph_rows.connection,
        routes: graph_rows.routes,
        observations: graph_rows.observations,
        presets: graph_rows.presets,
    };
    validate_graph_component(
        serde_json::to_value(&graph.template),
        "discovery credential ownership provider template",
    )
    .map_err(|_| corrupted("discovery credential ownership template is invalid"))?;
    validate_graph_component(
        serde_json::to_value(&graph.connection),
        "discovery credential ownership provider connection",
    )
    .map_err(|_| corrupted("discovery credential ownership connection is invalid"))?;
    validate_review_approval(connection, &attempt.plan)
        .map_err(|_| corrupted("discovery credential ownership review is invalid"))?;
    validate_credential_approval(connection, &graph)
        .map_err(|_| corrupted("discovery credential ownership approval is invalid"))?;
    validate_discovery_authority_approval_rows(connection, &attempt)?;
    validate_discovery_authority_evidence_rows(
        connection,
        &attempt.session_id,
        snapshot
            .review
            .as_ref()
            .ok_or_else(|| corrupted("discovery credential ownership review is missing"))?,
    )?;
    if graph_ownership_audit_hash(connection, &attempt.session_id)? != attempt.plan.graph_sha256 {
        return Err(corrupted(
            "discovery credential ownership graph differs from its audit authority",
        ));
    }
    graph_template_was_created(connection, &attempt.session_id)
        .map_err(|_| corrupted("discovery credential ownership template audit is invalid"))?;

    let actual_binding_sha256 = match binding_authority {
        DiscoveryCredentialBindingAuthority::Active => {
            crate::provider_credential_repository::provider_credential_connection_binding_sha256(
                connection,
                connection_id,
            )?
        }
        DiscoveryCredentialBindingAuthority::ArchivedSlotGarbage => {
            crate::provider_credential_repository::provider_credential_archived_connection_binding_sha256(
                connection,
                connection_id,
            )?
        }
    };
    if actual_binding_sha256 != expected_binding_sha256 {
        return Err(corrupted(
            "discovery credential ownership binding differs from its connection authority",
        ));
    }
    let completed_operation_id =
        validate_discovery_credential_completion_evidence(connection, &attempt, &snapshot)?;
    if completed_operation_id != authority_operation_id {
        return Err(corrupted(
            "discovery credential ownership names a different native operation than its completion history",
        ));
    }
    Ok(())
}

fn validate_discovery_authority_evidence_rows(
    connection: &Connection,
    session_id: &DiscoverySessionId,
    review: &DiscoveryReviewDiff,
) -> CoreResult<()> {
    let evidence_ids = review
        .changes
        .iter()
        .flat_map(|change| &change.evidence_ids)
        .collect::<BTreeSet<_>>();
    for evidence_id in evidence_ids {
        let row = connection
            .query_row(
                "SELECT id, session_id, kind, source_url, content_sha256,
                        extracted_json, fetched_at
                 FROM provider_discovery_evidence
                 WHERE id = ?1 AND session_id = ?2",
                params![evidence_id.as_str(), session_id.as_str()],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, String>(3)?,
                        row.get::<_, String>(4)?,
                        row.get::<_, String>(5)?,
                        row.get::<_, String>(6)?,
                    ))
                },
            )
            .optional()
            .map_err(database_error)?
            .ok_or_else(|| corrupted("discovery credential authority evidence is missing"))?;
        let canonical_extracted = encode_redacted_json(
            &decode_redacted_json(&row.5, "discovery credential authority evidence")?,
            "discovery credential authority evidence",
        )?;
        if canonical_extracted != row.5 {
            return Err(corrupted(
                "discovery credential authority evidence is not canonical",
            ));
        }
        let evidence = decode_evidence_row(row)?;
        if evidence.session_id != *session_id || evidence.id != *evidence_id {
            return Err(corrupted(
                "discovery credential authority evidence is detached from its session",
            ));
        }
    }
    Ok(())
}

fn validate_discovery_credential_completion_evidence(
    connection: &Connection,
    attempt: &DiscoveryCommitAttemptRecord,
    ready: &DiscoverySessionSnapshot,
) -> CoreResult<DiscoveryOperationId> {
    let mut start = load_discovery_authority_receipt_by_action(
        connection,
        &attempt.session_id,
        &attempt.action_id,
    )?;
    validate_atomic_commit_start_receipt(
        &start,
        attempt,
        ready,
        "approve_review",
        attempt.expected_revision,
    )?;
    if start.receipt.action_id != attempt.action_id || start.created_at != attempt.created_at {
        return Err(corrupted(
            "discovery credential commit preparation receipt is detached from its attempt",
        ));
    }
    let commit_audit_sequence = validate_exact_discovery_authority_audit(
        connection,
        &attempt.session_id,
        "commit_prepared",
        "discovery.audit.commit_prepared",
        &attempt.action_id,
        attempt.id.as_str(),
        start.receipt.resulting_revision,
        start.created_at,
    )?;
    if start.transition_audit_sequence >= commit_audit_sequence {
        return Err(corrupted(
            "discovery credential commit audit order is invalid",
        ));
    }
    let review_approval_audit_sequence = connection
        .query_row(
            "SELECT audit_sequence
             FROM provider_discovery_audit_log
             WHERE session_id = ?1
               AND audit_kind = 'approval_recorded'
               AND action_id = ?2
               AND summary_key = 'discovery.audit.approval_recorded'",
            params![attempt.session_id.as_str(), attempt.action_id.as_str()],
            |row| row.get::<_, u64>(0),
        )
        .map_err(database_error)?;
    if review_approval_audit_sequence <= start.transition_audit_sequence
        || review_approval_audit_sequence >= commit_audit_sequence
    {
        return Err(corrupted(
            "discovery credential review audit order is invalid",
        ));
    }
    start.commit_prepared_audit_sequence = Some(commit_audit_sequence);
    let completed_at = attempt
        .completed_at
        .ok_or_else(|| corrupted("completed discovery credential attempt has no timestamp"))?;
    validate_discovery_credential_operation_chain(connection, attempt, ready, start, completed_at)
}

#[allow(clippy::too_many_lines)]
fn validate_discovery_credential_operation_chain(
    connection: &Connection,
    attempt: &DiscoveryCommitAttemptRecord,
    ready: &DiscoverySessionSnapshot,
    mut start: DiscoveryAuthorityReceiptRecord,
    completed_at: DateTime<Utc>,
) -> CoreResult<DiscoveryOperationId> {
    let mut seen_operations = BTreeSet::new();
    loop {
        let operation = load_discovery_authority_operation_for_start(
            connection,
            attempt,
            &start,
            completed_at,
        )?;
        if !seen_operations.insert(operation.id.clone()) {
            return Err(corrupted(
                "discovery credential completion retry history contains a cycle",
            ));
        }
        let operation_start_audit_sequence =
            validate_discovery_operation_start_audit(connection, &operation)?;
        let finished_at = operation
            .finished_at
            .ok_or_else(|| corrupted("discovery credential completion has no timestamp"))?;
        match operation.status {
            DiscoveryOperationStatus::Succeeded => {
                validate_discovery_operation_terminal_audit_order(
                    &start,
                    operation_start_audit_sequence,
                    operation.expected_revision.saturating_add(1),
                    connection,
                )?;
                validate_succeeded_discovery_credential_completion(
                    connection,
                    attempt,
                    ready,
                    &start,
                    operation_start_audit_sequence.ok_or_else(|| {
                        corrupted("successful discovery credential operation has no start audit")
                    })?,
                    &operation,
                    completed_at,
                )?;
                return Ok(operation.id);
            }
            DiscoveryOperationStatus::OutcomeUnknown => {
                let next_start = validate_outcome_unknown_discovery_credential_completion(
                    connection,
                    attempt,
                    ready,
                    &start,
                    operation_start_audit_sequence,
                    &operation,
                    completed_at,
                )?;
                let Some(next_start) = next_start else {
                    return Ok(operation.id);
                };
                start = next_start;
            }
            DiscoveryOperationStatus::Interrupted => {
                let interrupted = load_discovery_authority_receipt_by_revision(
                    connection,
                    &attempt.session_id,
                    operation.expected_revision.saturating_add(1),
                )?;
                validate_discovery_receipt_follows(&start, &interrupted)?;
                validate_discovery_operation_terminal_audit_order_for_receipt(
                    &start,
                    operation_start_audit_sequence,
                    &interrupted,
                )?;
                validate_interrupted_discovery_authority_receipt(
                    &interrupted,
                    attempt,
                    ready,
                    "interrupt",
                    operation.expected_revision,
                )?;
                validate_discovery_operation_interrupted_audit(
                    connection,
                    &operation,
                    &interrupted,
                )?;
                validate_interrupted_discovery_operation_evidence(
                    connection,
                    attempt,
                    &operation,
                    &interrupted,
                    finished_at,
                )?;
                start = load_restart_discovery_authority_receipt(
                    connection,
                    attempt,
                    ready,
                    &interrupted,
                )?;
            }
            _ => {
                return Err(corrupted(
                    "discovery credential completion has no successful native outcome authority",
                ));
            }
        }
    }
}

fn validate_outcome_unknown_discovery_credential_completion(
    connection: &Connection,
    attempt: &DiscoveryCommitAttemptRecord,
    ready: &DiscoverySessionSnapshot,
    start: &DiscoveryAuthorityReceiptRecord,
    operation_start_audit_sequence: Option<u64>,
    operation: &DiscoveryOperationRecord,
    completed_at: DateTime<Utc>,
) -> CoreResult<Option<DiscoveryAuthorityReceiptRecord>> {
    let finished_at = operation
        .finished_at
        .ok_or_else(|| corrupted("outcome-unknown discovery operation has no finish timestamp"))?;
    let unknown = load_discovery_authority_receipt_by_revision(
        connection,
        &attempt.session_id,
        operation.expected_revision.saturating_add(1),
    )?;
    validate_discovery_receipt_follows(start, &unknown)?;
    validate_discovery_operation_terminal_audit_order_for_receipt(
        start,
        operation_start_audit_sequence,
        &unknown,
    )?;
    validate_unknown_discovery_credential_receipt(
        &unknown,
        attempt,
        ready,
        operation.expected_revision,
        finished_at,
    )?;
    validate_discovery_operation_interrupted_audit(connection, operation, &unknown)?;
    let resolution = load_discovery_authority_receipt_by_revision(
        connection,
        &attempt.session_id,
        unknown.receipt.resulting_revision.saturating_add(1),
    )?;
    validate_discovery_receipt_follows(&unknown, &resolution)?;
    if resolution.transition.session.state == DiscoveryState::Ready {
        validate_ready_discovery_authority_receipt(
            &resolution,
            ready,
            attempt,
            "resolve_unknown_outcome",
            unknown.receipt.resulting_revision,
        )?;
        validate_discovery_completion_chronology(attempt, ready, &resolution, completed_at)?;
        validate_discovery_unknown_outcome_resolution(
            connection,
            attempt,
            &resolution,
            &DiscoveryUnknownOutcomeResolution::ConfirmedCommitCompleted {
                connection_id: attempt.plan.connection_id.clone(),
            },
        )?;
        validate_discovery_authority_graph_audits(
            connection,
            attempt,
            operation.expected_revision,
            finished_at,
            false,
            operation_start_audit_sequence.ok_or_else(|| {
                corrupted("outcome-unknown discovery credential operation has no start audit")
            })?,
            unknown.transition_audit_sequence,
        )?;
        return Ok(None);
    }
    validate_interrupted_discovery_authority_receipt(
        &resolution,
        attempt,
        ready,
        "resolve_unknown_outcome",
        unknown.receipt.resulting_revision,
    )?;
    validate_discovery_unknown_outcome_resolution(
        connection,
        attempt,
        &resolution,
        &DiscoveryUnknownOutcomeResolution::ConfirmedNoEffect,
    )?;
    load_restart_discovery_authority_receipt(connection, attempt, ready, &resolution).map(Some)
}

#[allow(clippy::too_many_arguments)]
pub(in crate::discovery_repository) fn validate_exact_discovery_authority_audit(
    connection: &Connection,
    session_id: &DiscoverySessionId,
    audit_kind: &str,
    summary_key: &str,
    action_id: &DiscoveryActionId,
    subject_id: &str,
    session_revision: u64,
    created_at: DateTime<Utc>,
) -> CoreResult<u64> {
    let rows = {
        let mut statement = connection
            .prepare(
                "SELECT audit_sequence, action_id, session_revision, summary_key, created_at
                 FROM provider_discovery_audit_log
                 WHERE session_id = ?1
                   AND audit_kind = ?2
                   AND subject_id = ?3
                   AND action_id = ?4",
            )
            .map_err(database_error)?;
        statement
            .query_map(
                params![
                    session_id.as_str(),
                    audit_kind,
                    subject_id,
                    action_id.as_str()
                ],
                |row| {
                    Ok((
                        row.get::<_, u64>(0)?,
                        row.get::<_, Option<String>>(1)?,
                        row.get::<_, u64>(2)?,
                        row.get::<_, String>(3)?,
                        row.get::<_, String>(4)?,
                    ))
                },
            )
            .map_err(database_error)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(database_error)?
    };
    let exact = matches!(
        rows.as_slice(),
        [(_audit_sequence, Some(audited_action_id), audited_revision, audited_summary, audited_at)]
            if audited_action_id == action_id.as_str()
                && *audited_revision == session_revision
                && audited_summary == summary_key
                && parse_timestamp(audited_at, "discovery authority audit created_at")?
                    == created_at
    );
    if !exact {
        return Err(corrupted(format!(
            "discovery credential operation history is detached from its exact {audit_kind} audit"
        )));
    }
    Ok(rows[0].0)
}

fn validate_succeeded_discovery_credential_completion(
    connection: &Connection,
    attempt: &DiscoveryCommitAttemptRecord,
    ready: &DiscoverySessionSnapshot,
    start: &DiscoveryAuthorityReceiptRecord,
    operation_start_audit_sequence: u64,
    operation: &DiscoveryOperationRecord,
    completed_at: DateTime<Utc>,
) -> CoreResult<()> {
    let finished_at = operation
        .finished_at
        .ok_or_else(|| corrupted("successful discovery operation has no finish timestamp"))?;
    let ready_receipt = load_discovery_authority_receipt_by_revision(
        connection,
        &attempt.session_id,
        operation.expected_revision.saturating_add(1),
    )?;
    validate_discovery_receipt_follows(start, &ready_receipt)?;
    validate_ready_discovery_authority_receipt(
        &ready_receipt,
        ready,
        attempt,
        "commit_succeeded",
        operation.expected_revision,
    )?;
    if finished_at != completed_at {
        return Err(corrupted(
            "successful discovery credential operation does not finish its attempt",
        ));
    }
    validate_discovery_authority_graph_audits(
        connection,
        attempt,
        ready_receipt.receipt.expected_revision,
        ready_receipt.created_at,
        true,
        operation_start_audit_sequence,
        ready_receipt.transition_audit_sequence,
    )?;
    validate_discovery_completion_chronology(attempt, ready, &ready_receipt, completed_at)
}

fn validate_discovery_completion_chronology(
    attempt: &DiscoveryCommitAttemptRecord,
    ready: &DiscoverySessionSnapshot,
    receipt: &DiscoveryAuthorityReceiptRecord,
    completed_at: DateTime<Utc>,
) -> CoreResult<()> {
    if receipt.created_at != completed_at
        || ready.updated_at != completed_at
        || attempt.updated_at != completed_at
        || receipt.receipt.resulting_revision != ready.session.revision
    {
        return Err(corrupted(
            "discovery credential completion chronology is inconsistent",
        ));
    }
    Ok(())
}
