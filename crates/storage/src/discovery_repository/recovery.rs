//! Crash recovery classification and native retry authority validation.

mod compensation;
mod compensation_authority;
mod saga;

use super::{
    BTreeSet, Connection, CoreError, CoreResult, CredentialRef, DateTime, DiscoveryActionEnvelope,
    DiscoveryActionId, DiscoveryActionRequired, DiscoveryAuthorityReceiptRecord,
    DiscoveryCommitAttemptRecord, DiscoveryCommitPhase, DiscoveryCompensationStatus,
    DiscoveryCompletedOperationWrite, DiscoveryEffect, DiscoveryInterruptionOutcome,
    DiscoveryJsonUpdate, DiscoveryOperationId, DiscoveryOperationKind, DiscoveryOperationRecord,
    DiscoveryOperationStatus, DiscoveryRecoveryCheckpoint, DiscoveryRecoveryDisposition,
    DiscoveryRecoveryResult, DiscoverySessionId, DiscoverySessionSnapshot,
    DiscoverySideEffectClass, DiscoveryState, DiscoveryTransitionWrite,
    DiscoveryUnknownOutcomeResolution, DurableOperationOutcome, OptionalExtension,
    ProviderDiscoveryAction, Storage, Utc, canonical_json_result, contract_error, corrupted,
    database_error, discovery, discovery_error, load_discovery_authority_receipt_by_action,
    load_discovery_authority_receipt_by_revision, load_operation_by_id, load_session_snapshot,
    params, parse_timestamp, sha256_hex, validate_cancelled_pre_store_interruption_receipt,
    validate_discovery_operation_interrupted_audit, validate_discovery_operation_start_audit,
    validate_discovery_operation_terminal_audit_order_for_receipt,
    validate_discovery_receipt_follows, validate_discovery_unknown_outcome_resolution,
    validate_interrupted_discovery_authority_receipt,
    validate_interrupted_discovery_operation_evidence,
    validate_native_no_effect_operation_start_receipt,
};

use compensation_authority::validate_discovery_compensation_cancellation_chain;
pub(super) use compensation_authority::{
    load_discovery_credential_compensation_operation_id,
    validate_active_discovery_credential_cancellation_chain,
};
#[cfg(test)]
pub(super) use saga::{prepare_compensation_ledger, validate_failed_compensation_ledger};
pub(super) use saga::{reconcile_discovery_saga_ledger, validate_terminal_compensation_transition};

impl Storage {
    /// Classifies unfinished work after a crash and records interruption
    /// transitions. This method never executes or replays an external effect.
    pub fn recover_unfinished_discovery_operations(
        &self,
        recovered_at: DateTime<Utc>,
    ) -> CoreResult<Vec<DiscoveryRecoveryResult>> {
        self.recover_unfinished_discovery_operations_except(recovered_at, &BTreeSet::new())
    }

