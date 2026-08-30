use super::assistant_runtime::api_family_slug;
use super::*;

/// One exact native action which can safely resume a durable setup-assistant
/// boundary. Native clients must not infer this from the overall discovery
/// state or from opaque draft JSON.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProviderDiscoveryAssistantResumeAction {
    ApproveConsent,
    RunAssistant,
    WaitForAssistantOutcome,
    ResumeCoreHostAction,
    SupplyMoreEvidence,
    ApproveRetry,
    ReviewDraft,
    RestartInterrupted,
    ResolveUnknownOutcome,
}

/// Typed, secret-free recovery surface for a setup-assistant session.
#[derive(Debug, Clone, PartialEq)]
pub struct ProviderDiscoveryAssistantResumeBoundary {
    pub checkpoint: Option<DiscoveryAssistantCheckpoint>,
    pub action: ProviderDiscoveryAssistantResumeAction,
    pub questions: Vec<UnresolvedQuestion>,
    pub draft_review: Option<AssistantDraftReview>,
}

impl ProviderDiscoveryOrchestrator<'_> {
    pub fn assistant_resume_boundary(
        &self,
        session_id: &DiscoverySessionId,
    ) -> CoreResult<Option<ProviderDiscoveryAssistantResumeBoundary>> {
        let snapshot = self.get(session_id)?;
        let draft = hydrate_working_draft(&snapshot)?;
        match snapshot.session.state {
            DiscoveryState::AwaitingAssistantConsent => {
                let engine = restored_assistant(&draft)?;
                if engine.state() != AssistantState::AwaitingConsent {
                    return Err(corrupted_assistant_resume_boundary());
                }
                Ok(Some(ProviderDiscoveryAssistantResumeBoundary {
                    checkpoint: None,
                    action: ProviderDiscoveryAssistantResumeAction::ApproveConsent,
                    questions: Vec::new(),
                    draft_review: None,
                }))
            }
            DiscoveryState::BuildingAssistantManifestDraft => {
                let engine = restored_assistant(&draft)?;
                let checkpoint = assistant_checkpoint(engine.state())?;
                let action = match engine.state() {
                    AssistantState::Ready => ProviderDiscoveryAssistantResumeAction::RunAssistant,
                    AssistantState::AwaitingAssistant => {
                        ProviderDiscoveryAssistantResumeAction::WaitForAssistantOutcome
                    }
                    AssistantState::AwaitingToolResult => {
                        ProviderDiscoveryAssistantResumeAction::ResumeCoreHostAction
                    }
                    AssistantState::AwaitingRetryConsent => {
                        ProviderDiscoveryAssistantResumeAction::ApproveRetry
                    }
                    AssistantState::DraftReady => {
                        ProviderDiscoveryAssistantResumeAction::ReviewDraft
                    }
                    AssistantState::AwaitingMoreEvidence
                    | AssistantState::AwaitingConsent
                    | AssistantState::Interrupted
                    | AssistantState::Failed
                    | AssistantState::Cancelled => {
                        return Err(corrupted_assistant_resume_boundary());
                    }
                };
                let draft_review = if action == ProviderDiscoveryAssistantResumeAction::ReviewDraft
                {
                    Some(
                        engine
                            .draft_review()
                            .cloned()
                            .ok_or_else(corrupted_assistant_resume_boundary)?,
                    )
                } else {
                    None
                };
                Ok(Some(ProviderDiscoveryAssistantResumeBoundary {
                    checkpoint: Some(checkpoint),
                    action,
                    questions: Vec::new(),
                    draft_review,
                }))
            }
            DiscoveryState::AwaitingMoreEvidence if draft.assistant.is_some() => {
                let engine = restored_assistant(&draft)?;
                if engine.state() != AssistantState::AwaitingMoreEvidence
                    || draft.assistant_more_evidence_questions.is_empty()
                {
                    return Err(corrupted_assistant_resume_boundary());
                }
                Ok(Some(ProviderDiscoveryAssistantResumeBoundary {
                    checkpoint: Some(DiscoveryAssistantCheckpoint::AwaitingMoreEvidence),
                    action: ProviderDiscoveryAssistantResumeAction::SupplyMoreEvidence,
                    questions: draft.assistant_more_evidence_questions,
                    draft_review: None,
                }))
            }
            DiscoveryState::Interrupted
                if snapshot.session.recovery.as_ref().is_some_and(|recovery| {
                    recovery.operation == DiscoveryOperationKind::BuildAssistantManifestDraft
                }) =>
            {
                Ok(Some(ProviderDiscoveryAssistantResumeBoundary {
                    checkpoint: None,
                    action: ProviderDiscoveryAssistantResumeAction::RestartInterrupted,
                    questions: Vec::new(),
                    draft_review: None,
                }))
            }
            DiscoveryState::UnknownOutcome
                if snapshot.session.unknown_operation
                    == Some(DiscoveryOperationKind::BuildAssistantManifestDraft) =>
            {
                Ok(Some(ProviderDiscoveryAssistantResumeBoundary {
                    checkpoint: None,
                    action: ProviderDiscoveryAssistantResumeAction::ResolveUnknownOutcome,
                    questions: Vec::new(),
                    draft_review: None,
                }))
            }
            _ => Ok(None),
        }
    }
}

