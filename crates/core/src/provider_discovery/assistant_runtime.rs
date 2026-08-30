use super::*;

impl ProviderDiscoveryOrchestrator<'_> {
    pub fn begin_assistant_turn(
        &self,
        session_id: &DiscoverySessionId,
        estimate: AssistantCallEstimate,
    ) -> CoreResult<AssistantPromptPackage> {
        let snapshot = self.get(session_id)?;
        let operation = self
            .storage
            .get_current_discovery_operation(session_id)?
            .ok_or_else(|| CoreError::invalid("assistant discovery has no active operation"))?;
        if operation.kind != DiscoveryOperationKind::BuildAssistantManifestDraft {
            return Err(CoreError::invalid(
                "provider discovery is not running the setup assistant",
            ));
        }
        if operation.status == lorepia_storage::DiscoveryOperationStatus::Prepared
            && !self
                .storage
                .mark_discovery_operation_started(&operation.id, Utc::now())?
        {
            return Err(CoreError::invalid(
                "setup assistant operation changed concurrently",
            ));
        }
        if !matches!(
            operation.status,
            lorepia_storage::DiscoveryOperationStatus::Prepared
                | lorepia_storage::DiscoveryOperationStatus::Started
        ) {
            return Err(CoreError::invalid(
                "setup assistant operation is not active",
            ));
        }
        let mut draft = hydrate_working_draft(&snapshot)?;
        let mut engine = restored_assistant(&draft)?;
        let prompt = engine.begin_turn(estimate).map_err(assistant_error)?;
        synchronize_assistant_snapshot(&mut draft, &engine);
        self.persist_assistant_checkpoint(
            &snapshot,
            &draft,
            DiscoveryAssistantCheckpoint::AwaitingAssistant,
        )?;
        Ok(prompt)
    }

    pub(super) fn run_assistant_with_provider(
        &self,
        session_id: &DiscoverySessionId,
        route: &ModelRoute,
        provider: Arc<dyn Provider>,
        estimate: AssistantCallEstimate,
        credential: Option<&str>,
    ) -> CoreResult<AssistantHostAction> {
        for _ in 0..MAX_ASSISTANT_HOST_STEPS {
            let prompt = self.begin_assistant_turn(session_id, estimate)?;
            let output = self.runtime.block_on(run_setup_assistant_provider_call(
                Arc::clone(&provider),
                route,
                &prompt,
                estimate,
                credential,
            ));
            let action = match output {
                Ok(turn) => self.submit_assistant_turn(session_id, turn)?,
                Err(error) => {
                    let failure_kind = assistant_failure_kind(&error);
                    let retryable = error.recoverable
                        || matches!(
                            error.code,
                            CoreErrorCode::ProviderRateLimited
                                | CoreErrorCode::ProviderUnavailable
                                | CoreErrorCode::NetworkUnavailable
                        );
                    self.record_assistant_failure(session_id, failure_kind, retryable)?;
                    return Err(error);
                }
            };
            match action {
                AssistantHostAction::ExecuteTool {
                    session_id: action_session_id,
                    call_id,
                    call,
                } => {
                    if action_session_id != *session_id {
                        self.interrupt_assistant(
                            session_id,
                            DiscoveryInterruptionOutcome::ConfirmedNoExternalEffect,
                        )?;
                        return Err(CoreError::internal(
                            "setup assistant tool action escaped its discovery session",
                        ));
                    }
                    let result = match self.execute_assistant_tool(session_id, &call) {
                        Ok(result) => result,
                        Err(error) => {
                            self.interrupt_assistant(
                                session_id,
                                DiscoveryInterruptionOutcome::ConfirmedNoExternalEffect,
                            )?;
                            return Err(error);
                        }
                    };
                    if let Err(error) =
                        self.submit_assistant_tool_result(session_id, call_id, result)
                    {
                        self.interrupt_assistant(
                            session_id,
                            DiscoveryInterruptionOutcome::ConfirmedNoExternalEffect,
                        )?;
                        return Err(error);
                    }
                }
                boundary => return Ok(boundary),
            }
        }
        let snapshot = self.get(session_id)?;
        let mut draft = hydrate_working_draft(&snapshot)?;
        let operation_id = snapshot
            .active_operation_id
            .as_ref()
            .ok_or_else(|| CoreError::invalid("setup assistant operation disappeared"))?
            .clone();
        self.persist_operation_completion(
            &snapshot,
            &operation_id,
            &mut draft,
            ProviderDiscoveryAction::Fail {
                failure: DiscoveryFailure {
                    code: "assistant_host_loop_exhausted".to_owned(),
                    message_key: "provider.discovery.assistant_host_loop_exhausted".to_owned(),
                    recoverable: false,
                },
            },
            DurableOperationOutcome::Failed,
            Vec::new(),
            Vec::new(),
            DiscoveryJsonUpdate::Preserve,
        )?;
        Err(CoreError::invalid(
            "setup assistant exceeded its bounded host-action loop",
        ))
    }

    #[allow(clippy::too_many_lines)]
    pub(super) fn execute_assistant_tool(
        &self,
        session_id: &DiscoverySessionId,
        call: &AssistantToolCall,
    ) -> CoreResult<AssistantToolResult> {
        let snapshot = self.get(session_id)?;
        let draft = hydrate_working_draft(&snapshot)?;
        let allowed_evidence_ids = draft
            .evidence_ids
            .iter()
            .chain(&draft.extra_evidence_ids)
            .cloned()
            .collect::<BTreeSet<_>>();
        match call {
            AssistantToolCall::SearchOfficialDocs { query } => {
                let query = query.to_lowercase();
                let evidence_ids = self
                    .storage
                    .list_discovery_evidence(session_id, MAX_DISCOVERY_ROWS)?
                    .into_iter()
                    .filter(|record| allowed_evidence_ids.contains(&record.id))
                    .filter(|record| {
                        serde_json::to_string(&record.extracted_json)
                            .ok()
                            .is_some_and(|value| value.to_lowercase().contains(&query))
                    })
                    .take(128)
                    .map(|record| record.id)
                    .collect();
                Ok(AssistantToolResult::OfficialDocsSearch { evidence_ids })
            }
            AssistantToolCall::InspectEvidence { evidence_id } => {
                if !allowed_evidence_ids.contains(evidence_id) {
                    return Err(CoreError::invalid(
                        "setup assistant requested evidence outside this session",
                    ));
                }
                let record = self
                    .storage
                    .list_discovery_evidence(session_id, MAX_DISCOVERY_ROWS)?
                    .into_iter()
                    .find(|record| record.id == *evidence_id)
                    .ok_or_else(|| {
                        CoreError::new(
                            CoreErrorCode::NotFound,
                            "setup assistant evidence was not found",
                            false,
                        )
                    })?;
                let claims = draft
                    .assistant_evidence_claims
                    .get(&record.id)
                    .cloned()
                    .unwrap_or_default();
                let supported_fields = redacted_assistant_evidence(record, claims)?
                    .claims()
                    .iter()
                    .map(|claim| claim.field().clone())
                    .collect();
                Ok(AssistantToolResult::EvidenceInspection {
                    evidence_id: evidence_id.clone(),
                    supported_fields,
                })
            }
            AssistantToolCall::FetchDiscoveryDocument { candidate_id } => {
                let entry = self
                    .storage
                    .read_discovery_candidates(session_id, MAX_DISCOVERY_ROWS)?
                    .into_iter()
                    .find(|entry| entry.candidate.id == *candidate_id)
                    .ok_or_else(|| {
                        CoreError::new(
                            CoreErrorCode::NotFound,
                            "setup assistant document candidate was not found",
                            false,
                        )
                    })?;
                let evidence_ids = entry
                    .candidate
                    .evidence_ids
                    .into_iter()
                    .filter(|evidence_id| allowed_evidence_ids.contains(evidence_id))
                    .collect();
                Ok(AssistantToolResult::DiscoveryDocumentFetched {
                    candidate_id: candidate_id.clone(),
                    evidence_ids,
                })
            }
            AssistantToolCall::ListModels { connection_id } => {
                let connection = draft.connection.as_ref().ok_or_else(|| {
                    CoreError::invalid("setup assistant has no session-owned connection draft")
                })?;
                if connection.id != *connection_id {
                    return Err(CoreError::invalid(
                        "setup assistant requested models for another connection",
                    ));
                }
                Ok(AssistantToolResult::ModelsListed {
                    connection_id: connection_id.clone(),
                    model_route_ids: draft
                        .routes
                        .iter()
                        .map(|route| route.id.clone())
                        .take(128)
                        .collect(),
                })
            }
            AssistantToolCall::TestConnection { connection_id } => {
                let connection = draft.connection.as_ref().ok_or_else(|| {
                    CoreError::invalid("setup assistant has no session-owned connection draft")
                })?;
                if connection.id != *connection_id {
                    return Err(CoreError::invalid(
                        "setup assistant requested a test for another connection",
                    ));
                }
                let reachable = connection.status == ConnectionStatus::Connected;
                Ok(AssistantToolResult::ConnectionTested {
                    connection_id: connection_id.clone(),
                    reachable,
                    summary: if reachable {
                        "connected".to_owned()
                    } else {
                        "not_tested_before_origin_approval".to_owned()
                    },
                })
            }
            AssistantToolCall::ProbeCapability {
                model_route_id,
                capability,
            } => {
                if !draft.routes.iter().any(|route| route.id == *model_route_id) {
                    return Err(CoreError::invalid(
                        "setup assistant requested a capability for another model route",
                    ));
                }
                let observation = draft.observations.iter().rev().find(|observation| {
                    observation.model_route_id == *model_route_id
                        && observation.key == *capability
                        && observation.is_fresh_at(Utc::now())
                });
                let supported = observation.and_then(capability_observation_support);
                let evidence_ids = observation
                    .and_then(|observation| observation.evidence_ref.clone())
                    .filter(|evidence_id| allowed_evidence_ids.contains(evidence_id))
                    .into_iter()
                    .collect();
                Ok(AssistantToolResult::CapabilityProbed {
                    model_route_id: model_route_id.clone(),
                    capability: *capability,
                    supported,
                    evidence_ids,
                    summary: if observation.is_some() {
                        "existing_session_observation".to_owned()
                    } else {
                        "not_probed_before_capability_consent".to_owned()
                    },
                })
            }
            AssistantToolCall::ListManifestAdapterFamilies => {
                let mut families = draft
                    .deterministic
                    .as_ref()
                    .map(|output| {
                        output
                            .family_candidates
                            .iter()
                            .map(|candidate| candidate.api_family)
                            .collect::<Vec<_>>()
                    })
                    .unwrap_or_default();
                if families.is_empty() {
                    families = AdapterRegistry::built_in_templates()?
                        .into_iter()
                        .map(|template| template.api_family)
                        .collect();
                }
                families.sort_by_key(|family| api_family_slug(*family));
                families.dedup();
                Ok(AssistantToolResult::AdapterFamilies { families })
            }
            AssistantToolCall::ValidateManifestDraft { draft } => {
                let accepted = validate_manifest(&draft.manifest).is_ok();
                Ok(AssistantToolResult::ManifestValidation {
                    accepted,
                    violations: if accepted {
                        Vec::new()
                    } else {
                        vec!["manifest_rejected".to_owned()]
                    },
                })
            }
            AssistantToolCall::ShowUnresolvedQuestions => {
                Ok(AssistantToolResult::UnresolvedQuestions {
                    question_ids: self.current_assistant_unresolved_question_ids(
                        session_id,
                        snapshot.session.revision,
                    )?,
                })
            }
        }
    }

    fn current_assistant_unresolved_question_ids(
        &self,
        requested_session_id: &DiscoverySessionId,
        observed_revision: u64,
    ) -> CoreResult<Vec<String>> {
        let current = self.get(requested_session_id)?;
        let draft = hydrate_working_draft(&current)?;
        Self::validated_assistant_unresolved_question_ids(
            requested_session_id,
            observed_revision,
            &current,
            &draft,
        )
    }

    pub(super) fn validated_assistant_unresolved_question_ids(
        requested_session_id: &DiscoverySessionId,
        observed_revision: u64,
        current: &DiscoverySessionSnapshot,
        draft: &DiscoveryWorkingDraft,
    ) -> CoreResult<Vec<String>> {
        const MAX_QUESTION_COUNT: usize = 128;
        const MAX_QUESTION_ID_BYTES: usize = 128;
        const MAX_QUESTION_TEXT_BYTES: usize = 2 * 1024;
        const MAX_TOOL_RESULT_BYTES: usize = 4 * 1024;

        let corrupted = || {
            CoreError::new(
                CoreErrorCode::StorageCorrupted,
                "provider setup assistant unresolved questions are inconsistent",
                false,
            )
        };
        if current.session.id != *requested_session_id
            || current.session.revision != observed_revision
            || current.session.state != DiscoveryState::BuildingAssistantManifestDraft
        {
            return Err(corrupted());
        }
        let assistant = draft.assistant.as_ref().ok_or_else(&corrupted)?;
        if assistant.session_id() != requested_session_id
            || assistant.state() != AssistantState::AwaitingToolResult
        {
            return Err(corrupted());
        }
        let engine = restored_assistant(draft).map_err(|_| corrupted())?;
        let questions = &draft.assistant_more_evidence_questions;
        if questions.is_empty() || questions.len() > MAX_QUESTION_COUNT {
            return Err(corrupted());
        }

        let mut question_ids = Vec::with_capacity(questions.len());
        let mut previous_id: Option<&str> = None;
        for question in questions {
            let id = question.id.as_str();
            if id.is_empty()
                || id.len() > MAX_QUESTION_ID_BYTES
                || !id.bytes().all(|byte| {
                    byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b':' | b'.')
                })
                || previous_id.is_some_and(|previous| previous >= id)
                || question.question.trim().is_empty()
                || question.required_evidence.trim().is_empty()
                || question.question.len() > MAX_QUESTION_TEXT_BYTES
                || question.required_evidence.len() > MAX_QUESTION_TEXT_BYTES
                || question.question.bytes().any(|byte| byte == 0)
                || question.required_evidence.bytes().any(|byte| byte == 0)
            {
                return Err(corrupted());
            }
            previous_id = Some(id);
            question_ids.push(question.id.clone());
        }
        if engine.unresolved_question_ids() != question_ids {
            return Err(corrupted());
        }

        let result = AssistantToolResult::UnresolvedQuestions {
            question_ids: question_ids.clone(),
        };
        if serde_json::to_vec(&result).map_err(|_| corrupted())?.len() > MAX_TOOL_RESULT_BYTES {
            return Err(corrupted());
        }
        Ok(question_ids)
    }
}