    /// Recovers every unfinished operation except an exact Core-classified set
    /// of durably resumable operation identifiers.
    ///
    /// Storage never infers resumability from opaque draft JSON. The caller
    /// must derive this set from a validated product snapshot, and every
    /// preserved identifier is still checked against the session's active
    /// operation before it is left untouched.
    pub fn recover_unfinished_discovery_operations_except(
        &self,
        recovered_at: DateTime<Utc>,
        resumable_operation_ids: &BTreeSet<DiscoveryOperationId>,
    ) -> CoreResult<Vec<DiscoveryRecoveryResult>> {
        let unfinished = {
            let connection = self.connection()?;
            discovery::list_unfinished_discovery_operations(&connection).map_err(discovery_error)?
        };
        let mut recovered = Vec::with_capacity(unfinished.len());
        for unfinished_operation in unfinished {
            let session_id = DiscoverySessionId::from(unfinished_operation.session_id);
            let operation_id =
                DiscoveryOperationId::parse(unfinished_operation.id).map_err(contract_error)?;
            let snapshot = self.get_discovery_session(&session_id)?;
            if snapshot.active_operation_id.as_ref() != Some(&operation_id) {
                return Err(corrupted(
                    "unfinished discovery operation is not the session's active operation",
                ));
            }
            let operation = self
                .get_current_discovery_operation(&session_id)?
                .ok_or_else(|| corrupted("unfinished discovery operation cannot be hydrated"))?;
            if resumable_operation_ids.contains(&operation_id) {
                if operation.kind != DiscoveryOperationKind::BuildAssistantManifestDraft
                    || unfinished_operation.operation_kind != "build_assistant_manifest_draft"
                {
                    return Err(corrupted(
                        "only setup assistant operations may bypass startup recovery",
                    ));
                }
                continue;
            }
            let compensation_already_durable = operation.kind
                == DiscoveryOperationKind::Compensation
                && self.compensation_completion_is_durable(&snapshot)?;
            let interruption = match unfinished_operation.disposition {
                DiscoveryRecoveryDisposition::MarkInterrupted => {
                    DiscoveryInterruptionOutcome::ConfirmedNoExternalEffect
                }
                DiscoveryRecoveryDisposition::MarkUnknownOutcome => {
                    DiscoveryInterruptionOutcome::ExternalOutcomeUnknown
                }
            };
            let action = if compensation_already_durable {
                ProviderDiscoveryAction::CompensationSucceeded
            } else {
                ProviderDiscoveryAction::Interrupt {
                    operation: operation.kind,
                    outcome: interruption,
                }
            };
            let request_json =
                canonical_json_result(serde_json::to_value(&action), "discovery recovery action")?;
            let envelope = DiscoveryActionEnvelope {
                id: DiscoveryActionId::new(),
                expected_revision: snapshot.session.revision,
                request_sha256: sha256_hex(request_json.as_bytes()),
                action,
            };
            let transition = snapshot.session.apply(&envelope).map_err(|error| {
                CoreError::invalid(format!("recovery transition failed: {error}"))
            })?;
            let completed_outcome = if compensation_already_durable {
                DurableOperationOutcome::Succeeded
            } else {
                match unfinished_operation.disposition {
                    DiscoveryRecoveryDisposition::MarkInterrupted => {
                        DurableOperationOutcome::Interrupted
                    }
                    DiscoveryRecoveryDisposition::MarkUnknownOutcome => {
                        DurableOperationOutcome::OutcomeUnknown
                    }
                }
            };
            let write = DiscoveryTransitionWrite {
                transition,
                draft: DiscoveryJsonUpdate::Preserve,
                review: DiscoveryJsonUpdate::Preserve,
                new_evidence: Vec::new(),
                new_candidates: Vec::new(),
                approval: None,
                new_operation_id: None,
                completed_operation: Some(DiscoveryCompletedOperationWrite {
                    id: operation_id.clone(),
                    outcome: completed_outcome,
                }),
                prepared_commit: None,
                provider_graph: None,
                occurred_at: recovered_at,
            };
            self.persist_discovery_transition(&write)?;
            recovered.push(DiscoveryRecoveryResult {
                operation_id,
                session_id,
                state: write.transition.session.state,
                event: write.transition.event,
            });
        }
        Ok(recovered)
    }

    fn compensation_completion_is_durable(
        &self,
        snapshot: &DiscoverySessionSnapshot,
    ) -> CoreResult<bool> {
        let attempt_id = snapshot
            .session
            .commit_attempt_id
            .as_ref()
            .ok_or_else(|| corrupted("compensation recovery has no commit attempt"))?;
        let phase = self.get_discovery_commit_attempt(attempt_id)?.phase;
        if phase == DiscoveryCommitPhase::Compensated {
            return Ok(true);
        }
        if phase != DiscoveryCommitPhase::Compensating {
            return Ok(false);
        }
        let steps = self.list_discovery_compensation_steps(attempt_id)?;
        Ok(!steps.is_empty()
            && steps
                .iter()
                .all(|step| step.status == DiscoveryCompensationStatus::Completed))
    }
}

