//! Typed row hydration and low-level repository query helpers.

use super::{
    Connection, CoreErrorCode, CoreResult, CredentialRef, DateTime, DiscoveryActionId,
    DiscoveryApprovalBinding, DiscoveryApprovalGrant, DiscoveryApprovalId, DiscoveryApprovalRecord,
    DiscoveryCandidate, DiscoveryCandidateId, DiscoveryCommitAttemptId,
    DiscoveryCommitAttemptRecord, DiscoveryCommitPlan, DiscoveryCompensationRecord,
    DiscoveryCompensationStatus, DiscoveryCompensationStep, DiscoveryEvidenceKind,
    DiscoveryEvidenceRecord, DiscoveryNativeCredentialExecutionRecord, DiscoveryOperationId,
    DiscoveryOperationKind, DiscoveryOperationRecord, DiscoveryOperationStatus,
    DiscoveryOutboxEvent, DiscoveryRecoveryCheckpoint, DiscoveryReviewDiff, DiscoverySessionId,
    DiscoverySessionSnapshot, DiscoverySideEffectClass, DiscoveryState, DomainCompensationStatus,
    EvidenceId, HttpUrl, OptionalExtension, ProviderConnectionId, ProviderDiscoveryEvent,
    ProviderDiscoverySession, SanitizedDiscoveryInput, StoredDiscoveryCandidate, Transaction, Utc,
    Uuid, Value, approval_kind, candidate_kind, contract_error, corrupted, database_error,
    decode_redacted_json, encode_approval_grant, enum_wire_result, load_commit_attempt, params,
    parse_approval_decision, parse_discovery_state, parse_operation_kind, parse_side_effect_class,
    parse_timestamp, sha256_hex, validate_capability_probe_grant, validate_discovery_evidence,
    validate_identifier, validate_legacy_unbound_started_credential_execution,
    validate_pre_store_native_credential_interruption, validate_review_evidence_references,
    validate_sanitized_input, validate_session_evidence_ids, validate_sha256,
};

pub(super) type CompensationRow = (
    String,
    String,
    u32,
    String,
    String,
    String,
    String,
    u32,
    Option<String>,
    String,
    String,
    Option<String>,
);

pub(super) fn decode_compensation_row(
    row: CompensationRow,
    plan: &DiscoveryCommitPlan,
) -> CoreResult<DiscoveryCompensationRecord> {
    let kind = serde_json::from_value(Value::String(row.4))
        .map_err(|_| corrupted("stored discovery compensation kind is invalid"))?;
    let mut step = serde_json::from_str::<DiscoveryCompensationStep>(&row.5)
        .map_err(|_| corrupted("stored discovery compensation step is invalid"))?;
    step.validate_against(plan)
        .map_err(|_| corrupted("stored compensation target differs from its commit plan"))?;
    if step.status != DomainCompensationStatus::Pending {
        return Err(corrupted(
            "stored immutable compensation recipe is not pending",
        ));
    }
    let status = DiscoveryCompensationStatus::parse(&row.6)?;
    if step.ordinal != row.2 || step.action_id.as_str() != row.3 || step.kind != kind {
        return Err(corrupted(
            "stored compensation columns differ from their typed step",
        ));
    }
    step.status = match status {
        DiscoveryCompensationStatus::Pending => DomainCompensationStatus::Pending,
        DiscoveryCompensationStatus::InProgress => DomainCompensationStatus::InProgress,
        DiscoveryCompensationStatus::Completed => DomainCompensationStatus::Completed,
        DiscoveryCompensationStatus::Failed => DomainCompensationStatus::Failed,
        DiscoveryCompensationStatus::OutcomeUnknown => DomainCompensationStatus::OutcomeUnknown,
    };
    let last_failure = row
        .8
        .as_deref()
        .map(|json| {
            let failure = serde_json::from_str(json)
                .map_err(|_| corrupted("stored compensation failure is invalid"))?;
            lorepia_domain::discovery::DiscoveryFailure::validate(&failure)
                .map_err(|_| corrupted("stored compensation failure is invalid"))?;
            Ok(failure)
        })
        .transpose()?;
    Ok(DiscoveryCompensationRecord {
        id: row.0,
        commit_attempt_id: DiscoveryCommitAttemptId::parse(row.1).map_err(contract_error)?,
        ordinal: row.2,
        action_id: DiscoveryActionId::parse(row.3).map_err(contract_error)?,
        kind,
        step,
        status,
        attempt_count: row.7,
        last_failure,
        created_at: parse_timestamp(&row.9, "compensation created_at")?,
        updated_at: parse_timestamp(&row.10, "compensation updated_at")?,
        completed_at: row
            .11
            .as_deref()
            .map(|value| parse_timestamp(value, "compensation completed_at"))
            .transpose()?,
    })
}

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

