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

const DISCOVERY_REDACTION_VERSION: u32 = 1;
const NATIVE_NO_EFFECT_ATTESTATION_SCHEMA_VERSION: u32 = 1;
const NATIVE_NO_EFFECT_ATTESTATION_REDACTION_VERSION: u32 = 1;
const NATIVE_NO_EFFECT_ATTESTATION_INTEGRITY_PAGE_SIZE: u32 = 256;

#[derive(Serialize)]
struct NativeNoEffectAttestationEvidence<'a> {
    schema_version: u32,
    attestation_kind: &'a str,
    recovery_owner: &'a str,
    operation_id: &'a str,
    session_id: &'a str,
    commit_attempt_id: &'a str,
    commit_plan_sha256: &'a str,
    connection_id: &'a str,
}

#[derive(Serialize)]
struct NativeNoEffectExecutionBindingEvidence<'a> {
    schema_version: u32,
    redaction_version: u32,
    operation_id: &'a str,
    physical_authority_id: &'a str,
    session_id: &'a str,
    commit_attempt_id: &'a str,
    commit_plan_sha256: &'a str,
    connection_id: &'a str,
    connection_binding_sha256: &'a str,
    attestation_evidence_sha256: &'a str,
    attested_at: &'a str,
}

impl Storage {
    /// Atomically stores a native missing-slot attestation and the exact
    /// `interrupted` transition it authorizes.
    pub fn persist_native_no_effect_discovery_transition(
        &self,
        write: &DiscoveryTransitionWrite,
        attestation: &DiscoveryNativeNoEffectAttestationWrite,
    ) -> CoreResult<PersistDiscoveryTransition> {
        validate_transition_write(write)?;
        validate_native_no_effect_attestation_write(write, attestation)?;
        let mut connection = self.connection()?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(database_error)?;
        insert_or_validate_native_no_effect_attestation(&transaction, write, attestation)?;
        let result = persist_transition_in_transaction(&transaction, write, None)?;
        transaction.commit().map_err(database_error)?;
        Ok(result)
    }

    /// Reserves a fresh physical authority while the semantic operation stays
    /// Prepared. This lets the host finish every fallible slot precondition
    /// without granting recovery permission to adopt an external effect.
    pub fn reserve_discovery_credential_install_execution(
        &self,
        reservation: &DiscoveryNativeCredentialExecutionReservation,
    ) -> CoreResult<DiscoveryNativeCredentialExecutionRecord> {
        validate_sha256(
            "discovery credential execution plan hash",
            &reservation.commit_plan_sha256,
        )?;
        validate_sha256(
            "discovery credential execution connection binding",
            &reservation.connection_binding_sha256,
        )?;
        let mut connection = self.connection()?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(database_error)?;
        validate_native_credential_execution_reservation(&transaction, reservation)?;
        if load_discovery_native_credential_execution(&transaction, &reservation.operation_id)?
            .is_some()
        {
            return Err(CoreError::invalid(
                "discovery credential operation already has a native execution authority",
            ));
        }

        let physical_authority_id = format!("discovery-native-{}", Uuid::new_v4());
        transaction
            .execute(
                "INSERT INTO provider_discovery_native_credential_executions (
                     physical_authority_id, operation_id, session_id,
                     commit_attempt_id, commit_plan_sha256, connection_id,
                     connection_binding_sha256, reserved_at,
                     schema_version, redaction_version
                ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, 1, 1)",
                params![
                    physical_authority_id,
                    reservation.operation_id.as_str(),
                    reservation.session_id.as_str(),
                    reservation.commit_attempt_id.as_str(),
                    reservation.commit_plan_sha256,
                    reservation.connection_id.as_str(),
                    reservation.connection_binding_sha256,
                    reservation.reserved_at.to_rfc3339(),
                ],
            )
            .map_err(database_error)?;
        let execution =
            load_discovery_native_credential_execution(&transaction, &reservation.operation_id)?
                .ok_or_else(|| {
                    corrupted("reserved discovery credential execution was not durably recorded")
                })?;
        if execution.store_started_at.is_some() {
            return Err(corrupted(
                "fresh discovery credential reservation already has a store attempt",
            ));
        }
        transaction.commit().map_err(database_error)?;
        Ok(execution)
    }

    /// Commits an append-only store-attempt intent and moves the operation
    /// Prepared -> Started with its audit in one IMMEDIATE transaction.
    pub fn start_reserved_discovery_credential_install_execution(
        &self,
        start: &DiscoveryNativeCredentialStoreAttemptStart,
    ) -> CoreResult<DiscoveryNativeCredentialExecutionRecord> {
        validate_discovery_native_physical_authority_id(&start.physical_authority_id)?;
        let mut connection = self.connection()?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(database_error)?;
        let execution =
            load_discovery_native_credential_execution(&transaction, &start.operation_id)?
                .ok_or_else(|| {
                    CoreError::invalid("discovery credential execution is not reserved")
                })?;
        let operation = load_operation_by_id(&transaction, &start.operation_id)?;
        let snapshot = load_session_snapshot(&transaction, execution.session_id.as_str())?
            .ok_or_else(|| corrupted("reserved discovery credential session is missing"))?;
        let attempt = load_commit_attempt(&transaction, &execution.commit_attempt_id)?;
        if execution.physical_authority_id != start.physical_authority_id
            || execution.store_started_at.is_some()
            || start.started_at < execution.reserved_at
            || operation.status != DiscoveryOperationStatus::Prepared
            || operation.started_at.is_some()
            || operation.finished_at.is_some()
            || snapshot.session.state != DiscoveryState::Committing
            || snapshot.active_operation_id.as_ref() != Some(&start.operation_id)
            || snapshot.session.revision != operation.expected_revision
            || snapshot.session.commit_attempt_id.as_ref() != Some(&attempt.id)
            || snapshot.session.commit_plan_sha256.as_deref() != Some(attempt.plan_sha256.as_str())
            || snapshot.session.cancellation_pending
            || attempt.phase != DiscoveryCommitPhase::Prepared
            || attempt.plan_sha256 != execution.commit_plan_sha256
        {
            return Err(CoreError::invalid(
                "native credential store attempt is detached from its reservation",
            ));
        }
        transaction
            .execute(
                "INSERT INTO provider_discovery_native_credential_store_attempts (
                     operation_id, physical_authority_id, started_at,
                     schema_version, redaction_version
                 ) VALUES (?1, ?2, ?3, 1, 1)",
                params![
                    start.operation_id.as_str(),
                    start.physical_authority_id,
                    start.started_at.to_rfc3339(),
                ],
            )
            .map_err(database_error)?;
        let changed = transaction
            .execute(
                "UPDATE provider_discovery_operations
                 SET status = 'started', started_at = ?2, updated_at = ?2
                 WHERE id = ?1 AND status = 'prepared' AND started_at IS NULL
                   AND finished_at IS NULL",
                params![start.operation_id.as_str(), start.started_at.to_rfc3339()],
            )
            .map_err(database_error)?;
        if changed != 1 {
            return Err(CoreError::invalid(
                "discovery credential operation changed before its store attempt",
            ));
        }
        append_audit(
            &transaction,
            execution.session_id.as_str(),
            operation.expected_revision,
            "operation_started",
            Some(operation.action_id.as_str()),
            Some(start.operation_id.as_str()),
            "discovery.audit.operation_started",
            start.started_at,
        )?;
        let started =
            load_discovery_native_credential_execution(&transaction, &start.operation_id)?
                .ok_or_else(|| corrupted("started discovery credential execution disappeared"))?;
        if started.store_started_at != Some(start.started_at) {
            return Err(corrupted(
                "started discovery credential execution lost its store-attempt cutpoint",
            ));
        }
        transaction.commit().map_err(database_error)?;
        Ok(started)
    }

    /// Revalidates that the active native credential-install operation was
    /// created by the exact immutable commit-start receipt for this attempt.
    pub fn validate_discovery_credential_install_operation_authority(
        &self,
        session_id: &DiscoverySessionId,
        attempt_id: &DiscoveryCommitAttemptId,
        plan_sha256: &str,
        operation_id: &DiscoveryOperationId,
    ) -> CoreResult<()> {
        validate_sha256("discovery credential install plan hash", plan_sha256)?;
        let connection = self.connection()?;
        let attempt = load_commit_attempt(&connection, attempt_id)?;
        let snapshot = load_session_snapshot(&connection, session_id.as_str())?
            .ok_or_else(|| corrupted("discovery credential install session is missing"))?;
        let operation = load_operation_by_id(&connection, operation_id)?;
        if attempt.session_id != *session_id
            || attempt.phase != DiscoveryCommitPhase::Prepared
            || attempt.plan_sha256 != plan_sha256
            || attempt.plan.attempt_id != *attempt_id
            || snapshot.session.state != DiscoveryState::Committing
            || snapshot.session.commit_attempt_id.as_ref() != Some(attempt_id)
            || snapshot.session.commit_plan_sha256.as_deref() != Some(plan_sha256)
            || snapshot.active_operation_id.as_ref() != Some(operation_id)
            || operation.session_id != *session_id
            || operation.kind != DiscoveryOperationKind::AtomicCommit
            || operation.side_effect_class != DiscoverySideEffectClass::Persistent
            || !matches!(
                operation.status,
                DiscoveryOperationStatus::Prepared | DiscoveryOperationStatus::Started
            )
            || operation.expected_revision != snapshot.session.revision
        {
            return Err(corrupted(
                "discovery credential install operation is detached from its active commit",
            ));
        }
        validate_native_no_effect_operation_start_receipt(
            &connection,
            &attempt,
            operation.action_id.as_str(),
            operation.expected_revision,
            &operation.request_sha256,
            &operation.created_at.to_rfc3339(),
        )
        .map(|_| ())
    }

    /// Revalidates a credential-install operation before crash recovery.
    ///
    /// Unlike the normal execution boundary, recovery may observe an exact
    /// canonical cancellation chain after the immutable commit-start receipt.
    /// Only a Prepared or Started operation whose current session is the final
    /// durable `Cancel` response is accepted with that revision drift. The
    /// Prepared form covers the crash boundary between persisting cancellation
    /// and settling the still-unstarted operation.
    pub fn validate_discovery_credential_install_recovery_authority(
        &self,
        session_id: &DiscoverySessionId,
        attempt_id: &DiscoveryCommitAttemptId,
        plan_sha256: &str,
        operation_id: &DiscoveryOperationId,
    ) -> CoreResult<()> {
        validate_sha256("discovery credential install plan hash", plan_sha256)?;
        let connection = self.connection()?;
        let attempt = load_commit_attempt(&connection, attempt_id)?;
        let snapshot = load_session_snapshot(&connection, session_id.as_str())?
            .ok_or_else(|| corrupted("discovery credential install session is missing"))?;
        let operation = load_operation_by_id(&connection, operation_id)?;
        if attempt.session_id != *session_id
            || attempt.phase != DiscoveryCommitPhase::Prepared
            || attempt.plan_sha256 != plan_sha256
            || attempt.plan.attempt_id != *attempt_id
            || snapshot.session.state != DiscoveryState::Committing
            || snapshot.session.commit_attempt_id.as_ref() != Some(attempt_id)
            || snapshot.session.commit_plan_sha256.as_deref() != Some(plan_sha256)
            || snapshot.active_operation_id.as_ref() != Some(operation_id)
            || operation.session_id != *session_id
            || operation.kind != DiscoveryOperationKind::AtomicCommit
            || operation.side_effect_class != DiscoverySideEffectClass::Persistent
            || !matches!(
                operation.status,
                DiscoveryOperationStatus::Prepared | DiscoveryOperationStatus::Started
            )
            || operation.expected_revision > snapshot.session.revision
        {
            return Err(corrupted(
                "discovery credential recovery operation is detached from its active commit",
            ));
        }
        let start = validate_native_no_effect_operation_start_receipt(
            &connection,
            &attempt,
            operation.action_id.as_str(),
            operation.expected_revision,
            &operation.request_sha256,
            &operation.created_at.to_rfc3339(),
        )?;
        if snapshot.session.cancellation_pending {
            if snapshot.session.revision <= operation.expected_revision {
                return Err(corrupted(
                    "discovery credential recovery cancellation has no revision advance",
                ));
            }
            validate_active_discovery_credential_cancellation_chain(
                &connection,
                &attempt,
                &snapshot,
                &start,
            )?;
        } else if operation.expected_revision != snapshot.session.revision
            || start.transition.session != snapshot.session
        {
            return Err(corrupted(
                "discovery credential recovery operation has unexplained revision drift",
            ));
        }
        match (
            operation.status,
            load_discovery_native_credential_execution(&connection, operation_id)?,
        ) {
            (DiscoveryOperationStatus::Prepared | DiscoveryOperationStatus::Started, None) => {
                Ok(())
            }
            (DiscoveryOperationStatus::Prepared, Some(execution))
                if execution.store_started_at.is_none() =>
            {
                Ok(())
            }
            (DiscoveryOperationStatus::Started, Some(execution))
                if execution.store_started_at == operation.started_at =>
            {
                Ok(())
            }
            _ => Err(corrupted(
                "discovery credential recovery operation has no exact native execution state",
            )),
        }
    }

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

