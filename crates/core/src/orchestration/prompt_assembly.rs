use super::{
    GenerationPlanInput, KnowledgeSemanticBookEvidence, PromptContextMaterialization,
    PromptModuleOverlay, PromptPersonaMaterialization, PromptPlanPreview, PromptPresetPreparation,
    PromptProviderResolution, PromptQuickSettings, PromptTransformPreparation, PromptVariableState,
    apply_resolved_prompt_transforms, cacheable_prefix_has_volatile_before_fixed_after,
    orchestration_validation_error, prompt_execution_hash, provider_cacheable_prefix_tokens,
    redacted_prompt_preview, validate_prompt_binding_sources,
};
use crate::{
    Core,
    orchestration_runtime::{MemorySemanticQueryEvidence, ResolvedMemorySemanticQuery},
};
use chrono::{DateTime, Utc};
use lorepia_chat::{MaterializedPromptPlan, PromptPlanner};
use lorepia_domain::{
    CapabilityKey, Character, CharacterContentV1, CharacterPromptContent, ConversationBranchId,
    ConversationId, CoreError, CoreResult, GenerationId, GenerationTarget, KnowledgeBook,
    KnowledgeEntryId, MemoryProfile, Message, MessageId, MessageRole, PromptContextBindingEvidence,
    PromptContextSnapshotV1, PromptConversationMessage, PromptMemorySelectionEvidence,
    PromptMessageRole, PromptPreset, PromptResolutionContext, PromptResolveRequest,
    ResolvedPromptPlan, SelectedKnowledge, SelectedMemory, TransformSet, TransformSetId,
    VariableMap, VersionedJson, prompt_context_snapshot_sha256, prompt_local_user_id_sha256,
};
use lorepia_orchestration::{
    KnowledgeWorkBudget, TransformResult, reseal_prompt_resolution_evidence,
    resolve_prompt_plan as resolve_prompt_plan_engine, verify_resolved_prompt_plan,
};
use lorepia_providers::ProviderCompiledPromptPreview;
use lorepia_storage::{
    GenerationPromptPlanRecord, KnowledgeActivationLog, ObjectRevision, PromptPresetBinding,
    ProviderRequestSnapshotRecord, StoredRevision,
};
use std::collections::{BTreeMap, BTreeSet};

pub(crate) struct PreparedGenerationPlan {
    pub materialized: MaterializedPromptPlan,
    pub preview: PromptPlanPreview,
    pub prompt_preset_revision_id: String,
    pub execution_hash: String,
    pub transform_sets: Vec<TransformSet>,
    /// Exact immutable set revisions used by every transform phase. This
    /// content-free map is sealed into the generation prompt-plan diagnostics
    /// so terminal application logs can satisfy their revision foreign keys
    /// without consulting a newer active revision.
    pub transform_set_revisions: BTreeMap<TransformSetId, String>,
    pub variables: VariableMap,
    pub supported_capabilities: Vec<CapabilityKey>,
    pub knowledge_logs: Vec<KnowledgeActivationLog>,
    pub approved_import_source_ids: BTreeSet<String>,
    pub display_context: PromptResolutionContext,
    pub module_plan_sha256: Option<String>,
    pub cacheable_prefix_tokens: u32,
    pub tokenizer_id: String,
    pub tokenizer_version: String,
    pub memory_semantic_evidence: Option<MemorySemanticQueryEvidence>,
    pub knowledge_semantic_evidence: Vec<KnowledgeSemanticBookEvidence>,
}

struct PromptConversationPreparation {
    character_content: CharacterContentV1,
    prompt_character: CharacterPromptContent,
    prompt_messages: Vec<PromptConversationMessage>,
    scan_texts: Vec<String>,
}

