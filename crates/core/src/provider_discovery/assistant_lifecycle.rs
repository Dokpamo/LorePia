use super::{
    AdapterRegistry, AssistantError, AssistantFailureKind, AssistantHostAction, AssistantState,
    AssistantToolResult, CoreError, CoreResult, DiscoveryActionId, DiscoveryAssistantCheckpoint,
    DiscoveryFailure, DiscoveryInterruptionOutcome, DiscoveryJsonUpdate, DiscoveryOperationId,
    DiscoveryOperationKind, DiscoverySessionId, DiscoverySessionSnapshot, DiscoveryState,
    DiscoveryTransitionWrite, DiscoveryWorkingDraft, DurableOperationOutcome,
    ProviderDiscoveryAction, ProviderDiscoveryOrchestrator, ProviderManifest, ProviderTemplate,
    ProviderTemplateId, SetupAssistantEngine, TemplateSource, Utc, assistant_checkpoint,
    assistant_error, corrupted_assistant_resume_boundary, deterministic_error,
    embed_discovered_api_base_path, hydrate_working_draft, install_graph_seed_with_embedded_base,
    provider_discovery_action_envelope, restored_assistant, synchronize_assistant_snapshot,
    transition_error, validate_connection_fields, validate_manifest, watch, working_draft_value,
};