pub(super) fn load_session_snapshot(
    connection: &Connection,
    session_id: &str,
) -> CoreResult<Option<DiscoverySessionSnapshot>> {
    let row = connection
        .query_row(
            "SELECT id, state, revision, next_event_sequence, sanitized_input_json,
                    draft_json, review_diff_json, error_json, recovery_json,
                    unknown_operation, manifest_sha256, commit_plan_sha256,
                    commit_attempt_id, committed_connection_id, cancellation_pending,
                    active_operation_id, active_effect_approval_json,
                    created_at, updated_at
             FROM provider_discovery_sessions
             WHERE id = ?1",
            [session_id],
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
                    row.get(18)?,
                ))
            },
        )
        .optional()
        .map_err(database_error)?;
    row.map(|row| decode_session_row(connection, row))
        .transpose()
}

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

pub(super) fn decode_evidence_row(
    row: (String, String, String, String, String, String, String),
) -> CoreResult<DiscoveryEvidenceRecord> {
    let evidence = DiscoveryEvidenceRecord {
        id: EvidenceId::from(row.0),
        session_id: DiscoverySessionId::from(row.1),
        kind: DiscoveryEvidenceKind::parse(&row.2)?,
        source_url: HttpUrl::parse(&row.3)
            .map_err(|_| corrupted("stored discovery evidence URL is invalid"))?,
        content_sha256: row.4,
        extracted_json: decode_redacted_json(&row.5, "stored discovery evidence")?,
        fetched_at: parse_timestamp(&row.6, "discovery evidence fetched_at")?,
    };
    validate_discovery_evidence(&evidence)
        .map_err(|_| corrupted("stored discovery evidence violates its contract"))?;
    Ok(evidence)
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

pub(super) fn decode_candidate_row(
    row: (String, String, String, String, String, u64, String),
) -> CoreResult<StoredDiscoveryCandidate> {
    let summary = serde_json::from_str(&row.3)
        .map_err(|_| corrupted("stored discovery candidate summary is invalid"))?;
    let candidate = DiscoveryCandidate {
        id: DiscoveryCandidateId::parse(row.0).map_err(contract_error)?,
        session_id: DiscoverySessionId::from(row.1),
        summary,
        evidence_ids: serde_json::from_str(&row.4)
            .map_err(|_| corrupted("stored candidate evidence references are invalid"))?,
        created_at: parse_timestamp(&row.6, "discovery candidate created_at")?,
    };
    candidate
        .validate()
        .map_err(|_| corrupted("stored discovery candidate violates its contract"))?;
    if candidate_kind(&candidate) != row.2 {
        return Err(corrupted(
            "stored discovery candidate kind does not match its typed summary",
        ));
    }
    Ok(StoredDiscoveryCandidate {
        candidate,
        proposed_revision: row.5,
    })
}

pub(super) type ApprovalRow = (
    String,
    String,
    String,
    Option<String>,
    String,
    String,
    u64,
    String,
    String,
);

pub(super) fn decode_approval_row(row: ApprovalRow) -> CoreResult<DiscoveryApprovalRecord> {
    let decision = parse_approval_decision(&row.4)?;
    let grant = serde_json::from_str::<DiscoveryApprovalGrant>(&row.5)
        .map_err(|_| corrupted("stored discovery approval grant is invalid"))?;
    let canonical_grant = encode_approval_grant(&grant)
        .map_err(|_| corrupted("stored discovery approval grant is not canonical"))?;
    let expected_candidate_id = match &grant {
        DiscoveryApprovalGrant::TemplateSelection { candidate_id } => Some(candidate_id.as_str()),
        _ => None,
    };
    if row.2 != approval_kind(&grant)
        || row.3.as_deref() != expected_candidate_id
        || row.5 != canonical_grant
        || row.7 != sha256_hex(canonical_grant.as_bytes())
    {
        return Err(corrupted(
            "stored discovery approval columns do not match its typed grant",
        ));
    }
    let approval = DiscoveryApprovalRecord {
        id: DiscoveryApprovalId::parse(row.0).map_err(contract_error)?,
        session_id: DiscoverySessionId::from(row.1),
        session_revision: row.6,
        decision,
        grant,
        created_at: parse_timestamp(&row.8, "discovery approval created_at")?,
    };
    approval
        .validate()
        .map_err(|_| corrupted("stored discovery approval violates its contract"))?;
    Ok(approval)
}

pub(super) type NativeCredentialExecutionRow = (
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
    Option<String>,
    Option<u32>,
    Option<u32>,
);

pub(super) fn load_native_credential_execution_row(
    connection: &Connection,
    operation_id: &DiscoveryOperationId,
) -> CoreResult<Option<NativeCredentialExecutionRow>> {
    connection
        .query_row(
            "SELECT execution.physical_authority_id, execution.operation_id,
                    execution.session_id, execution.commit_attempt_id,
                    execution.commit_plan_sha256, execution.connection_id,
                    execution.connection_binding_sha256, execution.reserved_at,
                    execution.schema_version, execution.redaction_version,
                    store_attempt.started_at, store_attempt.schema_version,
                    store_attempt.redaction_version
             FROM provider_discovery_native_credential_executions AS execution
             LEFT JOIN provider_discovery_native_credential_store_attempts AS store_attempt
               ON store_attempt.operation_id = execution.operation_id
              AND store_attempt.physical_authority_id = execution.physical_authority_id
             WHERE execution.operation_id = ?1",
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
                ))
            },
        )
        .optional()
        .map_err(database_error)
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