fn record_discovery_selection_restore_authority(
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

fn load_discovery_selection_restore_revision(
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

fn validate_native_no_effect_attestation_write(
    write: &DiscoveryTransitionWrite,
    attestation: &DiscoveryNativeNoEffectAttestationWrite,
) -> CoreResult<()> {
    let completed = write.completed_operation.as_ref().ok_or_else(|| {
        CoreError::invalid("native no-effect attestation has no completed operation")
    })?;
    validate_sha256(
        "native no-effect commit plan hash",
        &attestation.commit_plan_sha256,
    )?;
    validate_sha256(
        "native no-effect evidence hash",
        &attestation.evidence_sha256,
    )?;
    validate_discovery_native_physical_authority_id(&attestation.physical_authority_id)
        .map_err(|_| CoreError::invalid("native no-effect physical authority is invalid"))?;
    if completed.outcome != DurableOperationOutcome::AttestedNoExternalEffect
        || completed.id != attestation.operation_id
        || write.transition.receipt.action_kind != "interrupt"
        || write.transition.session.state != DiscoveryState::Interrupted
        || write.transition.session.id != attestation.session_id
        || write.transition.session.commit_attempt_id.as_ref()
            != Some(&attestation.commit_attempt_id)
        || write.transition.session.commit_plan_sha256.as_deref()
            != Some(attestation.commit_plan_sha256.as_str())
        || attestation.kind != DiscoveryNativeNoEffectAttestationKind::CredentialSlotMissing
        || attestation.recovery_owner != DiscoveryNativeRecoveryOwner::NativePlatform
        || write.new_operation_id.is_some()
        || write.approval.is_some()
        || write.prepared_commit.is_some()
        || write.provider_graph.is_some()
        || !write.new_evidence.is_empty()
        || !write.new_candidates.is_empty()
    {
        return Err(CoreError::invalid(
            "native no-effect attestation does not match the exact interrupt transition",
        ));
    }
    let expected_evidence_sha256 = native_no_effect_evidence_sha256(attestation)?;
    if attestation.evidence_sha256 != expected_evidence_sha256 {
        return Err(CoreError::invalid(
            "native no-effect attestation evidence hash does not match its binding",
        ));
    }
    Ok(())
}

fn insert_or_validate_native_no_effect_attestation(
    transaction: &Transaction<'_>,
    write: &DiscoveryTransitionWrite,
    attestation: &DiscoveryNativeNoEffectAttestationWrite,
) -> CoreResult<()> {
    if let Some(existing) =
        load_native_no_effect_attestation(transaction, attestation.operation_id.as_str())?
    {
        if existing.operation_id == attestation.operation_id
            && existing.physical_authority_id == attestation.physical_authority_id
            && existing.session_id == attestation.session_id
            && existing.commit_attempt_id == attestation.commit_attempt_id
            && existing.commit_plan_sha256 == attestation.commit_plan_sha256
            && existing.connection_id == attestation.connection_id
            && existing.kind == attestation.kind
            && existing.recovery_owner == attestation.recovery_owner
            && existing.evidence_sha256 == attestation.evidence_sha256
            && existing.attested_at == write.occurred_at
        {
            return Ok(());
        }
        return Err(CoreError::invalid(
            "native no-effect attestation operation id is already bound differently",
        ));
    }

    let execution =
        validate_native_no_effect_database_binding(transaction, attestation, write.occurred_at)?;
    let execution_binding_sha256 = native_no_effect_execution_binding_sha256(
        attestation,
        &execution.connection_binding_sha256,
        write.occurred_at,
    )?;
    transaction
        .execute(
            "INSERT INTO provider_discovery_native_no_effect_execution_bindings (
                 operation_id, physical_authority_id, session_id,
                 commit_attempt_id, commit_plan_sha256, connection_id,
                 connection_binding_sha256, attestation_evidence_sha256,
                 execution_binding_sha256, attested_at,
                 schema_version, redaction_version
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, 1, 1)",
            params![
                attestation.operation_id.as_str(),
                attestation.physical_authority_id,
                attestation.session_id.as_str(),
                attestation.commit_attempt_id.as_str(),
                attestation.commit_plan_sha256,
                attestation.connection_id.as_str(),
                execution.connection_binding_sha256,
                attestation.evidence_sha256,
                execution_binding_sha256,
                write.occurred_at.to_rfc3339(),
            ],
        )
        .map_err(database_error)?;
    transaction
        .execute(
            "INSERT INTO provider_discovery_native_no_effect_attestations (
                 operation_id,
                 session_id,
                 commit_attempt_id,
                 commit_plan_sha256,
                 connection_id,
                 attestation_kind,
                 evidence_sha256,
                 recovery_owner,
                 schema_version,
                 redaction_version,
                 attested_at
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
            params![
                attestation.operation_id.as_str(),
                attestation.session_id.as_str(),
                attestation.commit_attempt_id.as_str(),
                attestation.commit_plan_sha256,
                attestation.connection_id.as_str(),
                attestation.kind.as_str(),
                attestation.evidence_sha256,
                attestation.recovery_owner.as_str(),
                NATIVE_NO_EFFECT_ATTESTATION_SCHEMA_VERSION,
                NATIVE_NO_EFFECT_ATTESTATION_REDACTION_VERSION,
                write.occurred_at.to_rfc3339(),
            ],
        )
        .map_err(database_error)?;
    Ok(())
}

fn validate_native_credential_execution_reservation(
    transaction: &Transaction<'_>,
    reservation: &DiscoveryNativeCredentialExecutionReservation,
) -> CoreResult<()> {
    let snapshot = load_session_snapshot(transaction, reservation.session_id.as_str())?
        .ok_or_else(|| corrupted("discovery credential execution session is missing"))?;
    let attempt = load_commit_attempt(transaction, &reservation.commit_attempt_id)?;
    let operation = load_operation_by_id(transaction, &reservation.operation_id)?;
    if snapshot.session.state != DiscoveryState::Committing
        || snapshot.session.revision != operation.expected_revision
        || snapshot.active_operation_id.as_ref() != Some(&reservation.operation_id)
        || snapshot.session.commit_attempt_id.as_ref() != Some(&reservation.commit_attempt_id)
        || snapshot.session.commit_plan_sha256.as_deref()
            != Some(reservation.commit_plan_sha256.as_str())
        || snapshot.session.cancellation_pending
        || operation.session_id != reservation.session_id
        || operation.kind != DiscoveryOperationKind::AtomicCommit
        || operation.side_effect_class != DiscoverySideEffectClass::Persistent
        || operation.status != DiscoveryOperationStatus::Prepared
        || operation.started_at.is_some()
        || operation.finished_at.is_some()
        || reservation.reserved_at < operation.created_at
        || attempt.session_id != reservation.session_id
        || attempt.id != reservation.commit_attempt_id
        || attempt.phase != DiscoveryCommitPhase::Prepared
        || attempt.plan_sha256 != reservation.commit_plan_sha256
        || attempt.plan.attempt_id != reservation.commit_attempt_id
        || attempt.plan.connection_id != reservation.connection_id
        || attempt
            .plan
            .credential_ref
            .as_ref()
            .map(CredentialRef::as_str)
            != Some(reservation.connection_id.as_str())
    {
        return Err(CoreError::invalid(
            "native credential reservation is detached from its prepared discovery commit",
        ));
    }
    validate_native_no_effect_operation_start_receipt(
        transaction,
        &attempt,
        operation.action_id.as_str(),
        operation.expected_revision,
        &operation.request_sha256,
        &operation.created_at.to_rfc3339(),
    )?;
    let authorized = transaction
        .query_row(
            "SELECT COUNT(*)
             FROM provider_discovery_authorized_native_commit_starts
             WHERE operation_id = ?1
               AND session_id = ?2
               AND commit_attempt_id = ?3
               AND commit_plan_sha256 = ?4
               AND operation_expected_revision = ?5",
            params![
                reservation.operation_id.as_str(),
                reservation.session_id.as_str(),
                reservation.commit_attempt_id.as_str(),
                reservation.commit_plan_sha256,
                operation.expected_revision,
            ],
            |row| row.get::<_, u64>(0),
        )
        .map_err(database_error)?;
    if authorized != 1 {
        return Err(corrupted(
            "native credential reservation has no unique approved start authority",
        ));
    }
    Ok(())
}

type NativeNoEffectOperationRow = (
    String,
    String,
    String,
    String,
    u64,
    String,
    String,
    String,
    Option<String>,
    String,
    u64,
    Option<String>,
    Option<String>,
    Option<String>,
);

fn load_native_no_effect_operation_row(
    connection: &Connection,
    operation_id: &DiscoveryOperationId,
) -> CoreResult<NativeNoEffectOperationRow> {
    connection
        .query_row(
            "SELECT operation.session_id,
                    operation.operation_kind,
                    operation.side_effect_class,
                    operation.status,
                    operation.expected_revision,
                    operation.action_id,
                    operation.request_sha256,
                    operation.created_at,
                    operation.started_at,
                    session.state,
                    session.revision,
                    session.active_operation_id,
                    session.commit_attempt_id,
                    session.commit_plan_sha256
             FROM provider_discovery_operations AS operation
             JOIN provider_discovery_sessions AS session
               ON session.id = operation.session_id
             WHERE operation.id = ?1",
            [operation_id.as_str()],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, u64>(4)?,
                    row.get::<_, String>(5)?,
                    row.get::<_, String>(6)?,
                    row.get::<_, String>(7)?,
                    row.get::<_, Option<String>>(8)?,
                    row.get::<_, String>(9)?,
                    row.get::<_, u64>(10)?,
                    row.get::<_, Option<String>>(11)?,
                    row.get::<_, Option<String>>(12)?,
                    row.get::<_, Option<String>>(13)?,
                ))
            },
        )
        .optional()
        .map_err(database_error)?
        .ok_or_else(|| {
            CoreError::new(
                CoreErrorCode::NotFound,
                "native no-effect attestation operation was not found",
                false,
            )
        })
}