pub(crate) fn resumable_assistant_operation_ids(
    storage: &Storage,
) -> CoreResult<BTreeSet<DiscoveryOperationId>> {
    let mut resumable = BTreeSet::new();
    for snapshot in storage.list_unfinished_discovery_sessions_for_recovery()? {
        if snapshot.session.state != DiscoveryState::BuildingAssistantManifestDraft {
            continue;
        }
        let operation_id = snapshot.active_operation_id.as_ref().ok_or_else(|| {
            CoreError::new(
                CoreErrorCode::StorageCorrupted,
                "active setup assistant has no durable operation",
                false,
            )
        })?;
        let operation = storage
            .get_current_discovery_operation(&snapshot.session.id)?
            .ok_or_else(|| {
                CoreError::new(
                    CoreErrorCode::StorageCorrupted,
                    "active setup assistant operation is missing",
                    false,
                )
            })?;
        if operation.id != *operation_id
            || operation.kind != DiscoveryOperationKind::BuildAssistantManifestDraft
        {
            return Err(CoreError::new(
                CoreErrorCode::StorageCorrupted,
                "active setup assistant operation does not match its session",
                false,
            ));
        }
        let engine = restored_assistant(&hydrate_working_draft(&snapshot)?)?;
        if matches!(
            engine.state(),
            AssistantState::Ready
                | AssistantState::AwaitingToolResult
                | AssistantState::AwaitingRetryConsent
                | AssistantState::DraftReady
        ) {
            resumable.insert(operation_id.clone());
        }
    }
    Ok(resumable)
}
pub(super) fn assistant_error(error: impl std::fmt::Display) -> CoreError {
    CoreError::invalid(format!(
        "provider setup assistant rejected the action: {error}"
    ))
}

pub(super) fn assistant_structured_output_error(error: impl std::fmt::Display) -> CoreError {
    CoreError::new(
        CoreErrorCode::InvalidInput,
        format!("provider setup assistant returned invalid structured output: {error}"),
        true,
    )
}

pub(super) fn corrupted_assistant_resume_boundary() -> CoreError {
    CoreError::new(
        CoreErrorCode::StorageCorrupted,
        "provider setup assistant recovery state is inconsistent",
        false,
    )
}

pub(super) fn restored_assistant(
    draft: &DiscoveryWorkingDraft,
) -> CoreResult<SetupAssistantEngine> {
    let engine = SetupAssistantEngine::from_snapshot(
        draft
            .assistant
            .clone()
            .ok_or_else(|| CoreError::internal("setup assistant snapshot is missing"))?,
    )
    .map_err(|_| corrupted_assistant_resume_boundary())?;
    if engine.unresolved_questions() != draft.assistant_more_evidence_questions {
        return Err(corrupted_assistant_resume_boundary());
    }
    Ok(engine)
}