pub(super) fn decode_native_credential_execution_row(
    row: NativeCredentialExecutionRow,
) -> CoreResult<(DiscoveryNativeCredentialExecutionRecord, String)> {
    let (
        physical_authority_id,
        operation_id,
        session_id,
        commit_attempt_id,
        commit_plan_sha256,
        connection_id,
        connection_binding_sha256,
        reserved_at,
        schema_version,
        redaction_version,
        store_started_at,
        store_schema_version,
        store_redaction_version,
    ) = row;
    validate_discovery_native_physical_authority_id(&physical_authority_id)?;
    validate_sha256("discovery native credential plan hash", &commit_plan_sha256)
        .map_err(|_| corrupted("stored native credential execution plan hash is invalid"))?;
    validate_sha256(
        "discovery native credential connection binding",
        &connection_binding_sha256,
    )
    .map_err(|_| corrupted("stored native credential execution connection binding is invalid"))?;
    if schema_version != 1
        || redaction_version != 1
        || store_started_at.is_some() != store_schema_version.is_some()
        || store_started_at.is_some() != store_redaction_version.is_some()
        || store_schema_version.is_some_and(|version| version != 1)
        || store_redaction_version.is_some_and(|version| version != 1)
    {
        return Err(corrupted(
            "stored native credential execution version is unsupported",
        ));
    }
    let record = DiscoveryNativeCredentialExecutionRecord {
        physical_authority_id,
        operation_id: DiscoveryOperationId::parse(operation_id)
            .map_err(|_| corrupted("stored native credential execution operation id is invalid"))?,
        session_id: DiscoverySessionId::from(session_id),
        commit_attempt_id: DiscoveryCommitAttemptId::parse(commit_attempt_id)
            .map_err(|_| corrupted("stored native credential execution attempt id is invalid"))?,
        commit_plan_sha256,
        connection_id: ProviderConnectionId::from(connection_id),
        connection_binding_sha256,
        reserved_at: parse_timestamp(&reserved_at, "native credential execution reserved_at")?,
        store_started_at: store_started_at
            .as_deref()
            .map(|value| parse_timestamp(value, "native credential store attempt started_at"))
            .transpose()?,
    };
    Ok((record, reserved_at))
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

pub(super) fn load_discovery_native_credential_execution(
    connection: &Connection,
    operation_id: &DiscoveryOperationId,
) -> CoreResult<Option<DiscoveryNativeCredentialExecutionRecord>> {
    let row = load_native_credential_execution_row(connection, operation_id)?;
    let operation = load_operation_by_id(connection, operation_id)?;
    let Some(row) = row else {
        validate_missing_native_credential_execution(connection, &operation)?;
        return Ok(None);
    };
    let (execution, reserved_at_raw) = decode_native_credential_execution_row(row)?;
    let attempt = load_commit_attempt(connection, &execution.commit_attempt_id)?;
    let valid_abandonment = validate_native_credential_abandonment(
        connection,
        &operation,
        &execution,
        &reserved_at_raw,
    )?;
    validate_native_credential_execution_commit_binding(
        connection,
        &operation,
        &attempt,
        &execution,
        valid_abandonment,
    )?;
    Ok(Some(execution))
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

pub(super) fn load_operation_by_id(
    connection: &Connection,
    operation_id: &DiscoveryOperationId,
) -> CoreResult<DiscoveryOperationRecord> {
    let row = connection
        .query_row(
            "SELECT id, session_id, operation_kind, side_effect_class, status,
                    action_id, expected_revision, request_sha256, approval_id,
                    approval_grant_sha256, started_at, finished_at, created_at, updated_at
             FROM provider_discovery_operations
             WHERE id = ?1",
            [operation_id.as_str()],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, String>(4)?,
                    row.get::<_, String>(5)?,
                    row.get::<_, u64>(6)?,
                    row.get::<_, String>(7)?,
                    row.get::<_, Option<String>>(8)?,
                    row.get::<_, Option<String>>(9)?,
                    row.get::<_, Option<String>>(10)?,
                    row.get::<_, Option<String>>(11)?,
                    row.get::<_, String>(12)?,
                    row.get::<_, String>(13)?,
                ))
            },
        )
        .optional()
        .map_err(database_error)?
        .ok_or_else(|| corrupted("active discovery operation is missing"))?;
    decode_operation_row(row)
}

