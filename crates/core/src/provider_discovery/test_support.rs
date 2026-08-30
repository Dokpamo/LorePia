use super::{
    AdapterRegistry, BuiltInTemplateId, CoreError, CoreResult, CredentialRef, DiscoveryActionId,
    DiscoveryApprovalId, DiscoveryCandidateSummary, DiscoveryJsonUpdate, DiscoveryOperationId,
    DiscoveryOperationKind, DiscoverySessionSnapshot, DiscoveryTransitionWrite,
    DurableOperationOutcome, HttpUrl, ProviderConnectionId, ProviderDiscoveryAction,
    ProviderDiscoveryConnectionOptions, ProviderDiscoveryCredentialInstallContext,
    ProviderDiscoveryCredentialLeaseContext, SanitizedDiscoveryInput, Utc,
    apply_listed_models_to_draft, hydrate_working_draft, model_candidates, operation_for_effect,
    provider_discovery_action_envelope, transition_error, working_draft_value,
};

/// Exact non-secret contexts for one synthetic Started discovery install.
pub struct SyntheticStartedCredentialInstall {
    pub install: ProviderDiscoveryCredentialInstallContext,
    pub lease: ProviderDiscoveryCredentialLeaseContext,
}

/// Seeds one fixed OpenRouter-shaped commit without network access and
/// advances its credential WAL through the exact native Started cutpoint.
pub fn seed_synthetic_started_credential_install(
    core: &crate::Core,
    connection_id: &str,
) -> CoreResult<SyntheticStartedCredentialInstall> {
    let template = AdapterRegistry::built_in_template(BuiltInTemplateId::OpenRouter)?;
    let connection_id = ProviderConnectionId::from(connection_id);
    let selecting = core.begin_provider_discovery_known(
        SanitizedDiscoveryInput {
            connection_id: connection_id.clone(),
            display_name: "Synthetic Shell direct-capture fixture".to_owned(),
            site_url: HttpUrl::parse("https://docs.openrouter.example/")
                .map_err(CoreError::invalid)?,
            docs_url: None,
            credential_ref: Some(CredentialRef(connection_id.as_str().to_owned())),
            preferred_assistant: None,
            connection_options: ProviderDiscoveryConnectionOptions::default(),
            supplied_evidence_ids: Vec::new(),
        },
        template.id.clone(),
    )?;
    let candidate = core
        .list_provider_discovery_candidates(&selecting.session.id)?
        .into_iter()
        .find(|candidate| {
            matches!(
                &candidate.candidate.summary,
                DiscoveryCandidateSummary::ProviderTemplate {
                    template_id,
                    template_version,
                } if template_id == &template.id
                    && *template_version == template.manifest_version
            )
        })
        .ok_or_else(|| CoreError::internal("synthetic discovery candidate is missing"))?;
    let selected = core.continue_provider_discovery(
        &selecting.session.id,
        provider_discovery_action_envelope(
            DiscoveryActionId::new(),
            selecting.session.revision,
            ProviderDiscoveryAction::SelectTemplate {
                candidate_id: candidate.candidate.id,
            },
        )?,
        None,
    )?;
    let approval = core
        .get_provider_discovery_approval_proposal(&selected.session.id)?
        .ok_or_else(|| CoreError::internal("synthetic credential approval is missing"))?;
    let listed = approve_and_seed_synthetic_listing(core, &selected, approval.id)?;
    let reviewed = core.continue_provider_discovery(
        &listed.session.id,
        provider_discovery_action_envelope(
            DiscoveryActionId::new(),
            listed.session.revision,
            ProviderDiscoveryAction::SkipProbes,
        )?,
        None,
    )?;
    let lease = core.get_provider_discovery_credential_lease_context(&reviewed.session.id)?;
    let proposal = core
        .get_provider_discovery_review_proposal(&reviewed.session.id)?
        .ok_or_else(|| CoreError::internal("synthetic review proposal is missing"))?;
    let committing = core.continue_provider_discovery(
        &reviewed.session.id,
        provider_discovery_action_envelope(
            DiscoveryActionId::new(),
            reviewed.session.revision,
            ProviderDiscoveryAction::ApproveReview {
                approval_id: proposal.approval.id,
                commit_attempt_id: proposal.commit_attempt_id,
                commit_plan_sha256: proposal.commit_plan_sha256,
                graph_sha256: proposal.review.graph_sha256,
            },
        )?,
        None,
    )?;
    let prepared =
        core.get_provider_discovery_credential_install_context(&committing.session.id)?;
    let reserved = core.reserve_provider_discovery_credential_install(
        &prepared.session_id,
        prepared.session_revision,
        &prepared.operation_id,
        &prepared.commit_attempt_id,
        &prepared.commit_plan_sha256,
    )?;
    let reservation_id = reserved
        .native_execution_reservation_id
        .as_deref()
        .ok_or_else(|| CoreError::internal("synthetic reservation is missing"))?;
    let install = core.start_provider_discovery_credential_install(
        &reserved.session_id,
        reserved.session_revision,
        &reserved.operation_id,
        &reserved.commit_attempt_id,
        &reserved.commit_plan_sha256,
        reservation_id,
    )?;
    Ok(SyntheticStartedCredentialInstall { install, lease })
}

