//! Discovery approval validation and durable authority binding.

use super::{
    ApprovalRow, Connection, CoreError, CoreResult, CredentialRedirectPolicy, DateTime,
    DiscoveredProviderGraph, DiscoveryActionId, DiscoveryApprovalDecision, DiscoveryApprovalGrant,
    DiscoveryApprovalId, DiscoveryApprovalRecord, DiscoveryAuthorityReceiptRecord,
    DiscoveryCommitAttemptRecord, DiscoveryCommitPlan, DiscoveryEffect, DiscoveryOperationKind,
    DiscoveryReviewDiff, DiscoveryState, DiscoveryUnknownOutcomeResolution, OptionalExtension, Utc,
    corrupted, database_error, decode_approval_row, load_discovery_authority_receipt_by_action,
    params, parse_timestamp, validate_review_evidence_references,
};

pub(super) fn approval_kind(grant: &DiscoveryApprovalGrant) -> &'static str {
    match grant {
        DiscoveryApprovalGrant::TemplateSelection { .. } => "template_selection",
        DiscoveryApprovalGrant::AssistantConsent { .. } => "assistant_consent",
        DiscoveryApprovalGrant::CredentialOrigin { .. } => "credential_origin",
        DiscoveryApprovalGrant::CapabilityProbe { .. } => "capability_probe",
        DiscoveryApprovalGrant::Review { .. } => "review",
        DiscoveryApprovalGrant::UnknownOutcomeResolution { .. } => "unknown_outcome_resolution",
    }
}

pub(super) fn validate_review_approval(
    transaction: &Connection,
    plan: &DiscoveryCommitPlan,
) -> CoreResult<()> {
    let review_json = transaction
        .query_row(
            "SELECT review_diff_json
             FROM provider_discovery_sessions
             WHERE id = ?1",
            [plan.session_id.as_str()],
            |row| row.get::<_, Option<String>>(0),
        )
        .optional()
        .map_err(database_error)?
        .flatten()
        .ok_or_else(|| CoreError::invalid("provider graph requires a persisted review"))?;
    let review = serde_json::from_str::<DiscoveryReviewDiff>(&review_json)
        .map_err(|_| corrupted("stored provider discovery review is invalid"))?;
    review
        .validate()
        .map_err(|_| corrupted("stored provider discovery review digest is invalid"))?;
    validate_review_evidence_references(transaction, &plan.session_id, &review)?;
    if review.sha256 != plan.review_sha256 || review.graph_sha256 != plan.graph_sha256 {
        return Err(CoreError::invalid(
            "provider graph commit plan differs from the approved review and graph digest",
        ));
    }
    let grants = {
        let mut statement = transaction
            .prepare(
                "SELECT grant_json
                 FROM provider_discovery_approvals
                 WHERE session_id = ?1
                   AND approval_kind = 'review'
                   AND decision = 'approved'",
            )
            .map_err(database_error)?;
        statement
            .query_map([plan.session_id.as_str()], |row| row.get::<_, String>(0))
            .map_err(database_error)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(database_error)?
    };
    let approved = grants.into_iter().any(|grant_json| {
        serde_json::from_str::<DiscoveryApprovalGrant>(&grant_json)
            .ok()
            .is_some_and(|grant| {
                matches!(
                    grant,
                    DiscoveryApprovalGrant::Review {
                        review_sha256,
                        graph_sha256,
                    } if review_sha256 == plan.review_sha256
                        && graph_sha256 == plan.graph_sha256
                )
            })
    });
    if !approved {
        return Err(CoreError::invalid(
            "provider graph requires an exact approved review hash",
        ));
    }
    Ok(())
}

pub(super) fn validate_credential_approval(
    transaction: &Connection,
    graph: &DiscoveredProviderGraph,
) -> CoreResult<()> {
    let (Some(credential_ref), Some(approval_id)) = (
        &graph.plan.credential_ref,
        &graph.plan.credential_approval_id,
    ) else {
        if graph.connection.credential_ref.is_some() || graph.connection.credential_scope.is_some()
        {
            return Err(CoreError::invalid(
                "credential-free commit plans cannot publish credential references",
            ));
        }
        return Ok(());
    };
    if graph.connection.credential_ref.as_ref() != Some(credential_ref) {
        return Err(CoreError::invalid(
            "provider connection credential reference differs from its commit plan",
        ));
    }
    let grant_json = transaction
        .query_row(
            "SELECT grant_json
             FROM provider_discovery_approvals
             WHERE id = ?1
               AND session_id = ?2
               AND approval_kind = 'credential_origin'
               AND decision = 'approved'",
            params![approval_id.as_str(), graph.plan.session_id.as_str()],
            |row| row.get::<_, String>(0),
        )
        .optional()
        .map_err(database_error)?
        .ok_or_else(|| {
            CoreError::invalid("provider graph credential approval was not persisted")
        })?;
    let grant = serde_json::from_str::<DiscoveryApprovalGrant>(&grant_json)
        .map_err(|_| corrupted("stored credential-origin grant is invalid"))?;
    let DiscoveryApprovalGrant::CredentialOrigin {
        origin,
        auth_binding,
        manifest_sha256,
    } = grant
    else {
        return Err(corrupted(
            "stored credential approval has the wrong typed grant",
        ));
    };
    let scope =
        graph.connection.credential_scope.as_ref().ok_or_else(|| {
            CoreError::invalid("credential reference requires a credential scope")
        })?;
    if origin != graph.connection.api_origin
        || auth_binding != scope.auth_binding
        || manifest_sha256 != graph.plan.manifest_sha256
        || scope.allowed_origins.as_slice() != [origin]
        || scope.redirect_policy != CredentialRedirectPolicy::Deny
    {
        return Err(CoreError::invalid(
            "provider credential scope differs from its approved origin grant",
        ));
    }
    Ok(())
}

