//! Atomic discovery transition, commit, and compensation ledger reconciliation.

use crate::discovery_repository::{
    CoreError, CoreResult, DateTime, DiscoveryApprovalDecision, DiscoveryApprovalGrant,
    DiscoveryCommitAttemptId, DiscoveryCommitAttemptRecord, DiscoveryCommitPhase,
    DiscoveryOperationKind, DiscoveryState, DiscoveryTransitionWrite,
    DiscoveryUnknownOutcomeResolution, OptionalExtension, Transaction, Utc, contract_error,
    corrupted, database_error, ensure_discovery_attempt_graph_absent, load_commit_attempt,
    load_discovered_provider_graph_rows, load_discovery_selection_restore_revision, params,
    parse_operation_kind, restore_discovery_provider_selection,
    validate_commit_phase_preconditions, verify_discovery_attempt_graph,
};

#[allow(clippy::too_many_lines)]
pub(in crate::discovery_repository) fn reconcile_discovery_saga_ledger(
    transaction: &Transaction<'_>,
    write: &DiscoveryTransitionWrite,
) -> CoreResult<()> {
    let action_kind = write.transition.receipt.action_kind.as_str();
    if write.transition.session.state == DiscoveryState::Compensating
        && matches!(
            action_kind,
            "commit_succeeded" | "compensation_required" | "restart_interrupted"
        )
    {
        prepare_compensation_ledger(transaction, write)?;
        return Ok(());
    }
    if action_kind == "resume_compensation"
        && write.transition.session.state == DiscoveryState::Compensating
    {
        reset_failed_compensation_steps(transaction, write)?;
        return Ok(());
    }
    if matches!(action_kind, "interrupt" | "external_outcome_became_unknown")
        && write.transition.session.state == DiscoveryState::UnknownOutcome
    {
        let operation = write
            .transition
            .session
            .unknown_operation
            .ok_or_else(|| corrupted("unknown-outcome transition has no operation"))?;
        if matches!(
            operation,
            DiscoveryOperationKind::AtomicCommit | DiscoveryOperationKind::Compensation
        ) {
            record_persistent_unknown_outcome(transaction, write, operation)?;
        }
        return Ok(());
    }
    if action_kind == "compensation_failed" {
        return validate_failed_compensation_ledger(transaction, write);
    }
    if action_kind != "resolve_unknown_outcome" {
        if write.approval.as_ref().is_some_and(|approval| {
            matches!(
                approval.grant,
                DiscoveryApprovalGrant::UnknownOutcomeResolution { .. }
            )
        }) {
            return Err(CoreError::invalid(
                "unknown-outcome approval must accompany its reconciliation action",
            ));
        }
        return Ok(());
    }
    let approval = write.approval.as_ref().ok_or_else(|| {
        CoreError::invalid("unknown-outcome reconciliation requires an approval record")
    })?;
    if approval.decision != DiscoveryApprovalDecision::Approved {
        return Err(CoreError::invalid(
            "unknown-outcome reconciliation requires an approved grant",
        ));
    }
    let DiscoveryApprovalGrant::UnknownOutcomeResolution {
        operation,
        resolution,
    } = &approval.grant
    else {
        return Err(CoreError::invalid(
            "unknown-outcome reconciliation has the wrong approval grant",
        ));
    };
    let stored = transaction
        .query_row(
            "SELECT state, unknown_operation, commit_attempt_id
             FROM provider_discovery_sessions
             WHERE id = ?1",
            [write.transition.session.id.as_str()],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, Option<String>>(1)?,
                    row.get::<_, Option<String>>(2)?,
                ))
            },
        )
        .optional()
        .map_err(database_error)?
        .ok_or_else(|| corrupted("unknown-outcome discovery session is missing"))?;
    let stored_operation = stored.1.as_deref().map(parse_operation_kind).transpose()?;
    if stored.0 != "unknown_outcome" || stored_operation.as_ref() != Some(operation) {
        return Err(CoreError::invalid(
            "unknown-outcome approval does not match the durable operation",
        ));
    }
    if !matches!(
        operation,
        DiscoveryOperationKind::AtomicCommit | DiscoveryOperationKind::Compensation
    ) {
        return match resolution {
            DiscoveryUnknownOutcomeResolution::ConfirmedCommitCompleted { .. }
            | DiscoveryUnknownOutcomeResolution::ConfirmedCompensated => Err(CoreError::invalid(
                "non-persistent work cannot use a commit reconciliation",
            )),
            DiscoveryUnknownOutcomeResolution::ConfirmedNoEffect
            | DiscoveryUnknownOutcomeResolution::ManuallyReconciledAsFailed => Ok(()),
        };
    }
    let attempt_id = stored
        .2
        .as_deref()
        .map(DiscoveryCommitAttemptId::parse)
        .transpose()
        .map_err(contract_error)?
        .ok_or_else(|| corrupted("persistent unknown outcome has no commit attempt"))?;
    let attempt = load_commit_attempt(transaction, &attempt_id)?;
    if attempt.session_id != write.transition.session.id
        || write.transition.session.commit_attempt_id.as_ref() != Some(&attempt.id)
    {
        return Err(corrupted(
            "persistent unknown outcome is detached from its commit attempt",
        ));
    }
    match resolution {
        DiscoveryUnknownOutcomeResolution::ConfirmedNoEffect => {
            reconcile_confirmed_no_effect(transaction, write, &attempt, *operation)
        }
        DiscoveryUnknownOutcomeResolution::ConfirmedCommitCompleted { connection_id } => {
            if *operation != DiscoveryOperationKind::AtomicCommit
                || connection_id != &attempt.plan.connection_id
                || attempt.phase != DiscoveryCommitPhase::OutcomeUnknown
            {
                return Err(CoreError::invalid(
                    "confirmed commit completion does not match the unknown attempt",
                ));
            }
            verify_discovery_attempt_graph(transaction, &attempt)?;
            let next_phase = match write.transition.session.state {
                DiscoveryState::Ready => {
                    if attempt.plan.credential_ref.is_some() {
                        DiscoveryCommitPhase::CredentialReferenceApplied
                    } else {
                        DiscoveryCommitPhase::DatabaseApplied
                    }
                }
                DiscoveryState::Compensating => DiscoveryCommitPhase::CompensationRequired,
                _ => {
                    return Err(CoreError::invalid(
                        "confirmed commit completion produced an invalid session state",
                    ));
                }
            };
            set_commit_phase_from_unknown(transaction, &attempt, next_phase, write.occurred_at)
        }
        DiscoveryUnknownOutcomeResolution::ConfirmedCompensated => {
            reconcile_confirmed_compensation_in_transaction(transaction, write)
        }
        DiscoveryUnknownOutcomeResolution::ManuallyReconciledAsFailed => {
            if attempt.plan.credential_ref.is_some() {
                return Err(CoreError::invalid(
                    "manual failure cannot attest native credential deletion",
                ));
            }
            reconcile_confirmed_compensation_in_transaction(transaction, write)
        }
    }
}

