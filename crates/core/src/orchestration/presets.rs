use std::collections::{BTreeMap, BTreeSet};

use super::{
    GenerationPlanInput, GenerationPromptAuthorityCapture, PromptModuleOverlay,
    PromptModuleOverlayInput, canonical_prompt_capabilities, prompt_creativity_temperature,
};
use crate::{
    Core, Revisioned,
    revision::{project_revision, project_revisions},
};
use chrono::Utc;
use lorepia_domain::{
    Character, ConversationBranchId, ConversationId, ConversationMode, CoreError, CoreResult,
    GenerationReasoningEffort, KnowledgeBook, MemoryProfile, ModuleScope, PersonaPromptContent,
    PromptContextPersonaEvidence, PromptPreset, PromptPresetId, SourceKind, TransformSet,
    prompt_local_user_id_sha256,
};
use lorepia_orchestration::validate_prompt_preset as validate_prompt_preset_document;
use lorepia_storage::{
    GenerationPromptQuickSettingsAuthority, GenerationPromptSelectionAuthority, ObjectRevision,
    PromptPresetBinding, PromptPresetRevisionDiff, PromptPresetRollbackApproval,
    PromptPresetRollbackCommit, PromptPresetRollbackReview, PromptResponseLength, StoredRevision,
    built_in_prompt_presets, generation_prompt_selection_authority_sha256,
    prompt_preset_rollback_approval_sha256,
};

/// Stable confirmation submitted after a user reviews an exact rollback.
///
/// `approval_id` is caller-stable so a retry after response loss can return
/// the already-applied revision. Core derives the approval hash and target
/// document; callers cannot submit either.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PromptPresetRollbackApplyRequest {
    pub review: PromptPresetRollbackReview,
    pub approval_id: String,
    pub expected_review_sha256: String,
}

/// Durable rollback result. A rollback always appends a new immutable revision.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PromptPresetRollbackReceipt {
    pub preset: Revisioned<PromptPreset>,
    pub approval: PromptPresetRollbackApproval,
}

pub(super) struct PromptPersonaMaterialization {
    pub(super) content: PersonaPromptContent,
    pub(super) evidence: PromptContextPersonaEvidence,
}

pub(super) type PromptPresetSelection = (
    PromptPreset,
    u64,
    String,
    Option<StoredRevision<PromptPresetBinding>>,
    Option<StoredRevision<lorepia_domain::ConversationPersonaSelection>>,
);

pub(super) struct PromptPresetPreparation {
    pub(super) preset: PromptPreset,
    pub(super) revision: u64,
    pub(super) revision_id: String,
    pub(super) binding: Option<StoredRevision<PromptPresetBinding>>,
    pub(super) prompt_persona: Option<PromptPersonaMaterialization>,
    pub(super) prompt_knowledge_books: Vec<ObjectRevision<KnowledgeBook>>,
    pub(super) prompt_transform_sets: Vec<ObjectRevision<TransformSet>>,
    pub(super) prompt_memory_profile: Option<ObjectRevision<MemoryProfile>>,
    pub(super) block_source_revisions: BTreeMap<lorepia_domain::PromptBlockId, String>,
    pub(super) module_overlay: PromptModuleOverlay,
    pub(super) warnings: Vec<String>,
}