pub(super) fn assistant_checkpoint(
    state: AssistantState,
) -> CoreResult<DiscoveryAssistantCheckpoint> {
    match state {
        AssistantState::Ready => Ok(DiscoveryAssistantCheckpoint::Ready),
        AssistantState::AwaitingAssistant => Ok(DiscoveryAssistantCheckpoint::AwaitingAssistant),
        AssistantState::AwaitingToolResult => Ok(DiscoveryAssistantCheckpoint::AwaitingToolResult),
        AssistantState::AwaitingMoreEvidence => {
            Ok(DiscoveryAssistantCheckpoint::AwaitingMoreEvidence)
        }
        AssistantState::AwaitingRetryConsent => {
            Ok(DiscoveryAssistantCheckpoint::AwaitingRetryConsent)
        }
        AssistantState::DraftReady => Ok(DiscoveryAssistantCheckpoint::DraftReady),
        AssistantState::AwaitingConsent
        | AssistantState::Interrupted
        | AssistantState::Failed
        | AssistantState::Cancelled => Err(CoreError::invalid(
            "setup assistant state cannot be checkpointed in the active operation",
        )),
    }
}
pub(super) fn assistant_proposal(
    snapshot: &DiscoverySessionSnapshot,
    draft: &DiscoveryWorkingDraft,
) -> CoreResult<ProviderDiscoveryApprovalProposal> {
    let engine = SetupAssistantEngine::from_snapshot(
        draft
            .assistant
            .clone()
            .ok_or_else(|| CoreError::internal("assistant proposal has no durable snapshot"))?,
    )
    .map_err(assistant_error)?;
    let request = engine.consent_request().map_err(assistant_error)?;
    let mut evidence_ids = request.evidence_ids;
    evidence_ids.sort();
    evidence_ids.dedup();
    let mut allowed_document_origins = request
        .source_origins
        .into_iter()
        .map(|origin| {
            CanonicalOrigin::parse(&origin)
                .map_err(|error| CoreError::invalid(format!("invalid assistant origin: {error}")))
        })
        .collect::<CoreResult<Vec<_>>>()?;
    allowed_document_origins.sort_by(|left, right| left.as_str().cmp(right.as_str()));
    allowed_document_origins.dedup();
    let max_input_tokens = u32::try_from(request.budget.max_input_tokens)
        .map_err(|_| CoreError::invalid("assistant input budget exceeds the approval contract"))?;
    let max_output_tokens = u32::try_from(request.budget.max_output_tokens)
        .map_err(|_| CoreError::invalid("assistant output budget exceeds the approval contract"))?;
    approval_proposal_for(
        &snapshot.session.id,
        snapshot.session.revision,
        DiscoveryApprovalGrant::AssistantConsent {
            assistant_route_id: request.assistant_route_id,
            evidence_ids,
            allowed_document_origins,
            max_calls: request.budget.max_turns,
            max_input_tokens,
            max_output_tokens,
            max_tool_calls: request.budget.max_tool_calls,
            max_retries: request.budget.max_retries,
            max_cost_micro_units: request.budget.max_cost_micro_units,
        },
    )
}

pub(super) fn grant_assistant_snapshot(
    snapshot: &DiscoverySessionSnapshot,
    draft: &mut DiscoveryWorkingDraft,
    grant: &DiscoveryApprovalGrant,
) -> CoreResult<()> {
    let DiscoveryApprovalGrant::AssistantConsent {
        assistant_route_id,
        evidence_ids,
        allowed_document_origins,
        ..
    } = grant
    else {
        return Err(CoreError::internal(
            "assistant approval used a non-assistant grant",
        ));
    };
    let mut engine = restored_assistant(draft)?;
    engine
        .grant_consent(AssistantConsent {
            session_id: snapshot.session.id.clone(),
            assistant_route_id: assistant_route_id.clone(),
            approved_evidence_ids: evidence_ids.clone(),
            approved_source_origins: allowed_document_origins
                .iter()
                .map(|origin| origin.as_str().to_owned())
                .collect(),
            allow_document_egress: true,
        })
        .map_err(assistant_error)?;
    synchronize_assistant_snapshot(draft, &engine);
    Ok(())
}

pub(super) fn synchronize_assistant_snapshot(
    draft: &mut DiscoveryWorkingDraft,
    engine: &SetupAssistantEngine,
) {
    draft.assistant_more_evidence_questions = engine.unresolved_questions().to_vec();
    draft.assistant = Some(engine.snapshot());
}

pub(super) fn cancel_assistant_snapshot(draft: &mut DiscoveryWorkingDraft) -> CoreResult<()> {
    if draft.assistant.is_none() {
        draft.assistant_more_evidence_questions.clear();
        return Ok(());
    }
    let mut engine = restored_assistant(draft)?;
    if !matches!(
        engine.state(),
        AssistantState::DraftReady | AssistantState::Failed | AssistantState::Cancelled
    ) {
        engine.cancel().map_err(assistant_error)?;
    }
    synchronize_assistant_snapshot(draft, &engine);
    Ok(())
}