fn reset_failed_compensation_steps(
    transaction: &Transaction<'_>,
    write: &DiscoveryTransitionWrite,
) -> CoreResult<()> {
    let attempt_id = write
        .transition
        .session
        .commit_attempt_id
        .as_ref()
        .ok_or_else(|| corrupted("resumed compensation has no commit attempt"))?;
    let attempt = load_commit_attempt(transaction, attempt_id)?;
    if attempt.session_id != write.transition.session.id
        || attempt.phase != DiscoveryCommitPhase::Compensating
    {
        return Err(CoreError::invalid(
            "compensation resume does not match the durable attempt",
        ));
    }
    let unresolved = transaction
        .query_row(
            "SELECT EXISTS(
                 SELECT 1 FROM provider_discovery_compensation_steps
                 WHERE commit_attempt_id = ?1
                   AND status IN ('in_progress', 'outcome_unknown')
             )",
            [attempt.id.as_str()],
            |row| row.get::<_, bool>(0),
        )
        .map_err(database_error)?;
    if unresolved {
        return Err(CoreError::invalid(
            "compensation resume requires every prior step outcome to be known",
        ));
    }
    transaction
        .execute(
            "UPDATE provider_discovery_compensation_steps
             SET status = 'pending',
                 last_failure_json = NULL,
                 updated_at = ?2,
                 completed_at = NULL
             WHERE commit_attempt_id = ?1 AND status = 'failed'",
            params![attempt.id.as_str(), write.occurred_at.to_rfc3339()],
        )
        .map_err(database_error)?;
    Ok(())
}