impl Core {
    pub(super) fn prepare_prompt_preset_sources(
        &self,
        input: &GenerationPlanInput<'_>,
    ) -> CoreResult<PromptPresetPreparation> {
        let (mut preset, revision, revision_id, binding, persona_selection) =
            self.resolve_generation_prompt_selection(input)?;
        enforce_application_policy(&mut preset);
        self.validate_prompt_preset(&preset)?;
        let prompt_persona = persona_selection
            .as_ref()
            .map(|selection| self.materialize_prompt_persona(selection))
            .transpose()?;
        let prompt_knowledge_books = self
            .storage()
            .get_prompt_preset_knowledge_book_revisions(&revision_id)?;
        let prompt_transform_sets = self
            .storage()
            .get_prompt_preset_transform_set_revisions(&revision_id)?;
        let prompt_memory_profile = self
            .storage()
            .get_prompt_preset_memory_profile_revision(&revision_id)?;
        let mut block_source_revisions = preset
            .blocks
            .iter()
            .filter(|block| block.provenance.source_kind != SourceKind::ApplicationBuiltIn)
            .map(|block| (block.id.clone(), revision_id.clone()))
            .collect::<BTreeMap<_, _>>();
        let module_overlay = self.resolve_prompt_module_overlay(
            &preset,
            &revision_id,
            PromptModuleOverlayInput {
                character: input
                    .prompt_selection_authority
                    .map_or(input.character, |authority| &authority.character),
                conversation_id: input.conversation_id,
                branch_id: input.branch_id,
                persona_id: persona_selection
                    .as_ref()
                    .map(|selection| &selection.value.persona_id),
                applied_plan_override: input.applied_module_plan_override,
                sealed_local_user_id_sha256: input
                    .prompt_selection_authority
                    .map(|authority| authority.local_user_id_sha256.as_str()),
                generation_attempt_id: input.generation_attempt_id,
            },
        )?;
        block_source_revisions.extend(module_overlay.prompt_block_source_revisions.clone());
        preset.blocks.extend(module_overlay.prompt_blocks.clone());
        preset.controls.extend(module_overlay.controls.clone());
        enforce_application_policy(&mut preset);
        preset.blocks.sort_by_key(|block| block.placement_zone);
        self.validate_prompt_preset(&preset)?;
        let warnings = module_overlay.warnings.clone();
        Ok(PromptPresetPreparation {
            preset,
            revision,
            revision_id,
            binding,
            prompt_persona,
            prompt_knowledge_books,
            prompt_transform_sets,
            prompt_memory_profile,
            block_source_revisions,
            module_overlay,
            warnings,
        })
    }

    fn materialize_prompt_persona(
        &self,
        selection: &StoredRevision<lorepia_domain::ConversationPersonaSelection>,
    ) -> CoreResult<PromptPersonaMaterialization> {
        let revision_id = selection.revision_id.as_deref().ok_or_else(|| {
            CoreError::new(
                lorepia_domain::CoreErrorCode::StorageCorrupted,
                "persona selection is missing its exact revision identity",
                false,
            )
        })?;
        let persona = self
            .storage()
            .get_persona_revision(&selection.value.persona_id, revision_id)?;
        Ok(PromptPersonaMaterialization {
            evidence: PromptContextPersonaEvidence {
                selection_revision: selection.revision,
                persona_id: selection.value.persona_id.clone(),
                persona_revision_id: persona.revision_id.clone(),
                persona_sha256: persona.sha256,
            },
            content: PersonaPromptContent {
                persona_id: persona.value.id,
                name: persona.value.name,
                description: persona.value.description,
            },
        })
    }

    /// Validates a prompt preset without changing durable state.
    pub fn validate_prompt_preset(&self, preset: &PromptPreset) -> CoreResult<()> {
        validate_prompt_preset_document(preset).map_err(orchestration_validation_error)
    }

    /// Inserts a new prompt preset or updates the exact expected revision.
    pub fn upsert_prompt_preset(
        &self,
        preset: &PromptPreset,
        expected_revision: Option<u64>,
    ) -> CoreResult<Revisioned<PromptPreset>> {
        if is_builtin_prompt_preset_id(&preset.id) {
            return Err(CoreError::invalid(
                "built-in prompt presets cannot be edited",
            ));
        }
        if preset.metadata.provenance.source_kind == SourceKind::ApplicationBuiltIn
            || preset
                .blocks
                .iter()
                .any(|block| block.provenance.source_kind == SourceKind::ApplicationBuiltIn)
        {
            return Err(CoreError::invalid(
                "creator prompt presets cannot claim application-built-in provenance",
            ));
        }
        let mut preset = preset.clone();
        preset
            .blocks
            .retain(|block| block.authority != lorepia_domain::InstructionAuthority::Application);
        enforce_application_policy(&mut preset);
        self.validate_prompt_preset(&preset)?;
        self.storage()
            .save_prompt_preset(&preset, expected_revision)
            .map(project_revision)
    }

    pub fn get_prompt_preset(&self, id: &PromptPresetId) -> CoreResult<Revisioned<PromptPreset>> {
        self.storage().get_prompt_preset(id).map(project_revision)
    }