pub(super) struct PromptSelectionInput<'a> {
    pub(super) preset: &'a PromptPreset,
    character_content: &'a CharacterContentV1,
    prompt_knowledge_books: &'a [ObjectRevision<KnowledgeBook>],
    module_knowledge_books: &'a [ObjectRevision<KnowledgeBook>],
    exact_character_knowledge_book: Option<&'a StoredRevision<KnowledgeBook>>,
    pub(super) memory_profile: Option<&'a ObjectRevision<MemoryProfile>>,
    pub(super) conversation_id: &'a ConversationId,
    pub(super) branch_id: &'a ConversationBranchId,
    pub(super) memory_lineage_branch_id: Option<&'a ConversationBranchId>,
    pub(super) memory_context_head_message_id: Option<&'a MessageId>,
    pub(super) generation_attempt_id: Option<&'a GenerationId>,
    pub(super) prompt_messages: &'a [PromptConversationMessage],
    scan_texts: &'a [String],
    manually_active_knowledge: &'a BTreeSet<KnowledgeEntryId>,
    variables: &'a VariableMap,
    supported_capabilities: &'a [CapabilityKey],
    pub(super) resolved_memory_semantics: Option<&'a ResolvedMemorySemanticQuery>,
    activation_seed: u64,
    resolution_time: DateTime<Utc>,
    knowledge_enabled: bool,
    memory_enabled: bool,
}

struct PromptSelectionPreparation {
    selected_knowledge: Vec<SelectedKnowledge>,
    knowledge_logs: Vec<KnowledgeActivationLog>,
    knowledge_semantic_evidence: Vec<KnowledgeSemanticBookEvidence>,
    selected_memory: Vec<SelectedMemory>,
    memory_evidence: Vec<PromptMemorySelectionEvidence>,
    warnings: Vec<String>,
}

struct PromptPlanSources {
    preset: PromptPresetPreparation,
    quick_settings: PromptQuickSettings,
    variables: PromptVariableState,
    transforms: PromptTransformPreparation,
    conversation: PromptConversationPreparation,
    selection: PromptSelectionPreparation,
}

struct PromptPlanAssembly {
    request: PromptResolveRequest,
    provider_resolution: PromptProviderResolution,
    prompt_preset_revision: u64,
    prompt_preset_revision_id: String,
    block_source_revisions: BTreeMap<lorepia_domain::PromptBlockId, String>,
    quick_settings: PromptQuickSettings,
    transform_sets: Vec<TransformSet>,
    transform_set_revisions: BTreeMap<TransformSetId, String>,
    approved_import_source_ids: BTreeSet<String>,
    variables: VariableMap,
    supported_capabilities: Vec<CapabilityKey>,
    knowledge_logs: Vec<KnowledgeActivationLog>,
    knowledge_semantic_evidence: Vec<KnowledgeSemanticBookEvidence>,
    memory_evidence: Vec<PromptMemorySelectionEvidence>,
    module_overlay: PromptModuleOverlay,
    preparation_warnings: Vec<String>,
}

struct ResolvedPromptAssembly {
    plan: ResolvedPromptPlan,
    provider_preview: ProviderCompiledPromptPreview,
    execution_hash: String,
    cacheable_prefix_tokens: u32,
}

impl PreparedGenerationPlan {
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn generation_prompt_plan_record(
        &self,
        generation_id: GenerationId,
        conversation_id: ConversationId,
        branch_id: ConversationBranchId,
        head_message_id: Option<MessageId>,
        latest_user_message_id: MessageId,
        generation_target: Option<&GenerationTarget>,
        provider_request_value: serde_json::Value,
        created_at: DateTime<Utc>,
    ) -> CoreResult<GenerationPromptPlanRecord> {
        let plan = self
            .materialized
            .request
            .resolved_prompt_plan
            .as_ref()
            .ok_or_else(|| CoreError::internal("prepared generation is missing its prompt plan"))?;
        verify_resolved_prompt_plan(plan).map_err(orchestration_validation_error)?;
        let plan_value = serde_json::to_value(plan).map_err(|error| {
            CoreError::internal(format!("cannot encode resolved prompt plan: {error}"))
        })?;
        let diagnostics_value = serde_json::json!({
            "execution_hash": &self.execution_hash,
            "neutral_plan_hash": &plan.plan_hash,
            "provider_family": self.preview.provider_family,
            "provider_messages": &self.preview.provider_messages,
            "provider_cache_boundaries": &self.preview.provider_cache_boundaries,
            "module_plan_sha256": &self.module_plan_sha256,
            "transform_set_revisions": self.transform_set_revisions.iter().map(
                |(set_id, revision_id)| serde_json::json!({
                    "set_id": set_id.as_str(),
                    "revision_id": revision_id,
                })
            ).collect::<Vec<_>>(),
            "memory_semantic_evidence": &self.memory_semantic_evidence,
            "knowledge_semantic_evidence": &self.knowledge_semantic_evidence,
            "cacheable_prefix_tokens": self.cacheable_prefix_tokens,
            "warnings": &self.preview.warnings,
        });
        Ok(GenerationPromptPlanRecord {
            id: self.execution_hash.clone(),
            generation_id: generation_id.clone(),
            conversation_id,
            branch_id,
            head_message_id,
            latest_user_message_id,
            prompt_preset_id: plan.preset_id.clone(),
            prompt_preset_revision_id: self.prompt_preset_revision_id.clone(),
            model_route_id: generation_target.map(|target| target.model_route_id.clone()),
            generation_preset_id: generation_target
                .map(|target| target.generation_preset_id.clone()),
            task_profile_revision_id: None,
            random_seed: plan.trace.session_seed,
            tokenizer_id: self.tokenizer_id.clone(),
            tokenizer_version: self.tokenizer_version.clone(),
            plan: VersionedJson {
                schema_version: plan.schema_version,
                value: plan_value,
            },
            plan_sha256: plan.plan_hash.clone(),
            input_fingerprint_sha256: self.execution_hash.clone(),
            context_limit_tokens: plan.trace.max_context_tokens,
            estimated_input_tokens: plan.trace.estimated_input_tokens,
            reserved_output_tokens: plan.trace.reserved_output_tokens,
            final_input_tokens: plan.trace.estimated_input_tokens,
            cacheable_prefix_tokens: self.cacheable_prefix_tokens,
            provider_request: ProviderRequestSnapshotRecord {
                id: format!("provider-request:{}", generation_id.0),
                api_family: self.preview.provider_family,
                request_schema_version: 1,
                request: VersionedJson {
                    schema_version: 1,
                    value: provider_request_value,
                },
                mapping_diagnostics: VersionedJson {
                    schema_version: 1,
                    value: diagnostics_value,
                },
                created_at,
            },
            created_at,
        })
    }
}