#[allow(clippy::too_many_lines)]
pub(super) async fn run_setup_assistant_provider_call(
    provider: Arc<dyn Provider>,
    route: &ModelRoute,
    prompt: &AssistantPromptPackage,
    estimate: AssistantCallEstimate,
    credential: Option<&str>,
) -> CoreResult<lorepia_providers::setup_assistant::AssistantTurn> {
    let conversation_id = ConversationId::new();
    let mut system = Message::user(
        conversation_id.clone(),
        prompt.system_instruction().to_owned(),
    );
    system.role = MessageRole::System;
    let untrusted_payload = prompt.untrusted_payload_json().map_err(assistant_error)?;
    let user = Message::user(conversation_id.clone(), untrusted_payload);
    let max_output_tokens = u32::try_from(estimate.maximum_output_tokens)
        .map_err(|_| CoreError::invalid("assistant output-token estimate is too large"))?;
    let request = GenerationRequest {
        generation_id: GenerationId::new(),
        conversation_id,
        model: route.model_id.clone(),
        messages: vec![system, user],
        temperature: None,
        max_output_tokens: Some(max_output_tokens),
        resolved_prompt_plan: None,
        provider_execution_plan_hash: None,
        provider_provenance: None,
        preserve_opaque_reasoning_state: false,
        opaque_reasoning_context: Vec::new(),
    };
    let output_limit = usize::try_from(estimate.maximum_output_tokens)
        .unwrap_or(usize::MAX)
        .saturating_mul(16)
        .clamp(1_024, 256 * 1024);
    let (sink, mut events) = mpsc::channel(32);
    let (_cancel_sender, cancel_receiver) = watch::channel(false);
    let request_plan = prompt.provider_request_plan(route.api_family);
    let generation = provider.generate_with_internal_plan(
        request,
        credential,
        sink,
        cancel_receiver,
        request_plan,
    );
    let collect = async move {
        let mut output = Vec::new();
        while let Some(event) = events.recv().await {
            match event {
                ProviderEvent::TextDelta(delta) => {
                    let next = output
                        .len()
                        .checked_add(delta.len())
                        .ok_or_else(|| CoreError::invalid("assistant output exceeded its bound"))?;
                    if next > output_limit {
                        return Err(CoreError::invalid(
                            "assistant output exceeded its bounded response size",
                        ));
                    }
                    output.extend_from_slice(delta.as_bytes());
                }
                ProviderEvent::ReasoningDelta(_) | ProviderEvent::OpaqueReasoningState(_) => {}
                ProviderEvent::ToolCallStarted { .. }
                | ProviderEvent::ToolCallArgumentsDelta { .. }
                | ProviderEvent::ToolCallCompleted { .. } => {
                    return Err(CoreError::invalid(
                        "provider-native tool calls are not allowed in setup assistant mode",
                    ));
                }
            }
        }
        if output.is_empty() {
            return Err(CoreError::invalid(
                "setup assistant returned an empty structured response",
            ));
        }
        Ok(output)
    };
    let (generation_result, output_result) = tokio::join!(generation, collect);
    if let Err(mut error) = generation_result {
        if let Ok(mut output) = output_result {
            output.zeroize();
        }
        let reflected = credential
            .filter(|value| !value.is_empty())
            .is_some_and(|credential| {
                error.message.contains(credential) || error.operation_id.contains(credential)
            });
        if reflected {
            error.message.zeroize();
            error.operation_id.zeroize();
            return Err(CoreError::new(
                CoreErrorCode::ProviderUnavailable,
                "setup assistant provider error reflected credential material",
                false,
            ));
        }
        return Err(error);
    }
    let mut output = output_result?;
    if credential
        .filter(|value| !value.is_empty())
        .is_some_and(|credential| {
            output
                .windows(credential.len())
                .any(|window| window == credential.as_bytes())
        })
    {
        output.zeroize();
        return Err(CoreError::new(
            CoreErrorCode::ProviderUnavailable,
            "setup assistant response reflected credential material",
            false,
        ));
    }
    let turn = match prompt.decode_schema_constrained_response(&output) {
        Ok(turn) => turn,
        Err(error) => {
            output.zeroize();
            return Err(assistant_structured_output_error(error));
        }
    };
    output.zeroize();
    Ok(turn)
}

