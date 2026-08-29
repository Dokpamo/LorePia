use super::{
    ActiveGenerationGuard, Arc, ChatEvent, ChatEventKind, CoreError, CoreErrorCode, CoreResult,
    Digest, GENERATION_PERSISTENCE_FAILURE_MESSAGE, GenerationCompletionContext,
    GenerationEventForwardingContext, GenerationFailure, GenerationOutcome, GenerationStatus,
    GenerationTask, GenerationTransformContext, Message, MessageDisplayProjectionWrite, MessageId,
    MessageStatus, MessageTransformApplicationWrite, MessageTransformDisposition,
    MessageTransformPipelineFailureWrite, MessageTransformStage, MissedTickBehavior,
    OpaqueReasoningState, PARTIAL_CHECKPOINT_BYTES, PARTIAL_CHECKPOINT_INTERVAL, Sha256,
    Sha256Digest, TerminalPersistenceContext, TransformPhase, mpsc, run_generation, time,
};

pub(super) async fn execute_generation_task(task: GenerationTask) {
    let GenerationTask {
        storage,
        active_generations,
        event_bus,
        branch_id,
        request,
        assistant,
        provider,
        credential,
        cancel_receiver,
        preserve_partial,
        transforms,
    } = task;
    let generation_id = request.generation_id.clone();
    let _active_generation = ActiveGenerationGuard {
        generation_id: generation_id.clone(),
        active_generations: Arc::clone(&active_generations),
    };
    let conversation_id = request.conversation_id.clone();
    let assistant_message_id = assistant.id.clone();
    let defer_text_events = generation_has_output_transforms(&transforms);
    let (event_sender, event_receiver) = mpsc::channel(128);
    let forward_events = tokio::spawn(forward_generation_events(
        event_receiver,
        GenerationEventForwardingContext {
            active_generations: Arc::clone(&active_generations),
            event_bus: event_bus.clone(),
            storage: Arc::clone(&storage),
            checkpoint: assistant.clone(),
            branch_id: branch_id.clone(),
            assistant_message_id: assistant_message_id.clone(),
            preserve_partial,
            defer_text_events,
        },
    ));
    let generation_result = run_generation(
        provider.as_ref(),
        request,
        credential.as_deref(),
        event_sender,
        cancel_receiver,
    )
    .await;
    drop(credential);
    drop(provider);
    let forwarding_result = forward_events
        .await
        .map_err(|error| {
            CoreError::internal(format!(
                "generation event forwarder stopped unexpectedly: {error}"
            ))
        })
        .and_then(std::convert::identity);
    let result = merge_generation_and_forwarding_results(generation_result, forwarding_result);
    finish_generation_task(
        GenerationCompletionContext {
            storage,
            active_generations,
            event_bus,
            branch_id,
            conversation_id,
            generation_id,
            assistant_message_id,
            preserve_partial,
            transforms,
        },
        assistant,
        result,
    );
}

