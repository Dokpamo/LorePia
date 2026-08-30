//! Typed row hydration and low-level repository query helpers.

use super::queries::load_operation_by_id;
use super::{
    Connection, CoreErrorCode, CoreResult, CredentialRef, DiscoveryApprovalBinding,
    DiscoveryApprovalGrant, DiscoveryCommitAttemptId, DiscoveryCommitAttemptRecord,
    DiscoveryCompensationStatus, DiscoveryNativeCredentialExecutionRecord, DiscoveryOperationId,
    DiscoveryOperationKind, DiscoveryOperationRecord, DiscoveryOperationStatus,
    DiscoveryRecoveryCheckpoint, DiscoveryReviewDiff, DiscoverySessionId, DiscoverySessionSnapshot,
    DiscoverySideEffectClass, DiscoveryState, OptionalExtension, ProviderDiscoverySession,
    SanitizedDiscoveryInput, Uuid, contract_error, corrupted, database_error, decode_redacted_json,
    load_commit_attempt, params, parse_discovery_state, parse_operation_kind, parse_timestamp,
    sha256_hex, validate_capability_probe_grant, validate_identifier,
    validate_legacy_unbound_started_credential_execution,
    validate_pre_store_native_credential_interruption, validate_review_evidence_references,
    validate_sanitized_input, validate_session_evidence_ids,
};

pub(super) const fn compensation_status_transition_allowed(
    expected: DiscoveryCompensationStatus,
    next: DiscoveryCompensationStatus,
) -> bool {
    matches!(
        (expected, next),
        (
            DiscoveryCompensationStatus::Pending,
            DiscoveryCompensationStatus::InProgress
        ) | (
            DiscoveryCompensationStatus::InProgress,
            DiscoveryCompensationStatus::Completed
                | DiscoveryCompensationStatus::Failed
                | DiscoveryCompensationStatus::OutcomeUnknown
        )
    )
}

pub(super) fn ensure_foreign_keys_clean(connection: &Connection) -> CoreResult<()> {
    let violation = {
        let mut statement = connection
            .prepare("PRAGMA foreign_key_check")
            .map_err(database_error)?;
        statement
            .query_row([], |_| Ok(()))
            .optional()
            .map_err(database_error)?
            .is_some()
    };
    if violation {
        Err(corrupted(
            "provider graph compensation created a foreign-key violation",
        ))
    } else {
        Ok(())
    }
}

pub(super) type SessionRow = (
    String,
    String,
    u64,
    u64,
    String,
    Option<String>,
    Option<String>,
    Option<String>,
    Option<String>,
    Option<String>,
    Option<String>,
    Option<String>,
    Option<String>,
    Option<String>,
    bool,
    Option<String>,
    Option<String>,
    String,
    String,
);

