//! Reverse compensation transitions and provider graph cleanup.

use crate::discovery_repository::{
    BTreeSet, CoreError, CoreResult, DateTime, DiscoveryCommitAttemptId, DiscoveryCommitPhase,
    DiscoveryCompensationKind, DiscoveryCompensationStatus, DiscoveryCompensationStep,
    DiscoveryCompensationTarget, DiscoveryOperationKind, DiscoverySessionId, DiscoveryState,
    DiscoveryTransitionWrite, DurableOperationOutcome, OptionalExtension,
    PersistDiscoveryTransition, Storage, Transaction, TransactionBehavior, Utc, append_audit,
    clear_provider_selections_for_discovery_compensation, compensation_status_transition_allowed,
    contract_error, corrupted, database_error, encode_json_result, ensure_foreign_keys_clean,
    enum_wire_result, graph_ownership_audit_hash, graph_template_was_created, load_commit_attempt,
    load_discovered_provider_graph_rows, params, persist_transition_in_transaction,
    record_discovery_selection_restore_authority, require_started_session_operation,
    stored_provider_graph_ownership_hash, validate_identifier, validate_transition_write,
};

impl Storage {
    /// Advances a compensation step without splitting failure state.
    ///
    /// A failed or unknown step must use the matching atomic transition API so
    /// the step, commit attempt, operation, session, receipt, audit, and outbox
    /// event commit together.
    #[allow(clippy::too_many_lines)]
    pub fn update_discovery_compensation_status(
        &self,
        step_id: &str,
        expected: DiscoveryCompensationStatus,
        next: DiscoveryCompensationStatus,
        failure: Option<&lorepia_domain::discovery::DiscoveryFailure>,
        updated_at: DateTime<Utc>,
    ) -> CoreResult<()> {
        validate_identifier("discovery compensation step id", step_id, 128)?;
        if matches!(
            next,
            DiscoveryCompensationStatus::Failed | DiscoveryCompensationStatus::OutcomeUnknown
        ) || failure.is_some()
        {
            return Err(CoreError::invalid(
                "compensation failures and unknown outcomes require their atomic step-and-session APIs",
            ));
        }
        if !compensation_status_transition_allowed(expected, next) {
            return Err(CoreError::invalid(
                "invalid discovery compensation status transition",
            ));
        }
        let mut connection = self.connection()?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(database_error)?;
        let context = transaction
            .query_row(
                "SELECT attempt.session_id, session.revision, attempt.phase,
                        session.state, step.step_kind, step.ordinal, attempt.id,
                        step.step_json
                 FROM provider_discovery_compensation_steps AS step
                 JOIN provider_discovery_commit_attempts AS attempt
                   ON attempt.id = step.commit_attempt_id
                 JOIN provider_discovery_sessions AS session
                   ON session.id = attempt.session_id
                 WHERE step.id = ?1 AND step.status = ?2",
                params![step_id, expected.as_str()],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, u64>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, String>(3)?,
                        row.get::<_, String>(4)?,
                        row.get::<_, u32>(5)?,
                        row.get::<_, String>(6)?,
                        row.get::<_, String>(7)?,
                    ))
                },
            )
            .optional()
            .map_err(database_error)?
            .ok_or_else(|| {
                CoreError::invalid("compensation step was missing or changed concurrently")
            })?;
        if context.2 != "compensating" || context.3 != "compensating" {
            return Err(CoreError::invalid(
                "compensation step work is not authorized by the active commit and session",
            ));
        }
        require_started_session_operation(
            &transaction,
            &DiscoverySessionId::from(context.0.clone()),
            "compensation",
        )?;
        let attempt_id =
            DiscoveryCommitAttemptId::parse(context.6.clone()).map_err(contract_error)?;
        let attempt = load_commit_attempt(&transaction, &attempt_id)?;
        let step = serde_json::from_str::<DiscoveryCompensationStep>(&context.7)
            .map_err(|_| corrupted("stored compensation step is invalid"))?;
        step.validate_against(&attempt.plan)
            .map_err(|_| corrupted("stored compensation target differs from its commit plan"))?;
        let stored_kind = enum_wire_result(
            serde_json::to_value(step.kind),
            "stored discovery compensation kind",
        )?;
        if stored_kind != context.4 || step.ordinal != context.5 {
            return Err(corrupted(
                "stored compensation columns differ from their typed step",
            ));
        }
        if next == DiscoveryCompensationStatus::Completed
            && step.kind != DiscoveryCompensationKind::RemoveCredentialSlot
        {
            return Err(CoreError::invalid(
                "only native credential removal may use generic compensation completion",
            ));
        }
        if next == DiscoveryCompensationStatus::InProgress {
            let higher_step_incomplete = transaction
                .query_row(
                    "SELECT EXISTS(
                         SELECT 1
                         FROM provider_discovery_compensation_steps
                         WHERE commit_attempt_id = ?1
                           AND ordinal > ?2
                           AND status <> 'completed'
                     )",
                    params![attempt.id.as_str(), context.5],
                    |row| row.get::<_, bool>(0),
                )
                .map_err(database_error)?;
            if higher_step_incomplete {
                return Err(CoreError::invalid(
                    "compensation steps must start in reverse ordinal order",
                ));
            }
        }
        let completed_at =
            (next == DiscoveryCompensationStatus::Completed).then(|| updated_at.to_rfc3339());
        let changed = transaction
            .execute(
                "UPDATE provider_discovery_compensation_steps
                 SET status = ?2,
                     attempt_count = attempt_count + CASE WHEN ?2 = 'in_progress' THEN 1 ELSE 0 END,
                     last_failure_json = ?3,
                     updated_at = ?4,
                     completed_at = ?5
                 WHERE id = ?1 AND status = ?6",
                params![
                    step_id,
                    next.as_str(),
                    Option::<String>::None,
                    updated_at.to_rfc3339(),
                    completed_at,
                    expected.as_str(),
                ],
            )
            .map_err(database_error)?;
        if changed != 1 {
            return Err(CoreError::invalid("compensation step changed concurrently"));
        }
        if next == DiscoveryCompensationStatus::InProgress {
            append_audit(
                &transaction,
                &context.0,
                context.1,
                "compensation_started",
                None,
                Some(step_id),
                "discovery.audit.compensation_started",
                updated_at,
            )?;
        }
        transaction.commit().map_err(database_error)
    }

    /// Atomically records a compensation-step failure and its domain
    /// transition. This prevents a crash from leaving a failed step without
    /// the session failure that makes `ResumeCompensation` reachable.
    #[allow(clippy::too_many_lines)]
    pub fn fail_discovery_compensation_and_persist_transition(
        &self,
        step_id: &str,
        write: &DiscoveryTransitionWrite,
    ) -> CoreResult<PersistDiscoveryTransition> {
        validate_identifier("discovery compensation step id", step_id, 128)?;
        validate_transition_write(write)?;
        let transition = &write.transition;
        let failure =
            transition.session.failure.as_ref().ok_or_else(|| {
                CoreError::invalid("compensation failure transition has no failure")
            })?;
        failure.validate().map_err(contract_error)?;
        if transition.receipt.action_kind != "compensation_failed"
            || transition.session.state != DiscoveryState::Compensating
            || transition.event.failure.as_ref() != Some(failure)
            || write
                .completed_operation
                .as_ref()
                .is_none_or(|completed| completed.outcome != DurableOperationOutcome::Failed)
        {
            return Err(CoreError::invalid(
                "atomic compensation failure requires the exact failed operation transition",
            ));
        }
        let failure_json =
            encode_json_result(serde_json::to_value(failure), "compensation failure")?;
        let mut connection = self.connection()?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(database_error)?;
        let receipt_exists = transaction
            .query_row(
                "SELECT EXISTS(
                     SELECT 1 FROM provider_discovery_action_receipts
                     WHERE action_id = ?1
                 )",
                [transition.receipt.action_id.as_str()],
                |row| row.get::<_, bool>(0),
            )
            .map_err(database_error)?;
        if !receipt_exists {
            let context = transaction
                .query_row(
                    "SELECT attempt.session_id, session.revision, attempt.phase,
                            session.state, step.step_kind, step.ordinal, attempt.id,
                            step.step_json, session.commit_attempt_id,
                            session.commit_plan_sha256
                     FROM provider_discovery_compensation_steps AS step
                     JOIN provider_discovery_commit_attempts AS attempt
                       ON attempt.id = step.commit_attempt_id
                     JOIN provider_discovery_sessions AS session
                       ON session.id = attempt.session_id
                     WHERE step.id = ?1 AND step.status = 'in_progress'",
                    [step_id],
                    |row| {
                        Ok((
                            row.get::<_, String>(0)?,
                            row.get::<_, u64>(1)?,
                            row.get::<_, String>(2)?,
                            row.get::<_, String>(3)?,
                            row.get::<_, String>(4)?,
                            row.get::<_, u32>(5)?,
                            row.get::<_, String>(6)?,
                            row.get::<_, String>(7)?,
                            row.get::<_, Option<String>>(8)?,
                            row.get::<_, Option<String>>(9)?,
                        ))
                    },
                )
                .optional()
                .map_err(database_error)?
                .ok_or_else(|| {
                    CoreError::invalid(
                        "compensation step was missing, not in progress, or changed concurrently",
                    )
                })?;
            if context.0 != transition.session.id.as_str()
                || context.2 != "compensating"
                || context.3 != "compensating"
                || context.8.as_deref() != Some(context.6.as_str())
                || context.9.as_deref() != transition.session.commit_plan_sha256.as_deref()
            {
                return Err(CoreError::invalid(
                    "compensation failure does not match the active session and commit",
                ));
            }
            require_started_session_operation(
                &transaction,
                &transition.session.id,
                "compensation",
            )?;
            let attempt_id =
                DiscoveryCommitAttemptId::parse(context.6.clone()).map_err(contract_error)?;
            let attempt = load_commit_attempt(&transaction, &attempt_id)?;
            if attempt.session_id != transition.session.id
                || attempt.plan_sha256 != context.9.as_deref().unwrap_or_default()
            {
                return Err(corrupted(
                    "compensation failure commit binding is inconsistent",
                ));
            }
            let step = serde_json::from_str::<DiscoveryCompensationStep>(&context.7)
                .map_err(|_| corrupted("stored compensation step is invalid"))?;
            step.validate_against(&attempt.plan).map_err(|_| {
                corrupted("stored compensation target differs from its commit plan")
            })?;
            let stored_kind = enum_wire_result(
                serde_json::to_value(step.kind),
                "stored discovery compensation kind",
            )?;
            if stored_kind != context.4 || step.ordinal != context.5 {
                return Err(corrupted(
                    "stored compensation columns differ from their typed step",
                ));
            }
            let changed = transaction
                .execute(
                    "UPDATE provider_discovery_compensation_steps
                     SET status = 'failed',
                         last_failure_json = ?2,
                         updated_at = ?3,
                         completed_at = NULL
                     WHERE id = ?1 AND status = 'in_progress'",
                    params![step_id, failure_json, write.occurred_at.to_rfc3339()],
                )
                .map_err(database_error)?;
            if changed != 1 {
                return Err(CoreError::invalid("compensation step changed concurrently"));
            }
        }

        let result = persist_transition_in_transaction(&transaction, write, None)?;
        let stored = transaction
            .query_row(
                "SELECT step.status, step.last_failure_json, attempt.session_id,
                        session.commit_attempt_id, session.commit_plan_sha256,
                        step.step_kind, step.ordinal, step.step_json, attempt.id
                 FROM provider_discovery_compensation_steps AS step
                 JOIN provider_discovery_commit_attempts AS attempt
                   ON attempt.id = step.commit_attempt_id
                 JOIN provider_discovery_sessions AS session
                   ON session.id = attempt.session_id
                 WHERE step.id = ?1",
                [step_id],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, Option<String>>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, Option<String>>(3)?,
                        row.get::<_, Option<String>>(4)?,
                        row.get::<_, String>(5)?,
                        row.get::<_, u32>(6)?,
                        row.get::<_, String>(7)?,
                        row.get::<_, String>(8)?,
                    ))
                },
            )
            .optional()
            .map_err(database_error)?
            .ok_or_else(|| corrupted("atomically failed compensation step disappeared"))?;
        if stored.0 != "failed"
            || stored.1.as_deref() != Some(failure_json.as_str())
            || stored.2 != transition.session.id.as_str()
            || stored.3.as_deref() != Some(stored.8.as_str())
            || stored.4.as_deref() != transition.session.commit_plan_sha256.as_deref()
        {
            return Err(corrupted(
                "atomically failed compensation step does not match its transition",
            ));
        }
        let attempt_id = DiscoveryCommitAttemptId::parse(stored.8).map_err(contract_error)?;
        let attempt = load_commit_attempt(&transaction, &attempt_id)?;
        let step = serde_json::from_str::<DiscoveryCompensationStep>(&stored.7)
            .map_err(|_| corrupted("stored compensation step is invalid"))?;
        step.validate_against(&attempt.plan)
            .map_err(|_| corrupted("stored compensation target differs from its commit plan"))?;
        let stored_kind = enum_wire_result(
            serde_json::to_value(step.kind),
            "stored discovery compensation kind",
        )?;
        if stored_kind != stored.5 || step.ordinal != stored.6 {
            return Err(corrupted(
                "stored compensation columns differ from their typed step",
            ));
        }
        transaction.commit().map_err(database_error)?;
        Ok(result)
    }

    /// Atomically records an unknown compensation outcome across the step,
    /// commit attempt, operation, session, receipt, and event outbox.
    #[allow(clippy::too_many_lines)]
    pub fn mark_discovery_compensation_unknown_and_persist_transition(
        &self,
        step_id: &str,
        write: &DiscoveryTransitionWrite,
    ) -> CoreResult<PersistDiscoveryTransition> {
        validate_identifier("discovery compensation step id", step_id, 128)?;
        validate_transition_write(write)?;
        let transition = &write.transition;
        if transition.receipt.action_kind != "external_outcome_became_unknown"
            || transition.session.state != DiscoveryState::UnknownOutcome
            || transition.session.unknown_operation != Some(DiscoveryOperationKind::Compensation)
            || transition.session.failure.is_some()
            || write.completed_operation.as_ref().is_none_or(|completed| {
                completed.outcome != DurableOperationOutcome::OutcomeUnknown
            })
        {
            return Err(CoreError::invalid(
                "atomic compensation unknown outcome requires the exact persistent transition",
            ));
        }

        let mut connection = self.connection()?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(database_error)?;
        let receipt_exists = transaction
            .query_row(
                "SELECT EXISTS(
                     SELECT 1 FROM provider_discovery_action_receipts
                     WHERE action_id = ?1
                 )",
                [transition.receipt.action_id.as_str()],
                |row| row.get::<_, bool>(0),
            )
            .map_err(database_error)?;
        if !receipt_exists {
            let context = transaction
                .query_row(
                    "SELECT attempt.session_id, attempt.phase, session.state,
                            step.step_kind, step.ordinal, attempt.id, step.step_json,
                            session.commit_attempt_id, session.commit_plan_sha256
                     FROM provider_discovery_compensation_steps AS step
                     JOIN provider_discovery_commit_attempts AS attempt
                       ON attempt.id = step.commit_attempt_id
                     JOIN provider_discovery_sessions AS session
                       ON session.id = attempt.session_id
                     WHERE step.id = ?1 AND step.status = 'in_progress'",
                    [step_id],
                    |row| {
                        Ok((
                            row.get::<_, String>(0)?,
                            row.get::<_, String>(1)?,
                            row.get::<_, String>(2)?,
                            row.get::<_, String>(3)?,
                            row.get::<_, u32>(4)?,
                            row.get::<_, String>(5)?,
                            row.get::<_, String>(6)?,
                            row.get::<_, Option<String>>(7)?,
                            row.get::<_, Option<String>>(8)?,
                        ))
                    },
                )
                .optional()
                .map_err(database_error)?
                .ok_or_else(|| {
                    CoreError::invalid(
                        "compensation step was missing, not in progress, or changed concurrently",
                    )
                })?;
            if context.0 != transition.session.id.as_str()
                || context.1 != "compensating"
                || context.2 != "compensating"
                || context.7.as_deref() != Some(context.5.as_str())
                || context.8.as_deref() != transition.session.commit_plan_sha256.as_deref()
            {
                return Err(CoreError::invalid(
                    "unknown compensation outcome does not match the active session and commit",
                ));
            }
            require_started_session_operation(
                &transaction,
                &transition.session.id,
                "compensation",
            )?;
            let attempt_id =
                DiscoveryCommitAttemptId::parse(context.5.clone()).map_err(contract_error)?;
            let attempt = load_commit_attempt(&transaction, &attempt_id)?;
            if attempt.session_id != transition.session.id
                || attempt.plan_sha256 != context.8.as_deref().unwrap_or_default()
            {
                return Err(corrupted(
                    "unknown compensation outcome commit binding is inconsistent",
                ));
            }
            let step = serde_json::from_str::<DiscoveryCompensationStep>(&context.6)
                .map_err(|_| corrupted("stored compensation step is invalid"))?;
            step.validate_against(&attempt.plan).map_err(|_| {
                corrupted("stored compensation target differs from its commit plan")
            })?;
            let stored_kind = enum_wire_result(
                serde_json::to_value(step.kind),
                "stored discovery compensation kind",
            )?;
            if stored_kind != context.3 || step.ordinal != context.4 {
                return Err(corrupted(
                    "stored compensation columns differ from their typed step",
                ));
            }
        }

        let result = persist_transition_in_transaction(&transaction, write, None)?;
        let stored = transaction
            .query_row(
                "SELECT step.status, attempt.phase, session.state,
                        session.unknown_operation, session.active_operation_id,
                        attempt.session_id, session.commit_attempt_id,
                        session.commit_plan_sha256, attempt.plan_sha256
                 FROM provider_discovery_compensation_steps AS step
                 JOIN provider_discovery_commit_attempts AS attempt
                   ON attempt.id = step.commit_attempt_id
                 JOIN provider_discovery_sessions AS session
                   ON session.id = attempt.session_id
                 WHERE step.id = ?1",
                [step_id],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, Option<String>>(3)?,
                        row.get::<_, Option<String>>(4)?,
                        row.get::<_, String>(5)?,
                        row.get::<_, Option<String>>(6)?,
                        row.get::<_, Option<String>>(7)?,
                        row.get::<_, String>(8)?,
                    ))
                },
            )
            .optional()
            .map_err(database_error)?
            .ok_or_else(|| corrupted("unknown compensation step disappeared"))?;
        if stored.0 != "outcome_unknown"
            || stored.1 != "outcome_unknown"
            || stored.2 != "unknown_outcome"
            || stored.3.as_deref() != Some("compensation")
            || stored.4.is_some()
            || stored.5 != transition.session.id.as_str()
            || stored.6.as_deref()
                != transition
                    .session
                    .commit_attempt_id
                    .as_ref()
                    .map(DiscoveryCommitAttemptId::as_str)
            || stored.7.as_deref() != Some(stored.8.as_str())
        {
            return Err(corrupted(
                "unknown compensation outcome was not recorded atomically",
            ));
        }
        transaction.commit().map_err(database_error)?;
        Ok(result)
    }

    /// Removes exactly the graph named by a compensating commit plan.
    ///
    /// Foreign keys deliberately make this fail if any generation has begun to
    /// depend on the graph. Credential-vault deletion remains a separate native
    /// compensation step and is never attempted here.
    #[allow(clippy::too_many_lines)]
    pub fn compensate_discovered_provider_graph(
        &self,
        attempt_id: &DiscoveryCommitAttemptId,
        completed_at: DateTime<Utc>,
    ) -> CoreResult<()> {
        let mut connection = self.connection()?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(database_error)?;
        let attempt = load_commit_attempt(&transaction, attempt_id)?;
        if attempt.phase != DiscoveryCommitPhase::Compensating {
            return Err(CoreError::invalid(
                "provider graph compensation requires the compensating phase",
            ));
        }
        let state = transaction
            .query_row(
                "SELECT state
                 FROM provider_discovery_sessions
                 WHERE id = ?1 AND commit_attempt_id = ?2",
                params![attempt.session_id.as_str(), attempt.id.as_str()],
                |row| row.get::<_, String>(0),
            )
            .optional()
            .map_err(database_error)?
            .ok_or_else(|| corrupted("compensating commit is detached from its session"))?;
        if state != "compensating" {
            return Err(CoreError::invalid(
                "provider graph compensation requires a compensating discovery session",
            ));
        }
        require_started_session_operation(&transaction, &attempt.session_id, "compensation")?;
        let graph_steps = {
            let mut statement = transaction
                .prepare(
                    "SELECT id, status, ordinal, step_json
                     FROM provider_discovery_compensation_steps
                     WHERE commit_attempt_id = ?1
                       AND step_kind = 'remove_connection_graph'
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
                    ))
                })
                .map_err(database_error)?
                .collect::<Result<Vec<_>, _>>()
                .map_err(database_error)?
        };
        let [(step_id, step_status, step_ordinal, step_json)] = graph_steps.as_slice() else {
            return Err(corrupted(
                "compensation requires exactly one remove-connection-graph step",
            ));
        };
        let step = serde_json::from_str::<DiscoveryCompensationStep>(step_json)
            .map_err(|_| corrupted("stored graph compensation step is invalid"))?;
        step.validate_against(&attempt.plan)
            .map_err(|_| corrupted("graph compensation target differs from its commit plan"))?;
        if !matches!(
            &step.target,
            DiscoveryCompensationTarget::RemoveConnectionGraph { connection_id }
                if connection_id == &attempt.plan.connection_id
        ) {
            return Err(corrupted(
                "graph compensation step has the wrong typed target",
            ));
        }
        let higher_step_incomplete = transaction
            .query_row(
                "SELECT EXISTS(
                     SELECT 1
                     FROM provider_discovery_compensation_steps
                     WHERE commit_attempt_id = ?1
                       AND ordinal > ?2
                       AND status <> 'completed'
                 )",
                params![attempt.id.as_str(), step_ordinal],
                |row| row.get::<_, bool>(0),
            )
            .map_err(database_error)?;
        if higher_step_incomplete {
            return Err(CoreError::invalid(
                "provider graph compensation must follow reverse recipe order",
            ));
        }
        let stored_graph = load_discovered_provider_graph_rows(
            &transaction,
            &attempt.plan.template_id,
            attempt.plan.template_version,
            &attempt.plan.connection_id,
        )?;
        if stored_graph.is_none() {
            let planned_route_remains =
                attempt
                    .plan
                    .model_route_ids
                    .iter()
                    .try_fold(false, |found, route_id| {
                        if found {
                            return Ok(true);
                        }
                        transaction
                            .query_row(
                                "SELECT EXISTS(SELECT 1 FROM provider_models WHERE id = ?1)",
                                [route_id.as_str()],
                                |row| row.get::<_, bool>(0),
                            )
                            .map_err(database_error)
                    })?;
            if planned_route_remains {
                return Err(corrupted(
                    "compensated connection is absent but one of its planned routes remains",
                ));
            }
            if step_status == "completed" {
                transaction.commit().map_err(database_error)?;
                return Ok(());
            }
            if step_status != "in_progress" {
                return Err(CoreError::invalid(
                    "provider graph compensation step must be in progress",
                ));
            }
            mark_connection_graph_step_completed(&transaction, step_id, completed_at)?;
            transaction.commit().map_err(database_error)?;
            return Ok(());
        }
        if step_status != "in_progress" {
            return Err(CoreError::invalid(
                "provider graph compensation step must be in progress",
            ));
        }
        let Some(stored_graph) = stored_graph else {
            return Err(corrupted("provider graph disappeared during compensation"));
        };
        let expected_ownership_hash =
            graph_ownership_audit_hash(&transaction, &attempt.session_id)?;
        if stored_provider_graph_ownership_hash(&stored_graph)? != expected_ownership_hash {
            return Err(CoreError::invalid(
                "refusing to compensate a provider graph changed after discovery commit",
            ));
        }
        let stored_routes = stored_graph
            .routes
            .iter()
            .map(|route| route.id.as_str().to_owned())
            .collect::<BTreeSet<_>>();
        let planned_routes = attempt
            .plan
            .model_route_ids
            .iter()
            .map(|id| id.as_str().to_owned())
            .collect::<BTreeSet<_>>();
        if stored_routes != planned_routes {
            return Err(CoreError::invalid(
                "refusing to compensate a provider graph that changed after commit",
            ));
        }
        let selection_revision_after_graph_removal =
            clear_provider_selections_for_discovery_compensation(
                &transaction,
                attempt.plan.connection_id.as_str(),
            )?;
        if let Some(selection_revision) = selection_revision_after_graph_removal {
            record_discovery_selection_restore_authority(
                &transaction,
                &attempt.id,
                selection_revision,
                completed_at,
            )?;
        }
        transaction
            .execute(
                "DELETE FROM generation_presets
                 WHERE model_route_id IN (
                     SELECT id FROM provider_models WHERE connection_id = ?1
                 )",
                [attempt.plan.connection_id.as_str()],
            )
            .map_err(database_error)?;
        transaction
            .execute(
                "DELETE FROM model_capability_observations
                 WHERE model_route_id IN (
                     SELECT id FROM provider_models WHERE connection_id = ?1
                 )",
                [attempt.plan.connection_id.as_str()],
            )
            .map_err(database_error)?;
        transaction
            .execute(
                "DELETE FROM provider_models WHERE connection_id = ?1",
                [attempt.plan.connection_id.as_str()],
            )
            .map_err(database_error)?;
        let deleted = transaction
            .execute(
                "DELETE FROM provider_connections
                 WHERE id = ?1 AND template_id = ?2 AND template_version = ?3",
                params![
                    attempt.plan.connection_id.as_str(),
                    attempt.plan.template_id.as_str(),
                    attempt.plan.template_version,
                ],
            )
            .map_err(database_error)?;
        if deleted != 1 {
            return Err(CoreError::invalid(
                "committed provider connection was missing or changed",
            ));
        }
        if graph_template_was_created(&transaction, &attempt.session_id)? {
            transaction
                .execute(
                    "DELETE FROM provider_templates
                     WHERE id = ?1 AND version = ?2 AND source_kind = 'user_discovered'
                       AND NOT EXISTS (
                           SELECT 1 FROM provider_connections
                           WHERE template_id = ?1 AND template_version = ?2
                       )",
                    params![
                        attempt.plan.template_id.as_str(),
                        attempt.plan.template_version
                    ],
                )
                .map_err(database_error)?;
        }
        ensure_foreign_keys_clean(&transaction)?;
        mark_connection_graph_step_completed(&transaction, step_id, completed_at)?;
        transaction.commit().map_err(database_error)
    }
}

fn mark_connection_graph_step_completed(
    transaction: &Transaction<'_>,
    step_id: &str,
    completed_at: DateTime<Utc>,
) -> CoreResult<()> {
    let changed = transaction
        .execute(
            "UPDATE provider_discovery_compensation_steps
             SET status = 'completed',
                 last_failure_json = NULL,
                 updated_at = ?2,
                 completed_at = ?2
             WHERE id = ?1
               AND step_kind = 'remove_connection_graph'
               AND status = 'in_progress'",
            params![step_id, completed_at.to_rfc3339()],
        )
        .map_err(database_error)?;
    if changed != 1 {
        return Err(CoreError::invalid(
            "provider graph compensation step changed concurrently",
        ));
    }
    Ok(())
}
