use std::{sync::Arc, time::Duration};

use chrono::Utc;
use lorepia_domain::{
    ConversationId, CoreError, CoreErrorCode, CoreResult, GenerationId,
    GenerationProviderProvenance, GenerationRequest, GenerationTarget, GenerationUsage, Message,
    MessageId, MessageRole, MessageStatus, TaskProfile,
};
use lorepia_providers::{Provider, ProviderEvent};
use lorepia_storage::StoredRevision;
use tokio::{
    sync::{mpsc, watch},
    time,
};
use zeroize::Zeroize;

use super::{
    BoundedTaskPrompt, ConnectionBoundCredential, ResolvedGenerationTarget,
    validate_connection_credential_binding, validate_generation_target_plan,
};
use crate::app::{
    AUXILIARY_PROVIDER_TEARDOWN_GRACE, Core, MAX_TASK_OUTPUT_BYTES, MAX_TASK_OUTPUT_CHARS,
};

/// Dispatch certainty for one auxiliary provider attempt.
///
/// Runtime fallback is allowed only for `BeforeDispatch` and
/// `KnownNoSideEffect`. A timeout, cancellation, or ambiguous transport error
/// after the provider future starts is always `UnknownOutcome`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TaskDispatchClassification {
    BeforeDispatch,
    KnownNoSideEffect,
    UnknownOutcome,
    ProviderRejected,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum TaskExecutionOutcome {
    Completed {
        canonical_text: String,
        usage: GenerationUsage,
    },
    Failed {
        classification: TaskDispatchClassification,
        error: CoreError,
    },
}

impl Core {
    /// Executes one exact auxiliary-task target without exposing its prompt,
    /// request body, credential, or provider events across the Rust boundary.
    ///
    /// The caller owns fallback policy. In particular, it must never retry or
    /// select a fallback after `UnknownOutcome` or `ProviderRejected`.
    pub(crate) async fn execute_task_profile_target(
        &self,
        task_profile: &StoredRevision<TaskProfile>,
        target: &GenerationTarget,
        resolved: ResolvedGenerationTarget,
        prompt: BoundedTaskPrompt,
        credential: ConnectionBoundCredential,
        cancelled: watch::Receiver<bool>,
    ) -> TaskExecutionOutcome {
        let before_dispatch = |error| TaskExecutionOutcome::Failed {
            classification: TaskDispatchClassification::BeforeDispatch,
            error,
        };
        if let Err(error) =
            self.validate_task_profile_dispatch(task_profile, target, &resolved, &credential)
        {
            return before_dispatch(error);
        }
        if *cancelled.borrow() {
            return TaskExecutionOutcome::Failed {
                classification: TaskDispatchClassification::KnownNoSideEffect,
                error: CoreError::new(
                    CoreErrorCode::Cancelled,
                    "auxiliary task was cancelled before provider dispatch",
                    true,
                ),
            };
        }

        let request = auxiliary_task_generation_request(target, &resolved, prompt);
        if let Err(error) = resolved.provider.snapshot_request(&request) {
            return before_dispatch(error);
        }
        dispatch_auxiliary_task_provider(
            Arc::clone(&resolved.provider),
            request,
            credential,
            task_profile.value.timeout_ms,
            cancelled,
        )
        .await
    }

    fn validate_task_profile_dispatch(
        &self,
        task_profile: &StoredRevision<TaskProfile>,
        target: &GenerationTarget,
        resolved: &ResolvedGenerationTarget,
        credential: &ConnectionBoundCredential,
    ) -> CoreResult<()> {
        let current_profile = self.storage().get_task_profile(&task_profile.value.id)?;
        if current_profile.revision != task_profile.revision
            || current_profile.revision_id != task_profile.revision_id
            || current_profile.value != task_profile.value
            || current_profile.deleted_at.is_some()
            || task_profile.revision_id.is_none()
        {
            return Err(CoreError::invalid(
                "auxiliary task profile changed before provider dispatch",
            ));
        }
        let target_plan = self.resolve_task_generation_targets(&task_profile.value.id)?;
        if !target_plan
            .targets
            .iter()
            .any(|candidate| candidate == target)
        {
            return Err(CoreError::invalid(
                "auxiliary task target is not part of the immutable task profile",
            ));
        }
        let current_target = validate_generation_target_plan(self, target)?;
        if current_target.connection.id != resolved.connection_id
            || current_target.route.model_id != resolved.model
            || current_target.prompt_wire_contract != resolved.prompt_wire_contract
        {
            return Err(CoreError::invalid(
                "auxiliary generation target changed before provider dispatch",
            ));
        }
        validate_connection_credential_binding(&current_target.connection, credential)
    }
}