pub(super) fn validate_pre_store_native_credential_interruption(
    connection: &Connection,
    operation: &DiscoveryOperationRecord,
    attempt: &DiscoveryCommitAttemptRecord,
) -> CoreResult<()> {
    let finished_at = operation.finished_at.ok_or_else(|| {
        corrupted("pre-store native credential interruption has no finish timestamp")
    })?;
    let started_at = operation.started_at.ok_or_else(|| {
        corrupted("pre-store native credential interruption has no recovery timestamp")
    })?;
    let attestation_count = connection
        .query_row(
            "SELECT COUNT(*)
             FROM provider_discovery_native_no_effect_attestations
             WHERE operation_id = ?1",
            [operation.id.as_str()],
            |row| row.get::<_, u64>(0),
        )
        .map_err(database_error)?;
    if operation.status != DiscoveryOperationStatus::Interrupted
        || operation.session_id != attempt.session_id
        || operation.expected_revision < attempt.expected_revision.saturating_add(1)
        || operation.created_at > started_at
        || started_at != finished_at
        || operation.updated_at != finished_at
        || attestation_count != 0
    {
        return Err(corrupted(
            "pre-store native credential interruption is not an exact prepared recovery",
        ));
    }
    let start = validate_native_no_effect_operation_start_receipt(
        connection,
        attempt,
        operation.action_id.as_str(),
        operation.expected_revision,
        &operation.request_sha256,
        &operation.created_at.to_rfc3339(),
    )?;
    let interrupted = load_discovery_operation_terminal_receipt(
        connection,
        operation,
        finished_at,
        "operation_interrupted",
    )?;
    let snapshot = load_session_snapshot(connection, attempt.session_id.as_str())?
        .ok_or_else(|| corrupted("pre-store native credential session is missing"))?;
    if interrupted.receipt.expected_revision == start.receipt.resulting_revision {
        validate_discovery_receipt_follows(&start, &interrupted)?;
        validate_interrupted_discovery_authority_receipt(
            &interrupted,
            attempt,
            &snapshot,
            "interrupt",
            operation.expected_revision,
        )?;
    } else {
        validate_discovery_compensation_cancellation_chain(
            connection,
            attempt,
            &start,
            &interrupted,
        )?;
        validate_cancelled_pre_store_interruption_receipt(
            &interrupted,
            attempt,
            &snapshot,
            operation.expected_revision,
        )?;
    }
    let start_audit = validate_discovery_operation_start_audit(connection, operation)?;
    if start_audit.is_some() {
        return Err(corrupted(
            "pre-store native credential interruption has a store-start audit",
        ));
    }
    validate_discovery_operation_terminal_audit_order_for_receipt(
        &start,
        start_audit,
        &interrupted,
    )?;
    validate_discovery_operation_interrupted_audit(connection, operation, &interrupted)?;
    validate_interrupted_discovery_operation_evidence(
        connection,
        attempt,
        operation,
        &interrupted,
        finished_at,
    )
}

fn load_discovery_operation_terminal_receipt(
    connection: &Connection,
    operation: &DiscoveryOperationRecord,
    finished_at: DateTime<Utc>,
    audit_kind: &str,
) -> CoreResult<DiscoveryAuthorityReceiptRecord> {
    let rows = {
        let mut statement = connection
            .prepare(
                "SELECT action_id, session_revision
                 FROM provider_discovery_audit_log
                 WHERE session_id = ?1
                   AND audit_kind = ?2
                   AND subject_id = ?3
                   AND summary_key = 'discovery.audit.operation_interrupted'
                   AND created_at = ?4
                 ORDER BY audit_sequence",
            )
            .map_err(database_error)?;
        statement
            .query_map(
                params![
                    operation.session_id.as_str(),
                    audit_kind,
                    operation.id.as_str(),
                    finished_at.to_rfc3339(),
                ],
                |row| Ok((row.get::<_, String>(0)?, row.get::<_, u64>(1)?)),
            )
            .map_err(database_error)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(database_error)?
    };
    let [(action_id, resulting_revision)] = rows.as_slice() else {
        return Err(corrupted(
            "native credential interruption has no unique terminal audit",
        ));
    };
    let action_id = DiscoveryActionId::parse(action_id.clone())
        .map_err(|_| corrupted("native credential interruption action id is invalid"))?;
    let receipt =
        load_discovery_authority_receipt_by_action(connection, &operation.session_id, &action_id)?;
    if receipt.receipt.resulting_revision != *resulting_revision
        || receipt.created_at != finished_at
    {
        return Err(corrupted(
            "native credential interruption terminal audit is detached from its receipt",
        ));
    }
    Ok(receipt)
}

