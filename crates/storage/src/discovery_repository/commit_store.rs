//! Atomic discovery commit bookkeeping and prior-selection restoration.

mod provider_graph;

use super::{
    Connection, CoreError, CoreErrorCode, CoreResult, DateTime, DiscoveryActionId,
    DiscoveryCommitAttemptId, DiscoveryCommitAttemptRecord, DiscoveryCommitPhase,
    DiscoveryCommitPlan, DiscoveryCompensationStep, DiscoveryCompensationTarget, DiscoveryEffect,
    DiscoveryOperationKind, DiscoveryOperationStatus, DiscoverySessionId, DiscoverySideEffectClass,
    DiscoveryState, DiscoveryTransition, DiscoveryTransitionWrite, DurableOperationOutcome,
    OptionalExtension, PersistDiscoveryTransition, Storage, Transaction, TransactionBehavior, Utc,
    contract_error, corrupted, database_error, encode_commit_plan_json,
    insert_discovery_credential_ownership_event, load_discovered_provider_graph_rows,
    load_discovery_native_credential_execution, load_operation_by_id, params, parse_timestamp,
    persist_transition_in_transaction, restore_discovery_provider_selection, sha256_hex,
    validate_transition_write,
};
pub(super) use provider_graph::{
    apply_provider_graph_in_transaction, ensure_discovery_attempt_graph_absent,
    graph_ownership_audit_hash, graph_template_was_created, provider_graph_ownership_hash,
    require_started_session_operation, stored_provider_graph_ownership_hash,
    validate_discovery_authority_graph_audits, validate_graph_component, validate_provider_graph,
    verify_discovery_attempt_graph,
};

impl Storage {
    /// Finalizes a credential-confirmed discovery commit in one `SQLite`
    /// transaction. Until this transaction commits, no provider graph row is
    /// visible to connection, route, preset, model-sync, or generation readers.
    #[allow(clippy::too_many_lines)]
    pub fn persist_credential_confirmed_discovery_commit(
        &self,
        write: &DiscoveryTransitionWrite,
    ) -> CoreResult<PersistDiscoveryTransition> {
        validate_transition_write(write)?;
        let transition = &write.transition;
        let graph = write.provider_graph.as_ref().ok_or_else(|| {
            CoreError::invalid("credential-confirmed commit requires its exact provider graph")
        })?;
        if graph.plan.credential_ref.is_none()
            || graph.connection.credential_ref != graph.plan.credential_ref
            || transition.receipt.action_kind != "commit_succeeded"
            || transition.session.state != DiscoveryState::Ready
            || transition.session.commit_attempt_id.as_ref() != Some(&graph.plan.attempt_id)
            || transition.session.commit_plan_sha256.as_deref() != Some(graph.plan_sha256.as_str())
            || transition.session.committed_connection_id.as_ref()
                != Some(&graph.plan.connection_id)
            || transition.effect != DiscoveryEffect::None
            || write.new_operation_id.is_some()
            || write
                .completed_operation
                .as_ref()
                .is_none_or(|completed| completed.outcome != DurableOperationOutcome::Succeeded)
        {
            return Err(CoreError::invalid(
                "credential-confirmed commit does not match the exact ready transition",
            ));
        }
        let authority_operation_id = &write
            .completed_operation
            .as_ref()
            .ok_or_else(|| {
                CoreError::invalid("credential-confirmed commit has no successful native operation")
            })?
            .id;

        let mut transition_only = write.clone();
        transition_only.provider_graph = None;
        let mut connection = self.connection()?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(database_error)?;
        let authority_observed_at = Utc::now();
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
            apply_provider_graph_in_transaction(
                &transaction,
                graph,
                transition.previous_revision,
                write.occurred_at,
                authority_observed_at,
            )?;
            let changed = transaction
                .execute(
                    "UPDATE provider_discovery_commit_attempts
                     SET phase = 'credential_reference_applied',
                         updated_at = ?3,
                         completed_at = NULL
                     WHERE id = ?1
                       AND session_id = ?2
                       AND plan_sha256 = ?4
                       AND phase = 'database_applied'",
                    params![
                        graph.plan.attempt_id.as_str(),
                        graph.plan.session_id.as_str(),
                        write.occurred_at.to_rfc3339(),
                        graph.plan_sha256.as_str(),
                    ],
                )
                .map_err(database_error)?;
            if changed != 1 {
                return Err(CoreError::invalid(
                    "credential-confirmed commit phase changed concurrently",
                ));
            }
        }