const fn assistant_failure_kind(error: &CoreError) -> AssistantFailureKind {
    match error.code {
        CoreErrorCode::ProviderRateLimited => AssistantFailureKind::RateLimited,
        CoreErrorCode::NetworkUnavailable | CoreErrorCode::ProviderUnavailable => {
            AssistantFailureKind::Transport
        }
        CoreErrorCode::ProviderAuthFailed | CoreErrorCode::PermissionDenied => {
            AssistantFailureKind::ProviderRejected
        }
        CoreErrorCode::InvalidInput | CoreErrorCode::UnsupportedContent => {
            AssistantFailureKind::InvalidStructuredOutput
        }
        CoreErrorCode::Cancelled => AssistantFailureKind::Timeout,
        CoreErrorCode::UnsafeArchive
        | CoreErrorCode::NotFound
        | CoreErrorCode::StorageUnavailable
        | CoreErrorCode::StorageCorrupted
        | CoreErrorCode::Internal => AssistantFailureKind::Internal,
    }
}

fn capability_observation_support(observation: &CapabilityObservation) -> Option<bool> {
    match observation.status {
        SupportStatus::Unsupported => Some(false),
        SupportStatus::Unknown => None,
        SupportStatus::Verified
        | SupportStatus::Documented
        | SupportStatus::Inferred
        | SupportStatus::Conditional => match &observation.value {
            CapabilityValue::Boolean(value) => Some(*value),
            CapabilityValue::Integer(_)
            | CapabilityValue::EnumValues(_)
            | CapabilityValue::Structured(_) => Some(true),
        },
    }
}