fn finish_generation_task(
    context: GenerationCompletionContext,
    mut assistant: Message,
    result: Result<GenerationOutcome, GenerationFailure>,
) {
    let GenerationCompletionContext {
        storage,
        active_generations,
        event_bus,
        branch_id,
        conversation_id,
        generation_id,
        assistant_message_id,
        preserve_partial,
        transforms,
    } = context;
    let (result, display_projection) = apply_generation_output_transforms(result, &transforms);
    let usage = result.as_ref().ok().map(|outcome| outcome.usage.clone());
    let opaque_reasoning_state = result
        .as_ref()
        .ok()
        .map(|outcome| outcome.opaque_reasoning_state.clone())
        .unwrap_or_default();
    let error_code = result
        .as_ref()
        .err()
        .map(|failure| failure.error.code.as_str().to_owned());

    let (mut sequence, terminal_kind, should_commit) =
        apply_generation_result(&mut assistant, result, preserve_partial);
    let (terminal_kind, committed, projection_committed) = persist_generation_terminal(
        TerminalPersistenceContext {
            storage: &storage,
            generation_id: &generation_id,
        },
        &mut assistant,
        usage.as_ref(),
        &opaque_reasoning_state,
        error_code.as_deref(),
        should_commit,
        display_projection.as_ref(),
        terminal_kind,
    );
    let deferred_display_text = display_projection.as_ref().and_then(|projection| {
        committed.then(|| {
            if projection_committed {
                projection.display_content.clone()
            } else {
                assistant.content.clone()
            }
        })
    });
    if let Some(display_text) = deferred_display_text.filter(|text| !text.is_empty()) {
        let _ = active_generations.publish(
            &event_bus,
            ChatEvent::new(
                generation_id.clone(),
                conversation_id.clone(),
                sequence,
                ChatEventKind::TextDelta(display_text),
            )
            .with_route(branch_id.clone(), assistant_message_id.clone()),
        );
        sequence = sequence.saturating_add(1);
    }
    if committed {
        let _ = active_generations.publish(
            &event_bus,
            ChatEvent::new(
                generation_id.clone(),
                conversation_id.clone(),
                sequence,
                ChatEventKind::MessageCommitted {
                    message_id: assistant.id.clone(),
                    status: assistant.status,
                },
            )
            .with_route(branch_id.clone(), assistant_message_id.clone()),
        );
        sequence = sequence.saturating_add(1);
    }
    let _ = active_generations.publish(
        &event_bus,
        ChatEvent::new(
            generation_id.clone(),
            conversation_id,
            sequence,
            terminal_kind,
        )
        .with_route(branch_id, assistant_message_id),
    );
}

#[allow(
    clippy::too_many_arguments,
    reason = "terminal persistence keeps the complete transaction and compensation inputs explicit"
)]
fn persist_generation_terminal(
    context: TerminalPersistenceContext<'_>,
    assistant: &mut Message,
    usage: Option<&lorepia_domain::GenerationUsage>,
    opaque_reasoning_state: &[OpaqueReasoningState],
    error_code: Option<&str>,
    should_commit: bool,
    display_projection: Option<&MessageDisplayProjectionWrite>,
    mut terminal_kind: ChatEventKind,
) -> (ChatEventKind, bool, bool) {
    let original_status = assistant.status;
    let display_projection = should_commit.then_some(display_projection).flatten();
    let persistence = context
        .storage
        .finalize_generation_with_protocol_state_and_display(
            assistant,
            usage,
            opaque_reasoning_state,
            error_code,
            should_commit,
            display_projection,
        );
    let persistence_succeeded = persistence.is_ok();
    let committed = if persistence_succeeded {
        should_commit
    } else {
        assistant.status = MessageStatus::Failed;
        let compensation = context
            .storage
            .fail_generation_after_finalize_error(assistant, should_commit);
        if compensation.is_ok() {
            terminal_kind = generation_persistence_failure();
            should_commit
        } else if context
            .storage
            .get_generation(context.generation_id)
            .is_ok_and(|generation| {
                generation.status == generation_status_for_message(original_status)
            })
        {
            assistant.status = original_status;
            should_commit
        } else {
            terminal_kind = generation_persistence_failure();
            false
        }
    };
    let projection_committed = committed
        && display_projection.is_some_and(|expected| {
            persistence_succeeded
                || context
                    .storage
                    .get_message_display_projection(assistant)
                    .is_ok_and(|stored| {
                        stored.is_some_and(|stored| {
                            stored.display_content == expected.display_content
                        })
                    })
        });
    (terminal_kind, committed, projection_committed)
}

const fn generation_status_for_message(status: MessageStatus) -> GenerationStatus {
    match status {
        MessageStatus::Pending => GenerationStatus::Running,
        MessageStatus::Complete => GenerationStatus::Complete,
        MessageStatus::Cancelled => GenerationStatus::Cancelled,
        MessageStatus::Failed => GenerationStatus::Failed,
    }
}