    /// Returns a creator revision without application policy.
    /// Saving validates creator content and restores Core policy.
    pub fn get_editable_prompt_preset(
        &self,
        id: &PromptPresetId,
    ) -> CoreResult<Revisioned<PromptPreset>> {
        let mut stored = self.get_prompt_preset(id)?;
        if is_builtin_prompt_preset_id(id)
            || stored.value.metadata.provenance.source_kind == SourceKind::ApplicationBuiltIn
        {
            return Err(CoreError::new(
                lorepia_domain::CoreErrorCode::PermissionDenied,
                "built-in prompt presets are read-only",
                false,
            ));
        }
        self.validate_prompt_preset(&stored.value)?;
        stored.value.blocks.retain(|block| {
            let application_authority =
                block.authority == lorepia_domain::InstructionAuthority::Application;
            let application_zone =
                block.placement_zone == lorepia_domain::PlacementZone::ApplicationPolicy;
            !application_authority && !application_zone
        });
        Ok(stored)
    }

    pub fn list_prompt_presets(&self) -> CoreResult<Vec<Revisioned<PromptPreset>>> {
        self.storage().list_prompt_presets().map(project_revisions)
    }

    /// Lists immutable preset history in ascending revision order.
    pub fn list_prompt_preset_revisions(
        &self,
        id: &PromptPresetId,
    ) -> CoreResult<Vec<ObjectRevision<PromptPreset>>> {
        self.storage().list_prompt_preset_revisions(id)
    }

    /// Returns a deterministic, content-addressed JSON diff between two
    /// immutable preset revisions.
    pub fn diff_prompt_preset_revisions(
        &self,
        id: &PromptPresetId,
        from_revision: u64,
        to_revision: u64,
    ) -> CoreResult<PromptPresetRevisionDiff> {
        self.storage()
            .diff_prompt_preset_revisions(id, from_revision, to_revision)
    }

    /// Reviews a rollback against the exact current preset, immutable target,
    /// dependency rows, and every binding whose effective revision can change.
    pub fn review_prompt_preset_rollback(
        &self,
        id: &PromptPresetId,
        expected_current_state_revision: u64,
        target_revision: u64,
    ) -> CoreResult<PromptPresetRollbackReview> {
        self.ensure_prompt_preset_is_creator_owned(id)?;
        self.load_creator_owned_prompt_preset_revision(id, target_revision)?;
        self.storage().review_prompt_preset_rollback(
            id,
            expected_current_state_revision,
            target_revision,
            Utc::now(),
        )
    }

    /// Applies a reviewed rollback as a new immutable revision.
    ///
    /// The target document is always loaded from Storage. Core removes every
    /// historical application-policy slot, injects the current canonical
    /// policy exactly once, validates the complete document, and delegates the
    /// final state/binding/dependency CAS to one Storage transaction.
    pub fn apply_prompt_preset_rollback(
        &self,
        request: &PromptPresetRollbackApplyRequest,
    ) -> CoreResult<PromptPresetRollbackReceipt> {
        self.ensure_prompt_preset_is_creator_owned(&request.review.preset_id)?;
        if request.expected_review_sha256 != request.review.review_sha256 {
            return Err(CoreError::invalid(
                "prompt preset rollback approval does not match the reviewed hash",
            ));
        }
        let target = self.load_creator_owned_prompt_preset_revision(
            &request.review.preset_id,
            request.review.target_revision,
        )?;
        if target.revision_id != request.review.target_revision_id
            || target.sha256 != request.review.target_sha256
        {
            return Err(CoreError::invalid(
                "prompt preset rollback target changed after review",
            ));
        }
        let mut canonical_target = target.value;
        canonical_target.blocks.retain(|block| {
            block.authority != lorepia_domain::InstructionAuthority::Application
                && block.placement_zone != lorepia_domain::PlacementZone::ApplicationPolicy
        });
        enforce_application_policy(&mut canonical_target);
        self.validate_prompt_preset(&canonical_target)?;

        let approval_sha256 = prompt_preset_rollback_approval_sha256(
            &request.approval_id,
            &request.expected_review_sha256,
        )?;
        let approval = PromptPresetRollbackApproval {
            approval_id: request.approval_id.clone(),
            expected_review_sha256: request.expected_review_sha256.clone(),
            approval_sha256,
            approved_at: Utc::now(),
        };
        let preset = self
            .storage()
            .apply_prompt_preset_rollback(&PromptPresetRollbackCommit {
                review: request.review.clone(),
                approval: approval.clone(),
                canonical_target,
            })?;
        let durable_approval = self
            .storage()
            .get_prompt_preset_rollback_approval(&request.approval_id)?;
        if durable_approval.approval_id != approval.approval_id
            || durable_approval.expected_review_sha256 != approval.expected_review_sha256
            || durable_approval.approval_sha256 != approval.approval_sha256
        {
            return Err(CoreError::new(
                lorepia_domain::CoreErrorCode::StorageCorrupted,
                "durable prompt preset rollback approval differs from the applied approval",
                false,
            ));
        }
        Ok(PromptPresetRollbackReceipt {
            preset: project_revision(preset),
            approval: durable_approval,
        })
    }

