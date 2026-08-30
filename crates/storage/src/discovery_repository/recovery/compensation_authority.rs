//! Durable compensation origin, cancellation, and receipt authority validation.

use crate::discovery_repository::{
    CompensationRow, Connection, CoreError, CoreResult, CredentialRef, DateTime,
    DiscoveryActionRequired, DiscoveryAuthorityReceiptRecord, DiscoveryCommitAttemptId,
    DiscoveryCommitAttemptRecord, DiscoveryCommitPhase, DiscoveryCompensationKind, DiscoveryEffect,
    DiscoveryOperationId, DiscoveryOperationKind, DiscoveryOperationRecord,
    DiscoveryOperationStatus, DiscoveryRecoveryCheckpoint, DiscoverySessionId,
    DiscoverySessionSnapshot, DiscoverySideEffectClass, DiscoveryState,
    DiscoveryUnknownOutcomeResolution, Utc, corrupted, database_error, decode_compensation_row,
    load_commit_attempt, load_discovery_authority_receipt_by_action,
    load_discovery_authority_receipt_by_revision, load_operation_by_id, load_session_snapshot,
    params, validate_discovery_authority_graph_audits,
    validate_discovery_operation_interrupted_audit, validate_discovery_operation_start_audit,
    validate_discovery_operation_terminal_audit_order_for_receipt,
    validate_discovery_receipt_follows, validate_discovery_unknown_outcome_resolution,
    validate_exact_discovery_authority_audit, validate_interrupted_discovery_operation_evidence,
    validate_native_no_effect_operation_start_receipt,
};

pub(in crate::discovery_repository) fn load_discovery_credential_compensation_operation_id(
    connection: &Connection,
    session_id: &DiscoverySessionId,
    attempt_id: &DiscoveryCommitAttemptId,
    plan_sha256: &str,
) -> CoreResult<DiscoveryOperationId> {
    let attempt = load_commit_attempt(connection, attempt_id)?;
    let snapshot = load_session_snapshot(connection, session_id.as_str())?
        .ok_or_else(|| corrupted("credential compensation session is missing"))?;
    if attempt.session_id != *session_id
        || attempt.plan_sha256 != plan_sha256
        || attempt.phase != DiscoveryCommitPhase::Compensating
        || snapshot.session.state != DiscoveryState::Compensating
        || snapshot.session.commit_attempt_id.as_ref() != Some(attempt_id)
        || snapshot.session.commit_plan_sha256.as_deref() != Some(plan_sha256)
        || snapshot.session.input.connection_id != attempt.plan.connection_id
        || snapshot.session.input.credential_ref != attempt.plan.credential_ref
        || attempt
            .plan
            .credential_ref
            .as_ref()
            .map(CredentialRef::as_str)
            != Some(attempt.plan.connection_id.as_str())
    {
        return Err(corrupted(
            "credential compensation is detached from its immutable commit attempt",
        ));
    }
    validate_discovery_credential_compensation_step(connection, &attempt)?;

    let origin_revision = connection
        .query_row(
            "SELECT MIN(receipt.resulting_revision)
             FROM provider_discovery_action_receipts AS receipt
             JOIN provider_discovery_event_outbox AS event
               ON event.id = receipt.event_id
              AND event.session_id = receipt.session_id
             WHERE receipt.session_id = ?1
               AND event.state = 'compensating'
               AND receipt.action_kind IN (
                   'commit_succeeded',
                   'compensation_required',
                   'resolve_unknown_outcome',
                   'restart_interrupted'
               )",
            [session_id.as_str()],
            |row| row.get::<_, Option<u64>>(0),
        )
        .map_err(database_error)?
        .ok_or_else(|| corrupted("credential compensation origin receipt is missing"))?;
    let origin =
        load_discovery_authority_receipt_by_revision(connection, session_id, origin_revision)?;
    validate_discovery_compensation_origin(&origin, &snapshot, &attempt)?;

    let authority_operation_id = match origin.receipt.action_kind.as_str() {
        "commit_succeeded" => validate_direct_discovery_compensation_operation(
            connection,
            &attempt,
            &origin,
            DiscoveryOperationStatus::Succeeded,
        ),
        "compensation_required" => validate_direct_discovery_compensation_operation(
            connection,
            &attempt,
            &origin,
            DiscoveryOperationStatus::Failed,
        ),
        "resolve_unknown_outcome" => validate_confirmed_commit_compensation_operation(
            connection, &attempt, &snapshot, &origin,
        ),
        "restart_interrupted" => validate_restarted_discovery_compensation_operation(
            connection, &attempt, &snapshot, &origin,
        ),
        _ => Err(corrupted(
            "credential compensation origin is not bound to a native commit operation",
        )),
    }?;
    validate_current_discovery_compensation_operation(
        connection,
        &attempt,
        &snapshot,
        origin.receipt.resulting_revision,
    )?;
    Ok(authority_operation_id)
}