fn validate_native_no_effect_database_binding(
    transaction: &Transaction<'_>,
    attestation: &DiscoveryNativeNoEffectAttestationWrite,
    attested_at: DateTime<Utc>,
) -> CoreResult<DiscoveryNativeCredentialExecutionRecord> {
    let operation = load_native_no_effect_operation_row(transaction, &attestation.operation_id)?;
    let attempt = load_commit_attempt(transaction, &attestation.commit_attempt_id)?;
    let execution =
        load_discovery_native_credential_execution(transaction, &attestation.operation_id)?
            .ok_or_else(|| CoreError::invalid("native no-effect execution is missing"))?;
    let created_at = parse_timestamp(&operation.7, "native no-effect operation created_at")?;
    let started_at = operation
        .8
        .as_deref()
        .ok_or_else(|| CoreError::invalid("native no-effect operation was not started"))
        .and_then(|value| parse_timestamp(value, "native no-effect operation started_at"))?;
    if operation.0 != attestation.session_id.as_str()
        || operation.1 != "atomic_commit"
        || operation.2 != "persistent"
        || operation.3 != "started"
        || created_at > started_at
        || started_at > attested_at
        || operation.4 != operation.10
        || operation.9 != "committing"
        || operation.11.as_deref() != Some(attestation.operation_id.as_str())
        || operation.12.as_deref() != Some(attestation.commit_attempt_id.as_str())
        || operation.13.as_deref() != Some(attestation.commit_plan_sha256.as_str())
        || attempt.session_id != attestation.session_id
        || attempt.phase != DiscoveryCommitPhase::Prepared
        || attempt.plan_sha256 != attestation.commit_plan_sha256
        || attempt.plan.attempt_id != attestation.commit_attempt_id
        || attempt.plan.connection_id != attestation.connection_id
        || attempt
            .plan
            .credential_ref
            .as_ref()
            .map(|value| value.0.as_str())
            != Some(attestation.connection_id.as_str())
        || execution.physical_authority_id != attestation.physical_authority_id
        || execution.operation_id != attestation.operation_id
        || execution.session_id != attestation.session_id
        || execution.commit_attempt_id != attestation.commit_attempt_id
        || execution.commit_plan_sha256 != attestation.commit_plan_sha256
        || execution.connection_id != attestation.connection_id
        || execution.store_started_at != Some(started_at)
    {
        return Err(CoreError::invalid(
            "native no-effect attestation is detached from the active credential commit",
        ));
    }
    validate_native_no_effect_operation_start_receipt(
        transaction,
        &attempt,
        &operation.5,
        operation.4,
        &operation.6,
        &operation.7,
    )?;
    Ok(execution)
}

type NativeNoEffectAttestationRow = (
    String,
    String,
    String,
    String,
    String,
    String,
    String,
    String,
    u32,
    u32,
    String,
);

fn is_exact_legacy_native_no_effect_snapshot(
    connection: &Connection,
    row: &NativeNoEffectAttestationRow,
) -> CoreResult<bool> {
    connection
        .query_row(
            "SELECT COUNT(*)
             FROM provider_discovery_native_no_effect_legacy_cutoff_snapshots
             WHERE operation_id = ?1
               AND session_id = ?2
               AND commit_attempt_id = ?3
               AND commit_plan_sha256 = ?4
               AND connection_id = ?5
               AND attestation_kind = ?6
               AND evidence_sha256 = ?7
               AND recovery_owner = ?8
               AND attestation_schema_version = ?9
               AND attestation_redaction_version = ?10
               AND attested_at = ?11
               AND cutoff_before_schema_version = 37
               AND snapshot_schema_version = 1",
            params![
                row.0, row.1, row.2, row.3, row.4, row.5, row.6, row.7, row.8, row.9, row.10,
            ],
            |query_row| query_row.get::<_, u64>(0),
        )
        .map(|count| count == 1)
        .map_err(database_error)
}

fn validate_legacy_native_no_effect_attestation(
    connection: &Connection,
    row: &NativeNoEffectAttestationRow,
) -> CoreResult<()> {
    let operation_id = DiscoveryOperationId::parse(row.0.clone())
        .map_err(|_| corrupted("legacy native no-effect operation id is invalid"))?;
    let session_id = DiscoverySessionId::from(row.1.clone());
    let attempt_id = DiscoveryCommitAttemptId::parse(row.2.clone())
        .map_err(|_| corrupted("legacy native no-effect commit attempt id is invalid"))?;
    let connection_id = ProviderConnectionId::from(row.4.clone());
    let kind = DiscoveryNativeNoEffectAttestationKind::parse(&row.5)?;
    let recovery_owner = DiscoveryNativeRecoveryOwner::parse(&row.7)?;
    let attested_at = parse_timestamp(&row.10, "legacy native no-effect attested_at")?;
    let expected_evidence_sha256 = native_no_effect_binding_sha256(
        kind,
        recovery_owner,
        operation_id.as_str(),
        session_id.as_str(),
        attempt_id.as_str(),
        &row.3,
        connection_id.as_str(),
    )?;
    if expected_evidence_sha256 != row.6 {
        return Err(corrupted(
            "legacy native no-effect evidence hash does not match its semantic binding",
        ));
    }
    let operation = load_operation_by_id(connection, &operation_id)?;
    let attempt = load_commit_attempt(connection, &attempt_id).map_err(|error| {
        if error.code == CoreErrorCode::NotFound {
            corrupted("legacy native no-effect commit attempt is missing")
        } else {
            error
        }
    })?;
    let has_physical_execution = connection
        .query_row(
            "SELECT EXISTS(
                 SELECT 1
                 FROM provider_discovery_native_credential_executions
                 WHERE operation_id = ?1
             )",
            [operation_id.as_str()],
            |query_row| query_row.get::<_, bool>(0),
        )
        .map_err(database_error)?;
    if operation.session_id != session_id
        || operation.kind != DiscoveryOperationKind::AtomicCommit
        || operation.side_effect_class != DiscoverySideEffectClass::Persistent
        || operation.status != DiscoveryOperationStatus::Interrupted
        || operation.expected_revision != attempt.expected_revision.saturating_add(1)
        || operation.action_id != attempt.action_id
        || operation.finished_at != Some(attested_at)
        || attempt.session_id != session_id
        || attempt.plan_sha256 != row.3
        || attempt.plan.attempt_id != attempt_id
        || attempt.plan.connection_id != connection_id
        || attempt
            .plan
            .credential_ref
            .as_ref()
            .map(CredentialRef::as_str)
            != Some(connection_id.as_str())
        || has_physical_execution
    {
        return Err(corrupted(
            "legacy native no-effect attestation is detached from its historical commit",
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

fn load_native_no_effect_attestation_row(
    connection: &Connection,
    operation_id: &str,
) -> CoreResult<Option<NativeNoEffectAttestationRow>> {
    connection
        .query_row(
            "SELECT operation_id, session_id, commit_attempt_id, commit_plan_sha256,
                    connection_id, attestation_kind, evidence_sha256, recovery_owner,
                    schema_version, redaction_version, attested_at
             FROM provider_discovery_native_no_effect_attestations
             WHERE operation_id = ?1",
            [operation_id],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, String>(4)?,
                    row.get::<_, String>(5)?,
                    row.get::<_, String>(6)?,
                    row.get::<_, String>(7)?,
                    row.get::<_, u32>(8)?,
                    row.get::<_, u32>(9)?,
                    row.get::<_, String>(10)?,
                ))
            },
        )
        .optional()
        .map_err(database_error)
}

