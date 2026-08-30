//! Credential action receipts, interruption chronology, and restart evidence.

use super::{
    DISCOVERY_REDACTION_VERSION,
    attestation::{
        load_native_no_effect_attestation,
        validate_schema37_abandoned_native_credential_reservation,
    },
    authority::validate_exact_discovery_authority_audit,
};
use crate::discovery_repository::{
    Connection, CoreResult, DateTime, DiscoveryActionId, DiscoveryActionReceipt,
    DiscoveryActionRequired, DiscoveryCommitAttemptRecord, DiscoveryEffect, DiscoveryEventId,
    DiscoveryNativeNoEffectAttestationKind, DiscoveryNativeRecoveryOwner, DiscoveryOperationId,
    DiscoveryOperationKind, DiscoveryOperationRecord, DiscoveryOperationStatus,
    DiscoveryRecoveryCheckpoint, DiscoverySessionId, DiscoverySessionSnapshot,
    DiscoverySideEffectClass, DiscoveryState, DiscoveryTransition, OptionalExtension,
    PROVIDER_DISCOVERY_EVENT_VERSION, ProviderDiscoveryEvent, Utc, Value, audit_kind_for_action,
    corrupted, database_error, encode_json_result, load_operation_by_id, params,
    parse_discovery_state, parse_timestamp, validate_discovery_authority_approval_rows,
    validate_native_no_effect_retry_predecessor, validate_sha256,
};

pub(in crate::discovery_repository) fn validate_native_no_effect_operation_start_receipt(
    connection: &Connection,
    attempt: &DiscoveryCommitAttemptRecord,
    action_id: &str,
    expected_operation_revision: u64,
    request_sha256: &str,
    operation_created_at: &str,
) -> CoreResult<DiscoveryAuthorityReceiptRecord> {
    let approval_audits = validate_discovery_authority_approval_rows(connection, attempt)?;
    let action_id = DiscoveryActionId::parse(action_id)
        .map_err(|_| corrupted("native no-effect operation action id is invalid"))?;
    let mut start =
        load_discovery_authority_receipt_by_action(connection, &attempt.session_id, &action_id)?;
    let initial_start = start.receipt.action_kind == "approve_review"
        && start.receipt.action_id == attempt.action_id
        && start.receipt.expected_revision == attempt.expected_revision;
    let retry_start = start.receipt.action_kind == "restart_interrupted"
        && start.receipt.expected_revision > attempt.expected_revision;
    if (!initial_start && !retry_start)
        || start.receipt.outcome
            != lorepia_domain::discovery::DiscoveryActionReceiptOutcome::Applied
        || start.receipt.resulting_revision != expected_operation_revision
        || start.receipt.resulting_revision != start.receipt.expected_revision.saturating_add(1)
        || start.receipt.request_sha256 != request_sha256
        || start.created_at
            != parse_timestamp(
                operation_created_at,
                "native no-effect operation created_at",
            )?
        || start.transition.session.state != DiscoveryState::Committing
        || start.transition.session.id != attempt.session_id
        || start.transition.session.input.connection_id != attempt.plan.connection_id
        || start.transition.session.input.credential_ref != attempt.plan.credential_ref
        || start.transition.session.manifest_sha256.as_deref()
            != Some(attempt.plan.manifest_sha256.as_str())
        || start.transition.session.commit_attempt_id.as_ref() != Some(&attempt.id)
        || start.transition.session.commit_plan_sha256.as_deref()
            != Some(attempt.plan_sha256.as_str())
        || start.transition.session.committed_connection_id.is_some()
        || start.transition.session.cancellation_pending
        || start.transition.session.active_effect_approval.is_some()
        || start.transition.session.failure.is_some()
        || start.transition.session.recovery.is_some()
        || start.transition.session.unknown_operation.is_some()
        || !matches!(
            &start.transition.effect,
            DiscoveryEffect::CommitAtomically {
                commit_attempt_id,
                plan_sha256,
            } if commit_attempt_id == &attempt.id && plan_sha256 == &attempt.plan_sha256
        )
    {
        return Err(corrupted(
            "native no-effect operation is detached from its exact commit start receipt",
        ));
    }
    if retry_start {
        validate_native_no_effect_retry_predecessor(connection, attempt, &start)?;
    }
    let commit_audit_sequence = validate_exact_discovery_authority_audit(
        connection,
        &attempt.session_id,
        "commit_prepared",
        "discovery.audit.commit_prepared",
        &start.receipt.action_id,
        attempt.id.as_str(),
        start.receipt.resulting_revision,
        start.created_at,
    )?;
    let initial_approval_audit_order_is_invalid = initial_start
        && !(approval_audits.credential < start.transition_audit_sequence
            && start.transition_audit_sequence < approval_audits.review
            && approval_audits.review < commit_audit_sequence);
    if start.transition_audit_sequence >= commit_audit_sequence
        || initial_approval_audit_order_is_invalid
    {
        return Err(corrupted(
            "native no-effect commit-start audit order is invalid",
        ));
    }
    start.commit_prepared_audit_sequence = Some(commit_audit_sequence);
    Ok(start)
}