#[allow(clippy::too_many_lines)]
pub(super) fn decode_session_row(
    connection: &Connection,
    row: SessionRow,
) -> CoreResult<DiscoverySessionSnapshot> {
    let input = serde_json::from_str::<SanitizedDiscoveryInput>(&row.4)
        .map_err(|_| corrupted("stored discovery input is invalid"))?;
    input
        .validate()
        .map_err(|_| corrupted("stored discovery input violates its contract"))?;
    validate_sanitized_input(&input)
        .map_err(|_| corrupted("stored discovery input contains credential-like material"))?;
    let state = parse_discovery_state(&row.1)?;
    let recovery = row
        .8
        .as_deref()
        .map(|json| {
            serde_json::from_str::<DiscoveryRecoveryCheckpoint>(json)
                .map_err(|_| corrupted("stored discovery recovery checkpoint is invalid"))
        })
        .transpose()?;
    let unknown_operation = row.9.as_deref().map(parse_operation_kind).transpose()?;
    let failure = row
        .7
        .as_deref()
        .map(|json| {
            serde_json::from_str(json).map_err(|_| corrupted("stored discovery failure is invalid"))
        })
        .transpose()?;
    let active_operation_id = row
        .15
        .map(DiscoveryOperationId::parse)
        .transpose()
        .map_err(contract_error)?;
    let active_effect_approval = row
        .16
        .as_deref()
        .map(|json| {
            let binding = serde_json::from_str::<DiscoveryApprovalBinding>(json)
                .map_err(|_| corrupted("stored active discovery approval is invalid"))?;
            binding
                .validate()
                .map_err(|_| corrupted("stored active discovery approval is invalid"))?;
            if serde_json::to_string(&binding)
                .map_err(|_| corrupted("stored active discovery approval cannot be encoded"))?
                != json
            {
                return Err(corrupted(
                    "stored active discovery approval is not canonical",
                ));
            }
            Ok(binding)
        })
        .transpose()?;
    if let Some(operation_id) = &active_operation_id {
        let operation = load_operation_by_id(connection, operation_id)?;
        if operation.session_id.as_str() != row.0
            || state.operation() != Some(operation.kind)
            || operation.approval != active_effect_approval
            || !matches!(
                operation.status,
                DiscoveryOperationStatus::Prepared | DiscoveryOperationStatus::Started
            )
        {
            return Err(corrupted(
                "active discovery operation does not match the session binding",
            ));
        }
        if let Some(binding) = &operation.approval {
            validate_recovery_approval_binding(connection, &row.0, binding, operation.kind)?;
        }
    } else if let Some(binding) = &active_effect_approval {
        let recoverable_operation = match state {
            DiscoveryState::Interrupted => recovery.as_ref().map(|checkpoint| checkpoint.operation),
            DiscoveryState::UnknownOutcome => unknown_operation,
            _ => None,
        };
        let Some(operation) = recoverable_operation.filter(|operation| {
            matches!(
                operation,
                DiscoveryOperationKind::BuildAssistantManifestDraft
                    | DiscoveryOperationKind::ProbeCapabilities
            )
        }) else {
            return Err(corrupted(
                "active discovery approval exists without recoverable billable work",
            ));
        };
        validate_recovery_approval_binding(connection, &row.0, binding, operation)?;
    }
    let session = ProviderDiscoverySession {
        id: DiscoverySessionId::from(row.0),
        input,
        state,
        revision: row.2,
        next_event_sequence: row.3,
        recovery,
        unknown_operation,
        manifest_sha256: row.10,
        commit_plan_sha256: row.11,
        commit_attempt_id: row
            .12
            .map(DiscoveryCommitAttemptId::parse)
            .transpose()
            .map_err(contract_error)?,
        committed_connection_id: row.13.map(Into::into),
        cancellation_pending: row.14,
        active_effect_approval,
        failure,
    };
    session
        .validate()
        .map_err(|_| corrupted("stored discovery session violates its domain contract"))?;
    if let Some(attempt_id) = &session.commit_attempt_id {
        let attempt = load_commit_attempt(connection, attempt_id).map_err(|error| {
            if error.code == CoreErrorCode::NotFound {
                corrupted("stored discovery session references a missing commit attempt")
            } else {
                error
            }
        })?;
        if attempt.session_id != session.id
            || session.commit_plan_sha256.as_deref() != Some(attempt.plan_sha256.as_str())
        {
            return Err(corrupted(
                "stored discovery session commit binding does not match its attempt",
            ));
        }
    }
    let draft_json = row
        .5
        .as_deref()
        .map(|json| decode_redacted_json(json, "stored discovery draft"))
        .transpose()?;
    let review = row
        .6
        .as_deref()
        .map(|json| {
            let review = serde_json::from_str::<DiscoveryReviewDiff>(json)
                .map_err(|_| corrupted("stored discovery review is invalid"))?;
            review
                .validate()
                .map_err(|_| corrupted("stored discovery review violates its contract"))?;
            Ok(review)
        })
        .transpose()?;
    if let Some(review) = &review {
        validate_review_evidence_references(connection, &session.id, review)
            .map_err(|_| corrupted("stored discovery review has invalid evidence references"))?;
    }
    Ok(DiscoverySessionSnapshot {
        session,
        active_operation_id,
        draft_json,
        review,
        created_at: parse_timestamp(&row.17, "discovery created_at")?,
        updated_at: parse_timestamp(&row.18, "discovery updated_at")?,
    })
}