type NativeNoEffectExecutionBindingRow = (
    String,
    String,
    String,
    String,
    String,
    String,
    String,
    String,
    String,
    u32,
    u32,
);

fn load_native_no_effect_execution_binding(
    connection: &Connection,
    operation_id: &str,
) -> CoreResult<Option<NativeNoEffectExecutionBindingRow>> {
    connection
        .query_row(
            "SELECT physical_authority_id, session_id, commit_attempt_id,
                    commit_plan_sha256, connection_id,
                    connection_binding_sha256, attestation_evidence_sha256,
                    execution_binding_sha256, attested_at,
                    schema_version, redaction_version
             FROM provider_discovery_native_no_effect_execution_bindings
             WHERE operation_id = ?1",
            [operation_id],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, String>(4)?,
                    row.get::<_, String>(5)?,
                    row.get::<_, String>(6)?,
                    row.get::<_, String>(7)?,
                    row.get::<_, String>(8)?,
                    row.get::<_, u32>(9)?,
                    row.get::<_, u32>(10)?,
                ))
            },
        )
        .optional()
        .map_err(database_error)
}

fn load_native_no_effect_attestation(
    connection: &Connection,
    operation_id: &str,
) -> CoreResult<Option<DiscoveryNativeNoEffectAttestationRecord>> {
    let row = load_native_no_effect_attestation_row(connection, operation_id)?;
    let Some(row) = row else {
        return Ok(None);
    };
    if row.8 != NATIVE_NO_EFFECT_ATTESTATION_SCHEMA_VERSION
        || row.9 != NATIVE_NO_EFFECT_ATTESTATION_REDACTION_VERSION
    {
        return Err(corrupted(
            "stored native no-effect attestation version is invalid",
        ));
    }
    validate_sha256("stored native no-effect commit plan hash", &row.3)
        .map_err(|_| corrupted("stored native no-effect commit plan hash is invalid"))?;
    validate_sha256("stored native no-effect evidence hash", &row.6)
        .map_err(|_| corrupted("stored native no-effect evidence hash is invalid"))?;
    let exact_legacy_snapshot = is_exact_legacy_native_no_effect_snapshot(connection, &row)?;
    let binding = load_native_no_effect_execution_binding(connection, operation_id)?;
    if exact_legacy_snapshot {
        if binding.is_some() {
            return Err(corrupted(
                "legacy native no-effect history cannot acquire a physical execution binding",
            ));
        }
        validate_legacy_native_no_effect_attestation(connection, &row)?;
        return Ok(None);
    }
    let binding = binding.ok_or_else(|| {
        corrupted("stored native no-effect attestation has no physical execution binding")
    })?;
    validate_discovery_native_physical_authority_id(&binding.0)?;
    validate_sha256("stored native no-effect connection binding", &binding.5)
        .map_err(|_| corrupted("stored native no-effect connection binding is invalid"))?;
    validate_sha256("stored native no-effect execution binding", &binding.7)
        .map_err(|_| corrupted("stored native no-effect execution binding is invalid"))?;
    if binding.9 != 1
        || binding.10 != 1
        || binding.1 != row.1
        || binding.2 != row.2
        || binding.3 != row.3
        || binding.4 != row.4
        || binding.6 != row.6
        || binding.8 != row.10
    {
        return Err(corrupted(
            "stored native no-effect execution binding differs from its attestation",
        ));
    }
    let record = DiscoveryNativeNoEffectAttestationRecord {
        operation_id: DiscoveryOperationId::parse(row.0)
            .map_err(|_| corrupted("stored native no-effect operation id is invalid"))?,
        physical_authority_id: binding.0,
        session_id: DiscoverySessionId::from(row.1),
        commit_attempt_id: DiscoveryCommitAttemptId::parse(row.2)
            .map_err(|_| corrupted("stored native no-effect commit attempt id is invalid"))?,
        commit_plan_sha256: row.3,
        connection_id: ProviderConnectionId::from(row.4),
        kind: DiscoveryNativeNoEffectAttestationKind::parse(&row.5)?,
        evidence_sha256: row.6,
        connection_binding_sha256: binding.5,
        execution_binding_sha256: binding.7,
        recovery_owner: DiscoveryNativeRecoveryOwner::parse(&row.7)?,
        attested_at: parse_timestamp(&row.10, "native no-effect attested_at")?,
    };
    let expected = native_no_effect_evidence_sha256_from_record(&record)?;
    if record.evidence_sha256 != expected {
        return Err(corrupted(
            "stored native no-effect evidence hash does not match its binding",
        ));
    }
    validate_stored_native_no_effect_attestation_binding(connection, &record)?;
    Ok(Some(record))
}

fn validate_stored_native_no_effect_attestation_binding(
    connection: &Connection,
    attestation: &DiscoveryNativeNoEffectAttestationRecord,
) -> CoreResult<()> {
    let operation = connection
        .query_row(
            "SELECT session_id, operation_kind, side_effect_class, status,
                    expected_revision, action_id, request_sha256,
                    started_at, finished_at, created_at
             FROM provider_discovery_operations
             WHERE id = ?1",
            [attestation.operation_id.as_str()],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, u64>(4)?,
                    row.get::<_, String>(5)?,
                    row.get::<_, String>(6)?,
                    row.get::<_, Option<String>>(7)?,
                    row.get::<_, Option<String>>(8)?,
                    row.get::<_, String>(9)?,
                ))
            },
        )
        .optional()
        .map_err(database_error)?
        .ok_or_else(|| corrupted("stored native no-effect operation is missing"))?;
    let attempt =
        load_commit_attempt(connection, &attestation.commit_attempt_id).map_err(|error| {
            if error.code == CoreErrorCode::NotFound {
                corrupted("stored native no-effect commit attempt is missing")
            } else {
                error
            }
        })?;
    let execution =
        load_discovery_native_credential_execution(connection, &attestation.operation_id)?
            .ok_or_else(|| corrupted("stored native no-effect execution is missing"))?;
    let finished_at = operation
        .8
        .as_deref()
        .ok_or_else(|| corrupted("stored native no-effect operation is unfinished"))
        .and_then(|value| parse_timestamp(value, "native no-effect operation finished_at"))?;
    let started_at = operation
        .7
        .as_deref()
        .ok_or_else(|| corrupted("stored native no-effect operation was never started"))
        .and_then(|value| parse_timestamp(value, "native no-effect operation started_at"))?;
    let created_at = parse_timestamp(&operation.9, "native no-effect operation created_at")?;
    if operation.0 != attestation.session_id.as_str()
        || operation.1 != "atomic_commit"
        || operation.2 != "persistent"
        || operation.3 != "interrupted"
        || created_at > started_at
        || started_at > finished_at
        || finished_at != attestation.attested_at
        || attempt.session_id != attestation.session_id
        || attempt.plan_sha256 != attestation.commit_plan_sha256
        || attempt.plan.attempt_id != attestation.commit_attempt_id
        || attempt.plan.connection_id != attestation.connection_id
        || attempt
            .plan
            .credential_ref
            .as_ref()
            .map(|value| value.0.as_str())
            != Some(attestation.connection_id.as_str())
        || execution.physical_authority_id != attestation.physical_authority_id
        || execution.session_id != attestation.session_id
        || execution.commit_attempt_id != attestation.commit_attempt_id
        || execution.commit_plan_sha256 != attestation.commit_plan_sha256
        || execution.connection_id != attestation.connection_id
        || execution.connection_binding_sha256 != attestation.connection_binding_sha256
        || execution.store_started_at != Some(started_at)
    {
        return Err(corrupted(
            "stored native no-effect attestation is detached from its credential commit",
        ));
    }
    validate_native_no_effect_operation_start_receipt(
        connection,
        &attempt,
        &operation.5,
        operation.4,
        &operation.6,
        &operation.9,
    )?;
    let expected_execution_binding =
        native_no_effect_execution_binding_sha256_from_record(attestation)?;
    if attestation.execution_binding_sha256 != expected_execution_binding {
        return Err(corrupted(
            "stored native no-effect execution evidence hash does not match its physical binding",
        ));
    }
    Ok(())
}