pub(in crate::discovery_repository) fn validate_discovery_operation_start_audit(
    connection: &Connection,
    operation: &DiscoveryOperationRecord,
) -> CoreResult<Option<u64>> {
    let started_at = operation
        .started_at
        .ok_or_else(|| corrupted("discovery credential operation has no start timestamp"))?;
    let attestation_rows = if operation.status == DiscoveryOperationStatus::Interrupted {
        connection
            .query_row(
                "SELECT COUNT(*)
                 FROM provider_discovery_native_no_effect_attestations
                 WHERE operation_id = ?1",
                [operation.id.as_str()],
                |row| row.get::<_, u64>(0),
            )
            .map_err(database_error)?
    } else {
        0
    };
    let attested = if operation.status == DiscoveryOperationStatus::Interrupted {
        load_native_no_effect_attestation(connection, operation.id.as_str())?.is_some()
    } else {
        false
    };
    if attestation_rows > 0 && !attested {
        return Err(corrupted(
            "legacy native no-effect history cannot authorize a schema-37 retry",
        ));
    }
    if operation.status == DiscoveryOperationStatus::Interrupted && !attested {
        validate_schema37_abandoned_native_credential_reservation(connection, operation)?;
        if operation.finished_at != Some(started_at) {
            return Err(corrupted(
                "prepared discovery credential interruption has inconsistent timestamps",
            ));
        }
        let count = connection
            .query_row(
                "SELECT COUNT(*)
                 FROM provider_discovery_audit_log
                 WHERE session_id = ?1
                   AND audit_kind = 'operation_started'
                   AND subject_id = ?2",
                params![operation.session_id.as_str(), operation.id.as_str()],
                |row| row.get::<_, u64>(0),
            )
            .map_err(database_error)?;
        if count != 0 {
            return Err(corrupted(
                "prepared discovery credential interruption has a forged start audit",
            ));
        }
        return Ok(None);
    }
    validate_exact_discovery_authority_audit(
        connection,
        &operation.session_id,
        "operation_started",
        "discovery.audit.operation_started",
        &operation.action_id,
        operation.id.as_str(),
        operation.expected_revision,
        started_at,
    )
    .map(Some)
}

pub(super) fn validate_discovery_operation_terminal_audit_order(
    start: &DiscoveryAuthorityReceiptRecord,
    operation_start_audit_sequence: Option<u64>,
    terminal_revision: u64,
    connection: &Connection,
) -> CoreResult<()> {
    let terminal = load_discovery_authority_receipt_by_revision(
        connection,
        &start.receipt.session_id,
        terminal_revision,
    )?;
    validate_discovery_operation_terminal_audit_order_for_receipt(
        start,
        operation_start_audit_sequence,
        &terminal,
    )
}

pub(in crate::discovery_repository) fn validate_discovery_operation_terminal_audit_order_for_receipt(
    start: &DiscoveryAuthorityReceiptRecord,
    operation_start_audit_sequence: Option<u64>,
    terminal: &DiscoveryAuthorityReceiptRecord,
) -> CoreResult<()> {
    let commit_audit_sequence = start
        .commit_prepared_audit_sequence
        .ok_or_else(|| corrupted("discovery credential commit-start audit is missing"))?;
    let lower_bound = operation_start_audit_sequence.unwrap_or(commit_audit_sequence);
    if commit_audit_sequence > lower_bound || lower_bound >= terminal.transition_audit_sequence {
        return Err(corrupted(
            "discovery credential operation audit order is invalid",
        ));
    }
    Ok(())
}