    pub fn delete_prompt_preset(
        &self,
        id: &PromptPresetId,
        expected_revision: u64,
    ) -> CoreResult<Revisioned<PromptPreset>> {
        let preset = self.get_prompt_preset(id)?;
        if is_builtin_prompt_preset_id(id)
            || preset.value.metadata.provenance.source_kind == SourceKind::ApplicationBuiltIn
        {
            return Err(CoreError::invalid(
                "built-in prompt presets cannot be deleted",
            ));
        }
        self.storage()
            .soft_delete_prompt_preset(id, expected_revision)
            .map(project_revision)
    }

    fn ensure_prompt_preset_is_creator_owned(&self, id: &PromptPresetId) -> CoreResult<()> {
        let preset = self.get_prompt_preset(id)?;
        if is_builtin_prompt_preset_id(id)
            || preset.value.metadata.provenance.source_kind == SourceKind::ApplicationBuiltIn
        {
            return Err(CoreError::new(
                lorepia_domain::CoreErrorCode::PermissionDenied,
                "built-in prompt presets are read-only",
                false,
            ));
        }
        Ok(())
    }

    fn load_creator_owned_prompt_preset_revision(
        &self,
        id: &PromptPresetId,
        revision: u64,
    ) -> CoreResult<ObjectRevision<PromptPreset>> {
        let target = self
            .storage()
            .list_prompt_preset_revisions(id)?
            .into_iter()
            .find(|candidate| candidate.revision == revision)
            .ok_or_else(|| {
                CoreError::new(
                    lorepia_domain::CoreErrorCode::NotFound,
                    "prompt preset rollback target revision was not found",
                    false,
                )
            })?;
        let claims_application_provenance = target.value.metadata.provenance.source_kind
            == SourceKind::ApplicationBuiltIn
            || target.value.blocks.iter().any(|block| {
                block.provenance.source_kind == SourceKind::ApplicationBuiltIn
                    && (block.authority != lorepia_domain::InstructionAuthority::Application
                        || block.placement_zone != lorepia_domain::PlacementZone::ApplicationPolicy)
            });
        if claims_application_provenance {
            return Err(CoreError::new(
                lorepia_domain::CoreErrorCode::PermissionDenied,
                "creator prompt preset rollback targets cannot claim application-built-in provenance",
                false,
            ));
        }
        Ok(target)
    }