fn validate_native_no_effect_operation_start_receipt(
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

pub(crate) fn validate_native_no_effect_attestation_integrity(
    connection: &Connection,
) -> CoreResult<()> {
    let has_orphan_binding = connection
        .query_row(
            "SELECT EXISTS(
                 SELECT 1
                 FROM provider_discovery_native_no_effect_execution_bindings AS binding
                 LEFT JOIN provider_discovery_native_no_effect_attestations AS attestation
                   ON attestation.operation_id = binding.operation_id
                 WHERE attestation.operation_id IS NULL
             )",
            [],
            |row| row.get::<_, bool>(0),
        )
        .map_err(database_error)?;
    let has_orphan_or_mismatched_legacy_snapshot = connection
        .query_row(
            "SELECT EXISTS(
                 SELECT 1
                 FROM provider_discovery_native_no_effect_legacy_cutoff_snapshots AS snapshot
                 LEFT JOIN provider_discovery_native_no_effect_attestations AS attestation
                   ON attestation.operation_id = snapshot.operation_id
                  AND attestation.session_id = snapshot.session_id
                  AND attestation.commit_attempt_id = snapshot.commit_attempt_id
                  AND attestation.commit_plan_sha256 = snapshot.commit_plan_sha256
                  AND attestation.connection_id = snapshot.connection_id
                  AND attestation.attestation_kind = snapshot.attestation_kind
                  AND attestation.evidence_sha256 = snapshot.evidence_sha256
                  AND attestation.recovery_owner = snapshot.recovery_owner
                  AND attestation.schema_version = snapshot.attestation_schema_version
                  AND attestation.redaction_version = snapshot.attestation_redaction_version
                  AND attestation.attested_at = snapshot.attested_at
                 WHERE attestation.operation_id IS NULL
                    OR snapshot.cutoff_before_schema_version <> 37
                    OR snapshot.snapshot_schema_version <> 1
             )",
            [],
            |row| row.get::<_, bool>(0),
        )
        .map_err(database_error)?;
    if has_orphan_binding || has_orphan_or_mismatched_legacy_snapshot {
        return Err(corrupted(
            "native no-effect physical or legacy evidence is orphaned from its attestation",
        ));
    }
    let mut after_operation_id = String::new();
    loop {
        let operation_ids = {
            let mut statement = connection
                .prepare(
                    "SELECT operation_id
                     FROM provider_discovery_native_no_effect_attestations
                     WHERE operation_id > ?1
                     ORDER BY operation_id
                     LIMIT ?2",
                )
                .map_err(database_error)?;
            statement
                .query_map(
                    params![
                        after_operation_id,
                        NATIVE_NO_EFFECT_ATTESTATION_INTEGRITY_PAGE_SIZE
                    ],
                    |row| row.get::<_, String>(0),
                )
                .map_err(database_error)?
                .collect::<Result<Vec<_>, _>>()
                .map_err(database_error)?
        };
        let Some(last_operation_id) = operation_ids.last().cloned() else {
            return Ok(());
        };
        for operation_id in operation_ids {
            let loaded = load_native_no_effect_attestation(connection, &operation_id)?;
            if loaded.is_none() {
                let legacy_snapshot = connection
                    .query_row(
                        "SELECT COUNT(*)
                         FROM provider_discovery_native_no_effect_legacy_cutoff_snapshots
                         WHERE operation_id = ?1",
                        [&operation_id],
                        |row| row.get::<_, u64>(0),
                    )
                    .map_err(database_error)?;
                if legacy_snapshot != 1 {
                    return Err(corrupted(
                        "native no-effect attestation disappeared during integrity validation",
                    ));
                }
            }
        }
        after_operation_id = last_operation_id;
    }
}

fn native_no_effect_evidence_sha256(
    attestation: &DiscoveryNativeNoEffectAttestationWrite,
) -> CoreResult<String> {
    native_no_effect_binding_sha256(
        attestation.kind,
        attestation.recovery_owner,
        attestation.operation_id.as_str(),
        attestation.session_id.as_str(),
        attestation.commit_attempt_id.as_str(),
        &attestation.commit_plan_sha256,
        attestation.connection_id.as_str(),
    )
}

fn native_no_effect_evidence_sha256_from_record(
    attestation: &DiscoveryNativeNoEffectAttestationRecord,
) -> CoreResult<String> {
    native_no_effect_binding_sha256(
        attestation.kind,
        attestation.recovery_owner,
        attestation.operation_id.as_str(),
        attestation.session_id.as_str(),
        attestation.commit_attempt_id.as_str(),
        &attestation.commit_plan_sha256,
        attestation.connection_id.as_str(),
    )
}

fn native_no_effect_execution_binding_sha256(
    attestation: &DiscoveryNativeNoEffectAttestationWrite,
    connection_binding_sha256: &str,
    attested_at: DateTime<Utc>,
) -> CoreResult<String> {
    let attested_at = attested_at.to_rfc3339();
    native_no_effect_execution_binding_sha256_inner(&NativeNoEffectExecutionBindingEvidence {
        schema_version: NATIVE_NO_EFFECT_ATTESTATION_SCHEMA_VERSION,
        redaction_version: NATIVE_NO_EFFECT_ATTESTATION_REDACTION_VERSION,
        operation_id: attestation.operation_id.as_str(),
        physical_authority_id: &attestation.physical_authority_id,
        session_id: attestation.session_id.as_str(),
        commit_attempt_id: attestation.commit_attempt_id.as_str(),
        commit_plan_sha256: &attestation.commit_plan_sha256,
        connection_id: attestation.connection_id.as_str(),
        connection_binding_sha256,
        attestation_evidence_sha256: &attestation.evidence_sha256,
        attested_at: &attested_at,
    })
}

fn native_no_effect_execution_binding_sha256_from_record(
    attestation: &DiscoveryNativeNoEffectAttestationRecord,
) -> CoreResult<String> {
    let attested_at = attestation.attested_at.to_rfc3339();
    native_no_effect_execution_binding_sha256_inner(&NativeNoEffectExecutionBindingEvidence {
        schema_version: NATIVE_NO_EFFECT_ATTESTATION_SCHEMA_VERSION,
        redaction_version: NATIVE_NO_EFFECT_ATTESTATION_REDACTION_VERSION,
        operation_id: attestation.operation_id.as_str(),
        physical_authority_id: &attestation.physical_authority_id,
        session_id: attestation.session_id.as_str(),
        commit_attempt_id: attestation.commit_attempt_id.as_str(),
        commit_plan_sha256: &attestation.commit_plan_sha256,
        connection_id: attestation.connection_id.as_str(),
        connection_binding_sha256: &attestation.connection_binding_sha256,
        attestation_evidence_sha256: &attestation.evidence_sha256,
        attested_at: &attested_at,
    })
}

fn native_no_effect_execution_binding_sha256_inner(
    evidence: &NativeNoEffectExecutionBindingEvidence<'_>,
) -> CoreResult<String> {
    let canonical = canonical_json_result(
        serde_json::to_value(evidence),
        "native no-effect execution binding evidence",
    )?;
    Ok(sha256_hex(canonical.as_bytes()))
}

fn native_no_effect_binding_sha256(
    kind: DiscoveryNativeNoEffectAttestationKind,
    recovery_owner: DiscoveryNativeRecoveryOwner,
    operation_id: &str,
    session_id: &str,
    commit_attempt_id: &str,
    commit_plan_sha256: &str,
    connection_id: &str,
) -> CoreResult<String> {
    let evidence = NativeNoEffectAttestationEvidence {
        schema_version: NATIVE_NO_EFFECT_ATTESTATION_SCHEMA_VERSION,
        attestation_kind: kind.as_str(),
        recovery_owner: recovery_owner.as_str(),
        operation_id,
        session_id,
        commit_attempt_id,
        commit_plan_sha256,
        connection_id,
    };
    let canonical = canonical_json_result(
        serde_json::to_value(evidence),
        "native no-effect attestation evidence",
    )?;
    Ok(sha256_hex(canonical.as_bytes()))
}

fn require_started_session_operation(
    transaction: &Transaction<'_>,
    session_id: &DiscoverySessionId,
    expected_kind: &str,
) -> CoreResult<DiscoveryOperationId> {
    let row = transaction
        .query_row(
            "SELECT session.active_operation_id, operation.operation_kind,
                    operation.side_effect_class, operation.status
             FROM provider_discovery_sessions AS session
             LEFT JOIN provider_discovery_operations AS operation
               ON operation.id = session.active_operation_id
              AND operation.session_id = session.id
             WHERE session.id = ?1",
            [session_id.as_str()],
            |row| {
                Ok((
                    row.get::<_, Option<String>>(0)?,
                    row.get::<_, Option<String>>(1)?,
                    row.get::<_, Option<String>>(2)?,
                    row.get::<_, Option<String>>(3)?,
                ))
            },
        )
        .optional()
        .map_err(database_error)?
        .ok_or_else(|| {
            CoreError::new(
                CoreErrorCode::NotFound,
                "provider discovery session was not found",
                false,
            )
        })?;
    let (Some(operation_id), Some(kind), Some(side_effect_class), Some(status)) = row else {
        return Err(corrupted(
            "discovery session has no durable active operation",
        ));
    };
    if kind != expected_kind || side_effect_class != "persistent" || status != "started" {
        return Err(CoreError::invalid(
            "persistent discovery work requires its exact durable operation to be started",
        ));
    }
    DiscoveryOperationId::parse(operation_id).map_err(contract_error)
}

fn ensure_provider_graph_ids_vacant(
    transaction: &Transaction<'_>,
    graph: &DiscoveredProviderGraph,
) -> CoreResult<()> {
    let connection_exists = transaction
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM provider_connections WHERE id = ?1)",
            [graph.connection.id.as_str()],
            |row| row.get::<_, bool>(0),
        )
        .map_err(database_error)?;
    if connection_exists {
        return Err(CoreError::invalid(
            "discovery commit connection identifier already belongs to another graph",
        ));
    }
    for route in &graph.routes {
        if transaction
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM provider_models WHERE id = ?1)",
                [route.id.as_str()],
                |row| row.get::<_, bool>(0),
            )
            .map_err(database_error)?
        {
            return Err(CoreError::invalid(
                "discovery commit model route identifier already exists",
            ));
        }
    }
    for observation in &graph.observations {
        if transaction
            .query_row(
                "SELECT EXISTS(
                     SELECT 1 FROM model_capability_observations WHERE id = ?1
                 )",
                [observation.id.as_str()],
                |row| row.get::<_, bool>(0),
            )
            .map_err(database_error)?
        {
            return Err(CoreError::invalid(
                "discovery commit capability observation identifier already exists",
            ));
        }
    }
    for preset in &graph.presets {
        if transaction
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM generation_presets WHERE id = ?1)",
                [preset.id.as_str()],
                |row| row.get::<_, bool>(0),
            )
            .map_err(database_error)?
        {
            return Err(CoreError::invalid(
                "discovery commit generation preset identifier already exists",
            ));
        }
    }
    Ok(())
}