type LegacyStartedRow = (
    String,
    String,
    String,
    String,
    bool,
    u64,
    u64,
    String,
    String,
    String,
    u64,
    u64,
    u64,
    String,
    String,
    u32,
    u32,
    u32,
);

fn load_legacy_started_cutoff(
    connection: &Connection,
    operation_id: &DiscoveryOperationId,
) -> CoreResult<Option<LegacyStartedRow>> {
    connection
        .query_row(
            "SELECT session_id, commit_attempt_id, commit_plan_sha256,
                    connection_id, session_cancellation_pending,
                    session_revision_at_cutoff,
                    session_next_event_sequence_at_cutoff,
                    start_action_id, start_action_kind, request_sha256,
                    operation_expected_revision,
                    start_transition_audit_sequence,
                    commit_prepared_audit_sequence,
                    operation_created_at, operation_started_at,
                    cutoff_before_schema_version, snapshot_schema_version,
                    redaction_version
             FROM provider_discovery_native_credential_legacy_started_cutoff_snapshots
             WHERE operation_id = ?1",
            [operation_id.as_str()],
            |row| {
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                    row.get(5)?,
                    row.get(6)?,
                    row.get(7)?,
                    row.get(8)?,
                    row.get(9)?,
                    row.get(10)?,
                    row.get(11)?,
                    row.get(12)?,
                    row.get(13)?,
                    row.get(14)?,
                    row.get(15)?,
                    row.get(16)?,
                    row.get(17)?,
                ))
            },
        )
        .optional()
        .map_err(database_error)
}

