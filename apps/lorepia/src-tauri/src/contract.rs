//! Canonical webview IPC contract.
//!
//! All host paths and credential values are ingress-only or remain in Rust.

use lorepia_shell_api::{
    ActivateContentModuleInput, AppSettingsDto, ApplyContentModuleRollbackInput,
    ApproveContentPackageImportReceiptDto, AssetDeliveryDto, CommitContentPackageImportReceiptDto,
    ContentModuleActivationPlanDto, ContentModuleActivationReceiptDto,
    ContentModuleActivationReviewDto, ContentModuleDeactivationReceiptDto,
    ContentModuleDeactivationReviewDto, ContentModuleDto, ContentModuleLifecycleBindingsDto,
    ContentModuleLifecycleCandidatesDto, ContentModuleRevisionDiffDto,
    ContentModuleRevisionListDto, ContentModuleRollbackPlanDto, ContentModuleRollbackReviewDto,
    ContentPackageImportSummaryDto, ContentPackageInspectionReviewDto, ContentPackageWorkspaceDto,
    ContentShareGateDto, ConversationModeDto, ConversationPersonaSelectionDto,
    CreatorPromptPresetDocumentDto, DeactivateContentModuleInput, ExpertPromptPreviewDto,
    ExplainPromptPlanInput, GenerationPresetDto, GenerationStartedDto, GenerationTargetDto,
    InteractionEffectDto, InteractionRuleSetDto, KnowledgeBookDto, KnowledgeSimulationDto,
    ListContentModuleLifecycleBindingsInput, ListContentModuleLifecycleCandidatesInput,
    ListRetryableGenerationAttemptsInput, ListRetryableMemoryQueryEmbeddingsInput,
    MemoryJobRetryReceiptDto, MemoryProfileDto, MemoryQueryEmbeddingRetryCandidateDto,
    MemoryRecordListDto, MemoryRecordProjectionDto, ModelRouteDto, ModuleBindingDto,
    PatchMemoryRecordInput, PersonaDeletionReceiptDto, PersonaDto, PersonaListPageDto,
    PromptPlanPreviewDto, PromptPresetBindingDto, PromptPresetSummaryDto, PromptResolutionTraceDto,
    ProviderConnectionDto, ProviderProfileDto, ProviderTemplateDto, ReorderPromptBlocksResultDto,
    ResolveContentModuleActivationInput, ResolveContentModuleRollbackInput,
    ResolvePromptPreviewInput, RetryInterruptedMemoryJobInput, RetryMemoryQueryEmbeddingInput,
    RetryableGenerationAttemptDto, ReviewContentModuleActivationInput,
    ReviewContentModuleDeactivationInput, ReviewContentModuleRollbackInput,
    ReviewedPromptSendInput, RevisionedDto, SelectContentPackageImportReceiptDto,
    SetMemoryRecordExclusionInput, TaskProfileDto, TransformSetDto,
};
use serde::{Deserialize, Serialize};
use tauri_plugin_lorepia_platform::{CredentialStatus, NativeCaptureStatus};

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ImportTicketDto {
    pub ticket_id: String,
    pub display_name: String,
    pub size_bytes: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TicketRequest {
    pub ticket_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InspectionRequest {
    pub inspection_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum DiscardImportRequest {
    Ticket { ticket_id: String },
    Inspection { inspection_id: String },
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CharacterRequest {
    pub character_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConversationRequest {
    pub conversation_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CharacterConversationsRequest {
    pub character_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BranchMessagesRequest {
    pub branch_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GenerationRequest {
    pub generation_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ChatStreamRequest {
    pub stream_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SubscribeGenerationRequest {
    pub generation_id: String,
    pub conversation_id: String,
    pub branch_id: String,
    pub sequence_baseline: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum CredentialTarget {
    LegacyProfile {
        provider_profile_id: String,
    },
    Connection {
        connection_id: String,
    },
    DiscoverySession {
        session_id: String,
        expected_revision: u64,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CredentialStatusRequest {
    pub target: CredentialTarget,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct CredentialStatusDto {
    pub status: CredentialStatus,
}

pub type NativeCaptureStatusDto = NativeCaptureStatus;

#[cfg(test)]
mod credential_target_contract_tests {
    use serde_json::json;

    use super::{CredentialStatusRequest, CredentialTarget};

    #[test]
    fn discovery_credential_target_accepts_only_session_and_revision() {
        let request = serde_json::from_value::<CredentialStatusRequest>(json!({
            "target": {
                "kind": "discovery_session",
                "session_id": "discovery-contract",
                "expected_revision": 7
            }
        }))
        .expect("decode renderer-safe discovery credential target");
        assert!(matches!(
            request.target,
            CredentialTarget::DiscoverySession {
                session_id,
                expected_revision: 7,
            } if session_id == "discovery-contract"
        ));

        for forbidden in ["commit_attempt_id", "commit_plan_sha256"] {
            let mut value = json!({
                "target": {
                    "kind": "discovery_session",
                    "session_id": "discovery-contract",
                    "expected_revision": 7
                }
            });
            value["target"][forbidden] = json!("renderer-must-not-authorize-install");
            serde_json::from_value::<CredentialStatusRequest>(value)
                .expect_err("renderer-authored durable authority must be rejected");
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum MemorySupervisorPhaseDto {
    NotStarted,
    Recovered,
    Running,
    Failed,
}

/// Bounded refresh signal for the Rust-only memory worker.
///
/// Queue payloads, job identifiers, credentials, provider state, and raw
/// errors are intentionally absent.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct MemorySupervisorStatusDto {
    pub sequence: u64,
    pub phase: MemorySupervisorPhaseDto,
    pub recovered_interrupted_jobs: u32,
    pub completed_jobs: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct InteractionEffectEventDto {
    pub delivery_id: String,
    pub effect_id: String,
    pub conversation_id: String,
    pub branch_id: String,
    pub resulting_state_revision: u64,
    pub event_created_at: String,
    pub effect: InteractionEffectDto,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InteractionEffectDeliveryRequest {
    pub delivery_id: String,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ProviderOverviewDto {
    pub settings: AppSettingsDto,
    pub templates: Vec<ProviderTemplateDto>,
    pub connections: Vec<ProviderConnectionDto>,
    pub legacy_profiles: Vec<ProviderProfileDto>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ModelRoutesRequest {
    pub connection_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GenerationPresetsRequest {
    pub model_route_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PreviewProviderRequest {
    pub target: GenerationTargetDto,
}

const _: fn() = || {
    fn assert_deserializable<T: for<'de> Deserialize<'de>>() {}
    fn assert_serializable<T: Serialize>() {}
    assert_deserializable::<ResolvePromptPreviewInput>();
    assert_deserializable::<ReviewedPromptSendInput>();
    assert_deserializable::<ExplainPromptPlanInput>();
    assert_deserializable::<PatchMemoryRecordInput>();
    assert_deserializable::<SetMemoryRecordExclusionInput>();
    assert_deserializable::<RetryInterruptedMemoryJobInput>();
    assert_deserializable::<ListRetryableMemoryQueryEmbeddingsInput>();
    assert_deserializable::<ListRetryableGenerationAttemptsInput>();
    assert_deserializable::<RetryMemoryQueryEmbeddingInput>();
    assert_deserializable::<ListContentModuleLifecycleBindingsInput>();
    assert_deserializable::<ListContentModuleLifecycleCandidatesInput>();
    assert_deserializable::<ReviewContentModuleActivationInput>();
    assert_deserializable::<ResolveContentModuleActivationInput>();
    assert_deserializable::<ActivateContentModuleInput>();
    assert_deserializable::<ReviewContentModuleDeactivationInput>();
    assert_deserializable::<DeactivateContentModuleInput>();
    assert_deserializable::<ReviewContentModuleRollbackInput>();
    assert_deserializable::<ResolveContentModuleRollbackInput>();
    assert_deserializable::<ApplyContentModuleRollbackInput>();
    assert_serializable::<ExpertPromptPreviewDto>();
    assert_serializable::<GenerationStartedDto>();
    assert_serializable::<PromptResolutionTraceDto>();
    assert_serializable::<ProviderOverviewDto>();
    assert_serializable::<AssetDeliveryDto>();
    assert_serializable::<Vec<ModelRouteDto>>();
    assert_serializable::<Vec<GenerationPresetDto>>();
    assert_serializable::<NativeCaptureStatusDto>();
    assert_serializable::<MemorySupervisorStatusDto>();
    assert_serializable::<InteractionEffectEventDto>();
    assert_serializable::<PersonaDto>();
    assert_serializable::<PersonaListPageDto>();
    assert_serializable::<PersonaDeletionReceiptDto>();
    assert_serializable::<ConversationPersonaSelectionDto>();
    assert_serializable::<ContentPackageInspectionReviewDto>();
    assert_serializable::<ContentPackageWorkspaceDto>();
    assert_serializable::<SelectContentPackageImportReceiptDto>();
    assert_serializable::<ApproveContentPackageImportReceiptDto>();
    assert_serializable::<CommitContentPackageImportReceiptDto>();
    assert_serializable::<ContentPackageImportSummaryDto>();
    assert_serializable::<RevisionedDto<TaskProfileDto>>();
    assert_serializable::<RevisionedDto<MemoryProfileDto>>();
    assert_serializable::<RevisionedDto<KnowledgeBookDto>>();
    assert_serializable::<RevisionedDto<TransformSetDto>>();
    assert_serializable::<RevisionedDto<InteractionRuleSetDto>>();
    assert_serializable::<RevisionedDto<ContentModuleDto>>();
    assert_serializable::<RevisionedDto<PromptPresetBindingDto>>();
    assert_serializable::<RevisionedDto<ModuleBindingDto>>();
    assert_serializable::<MemoryRecordListDto>();
    assert_serializable::<MemoryRecordProjectionDto>();
    assert_serializable::<MemoryJobRetryReceiptDto>();
    assert_serializable::<Vec<MemoryQueryEmbeddingRetryCandidateDto>>();
    assert_serializable::<Vec<RetryableGenerationAttemptDto>>();
    assert_serializable::<KnowledgeSimulationDto>();
    assert_serializable::<ContentModuleRevisionListDto>();
    assert_serializable::<ContentModuleRevisionDiffDto>();
    assert_serializable::<ContentShareGateDto>();
    assert_serializable::<ContentModuleLifecycleBindingsDto>();
    assert_serializable::<ContentModuleLifecycleCandidatesDto>();
    assert_serializable::<ContentModuleActivationReviewDto>();
    assert_serializable::<ContentModuleActivationPlanDto>();
    assert_serializable::<ContentModuleActivationReceiptDto>();
    assert_serializable::<ContentModuleDeactivationReviewDto>();
    assert_serializable::<ContentModuleDeactivationReceiptDto>();
    assert_serializable::<ContentModuleRollbackReviewDto>();
    assert_serializable::<ContentModuleRollbackPlanDto>();
    assert_serializable::<PromptPlanPreviewDto>();
    assert_serializable::<RevisionedDto<PromptPresetSummaryDto>>();
    assert_serializable::<RevisionedDto<CreatorPromptPresetDocumentDto>>();
    assert_serializable::<ReorderPromptBlocksResultDto>();

    let _ = crate::orchestration_commands::resolve_prompt_preview;
    let _ = crate::orchestration_commands::send_reviewed_prompt;
    let _: for<'a> fn(
        tauri::State<'a, crate::state::AppState>,
        ListRetryableGenerationAttemptsInput,
    ) -> crate::error::CommandResult<Vec<RetryableGenerationAttemptDto>> =
        crate::orchestration_commands::list_retryable_generation_attempts;
    let _: for<'a> fn(
        tauri::State<'a, crate::state::AppState>,
        RetryInterruptedMemoryJobInput,
    ) -> crate::error::CommandResult<MemoryJobRetryReceiptDto> =
        crate::orchestration_commands::retry_interrupted_memory_job;
    let _: for<'a> fn(
        tauri::State<'a, crate::state::AppState>,
        ExplainPromptPlanInput,
    ) -> crate::error::CommandResult<PromptResolutionTraceDto> =
        crate::orchestration_commands::explain_prompt_plan;
    let _ = ConversationModeDto::Chat;
};

#[cfg(test)]
mod interrupted_memory_job_retry_contract_tests {
    use serde_json::{Value, json};

    use super::RetryInterruptedMemoryJobInput;

    const INVOKE_REGISTRY_SOURCE: &str = include_str!("lib.rs");
    const BUILD_MANIFEST_SOURCE: &str = include_str!("../build.rs");
    const DEVELOPMENT_CAPABILITY: &str = include_str!("../capabilities/main-development.json");
    const RELEASE_CAPABILITY: &str = include_str!("../capabilities/main-release.json");
    const GENERATED_PERMISSION: &str =
        include_str!("../permissions/autogenerated/retry_interrupted_memory_job.toml");

    #[test]
    fn retry_request_keeps_exact_acknowledged_cas_shape() {
        let expected = json!({
            "memory_job_id": "memory-job-contract",
            "expected_revision": 7,
            "acknowledge_unknown_outcome": true
        });
        let decoded: RetryInterruptedMemoryJobInput =
            serde_json::from_value(expected.clone()).expect("retry request must decode");
        assert_eq!(
            serde_json::to_value(decoded).expect("retry request must encode"),
            expected
        );
        assert!(
            serde_json::from_value::<RetryInterruptedMemoryJobInput>(json!({
                "memory_job_id": "memory-job-contract",
                "expected_revision": 7,
                "acknowledge_unknown_outcome": true,
                "generic_execute": "forbidden"
            }))
            .is_err()
        );
    }

    #[test]
    fn retry_route_and_permission_are_registered_exactly_once() {
        assert_eq!(
            INVOKE_REGISTRY_SOURCE
                .matches("orchestration_commands::retry_interrupted_memory_job")
                .count(),
            1
        );
        assert_eq!(
            BUILD_MANIFEST_SOURCE
                .matches("\"retry_interrupted_memory_job\"")
                .count(),
            1
        );

        for (kind, source) in [
            ("development", DEVELOPMENT_CAPABILITY),
            ("release", RELEASE_CAPABILITY),
        ] {
            let capability: Value =
                serde_json::from_str(source).unwrap_or_else(|error| panic!("{kind}: {error}"));
            let permissions = capability["permissions"]
                .as_array()
                .unwrap_or_else(|| panic!("{kind} permissions must be an array"));
            assert_eq!(
                permissions
                    .iter()
                    .filter(|permission| {
                        permission.as_str() == Some("allow-retry-interrupted-memory-job")
                    })
                    .count(),
                1,
                "{kind} capability must grant the command exactly once"
            );
        }

        assert_eq!(
            GENERATED_PERMISSION,
            "# Automatically generated - DO NOT EDIT!\n\n\
[[permission]]\n\
identifier = \"allow-retry-interrupted-memory-job\"\n\
description = \"Enables the retry_interrupted_memory_job command without any pre-configured scope.\"\n\
commands.allow = [\"retry_interrupted_memory_job\"]\n\n\
[[permission]]\n\
identifier = \"deny-retry-interrupted-memory-job\"\n\
description = \"Denies the retry_interrupted_memory_job command without any pre-configured scope.\"\n\
commands.deny = [\"retry_interrupted_memory_job\"]\n"
        );
    }
}

#[cfg(test)]
mod retryable_generation_attempt_contract_tests {
    use serde_json::{Value, json};

    use super::{ListRetryableGenerationAttemptsInput, RetryableGenerationAttemptDto};

    #[test]
    fn safe_restart_projection_round_trips_through_tauri_ipc() {
        let request = json!({
            "conversation_id": "conversation-contract",
            "source_branch_id": "branch-contract",
            "limit": 100
        });
        assert_eq!(
            serde_json::to_value(
                serde_json::from_value::<ListRetryableGenerationAttemptsInput>(request.clone())
                    .expect("retryable attempt request must deserialize")
            )
            .expect("retryable attempt request must serialize"),
            request
        );

        let mut request_with_operation_state = request;
        request_with_operation_state["operation_nonce"] = json!("must-not-cross");
        assert!(
            serde_json::from_value::<ListRetryableGenerationAttemptsInput>(
                request_with_operation_state
            )
            .is_err(),
            "restart listing must reject operation state"
        );

        let projection = json!({
            "generation_id": "generation-contract",
            "status": "dispatch_ready",
            "created_at": "2026-08-10T00:00:00Z",
            "updated_at": "2026-08-10T00:00:01Z"
        });
        let dto = serde_json::from_value::<RetryableGenerationAttemptDto>(projection.clone())
            .expect("safe restart projection must deserialize");
        let encoded = serde_json::to_value(dto).expect("safe restart projection must serialize");
        assert_eq!(encoded, projection);
        assert_eq!(
            encoded.as_object().map(serde_json::Map::len),
            Some(4),
            "only safe restart fields may cross Tauri IPC"
        );
        for forbidden in [
            "operation_nonce",
            "prompt_plan",
            "provider_request",
            "credential",
        ] {
            assert_eq!(
                encoded.get(forbidden),
                None,
                "{forbidden} must stay Rust-only"
            );
        }

        let invalid_status = json!({
            "generation_id": "generation-contract",
            "status": "prepared",
            "created_at": "2026-08-10T00:00:00Z",
            "updated_at": "2026-08-10T00:00:01Z"
        });
        assert!(
            serde_json::from_value::<RetryableGenerationAttemptDto>(invalid_status).is_err(),
            "non-retryable statuses cannot cross the restart projection"
        );

        let encoded_text = serde_json::to_string(&encoded).expect("serialize safe projection");
        assert!(!encoded_text.contains("/Users/"));
        assert!(!encoded_text.contains("api_key"));
        assert_eq!(
            encoded["status"],
            Value::String("dispatch_ready".to_owned())
        );
    }
}

#[cfg(test)]
mod prompt_platform_contract_tests {
    use std::any::type_name;

    use lorepia_shell_api::{
        EditUserMessageInput, RegenerateAssistantMessageInput, SendMessageInput,
    };
    use serde_json::{Value, json};

    use super::{
        ExpertPromptPreviewDto, ExplainPromptPlanInput, GenerationStartedDto,
        PromptResolutionTraceDto, ResolvePromptPreviewInput, ReviewedPromptSendInput,
    };

    mod generated {
        include!(concat!(
            env!("OUT_DIR"),
            "/prompt_orchestration_platform_contract.rs"
        ));
    }

    const PLATFORM_GOLDEN: &str =
        include_str!("../contract/prompt-orchestration-platform-golden.json");
    const PLATFORM_GOLDEN_SHA256: &str =
        include_str!("../contract/prompt-orchestration-platform-golden.sha256");
    const RESOLVED_PLAN_GOLDEN_SHA256: &str = include_str!(
        "../../../../crates/orchestration/tests/fixtures/cross_platform_resolved_prompt_plan.sha256"
    );

    #[test]
    fn every_tauri_target_uses_the_reviewed_shell_prompt_contract() {
        let golden: Value =
            serde_json::from_str(PLATFORM_GOLDEN).expect("platform golden must be valid JSON");

        assert_eq!(
            generated::PROMPT_PLATFORM_GOLDEN_SHA256,
            PLATFORM_GOLDEN_SHA256.trim()
        );
        assert_eq!(
            generated::PROMPT_PROJECTION_OWNER,
            golden["projection_owner"]
                .as_str()
                .expect("projection owner must be a string")
        );
        assert_eq!(
            generated::PROMPT_RESOLVED_PLAN_GOLDEN_SHA256,
            RESOLVED_PLAN_GOLDEN_SHA256.trim()
        );
        assert_eq!(
            generated::PROMPT_SHARED_CONFIG,
            golden["shared_config"]
                .as_str()
                .expect("shared config path must be a string")
        );
        assert_eq!(
            generated::PROMPT_RELEASE_CONFIG,
            golden["release_config"]
                .as_str()
                .expect("release config path must be a string")
        );

        let platform_targets = golden["platforms"]
            .as_array()
            .expect("platforms must be an array")
            .iter()
            .map(|platform| {
                platform["platform"]
                    .as_str()
                    .expect("platform name must be a string")
            })
            .collect::<Vec<_>>();
        assert_eq!(
            platform_targets.as_slice(),
            generated::PROMPT_PLATFORM_TARGETS.as_slice()
        );

        let actual_command_contracts = [
            (
                "resolve_prompt_preview",
                short_type_name::<ResolvePromptPreviewInput>(),
                short_type_name::<ExpertPromptPreviewDto>(),
                "async",
                "AppHandle+State<AppState>",
            ),
            (
                "send_reviewed_prompt",
                short_type_name::<ReviewedPromptSendInput>(),
                short_type_name::<GenerationStartedDto>(),
                "async",
                "AppHandle+State<AppState>",
            ),
            (
                "explain_prompt_plan",
                short_type_name::<ExplainPromptPlanInput>(),
                short_type_name::<PromptResolutionTraceDto>(),
                "sync",
                "State<AppState>",
            ),
        ];
        assert_eq!(
            actual_command_contracts,
            generated::PROMPT_COMMAND_CONTRACTS
        );
    }

    #[test]
    fn reviewed_shell_prompt_projection_round_trips_through_tauri_ipc() {
        let fixture = json!({
            "generation_attempt_id": "generation-attempt-contract",
            "plan_id": "plan-contract",
            "plan_hash": RESOLVED_PLAN_GOLDEN_SHA256.trim(),
            "prompt_preset_id": "preset-contract",
            "prompt_preset_revision": 7,
            "generation_target": {
                "model_route_id": "route-contract",
                "generation_preset_id": "generation-preset-contract"
            },
            "estimated_input_tokens": 512,
            "available_input_tokens": 1792,
            "token_estimator_id": "estimator-contract",
            "token_estimate_exact": true,
            "messages": [],
            "provider_family": "openai_chat_completions",
            "provider_messages": [],
            "provider_cache_boundaries": [],
            "cache_directives": [],
            "blocks": [],
            "role_mappings": [],
            "overflow": [],
            "warnings": [],
            "truncated": false,
            "applied_parameters": [{
                "field": "temperature",
                "value_kind": "number",
                "item_count": null
            }],
            "prompt_diff": []
        });
        let projection: ExpertPromptPreviewDto = serde_json::from_value(fixture.clone())
            .expect("the reviewed fixture must deserialize through the Shell-owned IPC DTO");

        assert_eq!(
            projection.plan.plan_hash,
            RESOLVED_PLAN_GOLDEN_SHA256.trim()
        );
        assert_eq!(
            serde_json::to_value(projection).expect("Shell prompt projection must serialize"),
            fixture
        );
    }

    #[test]
    fn reviewed_shell_prompt_projection_rejects_legacy_literal_carriers() {
        let legacy = json!({
            "generation_attempt_id": "generation-attempt-contract",
            "plan_id": "plan-contract",
            "plan_hash": RESOLVED_PLAN_GOLDEN_SHA256.trim(),
            "prompt_preset_id": "preset-contract",
            "prompt_preset_revision": 7,
            "generation_target": {
                "model_route_id": "route-contract",
                "generation_preset_id": "generation-preset-contract"
            },
            "estimated_input_tokens": 1,
            "available_input_tokens": 2,
            "token_estimator_id": "estimator-contract",
            "token_estimate_exact": false,
            "messages": [],
            "provider_family": "openai_chat_completions",
            "provider_messages": [],
            "provider_cache_boundaries": [],
            "cache_directives": [],
            "blocks": [],
            "role_mappings": [],
            "overflow": [],
            "warnings": [],
            "truncated": false,
            "effective_messages": [{"content": null}],
            "provider_request": {"messages": []},
            "applied_parameters": [],
            "prompt_diff": []
        });

        assert!(serde_json::from_value::<ExpertPromptPreviewDto>(legacy).is_err());
    }

    #[test]
    fn ordinary_generation_operation_context_round_trips_through_tauri_request_contracts() {
        let send = json!({
            "conversation_id": "conversation-contract",
            "branch_id": "branch-contract",
            "expected_head": null,
            "mode": "chat",
            "text": "hello",
            "selection": {
                "kind": "legacy_profile",
                "provider_profile_id": "profile-contract"
            },
            "operation_nonce": "nonce-send-contract",
            "generation_attempt_id": null
        });
        assert_eq!(
            serde_json::to_value(
                serde_json::from_value::<SendMessageInput>(send.clone())
                    .expect("send nonce must decode")
            )
            .expect("send nonce must encode"),
            send
        );

        let edit = json!({
            "conversation_id": "conversation-contract",
            "branch_id": "branch-contract",
            "expected_head": "head-contract",
            "message_id": "message-contract",
            "replacement_text": "replacement",
            "selection": {
                "kind": "legacy_profile",
                "provider_profile_id": "profile-contract"
            },
            "operation_nonce": null,
            "generation_attempt_id": "generation-attempt-edit-contract"
        });
        assert_eq!(
            serde_json::to_value(
                serde_json::from_value::<EditUserMessageInput>(edit.clone())
                    .expect("edit nonce must decode")
            )
            .expect("edit nonce must encode"),
            edit
        );

        let regenerate = json!({
            "conversation_id": "conversation-contract",
            "branch_id": "branch-contract",
            "expected_head": "head-contract",
            "message_id": "message-contract",
            "selection": {
                "kind": "legacy_profile",
                "provider_profile_id": "profile-contract"
            },
            "operation_nonce": "nonce-regenerate-contract",
            "generation_attempt_id": null
        });
        assert_eq!(
            serde_json::to_value(
                serde_json::from_value::<RegenerateAssistantMessageInput>(regenerate.clone())
                    .expect("regenerate nonce must decode")
            )
            .expect("regenerate nonce must encode"),
            regenerate
        );
    }

    #[test]
    fn reviewed_generation_attempt_identity_round_trips_through_tauri_request_contracts() {
        let preview = json!({
            "conversation_id": "conversation-contract",
            "branch_id": "branch-contract",
            "expected_head": null,
            "user_text": "hello",
            "generation_target": {
                "model_route_id": "route-contract",
                "generation_preset_id": "generation-preset-contract"
            },
            "prompt_preset_id": null,
            "variable_overrides": { "values": [] },
            "expected_plan_hash": null,
            "operation_nonce": "nonce-reviewed-contract",
            "generation_attempt_id": null
        });
        assert_eq!(
            serde_json::to_value(
                serde_json::from_value::<ResolvePromptPreviewInput>(preview.clone())
                    .expect("preview nonce must decode")
            )
            .expect("preview nonce must encode"),
            preview
        );

        let mut explain = preview.clone();
        let explain_object = explain
            .as_object_mut()
            .expect("prompt request fixture must be an object");
        explain_object.remove("expected_plan_hash");
        explain_object.insert("operation_nonce".to_owned(), Value::Null);
        explain_object.insert("plan_hash".to_owned(), json!("a".repeat(64)));
        explain_object.insert(
            "generation_attempt_id".to_owned(),
            json!("generation-attempt-contract"),
        );
        assert_eq!(
            serde_json::to_value(
                serde_json::from_value::<ExplainPromptPlanInput>(explain.clone())
                    .expect("explain nonce must decode")
            )
            .expect("explain nonce must encode"),
            explain
        );

        let mut reviewed = preview;
        let reviewed_object = reviewed
            .as_object_mut()
            .expect("prompt request fixture must be an object");
        reviewed_object.insert("expected_plan_hash".to_owned(), json!("a".repeat(64)));
        reviewed_object.insert(
            "generation_attempt_id".to_owned(),
            json!("generation-attempt-contract"),
        );
        reviewed_object.remove("operation_nonce");
        assert_eq!(
            serde_json::to_value(
                serde_json::from_value::<ReviewedPromptSendInput>(reviewed.clone())
                    .expect("reviewed send nonce must decode")
            )
            .expect("reviewed send nonce must encode"),
            reviewed
        );
        let mut ambiguous_reviewed = reviewed;
        ambiguous_reviewed["operation_nonce"] = json!("must-be-rejected");
        assert!(
            serde_json::from_value::<ReviewedPromptSendInput>(ambiguous_reviewed).is_err(),
            "reviewed IPC must be attempt-id-only"
        );
    }

    fn short_type_name<T>() -> &'static str {
        type_name::<T>()
            .rsplit("::")
            .next()
            .expect("Rust type names are non-empty")
    }
}

#[cfg(test)]
mod content_module_lifecycle_contract_tests {
    use std::any::type_name;

    use serde::{Serialize, de::DeserializeOwned};
    use serde_json::{Value, json};

    use super::{
        ActivateContentModuleInput, ApplyContentModuleRollbackInput,
        ContentModuleActivationPlanDto, ContentModuleActivationReceiptDto,
        ContentModuleActivationReviewDto, ContentModuleDeactivationReceiptDto,
        ContentModuleDeactivationReviewDto, ContentModuleLifecycleBindingsDto,
        ContentModuleLifecycleCandidatesDto, ContentModuleRollbackPlanDto,
        ContentModuleRollbackReviewDto, DeactivateContentModuleInput,
        ListContentModuleLifecycleBindingsInput, ListContentModuleLifecycleCandidatesInput,
        ResolveContentModuleActivationInput, ResolveContentModuleRollbackInput,
        ReviewContentModuleActivationInput, ReviewContentModuleDeactivationInput,
        ReviewContentModuleRollbackInput,
    };

    const COMMAND_SOURCE: &str = include_str!("module_lifecycle_commands.rs");
    const INVOKE_REGISTRY_SOURCE: &str = include_str!("lib.rs");
    const BUILD_MANIFEST_SOURCE: &str = include_str!("../build.rs");
    const DEVELOPMENT_CAPABILITY: &str = include_str!("../capabilities/main-development.json");
    const RELEASE_CAPABILITY: &str = include_str!("../capabilities/main-release.json");
    const TIMESTAMP: &str = "2026-08-09T00:00:00Z";
    type LifecycleRouteContract = (&'static str, &'static str, &'static str, &'static str);

    #[test]
    fn every_lifecycle_route_keeps_the_reviewed_shell_signature_and_registration() {
        assert_lifecycle_route_signatures();
        let actual = actual_lifecycle_route_contracts();
        assert_eq!(actual, expected_lifecycle_route_contracts());
        assert_lifecycle_route_registration(&actual);
    }

    fn assert_lifecycle_route_signatures() {
        let _: for<'a> fn(
            tauri::State<'a, crate::state::AppState>,
            ListContentModuleLifecycleCandidatesInput,
        )
            -> crate::error::CommandResult<ContentModuleLifecycleCandidatesDto> =
            crate::module_lifecycle_commands::list_content_module_lifecycle_candidates;
        let _: for<'a> fn(
            tauri::State<'a, crate::state::AppState>,
            ListContentModuleLifecycleBindingsInput,
        )
            -> crate::error::CommandResult<ContentModuleLifecycleBindingsDto> =
            crate::module_lifecycle_commands::list_content_module_lifecycle_bindings;
        let _: for<'a> fn(
            tauri::State<'a, crate::state::AppState>,
            ReviewContentModuleActivationInput,
        )
            -> crate::error::CommandResult<ContentModuleActivationReviewDto> =
            crate::module_lifecycle_commands::review_content_module_activation;
        let _: for<'a> fn(
            tauri::State<'a, crate::state::AppState>,
            ResolveContentModuleActivationInput,
        ) -> crate::error::CommandResult<ContentModuleActivationPlanDto> =
            crate::module_lifecycle_commands::resolve_content_module_activation;
        let _: for<'a> fn(
            tauri::State<'a, crate::state::AppState>,
            ActivateContentModuleInput,
        )
            -> crate::error::CommandResult<ContentModuleActivationReceiptDto> =
            crate::module_lifecycle_commands::activate_content_module;
        let _: for<'a> fn(
            tauri::State<'a, crate::state::AppState>,
            ReviewContentModuleDeactivationInput,
        )
            -> crate::error::CommandResult<ContentModuleDeactivationReviewDto> =
            crate::module_lifecycle_commands::review_content_module_deactivation;
        let _: for<'a> fn(
            tauri::State<'a, crate::state::AppState>,
            DeactivateContentModuleInput,
        )
            -> crate::error::CommandResult<ContentModuleDeactivationReceiptDto> =
            crate::module_lifecycle_commands::deactivate_content_module;
        let _: for<'a> fn(
            tauri::State<'a, crate::state::AppState>,
            ReviewContentModuleRollbackInput,
        ) -> crate::error::CommandResult<ContentModuleRollbackReviewDto> =
            crate::module_lifecycle_commands::review_content_module_rollback;
        let _: for<'a> fn(
            tauri::State<'a, crate::state::AppState>,
            ResolveContentModuleRollbackInput,
        ) -> crate::error::CommandResult<ContentModuleRollbackPlanDto> =
            crate::module_lifecycle_commands::resolve_content_module_rollback;
        let _: for<'a> fn(
            tauri::State<'a, crate::state::AppState>,
            ApplyContentModuleRollbackInput,
        )
            -> crate::error::CommandResult<ContentModuleActivationReceiptDto> =
            crate::module_lifecycle_commands::apply_content_module_rollback;
    }

    fn actual_lifecycle_route_contracts() -> [LifecycleRouteContract; 10] {
        [
            (
                "list_content_module_lifecycle_candidates",
                short_type_name::<ListContentModuleLifecycleCandidatesInput>(),
                short_type_name::<ContentModuleLifecycleCandidatesDto>(),
                "request",
            ),
            (
                "list_content_module_lifecycle_bindings",
                short_type_name::<ListContentModuleLifecycleBindingsInput>(),
                short_type_name::<ContentModuleLifecycleBindingsDto>(),
                "request",
            ),
            (
                "review_content_module_activation",
                short_type_name::<ReviewContentModuleActivationInput>(),
                short_type_name::<ContentModuleActivationReviewDto>(),
                "request",
            ),
            (
                "resolve_content_module_activation",
                short_type_name::<ResolveContentModuleActivationInput>(),
                short_type_name::<ContentModuleActivationPlanDto>(),
                "request",
            ),
            (
                "activate_content_module",
                short_type_name::<ActivateContentModuleInput>(),
                short_type_name::<ContentModuleActivationReceiptDto>(),
                "request",
            ),
            (
                "review_content_module_deactivation",
                short_type_name::<ReviewContentModuleDeactivationInput>(),
                short_type_name::<ContentModuleDeactivationReviewDto>(),
                "request",
            ),
            (
                "deactivate_content_module",
                short_type_name::<DeactivateContentModuleInput>(),
                short_type_name::<ContentModuleDeactivationReceiptDto>(),
                "request",
            ),
            (
                "review_content_module_rollback",
                short_type_name::<ReviewContentModuleRollbackInput>(),
                short_type_name::<ContentModuleRollbackReviewDto>(),
                "request",
            ),
            (
                "resolve_content_module_rollback",
                short_type_name::<ResolveContentModuleRollbackInput>(),
                short_type_name::<ContentModuleRollbackPlanDto>(),
                "request",
            ),
            (
                "apply_content_module_rollback",
                short_type_name::<ApplyContentModuleRollbackInput>(),
                short_type_name::<ContentModuleActivationReceiptDto>(),
                "request",
            ),
        ]
    }

    const fn expected_lifecycle_route_contracts() -> [LifecycleRouteContract; 10] {
        [
            (
                "list_content_module_lifecycle_candidates",
                "ListContentModuleLifecycleCandidatesInput",
                "ContentModuleLifecycleCandidatesDto",
                "request",
            ),
            (
                "list_content_module_lifecycle_bindings",
                "ListContentModuleLifecycleBindingsInput",
                "ContentModuleLifecycleBindingsDto",
                "request",
            ),
            (
                "review_content_module_activation",
                "ReviewContentModuleActivationInput",
                "ContentModuleActivationReviewPresentation",
                "request",
            ),
            (
                "resolve_content_module_activation",
                "ResolveContentModuleActivationInput",
                "ResolvedModulePlan",
                "request",
            ),
            (
                "activate_content_module",
                "ActivateContentModuleInput",
                "ContentModuleActivationReceiptDto",
                "request",
            ),
            (
                "review_content_module_deactivation",
                "ReviewContentModuleDeactivationInput",
                "ContentModuleDeactivationReview",
                "request",
            ),
            (
                "deactivate_content_module",
                "DeactivateContentModuleInput",
                "ContentModuleDeactivationReceiptDto",
                "request",
            ),
            (
                "review_content_module_rollback",
                "ReviewContentModuleRollbackInput",
                "ContentModuleRollbackReviewPresentation",
                "request",
            ),
            (
                "resolve_content_module_rollback",
                "ContentModuleRollbackResolutionRequest",
                "ContentModuleRollbackPlan",
                "request",
            ),
            (
                "apply_content_module_rollback",
                "ContentModuleRollbackApplyRequest",
                "ContentModuleActivationReceiptDto",
                "request",
            ),
        ]
    }

    fn assert_lifecycle_route_registration(actual: &[LifecycleRouteContract]) {
        let development: Value = serde_json::from_str(DEVELOPMENT_CAPABILITY)
            .expect("development capability must be JSON");
        let release: Value =
            serde_json::from_str(RELEASE_CAPABILITY).expect("release capability must be JSON");
        for &(command, _, _, request_argument) in actual {
            let invoke_entry = format!("module_lifecycle_commands::{command}");
            assert_eq!(
                INVOKE_REGISTRY_SOURCE.matches(&invoke_entry).count(),
                1,
                "{command} must be registered exactly once in the invoke handler"
            );
            assert_eq!(
                BUILD_MANIFEST_SOURCE
                    .matches(&format!("\"{command}\""))
                    .count(),
                1,
                "{command} must be registered exactly once in the Tauri command manifest"
            );
            assert_command_argument_name(command, request_argument);
            let permission = format!("allow-{}", command.replace('_', "-"));
            assert_permission(&development, &permission, "development");
            assert_permission(&release, &permission, "release");
        }
    }

    #[test]
    fn lifecycle_request_wrappers_round_trip_with_exact_snake_case_json() {
        let activation = activation_request();
        let resolutions = resolution_set();
        let approval = activation_approval();
        let deactivation = deactivation_request();
        let rollback = rollback_resolution();

        assert_request_wrapper::<ListContentModuleLifecycleCandidatesInput>(
            "list_content_module_lifecycle_candidates",
            json!({"request": {"runtime_target": runtime_target(), "limit": 10}}),
        );
        assert_request_wrapper::<ListContentModuleLifecycleBindingsInput>(
            "list_content_module_lifecycle_bindings",
            json!({"request": {"runtime_target": runtime_target(), "limit": 10}}),
        );
        assert_request_wrapper::<ReviewContentModuleActivationInput>(
            "review_content_module_activation",
            json!({"request": {"activation": activation.clone()}}),
        );
        assert_request_wrapper::<ResolveContentModuleActivationInput>(
            "resolve_content_module_activation",
            json!({"request": {
                "activation": activation.clone(),
                "resolutions": resolutions.clone()
            }}),
        );
        assert_request_wrapper::<ActivateContentModuleInput>(
            "activate_content_module",
            json!({"request": {
                "activation": activation,
                "resolutions": resolutions.clone(),
                "approval": approval.clone()
            }}),
        );
        assert_request_wrapper::<ReviewContentModuleDeactivationInput>(
            "review_content_module_deactivation",
            json!({"request": {"deactivation": deactivation.clone()}}),
        );
        assert_request_wrapper::<DeactivateContentModuleInput>(
            "deactivate_content_module",
            json!({"request": {
                "deactivation": deactivation,
                "expected_review_sha256": digest('d')
            }}),
        );
        assert_request_wrapper::<ReviewContentModuleRollbackInput>(
            "review_content_module_rollback",
            json!({"request": {
                "binding_id": "binding-contract",
                "target_revision_id": "revision-contract-v1",
                "target_package_import_approval_id": null,
                "runtime_target": runtime_target()
            }}),
        );
        assert_request_wrapper::<ResolveContentModuleRollbackInput>(
            "resolve_content_module_rollback",
            json!({"request": rollback.clone()}),
        );
        assert_request_wrapper::<ApplyContentModuleRollbackInput>(
            "apply_content_module_rollback",
            json!({"request": {
                "resolution": rollback,
                "expected_rollback_plan_sha256": digest('e'),
                "activation_approval": approval
            }}),
        );
    }

    #[test]
    fn representative_lifecycle_dtos_keep_reviewed_tags_and_field_names() {
        assert_json_round_trip::<ContentModuleLifecycleCandidatesDto>(candidate_list());
        assert_json_round_trip::<ContentModuleLifecycleBindingsDto>(json!({
            "items": [],
            "truncated": false,
            "workspace_review_sha256": digest('a'),
            "workspace_state_revision": 7
        }));
        assert_json_round_trip::<ContentModuleActivationReviewDto>(activation_review_presentation());
        assert_json_round_trip::<ContentModuleActivationPlanDto>(activation_plan());
        assert_json_round_trip::<ContentModuleActivationReceiptDto>(activation_receipt());
        assert_json_round_trip::<ContentModuleDeactivationReviewDto>(deactivation_review());
        assert_json_round_trip::<ContentModuleDeactivationReceiptDto>(deactivation_receipt());
        assert_json_round_trip::<ContentModuleRollbackReviewDto>(rollback_review());
        assert_json_round_trip::<ContentModuleRollbackPlanDto>(rollback_plan());
    }

    fn runtime_target() -> Value {
        json!({
            "conversation_id": "conversation-contract",
            "branch_id": "branch-contract"
        })
    }

    fn binding_draft() -> Value {
        json!({
            "id": "binding-contract",
            "module_id": "module-contract",
            "scope": "branch",
            "target_id": "branch-contract",
            "conversation_id": "conversation-contract",
            "priority": 7,
            "resolution_mode": "pinned",
            "pinned_revision_id": "revision-contract-v2",
            "package_import_approval_id": null,
            "variable_overrides": {"values": []}
        })
    }

    fn binding() -> Value {
        json!({
            "id": "binding-contract",
            "module_id": "module-contract",
            "scope": "branch",
            "target_id": "branch-contract",
            "conversation_id": "conversation-contract",
            "priority": 7,
            "resolution_mode": "pinned",
            "pinned_revision_id": "revision-contract-v2",
            "enabled": true,
            "approved": true,
            "package_import_approval_id": null,
            "activation_approval_id": "approval-contract",
            "activation_review_sha256": digest('a'),
            "activation_plan_sha256": digest('b'),
            "variable_overrides": {"values": []},
            "revision_id": "revision-contract-v2",
            "created_at": TIMESTAMP
        })
    }

    fn lifecycle_binding() -> Value {
        json!({
            "binding": binding(),
            "state_revision": 7,
            "updated_at": TIMESTAMP
        })
    }

    fn activation_request() -> Value {
        json!({
            "runtime_target": runtime_target(),
            "expected_binding_revision": 7,
            "binding": binding_draft()
        })
    }

    fn resolution_set() -> Value {
        json!({
            "expected_review_sha256": digest('a'),
            "resolutions": []
        })
    }

    fn activation_approval() -> Value {
        json!({
            "approval_id": "approval-contract",
            "expected_review_sha256": digest('a'),
            "expected_plan_sha256": digest('b')
        })
    }

    fn deactivation_request() -> Value {
        json!({
            "runtime_target": runtime_target(),
            "binding_id": "binding-contract"
        })
    }

    fn rollback_resolution() -> Value {
        json!({
            "runtime_target": runtime_target(),
            "binding_id": "binding-contract",
            "target_revision_id": "revision-contract-v1",
            "target_package_import_approval_id": null,
            "expected_state_revision": 7,
            "expected_rollback_review_sha256": digest('c'),
            "resolutions": resolution_set()
        })
    }

    fn candidate_list() -> Value {
        json!({
            "items": [{
                "module_id": "module-contract",
                "revision_id": "revision-contract-v2",
                "revision_source_sha256": digest('2'),
                "name": "Contract module",
                "version": "2.0.0",
                "author": null,
                "license": "LicenseRef-Contract",
                "redistribution_allowed": false,
                "required_capabilities": ["declarative_interactions"],
                "source_kind": "imported_package",
                "local_use_allowed": true,
                "sharing_allowed": false,
                "share_reasons": ["contract fixture is not redistributable"],
                "component_count": 1,
                "completed_package_approvals": []
            }],
            "truncated": false,
            "scope_targets": [{
                "scope": "branch",
                "target_id": "branch-contract",
                "conversation_id": "conversation-contract",
                "label": "Current branch"
            }]
        })
    }

    fn revision_review() -> Value {
        json!({
            "module_id": "module-contract",
            "revision_id": "revision-contract-v2",
            "revision_source_sha256": digest('2'),
            "name": "Contract module",
            "version": "2.0.0",
            "author": null,
            "license": "LicenseRef-Contract",
            "redistribution_allowed": false,
            "required_capabilities": ["declarative_interactions"],
            "source_kind": "imported_package",
            "local_use_allowed": true,
            "sharing_allowed": false,
            "share_reasons": ["contract fixture is not redistributable"]
        })
    }

    fn activation_review() -> Value {
        json!({
            "review_sha256": digest('a'),
            "state_revision": 7,
            "context": {
                "local_user_id": "local-user-contract",
                "persona_id": null,
                "character_id": "character-contract",
                "conversation_id": "conversation-contract",
                "branch_id": "branch-contract",
                "supported_capabilities": ["declarative_interactions"]
            },
            "activation_binding_ids": ["binding-contract"],
            "ordered_bindings": [binding()],
            "ignored_bindings": [{
                "binding_id": "ignored-binding-contract",
                "reason": "awaiting_approval"
            }],
            "components": [],
            "conflicts": [],
            "import_approvals": [],
            "effective_variable_overrides": {"values": []}
        })
    }

    fn activation_review_presentation() -> Value {
        json!({
            "review": activation_review(),
            "proposed_revision": revision_review()
        })
    }

    fn selected_source() -> Value {
        json!({
            "binding_id": "binding-contract",
            "module_id": "module-contract",
            "revision_id": "revision-contract-v2",
            "revision_source_sha256": digest('2'),
            "scope": "branch",
            "target_id": "branch-contract",
            "conversation_id": "conversation-contract",
            "priority": 7,
            "module_ordinal": 0,
            "runtime_enabled_intent": true
        })
    }

    fn activation_plan() -> Value {
        json!({
            "plan_sha256": digest('b'),
            "review_sha256": digest('a'),
            "expected_state_revision": 7,
            "activation_binding_ids": ["binding-contract"],
            "ordered_binding_ids": ["binding-contract"],
            "components": [{
                "component": {"kind": "interaction_rule_set", "id": "interaction-contract"},
                "sha256": digest('3'),
                "selected_source": selected_source(),
                "coalesced_sources": [selected_source()],
                "runtime_enabled": true
            }],
            "omitted_components": [{"kind": "asset", "id": "asset-contract"}],
            "import_approvals": [],
            "effective_variable_overrides": {"values": []}
        })
    }

    fn activation_receipt() -> Value {
        json!({
            "verified": true,
            "binding": lifecycle_binding(),
            "approval_id": "approval-contract",
            "approval_sha256": digest('4'),
            "review_sha256": digest('a'),
            "plan_sha256": digest('b'),
            "approved_plan": activation_plan(),
            "approved_components": [{
                "component": {"kind": "interaction_rule_set", "id": "interaction-contract"},
                "component_sha256": digest('3'),
                "selected_source": selected_source(),
                "runtime_enabled": true
            }]
        })
    }

    fn deactivation_review() -> Value {
        json!({
            "review_sha256": digest('d'),
            "runtime_target": runtime_target(),
            "binding": binding(),
            "approved_revision_id": "revision-contract-v2",
            "expected_binding_revision": 7,
            "binding_updated_at": TIMESTAMP,
            "disposition": "needs_reapproval"
        })
    }

    fn deactivation_receipt() -> Value {
        let mut deleted_binding = lifecycle_binding();
        deleted_binding["state_revision"] = json!(8);
        json!({
            "verified": true,
            "review": deactivation_review(),
            "binding": deleted_binding,
            "deleted_at": TIMESTAMP
        })
    }

    fn rollback_review() -> Value {
        json!({
            "review": {
                "rollback": {
                    "review_sha256": digest('c'),
                    "expected_state_revision": 7,
                    "binding_id": "binding-contract",
                    "current_revision_id": "revision-contract-v2",
                    "current_source_sha256": digest('2'),
                    "target_revision_id": "revision-contract-v1",
                    "target_source_sha256": digest('1'),
                    "diff": null,
                    "blockers": [{
                        "kind": "unsupported_capability",
                        "capability": "declarative_interactions"
                    }],
                    "eligible": false
                },
                "activation": activation_review()
            },
            "target_revision": {
                "module_id": "module-contract",
                "revision_id": "revision-contract-v1",
                "revision_source_sha256": digest('1'),
                "name": "Contract module",
                "version": "1.0.0",
                "author": null,
                "license": "LicenseRef-Contract",
                "redistribution_allowed": false,
                "required_capabilities": ["declarative_interactions"],
                "source_kind": "imported_package",
                "local_use_allowed": true,
                "sharing_allowed": false,
                "share_reasons": ["contract fixture is not redistributable"]
            }
        })
    }

    fn rollback_plan() -> Value {
        json!({
            "rollback": {
                "plan_sha256": digest('e'),
                "review_sha256": digest('c'),
                "expected_state_revision": 7,
                "binding_id": "binding-contract",
                "expected_current_revision_id": "revision-contract-v2",
                "expected_current_source_sha256": digest('2'),
                "target_revision_id": "revision-contract-v1",
                "target_source_sha256": digest('1'),
                "diff_sha256": digest('f')
            },
            "activation": activation_plan()
        })
    }

    fn assert_request_wrapper<T>(command: &str, wrapper: Value)
    where
        T: DeserializeOwned + Serialize,
    {
        let object = wrapper
            .as_object()
            .unwrap_or_else(|| panic!("{command} request wrapper must be an object"));
        assert_eq!(
            object.len(),
            1,
            "{command} request wrapper must have one field"
        );
        let request = object
            .get("request")
            .unwrap_or_else(|| panic!("{command} request wrapper must use the request key"));
        let decoded: T = serde_json::from_value(request.clone())
            .unwrap_or_else(|error| panic!("{command} request JSON must decode: {error}"));
        assert_eq!(
            serde_json::to_value(decoded)
                .unwrap_or_else(|error| panic!("{command} request JSON must encode: {error}")),
            *request,
            "{command} request JSON field names or enum tags drifted"
        );
    }

    fn assert_json_round_trip<T>(expected: Value)
    where
        T: DeserializeOwned + Serialize,
    {
        let decoded: T = serde_json::from_value(expected.clone())
            .unwrap_or_else(|error| panic!("representative lifecycle DTO must decode: {error}"));
        assert_eq!(
            serde_json::to_value(decoded).unwrap_or_else(|error| panic!(
                "representative lifecycle DTO must encode: {error}"
            )),
            expected
        );
    }

    fn assert_command_argument_name(command: &str, expected: &str) {
        let needle = format!("pub fn {command}(");
        let start = COMMAND_SOURCE
            .find(&needle)
            .unwrap_or_else(|| panic!("{command} command wrapper must exist"));
        let signature = &COMMAND_SOURCE[start..];
        let end = signature
            .find(") ->")
            .unwrap_or_else(|| panic!("{command} command signature must be complete"));
        assert!(
            signature[..end].contains(&format!("{expected}:")),
            "{command} must preserve the Tauri {expected} argument name"
        );
    }

    fn assert_permission(capability: &Value, permission: &str, kind: &str) {
        assert!(
            capability["permissions"]
                .as_array()
                .unwrap_or_else(|| panic!("{kind} permissions must be an array"))
                .iter()
                .any(|candidate| candidate.as_str() == Some(permission)),
            "{kind} capability must contain {permission}"
        );
    }

    fn short_type_name<T>() -> &'static str {
        type_name::<T>()
            .rsplit("::")
            .next()
            .expect("Rust type names are non-empty")
    }

    fn digest(value: char) -> String {
        value.to_string().repeat(64)
    }
}

#[cfg(test)]
mod content_source_export_contract_tests {
    use lorepia_shell_api::{
        ContentSourceExportDescriptorDto, ContentSourceExportInput, ContentSourceExportKindDto,
        ContentSourceExportReceiptDto, ListCompletedContentPackageExportsInput,
    };
    use serde_json::{Value, json};

    const COMMAND_SOURCE: &str = include_str!("package_commands.rs");
    const INVOKE_REGISTRY_SOURCE: &str = include_str!("lib.rs");
    const BUILD_MANIFEST_SOURCE: &str = include_str!("../build.rs");
    const DEVELOPMENT_CAPABILITY: &str = include_str!("../capabilities/main-development.json");
    const RELEASE_CAPABILITY: &str = include_str!("../capabilities/main-release.json");
    const MACOS_ENTITLEMENTS_SOURCE: &str = include_str!("../Entitlements.plist");
    const PLATFORM_PLUGIN_BUILD_SOURCE: &str =
        include_str!("../../../../plugins/lorepia-platform/build.rs");

    #[test]
    fn export_route_is_registered_once_with_the_safe_async_contract() {
        let _ = crate::package_commands::export_content_source;
        assert_eq!(
            INVOKE_REGISTRY_SOURCE
                .matches("package_commands::export_content_source")
                .count(),
            1
        );
        assert_eq!(
            BUILD_MANIFEST_SOURCE
                .matches("\"export_content_source\"")
                .count(),
            1
        );
        assert!(COMMAND_SOURCE.contains("pub async fn export_content_source("));
        assert!(COMMAND_SOURCE.contains("request: shell::ContentSourceExportInput"));
        assert!(
            COMMAND_SOURCE.contains("CommandResult<Option<shell::ContentSourceExportReceiptDto>>")
        );

        let development: Value = serde_json::from_str(DEVELOPMENT_CAPABILITY)
            .expect("development capability must be JSON");
        let release: Value =
            serde_json::from_str(RELEASE_CAPABILITY).expect("release capability must be JSON");
        for (kind, capability) in [("development", development), ("release", release)] {
            assert!(
                capability["permissions"]
                    .as_array()
                    .unwrap_or_else(|| panic!("{kind} permissions must be an array"))
                    .iter()
                    .any(|candidate| candidate.as_str() == Some("allow-export-content-source")),
                "{kind} capability must allow only the high-level export command"
            );
        }
    }

    #[test]
    fn export_path_is_rust_only_and_the_platform_plugin_has_no_webview_commands() {
        assert!(
            COMMAND_SOURCE
                .contains("let prepared = state.shell()?.prepare_content_source_export(request)?")
        );
        assert!(COMMAND_SOURCE.contains("prepared.source_path()"));
        assert!(COMMAND_SOURCE.contains(".lorepia_platform()"));
        assert!(COMMAND_SOURCE.contains(".save_content_source("));
        assert!(
            PLATFORM_PLUGIN_BUILD_SOURCE.contains("const COMMANDS: &[&str] = &[];"),
            "the scoped native adapter must not register a JavaScript-callable filesystem command"
        );
    }

    #[test]
    fn sandboxed_macos_export_has_user_selected_read_write_entitlement() {
        assert!(
            MACOS_ENTITLEMENTS_SOURCE.contains(
                "<key>com.apple.security.files.user-selected.read-write</key>\n  <true/>"
            ),
            "the sandboxed macOS save-panel export must retain user-selected write access"
        );
        assert!(
            !MACOS_ENTITLEMENTS_SOURCE.contains("com.apple.security.files.user-selected.read-only"),
            "the read-only entitlement cannot authorize the export destination write"
        );
    }

    #[test]
    fn completed_export_catalog_is_registered_with_an_exact_path_free_contract() {
        let _ = crate::package_commands::list_completed_content_package_exports;
        assert_eq!(
            INVOKE_REGISTRY_SOURCE
                .matches("package_commands::list_completed_content_package_exports")
                .count(),
            1
        );
        assert_eq!(
            BUILD_MANIFEST_SOURCE
                .matches("\"list_completed_content_package_exports\"")
                .count(),
            1
        );
        assert!(COMMAND_SOURCE.contains("pub fn list_completed_content_package_exports("));
        assert!(COMMAND_SOURCE.contains("request: shell::ListCompletedContentPackageExportsInput"));
        assert!(
            COMMAND_SOURCE.contains("CommandResult<Vec<shell::ContentSourceExportDescriptorDto>>")
        );
        assert!(COMMAND_SOURCE.contains(".list_completed_content_package_exports(request)"));

        let development: Value = serde_json::from_str(DEVELOPMENT_CAPABILITY)
            .expect("development capability must be JSON");
        let release: Value =
            serde_json::from_str(RELEASE_CAPABILITY).expect("release capability must be JSON");
        for (kind, capability) in [("development", development), ("release", release)] {
            assert!(
                capability["permissions"]
                    .as_array()
                    .unwrap_or_else(|| panic!("{kind} permissions must be an array"))
                    .iter()
                    .any(|candidate| {
                        candidate.as_str() == Some("allow-list-completed-content-package-exports")
                    }),
                "{kind} capability must allow the read-only completed export catalog"
            );
        }

        let request = json!({"limit": 100});
        let decoded: ListCompletedContentPackageExportsInput =
            serde_json::from_value(request.clone()).expect("bounded catalog request must decode");
        assert_eq!(
            serde_json::to_value(decoded).expect("catalog request must encode"),
            request
        );
        assert!(
            serde_json::from_value::<ListCompletedContentPackageExportsInput>(json!({
                "limit": 100,
                "path": "/Users/synthetic/private"
            }))
            .is_err()
        );

        let descriptors = vec![ContentSourceExportDescriptorDto {
            kind: ContentSourceExportKindDto::LorepiaPackage,
            source_id: "import-1".to_owned(),
            sha256: digest('c'),
            size_bytes: 84,
            suggested_file_name: "lorepia-package-example-1.0.0.zip".to_owned(),
        }];
        let serialized = serde_json::to_value(descriptors).expect("catalog must encode");
        assert_eq!(
            serialized,
            json!([{
                "kind": "lorepia_package",
                "source_id": "import-1",
                "sha256": digest('c'),
                "size_bytes": 84,
                "suggested_file_name": "lorepia-package-example-1.0.0.zip"
            }])
        );
        let catalog_json = serialized.to_string();
        assert!(!catalog_json.contains("/Users/"));
        assert!(!catalog_json.contains("\"path\""));
        assert!(!catalog_json.contains("\"bytes\""));
    }

    #[test]
    fn export_request_and_receipt_keep_paths_and_bytes_out_of_ipc() {
        const CREDENTIAL_CANARY: &str = "sk-synthetic-package-export-canary-4f91";
        const PATH_CANARY: &str = "/Users/synthetic/private-origin-path-canary/package.zip";
        for expected in [
            json!({"kind": "character_source", "character_id": "character-1"}),
            json!({"kind": "content_package", "import_id": "import-1"}),
        ] {
            let decoded: ContentSourceExportInput = serde_json::from_value(expected.clone())
                .expect("representative export request must decode");
            assert_eq!(
                serde_json::to_value(decoded).expect("export request must encode"),
                expected
            );
        }
        for rejected in [
            json!({
                "kind": "character_source",
                "character_id": "character-1",
                "path": "/Users/synthetic/private/card.json"
            }),
            json!({
                "kind": "content_package",
                "import_id": "import-1",
                "bytes": [1, 2, 3]
            }),
            json!({
                "kind": "content_package",
                "import_id": "import-1",
                "credential": CREDENTIAL_CANARY
            }),
            json!({
                "kind": "content_package",
                "import_id": "import-1",
                "path": PATH_CANARY
            }),
        ] {
            let error = serde_json::from_value::<ContentSourceExportInput>(rejected)
                .expect_err("private export fields must fail before the command boundary");
            let rendered = format!("{error:?}");
            assert!(!rendered.contains(CREDENTIAL_CANARY));
            assert!(!rendered.contains(PATH_CANARY));
        }

        let descriptor = descriptor();
        let receipt = crate::package_commands::project_content_source_export_receipt(
            &descriptor,
            "actual-export.json",
            descriptor.size_bytes,
            &descriptor.sha256,
        )
        .expect("an exact native delivery must project a receipt");
        let serialized = serde_json::to_value(&receipt).expect("receipt must encode");
        assert_eq!(
            serialized,
            json!({
                "kind": "character_card_v3",
                "source_id": "character-1",
                "sha256": digest('a'),
                "size_bytes": 42,
                "file_name": "actual-export.json"
            })
        );
        let object = serialized.as_object().expect("receipt must be an object");
        assert_eq!(object.len(), 5);
        assert!(!object.contains_key("path"));
        assert!(!object.contains_key("bytes"));
        assert!(!object.contains_key("suggested_file_name"));
        assert!(!serialized.to_string().contains(CREDENTIAL_CANARY));
        assert!(!serialized.to_string().contains(PATH_CANARY));

        let _: ContentSourceExportReceiptDto = receipt;
    }

    #[test]
    fn native_delivery_must_match_the_prepared_hash_and_size_exactly() {
        let descriptor = descriptor();
        for error in [
            crate::package_commands::project_content_source_export_receipt(
                &descriptor,
                "actual-export.json",
                descriptor.size_bytes + 1,
                &descriptor.sha256,
            )
            .expect_err("a size mismatch must fail closed"),
            crate::package_commands::project_content_source_export_receipt(
                &descriptor,
                "actual-export.json",
                descriptor.size_bytes,
                &digest('b'),
            )
            .expect_err("a hash mismatch must fail closed"),
        ] {
            assert_eq!(error.code, "storage_unavailable");
            assert_eq!(error.message_key, "error.storage_unavailable");
            assert!(error.recoverable);
        }

        let invalid_name = crate::package_commands::project_content_source_export_receipt(
            &descriptor,
            "../private.json",
            descriptor.size_bytes,
            &descriptor.sha256,
        )
        .expect_err("a native path-like display name must not become IPC data");
        assert_eq!(invalid_name.code, "invalid_input");
    }

    fn descriptor() -> ContentSourceExportDescriptorDto {
        ContentSourceExportDescriptorDto {
            kind: ContentSourceExportKindDto::CharacterCardV3,
            source_id: "character-1".to_owned(),
            sha256: digest('a'),
            size_bytes: 42,
            suggested_file_name: "suggested-card.json".to_owned(),
        }
    }

    fn digest(value: char) -> String {
        value.to_string().repeat(64)
    }
}

#[cfg(test)]
mod dependency_boundary_tests {
    use std::{
        env,
        path::{Path, PathBuf},
        process::Command,
    };

    use serde::Deserialize;

    const FORBIDDEN_PACKAGES: [&str; 2] = ["lorepia-core", "lorepia-storage"];
    const BUILD_SCRIPT_SOURCE: &str = include_str!("../build.rs");

    #[derive(Debug, Deserialize)]
    struct CargoMetadata {
        packages: Vec<CargoPackage>,
    }

    #[derive(Debug, Deserialize)]
    struct CargoPackage {
        manifest_path: PathBuf,
        dependencies: Vec<CargoDependency>,
    }

    #[derive(Debug, Deserialize)]
    struct CargoDependency {
        name: String,
        rename: Option<String>,
        kind: Option<String>,
        target: Option<String>,
    }

    fn load_cargo_metadata(manifest_path: &Path) -> CargoMetadata {
        let cargo = env::var_os("CARGO").unwrap_or_else(|| "cargo".into());
        let output = Command::new(cargo)
            .args([
                "metadata",
                "--format-version",
                "1",
                "--no-deps",
                "--locked",
                "--offline",
            ])
            .arg("--manifest-path")
            .arg(manifest_path)
            .output()
            .expect("run cargo metadata for the Tauri dependency boundary");
        assert!(
            output.status.success(),
            "cargo metadata failed for {}: {}",
            manifest_path.display(),
            String::from_utf8_lossy(&output.stderr)
        );
        serde_json::from_slice(&output.stdout).expect("decode cargo metadata output")
    }

    fn package_for_manifest<'a>(
        metadata: &'a CargoMetadata,
        manifest_path: &Path,
    ) -> &'a CargoPackage {
        let expected = manifest_path
            .canonicalize()
            .expect("canonicalize the Tauri Cargo manifest path");
        metadata
            .packages
            .iter()
            .find(|package| {
                package
                    .manifest_path
                    .canonicalize()
                    .is_ok_and(|candidate| candidate == expected)
            })
            .expect("cargo metadata must contain the Tauri package")
    }

    fn forbidden_direct_dependencies(dependencies: &[CargoDependency]) -> Vec<String> {
        dependencies
            .iter()
            .filter(|dependency| FORBIDDEN_PACKAGES.contains(&dependency.name.as_str()))
            .map(|dependency| {
                let declared_name = dependency.rename.as_deref().unwrap_or(&dependency.name);
                let kind = dependency.kind.as_deref().unwrap_or("normal");
                let target = dependency.target.as_deref().unwrap_or("all targets");
                format!(
                    "{declared_name} -> {} (kind: {kind}, target: {target})",
                    dependency.name
                )
            })
            .collect()
    }

    #[test]
    fn tauri_app_does_not_depend_directly_on_core_or_storage() {
        let manifest_path = Path::new(env!("CARGO_MANIFEST_DIR")).join("Cargo.toml");
        let metadata = load_cargo_metadata(&manifest_path);
        let package = package_for_manifest(&metadata, &manifest_path);
        let forbidden = forbidden_direct_dependencies(&package.dependencies);

        assert!(
            forbidden.is_empty(),
            "the Tauri application layer must depend on Core and Storage only through \
             lorepia-shell-api; forbidden direct dependencies: {}",
            forbidden.join(", ")
        );
    }

    #[test]
    fn tauri_build_script_does_not_reach_into_core_or_storage_source() {
        for forbidden_source_path in ["crates/core", "crates/storage"] {
            assert!(
                !BUILD_SCRIPT_SOURCE.contains(forbidden_source_path),
                "the Tauri build script must not inspect {forbidden_source_path} source"
            );
        }
    }

    #[test]
    fn cargo_metadata_boundary_detects_renamed_target_scoped_packages_in_every_kind() {
        let dependencies = [
            CargoDependency {
                name: "lorepia-core".to_owned(),
                rename: Some("test-engine".to_owned()),
                kind: Some("dev".to_owned()),
                target: None,
            },
            CargoDependency {
                name: "lorepia-storage".to_owned(),
                rename: Some("windows-test-database".to_owned()),
                kind: Some("build".to_owned()),
                target: Some("cfg(target_os = \"windows\")".to_owned()),
            },
            CargoDependency {
                name: "lorepia-core".to_owned(),
                rename: None,
                kind: None,
                target: Some("cfg(target_os = \"android\")".to_owned()),
            },
        ];

        let forbidden = forbidden_direct_dependencies(&dependencies);

        assert_eq!(forbidden.len(), 3);
        assert!(forbidden[0].contains("test-engine -> lorepia-core"));
        assert!(forbidden[0].contains("kind: dev"));
        assert!(forbidden[1].contains("windows-test-database -> lorepia-storage"));
        assert!(forbidden[1].contains("target_os = \"windows\""));
        assert!(forbidden[2].contains("lorepia-core -> lorepia-core"));
        assert!(forbidden[2].contains("target_os = \"android\""));
    }
}

#[cfg(test)]
mod windows_installer_contract_tests {
    use serde_json::Value;

    const BASE_CONFIG_SOURCE: &str = include_str!("../tauri.conf.json");
    const RELEASE_CONFIG_SOURCE: &str = include_str!("../tauri.release.conf.json");
    const WINDOWS_CONFIG_SOURCE: &str = include_str!("../tauri.windows.conf.json");
    const PACKAGE_SOURCE: &str = include_str!("../../package.json");
    const WINDOWS_UPGRADE_CODE: &str = "1d306e7f-9b28-552c-ae87-32153f694083";

    fn parse(source: &str) -> Value {
        serde_json::from_str(source).expect("production Tauri configuration must be JSON")
    }

    fn merge_patch(target: &mut Value, patch: &Value) {
        match patch {
            Value::Object(patch_object) => {
                if !target.is_object() {
                    *target = serde_json::json!({});
                }
                let target_object = target.as_object_mut().expect("object initialized above");
                for (key, value) in patch_object {
                    if value.is_null() {
                        target_object.remove(key);
                    } else {
                        merge_patch(target_object.entry(key).or_insert(Value::Null), value);
                    }
                }
            }
            _ => *target = patch.clone(),
        }
    }

    #[test]
    fn production_windows_bundle_is_one_non_downgradable_msi_upgrade_family() {
        let raw = parse(WINDOWS_CONFIG_SOURCE);
        let schema_config: tauri::utils::config::Config =
            serde_json::from_str(WINDOWS_CONFIG_SOURCE)
                .expect("Windows overlay must satisfy the pinned Tauri config types");

        assert_eq!(raw["bundle"]["targets"], serde_json::json!(["msi"]));
        assert_eq!(
            raw["bundle"]["windows"]["wix"]["upgradeCode"],
            WINDOWS_UPGRADE_CODE
        );
        assert_eq!(raw["bundle"]["windows"]["wix"]["version"], "0.1.2");
        assert_eq!(raw["bundle"]["windows"]["allowDowngrades"], false);
        assert!(schema_config.bundle.windows.wix.is_some());
        assert!(!schema_config.bundle.windows.allow_downgrades);

        let schema_targets =
            serde_json::to_value(&schema_config.bundle.targets).expect("serialize bundle targets");
        assert_eq!(schema_targets, serde_json::json!(["msi"]));
        let wix = schema_config
            .bundle
            .windows
            .wix
            .as_ref()
            .expect("WiX contract");
        assert_eq!(
            wix.upgrade_code.expect("fixed upgrade code").to_string(),
            WINDOWS_UPGRADE_CODE
        );
        assert_eq!(wix.version.as_deref(), Some("0.1.2"));

        assert!(raw["bundle"]["windows"].get("nsis").is_none());
        for unassigned_legal_or_signing_field in [
            raw["bundle"].get("publisher"),
            raw["bundle"]["windows"].get("certificateThumbprint"),
            raw["bundle"]["windows"].get("digestAlgorithm"),
            raw["bundle"]["windows"].get("signCommand"),
        ] {
            assert!(
                unassigned_legal_or_signing_field.is_none(),
                "source continuity must not invent publisher or signing authority"
            );
        }
    }

    #[test]
    fn windows_installer_inherits_one_product_version_and_bundles_no_retained_binary() {
        let base = parse(BASE_CONFIG_SOURCE);
        let release = parse(RELEASE_CONFIG_SOURCE);
        let windows = parse(WINDOWS_CONFIG_SOURCE);
        let package = parse(PACKAGE_SOURCE);

        assert_eq!(base["productName"], "LorePia");
        assert_eq!(release["productName"], base["productName"]);
        assert_eq!(base["mainBinaryName"], "lorepia");
        assert_eq!(base["version"], "../package.json");
        assert_eq!(package["version"], env!("CARGO_PKG_VERSION"));
        assert_eq!(windows["identifier"], "dev.lorepia.windows");
        assert!(
            windows.get("productName").is_none() && windows.get("version").is_none(),
            "the Windows overlay must inherit the canonical product name and version"
        );

        for (name, config) in [
            ("base", &base),
            ("release", &release),
            ("windows", &windows),
        ] {
            assert!(
                config["bundle"].get("externalBin").is_none(),
                "{name} config must not package a retained native or rollback binary"
            );
            let rendered = config.to_string().to_ascii_lowercase();
            for retained_source in [
                "apps/windows",
                "apps/android",
                "apps/apple",
                "lorepia-native-reference",
                "native-baseline-before-tauri",
            ] {
                assert!(
                    !rendered.contains(retained_source),
                    "{name} config must not bundle retained source or binaries from {retained_source}"
                );
            }
        }
    }

    #[test]
    fn recovery_overlay_is_a_valid_later_version_in_the_same_msi_family() {
        let mut effective = parse(BASE_CONFIG_SOURCE);
        for overlay in [
            parse(WINDOWS_CONFIG_SOURCE),
            parse(RELEASE_CONFIG_SOURCE),
            serde_json::json!({
                "bundle": {
                    "windows": {
                        "wix": {
                            "version": "0.1.41"
                        }
                    }
                }
            }),
        ] {
            merge_patch(&mut effective, &overlay);
        }

        effective["version"] = serde_json::json!("0.1.0");
        let schema_config: tauri::utils::config::Config = serde_json::from_value(effective)
            .expect("effective recovery config must satisfy Tauri");
        let wix = schema_config
            .bundle
            .windows
            .wix
            .as_ref()
            .expect("effective recovery config must retain WiX");

        assert_eq!(schema_config.product_name.as_deref(), Some("LorePia"));
        assert_eq!(schema_config.version.as_deref(), Some("0.1.0"));
        assert_eq!(schema_config.identifier, "dev.lorepia.windows");
        assert!(!schema_config.bundle.windows.allow_downgrades);
        assert_eq!(
            serde_json::to_value(&schema_config.bundle.targets).expect("serialize bundle targets"),
            serde_json::json!(["msi"])
        );
        assert_eq!(wix.version.as_deref(), Some("0.1.41"));
        assert_eq!(
            wix.upgrade_code
                .expect("recovery must keep the production UpgradeCode")
                .to_string(),
            WINDOWS_UPGRADE_CODE
        );
    }
}