pub(in crate::discovery_repository) fn validate_discovery_operation_interrupted_audit(
    connection: &Connection,
    operation: &DiscoveryOperationRecord,
    receipt: &DiscoveryAuthorityReceiptRecord,
) -> CoreResult<u64> {
    let interrupted_audit_sequence = validate_exact_discovery_authority_audit(
        connection,
        &operation.session_id,
        "operation_interrupted",
        "discovery.audit.operation_interrupted",
        &receipt.receipt.action_id,
        operation.id.as_str(),
        receipt.receipt.resulting_revision,
        receipt.created_at,
    )?;
    if receipt.transition_audit_sequence >= interrupted_audit_sequence {
        return Err(corrupted(
            "discovery credential interruption audit order is invalid",
        ));
    }
    Ok(interrupted_audit_sequence)
}

pub(super) fn validate_atomic_commit_start_receipt(
    start: &DiscoveryAuthorityReceiptRecord,
    attempt: &DiscoveryCommitAttemptRecord,
    ready: &DiscoverySessionSnapshot,
    expected_action_kind: &str,
    expected_revision: u64,
) -> CoreResult<()> {
    if start.receipt.action_kind != expected_action_kind
        || start.receipt.expected_revision != expected_revision
        || start.receipt.resulting_revision != expected_revision.saturating_add(1)
        || start.receipt.outcome
            != lorepia_domain::discovery::DiscoveryActionReceiptOutcome::Applied
        || start.transition.session.state != DiscoveryState::Committing
        || start.transition.session.id != attempt.session_id
        || start.transition.session.input != ready.session.input
        || start.transition.session.commit_attempt_id.as_ref() != Some(&attempt.id)
        || start.transition.session.commit_plan_sha256.as_deref()
            != Some(attempt.plan_sha256.as_str())
        || start.transition.session.manifest_sha256.as_deref()
            != Some(attempt.plan.manifest_sha256.as_str())
        || start.transition.session.committed_connection_id.is_some()
        || start.transition.session.cancellation_pending
        || start.transition.session.failure.is_some()
        || start.transition.session.recovery.is_some()
        || start.transition.session.unknown_operation.is_some()
        || !matches!(
            &start.transition.effect,
            DiscoveryEffect::CommitAtomically {
                commit_attempt_id,
                plan_sha256,
            } if commit_attempt_id == &attempt.id && plan_sha256 == &attempt.plan_sha256
        )
    {
        return Err(corrupted(
            "discovery credential atomic commit start is detached from its attempt",
        ));
    }
    Ok(())
}

pub(super) fn load_discovery_authority_operation_for_start(
    connection: &Connection,
    attempt: &DiscoveryCommitAttemptRecord,
    start: &DiscoveryAuthorityReceiptRecord,
    completed_at: DateTime<Utc>,
) -> CoreResult<DiscoveryOperationRecord> {
    let operation_id = connection
        .query_row(
            "SELECT id
             FROM provider_discovery_operations
             WHERE session_id = ?1 AND action_id = ?2",
            params![
                attempt.session_id.as_str(),
                start.receipt.action_id.as_str()
            ],
            |row| row.get::<_, String>(0),
        )
        .optional()
        .map_err(database_error)?
        .ok_or_else(|| corrupted("discovery credential completion operation is missing"))?;
    let operation_id = DiscoveryOperationId::parse(operation_id)
        .map_err(|_| corrupted("discovery credential completion operation id is invalid"))?;
    let operation = load_operation_by_id(connection, &operation_id)?;
    if operation.session_id != attempt.session_id
        || operation.kind != DiscoveryOperationKind::AtomicCommit
        || operation.side_effect_class != DiscoverySideEffectClass::Persistent
        || operation.action_id != start.receipt.action_id
        || operation.expected_revision != start.receipt.resulting_revision
        || operation.request_sha256 != start.receipt.request_sha256
        || operation.approval.is_some()
        || operation.started_at.is_none()
        || operation.finished_at.is_none()
        || operation.created_at != start.created_at
    {
        return Err(corrupted(
            "discovery credential completion operation is detached from its commit attempt",
        ));
    }
    let finished_at = operation
        .finished_at
        .ok_or_else(|| corrupted("discovery credential completion has no timestamp"))?;
    if operation
        .started_at
        .is_some_and(|started_at| started_at > finished_at)
        || finished_at > completed_at
        || operation.updated_at != finished_at
    {
        return Err(corrupted(
            "discovery credential completion timestamps are inconsistent",
        ));
    }
    Ok(operation)
}