        let result = persist_transition_in_transaction(&transaction, &transition_only, None)?;
        let stored = transaction
            .query_row(
                "SELECT session.state, session.revision,
                        session.commit_attempt_id, session.commit_plan_sha256,
                        session.committed_connection_id, session.active_operation_id,
                        attempt.phase, attempt.completed_at
                 FROM provider_discovery_sessions AS session
                 JOIN provider_discovery_commit_attempts AS attempt
                   ON attempt.id = session.commit_attempt_id
                  AND attempt.session_id = session.id
                 WHERE session.id = ?1",
                [transition.session.id.as_str()],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, u64>(1)?,
                        row.get::<_, Option<String>>(2)?,
                        row.get::<_, Option<String>>(3)?,
                        row.get::<_, Option<String>>(4)?,
                        row.get::<_, Option<String>>(5)?,
                        row.get::<_, String>(6)?,
                        row.get::<_, Option<String>>(7)?,
                    ))
                },
            )
            .optional()
            .map_err(database_error)?
            .ok_or_else(|| corrupted("finalized discovery commit binding disappeared"))?;
        if stored.0 != "ready"
            || stored.1 != transition.session.revision
            || stored.2.as_deref() != Some(graph.plan.attempt_id.as_str())
            || stored.3.as_deref() != Some(graph.plan_sha256.as_str())
            || stored.4.as_deref() != Some(graph.plan.connection_id.as_str())
            || stored.5.is_some()
            || stored.6 != "completed"
            || stored.7.is_none()
        {
            return Err(corrupted(
                "credential-confirmed provider graph was not finalized atomically",
            ));
        }
        let authority_operation = load_operation_by_id(&transaction, authority_operation_id)?;
        if authority_operation.session_id != transition.session.id
            || authority_operation.kind != DiscoveryOperationKind::AtomicCommit
            || authority_operation.side_effect_class != DiscoverySideEffectClass::Persistent
            || authority_operation.status != DiscoveryOperationStatus::Succeeded
            || authority_operation.finished_at != Some(write.occurred_at)
            || authority_operation.updated_at != write.occurred_at
        {
            return Err(corrupted(
                "credential-confirmed provider ownership is detached from its successful native operation",
            ));
        }
        let stored_graph = load_discovered_provider_graph_rows(
            &transaction,
            &graph.plan.template_id,
            graph.plan.template_version,
            &graph.plan.connection_id,
        )?
        .ok_or_else(|| corrupted("finalized discovery provider graph is missing"))?;
        if stored_provider_graph_ownership_hash(&stored_graph)? != graph.plan.graph_sha256
            || graph_ownership_audit_hash(&transaction, &graph.plan.session_id)?
                != graph.plan.graph_sha256
        {
            return Err(corrupted(
                "finalized discovery provider graph differs from its approved ownership",
            ));
        }
        let authority_execution =
            load_discovery_native_credential_execution(&transaction, authority_operation_id)?
                .ok_or_else(|| {
                    corrupted(
                        "credential-confirmed commit has no physical native execution authority",
                    )
                })?;
        let connection_binding_sha256 = if receipt_exists {
            authority_execution.connection_binding_sha256.clone()
        } else {
            crate::provider_credential_repository::provider_credential_connection_binding_sha256(
                &transaction,
                &graph.plan.connection_id,
            )?
        };
        if authority_execution.connection_id != graph.plan.connection_id
            || authority_execution.commit_attempt_id != graph.plan.attempt_id
            || authority_execution.commit_plan_sha256 != graph.plan_sha256
            || authority_execution.connection_binding_sha256 != connection_binding_sha256
            || authority_execution.store_started_at != authority_operation.started_at
        {
            return Err(corrupted(
                "credential-confirmed physical authority is detached from its successful operation",
            ));
        }
        if receipt_exists {
            let replayed_projection = transaction
                .query_row(
                    "SELECT event.authority_sequence, event.ownership_state,
                            event.connection_binding_sha256, event.authority_id,
                            event.source_id, event.created_at,
                            ownership.ownership_state,
                            ownership.connection_binding_sha256,
                            ownership.authority_id, ownership.authority_sequence,
                            ownership.updated_at
                     FROM provider_credential_ownership_events AS event
                     JOIN provider_credential_ownership AS ownership
                       ON ownership.connection_id = event.connection_id
                      AND ownership.credential_ref = event.connection_id
                     WHERE event.connection_id = ?1
                       AND event.source_kind = 'discovery_commit'
                       AND event.source_id = ?2",
                    params![
                        graph.plan.connection_id.as_str(),
                        authority_operation_id.as_str(),
                    ],
                    |row| {
                        Ok((
                            row.get::<_, u64>(0)?,
                            row.get::<_, String>(1)?,
                            row.get::<_, Option<String>>(2)?,
                            row.get::<_, String>(3)?,
                            row.get::<_, String>(4)?,
                            row.get::<_, String>(5)?,
                            row.get::<_, String>(6)?,
                            row.get::<_, Option<String>>(7)?,
                            row.get::<_, String>(8)?,
                            row.get::<_, u64>(9)?,
                            row.get::<_, String>(10)?,
                        ))
                    },
                )
                .optional()
                .map_err(database_error)?
                .ok_or_else(|| {
                    corrupted("replayed credential-confirmed ownership event is missing")
                })?;
            let original_event_is_exact = replayed_projection.1 == "discovery_owned"
                && replayed_projection.2.as_deref() == Some(&connection_binding_sha256)
                && replayed_projection.3 == authority_execution.physical_authority_id
                && replayed_projection.4 == authority_operation_id.as_str()
                && parse_timestamp(
                    &replayed_projection.5,
                    "replayed discovery ownership event created_at",
                )? == write.occurred_at;
            let current_projection_is_exact = replayed_projection.6 == "discovery_owned"
                && replayed_projection.7.as_deref() == Some(&connection_binding_sha256)
                && replayed_projection.8 == authority_execution.physical_authority_id
                && replayed_projection.9 == replayed_projection.0
                && parse_timestamp(
                    &replayed_projection.10,
                    "replayed discovery ownership projection updated_at",
                )? == write.occurred_at;
            if !original_event_is_exact
                || replayed_projection.9 < replayed_projection.0
                || (replayed_projection.9 == replayed_projection.0 && !current_projection_is_exact)
            {
                return Err(corrupted(
                    "replayed credential-confirmed ownership projection differs from its physical authority",
                ));
            }
            if replayed_projection.9 == replayed_projection.0 {
                let active_binding = crate::provider_credential_repository::
                    provider_credential_connection_binding_sha256(
                        &transaction,
                        &graph.plan.connection_id,
                    )?;
                if active_binding != connection_binding_sha256 {
                    return Err(corrupted(
                        "replayed credential-confirmed ownership binding changed without a journal successor",
                    ));
                }
            } else {
                crate::provider_credential_repository::validate_superseded_provider_credential_ownership_event_history(
                    &transaction,
                    &graph.plan.connection_id,
                    replayed_projection.0,
                    &authority_execution.physical_authority_id,
                    &connection_binding_sha256,
                )?;
            }
            transaction.commit().map_err(database_error)?;
            return Ok(result);
        }
        let authority_sequence = insert_discovery_credential_ownership_event(
            &transaction,
            &graph.plan.connection_id,
            &connection_binding_sha256,
            &authority_execution.physical_authority_id,
            authority_operation_id,
            write.occurred_at,
        )?;
        let ownership_changed = transaction
            .execute(
                "UPDATE provider_credential_ownership
                 SET ownership_state = 'discovery_owned',
                     connection_binding_sha256 = ?2, authority_id = ?3,
                     authority_sequence = ?4, updated_at = ?5
                 WHERE connection_id = ?1 AND credential_ref = ?1",
                params![
                    graph.plan.connection_id.as_str(),
                    connection_binding_sha256,
                    authority_execution.physical_authority_id,
                    authority_sequence,
                    write.occurred_at.to_rfc3339(),
                ],
            )
            .map_err(database_error)?;
        if ownership_changed != 1 {
            return Err(corrupted(
                "credential-confirmed discovery lost its ownership projection",
            ));
        }
        transaction.commit().map_err(database_error)?;
        Ok(result)
    }

    #[allow(clippy::too_many_lines)]
    pub fn restore_discovery_previous_selection(
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
                "selection restoration requires the compensating phase",
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
                "selection restoration requires a compensating discovery session",
            ));
        }
        require_started_session_operation(&transaction, &attempt.session_id, "compensation")?;
        let rows = {
            let mut statement = transaction
                .prepare(
                    "SELECT step.id, step.ordinal, step.step_json, step.status,
                            authority.selection_revision_after_graph_removal
                     FROM provider_discovery_compensation_steps AS step
                     LEFT JOIN provider_discovery_selection_restore_authorities AS authority
                       ON authority.commit_attempt_id = step.commit_attempt_id
                      AND authority.restore_step_id = step.id
                     WHERE step.commit_attempt_id = ?1
                       AND step.step_kind = 'restore_previous_selection'
                     ORDER BY step.ordinal, step.id",
                )
                .map_err(database_error)?;
            statement
                .query_map([attempt.id.as_str()], |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, u32>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, String>(3)?,
                        row.get::<_, Option<i64>>(4)?,
                    ))
                })
                .map_err(database_error)?
                .collect::<Result<Vec<_>, _>>()
                .map_err(database_error)?
        };
        let [(step_id, ordinal, step_json, status, expected_selection_revision)] = rows.as_slice()
        else {
            return Err(corrupted(
                "compensation requires exactly one restore-previous-selection step",
            ));
        };
        if status == "completed" {
            transaction.commit().map_err(database_error)?;
            return Ok(());
        }
        if status != "in_progress" {
            return Err(CoreError::invalid(
                "selection restoration step must be in progress",
            ));
        }
        let step = serde_json::from_str::<DiscoveryCompensationStep>(step_json)
            .map_err(|_| corrupted("stored selection restoration step is invalid"))?;
        step.validate_against(&attempt.plan)
            .map_err(|_| corrupted("selection restoration target differs from its commit plan"))?;
        let DiscoveryCompensationTarget::RestorePreviousSelection { previous_selection } =
            &step.target
        else {
            return Err(corrupted(
                "selection restoration step has the wrong typed target",
            ));
        };
        let higher_step_incomplete = transaction
            .query_row(
                "SELECT EXISTS(
                     SELECT 1
                     FROM provider_discovery_compensation_steps
                     WHERE commit_attempt_id = ?1
                       AND ordinal > ?2
                       AND status <> 'completed'
                 )",
                params![attempt.id.as_str(), ordinal],
                |row| row.get::<_, bool>(0),
            )
            .map_err(database_error)?;
        if higher_step_incomplete {
            return Err(CoreError::invalid(
                "selection restoration must follow reverse recipe order",
            ));
        }
        let expected_selection_revision = expected_selection_revision
            .map(|revision| {
                u64::try_from(revision)
                    .map_err(|_| corrupted("stored provider selection revision is negative"))
            })
            .transpose()?;
        restore_discovery_provider_selection(
            &transaction,
            previous_selection,
            expected_selection_revision,
        )?;
        let changed = transaction
            .execute(
                "UPDATE provider_discovery_compensation_steps
                 SET status = 'completed',
                     last_failure_json = NULL,
                     updated_at = ?2,
                     completed_at = ?2
                 WHERE id = ?1
                   AND step_kind = 'restore_previous_selection'
                   AND status = 'in_progress'",
                params![step_id, completed_at.to_rfc3339()],
            )
            .map_err(database_error)?;
        if changed != 1 {
            return Err(CoreError::invalid(
                "selection restoration step changed concurrently",
            ));
        }
        transaction.commit().map_err(database_error)
    }
}