#[derive(Debug, Clone, Copy)]
pub(super) struct DiscoveryAuthorityApprovalAuditSequences {
    pub(super) credential: u64,
    pub(super) review: u64,
}

pub(super) fn validate_discovery_authority_approval_rows(
    connection: &Connection,
    attempt: &DiscoveryCommitAttemptRecord,
) -> CoreResult<DiscoveryAuthorityApprovalAuditSequences> {
    let rows = {
        let mut statement = connection
            .prepare(
                "SELECT id, session_id, approval_kind, candidate_id, decision,
                        grant_json, session_revision, grant_sha256, created_at
                 FROM provider_discovery_approvals
                 WHERE session_id = ?1
                   AND (approval_kind = 'review' OR id = ?2)",
            )
            .map_err(database_error)?;
        statement
            .query_map(
                params![
                    attempt.session_id.as_str(),
                    attempt
                        .plan
                        .credential_approval_id
                        .as_ref()
                        .map(DiscoveryApprovalId::as_str)
                ],
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
                    ))
                },
            )
            .map_err(database_error)?
            .collect::<Result<Vec<ApprovalRow>, _>>()
            .map_err(database_error)?
    };
    let approvals = rows
        .into_iter()
        .map(decode_approval_row)
        .collect::<CoreResult<Vec<_>>>()?;
    let credential_approval_id = attempt
        .plan
        .credential_approval_id
        .as_ref()
        .ok_or_else(|| corrupted("discovery credential authority has no credential approval"))?;
    let credential_approvals = approvals
        .iter()
        .filter(|approval| {
            approval.id == *credential_approval_id
                && approval.session_id == attempt.session_id
                && approval.decision == DiscoveryApprovalDecision::Approved
                && approval.session_revision < attempt.expected_revision
                && approval.created_at <= attempt.created_at
                && matches!(
                    approval.grant,
                    DiscoveryApprovalGrant::CredentialOrigin { .. }
                )
        })
        .collect::<Vec<_>>();
    let review_approvals = approvals
        .iter()
        .filter(|approval| {
            approval.session_id == attempt.session_id
                && approval.decision == DiscoveryApprovalDecision::Approved
                && approval.session_revision == attempt.expected_revision
                && approval.created_at == attempt.created_at
                && matches!(
                    &approval.grant,
                    DiscoveryApprovalGrant::Review {
                        review_sha256,
                        graph_sha256,
                    } if review_sha256 == &attempt.plan.review_sha256
                        && graph_sha256 == &attempt.plan.graph_sha256
                )
        })
        .collect::<Vec<_>>();
    if credential_approvals.len() != 1 || review_approvals.len() != 1 {
        return Err(corrupted(
            "discovery credential ownership approvals are missing or detached",
        ));
    }
    let credential =
        validate_discovery_approval_subject_audit(connection, credential_approvals[0])?;
    let review = validate_discovery_approval_action_audit(
        connection,
        review_approvals[0],
        &attempt.action_id,
        attempt.expected_revision.saturating_add(1),
        attempt.created_at,
    )?;
    Ok(DiscoveryAuthorityApprovalAuditSequences { credential, review })
}