pub(super) fn validate_legacy_unbound_started_credential_execution(
    connection: &Connection,
    operation: &DiscoveryOperationRecord,
    attempt: &DiscoveryCommitAttemptRecord,
) -> CoreResult<bool> {
    let Some(legacy) = load_legacy_started_cutoff(connection, &operation.id)? else {
        return Ok(false);
    };
    let created_at = parse_timestamp(&legacy.13, "legacy native Started created_at")?;
    let started_at = parse_timestamp(&legacy.14, "legacy native Started started_at")?;
    let snapshot = load_session_snapshot(connection, operation.session_id.as_str())?
        .ok_or_else(|| corrupted("legacy native Started session is missing"))?;
    let detached_physical_or_projection = connection
        .query_row(
            "SELECT EXISTS(
                 SELECT 1 FROM provider_discovery_native_credential_executions
                 WHERE operation_id = ?1
                 UNION ALL
                 SELECT 1 FROM provider_discovery_native_credential_store_attempts
                 WHERE operation_id = ?1
                 UNION ALL
                 SELECT 1 FROM provider_discovery_native_credential_abandoned_reservations
                 WHERE operation_id = ?1
                 UNION ALL
                 SELECT 1 FROM provider_discovery_native_no_effect_execution_bindings
                 WHERE operation_id = ?1
                 UNION ALL
                 SELECT 1 FROM provider_credential_ownership_events
                 WHERE source_kind = 'discovery_commit' AND source_id = ?1
             )",
            [operation.id.as_str()],
            |row| row.get::<_, bool>(0),
        )
        .map_err(database_error)?;
    if legacy.0 != operation.session_id.as_str()
        || legacy.1 != attempt.id.as_str()
        || legacy.2 != attempt.plan_sha256
        || legacy.3 != attempt.plan.connection_id.as_str()
        || legacy.7 != operation.action_id.as_str()
        || legacy.9 != operation.request_sha256
        || legacy.10 != operation.expected_revision
        || created_at != operation.created_at
        || operation.started_at != Some(started_at)
        || operation.created_at > started_at
        || legacy.15 != 37
        || legacy.16 != 1
        || legacy.17 != 1
        || operation.kind != DiscoveryOperationKind::AtomicCommit
        || operation.side_effect_class != DiscoverySideEffectClass::Persistent
        || attempt.session_id != operation.session_id
        || attempt.plan.attempt_id != attempt.id
        || attempt
            .plan
            .credential_ref
            .as_ref()
            .map(CredentialRef::as_str)
            != Some(legacy.3.as_str())
        || detached_physical_or_projection
    {
        return Err(corrupted(
            "legacy native Started cutoff is detached from its immutable commit",
        ));
    }
    let start = validate_native_no_effect_operation_start_receipt(
        connection,
        attempt,
        operation.action_id.as_str(),
        operation.expected_revision,
        &operation.request_sha256,
        &operation.created_at.to_rfc3339(),
    )?;
    if start.receipt.action_kind != legacy.8
        || start.transition_audit_sequence != legacy.11
        || start.commit_prepared_audit_sequence != Some(legacy.12)
        || validate_discovery_operation_start_audit(connection, operation)?.is_none()
    {
        return Err(corrupted(
            "legacy native Started cutoff differs from its exact start history",
        ));
    }
    match operation.status {
        DiscoveryOperationStatus::Started => validate_active_legacy_started_cutoff(
            operation, attempt, &snapshot, &legacy, started_at,
        )?,
        DiscoveryOperationStatus::OutcomeUnknown => validate_recovered_legacy_started_cutoff(
            connection, operation, attempt, &snapshot, &legacy, &start,
        )?,
        _ => {
            return Err(corrupted(
                "legacy native Started cutoff entered an unauthorized terminal state",
            ));
        }
    }
    Ok(true)
}

fn validate_active_legacy_started_cutoff(
    operation: &DiscoveryOperationRecord,
    attempt: &DiscoveryCommitAttemptRecord,
    snapshot: &DiscoverySessionSnapshot,
    legacy: &LegacyStartedRow,
    started_at: DateTime<Utc>,
) -> CoreResult<()> {
    if operation.finished_at.is_some()
        || operation.updated_at != started_at
        || snapshot.session.state != DiscoveryState::Committing
        || snapshot.active_operation_id.as_ref() != Some(&operation.id)
        || snapshot.session.commit_attempt_id.as_ref() != Some(&attempt.id)
        || snapshot.session.commit_plan_sha256.as_deref() != Some(attempt.plan_sha256.as_str())
        || snapshot.session.cancellation_pending != legacy.4
        || snapshot.session.revision != legacy.5
        || snapshot.session.next_event_sequence != legacy.6
    {
        return Err(corrupted(
            "legacy native Started cutoff is detached from its active session",
        ));
    }
    Ok(())
}