impl Core {
    pub(super) fn prepare_generation_plan_with_memory(
        &self,
        input: GenerationPlanInput<'_>,
        resolved_memory_semantics: Option<&ResolvedMemorySemanticQuery>,
        knowledge_work_budget: &mut KnowledgeWorkBudget,
    ) -> CoreResult<PreparedGenerationPlan> {
        let activation_seed = input.session_seed.ok_or_else(|| {
            CoreError::internal("generation prompt resolution is missing its attempt-owned seed")
        })?;
        let latest = input
            .history
            .last()
            .filter(|message| message.role == MessageRole::User)
            .ok_or_else(|| CoreError::invalid("prompt history must end with a user message"))?;
        if input
            .history
            .iter()
            .any(|message| message.conversation_id != *input.conversation_id)
        {
            return Err(CoreError::invalid(
                "prompt history contains a message from another conversation",
            ));
        }
        let sources = self.prepare_prompt_plan_sources(
            &input,
            resolved_memory_semantics,
            latest,
            activation_seed,
            knowledge_work_budget,
        )?;
        let mut assembly = self.assemble_prompt_plan(&input, latest, sources)?;
        let resolved =
            Self::resolve_prompt_plan_assembly(&input, resolved_memory_semantics, &mut assembly)?;
        Self::materialize_prompt_plan_assembly(
            &input,
            resolved_memory_semantics,
            assembly,
            resolved,
        )
    }