pub(in crate::discovery_repository) fn prepare_compensation_ledger(
    transaction: &Transaction<'_>,
    write: &DiscoveryTransitionWrite,
) -> CoreResult<()> {
    let attempt_id = write
        .transition
        .session
        .commit_attempt_id
        .as_ref()
        .ok_or_else(|| corrupted("compensating transition has no commit attempt"))?;
    let attempt = load_commit_attempt(transaction, attempt_id)?;
    if attempt.session_id != write.transition.session.id {
        return Err(CoreError::invalid(
            "compensation commit attempt belongs to another discovery session",
        ));
    }
    if write.transition.receipt.action_kind == "restart_interrupted"
        && matches!(
            attempt.phase,
            DiscoveryCommitPhase::CompensationRequired | DiscoveryCommitPhase::Compensating
        )
    {
        return Ok(());
    }
    if !(matches!(
        attempt.phase,
        DiscoveryCommitPhase::DatabaseApplied | DiscoveryCommitPhase::CredentialReferenceApplied
    ) || (attempt.phase == DiscoveryCommitPhase::Prepared
        && matches!(
            write.transition.receipt.action_kind.as_str(),
            "compensation_required" | "restart_interrupted"
        )))
    {
        return Err(CoreError::invalid(
            "compensation can start only after a durably applied commit phase",
        ));
    }
    let changed = transaction
        .execute(
            "UPDATE provider_discovery_commit_attempts
             SET phase = 'compensation_required', updated_at = ?2, completed_at = NULL
             WHERE id = ?1 AND phase = ?3",
            params![
                attempt.id.as_str(),
                write.occurred_at.to_rfc3339(),
                attempt.phase.as_str(),
            ],
        )
        .map_err(database_error)?;
    if changed != 1 {
        return Err(CoreError::invalid(
            "compensation commit attempt changed concurrently",
        ));
    }
    Ok(())
}

fn record_persistent_unknown_outcome(
    transaction: &Transaction<'_>,
    write: &DiscoveryTransitionWrite,
    operation: DiscoveryOperationKind,
) -> CoreResult<()> {
    let attempt_id = write
        .transition
        .session
        .commit_attempt_id
        .as_ref()
        .ok_or_else(|| corrupted("persistent unknown outcome has no commit attempt"))?;
    let attempt = load_commit_attempt(transaction, attempt_id)?;
    if attempt.session_id != write.transition.session.id {
        return Err(corrupted(
            "persistent unknown outcome has a foreign commit attempt",
        ));
    }
    let allowed_phase = match operation {
        DiscoveryOperationKind::AtomicCommit => matches!(
            attempt.phase,
            DiscoveryCommitPhase::Prepared
                | DiscoveryCommitPhase::DatabaseApplied
                | DiscoveryCommitPhase::CredentialReferenceApplied
        ),
        DiscoveryOperationKind::Compensation => matches!(
            attempt.phase,
            DiscoveryCommitPhase::CompensationRequired | DiscoveryCommitPhase::Compensating
        ),
        _ => false,
    };
    if !allowed_phase {
        return Err(CoreError::invalid(
            "persistent operation cannot become unknown from its durable commit phase",
        ));
    }
    if operation == DiscoveryOperationKind::Compensation {
        let in_progress_steps = transaction
            .query_row(
                "SELECT COUNT(*)
                 FROM provider_discovery_compensation_steps
                 WHERE commit_attempt_id = ?1 AND status = 'in_progress'",
                [attempt.id.as_str()],
                |row| row.get::<_, u32>(0),
            )
            .map_err(database_error)?;
        if in_progress_steps > 1 {
            return Err(corrupted("more than one compensation step was in progress"));
        }
        transaction
            .execute(
                "UPDATE provider_discovery_compensation_steps
                 SET status = 'outcome_unknown',
                     updated_at = ?2
                 WHERE commit_attempt_id = ?1 AND status = 'in_progress'",
                params![attempt.id.as_str(), write.occurred_at.to_rfc3339()],
            )
            .map_err(database_error)?;
    }
    let changed = transaction
        .execute(
            "UPDATE provider_discovery_commit_attempts
             SET phase = 'outcome_unknown', updated_at = ?2, completed_at = NULL
             WHERE id = ?1 AND phase = ?3",
            params![
                attempt.id.as_str(),
                write.occurred_at.to_rfc3339(),
                attempt.phase.as_str(),
            ],
        )
        .map_err(database_error)?;
    if changed != 1 {
        return Err(CoreError::invalid(
            "unknown-outcome commit attempt changed concurrently",
        ));
    }
    Ok(())
}

