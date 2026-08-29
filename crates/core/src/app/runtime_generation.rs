use std::{sync::Arc, time::Duration};

use chrono::Utc;
use lorepia_domain::{
    ConversationId, CoreError, CoreErrorCode, CoreResult, GenerationId,
    GenerationProviderProvenance, GenerationRequest, GenerationTarget, GenerationUsage, Message,
    MessageId, MessageRole, MessageStatus, ProviderConnectionId,
};
use lorepia_providers::{OpenAiCompatibleProvider, Provider};
use lorepia_storage::{
    RuntimeModelAuditFinish, RuntimeModelAuditStart, RuntimeModelAuditStatus,
    RuntimeModelCapability, Storage,
};
use tokio::sync::watch;

use super::{
    ConnectionBoundCredential, Core, MAX_RUNTIME_PROMPT_MESSAGES, MAX_TASK_PROMPT_BYTES,
    MAX_TASK_PROMPT_CHARS, RUNTIME_GENERATION_TIMEOUT_MS, TaskDispatchClassification,
    TaskExecutionOutcome, dispatch_auxiliary_task_provider,
    preflight_generation_target_connection_credential, resolve_generation_target,
};

pub(super) const RUNTIME_MAX_OUTPUT_TOKENS: u32 = 1_024;

/// ordinary chat path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimePromptMessage {
    pub role: MessageRole,
    pub content: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RuntimeGenerationCapability {
    Primary,
    Auxiliary,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimeGenerationAuditContext {
    pub request_id: String,
    pub character_id: String,
    pub character_content_revision_id: Option<String>,
    pub capability: RuntimeGenerationCapability,
    pub grant_sha256: String,
}

impl Core {
    /// Runs a one-shot generation requested by an imported character runtime
    /// against an exact catalog target.
    ///
    /// The runtime supplies only role/content messages. Provider selection,
    /// credential ownership, request compilation, output bounds, and timeout
    /// enforcement stay inside the native Core boundary.
    pub async fn generate_runtime_text_with_connection_credential(
        &self,
        target: &GenerationTarget,
        messages: &[RuntimePromptMessage],
        credential: ConnectionBoundCredential,
        cancelled: watch::Receiver<bool>,
        audit: RuntimeGenerationAuditContext,
    ) -> CoreResult<(String, GenerationUsage)> {
        preflight_generation_target_connection_credential(self, target, &credential)?;
        let resolved = resolve_generation_target(self, target)?;
        let request = runtime_generation_request(
            resolved.model.clone(),
            validate_runtime_prompt_messages(messages)?,
            Some(
                resolved
                    .prompt_wire_contract
                    .configured_max_output_tokens
                    .unwrap_or(RUNTIME_MAX_OUTPUT_TOKENS)
                    .min(RUNTIME_MAX_OUTPUT_TOKENS),
            ),
            Some(GenerationProviderProvenance {
                api_family: resolved.api_family,
                model_route_id: target.model_route_id.clone(),
                generation_preset_id: target.generation_preset_id.clone(),
            }),
        );
        resolved.provider.snapshot_request(&request)?;
        self.inner
            .storage
            .start_runtime_model_audit(&RuntimeModelAuditStart {
                request_id: audit.request_id.clone(),
                character_id: audit.character_id,
                character_content_revision_id: audit.character_content_revision_id,
                capability: runtime_model_capability(audit.capability),
                grant_sha256: audit.grant_sha256,
                provider_connection_id: resolved.connection_id.as_str().to_owned(),
                model_route_id: Some(target.model_route_id.0.clone()),
                generation_preset_id: Some(target.generation_preset_id.0.clone()),
                started_at: Utc::now(),
            })?;
        let outcome = dispatch_auxiliary_task_provider(
            resolved.provider,
            request,
            credential,
            RUNTIME_GENERATION_TIMEOUT_MS,
            cancelled,
        )
        .await;
        finish_runtime_model_audit(&self.inner.storage, &audit.request_id, &outcome);
        runtime_generation_result(outcome)
    }

    /// Runs a one-shot imported-runtime generation through a legacy provider
    /// profile retained for workspace migration compatibility.
    pub async fn generate_runtime_text_with_provider_profile(
        &self,
        provider_profile_id: &str,
        messages: &[RuntimePromptMessage],
        credential: Option<String>,
        cancelled: watch::Receiver<bool>,
        audit: RuntimeGenerationAuditContext,
    ) -> CoreResult<(String, GenerationUsage)> {
        let profile = self
            .inner
            .storage
            .get_provider_profile(provider_profile_id)?;
        let timeout = Duration::from_secs(u64::from(profile.timeout_seconds.max(1)));
        let provider: Arc<dyn Provider> =
            Arc::new(OpenAiCompatibleProvider::new(&profile.base_url, timeout)?);
        let request = runtime_generation_request(
            profile.model,
            validate_runtime_prompt_messages(messages)?,
            Some(RUNTIME_MAX_OUTPUT_TOKENS),
            None,
        );
        provider.snapshot_request(&request)?;
        self.inner
            .storage
            .start_runtime_model_audit(&RuntimeModelAuditStart {
                request_id: audit.request_id.clone(),
                character_id: audit.character_id,
                character_content_revision_id: audit.character_content_revision_id,
                capability: runtime_model_capability(audit.capability),
                grant_sha256: audit.grant_sha256,
                provider_connection_id: provider_profile_id.to_owned(),
                model_route_id: None,
                generation_preset_id: None,
                started_at: Utc::now(),
            })?;
        let outcome = dispatch_auxiliary_task_provider(
            provider,
            request,
            ConnectionBoundCredential::new(
                ProviderConnectionId::from(provider_profile_id.to_owned()),
                credential,
            ),
            u64::from(profile.timeout_seconds.max(1)).saturating_mul(1_000),
            cancelled,
        )
        .await;
        finish_runtime_model_audit(&self.inner.storage, &audit.request_id, &outcome);
        runtime_generation_result(outcome)
    }
}

fn validate_runtime_prompt_messages(
    messages: &[RuntimePromptMessage],
) -> CoreResult<Vec<RuntimePromptMessage>> {
    if messages.is_empty() || messages.len() > MAX_RUNTIME_PROMPT_MESSAGES {
        return Err(CoreError::invalid(
            "runtime generation prompt must contain between 1 and 128 messages",
        ));
    }
    let mut total_bytes = 0_usize;
    let mut total_chars = 0_usize;
    for message in messages {
        if message.content.trim().is_empty() || message.content.contains('\0') {
            return Err(CoreError::invalid(
                "runtime generation messages must contain non-empty text",
            ));
        }
        total_bytes = total_bytes
            .checked_add(message.content.len())
            .ok_or_else(|| CoreError::invalid("runtime generation prompt size overflowed"))?;
        total_chars = total_chars
            .checked_add(message.content.chars().count())
            .ok_or_else(|| CoreError::invalid("runtime generation prompt size overflowed"))?;
    }
    if total_bytes > MAX_TASK_PROMPT_BYTES || total_chars > MAX_TASK_PROMPT_CHARS {
        return Err(CoreError::invalid(
            "runtime generation prompt exceeds its size limit",
        ));
    }
    Ok(messages.to_vec())
}

pub(super) fn runtime_generation_request(
    model: String,
    messages: Vec<RuntimePromptMessage>,
    max_output_tokens: Option<u32>,
    provider_provenance: Option<GenerationProviderProvenance>,
) -> GenerationRequest {
    let conversation_id = ConversationId::new();
    let created_at = Utc::now();
    let mut parent_id = None;
    let messages = messages
        .into_iter()
        .map(|runtime_message| {
            let id = MessageId::new();
            let message = Message {
                id: id.clone(),
                conversation_id: conversation_id.clone(),
                parent_id: parent_id.clone(),
                role: runtime_message.role,
                content: runtime_message.content,
                status: MessageStatus::Complete,
                generation_id: None,
                created_at,
            };
            parent_id = Some(id);
            message
        })
        .collect();
    GenerationRequest {
        generation_id: GenerationId::new(),
        conversation_id,
        model,
        messages,
        resolved_prompt_plan: None,
        provider_execution_plan_hash: None,
        temperature: None,
        max_output_tokens: max_output_tokens.map(|value| value.min(RUNTIME_MAX_OUTPUT_TOKENS)),
        provider_provenance,
        preserve_opaque_reasoning_state: false,
        opaque_reasoning_context: Vec::new(),
    }
}

pub(super) fn runtime_generation_result(
    outcome: TaskExecutionOutcome,
) -> CoreResult<(String, GenerationUsage)> {
    match outcome {
        TaskExecutionOutcome::Completed {
            canonical_text,
            usage,
        } => Ok((canonical_text, usage)),
        TaskExecutionOutcome::Failed {
            classification: TaskDispatchClassification::UnknownOutcome,
            error,
        } => Err(CoreError::new(
            CoreErrorCode::Internal,
            format!(
                "runtime model provider outcome is unknown after dispatch ({})",
                error.code.as_str()
            ),
            false,
        )),
        TaskExecutionOutcome::Failed { error, .. } => Err(error),
    }
}

fn runtime_model_capability(value: RuntimeGenerationCapability) -> RuntimeModelCapability {
    match value {
        RuntimeGenerationCapability::Primary => RuntimeModelCapability::Primary,
        RuntimeGenerationCapability::Auxiliary => RuntimeModelCapability::Auxiliary,
    }
}

fn finish_runtime_model_audit(storage: &Storage, request_id: &str, outcome: &TaskExecutionOutcome) {
    let (status, usage, failure_code) = match outcome {
        TaskExecutionOutcome::Completed { usage, .. } => {
            (RuntimeModelAuditStatus::Succeeded, Some(usage), None)
        }
        TaskExecutionOutcome::Failed {
            classification,
            error,
        } => (
            if *classification == TaskDispatchClassification::UnknownOutcome {
                RuntimeModelAuditStatus::UnknownOutcome
            } else if error.code == CoreErrorCode::Cancelled {
                RuntimeModelAuditStatus::Cancelled
            } else {
                RuntimeModelAuditStatus::Failed
            },
            None,
            Some(error.code.as_str()),
        ),
    };
    // The call outcome is already known. Do not turn a terminal audit-write
    // failure into a retryable generation error that could duplicate cost;
    // startup recovery retains the incomplete row as `interrupted` evidence.
    let _ = storage.finish_runtime_model_audit(RuntimeModelAuditFinish {
        request_id,
        status,
        usage,
        failure_code,
        completed_at: Utc::now(),
    });
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use async_trait::async_trait;
    use lorepia_domain::ProviderCapabilities;
    use lorepia_providers::ProviderEventSender;
    use tokio::sync::watch;

    use super::*;

    struct CancellationTeardownBarrierProvider {
        entered: Mutex<Option<tokio::sync::oneshot::Sender<()>>>,
        cancellation_seen: Mutex<Option<tokio::sync::oneshot::Sender<()>>>,
        release: Mutex<Option<tokio::sync::oneshot::Receiver<()>>>,
    }

    #[async_trait]
    impl Provider for CancellationTeardownBarrierProvider {
        fn capabilities(&self) -> ProviderCapabilities {
            ProviderCapabilities {
                streaming: true,
                reasoning: false,
                max_context_tokens: None,
            }
        }

        async fn generate(
            &self,
            _request: GenerationRequest,
            _credential: Option<&str>,
            _sink: ProviderEventSender,
            mut cancelled: watch::Receiver<bool>,
        ) -> CoreResult<GenerationUsage> {
            if let Some(entered) = self.entered.lock().expect("teardown entered lock").take() {
                let _ = entered.send(());
            }
            while !*cancelled.borrow() {
                cancelled
                    .changed()
                    .await
                    .map_err(|_| CoreError::internal("teardown cancellation sender dropped"))?;
            }
            if let Some(cancellation_seen) = self
                .cancellation_seen
                .lock()
                .expect("teardown cancellation lock")
                .take()
            {
                let _ = cancellation_seen.send(());
            }
            let release = self
                .release
                .lock()
                .expect("teardown release lock")
                .take()
                .expect("teardown release receiver");
            release
                .await
                .map_err(|_| CoreError::internal("teardown release sender dropped"))?;
            Err(CoreError::new(
                CoreErrorCode::Cancelled,
                "cooperative provider completed cancellation teardown",
                false,
            ))
        }
    }

    #[tokio::test]
    async fn cancelled_dispatch_waits_for_cooperative_provider_teardown_within_grace() {
        let (entered_sender, entered_receiver) = tokio::sync::oneshot::channel();
        let (cancellation_seen_sender, cancellation_seen_receiver) =
            tokio::sync::oneshot::channel();
        let (release_sender, release_receiver) = tokio::sync::oneshot::channel();
        let provider = Arc::new(CancellationTeardownBarrierProvider {
            entered: Mutex::new(Some(entered_sender)),
            cancellation_seen: Mutex::new(Some(cancellation_seen_sender)),
            release: Mutex::new(Some(release_receiver)),
        });
        let request = runtime_generation_request(
            "cancellation-teardown-barrier-model".to_owned(),
            vec![RuntimePromptMessage {
                role: MessageRole::User,
                content: "bounded prompt".to_owned(),
            }],
            Some(RUNTIME_MAX_OUTPUT_TOKENS),
            None,
        );
        let (cancel_sender, cancelled) = watch::channel(false);
        let dispatch = tokio::spawn(dispatch_auxiliary_task_provider(
            provider,
            request,
            ConnectionBoundCredential::new(
                ProviderConnectionId::from("cancellation-teardown-barrier-connection"),
                None,
            ),
            5_000,
            cancelled,
        ));

        entered_receiver.await.expect("provider dispatch entered");
        cancel_sender.send(true).expect("cancel provider dispatch");
        cancellation_seen_receiver
            .await
            .expect("provider observes cancellation signal");
        assert!(
            !dispatch.is_finished(),
            "native dispatch must await provider teardown before becoming terminal"
        );
        release_sender
            .send(())
            .expect("release provider cancellation teardown");
        let outcome = dispatch.await.expect("join cancelled provider dispatch");
        assert!(matches!(
            outcome,
            TaskExecutionOutcome::Failed {
                classification: TaskDispatchClassification::UnknownOutcome,
                error: CoreError {
                    code: CoreErrorCode::Cancelled,
                    ..
                },
            }
        ));
    }
}