fn validate_discovery_credential_compensation_step(
    connection: &Connection,
    attempt: &DiscoveryCommitAttemptRecord,
) -> CoreResult<()> {
    let rows = {
        let mut statement = connection
            .prepare(
                "SELECT id, commit_attempt_id, ordinal, action_id, step_kind, step_json,
                        status, attempt_count, last_failure_json, created_at,
                        updated_at, completed_at
                 FROM provider_discovery_compensation_steps
                 WHERE commit_attempt_id = ?1
                   AND step_kind = 'remove_credential_slot'
                 ORDER BY ordinal, id",
            )
            .map_err(database_error)?;
        statement
            .query_map([attempt.id.as_str()], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, u32>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, String>(4)?,
                    row.get::<_, String>(5)?,
                    row.get::<_, String>(6)?,
                    row.get::<_, u32>(7)?,
                    row.get::<_, Option<String>>(8)?,
                    row.get::<_, String>(9)?,
                    row.get::<_, String>(10)?,
                    row.get::<_, Option<String>>(11)?,
                ))
            })
            .map_err(database_error)?
            .collect::<Result<Vec<CompensationRow>, _>>()
            .map_err(database_error)?
    };
    let [row] = rows.as_slice() else {
        return Err(corrupted(
            "credential compensation requires exactly one immutable slot-removal step",
        ));
    };
    let decoded = decode_compensation_row(row.clone(), &attempt.plan)?;
    if decoded.commit_attempt_id != attempt.id
        || decoded.kind != DiscoveryCompensationKind::RemoveCredentialSlot
    {
        return Err(corrupted(
            "credential compensation slot-removal step is detached from its attempt",
        ));
    }
    Ok(())
}

fn validate_discovery_compensation_origin(
    origin: &DiscoveryAuthorityReceiptRecord,
    current: &DiscoverySessionSnapshot,
    attempt: &DiscoveryCommitAttemptRecord,
) -> CoreResult<()> {
    if origin.receipt.outcome != lorepia_domain::discovery::DiscoveryActionReceiptOutcome::Applied
        || origin.transition.session.state != DiscoveryState::Compensating
        || origin.transition.session.input != current.session.input
        || origin.transition.session.commit_attempt_id.as_ref() != Some(&attempt.id)
        || origin.transition.session.commit_plan_sha256.as_deref()
            != Some(attempt.plan_sha256.as_str())
        || origin.transition.session.manifest_sha256.as_deref()
            != Some(attempt.plan.manifest_sha256.as_str())
        || origin.transition.session.recovery.is_some()
        || origin.transition.session.unknown_operation.is_some()
        || origin.transition.session.failure.is_some()
        || origin.transition.session.committed_connection_id.is_some()
        || origin.transition.session.active_effect_approval.is_some()
        || !origin.transition.session.cancellation_pending
        || !matches!(
            &origin.transition.effect,
            DiscoveryEffect::RunCompensation { commit_attempt_id }
                if commit_attempt_id == &attempt.id
        )
    {
        return Err(corrupted(
            "credential compensation origin is detached from its session and attempt",
        ));
    }
    Ok(())
}

