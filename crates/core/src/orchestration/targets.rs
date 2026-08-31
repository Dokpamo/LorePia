use std::collections::BTreeSet;

use lorepia_domain::{
    ApiFamily, CapabilityKey, CapabilityValue, CoreError, CoreResult, GenerationReasoningEffort,
    GenerationTarget, MessageId, ProviderMessageRole, ResolvedPromptPlan, RoleHint, TaskProfile,
    TaskProfileId, TransformSet, VariableMap,
};
use lorepia_orchestration::verify_resolved_prompt_plan;
use lorepia_providers::parameter_mapping::PromptCacheWireDialect;
use lorepia_providers::{
    DeveloperRoleCapability, ProviderCacheBoundaryDisposition, ProviderCompiledPromptPreview,
    ProviderPromptAdapterContract, ProviderPromptPlacement, ProviderWireRole,
};
use lorepia_storage::PromptResponseLength;
use sha2::{Digest, Sha256};

use crate::{
    Core, Revisioned,
    orchestration_runtime::MemorySemanticQueryEvidence,
    revision::{project_revision, project_revisions},
};

use super::{
    GenerationPlanInput, KnowledgeSemanticBookEvidence, PromptPlanMessagePreview,
    PromptPlanPreview, PromptQuickSettings, orchestration_validation_error,
};