    /// Reorders all blocks in a creator preset with optimistic concurrency.
    /// The canonical application-policy block must remain first.
    pub fn reorder_prompt_blocks(
        &self,
        id: &PromptPresetId,
        ordered_block_ids: &[lorepia_domain::PromptBlockId],
        expected_revision: u64,
    ) -> CoreResult<Revisioned<PromptPreset>> {
        if is_builtin_prompt_preset_id(id) {
            return Err(CoreError::invalid(
                "built-in prompt presets cannot be reordered",
            ));
        }
        let stored = self.get_prompt_preset(id)?;
        if stored.revision != expected_revision {
            return Err(CoreError::invalid(
                "prompt preset changed before blocks were reordered",
            ));
        }
        let mut preset = stored.value;
        self.validate_prompt_preset(&preset)?;
        let application_policy = preset
            .blocks
            .iter()
            .find(|block| {
                block.placement_zone == lorepia_domain::PlacementZone::ApplicationPolicy
                    && block.authority == lorepia_domain::InstructionAuthority::Application
            })
            .cloned()
            .ok_or_else(|| CoreError::internal("prompt preset is missing application policy"))?;
        let mut remaining = std::mem::take(&mut preset.blocks)
            .into_iter()
            .filter(|block| {
                block.placement_zone != lorepia_domain::PlacementZone::ApplicationPolicy
                    && block.authority != lorepia_domain::InstructionAuthority::Application
            })
            .map(|block| (block.id.clone(), block))
            .collect::<std::collections::BTreeMap<_, _>>();
        if ordered_block_ids.len() != remaining.len() {
            return Err(CoreError::invalid(
                "block reorder must contain every creator-owned block exactly once",
            ));
        }
        let mut blocks = Vec::with_capacity(ordered_block_ids.len().saturating_add(1));
        blocks.push(application_policy);
        for block_id in ordered_block_ids {
            let block = remaining.remove(block_id).ok_or_else(|| {
                CoreError::invalid("block reorder contains an unknown or duplicate block")
            })?;
            blocks.push(block);
        }
        if !remaining.is_empty() {
            return Err(CoreError::invalid(
                "block reorder omitted a creator-owned block",
            ));
        }
        preset.blocks = blocks;
        self.upsert_prompt_preset(&preset, Some(expected_revision))
    }

    /// Saves the prompt selection and quick-setting overrides for one scope.
    pub fn bind_prompt_preset(
        &self,
        binding: &PromptPresetBinding,
        expected_revision: Option<u64>,
    ) -> CoreResult<Revisioned<PromptPresetBinding>> {
        let preset = self.get_prompt_preset(&binding.prompt_preset_id)?.value;
        validate_prompt_binding_sources(&preset, Some(binding))?;
        self.storage()
            .save_prompt_preset_binding(binding, expected_revision)
            .map(project_revision)
    }

    pub(super) fn resolve_prompt_preset_selection(
        &self,
        character: &Character,
        conversation_id: &ConversationId,
        branch_id: &ConversationBranchId,
        mode: ConversationMode,
        explicit_id: Option<&PromptPresetId>,
    ) -> CoreResult<PromptPresetSelection> {
        let persona_selection = self
            .storage()
            .get_conversation_persona_selection(conversation_id)?;
        let persona_target = persona_selection
            .as_ref()
            .map(|selection| selection.value.persona_id.0.as_str());
        let mut scopes = vec![
            (ModuleScope::Branch, Some(branch_id.0.as_str())),
            (ModuleScope::Conversation, Some(conversation_id.0.as_str())),
            (ModuleScope::Character, Some(character.id.as_str())),
        ];
        if let Some(persona_id) = persona_target {
            scopes.push((ModuleScope::Persona, Some(persona_id)));
        }
        scopes.extend([(ModuleScope::User, None), (ModuleScope::App, None)]);
        let mut selected_binding = None;
        for (scope, target_id) in scopes {
            let enabled = self
                .storage()
                .list_prompt_preset_bindings(scope, target_id)?
                .into_iter()
                .filter(|stored| stored.deleted_at.is_none() && stored.value.enabled)
                .collect::<Vec<_>>();
            if enabled.len() > 1 {
                return Err(CoreError::invalid(
                    "multiple enabled prompt bindings apply at the same scope",
                ));
            }
            if let Some(stored) = enabled.into_iter().next() {
                selected_binding = Some(stored);
                break;
            }
        }
        let preset_id = if let Some(explicit_id) = explicit_id {
            explicit_id.clone()
        } else if let Some(binding) = &selected_binding {
            binding.value.prompt_preset_id.clone()
        } else {
            let built_ins = built_in_prompt_presets();
            match mode {
                ConversationMode::Chat => built_ins[0].id.clone(),
                ConversationMode::Story => built_ins[1].id.clone(),
            }
        };
        if selected_binding
            .as_ref()
            .is_some_and(|binding| binding.value.prompt_preset_id != preset_id)
        {
            selected_binding = None;
        }
        let stored = self.get_prompt_preset(&preset_id)?;
        let revision_id = stored.revision_id.clone().ok_or_else(|| {
            CoreError::internal("prompt preset is missing its immutable revision identity")
        })?;
        Ok((
            stored.value,
            stored.revision,
            revision_id,
            selected_binding,
            persona_selection,
        ))
    }