fn validate_direct_discovery_compensation_operation(
    connection: &Connection,
    attempt: &DiscoveryCommitAttemptRecord,
    terminal: &DiscoveryAuthorityReceiptRecord,
    expected_status: DiscoveryOperationStatus,
) -> CoreResult<DiscoveryOperationId> {
    let operation = load_discovery_compensation_source_operation(
        connection,
        attempt,
        terminal,
        expected_status,
    )?;
    let start = validate_native_no_effect_operation_start_receipt(
        connection,
        attempt,
        operation.action_id.as_str(),
        operation.expected_revision,
        &operation.request_sha256,
        &operation.created_at.to_rfc3339(),
    )?;
    validate_discovery_compensation_cancellation_chain(connection, attempt, &start, terminal)?;
    let start_audit = validate_discovery_operation_start_audit(connection, &operation)?;
    validate_discovery_operation_terminal_audit_order_for_receipt(&start, start_audit, terminal)?;
    let finished_at = operation
        .finished_at
        .ok_or_else(|| corrupted("credential compensation source operation has no finish time"))?;
    if operation.status != expected_status
        || operation.created_at > operation.started_at.unwrap_or(finished_at)
        || operation
            .started_at
            .is_none_or(|started_at| started_at > finished_at)
        || finished_at != terminal.created_at
        || operation.updated_at != finished_at
    {
        return Err(corrupted(
            "credential compensation source operation is detached from its terminal receipt",
        ));
    }
    Ok(operation.id)
}

fn validate_confirmed_commit_compensation_operation(
    connection: &Connection,
    attempt: &DiscoveryCommitAttemptRecord,
    current: &DiscoverySessionSnapshot,
    resolution: &DiscoveryAuthorityReceiptRecord,
) -> CoreResult<DiscoveryOperationId> {
    let unknown = load_discovery_authority_receipt_by_revision(
        connection,
        &attempt.session_id,
        resolution.receipt.expected_revision,
    )?;
    validate_discovery_receipt_follows(&unknown, resolution)?;
    if resolution.receipt.expected_revision != unknown.receipt.resulting_revision
        || resolution.receipt.resulting_revision
            != resolution.receipt.expected_revision.saturating_add(1)
        || resolution.receipt.action_kind != "resolve_unknown_outcome"
        || unknown.created_at > resolution.created_at
    {
        return Err(corrupted(
            "confirmed credential commit compensation has no exact unknown outcome history",
        ));
    }
    let source = validate_discovery_outcome_unknown_compensation_source(
        connection, attempt, current, &unknown,
    )?;
    let approval_audit = validate_discovery_unknown_outcome_resolution(
        connection,
        attempt,
        resolution,
        &DiscoveryUnknownOutcomeResolution::ConfirmedCommitCompleted {
            connection_id: attempt.plan.connection_id.clone(),
        },
    )?;
    if source.interrupted_audit_sequence >= resolution.transition_audit_sequence
        || resolution.transition_audit_sequence >= approval_audit
    {
        return Err(corrupted(
            "confirmed credential commit compensation audit order is invalid",
        ));
    }
    validate_discovery_authority_graph_audits(
        connection,
        attempt,
        source.operation.expected_revision,
        source.finished_at,
        false,
        source.start_audit_sequence,
        unknown.transition_audit_sequence,
    )?;
    Ok(source.operation.id)
}

struct DiscoveryOutcomeUnknownCompensationSource {
    operation: DiscoveryOperationRecord,
    start_audit_sequence: u64,
    interrupted_audit_sequence: u64,
    finished_at: DateTime<Utc>,
}