pub(super) fn validate_unknown_discovery_credential_receipt(
    unknown: &DiscoveryAuthorityReceiptRecord,
    attempt: &DiscoveryCommitAttemptRecord,
    ready: &DiscoverySessionSnapshot,
    expected_revision: u64,
    finished_at: DateTime<Utc>,
) -> CoreResult<()> {
    if !matches!(
        unknown.receipt.action_kind.as_str(),
        "interrupt" | "external_outcome_became_unknown"
    ) || unknown.receipt.expected_revision != expected_revision
        || unknown.receipt.resulting_revision != expected_revision.saturating_add(1)
        || unknown.receipt.outcome
            != lorepia_domain::discovery::DiscoveryActionReceiptOutcome::Applied
        || unknown.transition.session.state != DiscoveryState::UnknownOutcome
        || unknown.transition.session.input != ready.session.input
        || unknown.transition.session.commit_attempt_id.as_ref() != Some(&attempt.id)
        || unknown.transition.session.commit_plan_sha256.as_deref()
            != Some(attempt.plan_sha256.as_str())
        || unknown.transition.session.unknown_operation
            != Some(DiscoveryOperationKind::AtomicCommit)
        || unknown.transition.session.recovery.is_some()
        || unknown.transition.session.cancellation_pending
        || unknown.transition.effect != DiscoveryEffect::None
        || unknown.created_at != finished_at
    {
        return Err(corrupted(
            "outcome-unknown discovery credential receipt is detached from its operation",
        ));
    }
    Ok(())
}

pub(in crate::discovery_repository) fn validate_interrupted_discovery_authority_receipt(
    interrupted: &DiscoveryAuthorityReceiptRecord,
    attempt: &DiscoveryCommitAttemptRecord,
    ready: &DiscoverySessionSnapshot,
    expected_action_kind: &str,
    expected_revision: u64,
) -> CoreResult<()> {
    let recovery_matches = matches!(
        interrupted.transition.session.recovery.as_ref(),
        Some(DiscoveryRecoveryCheckpoint {
            interrupted_state: DiscoveryState::Committing,
            operation: DiscoveryOperationKind::AtomicCommit,
        })
    );
    if interrupted.receipt.action_kind != expected_action_kind
        || interrupted.receipt.expected_revision != expected_revision
        || interrupted.receipt.resulting_revision != expected_revision.saturating_add(1)
        || interrupted.receipt.outcome
            != lorepia_domain::discovery::DiscoveryActionReceiptOutcome::Applied
        || interrupted.transition.session.state != DiscoveryState::Interrupted
        || interrupted.transition.session.input != ready.session.input
        || interrupted.transition.session.commit_attempt_id.as_ref() != Some(&attempt.id)
        || interrupted.transition.session.commit_plan_sha256.as_deref()
            != Some(attempt.plan_sha256.as_str())
        || interrupted.transition.session.unknown_operation.is_some()
        || interrupted.transition.session.cancellation_pending
        || interrupted.transition.effect != DiscoveryEffect::None
        || !recovery_matches
    {
        return Err(corrupted(
            "interrupted discovery credential receipt is detached from its retry authority",
        ));
    }
    Ok(())
}

pub(in crate::discovery_repository) fn validate_cancelled_pre_store_interruption_receipt(
    interrupted: &DiscoveryAuthorityReceiptRecord,
    attempt: &DiscoveryCommitAttemptRecord,
    current: &DiscoverySessionSnapshot,
    operation_expected_revision: u64,
) -> CoreResult<()> {
    let recovery_matches = matches!(
        interrupted.transition.session.recovery.as_ref(),
        Some(DiscoveryRecoveryCheckpoint {
            interrupted_state: DiscoveryState::Compensating,
            operation: DiscoveryOperationKind::Compensation,
        })
    );
    if interrupted.receipt.action_kind != "interrupt"
        || interrupted.receipt.expected_revision <= operation_expected_revision
        || interrupted.receipt.resulting_revision
            != interrupted.receipt.expected_revision.saturating_add(1)
        || interrupted.receipt.outcome
            != lorepia_domain::discovery::DiscoveryActionReceiptOutcome::Applied
        || interrupted.transition.session.state != DiscoveryState::Interrupted
        || interrupted.transition.session.id != attempt.session_id
        || interrupted.transition.session.input != current.session.input
        || interrupted.transition.session.manifest_sha256.as_deref()
            != Some(attempt.plan.manifest_sha256.as_str())
        || interrupted.transition.session.commit_attempt_id.as_ref() != Some(&attempt.id)
        || interrupted.transition.session.commit_plan_sha256.as_deref()
            != Some(attempt.plan_sha256.as_str())
        || interrupted
            .transition
            .session
            .committed_connection_id
            .is_some()
        || interrupted.transition.session.unknown_operation.is_some()
        || interrupted.transition.session.failure.is_some()
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
    {
        return Err(corrupted(
            "cancelled pre-store credential interruption is detached from its recovery authority",
        ));
    }
    Ok(())
}