fn validate_recovered_legacy_started_cutoff(
    connection: &Connection,
    operation: &DiscoveryOperationRecord,
    attempt: &DiscoveryCommitAttemptRecord,
    snapshot: &DiscoverySessionSnapshot,
    legacy: &LegacyStartedRow,
    start: &DiscoveryAuthorityReceiptRecord,
) -> CoreResult<()> {
    let finished_at = operation.finished_at.ok_or_else(|| {
        corrupted("legacy native Started recovery has no outcome-unknown timestamp")
    })?;
    let unknown = load_discovery_authority_receipt_by_revision(
        connection,
        &operation.session_id,
        legacy.5.saturating_add(1),
    )?;
    if legacy.4 {
        validate_discovery_compensation_cancellation_chain(connection, attempt, start, &unknown)?;
    } else {
        validate_discovery_receipt_follows(start, &unknown)?;
        if unknown.receipt.expected_revision != start.receipt.resulting_revision {
            return Err(corrupted(
                "legacy native Started recovery has an unsealed cancellation gap",
            ));
        }
    }
    let recovery_matches = matches!(
        unknown.receipt.action_kind.as_str(),
        "interrupt" | "external_outcome_became_unknown"
    ) && unknown.receipt.expected_revision >= operation.expected_revision
        && unknown.receipt.resulting_revision
            == unknown.receipt.expected_revision.saturating_add(1)
        && unknown.receipt.outcome
            == lorepia_domain::discovery::DiscoveryActionReceiptOutcome::Applied
        && unknown.transition.session.state == DiscoveryState::UnknownOutcome
        && unknown.transition.session.input == snapshot.session.input
        && unknown.transition.session.commit_attempt_id.as_ref() == Some(&attempt.id)
        && unknown.transition.session.commit_plan_sha256.as_deref()
            == Some(attempt.plan_sha256.as_str())
        && unknown.transition.session.unknown_operation
            == Some(DiscoveryOperationKind::AtomicCommit)
        && unknown.transition.session.recovery.is_none()
        && unknown.transition.session.cancellation_pending == legacy.4
        && unknown.transition.effect == DiscoveryEffect::None
        && unknown.created_at == finished_at
        && operation.updated_at == finished_at
        && snapshot.session.input == unknown.transition.session.input;
    if !recovery_matches {
        return Err(corrupted(
            "legacy native Started recovery is not its exact unknown outcome",
        ));
    }
    validate_discovery_operation_terminal_audit_order_for_receipt(
        start,
        validate_discovery_operation_start_audit(connection, operation)?,
        &unknown,
    )?;
    validate_discovery_operation_interrupted_audit(connection, operation, &unknown).map(|_| ())
}

pub(super) fn validate_native_no_effect_retry_predecessor(
    connection: &Connection,
    attempt: &DiscoveryCommitAttemptRecord,
    restart: &DiscoveryAuthorityReceiptRecord,
) -> CoreResult<()> {
    let predecessor = load_discovery_authority_receipt_by_revision(
        connection,
        &attempt.session_id,
        restart.receipt.expected_revision,
    )?;
    validate_discovery_receipt_follows(&predecessor, restart)?;
    let recovery_matches = matches!(
        predecessor.transition.session.recovery.as_ref(),
        Some(DiscoveryRecoveryCheckpoint {
            interrupted_state: DiscoveryState::Committing,
            operation: DiscoveryOperationKind::AtomicCommit,
        })
    );
    if predecessor.receipt.outcome
        != lorepia_domain::discovery::DiscoveryActionReceiptOutcome::Applied
        || predecessor.receipt.resulting_revision
            != predecessor.receipt.expected_revision.saturating_add(1)
        || predecessor.transition.session.state != DiscoveryState::Interrupted
        || predecessor.transition.session.id != attempt.session_id
        || predecessor.transition.session.input != restart.transition.session.input
        || predecessor.transition.session.commit_attempt_id.as_ref() != Some(&attempt.id)
        || predecessor.transition.session.commit_plan_sha256.as_deref()
            != Some(attempt.plan_sha256.as_str())
        || predecessor.transition.session.manifest_sha256.as_deref()
            != Some(attempt.plan.manifest_sha256.as_str())
        || predecessor.transition.effect != DiscoveryEffect::None
        || predecessor.transition.session.unknown_operation.is_some()
        || predecessor
            .transition
            .session
            .committed_connection_id
            .is_some()
        || predecessor.transition.session.failure.is_some()
        || predecessor
            .transition
            .session
            .active_effect_approval
            .is_some()
        || predecessor.transition.session.cancellation_pending
        || predecessor.transition.event.action_required
            != Some(DiscoveryActionRequired::RestartInterrupted {
                operation: DiscoveryOperationKind::AtomicCommit,
            })
        || !recovery_matches
        || predecessor.created_at > restart.created_at
    {
        return Err(corrupted(
            "native no-effect retry has no exact interrupted predecessor",
        ));
    }
    let predecessor_authority_audit_sequence = match predecessor.receipt.action_kind.as_str() {
        "interrupt" => validate_native_retry_interrupted_operation(
            connection,
            attempt,
            &predecessor,
            predecessor.receipt.expected_revision,
        ),
        "resolve_unknown_outcome" => {
            validate_native_retry_unknown_predecessor(connection, attempt, &predecessor)
        }
        _ => Err(corrupted(
            "native no-effect retry predecessor action is invalid",
        )),
    }?;
    if predecessor_authority_audit_sequence >= restart.transition_audit_sequence {
        return Err(corrupted(
            "native no-effect retry predecessor audit order is invalid",
        ));
    }
    Ok(())
}