fn validate_discovery_outcome_unknown_compensation_source(
    connection: &Connection,
    attempt: &DiscoveryCommitAttemptRecord,
    current: &DiscoverySessionSnapshot,
    unknown: &DiscoveryAuthorityReceiptRecord,
) -> CoreResult<DiscoveryOutcomeUnknownCompensationSource> {
    if !matches!(
        unknown.receipt.action_kind.as_str(),
        "interrupt" | "external_outcome_became_unknown"
    ) || unknown.receipt.outcome
        != lorepia_domain::discovery::DiscoveryActionReceiptOutcome::Applied
        || unknown.receipt.resulting_revision != unknown.receipt.expected_revision.saturating_add(1)
        || unknown.transition.session.state != DiscoveryState::UnknownOutcome
        || unknown.transition.session.input != current.session.input
        || unknown.transition.session.commit_attempt_id.as_ref() != Some(&attempt.id)
        || unknown.transition.session.commit_plan_sha256.as_deref()
            != Some(attempt.plan_sha256.as_str())
        || unknown.transition.session.manifest_sha256.as_deref()
            != Some(attempt.plan.manifest_sha256.as_str())
        || unknown.transition.session.committed_connection_id.is_some()
        || unknown.transition.session.recovery.is_some()
        || unknown.transition.session.failure.is_some()
        || unknown.transition.session.active_effect_approval.is_some()
        || unknown.transition.session.unknown_operation
            != Some(DiscoveryOperationKind::AtomicCommit)
        || !unknown.transition.session.cancellation_pending
        || unknown.transition.effect != DiscoveryEffect::None
        || unknown.transition.event.action_required
            != Some(DiscoveryActionRequired::ReconcileUnknownOutcome {
                operation: DiscoveryOperationKind::AtomicCommit,
            })
    {
        return Err(corrupted(
            "credential compensation has no exact outcome-unknown source receipt",
        ));
    }
    let operation = load_discovery_compensation_source_operation(
        connection,
        attempt,
        unknown,
        DiscoveryOperationStatus::OutcomeUnknown,
    )?;
    let start = validate_native_no_effect_operation_start_receipt(
        connection,
        attempt,
        operation.action_id.as_str(),
        operation.expected_revision,
        &operation.request_sha256,
        &operation.created_at.to_rfc3339(),
    )?;
    validate_discovery_compensation_cancellation_chain(connection, attempt, &start, unknown)?;
    let start_audit_sequence = validate_discovery_operation_start_audit(connection, &operation)?
        .ok_or_else(|| corrupted("outcome-unknown credential commit has no start audit"))?;
    validate_discovery_operation_terminal_audit_order_for_receipt(
        &start,
        Some(start_audit_sequence),
        unknown,
    )?;
    let finished_at = operation
        .finished_at
        .ok_or_else(|| corrupted("outcome-unknown credential commit has no finish time"))?;
    if operation.status != DiscoveryOperationStatus::OutcomeUnknown
        || operation.created_at > operation.started_at.unwrap_or(finished_at)
        || operation
            .started_at
            .is_none_or(|started_at| started_at > finished_at)
        || finished_at != unknown.created_at
        || operation.updated_at != finished_at
    {
        return Err(corrupted(
            "outcome-unknown credential commit is detached from its terminal receipt",
        ));
    }
    let interrupted_audit_sequence =
        validate_discovery_operation_interrupted_audit(connection, &operation, unknown)?;
    Ok(DiscoveryOutcomeUnknownCompensationSource {
        operation,
        start_audit_sequence,
        interrupted_audit_sequence,
        finished_at,
    })
}