#[allow(clippy::too_many_lines)]
fn apply_provider_graph_in_transaction(
    transaction: &Transaction<'_>,
    graph: &DiscoveredProviderGraph,
    expected_session_revision: u64,
    applied_at: DateTime<Utc>,
    authority_observed_at: DateTime<Utc>,
) -> CoreResult<()> {
    validate_provider_graph(graph)?;
    let plan_json = encode_commit_plan_json(&graph.plan)?;
    if sha256_hex(plan_json.as_bytes()) != graph.plan_sha256 {
        return Err(CoreError::invalid(
            "provider graph plan hash does not match its canonical plan",
        ));
    }
    let session = transaction
        .query_row(
            "SELECT state, revision, commit_attempt_id, commit_plan_sha256,
                    sanitized_input_json, created_at
             FROM provider_discovery_sessions
             WHERE id = ?1",
            [graph.plan.session_id.as_str()],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, u64>(1)?,
                    row.get::<_, Option<String>>(2)?,
                    row.get::<_, Option<String>>(3)?,
                    row.get::<_, String>(4)?,
                    row.get::<_, String>(5)?,
                ))
            },
        )
        .optional()
        .map_err(database_error)?
        .ok_or_else(|| {
            CoreError::new(
                CoreErrorCode::NotFound,
                "provider discovery session was not found",
                false,
            )
        })?;
    if session.0 != "committing"
        || session.1 != expected_session_revision
        || session.2.as_deref() != Some(graph.plan.attempt_id.as_str())
        || session.3.as_deref() != Some(graph.plan_sha256.as_str())
        || graph.plan.expected_revision >= expected_session_revision
    {
        return Err(CoreError::invalid(
            "provider graph commit does not match the active discovery revision",
        ));
    }
    let input = serde_json::from_str::<SanitizedDiscoveryInput>(&session.4)
        .map_err(|_| corrupted("committing discovery input is invalid"))?;
    input
        .validate()
        .map_err(|_| corrupted("committing discovery input violates its contract"))?;
    validate_sanitized_input(&input)
        .map_err(|_| corrupted("committing discovery input contains forbidden data"))?;
    let session_created_at =
        parse_timestamp(&session.5, "committing discovery session creation time")?;
    validate_discovery_local_network_approval_binding(
        &input,
        session_created_at,
        authority_observed_at,
    )?;
    if graph.connection.id != input.connection_id
        || graph.connection.display_name != input.display_name
        || graph.connection.credential_ref != input.credential_ref
    {
        return Err(CoreError::invalid(
            "provider graph connection differs from the user-selected identity",
        ));
    }
    if graph.connection.config.network_mode != input.connection_options.network_mode
        || graph.connection.config.local_network_approval
            != input.connection_options.local_network_approval
    {
        return Err(CoreError::invalid(
            "provider graph network authority differs from its discovery session",
        ));
    }
    if input.connection_options.network_mode == ProviderNetworkMode::ApprovedLocalNetwork {
        let approval = input
            .connection_options
            .local_network_approval
            .as_ref()
            .ok_or_else(|| corrupted("committing LAN discovery approval is missing"))?;
        if graph.connection.created_at != session_created_at
            || graph.connection.api_origin != approval.origin
        {
            return Err(CoreError::invalid(
                "provider graph laundered its immutable LAN approval authority",
            ));
        }
    }
    require_started_session_operation(transaction, &graph.plan.session_id, "atomic_commit")?;
    let attempt = transaction
        .query_row(
            "SELECT plan_sha256, plan_json, phase
             FROM provider_discovery_commit_attempts
             WHERE id = ?1 AND session_id = ?2",
            params![
                graph.plan.attempt_id.as_str(),
                graph.plan.session_id.as_str()
            ],
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
        .ok_or_else(|| corrupted("active discovery commit attempt is missing"))?;
    if attempt.0 != graph.plan_sha256 || attempt.1 != plan_json {
        return Err(CoreError::invalid(
            "provider graph differs from its immutable commit attempt",
        ));
    }
    if !matches!(attempt.2.as_str(), "prepared" | "database_applied") {
        return Err(CoreError::invalid(
            "provider graph can only be applied from the prepared phase",
        ));
    }
    validate_review_approval(transaction, &graph.plan)?;
    validate_credential_approval(transaction, graph)?;
    validate_graph_evidence_references(transaction, graph)?;
    let requested_ownership_hash = provider_graph_ownership_hash(
        &graph.template,
        &graph.connection,
        &graph.routes,
        &graph.observations,
        &graph.presets,
    )?;
    if requested_ownership_hash != graph.plan.graph_sha256 {
        return Err(CoreError::invalid(
            "provider graph differs from the graph digest approved in the immutable commit plan",
        ));
    }
    if attempt.2 == "database_applied" {
        let stored_graph = load_discovered_provider_graph_rows(
            transaction,
            &graph.plan.template_id,
            graph.plan.template_version,
            &graph.plan.connection_id,
        )?
        .ok_or_else(|| corrupted("database-applied discovery graph is missing"))?;
        if stored_provider_graph_ownership_hash(&stored_graph)? != requested_ownership_hash
            || graph_ownership_audit_hash(transaction, &graph.plan.session_id)?
                != requested_ownership_hash
        {
            return Err(CoreError::invalid(
                "database-applied discovery graph differs from its immutable ownership record",
            ));
        }
        return Ok(());
    }
    validate_catalog_authority_in_transaction(transaction, graph, authority_observed_at)?;
    ensure_provider_graph_ids_vacant(transaction, graph)?;
    let template_existed = transaction
        .query_row(
            "SELECT EXISTS(
                 SELECT 1 FROM provider_templates WHERE id = ?1 AND version = ?2
             )",
            params![graph.plan.template_id.as_str(), graph.plan.template_version,],
            |row| row.get::<_, bool>(0),
        )
        .map_err(database_error)?;
    write_discovered_provider_graph_rows(
        transaction,
        &graph.template,
        &graph.connection,
        &graph.routes,
        &graph.observations,
        &graph.presets,
    )?;
    let stored_graph = load_discovered_provider_graph_rows(
        transaction,
        &graph.plan.template_id,
        graph.plan.template_version,
        &graph.plan.connection_id,
    )?
    .ok_or_else(|| corrupted("newly applied discovery graph is missing"))?;
    if stored_provider_graph_ownership_hash(&stored_graph)? != requested_ownership_hash {
        return Err(corrupted(
            "newly applied discovery graph does not match its requested rows",
        ));
    }
    append_audit(
        transaction,
        graph.plan.session_id.as_str(),
        expected_session_revision,
        "transition_applied",
        None,
        Some(&requested_ownership_hash),
        "discovery.audit.provider_graph_applied",
        applied_at,
    )?;
    append_audit(
        transaction,
        graph.plan.session_id.as_str(),
        expected_session_revision,
        "transition_applied",
        None,
        Some(if template_existed {
            "reused"
        } else {
            "created"
        }),
        "discovery.audit.provider_template_ownership",
        applied_at,
    )?;
    let changed = transaction
        .execute(
            "UPDATE provider_discovery_commit_attempts
             SET phase = 'database_applied', updated_at = ?2
             WHERE id = ?1 AND phase = 'prepared'",
            params![graph.plan.attempt_id.as_str(), applied_at.to_rfc3339()],
        )
        .map_err(database_error)?;
    if changed != 1 {
        return Err(CoreError::invalid(
            "discovery commit phase changed concurrently",
        ));
    }
    Ok(())
}

fn validate_catalog_authority_in_transaction(
    transaction: &Transaction<'_>,
    graph: &DiscoveredProviderGraph,
    authority_observed_at: DateTime<Utc>,
) -> CoreResult<()> {
    match (&graph.template.source, &graph.plan.catalog_authority) {
        (TemplateSource::SignedCatalog, Some(authority)) => {
            authority
                .validate_template(&graph.template)
                .map_err(contract_error)?;
            if authority_observed_at >= authority.expires_at {
                return Err(CoreError::new(
                    CoreErrorCode::InvalidInput,
                    "signed catalog authority expired before provider graph publication",
                    true,
                ));
            }
            let stored_state_version = transaction
                .query_row(
                    "SELECT state_version FROM provider_catalog_state WHERE singleton = 1",
                    [],
                    |row| row.get::<_, i64>(0),
                )
                .map_err(database_error)?;
            let current_state_version = u64::try_from(stored_state_version)
                .map_err(|_| corrupted("provider catalog state version is negative"))?;
            if current_state_version != authority.catalog_state_version {
                return Err(CoreError::new(
                    CoreErrorCode::InvalidInput,
                    "signed catalog authority changed before provider graph publication",
                    true,
                ));
            }
            Ok(())
        }
        (TemplateSource::SignedCatalog, None) => Err(CoreError::new(
            CoreErrorCode::InvalidInput,
            "legacy signed discovery plan has no catalog authority; restart provider discovery",
            true,
        )),
        (_, Some(_)) => Err(CoreError::invalid(
            "non-catalog provider graph cannot carry signed catalog authority",
        )),
        (_, None) => Ok(()),
    }
}

fn validate_graph_evidence_references(
    transaction: &Connection,
    graph: &DiscoveredProviderGraph,
) -> CoreResult<()> {
    for evidence_id in graph
        .observations
        .iter()
        .filter_map(|observation| observation.evidence_ref.as_ref())
    {
        let belongs = transaction
            .query_row(
                "SELECT EXISTS(
                     SELECT 1
                     FROM provider_discovery_evidence
                     WHERE id = ?1 AND session_id = ?2
                 )",
                params![evidence_id.as_str(), graph.plan.session_id.as_str()],
                |row| row.get::<_, bool>(0),
            )
            .map_err(database_error)?;
        if !belongs {
            return Err(CoreError::invalid(
                "capability observation evidence must belong to the committing discovery session",
            ));
        }
    }
    Ok(())
}

fn provider_graph_ownership_hash(
    template: &ProviderTemplate,
    connection: &ProviderConnection,
    routes: &[ModelRoute],
    observations: &[CapabilityObservation],
    presets: &[GenerationPreset],
) -> CoreResult<String> {
    let mut routes = routes.to_vec();
    routes.sort_by(|left, right| left.id.as_str().cmp(right.id.as_str()));
    let mut observations = observations.to_vec();
    observations.sort_by(|left, right| left.id.as_str().cmp(right.id.as_str()));
    let mut presets = presets.to_vec();
    presets.sort_by(|left, right| left.id.as_str().cmp(right.id.as_str()));
    let canonical = canonical_typed_json_result(
        serde_json::to_value((template, connection, routes, observations, presets)),
        "discovered provider graph ownership",
    )?;
    Ok(sha256_hex(canonical.as_bytes()))
}

