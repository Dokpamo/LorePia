//! Durable native credential execution boundaries for discovery commits.

#[path = "credential/attestation.rs"]
mod attestation;
#[path = "credential/authority.rs"]
mod authority;
#[path = "credential/history.rs"]
mod history;

use super::{
    Connection, CoreError, CoreResult, CredentialRef, DateTime, DiscoveryCommitAttemptId,
    DiscoveryCommitPhase, DiscoveryNativeCredentialExecutionRecord,
    DiscoveryNativeCredentialExecutionReservation, DiscoveryNativeCredentialStoreAttemptStart,
    DiscoveryNativeNoEffectAttestationKind, DiscoveryNativeNoEffectAttestationRecord,
    DiscoveryNativeNoEffectAttestationWrite, DiscoveryNativeRecoveryOwner, DiscoveryOperationId,
    DiscoveryOperationKind, DiscoveryOperationStatus, DiscoverySessionId, DiscoverySideEffectClass,
    DiscoveryState, DiscoveryTransitionWrite, DurableOperationOutcome, PersistDiscoveryTransition,
    Serialize, Storage, Transaction, TransactionBehavior, Utc, Uuid, append_audit,
    canonical_json_result, corrupted, database_error, load_commit_attempt,
    load_discovery_native_credential_execution, load_operation_by_id, load_session_snapshot,
    params, persist_transition_in_transaction, sha256_hex,
    validate_active_discovery_credential_cancellation_chain,
    validate_discovery_native_physical_authority_id, validate_sha256, validate_transition_write,
};
pub(super) use attestation::{
    load_native_no_effect_attestation, validate_native_no_effect_database_binding,
};
pub(super) use authority::{
    insert_discovery_credential_ownership_event, project_reconciled_discovery_credential_ownership,
    validate_exact_discovery_authority_audit,
};
pub(crate) use authority::{
    validate_archived_discovery_credential_ownership_authority_for_slot_gc,
    validate_discovery_credential_ownership_authority,
};
pub(super) use history::{
    DiscoveryAuthorityReceiptRecord, load_discovery_authority_receipt_by_action,
    load_discovery_authority_receipt_by_revision,
    validate_cancelled_pre_store_interruption_receipt,
    validate_discovery_operation_interrupted_audit, validate_discovery_operation_start_audit,
    validate_discovery_operation_terminal_audit_order_for_receipt,
    validate_discovery_receipt_follows, validate_interrupted_discovery_authority_receipt,
    validate_interrupted_discovery_operation_evidence,
    validate_native_no_effect_operation_start_receipt,
};

pub(super) const DISCOVERY_REDACTION_VERSION: u32 = 1;

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

pub(super) fn native_no_effect_evidence_sha256(
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

pub(super) fn native_no_effect_execution_binding_sha256(
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