fn validate_restarted_discovery_compensation_operation(
    connection: &Connection,
    attempt: &DiscoveryCommitAttemptRecord,
    current: &DiscoverySessionSnapshot,
    restart: &DiscoveryAuthorityReceiptRecord,
) -> CoreResult<DiscoveryOperationId> {
    let interrupted = load_discovery_authority_receipt_by_revision(
        connection,
        &attempt.session_id,
        restart.receipt.expected_revision,
    )?;
    validate_discovery_receipt_follows(&interrupted, restart)?;
    let recovery_matches = matches!(
        interrupted.transition.session.recovery.as_ref(),
        Some(DiscoveryRecoveryCheckpoint {
            interrupted_state: DiscoveryState::Compensating,
            operation: DiscoveryOperationKind::Compensation,
        })
    );
    if restart.receipt.action_kind != "restart_interrupted"
        || restart.receipt.expected_revision != interrupted.receipt.resulting_revision
        || restart.receipt.resulting_revision != restart.receipt.expected_revision.saturating_add(1)
        || interrupted.receipt.outcome
            != lorepia_domain::discovery::DiscoveryActionReceiptOutcome::Applied
        || interrupted.transition.session.state != DiscoveryState::Interrupted
        || interrupted.transition.session.input != current.session.input
        || interrupted.transition.session.commit_attempt_id.as_ref() != Some(&attempt.id)
        || interrupted.transition.session.commit_plan_sha256.as_deref()
            != Some(attempt.plan_sha256.as_str())
        || interrupted.transition.session.unknown_operation.is_some()
        || interrupted.transition.session.failure.is_some()
        || interrupted
            .transition
            .session
            .committed_connection_id
            .is_some()
        || interrupted
            .transition
            .session
            .active_effect_approval
            .is_some()
        || !interrupted.transition.session.cancellation_pending
        || interrupted.transition.effect != DiscoveryEffect::None
        || interrupted.transition.event.action_required
            != Some(DiscoveryActionRequired::RestartInterrupted {
                operation: DiscoveryOperationKind::Compensation,
            })
        || !recovery_matches
        || interrupted.created_at > restart.created_at
    {
        return Err(corrupted(
            "credential compensation restart has no exact interrupted predecessor",
        ));
    }
    let (operation_id, authority_audit_sequence) = match interrupted.receipt.action_kind.as_str() {
        "interrupt" => {
            validate_attested_no_effect_compensation_source(connection, attempt, &interrupted)
        }
        "resolve_unknown_outcome" => validate_confirmed_no_effect_compensation_source(
            connection,
            attempt,
            current,
            &interrupted,
        ),
        _ => Err(corrupted(
            "credential compensation restart predecessor action is invalid",
        )),
    }?;
    if authority_audit_sequence >= restart.transition_audit_sequence {
        return Err(corrupted(
            "credential compensation restart audit order is invalid",
        ));
    }
    Ok(operation_id)
}

fn validate_attested_no_effect_compensation_source(
    connection: &Connection,
    attempt: &DiscoveryCommitAttemptRecord,
    interrupted: &DiscoveryAuthorityReceiptRecord,
) -> CoreResult<(DiscoveryOperationId, u64)> {
    let operation = load_discovery_compensation_source_operation(
        connection,
        attempt,
        interrupted,
        DiscoveryOperationStatus::Interrupted,
    )?;
    let start = validate_native_no_effect_operation_start_receipt(
        connection,
        attempt,
        operation.action_id.as_str(),
        operation.expected_revision,
        &operation.request_sha256,
        &operation.created_at.to_rfc3339(),
    )?;
    validate_discovery_compensation_cancellation_chain(connection, attempt, &start, interrupted)?;
    let start_audit = validate_discovery_operation_start_audit(connection, &operation)?;
    validate_discovery_operation_terminal_audit_order_for_receipt(
        &start,
        start_audit,
        interrupted,
    )?;
    let finished_at = operation
        .finished_at
        .ok_or_else(|| corrupted("interrupted credential commit has no finish time"))?;
    if operation.status != DiscoveryOperationStatus::Interrupted
        || operation.updated_at != finished_at
        || interrupted.created_at != finished_at
    {
        return Err(corrupted(
            "interrupted credential commit is detached from its terminal receipt",
        ));
    }
    validate_interrupted_discovery_operation_evidence(
        connection,
        attempt,
        &operation,
        interrupted,
        finished_at,
    )?;
    let audit =
        validate_discovery_operation_interrupted_audit(connection, &operation, interrupted)?;
    Ok((operation.id, audit))
}