#[allow(clippy::too_many_lines)]
pub(super) fn validate_recovery_approval_binding(
    connection: &Connection,
    session_id: &str,
    binding: &DiscoveryApprovalBinding,
    operation: DiscoveryOperationKind,
) -> CoreResult<()> {
    let row = connection
        .query_row(
            "SELECT approval_kind, decision, grant_json, grant_sha256
             FROM provider_discovery_approvals
             WHERE id = ?1 AND session_id = ?2",
            params![binding.approval_id.as_str(), session_id],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                ))
            },
        )
        .optional()
        .map_err(database_error)?
        .ok_or_else(|| corrupted("recoverable billable approval record is missing"))?;
    if row.1 != "approved"
        || row.3 != binding.grant_sha256
        || sha256_hex(row.2.as_bytes()) != binding.grant_sha256
    {
        return Err(corrupted(
            "recoverable billable approval binding does not match its immutable grant",
        ));
    }
    let grant = serde_json::from_str::<DiscoveryApprovalGrant>(&row.2)
        .map_err(|_| corrupted("recoverable billable approval grant is invalid"))?;
    grant
        .validate()
        .map_err(|_| corrupted("recoverable billable approval grant is invalid"))?;
    if serde_json::to_string(&grant)
        .map_err(|_| corrupted("recoverable billable approval grant cannot be encoded"))?
        != row.2
    {
        return Err(corrupted(
            "recoverable billable approval grant is not canonical",
        ));
    }
    match &grant {
        DiscoveryApprovalGrant::AssistantConsent {
            assistant_route_id,
            evidence_ids,
            ..
        } => {
            let typed_session_id = DiscoverySessionId::from(session_id);
            validate_session_evidence_ids(
                connection,
                &typed_session_id,
                evidence_ids,
                "recoverable assistant consent",
            )
            .map_err(|_| {
                corrupted("recoverable assistant approval has invalid evidence references")
            })?;
            let route_exists = connection
                .query_row(
                    "SELECT EXISTS(SELECT 1 FROM provider_models WHERE id = ?1)",
                    [assistant_route_id.as_str()],
                    |row| row.get::<_, bool>(0),
                )
                .map_err(database_error)?;
            if !route_exists {
                return Err(corrupted("recoverable assistant approval route is missing"));
            }
        }
        DiscoveryApprovalGrant::CapabilityProbe {
            model_route_ids,
            budget,
        } => {
            validate_capability_probe_grant(
                connection,
                &DiscoverySessionId::from(session_id),
                model_route_ids,
                *budget,
            )
            .map_err(|_| {
                corrupted("recoverable capability approval differs from its durable proposal")
            })?;
        }
        _ => {}
    }
    let expected_kind = match operation {
        DiscoveryOperationKind::BuildAssistantManifestDraft => "assistant_consent",
        DiscoveryOperationKind::ProbeCapabilities => "capability_probe",
        _ => {
            return Err(corrupted(
                "non-billable operation carried a recovery approval",
            ));
        }
    };
    let grant_matches = matches!(
        (operation, &grant),
        (
            DiscoveryOperationKind::BuildAssistantManifestDraft,
            DiscoveryApprovalGrant::AssistantConsent { .. }
        ) | (
            DiscoveryOperationKind::ProbeCapabilities,
            DiscoveryApprovalGrant::CapabilityProbe { .. }
        )
    );
    if row.0 != expected_kind || !grant_matches {
        return Err(corrupted(
            "recoverable billable approval has the wrong grant type",
        ));
    }
    Ok(())
}