pub(in crate::discovery_repository) fn validate_interrupted_discovery_operation_evidence(
    connection: &Connection,
    attempt: &DiscoveryCommitAttemptRecord,
    operation: &DiscoveryOperationRecord,
    interrupted: &DiscoveryAuthorityReceiptRecord,
    finished_at: DateTime<Utc>,
) -> CoreResult<()> {
    if interrupted.created_at != finished_at {
        return Err(corrupted(
            "interrupted discovery credential operation has inconsistent chronology",
        ));
    }
    let attestation_rows = connection
        .query_row(
            "SELECT COUNT(*)
             FROM provider_discovery_native_no_effect_attestations
             WHERE operation_id = ?1",
            [operation.id.as_str()],
            |row| row.get::<_, u64>(0),
        )
        .map_err(database_error)?;
    if let Some(attestation) = load_native_no_effect_attestation(connection, operation.id.as_str())?
    {
        if attestation.session_id != attempt.session_id
            || attestation.commit_attempt_id != attempt.id
            || attestation.commit_plan_sha256 != attempt.plan_sha256
            || attestation.connection_id != attempt.plan.connection_id
            || attestation.kind != DiscoveryNativeNoEffectAttestationKind::CredentialSlotMissing
            || attestation.recovery_owner != DiscoveryNativeRecoveryOwner::NativePlatform
            || attestation.attested_at != finished_at
        {
            return Err(corrupted(
                "native no-effect attestation is detached from its retry operation",
            ));
        }
    } else if attestation_rows > 0 {
        return Err(corrupted(
            "legacy native no-effect history cannot authorize a schema-37 retry",
        ));
    } else if operation.started_at != Some(finished_at) {
        return Err(corrupted(
            "started persistent discovery operation was interrupted without native no-effect evidence",
        ));
    }
    Ok(())
}

pub(super) fn load_restart_discovery_authority_receipt(
    connection: &Connection,
    attempt: &DiscoveryCommitAttemptRecord,
    ready: &DiscoverySessionSnapshot,
    interrupted: &DiscoveryAuthorityReceiptRecord,
) -> CoreResult<DiscoveryAuthorityReceiptRecord> {
    if interrupted.receipt.resulting_revision >= ready.session.revision {
        return Err(corrupted(
            "interrupted discovery credential history has no bounded restart",
        ));
    }
    let mut restart = load_discovery_authority_receipt_by_revision(
        connection,
        &attempt.session_id,
        interrupted.receipt.resulting_revision.saturating_add(1),
    )?;
    validate_discovery_receipt_follows(interrupted, &restart)?;
    validate_atomic_commit_start_receipt(
        &restart,
        attempt,
        ready,
        "restart_interrupted",
        interrupted.receipt.resulting_revision,
    )?;
    let commit_audit_sequence = validate_exact_discovery_authority_audit(
        connection,
        &attempt.session_id,
        "commit_prepared",
        "discovery.audit.commit_prepared",
        &restart.receipt.action_id,
        attempt.id.as_str(),
        restart.receipt.resulting_revision,
        restart.created_at,
    )?;
    if restart.transition_audit_sequence >= commit_audit_sequence {
        return Err(corrupted(
            "discovery credential retry commit audit order is invalid",
        ));
    }
    restart.commit_prepared_audit_sequence = Some(commit_audit_sequence);
    Ok(restart)
}

