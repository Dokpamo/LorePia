use super::{
    AdapterRegistry, CoreError, CoreResult, DateTime, DiscoveredProviderGraph,
    DiscoveryApprovalDecision, DiscoveryApprovalGrant, DiscoveryApprovalId,
    DiscoveryApprovalRecord, DiscoveryCommitAttemptId, DiscoveryCommitPlan,
    DiscoveryPreviousSelection, DiscoveryProbeBudget, DiscoveryReviewChange,
    DiscoveryReviewChangeKind, DiscoveryReviewDiff, DiscoverySessionId, DiscoverySessionSnapshot,
    DiscoveryState, DiscoveryWorkingDraft, MAX_DISCOVERY_ROWS, ProviderDiscoveryOrchestrator,
    RequestPreview, Storage, Utc, assistant_proposal, canonical_serde_sha256, commit_plan_for,
    deterministic_commit_attempt_id, deterministic_id, hydrate_working_draft,
    standard_probe_budget, validate_manifest,
};

/// One immutable approval proposal derived from the current durable state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderDiscoveryApprovalProposal {
    pub id: DiscoveryApprovalId,
    pub grant: DiscoveryApprovalGrant,
    pub grant_sha256: String,
}

/// Review data plus the exact commit values that the approval action must echo.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderDiscoveryReviewProposal {
    pub review: DiscoveryReviewDiff,
    pub approval: ProviderDiscoveryApprovalProposal,
    pub commit_attempt_id: DiscoveryCommitAttemptId,
    pub commit_plan_sha256: String,
    pub request_preview: Option<RequestPreview>,
}

impl ProviderDiscoveryOrchestrator<'_> {
    pub fn approval_proposal(
        &self,
        session_id: &DiscoverySessionId,
    ) -> CoreResult<Option<ProviderDiscoveryApprovalProposal>> {
        let snapshot = self.get(session_id)?;
        let draft = hydrate_working_draft(&snapshot)?;
        proposal_for_state(&snapshot, &draft).transpose()
    }

    pub fn review_proposal(
        &self,
        session_id: &DiscoverySessionId,
    ) -> CoreResult<Option<ProviderDiscoveryReviewProposal>> {
        let snapshot = self.get(session_id)?;
        if snapshot.session.state != DiscoveryState::AwaitingReview {
            return Ok(None);
        }
        let draft = hydrate_working_draft(&snapshot)?;
        let review = snapshot
            .review
            .clone()
            .ok_or_else(|| CoreError::internal("review state has no persisted diff"))?;
        let plan = commit_plan_for(
            self.storage,
            &snapshot,
            &draft,
            deterministic_commit_attempt_id(&snapshot.session.id, snapshot.session.revision),
            &review,
        )?;
        let commit_plan_sha256 = canonical_serde_sha256(&plan, "discovery commit plan")?;
        let approval = approval_proposal_for(
            &snapshot.session.id,
            snapshot.session.revision,
            DiscoveryApprovalGrant::Review {
                review_sha256: review.sha256.clone(),
                graph_sha256: review.graph_sha256.clone(),
            },
        )?;
        let request_preview = match (
            draft.template.as_ref(),
            draft.connection.as_ref(),
            draft.routes.first(),
        ) {
            (Some(template), Some(connection), Some(route)) => Some(
                AdapterRegistry::new()
                    .preview_provider_request(template, connection, route, None)?,
            ),
            _ => None,
        };
        Ok(Some(ProviderDiscoveryReviewProposal {
            review,
            approval,
            commit_attempt_id: plan.attempt_id,
            commit_plan_sha256,
            request_preview,
        }))
    }
}

pub(super) fn approval_proposal_for(
    session_id: &DiscoverySessionId,
    revision: u64,
    grant: DiscoveryApprovalGrant,
) -> CoreResult<ProviderDiscoveryApprovalProposal> {
    grant
        .validate()
        .map_err(|error| CoreError::invalid(format!("invalid discovery approval: {error}")))?;
    let grant_sha256 = canonical_serde_sha256(&grant, "discovery approval grant")?;
    let id = DiscoveryApprovalId::parse(deterministic_id(
        session_id,
        revision,
        &format!("approval:{grant_sha256}"),
    ))
    .map_err(|error| CoreError::internal(format!("approval id failed: {error}")))?;
    Ok(ProviderDiscoveryApprovalProposal {
        id,
        grant,
        grant_sha256,
    })
}

pub(super) fn approval_record(
    snapshot: &DiscoverySessionSnapshot,
    proposal: ProviderDiscoveryApprovalProposal,
    decision: DiscoveryApprovalDecision,
    created_at: DateTime<Utc>,
) -> DiscoveryApprovalRecord {
    DiscoveryApprovalRecord {
        id: proposal.id,
        session_id: snapshot.session.id.clone(),
        session_revision: snapshot.session.revision,
        decision,
        grant: proposal.grant,
        created_at,
    }
}