pub(super) fn record_discovery_selection_restore_authority(
    transaction: &Transaction<'_>,
    attempt_id: &DiscoveryCommitAttemptId,
    selection_revision: u64,
    created_at: DateTime<Utc>,
) -> CoreResult<()> {
    let restore_step_ids = {
        let mut statement = transaction
            .prepare(
                "SELECT id
                 FROM provider_discovery_compensation_steps
                 WHERE commit_attempt_id = ?1
                   AND step_kind = 'restore_previous_selection'
                 ORDER BY ordinal, id",
            )
            .map_err(database_error)?;
        statement
            .query_map([attempt_id.as_str()], |row| row.get::<_, String>(0))
            .map_err(database_error)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(database_error)?
    };
    let [restore_step_id] = restore_step_ids.as_slice() else {
        return Err(corrupted(
            "graph removal requires exactly one selection restoration step",
        ));
    };
    let selection_revision = i64::try_from(selection_revision)
        .map_err(|_| CoreError::internal("provider selection revision exceeds SQLite range"))?;
    transaction
        .execute(
            "INSERT INTO provider_discovery_selection_restore_authorities (
                 commit_attempt_id, restore_step_id,
                 selection_revision_after_graph_removal, created_at
             ) VALUES (?1, ?2, ?3, ?4)",
            params![
                attempt_id.as_str(),
                restore_step_id,
                selection_revision,
                created_at.to_rfc3339(),
            ],
        )
        .map_err(database_error)?;
    Ok(())
}