pub(super) fn initialize_assistant(
    storage: &Storage,
    snapshot: &DiscoverySessionSnapshot,
    draft: &mut DiscoveryWorkingDraft,
) -> CoreResult<()> {
    if draft.assistant.is_some() {
        restored_assistant(draft)?;
        return Ok(());
    }
    let assistant_route_id = snapshot
        .session
        .input
        .preferred_assistant
        .clone()
        .ok_or_else(|| CoreError::invalid("provider setup assistant route was not selected"))?;
    let wanted_ids = draft
        .evidence_ids
        .iter()
        .chain(&draft.extra_evidence_ids)
        .cloned()
        .collect::<BTreeSet<_>>();
    if wanted_ids.is_empty() {
        return Err(CoreError::invalid(
            "provider setup assistant requires redacted evidence",
        ));
    }
    let records = storage.list_discovery_evidence(&snapshot.session.id, MAX_DISCOVERY_ROWS)?;
    let evidence = records
        .into_iter()
        .filter(|record| wanted_ids.contains(&record.id))
        .map(|record| {
            let claims = draft
                .assistant_evidence_claims
                .get(&record.id)
                .cloned()
                .unwrap_or_default();
            redacted_assistant_evidence(record, claims)
        })
        .collect::<CoreResult<Vec<_>>>()?;
    if evidence.len() != wanted_ids.len() {
        return Err(CoreError::invalid(
            "provider setup assistant evidence is incomplete",
        ));
    }
    let mut allowed_api_families = draft
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
    if allowed_api_families.is_empty() {
        allowed_api_families = AdapterRegistry::built_in_templates()?
            .into_iter()
            .map(|template| template.api_family)
            .collect();
    }
    let mut engine = SetupAssistantEngine::new(
        snapshot.session.id.clone(),
        assistant_route_id,
        allowed_api_families,
        evidence,
        AssistantBudget::default(),
    )
    .map_err(assistant_error)?;
    if !draft.assistant_more_evidence_questions.is_empty() {
        let durable_questions = draft.assistant_more_evidence_questions.clone();
        engine
            .replace_unresolved_questions_before_consent(durable_questions.clone())
            .map_err(assistant_error)?;
        if engine.unresolved_questions() != durable_questions {
            return Err(corrupted_assistant_resume_boundary());
        }
    }
    synchronize_assistant_snapshot(draft, &engine);
    draft.assistant_approval_binding = None;
    Ok(())
}

pub(super) fn redacted_assistant_evidence(
    record: DiscoveryEvidenceRecord,
    claims: Vec<EvidenceClaim>,
) -> CoreResult<RedactedAssistantEvidence> {
    let kind = match record.kind {
        DiscoveryEvidenceKind::OpenApi | DiscoveryEvidenceKind::JsonSchema => {
            AssistantEvidenceKind::ApiSpecification
        }
        DiscoveryEvidenceKind::JsonDocument => AssistantEvidenceKind::DeterministicExtraction,
        DiscoveryEvidenceKind::HtmlDocument
        | DiscoveryEvidenceKind::YamlDocument
        | DiscoveryEvidenceKind::XmlDocument
        | DiscoveryEvidenceKind::PlainTextDocument => AssistantEvidenceKind::OfficialDocument,
    };
    let excerpt_value = assistant_evidence_excerpt_value(&record.extracted_json);
    let excerpt = bounded_utf8_prefix(
        &serde_json::to_string(&excerpt_value)
            .map_err(|_| CoreError::internal("redacted assistant evidence could not be encoded"))?,
        16 * 1024,
    );
    RedactedAssistantEvidence::new(
        record.id,
        kind,
        record.source_url.as_str(),
        record.content_sha256,
        excerpt,
        claims,
        1,
    )
    .map_err(assistant_error)
}

fn assistant_evidence_excerpt_value(value: &Value) -> Value {
    match value {
        Value::Array(values) => Value::Array(
            values
                .iter()
                .map(assistant_evidence_excerpt_value)
                .collect(),
        ),
        Value::Object(values) => Value::Object(
            values
                .iter()
                .filter(|(name, _)| {
                    !matches!(
                        name.as_str(),
                        "content_sha256"
                            | "manifest_sha256"
                            | "path_sha256"
                            | "source_path_sha256"
                            | "template_id"
                    )
                })
                .map(|(name, value)| (name.clone(), assistant_evidence_excerpt_value(value)))
                .collect(),
        ),
        _ => value.clone(),
    }
}

fn bounded_utf8_prefix(value: &str, max_bytes: usize) -> String {
    if value.len() <= max_bytes {
        return value.to_owned();
    }
    let mut boundary = max_bytes;
    while !value.is_char_boundary(boundary) {
        boundary -= 1;
    }
    value[..boundary].to_owned()
}
pub(super) fn record_deterministic_assistant_claims(
    snapshot: &DiscoverySessionSnapshot,
    output: &DeterministicDiscoveryOutput,
    draft: &mut DiscoveryWorkingDraft,
) -> CoreResult<()> {
    for (index, item) in output.evidence.iter().enumerate() {
        let evidence_id = EvidenceId::from(deterministic_id(
            &snapshot.session.id,
            0,
            &format!("evidence:{index}:{}", item.content_sha256),
        ));
        let claims = deterministic_assistant_claims(output, index)?;
        if !claims.is_empty() {
            draft.assistant_evidence_claims.insert(evidence_id, claims);
        }
    }
    Ok(())
}