pub(super) fn require_approval_id(
    actual: &DiscoveryApprovalId,
    proposal: &ProviderDiscoveryApprovalProposal,
) -> CoreResult<()> {
    if actual != &proposal.id {
        return Err(CoreError::invalid(
            "discovery approval identifier does not match the current proposal",
        ));
    }
    Ok(())
}

pub(super) fn require_approval_binding(
    actual_id: &DiscoveryApprovalId,
    actual_sha256: &str,
    proposal: &ProviderDiscoveryApprovalProposal,
) -> CoreResult<()> {
    require_approval_id(actual_id, proposal)?;
    if actual_sha256 != proposal.grant_sha256 {
        return Err(CoreError::invalid(
            "discovery approval hash does not match the exact typed grant",
        ));
    }
    Ok(())
}

pub(super) fn credential_origin_proposal(
    snapshot: &DiscoverySessionSnapshot,
    draft: &DiscoveryWorkingDraft,
) -> CoreResult<ProviderDiscoveryApprovalProposal> {
    let grant = credential_origin_grant(snapshot, draft)?;
    approval_proposal_for(&snapshot.session.id, snapshot.session.revision, grant)
}

pub(super) fn credential_origin_grant(
    snapshot: &DiscoverySessionSnapshot,
    draft: &DiscoveryWorkingDraft,
) -> CoreResult<DiscoveryApprovalGrant> {
    let template = draft
        .template
        .as_ref()
        .ok_or_else(|| CoreError::internal("credential proposal has no template"))?;
    let connection = draft
        .connection
        .as_ref()
        .ok_or_else(|| CoreError::internal("credential proposal has no connection"))?;
    let manifest_sha256 = snapshot
        .session
        .manifest_sha256
        .clone()
        .or_else(|| {
            validate_manifest(&template.default_manifest)
                .ok()
                .map(|validated| validated.sha256().to_owned())
        })
        .ok_or_else(|| CoreError::internal("credential proposal has no manifest hash"))?;
    Ok(DiscoveryApprovalGrant::CredentialOrigin {
        origin: connection.api_origin.clone(),
        auth_binding: template.default_manifest.auth.clone(),
        manifest_sha256,
    })
}

pub(super) fn probe_proposal(
    snapshot: &DiscoverySessionSnapshot,
    draft: &DiscoveryWorkingDraft,
) -> CoreResult<ProviderDiscoveryApprovalProposal> {
    let mut route_ids = draft.probe_route_ids.clone();
    route_ids.sort();
    route_ids.dedup();
    let budget = standard_probe_budget(route_ids.len())?;
    approval_proposal_for(
        &snapshot.session.id,
        snapshot.session.revision,
        DiscoveryApprovalGrant::CapabilityProbe {
            model_route_ids: route_ids,
            budget,
        },
    )
}

pub(super) fn approved_probe_budget(
    storage: &Storage,
    snapshot: &DiscoverySessionSnapshot,
    draft: &DiscoveryWorkingDraft,
) -> CoreResult<DiscoveryProbeBudget> {
    let binding = snapshot
        .session
        .active_effect_approval
        .as_ref()
        .ok_or_else(|| CoreError::invalid("capability probe has no active approval binding"))?;
    let approval = storage
        .list_discovery_approvals(&snapshot.session.id, MAX_DISCOVERY_ROWS)?
        .into_iter()
        .find(|approval| approval.id == binding.approval_id)
        .ok_or_else(|| CoreError::invalid("capability probe approval record is missing"))?;
    if approval.decision != DiscoveryApprovalDecision::Approved
        || canonical_serde_sha256(&approval.grant, "capability probe approval grant")?
            != binding.grant_sha256
    {
        return Err(CoreError::invalid(
            "capability probe approval binding does not match its immutable grant",
        ));
    }
    let DiscoveryApprovalGrant::CapabilityProbe {
        model_route_ids,
        budget,
    } = approval.grant
    else {
        return Err(CoreError::invalid(
            "capability probe approval has the wrong grant type",
        ));
    };
    let mut expected_route_ids = draft.probe_route_ids.clone();
    expected_route_ids.sort();
    expected_route_ids.dedup();
    if model_route_ids != expected_route_ids
        || budget != standard_probe_budget(expected_route_ids.len())?
    {
        return Err(CoreError::invalid(
            "capability probe execution differs from the approved routes or budget",
        ));
    }
    Ok(budget)
}