pub(super) fn load_discovery_selection_restore_revision(
    connection: &Connection,
    attempt_id: &DiscoveryCommitAttemptId,
) -> CoreResult<Option<u64>> {
    connection
        .query_row(
            "SELECT selection_revision_after_graph_removal
             FROM provider_discovery_selection_restore_authorities
             WHERE commit_attempt_id = ?1",
            [attempt_id.as_str()],
            |row| row.get::<_, i64>(0),
        )
        .optional()
        .map_err(database_error)?
        .map(|revision| {
            u64::try_from(revision)
                .map_err(|_| corrupted("stored provider selection revision is negative"))
        })
        .transpose()
}

pub(super) fn finalize_commit_failed_before_apply(
    transaction: &Transaction<'_>,
    write: &DiscoveryTransitionWrite,
) -> CoreResult<()> {
    if write.transition.receipt.action_kind != "commit_failed_before_apply" {
        return Ok(());
    }
    let attempt_id = write
        .transition
        .session
        .commit_attempt_id
        .as_ref()
        .ok_or_else(|| corrupted("failed-before-apply transition has no commit attempt"))?;
    let attempt = load_commit_attempt(transaction, attempt_id)?;
    let session_owns_attempt = transaction
        .query_row(
            "SELECT commit_attempt_id = ?2
             FROM provider_discovery_sessions
             WHERE id = ?1",
            params![write.transition.session.id.as_str(), attempt.id.as_str(),],
            |row| row.get::<_, bool>(0),
        )
        .optional()
        .map_err(database_error)?
        .unwrap_or(false);
    if attempt.session_id != write.transition.session.id
        || !session_owns_attempt
        || attempt.phase != DiscoveryCommitPhase::Prepared
    {
        return Err(CoreError::invalid(
            "failed-before-apply requires the session's own prepared commit attempt",
        ));
    }
    if load_discovered_provider_graph_rows(
        transaction,
        &attempt.plan.template_id,
        attempt.plan.template_version,
        &attempt.plan.connection_id,
    )?
    .is_some()
    {
        return Err(CoreError::invalid(
            "failed-before-apply cannot finalize after provider graph publication",
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
            return Err(CoreError::invalid(
                "failed-before-apply found a planned route already persisted",
            ));
        }
    }
    if attempt.plan.credential_ref.is_some() {
        return Err(CoreError::invalid(
            "failed-before-apply cannot attest native credential cleanup",
        ));
    }
    restore_discovery_provider_selection(transaction, &attempt.plan.previous_selection, None)?;
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
             WHERE id = ?1 AND phase = 'prepared'",
            params![attempt.id.as_str(), write.occurred_at.to_rfc3339()],
        )
        .map_err(database_error)?;
    if changed != 1 {
        return Err(CoreError::invalid(
            "failed-before-apply commit attempt changed concurrently",
        ));
    }
    Ok(())
}