pub(super) fn validate_missing_native_credential_execution(
    connection: &Connection,
    operation: &DiscoveryOperationRecord,
) -> CoreResult<()> {
    let authorized_attempts = {
        let mut statement = connection
            .prepare(
                "SELECT commit_attempt_id
                 FROM provider_discovery_authorized_native_commit_starts
                 WHERE operation_id = ?1
                 ORDER BY commit_attempt_id",
            )
            .map_err(database_error)?;
        statement
            .query_map([operation.id.as_str()], |row| row.get::<_, String>(0))
            .map_err(database_error)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(database_error)?
    };
    let [] = authorized_attempts.as_slice() else {
        let [attempt_id] = authorized_attempts.as_slice() else {
            return Err(corrupted(
                "native credential operation has ambiguous approved start authority",
            ));
        };
        let attempt_id = DiscoveryCommitAttemptId::parse(attempt_id.clone())
            .map_err(|_| corrupted("native credential operation approved attempt id is invalid"))?;
        let attempt = load_commit_attempt(connection, &attempt_id)?;
        return match operation.status {
            DiscoveryOperationStatus::Prepared => Ok(()),
            DiscoveryOperationStatus::Interrupted => {
                validate_pre_store_native_credential_interruption(connection, operation, &attempt)
            }
            DiscoveryOperationStatus::Started | DiscoveryOperationStatus::OutcomeUnknown
                if validate_legacy_unbound_started_credential_execution(
                    connection, operation, &attempt,
                )? =>
            {
                Ok(())
            }
            _ => Err(corrupted(
                "started discovery credential operation has no immutable native execution",
            )),
        };
    };
    Ok(())
}