fn generation_persistence_failure() -> ChatEventKind {
    ChatEventKind::GenerationFailed {
        code: CoreErrorCode::StorageUnavailable.as_str().to_owned(),
        message: GENERATION_PERSISTENCE_FAILURE_MESSAGE.to_owned(),
    }
}

async fn forward_generation_events(
    mut event_receiver: mpsc::Receiver<ChatEvent>,
    context: GenerationEventForwardingContext,
) -> CoreResult<()> {
    let GenerationEventForwardingContext {
        active_generations,
        event_bus,
        storage,
        mut checkpoint,
        branch_id,
        assistant_message_id,
        preserve_partial,
        defer_text_events,
    } = context;
    let start = time::Instant::now() + PARTIAL_CHECKPOINT_INTERVAL;
    let mut interval = time::interval_at(start, PARTIAL_CHECKPOINT_INTERVAL);
    interval.set_missed_tick_behavior(MissedTickBehavior::Delay);
    let mut last_checkpoint_bytes = 0;
    let mut dirty = false;

    loop {
        tokio::select! {
            event = event_receiver.recv() => {
                let Some(event) = event else {
                    if preserve_partial && dirty {
                        storage.checkpoint_pending_assistant(&checkpoint)?;
                    }
                    return Ok(());
                };
                let is_text_delta = matches!(&event.kind, ChatEventKind::TextDelta(_));
                if !defer_text_events {
                    if preserve_partial
                        && let ChatEventKind::TextDelta(delta) = &event.kind
                    {
                        checkpoint.content.push_str(delta);
                        dirty = true;
                    }
                    active_generations.publish(
                        &event_bus,
                        event.with_route(branch_id.clone(), assistant_message_id.clone())
                    )?;
                } else if !is_text_delta {
                    active_generations.publish(
                        &event_bus,
                        event.with_route(branch_id.clone(), assistant_message_id.clone())
                    )?;
                }
                if preserve_partial
                    && dirty
                    && partial_checkpoint_due(checkpoint.content.len(), last_checkpoint_bytes)
                {
                    storage.checkpoint_pending_assistant(&checkpoint)?;
                    last_checkpoint_bytes = checkpoint.content.len();
                    dirty = false;
                }
            }
            _ = interval.tick(), if preserve_partial => {
                if dirty {
                    storage.checkpoint_pending_assistant(&checkpoint)?;
                    last_checkpoint_bytes = checkpoint.content.len();
                    dirty = false;
                }
            }
        }
    }
}

pub(super) fn partial_checkpoint_due(current_bytes: usize, last_checkpoint_bytes: usize) -> bool {
    current_bytes.saturating_sub(last_checkpoint_bytes) >= PARTIAL_CHECKPOINT_BYTES
}

fn merge_generation_and_forwarding_results(
    generation: Result<GenerationOutcome, GenerationFailure>,
    forwarding: CoreResult<()>,
) -> Result<GenerationOutcome, GenerationFailure> {
    match (generation, forwarding) {
        (result, Ok(())) => result,
        (Ok(outcome), Err(error)) => Err(GenerationFailure {
            error,
            partial_text: outcome.text,
            last_sequence: outcome.last_sequence,
        }),
        (Err(mut failure), Err(error)) => {
            failure.error = error;
            Err(failure)
        }
    }
}

fn generation_has_output_transforms(context: &GenerationTransformContext) -> bool {
    context.sets.iter().any(|set| {
        set.enabled
            && set.rules.iter().any(|rule| {
                rule.enabled
                    && matches!(
                        rule.phase,
                        TransformPhase::ProviderOutputCanonical | TransformPhase::DisplayOnly
                    )
            })
    })
}