fn validate_native_retry_unknown_predecessor(
    connection: &Connection,
    attempt: &DiscoveryCommitAttemptRecord,
    resolution: &DiscoveryAuthorityReceiptRecord,
) -> CoreResult<u64> {
    let approval_audit_sequence = validate_discovery_unknown_outcome_resolution(
        connection,
        attempt,
        resolution,
        &DiscoveryUnknownOutcomeResolution::ConfirmedNoEffect,
    )?;
    let unknown = load_discovery_authority_receipt_by_revision(
        connection,
        &attempt.session_id,
        resolution.receipt.expected_revision,
    )?;
    validate_discovery_receipt_follows(&unknown, resolution)?;
    if !matches!(
        unknown.receipt.action_kind.as_str(),
        "interrupt" | "external_outcome_became_unknown"
    ) || unknown.receipt.outcome
        != lorepia_domain::discovery::DiscoveryActionReceiptOutcome::Applied
        || unknown.receipt.resulting_revision != unknown.receipt.expected_revision.saturating_add(1)
        || unknown.transition.session.state != DiscoveryState::UnknownOutcome
        || unknown.transition.session.id != attempt.session_id
        || unknown.transition.session.input != resolution.transition.session.input
        || unknown.transition.session.unknown_operation
            != Some(DiscoveryOperationKind::AtomicCommit)
        || unknown.transition.session.commit_attempt_id.as_ref() != Some(&attempt.id)
        || unknown.transition.session.commit_plan_sha256.as_deref()
            != Some(attempt.plan_sha256.as_str())
        || unknown.transition.session.manifest_sha256.as_deref()
            != Some(attempt.plan.manifest_sha256.as_str())
        || unknown.transition.effect != DiscoveryEffect::None
        || unknown.transition.session.committed_connection_id.is_some()
        || unknown.transition.session.recovery.is_some()
        || unknown.transition.session.failure.is_some()
        || unknown.transition.session.active_effect_approval.is_some()
        || unknown.transition.session.cancellation_pending
        || unknown.transition.event.action_required
            != Some(DiscoveryActionRequired::ReconcileUnknownOutcome {
                operation: DiscoveryOperationKind::AtomicCommit,
            })
        || unknown.created_at > resolution.created_at
    {
        return Err(corrupted(
            "native no-effect retry resolution has no exact unknown predecessor",
        ));
    }
    let interrupted_audit_sequence = validate_native_retry_unknown_operation(
        connection,
        attempt,
        &unknown,
        unknown.receipt.expected_revision,
    )?;
    if interrupted_audit_sequence >= resolution.transition_audit_sequence
        || resolution.transition_audit_sequence >= approval_audit_sequence
    {
        return Err(corrupted(
            "native no-effect retry resolution audit order is invalid",
        ));
    }
    Ok(approval_audit_sequence)
}