    fn prepare_prompt_plan_sources(
        &self,
        input: &GenerationPlanInput<'_>,
        resolved_memory_semantics: Option<&ResolvedMemorySemanticQuery>,
        latest: &Message,
        activation_seed: u64,
        knowledge_work_budget: &mut KnowledgeWorkBudget,
    ) -> CoreResult<PromptPlanSources> {
        let mut preset = self.prepare_prompt_preset_sources(input)?;
        let binding = preset.binding.as_ref().map(|stored| &stored.value);
        let quick_settings = self.resolve_prompt_quick_settings(binding, input)?;
        preset
            .warnings
            .extend(quick_settings.warnings.iter().cloned());
        let variables = self.resolve_prompt_variable_state(
            &preset.preset,
            binding,
            &preset.module_overlay,
            input,
        )?;
        let transforms = self.prepare_prompt_transforms(
            &preset.prompt_transform_sets,
            &preset.module_overlay,
            input,
            latest,
            &variables.variables,
        )?;
        let conversation =
            self.prepare_prompt_conversation(input, latest, &transforms.transformed_latest)?;
        let selection = self.prepare_prompt_selections(
            PromptSelectionInput {
                preset: &preset.preset,
                character_content: &conversation.character_content,
                prompt_knowledge_books: &preset.prompt_knowledge_books,
                module_knowledge_books: &preset.module_overlay.knowledge_books,
                exact_character_knowledge_book: input
                    .prompt_selection_authority
                    .and_then(|authority| authority.character_knowledge_book.as_ref()),
                memory_profile: preset.prompt_memory_profile.as_ref(),
                conversation_id: input.conversation_id,
                branch_id: input.branch_id,
                memory_lineage_branch_id: input.memory_lineage_branch_id,
                memory_context_head_message_id: input.context_head_message_id,
                generation_attempt_id: input.generation_attempt_id,
                prompt_messages: &conversation.prompt_messages,
                scan_texts: &conversation.scan_texts,
                manually_active_knowledge: &variables.manually_active_knowledge,
                variables: &variables.variables,
                supported_capabilities: &transforms.supported_capabilities,
                resolved_memory_semantics,
                activation_seed,
                resolution_time: input.resolution_time,
                knowledge_enabled: quick_settings.knowledge_enabled,
                memory_enabled: quick_settings.memory_enabled,
            },
            knowledge_work_budget,
        )?;
        preset.warnings.extend(selection.warnings.iter().cloned());
        Ok(PromptPlanSources {
            preset,
            quick_settings,
            variables,
            transforms,
            conversation,
            selection,
        })
    }

    fn assemble_prompt_plan(
        &self,
        input: &GenerationPlanInput<'_>,
        latest: &Message,
        sources: PromptPlanSources,
    ) -> CoreResult<PromptPlanAssembly> {
        let PromptPlanSources {
            preset,
            quick_settings,
            variables,
            transforms,
            conversation,
            selection,
        } = sources;
        let prompt_context = self.materialize_prompt_context_sources(
            &preset.preset,
            preset.binding.as_ref(),
            preset.prompt_persona.as_ref(),
            input.conversation_id,
            input.branch_id,
            input.context_source_branch_id,
            input.context_head_message_id,
            latest.parent_id.as_ref(),
            &conversation.prompt_messages,
            input.generation_attempt_id,
            input
                .prompt_selection_authority
                .map(|authority| authority.local_user_id_sha256.as_str()),
        )?;
        let (provider_resolution, preparation_warnings) =
            self.resolve_prompt_provider_and_warnings(input, &quick_settings, preset.warnings)?;
        let PromptSelectionPreparation {
            selected_knowledge,
            knowledge_logs,
            knowledge_semantic_evidence,
            selected_memory,
            memory_evidence,
            ..
        } = selection;
        let PromptConversationPreparation {
            prompt_character,
            prompt_messages,
            ..
        } = conversation;
        let persona = preset
            .prompt_persona
            .as_ref()
            .map(|materialized| materialized.content.clone());
        let request = PromptResolveRequest {
            preset: preset.preset,
            context: PromptResolutionContext {
                conversation_id: input.conversation_id.clone(),
                branch_id: input.branch_id.clone(),
                character: prompt_character,
                persona,
                user_name: prompt_context.user_name,
                messages: prompt_messages,
                latest_user_message_id: latest.id.clone(),
                selected_knowledge,
                selected_memory,
                summary_boundaries: prompt_context.summaries.boundaries,
                conversation_summary: prompt_context.summaries.conversation_summary,
                author_note: prompt_context.author_note,
                group_context: prompt_context.group_context,
                variables: variables.variables.clone(),
                slots: prompt_context.slots,
                current_date: input.resolution_time.format("%Y-%m-%d").to_string(),
                current_time: input.resolution_time.format("%H:%M:%S%:z").to_string(),
                supported_capabilities: transforms.supported_capabilities.clone(),
                session_seed: input.session_seed,
                context_snapshot: Some(prompt_context.snapshot),
            },
            provider: provider_resolution.contract.clone(),
            generation_preset_id: input
                .generation_target
                .map(|target| target.generation_preset_id.clone()),
            max_context_tokens: provider_resolution.max_context_tokens,
            reserved_output_tokens: provider_resolution.reserved_output_tokens,
        };
        Ok(PromptPlanAssembly {
            request,
            provider_resolution,
            prompt_preset_revision: preset.revision,
            prompt_preset_revision_id: preset.revision_id,
            block_source_revisions: preset.block_source_revisions,
            quick_settings,
            transform_sets: transforms.transform_sets,
            transform_set_revisions: transforms.transform_set_revisions,
            approved_import_source_ids: transforms.approved_import_source_ids,
            variables: variables.variables,
            supported_capabilities: transforms.supported_capabilities,
            knowledge_logs,
            knowledge_semantic_evidence,
            memory_evidence,
            module_overlay: preset.module_overlay,
            preparation_warnings,
        })
    }