fn deterministic_assistant_claims(
    output: &DeterministicDiscoveryOutput,
    evidence_index: usize,
) -> CoreResult<Vec<EvidenceClaim>> {
    let mut projected = BTreeMap::<DraftField, BTreeSet<String>>::new();
    for family in output
        .family_candidates
        .iter()
        .filter(|candidate| candidate.evidence_indices.contains(&evidence_index))
        .map(|candidate| candidate.api_family)
    {
        projected
            .entry(DraftField::ApiFamily)
            .or_default()
            .insert(api_family_slug(family).to_owned());
    }
    for candidate in output
        .manifest_candidates
        .iter()
        .filter(|candidate| candidate.evidence_indices.contains(&evidence_index))
    {
        let manifest = &candidate.template.default_manifest;
        projected
            .entry(DraftField::ApiFamily)
            .or_default()
            .insert(api_family_slug(manifest.api_family).to_owned());
        if let Some(origin) = &manifest.default_api_origin {
            projected
                .entry(DraftField::DefaultApiOrigin)
                .or_default()
                .insert(origin.as_str().to_owned());
        }
        if candidate.auth_evidenced {
            projected.entry(DraftField::Auth).or_default().insert(
                serde_json::to_string(&manifest.auth)
                    .map_err(|_| CoreError::internal("assistant auth claim encoding failed"))?,
            );
        }
        if candidate.generation_endpoint_evidenced {
            projected
                .entry(DraftField::GenerateEndpoint)
                .or_default()
                .insert(endpoint_claim(
                    manifest.endpoints.generate.method,
                    manifest.endpoints.generate.path.as_str(),
                ));
            projected
                .entry(DraftField::ResponseDecoder)
                .or_default()
                .insert(decoder_slug(manifest.decoders.response).to_owned());
        }
        if candidate.model_endpoint_evidenced
            && let Some(endpoint) = &manifest.endpoints.models
        {
            projected
                .entry(DraftField::ModelsEndpoint)
                .or_default()
                .insert(endpoint_claim(endpoint.method, endpoint.path.as_str()));
        }
        if deterministic_evidence_supports_streaming(&output.evidence[evidence_index])
            && let Some(decoder) = manifest.decoders.streaming
        {
            projected
                .entry(DraftField::StreamingDecoder)
                .or_default()
                .insert(decoder_slug(decoder).to_owned());
        }
    }
    projected
        .into_iter()
        .filter_map(|(field, values)| {
            (values.len() == 1).then(|| {
                EvidenceClaim::new(field, values.into_iter().next().expect("one value"))
                    .map_err(assistant_error)
            })
        })
        .collect()
}

fn deterministic_evidence_supports_streaming(
    evidence: &crate::provider_discovery_deterministic::RedactedDiscoveryEvidenceRecord,
) -> bool {
    [
        Some(&evidence.extracted_json),
        evidence.extracted_json.get("extracted"),
    ]
    .into_iter()
    .flatten()
    .any(|value| {
        value.get("stream_hint").and_then(Value::as_bool) == Some(true)
            || value
                .get("streaming_media_types")
                .and_then(Value::as_array)
                .is_some_and(|types| !types.is_empty())
    })
}

pub(super) fn endpoint_claim(method: HttpMethod, path: &str) -> String {
    let method = match method {
        HttpMethod::Get => "GET",
        HttpMethod::Post => "POST",
    };
    format!("{method} {path}")
}

pub(super) const fn decoder_slug(decoder: DecoderId) -> &'static str {
    match decoder {
        DecoderId::OpenAiJsonV1 => "open_ai_json_v1",
        DecoderId::OpenAiSseV1 => "open_ai_sse_v1",
        DecoderId::AnthropicJsonV1 => "anthropic_json_v1",
        DecoderId::AnthropicSseV1 => "anthropic_sse_v1",
        DecoderId::GeminiJsonV1 => "gemini_json_v1",
        DecoderId::GeminiSseV1 => "gemini_sse_v1",
        DecoderId::OllamaJsonV1 => "ollama_json_v1",
        DecoderId::OllamaJsonlV1 => "ollama_jsonl_v1",
    }
}
impl crate::app::Core {
    pub fn get_provider_discovery_assistant_resume_boundary(
        &self,
        session_id: &DiscoverySessionId,
    ) -> CoreResult<Option<ProviderDiscoveryAssistantResumeBoundary>> {
        self.provider_discovery()
            .assistant_resume_boundary(session_id)
    }
}