pub(super) fn complete_commit_attempt_for_ready_transition(
    transaction: &Transaction<'_>,
    transition: &DiscoveryTransition,
    completed_at: DateTime<Utc>,
) -> CoreResult<()> {
    let attempt_id = transition
        .session
        .commit_attempt_id
        .as_ref()
        .ok_or_else(|| corrupted("ready discovery session has no commit attempt"))?;
    let (phase, plan_sha256, plan_json) = transaction
        .query_row(
            "SELECT phase, plan_sha256, plan_json
             FROM provider_discovery_commit_attempts
             WHERE id = ?1 AND session_id = ?2",
            params![attempt_id.as_str(), transition.session.id.as_str()],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                ))
            },
        )
        .optional()
        .map_err(database_error)?
        .ok_or_else(|| corrupted("ready discovery commit attempt is missing"))?;
    if phase == "completed" {
        return Ok(());
    }
    let plan = serde_json::from_str::<DiscoveryCommitPlan>(&plan_json)
        .map_err(|_| corrupted("stored discovery commit plan is invalid"))?;
    plan.validate()
        .map_err(|_| corrupted("stored discovery commit plan violates its contract"))?;
    if sha256_hex(plan_json.as_bytes()) != plan_sha256
        || transition.session.commit_plan_sha256.as_deref() != Some(plan_sha256.as_str())
        || transition.session.committed_connection_id.as_ref() != Some(&plan.connection_id)
    {
        return Err(CoreError::invalid(
            "ready discovery session does not match its immutable commit plan",
        ));
    }
    let required_phase = if plan.credential_ref.is_some() {
        "credential_reference_applied"
    } else {
        "database_applied"
    };
    if phase != required_phase {
        return Err(CoreError::invalid(
            "discovery commit cannot finish before all durable phases are applied",
        ));
    }
    let changed = transaction
        .execute(
            "UPDATE provider_discovery_commit_attempts
             SET phase = 'completed', updated_at = ?2, completed_at = ?2
             WHERE id = ?1 AND phase = ?3",
            params![
                attempt_id.as_str(),
                completed_at.to_rfc3339(),
                required_phase
            ],
        )
        .map_err(database_error)?;
    if changed != 1 {
        return Err(CoreError::invalid(
            "discovery commit phase changed concurrently",
        ));
    }
    Ok(())
}

