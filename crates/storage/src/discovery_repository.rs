//! Typed repository facade for durable provider discovery.
//!
//! The lower-level [`crate::discovery`] module owns the `SQLite` state-machine
//! primitives. This module is the product-facing boundary: it hydrates domain
//! aggregates, validates bounded redacted payloads, and keeps provider graph
//! publication in the same transaction as discovery commit bookkeeping.

use std::collections::BTreeSet;

use chrono::{DateTime, Utc};
use lorepia_domain::{
    CapabilityObservation, Confidence, ConnectionConfigValue, CoreError, CoreErrorCode, CoreResult,
    CredentialRedirectPolicy, CredentialRef, DiscoverySessionId, EvidenceId, GenerationPreset,
    HttpUrl, ModelMetadataSource, ModelRoute, ObservationSource, ProviderConnection,
    ProviderConnectionId, ProviderNetworkMode, ProviderTemplate, SupportStatus, TemplateSource,
    discovery::{
        DiscoveryActionEnvelope, DiscoveryActionId, DiscoveryActionReceipt,
        DiscoveryActionRequired, DiscoveryApprovalBinding, DiscoveryApprovalDecision,
        DiscoveryApprovalGrant, DiscoveryApprovalId, DiscoveryApprovalRecord, DiscoveryCandidate,
        DiscoveryCandidateId, DiscoveryCommitAttemptId, DiscoveryCommitPlan,
        DiscoveryCompensationKind, DiscoveryCompensationStatus as DomainCompensationStatus,
        DiscoveryCompensationStep, DiscoveryCompensationTarget, DiscoveryEffect, DiscoveryEventId,
        DiscoveryInterruptionOutcome, DiscoveryOperationId, DiscoveryOperationKind,
        DiscoveryPreviousSelection, DiscoveryRecoveryCheckpoint, DiscoveryReviewDiff,
        DiscoverySideEffectClass, DiscoveryState, DiscoveryTransition,
        DiscoveryUnknownOutcomeResolution, PROVIDER_DISCOVERY_EVENT_VERSION,
        ProviderDiscoveryAction, ProviderDiscoveryEvent, ProviderDiscoverySession,
        SanitizedDiscoveryInput,
    },
};
use rusqlite::{Connection, OptionalExtension, Transaction, TransactionBehavior, params};
use serde::Serialize;
use serde_json::Value;
use uuid::Uuid;

use crate::{
    ProviderCredentialAccessAuthority, Storage,
    database::{
        StoredDiscoveredProviderGraphRows, clear_provider_selections_for_discovery_compensation,
        load_discovered_provider_graph_rows, load_discovery_previous_selection,
        restore_discovery_provider_selection, write_discovered_provider_graph_rows,
    },
    discovery::{
        self, CompletedDiscoveryOperation, DiscoveryRecoveryDisposition, DurableDiscoveryEffect,
        DurableDiscoveryTransition, DurableOperationOutcome, NewDiscoveryApproval,
        NewDiscoveryCommitAttempt, NewDiscoveryCompensationStep, NewDiscoveryOperation,
        PersistDiscoveryTransition,
    },
    generation_attempt::validate_provider_credential_access_authority_in_transaction,
    validate_provider_api_route_metadata,
};

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