fn stored_provider_graph_ownership_hash(
    graph: &StoredDiscoveredProviderGraphRows,
) -> CoreResult<String> {
    provider_graph_ownership_hash(
        &graph.template,
        &graph.connection,
        &graph.routes,
        &graph.observations,
        &graph.presets,
    )
}

fn graph_ownership_audit_hash(
    transaction: &Connection,
    session_id: &DiscoverySessionId,
) -> CoreResult<String> {
    let hashes = {
        let mut statement = transaction
            .prepare(
                "SELECT subject_id
                 FROM provider_discovery_audit_log
                 WHERE session_id = ?1
                   AND summary_key = 'discovery.audit.provider_graph_applied'
                 ORDER BY audit_sequence",
            )
            .map_err(database_error)?;
        statement
            .query_map([session_id.as_str()], |row| row.get::<_, Option<String>>(0))
            .map_err(database_error)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(database_error)?
    };
    if hashes.len() != 1 {
        return Err(corrupted(
            "discovery commit must have exactly one provider graph ownership record",
        ));
    }
    let hash = hashes
        .into_iter()
        .next()
        .flatten()
        .ok_or_else(|| corrupted("provider graph ownership record has no digest"))?;
    validate_sha256("provider graph ownership digest", &hash)
        .map_err(|_| corrupted("provider graph ownership digest is invalid"))?;
    Ok(hash)
}

fn graph_template_was_created(
    transaction: &Connection,
    session_id: &DiscoverySessionId,
) -> CoreResult<bool> {
    let records = {
        let mut statement = transaction
            .prepare(
                "SELECT subject_id
                 FROM provider_discovery_audit_log
                 WHERE session_id = ?1
                   AND summary_key = 'discovery.audit.provider_template_ownership'
                 ORDER BY audit_sequence",
            )
            .map_err(database_error)?;
        statement
            .query_map([session_id.as_str()], |row| row.get::<_, Option<String>>(0))
            .map_err(database_error)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(database_error)?
    };
    match records.as_slice() {
        [Some(value)] if value == "created" => Ok(true),
        [Some(value)] if value == "reused" => Ok(false),
        _ => Err(corrupted(
            "discovery commit has an invalid provider template ownership record",
        )),
    }
}

#[allow(clippy::too_many_lines)]
fn validate_provider_graph(graph: &DiscoveredProviderGraph) -> CoreResult<()> {
    graph.plan.validate().map_err(contract_error)?;
    validate_sha256("provider graph plan hash", &graph.plan_sha256)?;
    if let Some(reference) = &graph.plan.credential_ref {
        validate_opaque_credential_reference(reference.as_str())?;
    }
    validate_graph_component(serde_json::to_value(&graph.template), "provider template")?;
    validate_graph_component(
        serde_json::to_value(&graph.connection),
        "provider connection",
    )?;
    for route in &graph.routes {
        validate_graph_component(serde_json::to_value(route), "model route")?;
    }
    for preset in &graph.presets {
        validate_graph_component(serde_json::to_value(preset), "generation preset")?;
    }
    validate_persistable_discovery_url(
        graph.connection.api_origin.as_str(),
        "provider connection origin",
    )?;
    if graph.template.id != graph.plan.template_id
        || graph.template.manifest_version != graph.plan.template_version
        || graph.connection.id != graph.plan.connection_id
        || graph.connection.template_id != graph.plan.template_id
        || graph.connection.template_version != graph.plan.template_version
        || graph.connection.credential_ref != graph.plan.credential_ref
    {
        return Err(CoreError::invalid(
            "provider graph identities do not match the discovery commit plan",
        ));
    }
    let manifest_json = canonical_json_result(
        serde_json::to_value(&graph.template.default_manifest),
        "provider manifest",
    )?;
    if sha256_hex(manifest_json.as_bytes()) != graph.plan.manifest_sha256 {
        return Err(CoreError::invalid(
            "provider graph manifest does not match the validated manifest hash",
        ));
    }
    for route in &graph.routes {
        validate_discovery_route_metadata(route)?;
    }
    for observation in graph
        .observations
        .iter()
        .filter(|observation| observation.source == ObservationSource::ProviderApi)
    {
        let route = graph
            .routes
            .iter()
            .find(|route| route.id == observation.model_route_id)
            .ok_or_else(|| {
                CoreError::invalid(
                    "provider API capability observation references a route outside the graph",
                )
            })?;
        if route.metadata_source != ModelMetadataSource::ProviderApi
            || route.metadata_observed_at != Some(observation.observed_at)
            || observation.confidence != Confidence::High
            || !matches!(
                observation.status,
                SupportStatus::Verified | SupportStatus::Unsupported
            )
            || observation.evidence_ref.is_some()
            || observation
                .expires_at
                .is_none_or(|expires_at| expires_at <= observation.observed_at)
        {
            return Err(CoreError::invalid(
                "provider API capability observation provenance differs from its route metadata",
            ));
        }
    }
    for entry in &graph.connection.config.values {
        if let ConnectionConfigValue::Text(value) = &entry.value
            && looks_like_secret(value)
        {
            return Err(CoreError::invalid(
                "discovered provider connection configuration contains credential-like material",
            ));
        }
    }
    for route in &graph.routes {
        for entry in &route.route_config.values {
            if let ConnectionConfigValue::Text(value) = &entry.value
                && looks_like_secret(value)
            {
                return Err(CoreError::invalid(
                    "discovered model route configuration contains credential-like material",
                ));
            }
        }
    }
    for observation in &graph.observations {
        let value = serde_json::to_value(&observation.value)
            .map_err(|_| CoreError::internal("cannot inspect discovered capability value"))?;
        validate_redacted_value(&value)?;
    }
    let planned = graph.plan.model_route_ids.iter().collect::<BTreeSet<_>>();
    let actual = graph
        .routes
        .iter()
        .map(|route| &route.id)
        .collect::<BTreeSet<_>>();
    if planned.len() != graph.plan.model_route_ids.len()
        || actual.len() != graph.routes.len()
        || planned != actual
        || graph
            .routes
            .iter()
            .any(|route| route.connection_id != graph.connection.id)
        || graph
            .observations
            .iter()
            .any(|observation| !actual.contains(&observation.model_route_id))
        || graph
            .presets
            .iter()
            .any(|preset| !actual.contains(&preset.model_route_id))
    {
        return Err(CoreError::invalid(
            "provider graph routes and dependants do not match the commit plan",
        ));
    }
    Ok(())
}

fn validate_discovery_route_metadata(route: &ModelRoute) -> CoreResult<()> {
    if route.last_reconciled_sync_job_id.is_some() || route.metadata_sync_job_id.is_some() {
        return Err(CoreError::invalid(
            "initial discovery routes cannot claim model synchronization provenance",
        ));
    }
    match (
        route.raw_metadata.as_ref(),
        route.metadata_source,
        route.metadata_observed_at,
    ) {
        (Some(metadata), ModelMetadataSource::ProviderApi, Some(observed_at)) => {
            if route.miss_count != 0
                || route.first_seen_at != observed_at
                || route.last_seen_at != Some(observed_at)
            {
                return Err(CoreError::invalid(
                    "discovered provider API route metadata has inconsistent observation times",
                ));
            }
            validate_provider_api_route_metadata(Some(metadata))
        }
        (None, ModelMetadataSource::Legacy | ModelMetadataSource::UserOverride, None) => {
            if route.miss_count != 0 {
                return Err(CoreError::invalid(
                    "initial discovery routes cannot carry model synchronization miss counts",
                ));
            }
            Ok(())
        }
        _ => Err(CoreError::invalid(
            "discovered route metadata must be absent or a normalized provider API projection",
        )),
    }
}

fn validate_graph_component(
    component: Result<Value, serde_json::Error>,
    label: &str,
) -> CoreResult<()> {
    let value = component.map_err(|_| CoreError::internal(format!("cannot inspect {label}")))?;
    validate_redacted_value(&value)
        .map_err(|_| CoreError::invalid(format!("{label} contains forbidden data")))
}

fn finalize_commit_failed_before_apply(
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

fn ensure_discovery_attempt_graph_absent(
    transaction: &Transaction<'_>,
    attempt: &DiscoveryCommitAttemptRecord,
) -> CoreResult<()> {
    if load_discovered_provider_graph_rows(
        transaction,
        &attempt.plan.template_id,
        attempt.plan.template_version,
        &attempt.plan.connection_id,
    )?
    .is_some()
    {
        return Err(CoreError::invalid(
            "commit graph must be absent before this ledger transition",
        ));
    }
    for route_id in &attempt.plan.model_route_ids {
        let exists = transaction
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM provider_models WHERE id = ?1)",
                [route_id.as_str()],
                |row| row.get::<_, bool>(0),
            )
            .map_err(database_error)?;
        if exists {
            return Err(corrupted(
                "commit graph is absent but a planned route remains",
            ));
        }
    }
    Ok(())
}