fn auxiliary_task_generation_request(
    target: &GenerationTarget,
    resolved: &ResolvedGenerationTarget,
    prompt: BoundedTaskPrompt,
) -> GenerationRequest {
    let conversation_id = ConversationId::new();
    let created_at = Utc::now();
    let system_message = Message {
        id: MessageId::new(),
        conversation_id: conversation_id.clone(),
        parent_id: None,
        role: MessageRole::System,
        content: prompt.system_instruction,
        status: MessageStatus::Complete,
        generation_id: None,
        created_at,
    };
    let user_message = Message {
        id: MessageId::new(),
        conversation_id: conversation_id.clone(),
        parent_id: Some(system_message.id.clone()),
        role: MessageRole::User,
        content: prompt.input,
        status: MessageStatus::Complete,
        generation_id: None,
        created_at,
    };
    GenerationRequest {
        generation_id: GenerationId::new(),
        conversation_id,
        model: resolved.model.clone(),
        messages: vec![system_message, user_message],
        resolved_prompt_plan: None,
        provider_execution_plan_hash: None,
        temperature: None,
        max_output_tokens: resolved.prompt_wire_contract.configured_max_output_tokens,
        provider_provenance: Some(GenerationProviderProvenance {
            api_family: resolved.api_family,
            model_route_id: target.model_route_id.clone(),
            generation_preset_id: target.generation_preset_id.clone(),
        }),
        preserve_opaque_reasoning_state: false,
        opaque_reasoning_context: Vec::new(),
    }
}

pub(in crate::app) async fn dispatch_auxiliary_task_provider(
    provider: Arc<dyn Provider>,
    request: GenerationRequest,
    credential: ConnectionBoundCredential,
    timeout_ms: u64,
    mut cancelled: watch::Receiver<bool>,
) -> TaskExecutionOutcome {
    if *cancelled.borrow() {
        return TaskExecutionOutcome::Failed {
            classification: TaskDispatchClassification::KnownNoSideEffect,
            error: CoreError::new(
                CoreErrorCode::Cancelled,
                "auxiliary task was cancelled before provider dispatch",
                true,
            ),
        };
    }
    let (event_sender, event_receiver) = mpsc::channel(128);
    let (attempt_cancel_sender, attempt_cancel_receiver) = watch::channel(false);
    let provider_attempt = async {
        tokio::join!(
            provider.generate(
                request,
                credential.value.as_deref(),
                event_sender,
                attempt_cancel_receiver,
            ),
            collect_task_provider_events(event_receiver),
        )
    };
    tokio::pin!(provider_attempt);
    let timeout = time::sleep(Duration::from_millis(timeout_ms));
    tokio::pin!(timeout);
    let cancellation = async {
        loop {
            if *cancelled.borrow() {
                break;
            }
            if cancelled.changed().await.is_err() {
                std::future::pending::<()>().await;
            }
        }
    };
    tokio::pin!(cancellation);

    let (provider_result, output_result) = tokio::select! {
        result = &mut provider_attempt => result,
        () = &mut cancellation => {
            let _ = attempt_cancel_sender.send(true);
            // Built-in adapters tear down in-flight transport on this signal; briefly await
            // local confirmation before dropping futures. The remote outcome remains unknown.
            let _ = time::timeout(
                AUXILIARY_PROVIDER_TEARDOWN_GRACE,
                &mut provider_attempt,
            )
            .await;
            return unknown_task_outcome("auxiliary task was cancelled after provider dispatch began");
        }
        () = &mut timeout => {
            let _ = attempt_cancel_sender.send(true);
            // Apply the same bounded local teardown handshake on timeout. A
            // provider which ignores cancellation still has its local attempt
            // force-dropped when this grace period expires.
            let _ = time::timeout(
                AUXILIARY_PROVIDER_TEARDOWN_GRACE,
                &mut provider_attempt,
            )
            .await;
            return unknown_task_outcome("auxiliary task timed out after provider dispatch began");
        }
    };
    classify_task_provider_result(provider_result, output_result)
}