    pub(crate) fn capture_generation_prompt_selection_authority(
        &self,
        input: GenerationPromptAuthorityCapture<'_>,
    ) -> CoreResult<GenerationPromptSelectionAuthority> {
        let GenerationPromptAuthorityCapture {
            character,
            conversation_id,
            branch_id,
            mode,
            explicit_preset_id,
            generation_target,
            temperature,
            max_output_tokens,
            prompt_wire_contract,
            provider_target_authority,
        } = input;
        let (preset, preset_revision, preset_revision_id, binding, persona_selection) = self
            .resolve_prompt_preset_selection(
                character,
                conversation_id,
                branch_id,
                mode,
                explicit_preset_id,
            )?;
        let character_content = match self.storage().get_character_content(&character.id) {
            Ok(stored) => Some(stored),
            Err(error) if error.code == lorepia_domain::CoreErrorCode::NotFound => None,
            Err(error) => return Err(error),
        };
        let character_knowledge_book = character_content
            .as_ref()
            .and_then(|content| content.value.knowledge_book.as_ref())
            .and_then(|reference| reference.id.as_ref())
            .map(|book_id| self.storage().get_knowledge_book(book_id))
            .transpose()?;
        let supported_capabilities = generation_target.map_or_else(
            || Ok(Vec::new()),
            |target| self.prompt_supported_capabilities(&target.model_route_id),
        )?;
        let supported_capabilities = canonical_prompt_capabilities(supported_capabilities)?;
        let binding_value = binding.as_ref().map(|stored| &stored.value);
        let response_length = binding_value.map_or(PromptResponseLength::Balanced, |value| {
            value.response_length
        });
        let creativity = binding_value.map_or(50, |value| value.creativity);
        let supports_temperature = prompt_wire_contract.map_or_else(
            || {
                generation_target.map_or(Ok(temperature.is_some()), |target| {
                    crate::app::prompt_route_supports_temperature(self, target)
                })
            },
            |contract| Ok(contract.supports_temperature),
        )?;
        let resolved_temperature = temperature.or_else(|| {
            (binding_value.is_some() && supports_temperature)
                .then_some(prompt_creativity_temperature(creativity))
        });
        let resolved_max_output_tokens = max_output_tokens.or_else(|| {
            binding_value.map(|_| match response_length {
                PromptResponseLength::Short => 512,
                PromptResponseLength::Balanced => 2_048,
                PromptResponseLength::Long => 4_096,
            })
        });
        let authority = GenerationPromptSelectionAuthority {
            schema_version: 1,
            mode,
            local_user_id_sha256: prompt_local_user_id_sha256(
                &self.storage().load_settings()?.local_user_id,
            ),
            character: character.clone(),
            character_content,
            character_knowledge_book,
            supported_capabilities,
            quick_settings: GenerationPromptQuickSettingsAuthority {
                response_length,
                creativity,
                reasoning_effort: binding_value.and_then(|value| value.reasoning_effort),
                memory_enabled: binding_value.is_none_or(|value| value.memory_enabled),
                knowledge_enabled: binding_value.is_none_or(|value| value.knowledge_enabled),
                supports_temperature,
                resolved_temperature,
                resolved_max_output_tokens,
            },
            provider_target_authority: Some(provider_target_authority),
            explicit_preset_id: explicit_preset_id.cloned(),
            preset,
            preset_revision,
            preset_revision_id,
            binding,
            persona_selection,
        };
        generation_prompt_selection_authority_sha256(&authority)?;
        Ok(authority)
    }