fn validate_confirmed_no_effect_compensation_source(
    connection: &Connection,
    attempt: &DiscoveryCommitAttemptRecord,
    current: &DiscoverySessionSnapshot,
    resolution: &DiscoveryAuthorityReceiptRecord,
) -> CoreResult<(DiscoveryOperationId, u64)> {
    let unknown = load_discovery_authority_receipt_by_revision(
        connection,
        &attempt.session_id,
        resolution.receipt.expected_revision,
    )?;
    validate_discovery_receipt_follows(&unknown, resolution)?;
    if resolution.receipt.expected_revision != unknown.receipt.resulting_revision
        || resolution.receipt.resulting_revision
            != resolution.receipt.expected_revision.saturating_add(1)
        || unknown.created_at > resolution.created_at
    {
        return Err(corrupted(
            "confirmed no-effect compensation is detached from its unknown outcome",
        ));
    }
    let source = validate_discovery_outcome_unknown_compensation_source(
        connection, attempt, current, &unknown,
    )?;
    let approval_audit = validate_discovery_unknown_outcome_resolution(
        connection,
        attempt,
        resolution,
        &DiscoveryUnknownOutcomeResolution::ConfirmedNoEffect,
    )?;
    if source.interrupted_audit_sequence >= resolution.transition_audit_sequence
        || resolution.transition_audit_sequence >= approval_audit
    {
        return Err(corrupted(
            "confirmed no-effect compensation audit order is invalid",
        ));
    }
    Ok((source.operation.id, approval_audit))
}

fn validate_current_discovery_compensation_operation(
    connection: &Connection,
    attempt: &DiscoveryCommitAttemptRecord,
    current: &DiscoverySessionSnapshot,
    origin_revision: u64,
) -> CoreResult<()> {
    let operation_id = current
        .active_operation_id
        .as_ref()
        .ok_or_else(|| corrupted("credential compensation has no active operation"))?;
    let operation = load_operation_by_id(connection, operation_id)?;
    let start = load_discovery_authority_receipt_by_action(
        connection,
        &attempt.session_id,
        &operation.action_id,
    )?;
    if operation.session_id != attempt.session_id
        || operation.kind != DiscoveryOperationKind::Compensation
        || operation.side_effect_class != DiscoverySideEffectClass::Persistent
        || operation.status != DiscoveryOperationStatus::Started
        || operation.expected_revision != start.receipt.resulting_revision
        || operation.request_sha256 != start.receipt.request_sha256
        || operation.approval.is_some()
        || operation.created_at != start.created_at
        || operation.finished_at.is_some()
        || operation.started_at.is_none()
        || operation.updated_at != operation.started_at.unwrap_or(operation.updated_at)
        || start.receipt.resulting_revision < origin_revision
        || !matches!(
            start.receipt.action_kind.as_str(),
            "commit_succeeded"
                | "compensation_required"
                | "resolve_unknown_outcome"
                | "restart_interrupted"
                | "resume_compensation"
        )
        || start.receipt.outcome
            != lorepia_domain::discovery::DiscoveryActionReceiptOutcome::Applied
        || start.transition.session.state != DiscoveryState::Compensating
        || start.transition.session.input != current.session.input
        || start.transition.session.commit_attempt_id.as_ref() != Some(&attempt.id)
        || start.transition.session.commit_plan_sha256.as_deref()
            != Some(attempt.plan_sha256.as_str())
        || start.transition.session.recovery.is_some()
        || start.transition.session.unknown_operation.is_some()
        || start.transition.session.failure.is_some()
        || !start.transition.session.cancellation_pending
        || !matches!(
            &start.transition.effect,
            DiscoveryEffect::RunCompensation { commit_attempt_id }
                if commit_attempt_id == &attempt.id
        )
    {
        return Err(corrupted(
            "active credential compensation operation is detached from its creating receipt",
        ));
    }
    let started_at = operation
        .started_at
        .ok_or_else(|| corrupted("active credential compensation operation was not started"))?;
    let start_audit = validate_exact_discovery_authority_audit(
        connection,
        &attempt.session_id,
        "operation_started",
        "discovery.audit.operation_started",
        &operation.action_id,
        operation.id.as_str(),
        operation.expected_revision,
        started_at,
    )?;
    if start.transition_audit_sequence >= start_audit {
        return Err(corrupted(
            "active credential compensation operation audit order is invalid",
        ));
    }
    Ok(())
}