fn reconcile_confirmed_no_effect(
    transaction: &Transaction<'_>,
    write: &DiscoveryTransitionWrite,
    attempt: &DiscoveryCommitAttemptRecord,
    operation: DiscoveryOperationKind,
) -> CoreResult<()> {
    if attempt.phase != DiscoveryCommitPhase::OutcomeUnknown {
        return Err(CoreError::invalid(
            "confirmed no-effect resolution requires an unknown commit phase",
        ));
    }
    match operation {
        DiscoveryOperationKind::AtomicCommit => {
            reconcile_atomic_commit_confirmed_no_effect(transaction, write, attempt)
        }
        DiscoveryOperationKind::Compensation => {
            reconcile_compensation_confirmed_no_effect(transaction, write, attempt)
        }
        _ => Err(CoreError::invalid(
            "confirmed no-effect ledger reconciliation requires persistent work",
        )),
    }
}

fn reconcile_atomic_commit_confirmed_no_effect(
    transaction: &Transaction<'_>,
    write: &DiscoveryTransitionWrite,
    attempt: &DiscoveryCommitAttemptRecord,
) -> CoreResult<()> {
    ensure_discovery_attempt_graph_absent(transaction, attempt)?;
    let touched_steps = transaction
        .query_row(
            "SELECT COUNT(*)
             FROM provider_discovery_compensation_steps
             WHERE commit_attempt_id = ?1 AND status <> 'pending'",
            [attempt.id.as_str()],
            |row| row.get::<_, u32>(0),
        )
        .map_err(database_error)?;
    if touched_steps != 0 {
        return Err(CoreError::invalid(
            "no-effect commit has already touched its compensation recipe",
        ));
    }
    match write.transition.session.state {
        DiscoveryState::Interrupted => {
            let next_phase =
                if write
                    .transition
                    .session
                    .recovery
                    .as_ref()
                    .is_some_and(|checkpoint| {
                        checkpoint.operation == DiscoveryOperationKind::Compensation
                    })
                {
                    DiscoveryCommitPhase::CompensationRequired
                } else {
                    DiscoveryCommitPhase::Prepared
                };
            set_commit_phase_from_unknown(transaction, attempt, next_phase, write.occurred_at)
        }
        DiscoveryState::Cancelled => {
            restore_discovery_provider_selection(
                transaction,
                &attempt.plan.previous_selection,
                None,
            )?;
            complete_no_effect_recipe(transaction, attempt, write.occurred_at)
        }
        _ => Err(CoreError::invalid(
            "confirmed no-effect commit produced an invalid session state",
        )),
    }
}

fn reconcile_compensation_confirmed_no_effect(
    transaction: &Transaction<'_>,
    write: &DiscoveryTransitionWrite,
    attempt: &DiscoveryCommitAttemptRecord,
) -> CoreResult<()> {
    transaction
        .execute(
            "UPDATE provider_discovery_compensation_steps
             SET status = 'pending',
                 last_failure_json = NULL,
                 updated_at = ?2,
                 completed_at = NULL
             WHERE commit_attempt_id = ?1 AND status = 'outcome_unknown'",
            params![attempt.id.as_str(), write.occurred_at.to_rfc3339()],
        )
        .map_err(database_error)?;
    let in_progress = transaction
        .query_row(
            "SELECT EXISTS(
                 SELECT 1 FROM provider_discovery_compensation_steps
                 WHERE commit_attempt_id = ?1 AND status = 'in_progress'
             )",
            [attempt.id.as_str()],
            |row| row.get::<_, bool>(0),
        )
        .map_err(database_error)?;
    if in_progress {
        return Err(corrupted(
            "confirmed no-effect compensation left a step in progress",
        ));
    }
    if write.transition.session.state != DiscoveryState::Interrupted {
        return Err(CoreError::invalid(
            "incomplete compensation cannot be terminalized as no-effect",
        ));
    }
    set_commit_phase_from_unknown(
        transaction,
        attempt,
        DiscoveryCommitPhase::Compensating,
        write.occurred_at,
    )
}