pub(super) fn apply_generation_output_transforms(
    mut result: Result<GenerationOutcome, GenerationFailure>,
    context: &GenerationTransformContext,
) -> (
    Result<GenerationOutcome, GenerationFailure>,
    Option<MessageDisplayProjectionWrite>,
) {
    if !generation_has_output_transforms(context) {
        return (result, None);
    }
    let text = match &result {
        Ok(outcome) => outcome.text.as_str(),
        Err(failure) => failure.partial_text.as_str(),
    };
    let canonical_phase = apply_generation_transform_phase(
        context,
        TransformPhase::ProviderOutputCanonical,
        text,
        MessageTransformStage::ProviderOutputCanonical,
    );
    let display_phase = apply_generation_transform_phase(
        context,
        TransformPhase::DisplayOnly,
        &canonical_phase.output,
        MessageTransformStage::DisplayOnly,
    );
    let canonical = canonical_phase.output;
    let display = context.display_context.as_ref().map_or_else(
        || display_phase.output.clone(),
        |base_context| {
            let mut display_context = base_context.clone();
            display_context
                .messages
                .push(lorepia_domain::PromptConversationMessage {
                    id: MessageId("portable-display-output".to_owned()),
                    branch_id: display_context.branch_id.clone(),
                    role: lorepia_domain::PromptMessageRole::Assistant,
                    content: canonical.clone(),
                    turn_index: u32::try_from(display_context.messages.len()).unwrap_or(u32::MAX),
                });
            lorepia_orchestration::render_portable_text(&display_phase.output, &display_context)
        },
    );
    match &mut result {
        Ok(outcome) => outcome.text.clone_from(&canonical),
        Err(failure) => failure.partial_text.clone_from(&canonical),
    }
    let mut applications = canonical_phase.applications;
    applications.extend(display_phase.applications);
    let pipeline_failures = canonical_phase
        .pipeline_failure
        .into_iter()
        .chain(display_phase.pipeline_failure)
        .collect();
    (
        result,
        Some(MessageDisplayProjectionWrite {
            display_content: display,
            applications,
            pipeline_failures,
        }),
    )
}

struct GenerationTransformPhaseResult {
    output: String,
    applications: Vec<MessageTransformApplicationWrite>,
    pipeline_failure: Option<MessageTransformPipelineFailureWrite>,
}

fn apply_generation_transform_phase(
    context: &GenerationTransformContext,
    phase: TransformPhase,
    input: &str,
    stage: MessageTransformStage,
) -> GenerationTransformPhaseResult {
    let transformed = crate::orchestration::apply_transform_sets_with_import_approvals(
        &context.sets,
        phase,
        input,
        &context.variables,
        &context.supported_capabilities,
        &context.approved_import_source_ids,
    );
    let Ok(transformed) = transformed else {
        return GenerationTransformPhaseResult {
            output: input.to_owned(),
            applications: Vec::new(),
            pipeline_failure: Some(MessageTransformPipelineFailureWrite {
                stage,
                code: "pipeline_invalid".to_owned(),
                before_sha256: transform_content_sha256(input),
            }),
        };
    };
    let mut diagnostic_invalid = false;
    let applications = transformed
        .reports
        .iter()
        .filter_map(|report| {
            let application = map_generation_transform_report(report, stage);
            diagnostic_invalid |= application.is_none();
            application
        })
        .collect::<Vec<_>>();
    let pipeline_failure =
        transformed
            .error
            .as_ref()
            .map(|error| MessageTransformPipelineFailureWrite {
                stage,
                code: error.code.as_str().to_owned(),
                before_sha256: transform_content_sha256(input),
            });
    GenerationTransformPhaseResult {
        output: transformed.output,
        applications: if diagnostic_invalid {
            Vec::new()
        } else {
            applications
        },
        pipeline_failure: pipeline_failure.or_else(|| {
            diagnostic_invalid.then(|| MessageTransformPipelineFailureWrite {
                stage,
                code: "diagnostic_invalid".to_owned(),
                before_sha256: transform_content_sha256(input),
            })
        }),
    }
}