fn validate_discovery_approval_subject_audit(
    connection: &Connection,
    approval: &DiscoveryApprovalRecord,
) -> CoreResult<u64> {
    let records = {
        let mut statement = connection
            .prepare(
                "SELECT audit_sequence, action_id, session_revision, created_at
                 FROM provider_discovery_audit_log
                 WHERE session_id = ?1
                   AND audit_kind = 'approval_recorded'
                   AND subject_id = ?2
                   AND summary_key = 'discovery.audit.approval_recorded'",
            )
            .map_err(database_error)?;
        statement
            .query_map(
                params![approval.session_id.as_str(), approval.id.as_str()],
                |row| {
                    Ok((
                        row.get::<_, u64>(0)?,
                        row.get::<_, Option<String>>(1)?,
                        row.get::<_, u64>(2)?,
                        row.get::<_, String>(3)?,
                    ))
                },
            )
            .map_err(database_error)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(database_error)?
    };
    let [(approval_audit_sequence, Some(action_id), session_revision, created_at)] =
        records.as_slice()
    else {
        return Err(corrupted(
            "discovery credential approval is detached from its audit action",
        ));
    };
    if *session_revision != approval.session_revision.saturating_add(1)
        || parse_timestamp(created_at, "approval audit created_at")? != approval.created_at
    {
        return Err(corrupted(
            "discovery credential approval is detached from its audit action",
        ));
    }
    let action_id = DiscoveryActionId::parse(action_id)
        .map_err(|_| corrupted("discovery credential approval action id is invalid"))?;
    let receipt =
        load_discovery_authority_receipt_by_action(connection, &approval.session_id, &action_id)?;
    if receipt.receipt.action_kind != "approve_credential_origin"
        || receipt.receipt.expected_revision != approval.session_revision
        || receipt.receipt.resulting_revision != approval.session_revision.saturating_add(1)
        || receipt.receipt.outcome
            != lorepia_domain::discovery::DiscoveryActionReceiptOutcome::Applied
        || receipt.created_at != approval.created_at
        || receipt.transition.session.state != DiscoveryState::ListingModels
        || receipt.transition.effect != DiscoveryEffect::ListModels
        || receipt.transition_audit_sequence >= *approval_audit_sequence
    {
        return Err(corrupted(
            "discovery credential approval is detached from its exact receipt",
        ));
    }
    Ok(*approval_audit_sequence)
}

fn validate_discovery_approval_action_audit(
    connection: &Connection,
    approval: &DiscoveryApprovalRecord,
    action_id: &DiscoveryActionId,
    resulting_revision: u64,
    created_at: DateTime<Utc>,
) -> CoreResult<u64> {
    let records = {
        let mut statement = connection
            .prepare(
                "SELECT audit_sequence, subject_id, session_revision, created_at
                 FROM provider_discovery_audit_log
                 WHERE session_id = ?1
                   AND audit_kind = 'approval_recorded'
                   AND action_id = ?2
                   AND summary_key = 'discovery.audit.approval_recorded'",
            )
            .map_err(database_error)?;
        statement
            .query_map(
                params![approval.session_id.as_str(), action_id.as_str()],
                |row| {
                    Ok((
                        row.get::<_, u64>(0)?,
                        row.get::<_, Option<String>>(1)?,
                        row.get::<_, u64>(2)?,
                        row.get::<_, String>(3)?,
                    ))
                },
            )
            .map_err(database_error)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(database_error)?
    };
    let exact = matches!(
        records.as_slice(),
        [(approval_audit_sequence, Some(subject_id), session_revision, audited_at)]
            if subject_id == approval.id.as_str()
                && *session_revision == resulting_revision
                && parse_timestamp(audited_at, "approval audit created_at")? == created_at
    );
    if !exact {
        return Err(corrupted(
            "discovery approval is detached from its exact receipt action",
        ));
    }
    let receipt =
        load_discovery_authority_receipt_by_action(connection, &approval.session_id, action_id)?;
    if receipt.transition_audit_sequence >= records[0].0 {
        return Err(corrupted(
            "discovery approval audit precedes its transition audit",
        ));
    }
    Ok(records[0].0)
}

pub(super) fn validate_discovery_unknown_outcome_resolution(
    connection: &Connection,
    attempt: &DiscoveryCommitAttemptRecord,
    resolution_receipt: &DiscoveryAuthorityReceiptRecord,
    expected_resolution: &DiscoveryUnknownOutcomeResolution,
) -> CoreResult<u64> {
    let rows = {
        let mut statement = connection
            .prepare(
                "SELECT id, session_id, approval_kind, candidate_id, decision,
                        grant_json, session_revision, grant_sha256, created_at
                 FROM provider_discovery_approvals
                 WHERE session_id = ?1
                   AND approval_kind = 'unknown_outcome_resolution'",
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
                    row.get(7)?,
                    row.get(8)?,
                ))
            })
            .map_err(database_error)?
            .collect::<Result<Vec<ApprovalRow>, _>>()
            .map_err(database_error)?
    };
    let approvals = rows
        .into_iter()
        .map(decode_approval_row)
        .collect::<CoreResult<Vec<_>>>()?;
    let confirmed = approvals
        .iter()
        .filter(|approval| {
            approval.decision == DiscoveryApprovalDecision::Approved
                && approval.session_revision == resolution_receipt.receipt.expected_revision
                && approval.created_at == resolution_receipt.created_at
                && matches!(
                    &approval.grant,
                    DiscoveryApprovalGrant::UnknownOutcomeResolution {
                        operation: DiscoveryOperationKind::AtomicCommit,
                        resolution,
                    } if resolution == expected_resolution
                )
        })
        .collect::<Vec<_>>();
    if confirmed.len() != 1 {
        return Err(corrupted(
            "outcome-unknown discovery credential commit has no exact approved completion",
        ));
    }
    validate_discovery_approval_action_audit(
        connection,
        confirmed[0],
        &resolution_receipt.receipt.action_id,
        resolution_receipt.receipt.resulting_revision,
        resolution_receipt.created_at,
    )
}