pub(super) type OperationRow = (
    String,
    String,
    String,
    String,
    String,
    String,
    u64,
    String,
    Option<String>,
    Option<String>,
    Option<String>,
    Option<String>,
    String,
    String,
);

pub(super) fn decode_operation_row(row: OperationRow) -> CoreResult<DiscoveryOperationRecord> {
    let approval = match (row.8, row.9) {
        (None, None) => None,
        (Some(approval_id), Some(grant_sha256)) => Some(DiscoveryApprovalBinding {
            approval_id: DiscoveryApprovalId::parse(approval_id).map_err(contract_error)?,
            grant_sha256,
        }),
        _ => {
            return Err(corrupted(
                "stored discovery operation has a partial approval binding",
            ));
        }
    };
    if let Some(binding) = &approval {
        binding
            .validate()
            .map_err(|_| corrupted("stored operation approval binding is invalid"))?;
    }
    let kind = parse_operation_kind(&row.2)?;
    let side_effect_class = parse_side_effect_class(&row.3)?;
    if kind.side_effect_class() != side_effect_class {
        return Err(corrupted(
            "stored discovery operation side-effect class does not match its kind",
        ));
    }
    Ok(DiscoveryOperationRecord {
        id: DiscoveryOperationId::parse(row.0).map_err(contract_error)?,
        session_id: DiscoverySessionId::from(row.1),
        kind,
        side_effect_class,
        status: DiscoveryOperationStatus::parse(&row.4)?,
        action_id: DiscoveryActionId::parse(row.5).map_err(contract_error)?,
        expected_revision: row.6,
        request_sha256: row.7,
        approval,
        started_at: row
            .10
            .as_deref()
            .map(|value| parse_timestamp(value, "discovery operation started_at"))
            .transpose()?,
        finished_at: row
            .11
            .as_deref()
            .map(|value| parse_timestamp(value, "discovery operation finished_at"))
            .transpose()?,
        created_at: parse_timestamp(&row.12, "discovery operation created_at")?,
        updated_at: parse_timestamp(&row.13, "discovery operation updated_at")?,
    })
}