fn load_native_retry_predecessor_operation(
    connection: &Connection,
    attempt: &DiscoveryCommitAttemptRecord,
    expected_revision: u64,
) -> CoreResult<DiscoveryOperationRecord> {
    let ids = {
        let mut statement = connection
            .prepare(
                "SELECT id
                 FROM provider_discovery_operations
                 WHERE session_id = ?1
                   AND operation_kind = 'atomic_commit'
                   AND side_effect_class = 'persistent'
                   AND expected_revision = ?2",
            )
            .map_err(database_error)?;
        statement
            .query_map(
                params![attempt.session_id.as_str(), expected_revision],
                |row| row.get::<_, String>(0),
            )
            .map_err(database_error)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(database_error)?
    };
    let [operation_id] = ids.as_slice() else {
        return Err(corrupted(
            "native no-effect retry predecessor operation is missing or ambiguous",
        ));
    };
    let operation_id = DiscoveryOperationId::parse(operation_id)
        .map_err(|_| corrupted("native no-effect retry predecessor operation id is invalid"))?;
    load_operation_by_id(connection, &operation_id)
}

fn validate_native_retry_interrupted_operation(
    connection: &Connection,
    attempt: &DiscoveryCommitAttemptRecord,
    receipt: &DiscoveryAuthorityReceiptRecord,
    expected_revision: u64,
) -> CoreResult<u64> {
    let operation =
        load_native_retry_predecessor_operation(connection, attempt, expected_revision)?;
    let start = validate_native_no_effect_operation_start_receipt(
        connection,
        attempt,
        operation.action_id.as_str(),
        operation.expected_revision,
        &operation.request_sha256,
        &operation.created_at.to_rfc3339(),
    )?;
    validate_discovery_receipt_follows(&start, receipt)?;
    let finished_at = operation
        .finished_at
        .ok_or_else(|| corrupted("native retry interrupted operation has no finish timestamp"))?;
    let started_at = operation
        .started_at
        .ok_or_else(|| corrupted("native retry interrupted operation has no start timestamp"))?;
    if operation.status != DiscoveryOperationStatus::Interrupted
        || operation.created_at > started_at
        || started_at > finished_at
        || operation.updated_at != finished_at
        || receipt.created_at != finished_at
    {
        return Err(corrupted(
            "native retry interrupted operation is detached from its receipt",
        ));
    }
    let operation_start_audit_sequence =
        validate_discovery_operation_start_audit(connection, &operation)?;
    validate_discovery_operation_terminal_audit_order_for_receipt(
        &start,
        operation_start_audit_sequence,
        receipt,
    )?;
    let interrupted_audit_sequence =
        validate_discovery_operation_interrupted_audit(connection, &operation, receipt)?;
    validate_interrupted_discovery_operation_evidence(
        connection,
        attempt,
        &operation,
        receipt,
        finished_at,
    )?;
    Ok(interrupted_audit_sequence)
}

fn validate_native_retry_unknown_operation(
    connection: &Connection,
    attempt: &DiscoveryCommitAttemptRecord,
    receipt: &DiscoveryAuthorityReceiptRecord,
    expected_revision: u64,
) -> CoreResult<u64> {
    let operation =
        load_native_retry_predecessor_operation(connection, attempt, expected_revision)?;
    let start = validate_native_no_effect_operation_start_receipt(
        connection,
        attempt,
        operation.action_id.as_str(),
        operation.expected_revision,
        &operation.request_sha256,
        &operation.created_at.to_rfc3339(),
    )?;
    validate_discovery_receipt_follows(&start, receipt)?;
    let finished_at = operation
        .finished_at
        .ok_or_else(|| corrupted("native retry unknown operation has no finish timestamp"))?;
    let started_at = operation
        .started_at
        .ok_or_else(|| corrupted("native retry unknown operation has no start timestamp"))?;
    if operation.status != DiscoveryOperationStatus::OutcomeUnknown
        || operation.created_at > started_at
        || started_at > finished_at
        || operation.updated_at != finished_at
        || receipt.created_at != finished_at
    {
        return Err(corrupted(
            "native retry unknown operation is detached from its receipt",
        ));
    }
    let operation_start_audit_sequence =
        validate_discovery_operation_start_audit(connection, &operation)?;
    validate_discovery_operation_terminal_audit_order_for_receipt(
        &start,
        operation_start_audit_sequence,
        receipt,
    )?;
    validate_discovery_operation_interrupted_audit(connection, &operation, receipt)
}
