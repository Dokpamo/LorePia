//! Typed hydration for durable discovery query rows.

use super::{
    CoreResult, DiscoveryActionId, DiscoveryApprovalBinding, DiscoveryApprovalGrant,
    DiscoveryApprovalId, DiscoveryApprovalRecord, DiscoveryCandidate, DiscoveryCandidateId,
    DiscoveryCommitAttemptId, DiscoveryCommitPlan, DiscoveryCompensationRecord,
    DiscoveryCompensationStatus, DiscoveryCompensationStep, DiscoveryEvidenceKind,
    DiscoveryEvidenceRecord, DiscoveryNativeCredentialExecutionRecord, DiscoveryOperationId,
    DiscoveryOperationRecord, DiscoveryOperationStatus, DiscoveryOutboxEvent, DiscoverySessionId,
    DomainCompensationStatus, EvidenceId, HttpUrl, ProviderConnectionId, ProviderDiscoveryEvent,
    StoredDiscoveryCandidate, Value, approval_kind, candidate_kind, contract_error, corrupted,
    decode_redacted_json, encode_approval_grant, enum_wire_result, parse_approval_decision,
    parse_operation_kind, parse_side_effect_class, parse_timestamp, sha256_hex,
    validate_discovery_evidence, validate_discovery_native_physical_authority_id, validate_sha256,
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