pub(super) type NativeCredentialAbandonmentRow = (
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

pub(super) fn validate_native_credential_abandonment(
    connection: &Connection,
    operation: &DiscoveryOperationRecord,
    execution: &DiscoveryNativeCredentialExecutionRecord,
    reserved_at_raw: &str,
) -> CoreResult<bool> {
    let abandonment: Option<NativeCredentialAbandonmentRow> = connection
        .query_row(
            "SELECT physical_authority_id, session_id, commit_attempt_id,
                    commit_plan_sha256, connection_id,
                    connection_binding_sha256, reserved_at,
                    abandonment_kind, abandoned_at,
                    schema_version, redaction_version
             FROM provider_discovery_native_credential_abandoned_reservations
             WHERE operation_id = ?1",
            [operation.id.as_str()],
            |query_row| {
                Ok((
                    query_row.get::<_, String>(0)?,
                    query_row.get::<_, String>(1)?,
                    query_row.get::<_, String>(2)?,
                    query_row.get::<_, String>(3)?,
                    query_row.get::<_, String>(4)?,
                    query_row.get::<_, String>(5)?,
                    query_row.get::<_, String>(6)?,
                    query_row.get::<_, String>(7)?,
                    query_row.get::<_, String>(8)?,
                    query_row.get::<_, u32>(9)?,
                    query_row.get::<_, u32>(10)?,
                ))
            },
        )
        .optional()
        .map_err(database_error)?;
    let Some(abandonment) = abandonment else {
        return Ok(false);
    };
    let abandoned_at =
        parse_timestamp(&abandonment.8, "native credential reservation abandoned_at")?;
    let valid = abandonment.0 == execution.physical_authority_id
        && abandonment.1 == execution.session_id.as_str()
        && abandonment.2 == execution.commit_attempt_id.as_str()
        && abandonment.3 == execution.commit_plan_sha256
        && abandonment.4 == execution.connection_id.as_str()
        && abandonment.5 == execution.connection_binding_sha256
        && abandonment.6 == reserved_at_raw
        && abandonment.7 == "prepared_interrupted_before_native_store"
        && operation.finished_at == Some(abandoned_at)
        && abandonment.9 == 1
        && abandonment.10 == 1;
    if !valid {
        return Err(corrupted(
            "stored native credential abandonment differs from its reservation",
        ));
    }
    Ok(true)
}

pub(super) fn validate_native_credential_execution_commit_binding(
    connection: &Connection,
    operation: &DiscoveryOperationRecord,
    attempt: &DiscoveryCommitAttemptRecord,
    execution: &DiscoveryNativeCredentialExecutionRecord,
    valid_abandonment: bool,
) -> CoreResult<()> {
    let authorized = connection
        .query_row(
            "SELECT COUNT(*)
             FROM provider_discovery_authorized_native_commit_starts
             WHERE operation_id = ?1
               AND session_id = ?2
               AND commit_attempt_id = ?3
               AND commit_plan_sha256 = ?4
               AND operation_expected_revision = ?5",
            params![
                operation.id.as_str(),
                execution.session_id.as_str(),
                execution.commit_attempt_id.as_str(),
                execution.commit_plan_sha256,
                operation.expected_revision,
            ],
            |row| row.get::<_, u64>(0),
        )
        .map_err(database_error)?;
    let operation_execution_state_valid = match operation.status {
        DiscoveryOperationStatus::Prepared => {
            operation.started_at.is_none()
                && operation.finished_at.is_none()
                && execution.store_started_at.is_none()
                && !valid_abandonment
        }
        DiscoveryOperationStatus::Started => {
            execution.store_started_at.is_some()
                && operation.started_at == execution.store_started_at
                && operation.finished_at.is_none()
                && !valid_abandonment
        }
        DiscoveryOperationStatus::Interrupted if execution.store_started_at.is_none() => {
            if valid_abandonment {
                validate_pre_store_native_credential_interruption(connection, operation, attempt)?;
                true
            } else {
                false
            }
        }
        DiscoveryOperationStatus::Succeeded
        | DiscoveryOperationStatus::Failed
        | DiscoveryOperationStatus::Interrupted
        | DiscoveryOperationStatus::OutcomeUnknown => {
            execution.store_started_at.is_some()
                && operation.started_at == execution.store_started_at
                && operation.finished_at.is_some()
                && !valid_abandonment
        }
    };
    if execution.operation_id != operation.id
        || operation.session_id != execution.session_id
        || operation.kind != DiscoveryOperationKind::AtomicCommit
        || operation.side_effect_class != DiscoverySideEffectClass::Persistent
        || !operation_execution_state_valid
        || execution
            .store_started_at
            .is_some_and(|started| started < execution.reserved_at)
        || operation.finished_at.is_some_and(|finished| {
            execution
                .store_started_at
                .map_or(finished < execution.reserved_at, |started| {
                    finished < started
                })
        })
        || attempt.id != execution.commit_attempt_id
        || attempt.session_id != execution.session_id
        || attempt.plan_sha256 != execution.commit_plan_sha256
        || attempt.plan.attempt_id != execution.commit_attempt_id
        || attempt.plan.connection_id != execution.connection_id
        || attempt
            .plan
            .credential_ref
            .as_ref()
            .map(CredentialRef::as_str)
            != Some(execution.connection_id.as_str())
        || authorized != 1
    {
        return Err(corrupted(
            "stored native credential execution is detached from its immutable discovery commit",
        ));
    }
    Ok(())
}

pub(super) fn validate_discovery_native_physical_authority_id(
    authority_id: &str,
) -> CoreResult<()> {
    const PREFIX: &str = "discovery-native-";
    validate_identifier(
        "discovery native credential physical authority",
        authority_id,
        256,
    )
    .map_err(|_| corrupted("stored native credential physical authority id is invalid"))?;
    let suffix = authority_id
        .strip_prefix(PREFIX)
        .ok_or_else(|| corrupted("stored native credential physical authority id is invalid"))?;
    let parsed = Uuid::parse_str(suffix)
        .map_err(|_| corrupted("stored native credential physical authority id is invalid"))?;
    if parsed.get_version() != Some(uuid::Version::Random)
        || parsed.hyphenated().to_string() != suffix
    {
        return Err(corrupted(
            "stored native credential physical authority id is invalid",
        ));
    }
    Ok(())
}