fn load_discovery_compensation_source_operation(
    connection: &Connection,
    attempt: &DiscoveryCommitAttemptRecord,
    terminal: &DiscoveryAuthorityReceiptRecord,
    expected_status: DiscoveryOperationStatus,
) -> CoreResult<DiscoveryOperationRecord> {
    let status = match expected_status {
        DiscoveryOperationStatus::Succeeded => "succeeded",
        DiscoveryOperationStatus::Failed => "failed",
        DiscoveryOperationStatus::Interrupted => "interrupted",
        DiscoveryOperationStatus::OutcomeUnknown => "outcome_unknown",
        _ => {
            return Err(CoreError::internal(
                "unsupported direct credential compensation operation status",
            ));
        }
    };
    let ids = {
        let mut statement = connection
            .prepare(
                "SELECT operation.id
                 FROM provider_discovery_operations AS operation
                 JOIN provider_discovery_authorized_native_commit_starts AS authorized
                   ON authorized.operation_id = operation.id
                  AND authorized.session_id = operation.session_id
                  AND authorized.operation_expected_revision = operation.expected_revision
                 WHERE operation.session_id = ?1
                   AND operation.operation_kind = 'atomic_commit'
                   AND operation.side_effect_class = 'persistent'
                   AND operation.status = ?2
                   AND operation.finished_at = ?3
                   AND authorized.commit_attempt_id = ?4
                   AND authorized.commit_plan_sha256 = ?5
                 ORDER BY operation.id",
            )
            .map_err(database_error)?;
        statement
            .query_map(
                params![
                    attempt.session_id.as_str(),
                    status,
                    terminal.created_at.to_rfc3339(),
                    attempt.id.as_str(),
                    attempt.plan_sha256.as_str(),
                ],
                |row| row.get::<_, String>(0),
            )
            .map_err(database_error)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(database_error)?
    };
    let [operation_id] = ids.as_slice() else {
        return Err(corrupted(
            "credential compensation source operation is missing or ambiguous",
        ));
    };
    load_operation_by_id(
        connection,
        &DiscoveryOperationId::parse(operation_id)
            .map_err(|_| corrupted("credential compensation source operation id is invalid"))?,
    )
}

pub(in crate::discovery_repository) fn validate_active_discovery_credential_cancellation_chain(
    connection: &Connection,
    attempt: &DiscoveryCommitAttemptRecord,
    current: &DiscoverySessionSnapshot,
    start: &DiscoveryAuthorityReceiptRecord,
) -> CoreResult<()> {
    if current.session.revision <= start.receipt.resulting_revision {
        return Err(corrupted(
            "active discovery credential cancellation has no durable receipt",
        ));
    }
    let mut previous = start;
    let mut owned = Vec::new();
    for resulting_revision in
        start.receipt.resulting_revision.saturating_add(1)..=current.session.revision
    {
        let receipt = load_discovery_authority_receipt_by_revision(
            connection,
            &attempt.session_id,
            resulting_revision,
        )?;
        validate_discovery_receipt_follows(previous, &receipt)?;
        let first = owned.is_empty();
        let effect_matches = if first {
            receipt.transition.effect
                == DiscoveryEffect::RequestCancellation {
                    operation: DiscoveryOperationKind::AtomicCommit,
                }
        } else {
            receipt.transition.effect == DiscoveryEffect::None
        };
        if receipt.receipt.action_kind != "cancel"
            || receipt.receipt.outcome
                != lorepia_domain::discovery::DiscoveryActionReceiptOutcome::Applied
            || receipt.receipt.expected_revision != previous.receipt.resulting_revision
            || receipt.receipt.resulting_revision
                != receipt.receipt.expected_revision.saturating_add(1)
            || receipt.transition.session.state != DiscoveryState::Committing
            || receipt.transition.session.id != attempt.session_id
            || receipt.transition.session.input != start.transition.session.input
            || receipt.transition.session.manifest_sha256
                != start.transition.session.manifest_sha256
            || receipt.transition.session.commit_attempt_id.as_ref() != Some(&attempt.id)
            || receipt.transition.session.commit_plan_sha256.as_deref()
                != Some(attempt.plan_sha256.as_str())
            || receipt.transition.session.committed_connection_id.is_some()
            || receipt.transition.session.recovery.is_some()
            || receipt.transition.session.unknown_operation.is_some()
            || receipt.transition.session.failure.is_some()
            || receipt.transition.session.active_effect_approval.is_some()
            || !receipt.transition.session.cancellation_pending
            || !effect_matches
            || previous.transition_audit_sequence >= receipt.transition_audit_sequence
            || previous.created_at > receipt.created_at
        {
            return Err(corrupted(
                "active discovery credential cancellation history is not canonical",
            ));
        }
        owned.push(receipt);
        previous = owned
            .last()
            .expect("a just-pushed cancellation receipt exists");
    }
    if previous.transition.session != current.session {
        return Err(corrupted(
            "active discovery credential cancellation is detached from the current session",
        ));
    }
    Ok(())
}