    fn resolve_prompt_plan_assembly(
        input: &GenerationPlanInput<'_>,
        resolved_memory_semantics: Option<&ResolvedMemorySemanticQuery>,
        assembly: &mut PromptPlanAssembly,
    ) -> CoreResult<ResolvedPromptAssembly> {
        let plan = resolve_prompt_plan_engine(&assembly.request)
            .map_err(orchestration_validation_error)?;
        let plan = reseal_prompt_resolution_evidence(
            &plan,
            &assembly.block_source_revisions,
            &assembly.memory_evidence,
        )
        .map_err(orchestration_validation_error)?;
        let (plan, transform_warnings) = apply_resolved_prompt_transforms(
            &plan,
            &assembly.transform_sets,
            &assembly.variables,
            &assembly.supported_capabilities,
            &assembly.approved_import_source_ids,
        )?;
        assembly.preparation_warnings.extend(transform_warnings);
        verify_resolved_prompt_plan(&plan).map_err(orchestration_validation_error)?;
        let provider_preview = assembly
            .provider_resolution
            .adapter
            .compile_resolved_plan(
                &plan,
                assembly.provider_resolution.developer_capability,
                assembly.provider_resolution.cache_dialect,
            )
            .map_err(|error| {
                CoreError::invalid(format!(
                    "resolved prompt cannot be represented by the selected provider route: {error}"
                ))
            })?
            .preview();
        let cacheable_prefix_tokens = provider_cacheable_prefix_tokens(&provider_preview);
        if cacheable_prefix_has_volatile_before_fixed_after(&plan, &provider_preview) {
            assembly.preparation_warnings.push(
                "cache boundary has volatile prompt content before fixed content; moving fixed blocks earlier may improve cache reuse"
                    .to_owned(),
            );
        }
        let memory_semantic_evidence = resolved_memory_semantics.map(|resolved| &resolved.evidence);
        let execution_hash = prompt_execution_hash(
            &plan,
            &assembly.prompt_preset_revision_id,
            input.generation_target,
            &assembly.provider_resolution,
            &provider_preview,
            assembly.quick_settings.temperature,
            assembly.quick_settings.response_length,
            assembly.quick_settings.creativity,
            assembly.quick_settings.reasoning_effort,
            assembly.quick_settings.memory_enabled,
            assembly.quick_settings.knowledge_enabled,
            &assembly.variables,
            &assembly.transform_sets,
            assembly.module_overlay.plan_sha256.as_deref(),
            &assembly.approved_import_source_ids,
            memory_semantic_evidence,
            &assembly.knowledge_semantic_evidence,
        )?;
        if input
            .expected_plan_hash
            .is_some_and(|expected| expected != execution_hash)
        {
            return Err(CoreError::invalid(
                "prompt plan changed after preview; resolve a new preview before sending",
            ));
        }
        Ok(ResolvedPromptAssembly {
            plan,
            provider_preview,
            execution_hash,
            cacheable_prefix_tokens,
        })
    }