/// Deterministic primary and fallback targets for one auxiliary task.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct TaskGenerationTargetPlan {
    pub task_profile_id: TaskProfileId,
    pub targets: Vec<GenerationTarget>,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct PromptProviderMessagePreview {
    pub sequence: u32,
    pub block_id: lorepia_domain::PromptBlockId,
    pub effective_role: ProviderMessageRole,
    pub wire_role: ProviderWireRole,
    pub placement: ProviderPromptPlacement,
    pub estimated_tokens: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PromptEffectiveMessageContentPreview {
    pub sequence: u32,
    pub block_id: lorepia_domain::PromptBlockId,
    pub block_kind: lorepia_domain::PromptBlockKind,
    pub requested_role: RoleHint,
    pub effective_role: ProviderMessageRole,
    pub estimated_tokens: u32,
    pub source_message_ids: Vec<MessageId>,
    pub content: String,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PromptAppliedParameterPreview {
    pub field: String,
    pub value: serde_json::Value,
}

pub(super) struct PromptProviderResolution {
    pub(super) contract: lorepia_domain::ProviderPromptContract,
    pub(super) adapter: ProviderPromptAdapterContract,
    pub(super) developer_capability: DeveloperRoleCapability,
    pub(super) cache_dialect: PromptCacheWireDialect,
    pub(super) max_context_tokens: u32,
    pub(super) reserved_output_tokens: u32,
    reasoning_effort_applied: Option<GenerationReasoningEffort>,
    request_plan_sha256: String,
    generation_preset_sha256: String,
}

struct PromptProviderWireMetadata {
    developer_capability: DeveloperRoleCapability,
    cache_dialect: PromptCacheWireDialect,
    request_plan_sha256: String,
    generation_preset_sha256: String,
}

impl Core {
    pub(super) fn resolve_prompt_provider_and_warnings(
        &self,
        input: &GenerationPlanInput<'_>,
        quick_settings: &PromptQuickSettings,
        mut warnings: Vec<String>,
    ) -> CoreResult<(PromptProviderResolution, Vec<String>)> {
        let provider = self.prompt_provider_contract(
            input.generation_target,
            input.provider_family,
            quick_settings.max_output_tokens,
            quick_settings.reasoning_effort,
            input.prompt_wire_contract,
        )?;
        if quick_settings.reasoning_effort.is_some()
            && provider.reasoning_effort_applied != quick_settings.reasoning_effort
        {
            warnings.push(
                "reasoning effort quick setting was omitted because the selected route does not expose that exact effort"
                    .to_owned(),
            );
        }
        Ok((provider, warnings))
    }

    pub fn upsert_task_profile(
        &self,
        profile: &TaskProfile,
        expected_revision: Option<u64>,
    ) -> CoreResult<Revisioned<TaskProfile>> {
        self.validate_task_profile(profile)?;
        self.storage()
            .save_task_profile(profile, expected_revision)
            .map(project_revision)
    }

    pub fn get_task_profile(&self, id: &TaskProfileId) -> CoreResult<Revisioned<TaskProfile>> {
        self.storage().get_task_profile(id).map(project_revision)
    }

    pub fn list_task_profiles(&self) -> CoreResult<Vec<Revisioned<TaskProfile>>> {
        self.storage().list_task_profiles().map(project_revisions)
    }

    pub fn delete_task_profile(
        &self,
        id: &TaskProfileId,
        expected_revision: u64,
    ) -> CoreResult<Revisioned<TaskProfile>> {
        self.storage()
            .soft_delete_task_profile(id, expected_revision)
            .map(project_revision)
    }

    /// Resolves a task profile to an ordered, provider-valid target list.
    ///
    /// The explicitly configured route/preset is always first. Each fallback
    /// route contributes its first stored preset in the storage-defined stable
    /// ordering. Missing fallback configuration is rejected before a job is
    /// launched, so background work never silently switches parameters.
    pub fn resolve_task_generation_targets(
        &self,
        id: &TaskProfileId,
    ) -> CoreResult<TaskGenerationTargetPlan> {
        let profile = self.get_task_profile(id)?.value;
        self.validate_task_profile(&profile)?;
        let mut targets = vec![GenerationTarget {
            model_route_id: profile.route_id.clone(),
            generation_preset_id: profile.generation_preset_id,
        }];
        for route_id in profile.fallback_route_ids {
            if targets
                .iter()
                .any(|target| target.model_route_id == route_id)
            {
                continue;
            }
            let preset = self
                .storage()
                .list_generation_presets(&route_id)?
                .into_iter()
                .next()
                .ok_or_else(|| {
                    CoreError::invalid(format!(
                        "task fallback route {} has no generation preset",
                        route_id.as_str()
                    ))
                })?;
            targets.push(GenerationTarget {
                model_route_id: route_id,
                generation_preset_id: preset.id,
            });
        }
        Ok(TaskGenerationTargetPlan {
            task_profile_id: id.clone(),
            targets,
        })
    }

    fn validate_task_profile(&self, profile: &TaskProfile) -> CoreResult<()> {
        if profile.timeout_ms == 0
            || profile.concurrency_limit == 0
            || profile.rate_limit.requests == 0
            || profile.rate_limit.per_seconds == 0
        {
            return Err(CoreError::invalid(
                "task profile timeout, concurrency, and rate limits must be greater than zero",
            ));
        }
        self.storage().get_model_route(&profile.route_id)?;
        let primary_preset = self
            .storage()
            .get_generation_preset(&profile.generation_preset_id)?;
        if primary_preset.model_route_id != profile.route_id {
            return Err(CoreError::invalid(
                "task profile generation preset does not belong to its primary route",
            ));
        }
        let mut seen = std::collections::HashSet::new();
        seen.insert(profile.route_id.clone());
        for route_id in &profile.fallback_route_ids {
            if !seen.insert(route_id.clone()) {
                return Err(CoreError::invalid(
                    "task profile fallback routes must be unique",
                ));
            }
            self.storage().get_model_route(route_id)?;
            if self.storage().list_generation_presets(route_id)?.is_empty() {
                return Err(CoreError::invalid(format!(
                    "task fallback route {} has no generation preset",
                    route_id.as_str()
                )));
            }
        }
        Ok(())
    }

    pub(super) fn prompt_supported_capabilities(
        &self,
        model_route_id: &lorepia_domain::ModelRouteId,
    ) -> CoreResult<Vec<CapabilityKey>> {
        const KEYS: [CapabilityKey; 16] = [
            CapabilityKey::Streaming,
            CapabilityKey::Reasoning,
            CapabilityKey::PromptCaching,
            CapabilityKey::ToolCalling,
            CapabilityKey::ParallelToolCalling,
            CapabilityKey::StructuredOutput,
            CapabilityKey::JsonMode,
            CapabilityKey::ImageInput,
            CapabilityKey::AudioInput,
            CapabilityKey::AudioOutput,
            CapabilityKey::Logprobs,
            CapabilityKey::Seed,
            CapabilityKey::Batch,
            CapabilityKey::Background,
            CapabilityKey::ContextWindow,
            CapabilityKey::MaxOutputTokens,
        ];
        let mut supported = Vec::new();
        for key in KEYS {
            let Some(capability) = self.effective_capability(model_route_id, key)? else {
                continue;
            };
            if capability.has_conflict || capability.selected_is_stale {
                continue;
            }
            if matches!(
                capability.selected.status,
                lorepia_domain::SupportStatus::Unsupported | lorepia_domain::SupportStatus::Unknown
            ) || matches!(capability.selected.value, CapabilityValue::Boolean(false))
            {
                continue;
            }
            supported.push(key);
        }
        Ok(supported)
    }

    fn prompt_provider_contract(
        &self,
        target: Option<&GenerationTarget>,
        family: Option<ApiFamily>,
        max_output_tokens: Option<u32>,
        requested_reasoning_effort: Option<GenerationReasoningEffort>,
        supplied_wire_contract: Option<&crate::app::PromptRouteWireContract>,
    ) -> CoreResult<PromptProviderResolution> {
        let owned_wire_contract = self.resolve_owned_prompt_wire_contract(
            target,
            supplied_wire_contract,
            requested_reasoning_effort,
        )?;
        let wire_contract = supplied_wire_contract.or(owned_wire_contract.as_ref());
        if wire_contract.is_some_and(|contract| {
            contract.reasoning_effort_applied != requested_reasoning_effort
                && contract.reasoning_effort_applied.is_some()
        }) {
            return Err(CoreError::internal(
                "provider snapshot reasoning overlay does not match prompt quick settings",
            ));
        }
        let family = resolve_prompt_provider_family(family, wire_contract)?;
        let (max_context_tokens, reserved_output_tokens) =
            prompt_provider_token_limits(wire_contract, max_output_tokens)?;
        let metadata = prompt_provider_wire_metadata(family, wire_contract);
        let adapter = ProviderPromptAdapterContract::for_family(family)
            .with_context_limit_tokens(Some(max_context_tokens))
            .map_err(orchestration_validation_error)?;
        let mut contract = adapter.resolution_contract(metadata.developer_capability);
        contract.supports_explicit_cache = matches!(
            metadata.cache_dialect,
            PromptCacheWireDialect::Anthropic {
                supports_explicit_breakpoints: true,
                ..
            }
        );
        contract.max_cache_boundaries = if contract.supports_explicit_cache {
            4
        } else {
            0
        };
        Ok(PromptProviderResolution {
            contract,
            adapter,
            developer_capability: metadata.developer_capability,
            cache_dialect: metadata.cache_dialect,
            max_context_tokens,
            reserved_output_tokens,
            reasoning_effort_applied: wire_contract
                .and_then(|contract| contract.reasoning_effort_applied),
            request_plan_sha256: metadata.request_plan_sha256,
            generation_preset_sha256: metadata.generation_preset_sha256,
        })
    }

    fn resolve_owned_prompt_wire_contract(
        &self,
        target: Option<&GenerationTarget>,
        supplied: Option<&crate::app::PromptRouteWireContract>,
        reasoning_effort: Option<GenerationReasoningEffort>,
    ) -> CoreResult<Option<crate::app::PromptRouteWireContract>> {
        match (target, supplied) {
            (Some(_), Some(_)) | (None, None) => Ok(None),
            (Some(target), None) => crate::app::prompt_route_wire_contract_with_reasoning_effort(
                self,
                target,
                reasoning_effort,
            )
            .map(Some),
            (None, Some(_)) => Err(CoreError::internal(
                "legacy provider cannot carry a catalog route contract",
            )),
        }
    }
}

fn resolve_prompt_provider_family(
    family: Option<ApiFamily>,
    wire_contract: Option<&crate::app::PromptRouteWireContract>,
) -> CoreResult<ApiFamily> {
    match (family, wire_contract) {
        (Some(family), Some(contract)) if family != contract.api_family => Err(
            CoreError::internal("provider snapshot API family does not match prompt preparation"),
        ),
        (Some(family), _) => Ok(family),
        (None, Some(contract)) => Ok(contract.api_family),
        (None, None) => Ok(ApiFamily::OpenAiChatCompletions),
    }
}

fn prompt_provider_token_limits(
    wire_contract: Option<&crate::app::PromptRouteWireContract>,
    max_output_tokens: Option<u32>,
) -> CoreResult<(u32, u32)> {
    let max_context_tokens = wire_contract
        .and_then(|contract| contract.context_limit_tokens)
        .unwrap_or(8_192);
    let requested_output_tokens = max_output_tokens
        .or_else(|| wire_contract.and_then(|contract| contract.configured_max_output_tokens))
        .unwrap_or(4_096);
    let reserved_output_tokens = wire_contract
        .and_then(|contract| contract.observed_max_output_tokens)
        .map_or(requested_output_tokens, |limit| {
            requested_output_tokens.min(limit)
        });
    if reserved_output_tokens >= max_context_tokens {
        return Err(CoreError::invalid(
            "reserved output tokens must be smaller than the model context limit",
        ));
    }
    Ok((max_context_tokens, reserved_output_tokens))
}

fn prompt_provider_wire_metadata(
    family: ApiFamily,
    wire_contract: Option<&crate::app::PromptRouteWireContract>,
) -> PromptProviderWireMetadata {
    wire_contract.map_or_else(
        || PromptProviderWireMetadata {
            developer_capability: match family {
                ApiFamily::OpenAiResponses => DeveloperRoleCapability::Supported,
                ApiFamily::OpenAiChatCompletions => DeveloperRoleCapability::Unknown,
                ApiFamily::AnthropicMessages
                | ApiFamily::GeminiGenerateContent
                | ApiFamily::OllamaNative => DeveloperRoleCapability::Unsupported,
            },
            cache_dialect: PromptCacheWireDialect::Unsupported,
            request_plan_sha256: "legacy-provider-request-plan".to_owned(),
            generation_preset_sha256: "legacy-generation-preset".to_owned(),
        },
        |contract| PromptProviderWireMetadata {
            developer_capability: contract.developer_capability,
            cache_dialect: contract.cache_dialect,
            request_plan_sha256: contract.request_plan_sha256.clone(),
            generation_preset_sha256: contract.generation_preset_sha256.clone(),
        },
    )
}
pub(super) fn redacted_prompt_preview(
    plan: &ResolvedPromptPlan,
    execution_hash: &str,
    prompt_preset_revision: u64,
    prompt_preset_revision_id: &str,
    generation_target: Option<GenerationTarget>,
    provider: &ProviderCompiledPromptPreview,
    preparation_warnings: &[String],
) -> CoreResult<PromptPlanPreview> {
    verify_resolved_prompt_plan(plan).map_err(orchestration_validation_error)?;
    let mut warnings = plan.trace.warnings.clone();
    warnings.extend_from_slice(preparation_warnings);
    for boundary in &provider.cache_boundaries {
        if let ProviderCacheBoundaryDisposition::Ignored { warning } = boundary.disposition {
            warnings.push(format!(
                "provider ignored cache boundary {}: {warning:?}",
                boundary.boundary_id.as_str()
            ));
        }
    }
    Ok(PromptPlanPreview {
        plan_id: execution_hash.to_owned(),
        plan_hash: execution_hash.to_owned(),
        neutral_plan_hash: plan.plan_hash.clone(),
        prompt_preset_id: plan.preset_id.clone(),
        prompt_preset_revision,
        prompt_preset_revision_id: prompt_preset_revision_id.to_owned(),
        generation_target,
        estimated_input_tokens: plan.trace.estimated_input_tokens,
        available_input_tokens: plan.trace.available_input_tokens,
        token_estimator_id: plan.trace.estimator_id.clone(),
        token_estimate_exact: false,
        messages: plan
            .effective_messages
            .iter()
            .map(|message| PromptPlanMessagePreview {
                sequence: message.sequence,
                block_id: message.block_id.clone(),
                block_kind: message.block_kind,
                requested_role: message.requested_role,
                effective_role: message.effective_role,
                estimated_tokens: message.estimated_tokens,
                source_message_ids: message.source_message_ids.clone(),
            })
            .collect(),
        provider_family: provider.family,
        provider_messages: provider
            .messages
            .iter()
            .map(|message| PromptProviderMessagePreview {
                sequence: message.sequence,
                block_id: message.block_id.clone(),
                effective_role: message.effective_role,
                wire_role: message.wire_role,
                placement: message.placement,
                estimated_tokens: message.estimated_tokens,
            })
            .collect(),
        provider_cache_boundaries: provider.cache_boundaries.clone(),
        cache_directives: plan.cache_directives.clone(),
        blocks: plan.trace.blocks.clone(),
        role_mappings: plan.trace.role_mappings.clone(),
        overflow: plan.trace.overflow.clone(),
        warnings,
    })
}

pub(super) fn provider_cacheable_prefix_tokens(provider: &ProviderCompiledPromptPreview) -> u32 {
    let last_applied_sequence = provider
        .cache_boundaries
        .iter()
        .filter(|boundary| {
            matches!(
                boundary.disposition,
                ProviderCacheBoundaryDisposition::Mapped { .. }
            )
        })
        .filter_map(|boundary| boundary.after_message_sequence)
        .max();
    last_applied_sequence.map_or(0, |last| {
        provider
            .messages
            .iter()
            .filter(|message| message.sequence <= last)
            .map(|message| message.estimated_tokens)
            .fold(0_u32, u32::saturating_add)
    })
}

pub(super) fn cacheable_prefix_has_volatile_before_fixed_after(
    plan: &ResolvedPromptPlan,
    provider: &ProviderCompiledPromptPreview,
) -> bool {
    let Some(last_applied_sequence) = provider
        .cache_boundaries
        .iter()
        .filter(|boundary| {
            matches!(
                boundary.disposition,
                ProviderCacheBoundaryDisposition::Mapped { .. }
            )
        })
        .filter_map(|boundary| boundary.after_message_sequence)
        .max()
    else {
        return false;
    };
    let volatile_before = plan.effective_messages.iter().any(|message| {
        message.sequence <= last_applied_sequence && prompt_block_is_volatile(message.block_kind)
    });
    let fixed_after = plan.effective_messages.iter().any(|message| {
        message.sequence > last_applied_sequence && !prompt_block_is_volatile(message.block_kind)
    });
    volatile_before && fixed_after
}

const fn prompt_block_is_volatile(kind: lorepia_domain::PromptBlockKind) -> bool {
    matches!(
        kind,
        lorepia_domain::PromptBlockKind::WorldKnowledge
            | lorepia_domain::PromptBlockKind::RetrievedMemory
            | lorepia_domain::PromptBlockKind::ConversationSummary
            | lorepia_domain::PromptBlockKind::HistorySlice
            | lorepia_domain::PromptBlockKind::LatestUserTurn
            | lorepia_domain::PromptBlockKind::AuthorNote
            | lorepia_domain::PromptBlockKind::AssistantPrefill
            | lorepia_domain::PromptBlockKind::GroupContext
    )
}

pub(super) fn canonical_prompt_capabilities(
    capabilities: Vec<CapabilityKey>,
) -> CoreResult<Vec<CapabilityKey>> {
    let mut keyed = capabilities
        .into_iter()
        .map(|capability| {
            serde_json::to_string(&capability)
                .map(|key| (key, capability))
                .map_err(|error| {
                    CoreError::internal(format!("prompt capability cannot be encoded: {error}"))
                })
        })
        .collect::<CoreResult<Vec<_>>>()?;
    keyed.sort_by(|left, right| left.0.cmp(&right.0));
    keyed.dedup_by(|left, right| left.0 == right.0);
    Ok(keyed
        .into_iter()
        .map(|(_, capability)| capability)
        .collect())
}

#[allow(clippy::too_many_arguments)]
pub(super) fn prompt_execution_hash(
    plan: &ResolvedPromptPlan,
    prompt_preset_revision_id: &str,
    generation_target: Option<&GenerationTarget>,
    provider: &PromptProviderResolution,
    provider_preview: &ProviderCompiledPromptPreview,
    temperature: Option<f64>,
    response_length: PromptResponseLength,
    creativity: u8,
    requested_reasoning_effort: Option<GenerationReasoningEffort>,
    memory_enabled: bool,
    knowledge_enabled: bool,
    variables: &VariableMap,
    transform_sets: &[TransformSet],
    module_plan_sha256: Option<&str>,
    approved_import_source_ids: &BTreeSet<String>,
    memory_semantic_evidence: Option<&MemorySemanticQueryEvidence>,
    knowledge_semantic_evidence: &[KnowledgeSemanticBookEvidence],
) -> CoreResult<String> {
    #[derive(serde::Serialize)]
    struct ExecutionIdentity<'a> {
        schema_version: u32,
        neutral_plan_hash: &'a str,
        prompt_preset_revision_id: &'a str,
        generation_target: Option<&'a GenerationTarget>,
        provider_family: ApiFamily,
        developer_capability: DeveloperRoleCapability,
        cache_dialect: PromptCacheWireDialect,
        request_plan_sha256: &'a str,
        generation_preset_sha256: &'a str,
        context_limit_tokens: u32,
        reserved_output_tokens: u32,
        temperature: Option<f64>,
        response_length: PromptResponseLength,
        creativity: u8,
        requested_reasoning_effort: Option<GenerationReasoningEffort>,
        reasoning_effort_applied: Option<GenerationReasoningEffort>,
        memory_enabled: bool,
        knowledge_enabled: bool,
        variables: &'a VariableMap,
        transform_sets: &'a [TransformSet],
        module_plan_sha256: Option<&'a str>,
        approved_import_source_ids: &'a BTreeSet<String>,
        provider_preview: &'a ProviderCompiledPromptPreview,
        memory_semantic_evidence: Option<&'a MemorySemanticQueryEvidence>,
        knowledge_semantic_evidence: &'a [KnowledgeSemanticBookEvidence],
    }

    let encoded = serde_json::to_vec(&ExecutionIdentity {
        schema_version: 1,
        neutral_plan_hash: &plan.plan_hash,
        prompt_preset_revision_id,
        generation_target,
        provider_family: provider.adapter.family(),
        developer_capability: provider.developer_capability,
        cache_dialect: provider.cache_dialect,
        request_plan_sha256: &provider.request_plan_sha256,
        generation_preset_sha256: &provider.generation_preset_sha256,
        context_limit_tokens: provider.max_context_tokens,
        reserved_output_tokens: provider.reserved_output_tokens,
        temperature,
        response_length,
        creativity,
        requested_reasoning_effort,
        reasoning_effort_applied: provider.reasoning_effort_applied,
        memory_enabled,
        knowledge_enabled,
        variables,
        transform_sets,
        module_plan_sha256,
        approved_import_source_ids,
        provider_preview,
        memory_semantic_evidence,
        knowledge_semantic_evidence,
    })
    .map_err(|error| {
        CoreError::internal(format!("cannot encode prompt execution identity: {error}"))
    })?;
    Ok(format!("{:x}", Sha256::digest(encoded)))
}