pub(super) fn validate_discovery_compensation_cancellation_chain(
    connection: &Connection,
    attempt: &DiscoveryCommitAttemptRecord,
    start: &DiscoveryAuthorityReceiptRecord,
    terminal: &DiscoveryAuthorityReceiptRecord,
) -> CoreResult<()> {
    if terminal.receipt.expected_revision <= start.receipt.resulting_revision {
        return Err(corrupted(
            "credential compensation has no durable cancellation receipt",
        ));
    }
    let mut previous = start;
    let mut owned = Vec::new();
    for resulting_revision in
        start.receipt.resulting_revision.saturating_add(1)..=terminal.receipt.expected_revision
    {
        let receipt = load_discovery_authority_receipt_by_revision(
            connection,
            &attempt.session_id,
            resulting_revision,
        )?;
        validate_discovery_receipt_follows(previous, &receipt)?;
        let first = owned.is_empty();
        let effect_matches = if first {
            receipt.transition.effect
                == DiscoveryEffect::RequestCancellation {
                    operation: DiscoveryOperationKind::AtomicCommit,
                }
        } else {
            receipt.transition.effect == DiscoveryEffect::None
        };
        if receipt.receipt.action_kind != "cancel"
            || receipt.receipt.outcome
                != lorepia_domain::discovery::DiscoveryActionReceiptOutcome::Applied
            || receipt.receipt.resulting_revision
                != receipt.receipt.expected_revision.saturating_add(1)
            || receipt.receipt.expected_revision != previous.receipt.resulting_revision
            || receipt.transition.session.state != DiscoveryState::Committing
            || receipt.transition.session.id != attempt.session_id
            || receipt.transition.session.input.connection_id != attempt.plan.connection_id
            || receipt.transition.session.input.credential_ref != attempt.plan.credential_ref
            || receipt.transition.session.commit_attempt_id.as_ref() != Some(&attempt.id)
            || receipt.transition.session.commit_plan_sha256.as_deref()
                != Some(attempt.plan_sha256.as_str())
            || receipt.transition.session.recovery.is_some()
            || receipt.transition.session.unknown_operation.is_some()
            || receipt.transition.session.failure.is_some()
            || !receipt.transition.session.cancellation_pending
            || !effect_matches
            || previous.transition_audit_sequence >= receipt.transition_audit_sequence
            || previous.created_at > receipt.created_at
            || receipt.created_at > terminal.created_at
        {
            return Err(corrupted(
                "credential compensation cancellation history is not canonical",
            ));
        }
        owned.push(receipt);
        previous = owned
            .last()
            .expect("a just-pushed cancellation receipt exists");
    }
    validate_discovery_receipt_follows(previous, terminal)?;
    if terminal.receipt.expected_revision != previous.receipt.resulting_revision
        || previous.created_at > terminal.created_at
        || previous.transition_audit_sequence >= terminal.transition_audit_sequence
    {
        return Err(corrupted(
            "credential compensation terminal audit precedes its cancellation",
        ));
    }
    Ok(())
}