    fn materialize_prompt_plan_assembly(
        input: &GenerationPlanInput<'_>,
        resolved_memory_semantics: Option<&ResolvedMemorySemanticQuery>,
        assembly: PromptPlanAssembly,
        resolved: ResolvedPromptAssembly,
    ) -> CoreResult<PreparedGenerationPlan> {
        let mut display_context = assembly.request.context.clone();
        display_context.selected_knowledge.clear();
        display_context.selected_memory.clear();
        display_context.context_snapshot = None;
        let preview = redacted_prompt_preview(
            &resolved.plan,
            &resolved.execution_hash,
            assembly.prompt_preset_revision,
            &assembly.prompt_preset_revision_id,
            input.generation_target.cloned(),
            &resolved.provider_preview,
            &assembly.preparation_warnings,
        )?;
        let tokenizer_id = resolved.plan.trace.estimator_id.clone();
        let provider_execution_hash = resolved.provider_preview.execution_hash.clone();
        let mut materialized = PromptPlanner::materialize_resolved_plan(
            input.conversation_id.clone(),
            resolved.plan,
            input.model,
            assembly.quick_settings.temperature,
            assembly.quick_settings.max_output_tokens,
        )?;
        materialized.request.provider_execution_plan_hash = Some(provider_execution_hash);
        Ok(PreparedGenerationPlan {
            materialized,
            preview,
            prompt_preset_revision_id: assembly.prompt_preset_revision_id,
            execution_hash: resolved.execution_hash,
            transform_sets: assembly.transform_sets,
            transform_set_revisions: assembly.transform_set_revisions,
            variables: assembly.variables,
            supported_capabilities: assembly.supported_capabilities,
            knowledge_logs: assembly.knowledge_logs,
            approved_import_source_ids: assembly.approved_import_source_ids,
            display_context,
            module_plan_sha256: assembly.module_overlay.plan_sha256,
            cacheable_prefix_tokens: resolved.cacheable_prefix_tokens,
            tokenizer_id,
            tokenizer_version: "fallback-inexact-v1".to_owned(),
            memory_semantic_evidence: resolved_memory_semantics
                .map(|resolved| resolved.evidence.clone()),
            knowledge_semantic_evidence: assembly.knowledge_semantic_evidence,
        })
    }

    fn prepare_prompt_selections(
        &self,
        input: PromptSelectionInput<'_>,
        knowledge_work_budget: &mut KnowledgeWorkBudget,
    ) -> CoreResult<PromptSelectionPreparation> {
        let mut warnings = Vec::new();
        let (selected_knowledge, knowledge_logs, knowledge_semantic_evidence) =
            if input.knowledge_enabled {
                self.select_prompt_knowledge(
                    input.preset,
                    input.character_content,
                    input.prompt_knowledge_books,
                    input.module_knowledge_books,
                    input.exact_character_knowledge_book,
                    input.conversation_id,
                    input.branch_id,
                    input.scan_texts,
                    input.manually_active_knowledge,
                    input.variables,
                    input.supported_capabilities,
                    input.resolved_memory_semantics,
                    input.activation_seed,
                    input.resolution_time,
                    knowledge_work_budget,
                )?
            } else {
                warnings.push("knowledge retrieval was disabled by quick settings".to_owned());
                (Vec::new(), Vec::new(), Vec::new())
            };
        let (selected_memory, memory_evidence) = if input.memory_enabled {
            self.select_prompt_memory(&input)?
        } else {
            warnings.push("memory retrieval was disabled by quick settings".to_owned());
            (Vec::new(), Vec::new())
        };
        Ok(PromptSelectionPreparation {
            selected_knowledge,
            knowledge_logs,
            knowledge_semantic_evidence,
            selected_memory,
            memory_evidence,
            warnings,
        })
    }

    fn prepare_prompt_conversation(
        &self,
        input: &GenerationPlanInput<'_>,
        latest: &Message,
        transformed_latest: &TransformResult,
    ) -> CoreResult<PromptConversationPreparation> {
        let (character, character_content) =
            if let Some(authority) = input.prompt_selection_authority {
                (
                    &authority.character,
                    authority
                        .character_content
                        .as_ref()
                        .map_or_else(CharacterContentV1::default, |stored| stored.value.clone()),
                )
            } else {
                let content = match self.storage().get_character_content(&input.character.id) {
                    Ok(stored) => stored.value,
                    Err(error) if error.code == lorepia_domain::CoreErrorCode::NotFound => {
                        CharacterContentV1::default()
                    }
                    Err(error) => return Err(error),
                };
                (input.character, content)
            };
        let prompt_character = character_prompt_content(character, &character_content);
        let prompt_messages = input
            .history
            .iter()
            .filter(|message| message.role != MessageRole::System)
            .enumerate()
            .map(|(index, message)| PromptConversationMessage {
                id: message.id.clone(),
                branch_id: input.branch_id.clone(),
                role: match message.role {
                    MessageRole::System => PromptMessageRole::System,
                    MessageRole::User => PromptMessageRole::User,
                    MessageRole::Assistant => PromptMessageRole::Assistant,
                },
                content: if message.id == latest.id {
                    transformed_latest.output.clone()
                } else {
                    message.content.clone()
                },
                turn_index: u32::try_from(index).unwrap_or(u32::MAX),
            })
            .collect::<Vec<_>>();
        let scan_texts = prompt_messages
            .iter()
            .map(|message| message.content.clone())
            .collect();
        Ok(PromptConversationPreparation {
            character_content,
            prompt_character,
            prompt_messages,
            scan_texts,
        })
    }