impl ProviderDiscoveryOrchestrator<'_> {
    #[cfg(test)]
    pub(super) fn submit_assistant_turn_json(
        &self,
        session_id: &DiscoverySessionId,
        output: &[u8],
    ) -> CoreResult<AssistantHostAction> {
        let snapshot = self.get(session_id)?;
        let draft = hydrate_working_draft(&snapshot)?;
        let mut engine = restored_assistant(&draft)?;
        let submission = engine.submit_turn_json(output);
        self.persist_assistant_submission(&snapshot, draft, engine, submission)
    }

    pub(super) fn submit_assistant_turn(
        &self,
        session_id: &DiscoverySessionId,
        turn: lorepia_providers::setup_assistant::AssistantTurn,
    ) -> CoreResult<AssistantHostAction> {
        let snapshot = self.get(session_id)?;
        let draft = hydrate_working_draft(&snapshot)?;
        let mut engine = restored_assistant(&draft)?;
        let submission = engine.submit_turn(turn);
        self.persist_assistant_submission(&snapshot, draft, engine, submission)
    }

    fn persist_assistant_submission(
        &self,
        snapshot: &DiscoverySessionSnapshot,
        mut draft: DiscoveryWorkingDraft,
        engine: SetupAssistantEngine,
        submission: Result<AssistantHostAction, AssistantError>,
    ) -> CoreResult<AssistantHostAction> {
        let state = engine.state();
        synchronize_assistant_snapshot(&mut draft, &engine);
        match submission {
            Ok(action) => {
                if let AssistantHostAction::RequestMoreEvidence { questions, .. } = &action {
                    let question_count = u32::try_from(questions.len()).map_err(|_| {
                        CoreError::invalid("setup assistant returned too many evidence questions")
                    })?;
                    if draft.assistant_more_evidence_questions != *questions {
                        return Err(corrupted_assistant_resume_boundary());
                    }
                    let operation_id = snapshot
                        .active_operation_id
                        .as_ref()
                        .ok_or_else(|| {
                            CoreError::invalid("assistant discovery has no active operation")
                        })?
                        .clone();
                    self.persist_operation_completion(
                        snapshot,
                        &operation_id,
                        &mut draft,
                        ProviderDiscoveryAction::AssistantRequestedMoreEvidence { question_count },
                        DurableOperationOutcome::Succeeded,
                        Vec::new(),
                        Vec::new(),
                        DiscoveryJsonUpdate::Preserve,
                    )?;
                } else {
                    let checkpoint = assistant_checkpoint(state)?;
                    self.persist_assistant_checkpoint(snapshot, &draft, checkpoint)?;
                }
                Ok(action)
            }
            Err(error) => {
                match state {
                    AssistantState::AwaitingRetryConsent => {
                        self.persist_assistant_checkpoint(
                            snapshot,
                            &draft,
                            DiscoveryAssistantCheckpoint::AwaitingRetryConsent,
                        )?;
                    }
                    AssistantState::Failed => {
                        let operation_id = snapshot
                            .active_operation_id
                            .as_ref()
                            .ok_or_else(|| {
                                CoreError::invalid("assistant discovery has no active operation")
                            })?
                            .clone();
                        self.persist_operation_completion(
                            snapshot,
                            &operation_id,
                            &mut draft,
                            ProviderDiscoveryAction::Fail {
                                failure: DiscoveryFailure {
                                    code: "assistant_invalid_output".to_owned(),
                                    message_key: "provider.discovery.assistant_invalid_output"
                                        .to_owned(),
                                    recoverable: false,
                                },
                            },
                            DurableOperationOutcome::Failed,
                            Vec::new(),
                            Vec::new(),
                            DiscoveryJsonUpdate::Preserve,
                        )?;
                    }
                    _ => {}
                }
                Err(assistant_error(error))
            }
        }
    }

    pub fn submit_assistant_tool_result(
        &self,
        session_id: &DiscoverySessionId,
        call_id: u64,
        result: AssistantToolResult,
    ) -> CoreResult<DiscoverySessionSnapshot> {
        let snapshot = self.get(session_id)?;
        let mut draft = hydrate_working_draft(&snapshot)?;
        let mut engine = restored_assistant(&draft)?;
        engine
            .submit_tool_result(call_id, result)
            .map_err(assistant_error)?;
        synchronize_assistant_snapshot(&mut draft, &engine);
        self.persist_assistant_checkpoint(&snapshot, &draft, DiscoveryAssistantCheckpoint::Ready)?;
        self.get(session_id)
    }

    /// Resumes one already-checkpointed Core-owned typed tool action.
    ///
    /// No model call is made and no native-provided tool payload is accepted.
    /// Every tool remains session-scoped and allowlisted by
    /// [`Self::execute_assistant_tool`], so a crash between execution and the
    /// checkpoint can safely repeat this idempotent read-only action.
    pub fn resume_assistant_core_host_action(
        &self,
        session_id: &DiscoverySessionId,
    ) -> CoreResult<DiscoverySessionSnapshot> {
        let snapshot = self.get(session_id)?;
        if snapshot.session.state != DiscoveryState::BuildingAssistantManifestDraft {
            return Err(CoreError::invalid(
                "provider discovery is not running the setup assistant",
            ));
        }
        let draft = hydrate_working_draft(&snapshot)?;
        let engine = restored_assistant(&draft)?;
        let (call_id, call) = engine.pending_core_tool_call().map_err(assistant_error)?;
        let result = self.execute_assistant_tool(session_id, &call)?;
        self.submit_assistant_tool_result(session_id, call_id, result)
    }

    pub fn approve_assistant_retry(
        &self,
        session_id: &DiscoverySessionId,
    ) -> CoreResult<DiscoverySessionSnapshot> {
        let snapshot = self.get(session_id)?;
        let mut draft = hydrate_working_draft(&snapshot)?;
        let mut engine = restored_assistant(&draft)?;
        engine
            .approve_retry(session_id, true)
            .map_err(assistant_error)?;
        synchronize_assistant_snapshot(&mut draft, &engine);
        self.persist_assistant_checkpoint(&snapshot, &draft, DiscoveryAssistantCheckpoint::Ready)?;
        self.get(session_id)
    }

    pub fn request_assistant_draft_revision(
        &self,
        session_id: &DiscoverySessionId,
    ) -> CoreResult<DiscoverySessionSnapshot> {
        let snapshot = self.get(session_id)?;
        let mut draft = hydrate_working_draft(&snapshot)?;
        let mut engine = restored_assistant(&draft)?;
        engine.request_draft_revision().map_err(assistant_error)?;
        synchronize_assistant_snapshot(&mut draft, &engine);
        self.persist_assistant_checkpoint(
            &snapshot,
            &draft,
            DiscoveryAssistantCheckpoint::AwaitingRetryConsent,
        )?;
        self.get(session_id)
    }

    pub fn accept_assistant_draft(
        &self,
        session_id: &DiscoverySessionId,
    ) -> CoreResult<DiscoverySessionSnapshot> {
        let snapshot = self.get(session_id)?;
        let operation_id = snapshot
            .active_operation_id
            .as_ref()
            .ok_or_else(|| CoreError::invalid("assistant discovery has no active operation"))?
            .clone();
        let mut draft = hydrate_working_draft(&snapshot)?;
        let engine = restored_assistant(&draft)?;
        let review = engine
            .draft_review()
            .ok_or_else(|| CoreError::invalid("setup assistant has no draft to accept"))?;
        if !review.unresolved_conflicts.is_empty() || !review.draft.unresolved_questions.is_empty()
        {
            return Err(CoreError::invalid(
                "setup assistant draft still has unresolved conflicts or questions",
            ));
        }
        install_assistant_graph(&snapshot, &mut draft, &review.draft.manifest)?;
        draft.assistant_approval_binding = None;
        draft.assistant_more_evidence_questions.clear();
        let manifest_sha256 = validate_manifest(&review.draft.manifest)?
            .sha256()
            .to_owned();
        self.persist_operation_completion(
            &snapshot,
            &operation_id,
            &mut draft,
            ProviderDiscoveryAction::ManifestDraftBuilt { manifest_sha256 },
            DurableOperationOutcome::Succeeded,
            Vec::new(),
            Vec::new(),
            DiscoveryJsonUpdate::Preserve,
        )?;
        let (_cancel, cancelled) = watch::channel(false);
        self.drive_nonpersistent(session_id, None, cancelled)
    }

    pub fn record_assistant_failure(
        &self,
        session_id: &DiscoverySessionId,
        kind: AssistantFailureKind,
        retryable: bool,
    ) -> CoreResult<DiscoverySessionSnapshot> {
        let snapshot = self.get(session_id)?;
        let mut draft = hydrate_working_draft(&snapshot)?;
        let mut engine = restored_assistant(&draft)?;
        engine
            .record_failure(kind, retryable)
            .map_err(assistant_error)?;
        let state = engine.state();
        synchronize_assistant_snapshot(&mut draft, &engine);
        if state == AssistantState::AwaitingRetryConsent {
            self.persist_assistant_checkpoint(
                &snapshot,
                &draft,
                DiscoveryAssistantCheckpoint::AwaitingRetryConsent,
            )?;
        } else {
            let operation_id = snapshot
                .active_operation_id
                .as_ref()
                .ok_or_else(|| CoreError::invalid("assistant discovery has no active operation"))?;
            self.persist_operation_completion(
                &snapshot,
                operation_id,
                &mut draft,
                ProviderDiscoveryAction::Fail {
                    failure: DiscoveryFailure {
                        code: "assistant_failed".to_owned(),
                        message_key: "provider.discovery.assistant_failed".to_owned(),
                        recoverable: false,
                    },
                },
                DurableOperationOutcome::Failed,
                Vec::new(),
                Vec::new(),
                DiscoveryJsonUpdate::Preserve,
            )?;
        }
        self.get(session_id)
    }

    pub fn interrupt_assistant(
        &self,
        session_id: &DiscoverySessionId,
        outcome: DiscoveryInterruptionOutcome,
    ) -> CoreResult<DiscoverySessionSnapshot> {
        let snapshot = self.get(session_id)?;
        let mut draft = hydrate_working_draft(&snapshot)?;
        let mut engine = restored_assistant(&draft)?;
        engine.mark_interrupted().map_err(assistant_error)?;
        synchronize_assistant_snapshot(&mut draft, &engine);
        let operation_id = snapshot
            .active_operation_id
            .as_ref()
            .ok_or_else(|| CoreError::invalid("assistant discovery has no active operation"))?;
        let durable_outcome = match outcome {
            DiscoveryInterruptionOutcome::ConfirmedNoExternalEffect => {
                DurableOperationOutcome::Interrupted
            }
            DiscoveryInterruptionOutcome::ExternalOutcomeUnknown => {
                DurableOperationOutcome::OutcomeUnknown
            }
        };
        self.persist_operation_completion(
            &snapshot,
            operation_id,
            &mut draft,
            ProviderDiscoveryAction::Interrupt {
                operation: DiscoveryOperationKind::BuildAssistantManifestDraft,
                outcome,
            },
            durable_outcome,
            Vec::new(),
            Vec::new(),
            DiscoveryJsonUpdate::Preserve,
        )?;
        self.get(session_id)
    }

    pub fn restart_assistant_after_interruption(
        &self,
        session_id: &DiscoverySessionId,
    ) -> CoreResult<DiscoverySessionSnapshot> {
        let snapshot = self.get(session_id)?;
        if snapshot.session.state != DiscoveryState::Interrupted
            || !snapshot.session.recovery.as_ref().is_some_and(|recovery| {
                recovery.operation == DiscoveryOperationKind::BuildAssistantManifestDraft
            })
        {
            return Err(CoreError::invalid(
                "provider setup assistant is not explicitly restartable",
            ));
        }
        let mut draft = hydrate_working_draft(&snapshot)?;
        let mut engine = restored_assistant(&draft)?;
        if engine.state() != AssistantState::Interrupted {
            engine.mark_interrupted().map_err(assistant_error)?;
        }
        engine
            .restart_after_interruption(session_id, true)
            .map_err(assistant_error)?;
        synchronize_assistant_snapshot(&mut draft, &engine);
        let envelope = provider_discovery_action_envelope(
            DiscoveryActionId::new(),
            snapshot.session.revision,
            ProviderDiscoveryAction::RestartInterrupted,
        )?;
        let transition = snapshot
            .session
            .apply(&envelope)
            .map_err(transition_error)?;
        self.storage
            .persist_discovery_transition(&DiscoveryTransitionWrite {
                new_operation_id: Some(DiscoveryOperationId::new()),
                transition,
                draft: DiscoveryJsonUpdate::Replace(working_draft_value(&draft)?),
                review: DiscoveryJsonUpdate::Preserve,
                new_evidence: Vec::new(),
                new_candidates: Vec::new(),
                approval: None,
                completed_operation: None,
                prepared_commit: None,
                provider_graph: None,
                occurred_at: Utc::now(),
            })?;
        self.get(session_id)
    }

    pub(super) fn persist_assistant_checkpoint(
        &self,
        snapshot: &DiscoverySessionSnapshot,
        draft: &DiscoveryWorkingDraft,
        checkpoint: DiscoveryAssistantCheckpoint,
    ) -> CoreResult<()> {
        let envelope = provider_discovery_action_envelope(
            DiscoveryActionId::new(),
            snapshot.session.revision,
            ProviderDiscoveryAction::AssistantCheckpointed { checkpoint },
        )?;
        let transition = snapshot
            .session
            .apply(&envelope)
            .map_err(transition_error)?;
        self.storage
            .persist_discovery_transition(&DiscoveryTransitionWrite {
                transition,
                draft: DiscoveryJsonUpdate::Replace(working_draft_value(draft)?),
                review: DiscoveryJsonUpdate::Preserve,
                new_evidence: Vec::new(),
                new_candidates: Vec::new(),
                approval: None,
                new_operation_id: None,
                completed_operation: None,
                prepared_commit: None,
                provider_graph: None,
                occurred_at: Utc::now(),
            })?;
        Ok(())
    }
}