fn proposal_for_state(
    snapshot: &DiscoverySessionSnapshot,
    draft: &DiscoveryWorkingDraft,
) -> Option<CoreResult<ProviderDiscoveryApprovalProposal>> {
    match snapshot.session.state {
        DiscoveryState::AwaitingCredentialOriginApproval => {
            Some(credential_origin_proposal(snapshot, draft))
        }
        DiscoveryState::AwaitingProbeConsent => Some(probe_proposal(snapshot, draft)),
        DiscoveryState::AwaitingAssistantConsent => Some(assistant_proposal(snapshot, draft)),
        DiscoveryState::AwaitingReview => {
            let review = snapshot.review.as_ref()?;
            Some(sanitized_graph_sha256(draft).and_then(|graph_sha256| {
                approval_proposal_for(
                    &snapshot.session.id,
                    snapshot.session.revision,
                    DiscoveryApprovalGrant::Review {
                        review_sha256: review.sha256.clone(),
                        graph_sha256,
                    },
                )
            }))
        }
        _ => None,
    }
}

pub(super) fn sanitized_graph_sha256(draft: &DiscoveryWorkingDraft) -> CoreResult<String> {
    let template = draft
        .template
        .clone()
        .ok_or_else(|| CoreError::internal("provider graph has no template"))?;
    let connection = draft
        .connection
        .clone()
        .ok_or_else(|| CoreError::internal("provider graph has no connection"))?;
    let placeholder_plan = DiscoveryCommitPlan {
        attempt_id: DiscoveryCommitAttemptId::parse("ownership-hash-placeholder")
            .map_err(|error| CoreError::internal(format!("placeholder id failed: {error}")))?,
        session_id: DiscoverySessionId::from("ownership-hash-placeholder"),
        expected_revision: 0,
        manifest_sha256: "0".repeat(64),
        graph_sha256: "0".repeat(64),
        template_id: template.id.clone(),
        template_version: template.manifest_version,
        connection_id: connection.id.clone(),
        model_route_ids: draft.routes.iter().map(|route| route.id.clone()).collect(),
        credential_ref: connection.credential_ref.clone(),
        credential_approval_id: draft.credential_approval_id.clone(),
        review_sha256: "0".repeat(64),
        catalog_authority: draft.catalog_authority.clone(),
        previous_selection: DiscoveryPreviousSelection::None,
    };
    DiscoveredProviderGraph {
        plan: placeholder_plan,
        plan_sha256: "0".repeat(64),
        template,
        connection,
        routes: draft.routes.clone(),
        observations: draft.observations.clone(),
        presets: draft.presets.clone(),
    }
    .ownership_sha256()
}

pub(super) fn build_review(draft: &DiscoveryWorkingDraft) -> CoreResult<DiscoveryReviewDiff> {
    let graph_sha256 = sanitized_graph_sha256(draft)?;
    let template = draft
        .template
        .as_ref()
        .ok_or_else(|| CoreError::internal("review has no provider template"))?;
    let connection = draft
        .connection
        .as_ref()
        .ok_or_else(|| CoreError::internal("review has no provider connection"))?;
    let mut changes = vec![
        DiscoveryReviewChange {
            kind: DiscoveryReviewChangeKind::Add,
            target_kind: "provider_template".to_owned(),
            target_id: template.id.as_str().to_owned(),
            summary_key: "discovery.review.add_provider_template".to_owned(),
            evidence_ids: draft.evidence_ids.clone(),
        },
        DiscoveryReviewChange {
            kind: DiscoveryReviewChangeKind::Add,
            target_kind: "provider_connection".to_owned(),
            target_id: connection.id.as_str().to_owned(),
            summary_key: "discovery.review.add_provider_connection".to_owned(),
            evidence_ids: draft.evidence_ids.clone(),
        },
    ];
    changes.extend(draft.routes.iter().map(|route| DiscoveryReviewChange {
        kind: DiscoveryReviewChangeKind::Add,
        target_kind: "model_route".to_owned(),
        target_id: route.id.as_str().to_owned(),
        summary_key: "discovery.review.add_model_route".to_owned(),
        evidence_ids: Vec::new(),
    }));
    DiscoveryReviewDiff::new(graph_sha256, changes, 0, draft.probe_failure_count)
        .map_err(|error| CoreError::invalid(format!("invalid discovery review: {error}")))
}

impl crate::app::Core {
    pub fn list_provider_discovery_approvals(
        &self,
        session_id: &DiscoverySessionId,
    ) -> CoreResult<Vec<DiscoveryApprovalRecord>> {
        self.provider_discovery().approvals(session_id)
    }

    pub fn get_provider_discovery_approval_proposal(
        &self,
        session_id: &DiscoverySessionId,
    ) -> CoreResult<Option<ProviderDiscoveryApprovalProposal>> {
        self.provider_discovery().approval_proposal(session_id)
    }

    pub fn get_provider_discovery_review_proposal(
        &self,
        session_id: &DiscoverySessionId,
    ) -> CoreResult<Option<ProviderDiscoveryReviewProposal>> {
        self.provider_discovery().review_proposal(session_id)
    }
}