pub(in crate::discovery_repository) fn validate_discovery_receipt_follows(
    previous: &DiscoveryAuthorityReceiptRecord,
    next: &DiscoveryAuthorityReceiptRecord,
) -> CoreResult<()> {
    if next.receipt.event_sequence != previous.transition.session.next_event_sequence {
        return Err(corrupted(
            "discovery credential receipt history has a detached event sequence",
        ));
    }
    Ok(())
}

pub(in crate::discovery_repository) struct DiscoveryAuthorityReceiptRecord {
    pub(in crate::discovery_repository) receipt: DiscoveryActionReceipt,
    pub(in crate::discovery_repository) transition: DiscoveryTransition,
    pub(in crate::discovery_repository) created_at: DateTime<Utc>,
    pub(in crate::discovery_repository) transition_audit_sequence: u64,
    pub(in crate::discovery_repository) commit_prepared_audit_sequence: Option<u64>,
}

type DiscoveryAuthorityReceiptRow = (
    String,
    String,
    String,
    u64,
    u64,
    u64,
    String,
    String,
    u32,
    String,
    String,
    String,
    u64,
    u32,
    u64,
    String,
    String,
    u32,
    String,
);

pub(in crate::discovery_repository) fn load_discovery_authority_receipt_by_action(
    connection: &Connection,
    session_id: &DiscoverySessionId,
    action_id: &DiscoveryActionId,
) -> CoreResult<DiscoveryAuthorityReceiptRecord> {
    load_discovery_authority_receipt(
        connection,
        session_id,
        "receipt.action_id = ?2",
        action_id.as_str(),
    )
}

pub(in crate::discovery_repository) fn load_discovery_authority_receipt_by_revision(
    connection: &Connection,
    session_id: &DiscoverySessionId,
    resulting_revision: u64,
) -> CoreResult<DiscoveryAuthorityReceiptRecord> {
    load_discovery_authority_receipt(
        connection,
        session_id,
        "receipt.resulting_revision = ?2",
        resulting_revision,
    )
}

fn load_discovery_authority_receipt(
    connection: &Connection,
    session_id: &DiscoverySessionId,
    predicate: &str,
    selector: impl rusqlite::ToSql,
) -> CoreResult<DiscoveryAuthorityReceiptRecord> {
    let sql = format!(
        "SELECT receipt.action_id, receipt.action_kind, receipt.request_sha256,
                receipt.expected_revision, receipt.resulting_revision,
                receipt.event_sequence, receipt.outcome, receipt.response_json,
                receipt.redaction_version, receipt.created_at,
                event.id, event.session_id, event.sequence,
                event.event_version, event.session_revision, event.state,
                event.event_json, event.redaction_version, event.created_at
         FROM provider_discovery_action_receipts AS receipt
         JOIN provider_discovery_event_outbox AS event
           ON event.id = receipt.event_id
          AND event.session_id = receipt.session_id
         WHERE receipt.session_id = ?1 AND {predicate}"
    );
    let row = connection
        .query_row(&sql, params![session_id.as_str(), selector], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, u64>(3)?,
                row.get::<_, u64>(4)?,
                row.get::<_, u64>(5)?,
                row.get::<_, String>(6)?,
                row.get::<_, String>(7)?,
                row.get::<_, u32>(8)?,
                row.get::<_, String>(9)?,
                row.get::<_, String>(10)?,
                row.get::<_, String>(11)?,
                row.get::<_, u64>(12)?,
                row.get::<_, u32>(13)?,
                row.get::<_, u64>(14)?,
                row.get::<_, String>(15)?,
                row.get::<_, String>(16)?,
                row.get::<_, u32>(17)?,
                row.get::<_, String>(18)?,
            ))
        })
        .optional()
        .map_err(database_error)?
        .ok_or_else(|| corrupted("discovery credential authority receipt is missing"))?;
    let mut record = decode_discovery_authority_receipt_row(row, session_id)?;
    record.transition_audit_sequence = validate_exact_discovery_authority_audit(
        connection,
        session_id,
        audit_kind_for_action(&record.receipt.action_kind),
        "discovery.audit.transition_applied",
        &record.receipt.action_id,
        record.transition.event.id.as_str(),
        record.receipt.resulting_revision,
        record.created_at,
    )?;
    Ok(record)
}