    pub(super) fn resolve_generation_prompt_selection(
        &self,
        input: &GenerationPlanInput<'_>,
    ) -> CoreResult<PromptPresetSelection> {
        let Some(authority) = input.prompt_selection_authority else {
            return self.resolve_prompt_preset_selection(
                input.character,
                input.conversation_id,
                input.branch_id,
                input.mode,
                input.prompt_preset_id,
            );
        };
        generation_prompt_selection_authority_sha256(authority)?;
        if authority.explicit_preset_id.as_ref() != input.prompt_preset_id {
            return Err(CoreError::new(
                lorepia_domain::CoreErrorCode::StorageCorrupted,
                "attempt prompt selection differs from the requested prompt context",
                false,
            ));
        }
        Ok((
            authority.preset.clone(),
            authority.preset_revision,
            authority.preset_revision_id.clone(),
            authority.binding.clone(),
            authority.persona_selection.clone(),
        ))
    }

    pub(crate) fn prompt_reasoning_effort_for_context(
        &self,
        conversation_id: &ConversationId,
        branch_id: &ConversationBranchId,
        mode: ConversationMode,
        explicit_id: Option<&PromptPresetId>,
    ) -> CoreResult<Option<GenerationReasoningEffort>> {
        let conversation = self.storage().get_conversation(conversation_id)?;
        let character = self.storage().get_character(&conversation.character_id)?;
        self.resolve_prompt_preset_selection(
            &character,
            conversation_id,
            branch_id,
            mode,
            explicit_id,
        )
        .map(|(_, _, _, binding, _)| binding.and_then(|binding| binding.value.reasoning_effort))
    }
}

pub(super) fn orchestration_validation_error(error: impl std::fmt::Display) -> CoreError {
    CoreError::invalid(format!("prompt preset is invalid: {error}"))
}

pub(super) fn validate_prompt_binding_sources(
    preset: &PromptPreset,
    binding: Option<&PromptPresetBinding>,
) -> CoreResult<()> {
    let needs_author_note = preset.blocks.iter().any(|block| {
        block.enabled && matches!(block.source, lorepia_domain::BlockSource::AuthorNote)
    });
    let needs_group_context = preset.blocks.iter().any(|block| {
        block.enabled && matches!(block.source, lorepia_domain::BlockSource::GroupContext)
    });
    if needs_author_note
        && binding
            .and_then(|binding| binding.author_note.as_ref())
            .is_none()
    {
        return Err(CoreError::invalid(
            "enabled author-note block requires a room author note",
        ));
    }
    if needs_group_context
        && binding
            .and_then(|binding| binding.group_context.as_ref())
            .is_none()
    {
        return Err(CoreError::invalid(
            "enabled group-context block requires room group context",
        ));
    }
    let required_slots = preset
        .blocks
        .iter()
        .filter(|block| block.enabled)
        .filter_map(|block| block.template.as_ref())
        .flat_map(|template| &template.parts)
        .filter_map(|part| match part {
            lorepia_domain::TemplatePart::Slot { name } if name != "block_content" => {
                Some(name.as_str())
            }
            _ => None,
        })
        .collect::<BTreeSet<_>>();
    let available_slots = binding
        .map(|binding| {
            binding
                .template_slots
                .iter()
                .map(|slot| slot.name.as_str())
                .collect::<BTreeSet<_>>()
        })
        .unwrap_or_default();
    if let Some(missing) = required_slots
        .iter()
        .find(|name| !available_slots.contains(**name))
    {
        return Err(CoreError::invalid(format!(
            "enabled prompt template requires unavailable room slot `{missing}`"
        )));
    }
    Ok(())
}

pub(crate) fn enforce_application_policy(preset: &mut PromptPreset) {
    let mut built_ins = built_in_prompt_presets();
    let story_policy_index = usize::from(preset.id == built_ins[1].id);
    let application_policy = built_ins[story_policy_index].blocks.remove(0);
    // Replace only the reserved policy slot. Other trusted built-in blocks
    // (including the story-mode instruction and compatibility prompt blocks)
    // deliberately carry application provenance and must survive runtime
    // normalization. Creator writes are separately stripped of application
    // authority by `upsert_prompt_preset`, while module overlays reject that
    // authority before reaching this merge.
    preset
        .blocks
        .retain(|block| block.placement_zone != lorepia_domain::PlacementZone::ApplicationPolicy);
    preset.blocks.insert(0, application_policy);
}

fn is_builtin_prompt_preset_id(id: &PromptPresetId) -> bool {
    built_in_prompt_presets()
        .iter()
        .any(|preset| preset.id == *id)
}
