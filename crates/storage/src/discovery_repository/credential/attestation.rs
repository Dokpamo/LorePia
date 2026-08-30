//! Native no-effect attestations and abandoned reservation validation.

use super::{
    NATIVE_NO_EFFECT_ATTESTATION_REDACTION_VERSION, NATIVE_NO_EFFECT_ATTESTATION_SCHEMA_VERSION,
    native_no_effect_binding_sha256, native_no_effect_evidence_sha256_from_record,
    native_no_effect_execution_binding_sha256_from_record,
    validate_native_no_effect_operation_start_receipt,
};
use crate::discovery_repository::{
    Connection, CoreError, CoreErrorCode, CoreResult, CredentialRef, DateTime,
    DiscoveryCommitAttemptId, DiscoveryCommitPhase, DiscoveryNativeCredentialExecutionRecord,
    DiscoveryNativeNoEffectAttestationKind, DiscoveryNativeNoEffectAttestationRecord,
    DiscoveryNativeNoEffectAttestationWrite, DiscoveryNativeRecoveryOwner, DiscoveryOperationId,
    DiscoveryOperationKind, DiscoveryOperationRecord, DiscoveryOperationStatus, DiscoverySessionId,
    DiscoverySideEffectClass, OptionalExtension, ProviderConnectionId, Transaction, Utc, corrupted,
    database_error, load_commit_attempt, load_discovery_native_credential_execution,
    load_operation_by_id, params, parse_timestamp, validate_discovery_native_physical_authority_id,
    validate_sha256,
};

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

pub(in crate::discovery_repository) fn validate_native_no_effect_database_binding(
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

pub(in crate::discovery_repository) fn load_native_no_effect_attestation(
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

pub(in crate::discovery_repository) fn validate_schema37_abandoned_native_credential_reservation(
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