fn install_assistant_graph(
    snapshot: &DiscoverySessionSnapshot,
    draft: &mut DiscoveryWorkingDraft,
    manifest: &ProviderManifest,
) -> CoreResult<()> {
    let mut manifest = manifest.clone();
    let api_base_path = snapshot
        .session
        .input
        .connection_options
        .api_base_path
        .as_ref()
        .or_else(|| {
            draft.deterministic.as_ref().and_then(|output| {
                output
                    .connection_hints
                    .iter()
                    .find(|hint| hint.api_family == manifest.api_family)
                    .and_then(|hint| hint.api_base_path.as_ref())
            })
        });
    embed_discovered_api_base_path(&mut manifest, api_base_path).map_err(deterministic_error)?;
    let validated = validate_manifest(&manifest)?;
    let manifest_sha256 = validated.sha256().to_owned();
    let connection_fields = AdapterRegistry::built_in_templates()?
        .into_iter()
        .find(|template| template.api_family == manifest.api_family)
        .map(|template| template.connection_fields)
        .unwrap_or_default();
    let template = ProviderTemplate {
        id: ProviderTemplateId::from(format!("discovered-{manifest_sha256}")),
        display_name: snapshot.session.input.display_name.clone(),
        manifest_version: 1,
        source: TemplateSource::UserDiscovered,
        api_family: manifest.api_family,
        connection_fields,
        default_manifest: manifest,
    };
    validate_connection_fields(&template.connection_fields)?;
    install_graph_seed_with_embedded_base(snapshot, draft, template, Utc::now())
}