fn validate_pre_store_native_credential_interruption(
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

fn validate_legacy_unbound_started_credential_execution(
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

fn load_discovery_credential_compensation_operation_id(
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

fn validate_active_discovery_credential_cancellation_chain(
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

fn validate_discovery_compensation_cancellation_chain(
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

fn validate_native_no_effect_retry_predecessor(
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

#[allow(clippy::too_many_lines)]
fn reconcile_discovery_saga_ledger(
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

fn prepare_compensation_ledger(
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

fn validate_failed_compensation_ledger(
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

fn validate_terminal_compensation_transition(
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

mod approval_store;
mod commit_store;
pub(crate) mod contract_codec;
mod credential_execution;
mod errors;
mod event_outbox;
mod queries;
mod repository_io;
mod row_mapping;
mod semantic_view;
mod transition_store;
mod types;
mod validation;

pub use semantic_view::{DiscoveryCandidateSnapshot, StoredDiscoveryCandidate};
pub use types::{
    DiscoveredProviderGraph, DiscoveryActionReplay, DiscoveryCommitAttemptRecord,
    DiscoveryCommitPhase, DiscoveryCompensationRecord, DiscoveryCompensationStatus,
    DiscoveryCompletedOperationWrite, DiscoveryEvidenceKind, DiscoveryEvidenceRecord,
    DiscoveryJsonUpdate, DiscoveryNativeCredentialExecutionRecord,
    DiscoveryNativeCredentialExecutionReservation, DiscoveryNativeCredentialStoreAttemptStart,
    DiscoveryNativeNoEffectAttestationKind, DiscoveryNativeNoEffectAttestationRecord,
    DiscoveryNativeNoEffectAttestationWrite, DiscoveryNativeRecoveryOwner,
    DiscoveryOperationRecord, DiscoveryOperationStatus, DiscoveryOutboxEvent,
    DiscoveryRecoveryResult, DiscoverySessionSnapshot, DiscoveryTransitionWrite,
    PreparedDiscoveryCommit, PreparedDiscoveryCompensationStep,
};

use approval_store::{
    approval_kind, validate_credential_approval, validate_discovery_authority_approval_rows,
    validate_discovery_unknown_outcome_resolution, validate_review_approval,
};
use commit_store::{
    apply_provider_graph_in_transaction, complete_commit_attempt_for_ready_transition,
    ensure_discovery_attempt_graph_absent, finalize_commit_failed_before_apply,
    graph_ownership_audit_hash, graph_template_was_created, load_commit_attempt,
    load_discovery_selection_restore_revision, provider_graph_ownership_hash,
    record_discovery_selection_restore_authority, require_started_session_operation,
    stored_provider_graph_ownership_hash, validate_commit_phase_preconditions,
    validate_discovery_authority_graph_audits, validate_graph_component, validate_provider_graph,
    verify_discovery_attempt_graph,
};
use contract_codec::{
    append_audit, candidate_kind, canonical_json_result, canonical_typed_json_result,
    decode_redacted_json, encode_approval_grant, encode_commit_plan_json, encode_json_result,
    encode_redacted_json, enum_wire_result, parse_approval_decision, parse_discovery_state,
    parse_operation_kind, parse_side_effect_class, parse_timestamp, sha256_hex,
};
#[cfg(test)]
use credential_execution::native_no_effect_execution_binding_sha256;
use credential_execution::{
    DISCOVERY_REDACTION_VERSION, DiscoveryAuthorityReceiptRecord,
    insert_discovery_credential_ownership_event, load_discovery_authority_receipt_by_action,
    load_discovery_authority_receipt_by_revision, load_native_no_effect_attestation,
    native_no_effect_evidence_sha256, project_reconciled_discovery_credential_ownership,
    validate_cancelled_pre_store_interruption_receipt,
    validate_discovery_operation_interrupted_audit, validate_discovery_operation_start_audit,
    validate_discovery_operation_terminal_audit_order_for_receipt,
    validate_discovery_receipt_follows, validate_exact_discovery_authority_audit,
    validate_interrupted_discovery_authority_receipt,
    validate_interrupted_discovery_operation_evidence,
    validate_native_no_effect_operation_start_receipt,
};
pub(crate) use credential_execution::{
    validate_archived_discovery_credential_ownership_authority_for_slot_gc,
    validate_discovery_credential_ownership_authority,
    validate_native_no_effect_attestation_integrity,
};
use errors::{contract_error, corrupted, database_error, discovery_error};
use queries::{
    load_discovery_native_credential_execution, load_operation_by_id, load_pollable_outbox_rows,
    load_pollable_outbox_rows_for_session, load_session_snapshot,
};
use repository_io::{
    compensation_status_transition_allowed, ensure_foreign_keys_clean,
    validate_discovery_native_physical_authority_id,
};
use row_mapping::{
    ApprovalRow, CompensationRow, decode_approval_row, decode_compensation_row, decode_evidence_row,
};
#[cfg(test)]
use transition_store::validate_completed_operation_binding;
use transition_store::{
    audit_kind_for_action, map_discovery_effect, persist_transition_in_transaction,
    validate_discovery_local_network_approval_binding, validate_prepared_commit,
};
#[cfg(test)]
use validation::is_pristine_discovery_session;
use validation::{
    ensure_provider_credential_operation_settled_for_discovery, looks_like_secret, require_session,
    validate_approval_references, validate_atomic_discovery_begin,
    validate_candidate_evidence_references, validate_capability_probe_grant,
    validate_discovery_evidence, validate_identifier, validate_limit,
    validate_opaque_credential_reference, validate_persistable_discovery_url,
    validate_redacted_value, validate_review_evidence_references, validate_sanitized_input,
    validate_session_evidence_ids, validate_sha256, validate_transition_write,
};
#[cfg(test)]
mod tests;