pub(super) const fn api_family_slug(family: ApiFamily) -> &'static str {
    match family {
        ApiFamily::OpenAiResponses => "openai_responses",
        ApiFamily::OpenAiChatCompletions => "openai_chat_completions",
        ApiFamily::AnthropicMessages => "anthropic_messages",
        ApiFamily::GeminiGenerateContent => "gemini_generate_content",
        ApiFamily::OllamaNative => "ollama_native",
    }
}
impl crate::app::Core {
    pub fn run_provider_discovery_assistant_turn(
        &self,
        session_id: &DiscoverySessionId,
        estimate: AssistantCallEstimate,
        credential: Option<&str>,
    ) -> CoreResult<AssistantHostAction> {
        let snapshot = self.provider_discovery().get(session_id)?;
        let draft = hydrate_working_draft(&snapshot)?;
        let assistant_route_id = draft
            .assistant
            .as_ref()
            .ok_or_else(|| CoreError::internal("setup assistant snapshot is missing"))?
            .assistant_route_id()
            .clone();
        let settings = self.get_settings()?;
        let selected_route_id = settings.selected_model_route_id.ok_or_else(|| {
            CoreError::invalid("setup assistant requires a selected model route and preset")
        })?;
        let selected_preset_id = settings.selected_generation_preset_id.ok_or_else(|| {
            CoreError::invalid("setup assistant requires a selected model route and preset")
        })?;
        if selected_route_id != assistant_route_id {
            return Err(CoreError::invalid(
                "setup assistant route must match the selected model route",
            ));
        }
        let target = GenerationTarget {
            model_route_id: selected_route_id.clone(),
            generation_preset_id: selected_preset_id,
        };
        let resolved = crate::app::resolve_generation_target(self, &target)?;
        let route = self.storage().get_model_route(&selected_route_id)?;
        if resolved.model != route.model_id {
            return Err(CoreError::internal(
                "selected setup assistant target resolved to a different model",
            ));
        }
        self.provider_discovery().run_assistant_with_provider(
            session_id,
            &route,
            resolved.provider,
            estimate,
            credential,
        )
    }
}