impl crate::app::Core {
    pub fn approve_provider_discovery_assistant_retry(
        &self,
        session_id: &DiscoverySessionId,
    ) -> CoreResult<DiscoverySessionSnapshot> {
        self.provider_discovery()
            .approve_assistant_retry(session_id)
    }

    pub fn resume_provider_discovery_assistant_core_host_action(
        &self,
        session_id: &DiscoverySessionId,
    ) -> CoreResult<DiscoverySessionSnapshot> {
        self.provider_discovery()
            .resume_assistant_core_host_action(session_id)
    }

    pub fn request_provider_discovery_assistant_revision(
        &self,
        session_id: &DiscoverySessionId,
    ) -> CoreResult<DiscoverySessionSnapshot> {
        self.provider_discovery()
            .request_assistant_draft_revision(session_id)
    }

    pub fn accept_provider_discovery_assistant_draft(
        &self,
        session_id: &DiscoverySessionId,
    ) -> CoreResult<DiscoverySessionSnapshot> {
        self.provider_discovery().accept_assistant_draft(session_id)
    }

    pub fn record_provider_discovery_assistant_failure(
        &self,
        session_id: &DiscoverySessionId,
        kind: AssistantFailureKind,
        retryable: bool,
    ) -> CoreResult<DiscoverySessionSnapshot> {
        self.provider_discovery()
            .record_assistant_failure(session_id, kind, retryable)
    }

    pub fn interrupt_provider_discovery_assistant(
        &self,
        session_id: &DiscoverySessionId,
        outcome: DiscoveryInterruptionOutcome,
    ) -> CoreResult<DiscoverySessionSnapshot> {
        self.provider_discovery()
            .interrupt_assistant(session_id, outcome)
    }

    pub fn restart_provider_discovery_assistant_after_interruption(
        &self,
        session_id: &DiscoverySessionId,
    ) -> CoreResult<DiscoverySessionSnapshot> {
        self.provider_discovery()
            .restart_assistant_after_interruption(session_id)
    }
}