fn set_commit_phase_from_unknown(
    transaction: &Transaction<'_>,
    attempt: &DiscoveryCommitAttemptRecord,
    next: DiscoveryCommitPhase,
    updated_at: DateTime<Utc>,
) -> CoreResult<()> {
    let changed = transaction
        .execute(
            "UPDATE provider_discovery_commit_attempts
             SET phase = ?2, updated_at = ?3, completed_at = NULL
             WHERE id = ?1 AND phase = 'outcome_unknown'",
            params![attempt.id.as_str(), next.as_str(), updated_at.to_rfc3339()],
        )
        .map_err(database_error)?;
    if changed != 1 {
        return Err(CoreError::invalid(
            "unknown commit attempt changed concurrently",
        ));
    }
    Ok(())
}

fn complete_no_effect_recipe(
    transaction: &Transaction<'_>,
    attempt: &DiscoveryCommitAttemptRecord,
    completed_at: DateTime<Utc>,
) -> CoreResult<()> {
    transaction
        .execute(
            "UPDATE provider_discovery_compensation_steps
             SET status = 'completed',
                 last_failure_json = NULL,
                 updated_at = ?2,
                 completed_at = ?2
             WHERE commit_attempt_id = ?1",
            params![attempt.id.as_str(), completed_at.to_rfc3339()],
        )
        .map_err(database_error)?;
    let changed = transaction
        .execute(
            "UPDATE provider_discovery_commit_attempts
             SET phase = 'compensated', updated_at = ?2, completed_at = ?2
             WHERE id = ?1 AND phase = 'outcome_unknown'",
            params![attempt.id.as_str(), completed_at.to_rfc3339()],
        )
        .map_err(database_error)?;
    if changed != 1 {
        return Err(CoreError::invalid(
            "no-effect commit attempt changed concurrently",
        ));
    }
    Ok(())
}

pub(in crate::discovery_repository) fn validate_failed_compensation_ledger(
    transaction: &Transaction<'_>,
    write: &DiscoveryTransitionWrite,
) -> CoreResult<()> {
    let attempt_id = write
        .transition
        .session
        .commit_attempt_id
        .as_ref()
        .ok_or_else(|| corrupted("failed compensation has no commit attempt"))?;
    let attempt = load_commit_attempt(transaction, attempt_id)?;
    if attempt.session_id != write.transition.session.id
        || write.transition.session.commit_plan_sha256.as_deref()
            != Some(attempt.plan_sha256.as_str())
    {
        return Err(CoreError::invalid(
            "failed compensation does not own its commit attempt",
        ));
    }
    if attempt.phase != DiscoveryCommitPhase::Compensating {
        return Err(CoreError::invalid(
            "failed compensation requires the compensating commit phase",
        ));
    }
    let unresolved_step = transaction
        .query_row(
            "SELECT EXISTS(
                 SELECT 1 FROM provider_discovery_compensation_steps
                 WHERE commit_attempt_id = ?1
                   AND status IN ('in_progress', 'outcome_unknown')
             )",
            [attempt.id.as_str()],
            |row| row.get::<_, bool>(0),
        )
        .map_err(database_error)?;
    if unresolved_step {
        return Err(CoreError::invalid(
            "failed compensation must first durably fail its active step",
        ));
    }
    Ok(())
}