fn verify_discovery_attempt_graph(
    transaction: &Transaction<'_>,
    attempt: &DiscoveryCommitAttemptRecord,
) -> CoreResult<()> {
    let graph = load_discovered_provider_graph_rows(
        transaction,
        &attempt.plan.template_id,
        attempt.plan.template_version,
        &attempt.plan.connection_id,
    )?
    .ok_or_else(|| CoreError::invalid("confirmed commit graph is missing"))?;
    let ownership = stored_provider_graph_ownership_hash(&graph)?;
    if ownership != attempt.plan.graph_sha256
        || graph_ownership_audit_hash(transaction, &attempt.session_id)? != ownership
    {
        return Err(CoreError::invalid(
            "confirmed commit graph differs from its approved ownership digest",
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

fn complete_commit_attempt_for_ready_transition(
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

fn project_reconciled_discovery_credential_ownership(
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

fn insert_discovery_credential_ownership_event(
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

fn load_commit_attempt(
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
fn validate_exact_discovery_authority_audit(
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

type AbandonedNativeCredentialExecutionRow = (
    String,
    String,
    String,
    String,
    String,
    String,
    String,
    u32,
    u32,
    String,
    u32,
    u32,
);

fn load_schema37_abandoned_native_credential_reservation(
    connection: &Connection,
    operation: &DiscoveryOperationRecord,
) -> CoreResult<AbandonedNativeCredentialExecutionRow> {
    connection
        .query_row(
            "SELECT execution.physical_authority_id, execution.session_id,
                    execution.commit_attempt_id, execution.commit_plan_sha256,
                    execution.connection_id,
                    execution.connection_binding_sha256, execution.reserved_at,
                    execution.schema_version, execution.redaction_version,
                    abandonment.abandoned_at, abandonment.schema_version,
                    abandonment.redaction_version
             FROM provider_discovery_native_credential_executions AS execution
             JOIN provider_discovery_native_credential_abandoned_reservations AS abandonment
               ON abandonment.operation_id = execution.operation_id
              AND abandonment.physical_authority_id = execution.physical_authority_id
              AND abandonment.session_id = execution.session_id
              AND abandonment.commit_attempt_id = execution.commit_attempt_id
              AND abandonment.commit_plan_sha256 = execution.commit_plan_sha256
              AND abandonment.connection_id = execution.connection_id
              AND abandonment.connection_binding_sha256
                  = execution.connection_binding_sha256
              AND abandonment.reserved_at = execution.reserved_at
              AND abandonment.abandonment_kind
                  = 'prepared_interrupted_before_native_store'
             JOIN provider_discovery_commit_attempts AS attempt
               ON attempt.id = execution.commit_attempt_id
              AND attempt.session_id = execution.session_id
              AND attempt.plan_sha256 = execution.commit_plan_sha256
             JOIN provider_discovery_authorized_native_commit_starts AS authorized
               ON authorized.operation_id = execution.operation_id
              AND authorized.session_id = execution.session_id
              AND authorized.commit_attempt_id = execution.commit_attempt_id
              AND authorized.commit_plan_sha256 = execution.commit_plan_sha256
              AND authorized.operation_expected_revision = ?2
             WHERE execution.operation_id = ?1
               AND execution.session_id = ?3
               AND json_extract(attempt.plan_json, '$.connection_id')
                   = execution.connection_id
               AND json_extract(attempt.plan_json, '$.credential_ref')
                   = execution.connection_id
               AND NOT EXISTS (
                   SELECT 1
                   FROM provider_discovery_native_credential_store_attempts AS store_attempt
                   WHERE store_attempt.operation_id = execution.operation_id
                      OR store_attempt.physical_authority_id
                          = execution.physical_authority_id
               )
               AND NOT EXISTS (
                   SELECT 1
                   FROM provider_discovery_native_no_effect_attestations AS attestation
                   WHERE attestation.operation_id = execution.operation_id
               )",
            params![
                operation.id.as_str(),
                operation.expected_revision,
                operation.session_id.as_str(),
            ],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, String>(4)?,
                    row.get::<_, String>(5)?,
                    row.get::<_, String>(6)?,
                    row.get::<_, u32>(7)?,
                    row.get::<_, u32>(8)?,
                    row.get::<_, String>(9)?,
                    row.get::<_, u32>(10)?,
                    row.get::<_, u32>(11)?,
                ))
            },
        )
        .optional()
        .map_err(database_error)?
        .ok_or_else(|| {
            corrupted("schema-37 native credential reservation has no exact abandonment")
        })
}

fn validate_schema37_abandoned_native_credential_reservation(
    connection: &Connection,
    operation: &DiscoveryOperationRecord,
) -> CoreResult<bool> {
    let execution_exists = connection
        .query_row(
            "SELECT EXISTS(
                 SELECT 1
                 FROM provider_discovery_native_credential_executions
                 WHERE operation_id = ?1
             )",
            [operation.id.as_str()],
            |row| row.get::<_, bool>(0),
        )
        .map_err(database_error)?;
    if !execution_exists {
        return Ok(false);
    }
    let exact = load_schema37_abandoned_native_credential_reservation(connection, operation)?;
    validate_discovery_native_physical_authority_id(&exact.0)?;
    validate_sha256("abandoned native credential plan hash", &exact.3)
        .map_err(|_| corrupted("abandoned native credential plan hash is invalid"))?;
    validate_sha256("abandoned native credential connection binding", &exact.5)
        .map_err(|_| corrupted("abandoned native credential connection binding is invalid"))?;
    let attempt_id = DiscoveryCommitAttemptId::parse(exact.2)
        .map_err(|_| corrupted("abandoned native credential attempt id is invalid"))?;
    let attempt = load_commit_attempt(connection, &attempt_id)?;
    let reserved_at = parse_timestamp(&exact.6, "abandoned native credential reserved_at")?;
    let abandoned_at = parse_timestamp(&exact.9, "abandoned native credential abandoned_at")?;
    if operation.status != DiscoveryOperationStatus::Interrupted
        || exact.1 != operation.session_id.as_str()
        || operation.started_at != Some(abandoned_at)
        || operation.finished_at != Some(abandoned_at)
        || operation.updated_at != abandoned_at
        || operation.created_at > abandoned_at
        || reserved_at > abandoned_at
        || exact.7 != 1
        || exact.8 != 1
        || exact.10 != 1
        || exact.11 != 1
        || attempt.session_id != operation.session_id
        || attempt.plan_sha256 != exact.3
        || attempt.plan.connection_id.as_str() != exact.4
        || attempt
            .plan
            .credential_ref
            .as_ref()
            .map(CredentialRef::as_str)
            != Some(exact.4.as_str())
    {
        return Err(corrupted(
            "schema-37 native credential abandonment is detached from its operation",
        ));
    }
    Ok(true)
}

fn validate_discovery_operation_start_audit(
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

fn validate_discovery_operation_terminal_audit_order(
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

fn validate_discovery_operation_terminal_audit_order_for_receipt(
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

fn validate_discovery_operation_interrupted_audit(
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

fn validate_atomic_commit_start_receipt(
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

fn load_discovery_authority_operation_for_start(
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

type DiscoveryAuthorityGraphAuditRow = (
    u64,
    String,
    Option<String>,
    Option<String>,
    u64,
    String,
    String,
);

fn validate_discovery_authority_graph_audits(
    connection: &Connection,
    attempt: &DiscoveryCommitAttemptRecord,
    authority_revision_bound: u64,
    authority_time_bound: DateTime<Utc>,
    applied_with_bound: bool,
    operation_start_audit_sequence: u64,
    terminal_audit_sequence: u64,
) -> CoreResult<()> {
    let rows = {
        let mut statement = connection
            .prepare(
                "SELECT audit_sequence, audit_kind, action_id, subject_id, session_revision,
                        summary_key, created_at
                 FROM provider_discovery_audit_log
                 WHERE session_id = ?1
                   AND summary_key IN (
                       'discovery.audit.provider_graph_applied',
                       'discovery.audit.provider_template_ownership'
                   )
                 ORDER BY summary_key",
            )
            .map_err(database_error)?;
        statement
            .query_map([attempt.session_id.as_str()], |row| {
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                    row.get(5)?,
                    row.get(6)?,
                ))
            })
            .map_err(database_error)?
            .collect::<Result<Vec<DiscoveryAuthorityGraphAuditRow>, _>>()
            .map_err(database_error)?
    };
    if rows.len() != 2 {
        return Err(corrupted(
            "discovery credential graph ownership audits are incomplete",
        ));
    }
    let graph = rows
        .iter()
        .find(|row| row.5 == "discovery.audit.provider_graph_applied")
        .ok_or_else(|| corrupted("discovery credential graph ownership audit is missing"))?;
    let template = rows
        .iter()
        .find(|row| row.5 == "discovery.audit.provider_template_ownership")
        .ok_or_else(|| corrupted("discovery credential template ownership audit is missing"))?;
    let graph_at = parse_timestamp(&graph.6, "provider graph authority audit created_at")?;
    let template_at = parse_timestamp(&template.6, "provider template authority audit created_at")?;
    let bounded = graph.4 == authority_revision_bound
        && graph_at <= authority_time_bound
        && (!applied_with_bound || graph_at == authority_time_bound);
    if graph.1 != "transition_applied"
        || template.1 != "transition_applied"
        || graph.2.is_some()
        || template.2.is_some()
        || graph.3.as_deref() != Some(attempt.plan.graph_sha256.as_str())
        || !matches!(template.3.as_deref(), Some("created" | "reused"))
        || graph.4 != template.4
        || graph_at != template_at
        || operation_start_audit_sequence >= graph.0
        || graph.0 >= template.0
        || template.0 >= terminal_audit_sequence
        || !bounded
    {
        return Err(corrupted(
            "discovery credential graph ownership audits are detached from the terminal history",
        ));
    }
    Ok(())
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

fn validate_unknown_discovery_credential_receipt(
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

fn validate_interrupted_discovery_authority_receipt(
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

fn validate_cancelled_pre_store_interruption_receipt(
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

fn validate_interrupted_discovery_operation_evidence(
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

fn load_restart_discovery_authority_receipt(
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

fn validate_discovery_receipt_follows(
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

struct DiscoveryAuthorityReceiptRecord {
    receipt: DiscoveryActionReceipt,
    transition: DiscoveryTransition,
    created_at: DateTime<Utc>,
    transition_audit_sequence: u64,
    commit_prepared_audit_sequence: Option<u64>,
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

fn load_discovery_authority_receipt_by_action(
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

fn load_discovery_authority_receipt_by_revision(
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

fn validate_ready_discovery_authority_receipt(
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

fn validate_commit_phase_preconditions(
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

mod approval_store;
pub(crate) mod contract_codec;
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
use contract_codec::{
    append_audit, candidate_kind, canonical_json_result, canonical_typed_json_result,
    decode_redacted_json, encode_approval_grant, encode_commit_plan_json, encode_json_result,
    encode_redacted_json, enum_wire_result, parse_approval_decision, parse_discovery_state,
    parse_operation_kind, parse_side_effect_class, parse_timestamp, sha256_hex,
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