pub(in crate::app) fn unknown_task_outcome(message: &'static str) -> TaskExecutionOutcome {
    TaskExecutionOutcome::Failed {
        classification: TaskDispatchClassification::UnknownOutcome,
        error: CoreError::new(CoreErrorCode::Cancelled, message, false),
    }
}

fn classify_task_provider_result(
    provider_result: CoreResult<GenerationUsage>,
    output_result: CoreResult<String>,
) -> TaskExecutionOutcome {
    match (provider_result, output_result) {
        (Ok(usage), Ok(canonical_text)) if !canonical_text.trim().is_empty() => {
            TaskExecutionOutcome::Completed {
                canonical_text,
                usage,
            }
        }
        (Ok(_), Ok(mut canonical_text)) => {
            canonical_text.zeroize();
            TaskExecutionOutcome::Failed {
                classification: TaskDispatchClassification::ProviderRejected,
                error: CoreError::new(
                    CoreErrorCode::UnsupportedContent,
                    "auxiliary provider returned no canonical text",
                    false,
                ),
            }
        }
        (Ok(_), Err(error)) => TaskExecutionOutcome::Failed {
            classification: TaskDispatchClassification::ProviderRejected,
            error,
        },
        (Err(error), output_result) => {
            if let Ok(mut output) = output_result {
                output.zeroize();
            }
            TaskExecutionOutcome::Failed {
                classification: task_provider_error_classification(error.code),
                error,
            }
        }
    }
}

fn task_provider_error_classification(code: CoreErrorCode) -> TaskDispatchClassification {
    match code {
        CoreErrorCode::InvalidInput
        | CoreErrorCode::UnsupportedContent
        | CoreErrorCode::NotFound
        | CoreErrorCode::PermissionDenied
        | CoreErrorCode::ProviderAuthFailed
        | CoreErrorCode::ProviderRateLimited => TaskDispatchClassification::ProviderRejected,
        CoreErrorCode::UnsafeArchive
        | CoreErrorCode::StorageUnavailable
        | CoreErrorCode::StorageCorrupted
        | CoreErrorCode::ProviderUnavailable
        | CoreErrorCode::NetworkUnavailable
        | CoreErrorCode::Cancelled
        | CoreErrorCode::Internal => TaskDispatchClassification::UnknownOutcome,
    }
}

async fn collect_task_provider_events(
    mut receiver: mpsc::Receiver<ProviderEvent>,
) -> CoreResult<String> {
    let mut output = String::new();
    let mut rejected = None;
    while let Some(event) = receiver.recv().await {
        match event {
            ProviderEvent::TextDelta(mut delta) => {
                if rejected.is_some() {
                    delta.zeroize();
                    continue;
                }
                let next_bytes = output.len().checked_add(delta.len());
                let next_chars = output.chars().count().checked_add(delta.chars().count());
                if next_bytes.is_none_or(|bytes| bytes > MAX_TASK_OUTPUT_BYTES)
                    || next_chars.is_none_or(|chars| chars > MAX_TASK_OUTPUT_CHARS)
                {
                    output.zeroize();
                    delta.zeroize();
                    rejected = Some(CoreError::new(
                        CoreErrorCode::UnsupportedContent,
                        "auxiliary provider output exceeded its size limit",
                        false,
                    ));
                } else {
                    output.push_str(&delta);
                    delta.zeroize();
                }
            }
            ProviderEvent::ReasoningDelta(mut reasoning) => reasoning.zeroize(),
            ProviderEvent::OpaqueReasoningState(mut state) => {
                state.zeroize_sensitive_payloads();
                rejected.get_or_insert_with(|| {
                    CoreError::new(
                        CoreErrorCode::UnsupportedContent,
                        "auxiliary provider returned unsupported opaque reasoning state",
                        false,
                    )
                });
            }
            ProviderEvent::ToolCallStarted { .. }
            | ProviderEvent::ToolCallArgumentsDelta { .. }
            | ProviderEvent::ToolCallCompleted { .. } => {
                rejected.get_or_insert_with(|| {
                    CoreError::new(
                        CoreErrorCode::UnsupportedContent,
                        "auxiliary provider returned an unsupported tool call",
                        false,
                    )
                });
            }
        }
    }
    if let Some(error) = rejected {
        output.zeroize();
        Err(error)
    } else {
        Ok(output)
    }
}