fn map_generation_transform_report(
    report: &lorepia_orchestration::TransformRuleReport,
    stage: MessageTransformStage,
) -> Option<MessageTransformApplicationWrite> {
    let audit = report.execution_audit.as_ref()?;
    let before_sha256 = Sha256Digest::parse(audit.before_sha256.clone()).ok()?;
    let after_sha256 = audit
        .after_sha256
        .as_ref()
        .map(|value| Sha256Digest::parse(value.clone()))
        .transpose()
        .ok()?;
    let (disposition, code) = match report.status {
        lorepia_orchestration::TransformRuleStatus::Applied => {
            (MessageTransformDisposition::Applied, None)
        }
        lorepia_orchestration::TransformRuleStatus::NoMatch => {
            (MessageTransformDisposition::NoMatch, None)
        }
        lorepia_orchestration::TransformRuleStatus::Disabled => {
            (MessageTransformDisposition::Disabled, None)
        }
        lorepia_orchestration::TransformRuleStatus::PendingImportApproval => {
            (MessageTransformDisposition::PendingImportApproval, None)
        }
        lorepia_orchestration::TransformRuleStatus::ResolvedPromptDisabled => {
            (MessageTransformDisposition::ResolvedPromptDisabled, None)
        }
        lorepia_orchestration::TransformRuleStatus::ConditionFalse => {
            (MessageTransformDisposition::ConditionFalse, None)
        }
        lorepia_orchestration::TransformRuleStatus::Failed => {
            let failure_code = audit.failure_code?;
            let disposition = if matches!(
                failure_code,
                lorepia_orchestration::TransformFailureCode::InputLimitExceeded
                    | lorepia_orchestration::TransformFailureCode::OutputLimitExceeded
            ) {
                MessageTransformDisposition::LimitRejected
            } else {
                MessageTransformDisposition::Failed
            };
            (disposition, Some(failure_code.as_str().to_owned()))
        }
    };
    Some(MessageTransformApplicationWrite {
        set_id: audit.set_id.as_str().to_owned(),
        rule_id: report.trace.rule_id.as_str().to_owned(),
        stage,
        disposition,
        code,
        before_sha256,
        after_sha256,
        replacement_count: report.trace.replacements,
        input_chars: report.trace.input_chars,
        output_chars: report.trace.output_chars,
    })
}

pub(super) fn transform_content_sha256(value: &str) -> Sha256Digest {
    match Sha256Digest::parse(format!("{:x}", Sha256::digest(value.as_bytes()))) {
        Ok(digest) => digest,
        Err(error) => unreachable!("SHA-256 formatter produced an invalid digest: {error}"),
    }
}

pub(super) fn apply_generation_result(
    assistant: &mut Message,
    result: Result<GenerationOutcome, GenerationFailure>,
    preserve_partial: bool,
) -> (u64, ChatEventKind, bool) {
    match result {
        Ok(outcome) => {
            assistant.content = outcome.text;
            assistant.status = MessageStatus::Complete;
            (
                outcome.last_sequence.saturating_add(1),
                ChatEventKind::GenerationFinished,
                true,
            )
        }
        Err(failure) => {
            let cancelled = failure.error.code == CoreErrorCode::Cancelled;
            assistant.content = failure.partial_text;
            assistant.status = if cancelled {
                MessageStatus::Cancelled
            } else {
                MessageStatus::Failed
            };
            let terminal = if cancelled {
                ChatEventKind::GenerationCancelled
            } else {
                ChatEventKind::GenerationFailed {
                    code: failure.error.code.as_str().to_owned(),
                    message: failure.error.message,
                }
            };
            (
                failure.last_sequence.saturating_add(1),
                terminal,
                preserve_partial && !assistant.content.is_empty(),
            )
        }
    }
}