pub(in crate::discovery_repository) fn validate_terminal_compensation_transition(
    transaction: &Transaction<'_>,
    write: &DiscoveryTransitionWrite,
) -> CoreResult<()> {
    let action_is_success = write.transition.receipt.action_kind == "compensation_succeeded";
    let result_is_terminal_failure = matches!(
        write.transition.session.state,
        DiscoveryState::Cancelled | DiscoveryState::Failed
    );
    if !action_is_success
        && (!result_is_terminal_failure || write.transition.session.commit_attempt_id.is_none())
    {
        return Ok(());
    }
    let attempt_id = write
        .transition
        .session
        .commit_attempt_id
        .as_ref()
        .ok_or_else(|| corrupted("terminal compensation transition has no commit attempt"))?;
    let attempt = load_commit_attempt(transaction, attempt_id)?;
    if attempt.session_id != write.transition.session.id {
        return Err(corrupted(
            "terminal compensation commit attempt belongs to another session",
        ));
    }
    if action_is_success && attempt.phase == DiscoveryCommitPhase::Compensating {
        validate_commit_phase_preconditions(
            transaction,
            &attempt,
            DiscoveryCommitPhase::Compensated,
        )?;
        let changed = transaction
            .execute(
                "UPDATE provider_discovery_commit_attempts
                 SET phase = 'compensated', updated_at = ?2, completed_at = ?2
                 WHERE id = ?1 AND phase = 'compensating'",
                params![attempt.id.as_str(), write.occurred_at.to_rfc3339()],
            )
            .map_err(database_error)?;
        if changed != 1 {
            return Err(CoreError::invalid(
                "terminal compensation attempt changed concurrently",
            ));
        }
    } else if attempt.phase != DiscoveryCommitPhase::Compensated {
        return Err(CoreError::invalid(
            "terminal discovery transition would abandon an incomplete compensation recipe",
        ));
    }
    Ok(())
}

fn reconcile_confirmed_compensation_in_transaction(
    transaction: &Transaction<'_>,
    write: &DiscoveryTransitionWrite,
) -> CoreResult<()> {
    let attempt_id = write
        .transition
        .session
        .commit_attempt_id
        .as_ref()
        .ok_or_else(|| corrupted("confirmed compensation has no commit attempt"))?;
    let attempt = load_commit_attempt(transaction, attempt_id)?;
    if attempt.session_id != write.transition.session.id
        || !matches!(
            attempt.phase,
            DiscoveryCommitPhase::Compensating | DiscoveryCommitPhase::OutcomeUnknown
        )
    {
        return Err(CoreError::invalid(
            "confirmed compensation does not match an unresolved durable attempt",
        ));
    }
    let graph = load_discovered_provider_graph_rows(
        transaction,
        &attempt.plan.template_id,
        attempt.plan.template_version,
        &attempt.plan.connection_id,
    )?;
    if graph.is_some() {
        return Err(CoreError::invalid(
            "cannot confirm compensation while the provider graph still exists",
        ));
    }
    for route_id in &attempt.plan.model_route_ids {
        let route_exists = transaction
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM provider_models WHERE id = ?1)",
                [route_id.as_str()],
                |row| row.get::<_, bool>(0),
            )
            .map_err(database_error)?;
        if route_exists {
            return Err(corrupted(
                "confirmed compensation left a planned model route behind",
            ));
        }
    }
    let expected_selection_revision =
        load_discovery_selection_restore_revision(transaction, &attempt.id)?;
    restore_discovery_provider_selection(
        transaction,
        &attempt.plan.previous_selection,
        expected_selection_revision,
    )?;
    transaction
        .execute(
            "UPDATE provider_discovery_compensation_steps
             SET status = 'completed',
                 last_failure_json = NULL,
                 updated_at = ?2,
                 completed_at = ?2
             WHERE commit_attempt_id = ?1
               AND status <> 'completed'",
            params![attempt.id.as_str(), write.occurred_at.to_rfc3339()],
        )
        .map_err(database_error)?;
    let changed = transaction
        .execute(
            "UPDATE provider_discovery_commit_attempts
             SET phase = 'compensated', updated_at = ?2, completed_at = ?2
             WHERE id = ?1 AND phase IN ('compensating', 'outcome_unknown')",
            params![attempt.id.as_str(), write.occurred_at.to_rfc3339()],
        )
        .map_err(database_error)?;
    if changed != 1 {
        return Err(CoreError::invalid(
            "confirmed compensation attempt changed concurrently",
        ));
    }
    Ok(())
}