fn approve_and_seed_synthetic_listing(
    core: &crate::Core,
    snapshot: &DiscoverySessionSnapshot,
    approval_id: DiscoveryApprovalId,
) -> CoreResult<DiscoverySessionSnapshot> {
    let orchestrator = core.provider_discovery();
    let envelope = provider_discovery_action_envelope(
        DiscoveryActionId::new(),
        snapshot.session.revision,
        ProviderDiscoveryAction::ApproveCredentialOrigin { approval_id },
    )?;
    let mut draft = hydrate_working_draft(snapshot)?;
    let occurred_at = Utc::now();
    let (approval, review, prepared_commit) =
        orchestrator.prepare_user_action(snapshot, &envelope, &mut draft, occurred_at)?;
    let transition = snapshot
        .session
        .apply(&envelope)
        .map_err(transition_error)?;
    let new_operation_id =
        operation_for_effect(&transition.effect).map(|_| DiscoveryOperationId::new());
    orchestrator
        .storage
        .persist_discovery_transition(&DiscoveryTransitionWrite {
            transition,
            draft: DiscoveryJsonUpdate::Replace(working_draft_value(&draft)?),
            review,
            new_evidence: Vec::new(),
            new_candidates: Vec::new(),
            approval,
            new_operation_id,
            completed_operation: None,
            prepared_commit,
            provider_graph: None,
            occurred_at,
        })?;
    let listing = orchestrator.get(&snapshot.session.id)?;
    let operation = orchestrator
        .storage
        .get_current_discovery_operation(&snapshot.session.id)?
        .ok_or_else(|| CoreError::internal("synthetic model listing is missing"))?;
    if operation.kind != DiscoveryOperationKind::ListModels
        || !orchestrator
            .storage
            .mark_discovery_operation_started(&operation.id, Utc::now())?
    {
        return Err(CoreError::internal(
            "synthetic model listing did not reach its start cutpoint",
        ));
    }
    let mut draft = hydrate_working_draft(&listing)?;
    apply_listed_models_to_draft(&mut draft, &[synthetic_listed_model()], Utc::now())?;
    draft.probe_route_ids = draft.routes.iter().map(|route| route.id.clone()).collect();
    let candidates = model_candidates(&listing, &draft)?;
    orchestrator.persist_operation_completion(
        &listing,
        &operation.id,
        &mut draft,
        ProviderDiscoveryAction::ModelsListed {
            model_count: 1,
            probe_candidate_count: 1,
        },
        DurableOperationOutcome::Succeeded,
        Vec::new(),
        candidates,
        DiscoveryJsonUpdate::Preserve,
    )?;
    orchestrator.get(&snapshot.session.id)
}

fn synthetic_listed_model() -> lorepia_providers::ListedModel {
    lorepia_providers::ListedModel {
        model_id: "openai/synthetic-shell-direct-capture".to_owned(),
        display_name: Some("Synthetic Shell direct capture".to_owned()),
        max_input_tokens: Some(4_096),
        max_output_tokens: Some(1_024),
        supported_generation_methods: Vec::new(),
        capabilities: lorepia_providers::ListedModelCapabilities {
            supported: Vec::new(),
            parameters: lorepia_providers::OpenRouterSupportedParameterSupport::Exact(Vec::new()),
            reasoning: None,
        },
        source: lorepia_providers::ModelRecordSource::ProviderApi,
        availability: lorepia_domain::ModelAvailability::Available,
    }
}