pub(super) fn load_commit_attempt(
    connection: &Connection,
    attempt_id: &DiscoveryCommitAttemptId,
) -> CoreResult<DiscoveryCommitAttemptRecord> {
    let row = connection
        .query_row(
            "SELECT id, session_id, attempt_number, action_id, expected_revision,
                    plan_sha256, plan_json, phase, created_at, updated_at, completed_at
             FROM provider_discovery_commit_attempts
             WHERE id = ?1",
            [attempt_id.as_str()],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, u32>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, u64>(4)?,
                    row.get::<_, String>(5)?,
                    row.get::<_, String>(6)?,
                    row.get::<_, String>(7)?,
                    row.get::<_, String>(8)?,
                    row.get::<_, String>(9)?,
                    row.get::<_, Option<String>>(10)?,
                ))
            },
        )
        .optional()
        .map_err(database_error)?
        .ok_or_else(|| {
            CoreError::new(
                CoreErrorCode::NotFound,
                "discovery commit attempt was not found",
                false,
            )
        })?;
    let plan = serde_json::from_str::<DiscoveryCommitPlan>(&row.6)
        .map_err(|_| corrupted("stored discovery commit plan is invalid"))?;
    plan.validate()
        .map_err(|_| corrupted("stored discovery commit plan violates its contract"))?;
    let canonical_plan_json = encode_commit_plan_json(&plan)
        .map_err(|_| corrupted("stored discovery commit plan is not canonical"))?;
    if plan.attempt_id.as_str() != row.0
        || plan.session_id.as_str() != row.1
        || plan.expected_revision != row.4
        || canonical_plan_json != row.6
        || sha256_hex(row.6.as_bytes()) != row.5
    {
        return Err(corrupted(
            "stored discovery commit attempt does not match its plan",
        ));
    }
    Ok(DiscoveryCommitAttemptRecord {
        id: DiscoveryCommitAttemptId::parse(row.0).map_err(contract_error)?,
        session_id: DiscoverySessionId::from(row.1),
        attempt_number: row.2,
        action_id: DiscoveryActionId::parse(row.3).map_err(contract_error)?,
        expected_revision: row.4,
        plan_sha256: row.5,
        plan,
        phase: DiscoveryCommitPhase::parse(&row.7)?,
        created_at: parse_timestamp(&row.8, "commit attempt created_at")?,
        updated_at: parse_timestamp(&row.9, "commit attempt updated_at")?,
        completed_at: row
            .10
            .as_deref()
            .map(|value| parse_timestamp(value, "commit attempt completed_at"))
            .transpose()?,
    })
}