pub(super) fn load_pollable_outbox_rows(
    transaction: &Transaction<'_>,
    limit: u32,
    available_at: DateTime<Utc>,
) -> CoreResult<Vec<DiscoveryOutboxEvent>> {
    let mut statement = transaction
        .prepare(
            "SELECT event.id, event.session_id, event.sequence, event.event_version,
                    event.session_revision, event.state, event.event_json,
                    event.delivery_attempts, event.available_at, event.created_at
             FROM provider_discovery_event_outbox AS event
             WHERE event.delivered_at IS NULL
               AND event.available_at <= ?1
               AND NOT EXISTS (
                   SELECT 1
                   FROM provider_discovery_event_outbox AS earlier
                   WHERE earlier.session_id = event.session_id
                     AND earlier.delivered_at IS NULL
                     AND earlier.sequence < event.sequence
               )
             ORDER BY event.available_at, event.session_id, event.sequence
             LIMIT ?2",
        )
        .map_err(database_error)?;
    statement
        .query_map(params![available_at.to_rfc3339(), limit], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, u64>(2)?,
                row.get::<_, u32>(3)?,
                row.get::<_, u64>(4)?,
                row.get::<_, String>(5)?,
                row.get::<_, String>(6)?,
                row.get::<_, u32>(7)?,
                row.get::<_, String>(8)?,
                row.get::<_, String>(9)?,
            ))
        })
        .map_err(database_error)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(database_error)?
        .into_iter()
        .map(decode_outbox_row)
        .collect()
}

pub(super) fn load_pollable_outbox_rows_for_session(
    transaction: &Transaction<'_>,
    session_id: &DiscoverySessionId,
    limit: u32,
    available_at: DateTime<Utc>,
) -> CoreResult<Vec<DiscoveryOutboxEvent>> {
    let mut statement = transaction
        .prepare(
            "SELECT event.id, event.session_id, event.sequence, event.event_version,
                    event.session_revision, event.state, event.event_json,
                    event.delivery_attempts, event.available_at, event.created_at
             FROM provider_discovery_event_outbox AS event
             WHERE event.session_id = ?2
               AND event.delivered_at IS NULL
               AND event.available_at <= ?1
               AND NOT EXISTS (
                   SELECT 1
                   FROM provider_discovery_event_outbox AS earlier
                   WHERE earlier.session_id = event.session_id
                     AND earlier.delivered_at IS NULL
                     AND earlier.sequence < event.sequence
               )
             ORDER BY event.available_at, event.session_id, event.sequence
             LIMIT ?3",
        )
        .map_err(database_error)?;
    statement
        .query_map(
            params![available_at.to_rfc3339(), session_id.as_str(), limit],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, u64>(2)?,
                    row.get::<_, u32>(3)?,
                    row.get::<_, u64>(4)?,
                    row.get::<_, String>(5)?,
                    row.get::<_, String>(6)?,
                    row.get::<_, u32>(7)?,
                    row.get::<_, String>(8)?,
                    row.get::<_, String>(9)?,
                ))
            },
        )
        .map_err(database_error)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(database_error)?
        .into_iter()
        .map(decode_outbox_row)
        .collect()
}

pub(super) type OutboxRow = (
    String,
    String,
    u64,
    u32,
    u64,
    String,
    String,
    u32,
    String,
    String,
);

pub(super) fn decode_outbox_row(row: OutboxRow) -> CoreResult<DiscoveryOutboxEvent> {
    let event = serde_json::from_str::<ProviderDiscoveryEvent>(&row.6)
        .map_err(|_| corrupted("stored discovery outbox event is invalid"))?;
    if event.id.as_str() != row.0
        || event.session_id.as_str() != row.1
        || event.sequence != row.2
        || event.version != row.3
        || event.session_revision != row.4
        || enum_wire_result(serde_json::to_value(event.state), "discovery event state")? != row.5
    {
        return Err(corrupted(
            "stored discovery outbox columns do not match the typed event",
        ));
    }
    Ok(DiscoveryOutboxEvent {
        event,
        delivery_attempts: row.7,
        available_at: parse_timestamp(&row.8, "discovery event available_at")?,
        created_at: parse_timestamp(&row.9, "discovery event created_at")?,
    })
}