fn decode_discovery_authority_receipt_row(
    row: DiscoveryAuthorityReceiptRow,
    session_id: &DiscoverySessionId,
) -> CoreResult<DiscoveryAuthorityReceiptRecord> {
    let receipt = DiscoveryActionReceipt {
        action_id: DiscoveryActionId::parse(&row.0)
            .map_err(|_| corrupted("discovery credential receipt action id is invalid"))?,
        session_id: session_id.clone(),
        action_kind: row.1,
        request_sha256: row.2,
        expected_revision: row.3,
        resulting_revision: row.4,
        event_sequence: row.5,
        outcome: serde_json::from_value(Value::String(row.6))
            .map_err(|_| corrupted("discovery credential receipt outcome is invalid"))?,
    };
    let transition = serde_json::from_str::<DiscoveryTransition>(&row.7)
        .map_err(|_| corrupted("discovery credential receipt response is invalid"))?;
    let event = serde_json::from_str::<ProviderDiscoveryEvent>(&row.16)
        .map_err(|_| corrupted("discovery credential receipt event is invalid"))?;
    let canonical_transition_json = encode_json_result(
        serde_json::to_value(&transition),
        "discovery credential receipt response",
    )?;
    let canonical_event_json = encode_json_result(
        serde_json::to_value(&event),
        "discovery credential receipt event",
    )?;
    let event_id = DiscoveryEventId::parse(row.10)
        .map_err(|_| corrupted("discovery credential receipt event id is invalid"))?;
    let event_state = parse_discovery_state(&row.15)?;
    let receipt_created_at = parse_timestamp(&row.9, "credential receipt created_at")?;
    let event_created_at = parse_timestamp(&row.18, "credential event created_at")?;
    transition
        .session
        .validate()
        .map_err(|_| corrupted("discovery credential receipt session is invalid"))?;
    validate_sha256(
        "discovery credential receipt request",
        &receipt.request_sha256,
    )
    .map_err(|_| corrupted("discovery credential receipt request hash is invalid"))?;
    if canonical_transition_json != row.7
        || canonical_event_json != row.16
        || row.8 != DISCOVERY_REDACTION_VERSION
        || row.17 != DISCOVERY_REDACTION_VERSION
        || transition.receipt != receipt
        || transition.event != event
        || transition.previous_revision != receipt.expected_revision
        || transition.session.id != *session_id
        || transition.session.revision != receipt.resulting_revision
        || event.id != event_id
        || event.session_id.as_str() != row.11
        || event.session_id != *session_id
        || event.sequence != row.12
        || event.sequence != receipt.event_sequence
        || event.version != row.13
        || event.version != PROVIDER_DISCOVERY_EVENT_VERSION
        || event.session_revision != row.14
        || event.session_revision != receipt.resulting_revision
        || event.state != event_state
        || event.state != transition.session.state
        || event.failure != transition.session.failure
        || event.sequence.saturating_add(1) != transition.session.next_event_sequence
        || event.action_id != receipt.action_id
        || event_created_at != receipt_created_at
    {
        return Err(corrupted(
            "discovery credential authority receipt is detached from its event or response",
        ));
    }
    Ok(DiscoveryAuthorityReceiptRecord {
        receipt,
        transition,
        created_at: receipt_created_at,
        transition_audit_sequence: 0,
        commit_prepared_audit_sequence: None,
    })
}

pub(super) fn validate_ready_discovery_authority_receipt(
    receipt: &DiscoveryAuthorityReceiptRecord,
    ready: &DiscoverySessionSnapshot,
    attempt: &DiscoveryCommitAttemptRecord,
    expected_action_kind: &str,
    expected_revision: u64,
) -> CoreResult<()> {
    if receipt.receipt.action_kind != expected_action_kind
        || receipt.receipt.expected_revision != expected_revision
        || receipt.receipt.resulting_revision != ready.session.revision
        || receipt.receipt.outcome
            != lorepia_domain::discovery::DiscoveryActionReceiptOutcome::Applied
        || receipt.transition.session != ready.session
        || receipt.transition.session.state != DiscoveryState::Ready
        || receipt.transition.session.commit_attempt_id.as_ref() != Some(&attempt.id)
        || receipt.transition.session.commit_plan_sha256.as_deref()
            != Some(attempt.plan_sha256.as_str())
        || receipt.transition.session.committed_connection_id.as_ref()
            != Some(&attempt.plan.connection_id)
    {
        return Err(corrupted(
            "ready discovery credential receipt is detached from its completed authority",
        ));
    }
    Ok(())
}