pub(super) fn validate_commit_phase_preconditions(
    transaction: &Transaction<'_>,
    attempt: &DiscoveryCommitAttemptRecord,
    next: DiscoveryCommitPhase,
) -> CoreResult<()> {
    let session_state = transaction
        .query_row(
            "SELECT state
             FROM provider_discovery_sessions
             WHERE id = ?1 AND commit_attempt_id = ?2",
            params![attempt.session_id.as_str(), attempt.id.as_str()],
            |row| row.get::<_, String>(0),
        )
        .optional()
        .map_err(database_error)?
        .ok_or_else(|| corrupted("commit attempt is detached from its discovery session"))?;
    match next {
        DiscoveryCommitPhase::CredentialReferenceApplied => {
            if attempt.plan.credential_ref.is_none() || session_state != "committing" {
                return Err(CoreError::invalid(
                    "credential confirmation requires a credential-bearing active commit",
                ));
            }
            require_started_session_operation(transaction, &attempt.session_id, "atomic_commit")?;
            verify_discovery_attempt_graph(transaction, attempt)
        }
        DiscoveryCommitPhase::Compensated => {
            if session_state != "compensating" {
                return Err(CoreError::invalid(
                    "compensated phase requires a compensating discovery session",
                ));
            }
            require_started_session_operation(transaction, &attempt.session_id, "compensation")?;
            let (total_steps, incomplete_steps) = transaction
                .query_row(
                    "SELECT COUNT(*),
                            COALESCE(SUM(
                                CASE WHEN status = 'completed' THEN 0 ELSE 1 END
                            ), 0)
                     FROM provider_discovery_compensation_steps
                     WHERE commit_attempt_id = ?1",
                    [attempt.id.as_str()],
                    |row| Ok((row.get::<_, u32>(0)?, row.get::<_, u32>(1)?)),
                )
                .map_err(database_error)?;
            ensure_discovery_attempt_graph_absent(transaction, attempt)?;
            if total_steps == 0 || incomplete_steps != 0 {
                return Err(CoreError::invalid(
                    "compensated phase requires every recipe step to be complete",
                ));
            }
            Ok(())
        }
        _ => Err(CoreError::internal(
            "unsupported standalone discovery commit phase validation",
        )),
    }
}