    #[allow(clippy::too_many_arguments)]
    fn materialize_prompt_context_sources(
        &self,
        preset: &PromptPreset,
        binding: Option<&StoredRevision<PromptPresetBinding>>,
        persona: Option<&PromptPersonaMaterialization>,
        conversation_id: &ConversationId,
        prompt_branch_id: &ConversationBranchId,
        context_source_branch_id: &ConversationBranchId,
        context_head_message_id: Option<&MessageId>,
        hypothetical_parent_id: Option<&MessageId>,
        messages: &[PromptConversationMessage],
        generation_attempt_id: Option<&GenerationId>,
        sealed_local_user_id_sha256: Option<&str>,
    ) -> CoreResult<PromptContextMaterialization> {
        if context_head_message_id != hypothetical_parent_id {
            return Err(CoreError::invalid(
                "attempt prompt context head differs from the hypothetical user turn",
            ));
        }
        if messages
            .iter()
            .any(|message| message.branch_id != *prompt_branch_id)
        {
            return Err(CoreError::internal(
                "materialized prompt messages use an inconsistent target branch",
            ));
        }
        let binding_value = binding.map(|stored| &stored.value);
        validate_prompt_binding_sources(preset, binding_value)?;
        let summaries = self.materialize_prompt_summaries(
            preset,
            conversation_id,
            context_source_branch_id,
            context_head_message_id,
            messages,
            generation_attempt_id,
        )?;
        let local_user_id_sha256 = sealed_local_user_id_sha256.map_or_else(
            || {
                self.storage()
                    .load_settings()
                    .map(|settings| prompt_local_user_id_sha256(&settings.local_user_id))
            },
            |sha256| Ok(sha256.to_owned()),
        )?;
        let binding_evidence = binding
            .map(|stored| {
                Ok(PromptContextBindingEvidence {
                    binding_id: stored.value.id.clone(),
                    binding_revision: stored.revision,
                    document_sha256: stored.value.canonical_document_sha256()?,
                })
            })
            .transpose()?;
        let mut snapshot = PromptContextSnapshotV1 {
            schema_version: 1,
            conversation_id: conversation_id.clone(),
            source_branch_id: context_source_branch_id.clone(),
            context_head_message_id: context_head_message_id.cloned(),
            local_user_id_sha256,
            binding: binding_evidence,
            persona: persona.map(|materialized| materialized.evidence.clone()),
            conversation_summary_id: summaries.conversation_summary_id.clone(),
            summaries: summaries.evidence.clone(),
            snapshot_sha256: String::new(),
        };
        snapshot.snapshot_sha256 =
            prompt_context_snapshot_sha256(&snapshot).map_err(orchestration_validation_error)?;
        let user_name = persona
            .map(|materialized| materialized.content.name.clone())
            .or_else(|| binding_value.and_then(|binding| binding.user_name_override.clone()))
            .unwrap_or_else(|| "Local user".to_owned());
        Ok(PromptContextMaterialization {
            user_name,
            author_note: binding_value.and_then(|binding| binding.author_note.clone()),
            group_context: binding_value.and_then(|binding| binding.group_context.clone()),
            slots: binding_value
                .map(|binding| binding.template_slots.clone())
                .unwrap_or_default(),
            summaries,
            snapshot,
        })
    }
}

fn character_prompt_content(
    character: &Character,
    content: &CharacterContentV1,
) -> CharacterPromptContent {
    CharacterPromptContent {
        character_id: character.id.clone(),
        name: character.name.clone(),
        aliases: Vec::new(),
        description: character.description.clone(),
        personality: content.personality.clone(),
        scenario: content.scenario.clone(),
        first_message: content.first_message.clone(),
        dialogue_examples: content.example_dialogs.clone(),
        system_instruction: content.system_instruction.clone(),
        post_history_instruction: content.post_history_instruction.clone(),
        alternate_greetings: content.alternate_greetings.clone(),
        knowledge_book_ids: content
            .knowledge_book
            .as_ref()
            .and_then(|reference| reference.id.clone())
            .into_iter()
            .collect(),
        asset_ids: content
            .assets
            .iter()
            .map(|asset| asset.id.clone())
            .collect(),
    }
}
