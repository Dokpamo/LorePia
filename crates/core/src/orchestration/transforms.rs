use std::collections::{BTreeMap, BTreeSet};

use lorepia_domain::{
    CapabilityKey, CoreError, CoreResult, Message, ResolvedPromptPlan, TransformRuleId,
    TransformSet, TransformSetId, VariableMap,
};
use lorepia_orchestration::{
    TransformApplyOptions, TransformCompileOptions, TransformContext, TransformLimits,
    TransformPipeline, TransformResult, preview_transform_rule, reseal_resolved_prompt_plan,
};
use lorepia_storage::ObjectRevision;

use crate::Core;

use super::{GenerationPlanInput, PromptModuleOverlay, orchestration_validation_error};

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct TransformPreviewRequest {
    pub transform_set_id: TransformSetId,
    pub rule_id: TransformRuleId,
    pub input: String,
    pub variables: VariableMap,
    pub supported_capabilities: Vec<CapabilityKey>,
    pub approved_import_source_ids: Vec<String>,
    pub allow_resolved_prompt: bool,
}

pub(super) struct PromptTransformPreparation {
    pub(super) transform_sets: Vec<TransformSet>,
    pub(super) transform_set_revisions: BTreeMap<TransformSetId, String>,
    pub(super) approved_import_source_ids: BTreeSet<String>,
    pub(super) supported_capabilities: Vec<CapabilityKey>,
    pub(super) transformed_latest: TransformResult,
}

impl Core {
    pub(super) fn prepare_prompt_transforms(
        &self,
        prompt_transform_sets: &[ObjectRevision<TransformSet>],
        module_overlay: &PromptModuleOverlay,
        input: &GenerationPlanInput<'_>,
        latest: &Message,
        variables: &VariableMap,
    ) -> CoreResult<PromptTransformPreparation> {
        let mut transform_set_revisions = BTreeMap::new();
        for revision in prompt_transform_sets
            .iter()
            .chain(&module_overlay.transform_sets)
        {
            if transform_set_revisions
                .insert(revision.value.id.clone(), revision.revision_id.clone())
                .is_some()
            {
                return Err(CoreError::invalid(
                    "prompt preset and approved module select the same transform set ambiguously",
                ));
            }
        }
        let mut transform_sets = prompt_transform_sets
            .iter()
            .map(|revision| revision.value.clone())
            .collect::<Vec<_>>();
        append_exact_module_transform_sets(&mut transform_sets, &module_overlay.transform_sets)?;
        // Imported character-card transforms are session-granted display behavior.
        // Core has no revision-bound portable-runtime grant, so it must not add the
        // stored native projection to canonical generation transforms implicitly.
        let approved_import_source_ids = module_overlay.approved_import_source_ids.clone();
        let supported_capabilities = if let Some(authority) = input.prompt_selection_authority {
            authority.supported_capabilities.clone()
        } else {
            input.generation_target.map_or_else(
                || Ok(Vec::new()),
                |target| self.prompt_supported_capabilities(&target.model_route_id),
            )?
        };
        let transformed_latest = apply_transform_sets_with_import_approvals(
            &transform_sets,
            lorepia_domain::TransformPhase::UserInputForRequest,
            &latest.content,
            variables,
            &supported_capabilities,
            &approved_import_source_ids,
        )?;
        Ok(PromptTransformPreparation {
            transform_sets,
            transform_set_revisions,
            approved_import_source_ids,
            supported_capabilities,
            transformed_latest,
        })
    }

    pub fn preview_transform(
        &self,
        request: &TransformPreviewRequest,
    ) -> CoreResult<TransformResult> {
        let transform_set = self.get_transform_set(&request.transform_set_id)?.value;
        let rule = transform_set
            .rules
            .iter()
            .find(|rule| rule.id == request.rule_id)
            .ok_or_else(|| {
                CoreError::new(
                    lorepia_domain::CoreErrorCode::NotFound,
                    "transform rule was not found in the selected set",
                    false,
                )
            })?;
        let approved_import_source_ids =
            request.approved_import_source_ids.iter().cloned().collect();
        preview_transform_rule(
            rule,
            &request.input,
            TransformContext {
                variables: &request.variables,
                model_capabilities: &request.supported_capabilities,
            },
            TransformLimits::default(),
            &TransformCompileOptions {
                approved_import_source_ids,
            },
            TransformApplyOptions {
                allow_resolved_prompt: request.allow_resolved_prompt,
            },
        )
        .map_err(orchestration_validation_error)
    }
}

fn append_exact_module_transform_sets(
    target: &mut Vec<TransformSet>,
    module_sets: &[ObjectRevision<TransformSet>],
) -> CoreResult<()> {
    for revision in module_sets {
        if target.iter().any(|set| set.id == revision.value.id) {
            return Err(CoreError::invalid(
                "prompt preset and approved module select the same transform set ambiguously",
            ));
        }
        target.push(revision.value.clone());
    }
    Ok(())
}

pub(crate) fn apply_transform_sets_with_import_approvals(
    sets: &[TransformSet],
    phase: lorepia_domain::TransformPhase,
    input: &str,
    variables: &VariableMap,
    supported_capabilities: &[CapabilityKey],
    approved_import_source_ids: &BTreeSet<String>,
) -> CoreResult<TransformResult> {
    let pipeline = TransformPipeline::compile_with_options(
        sets,
        TransformLimits::default(),
        &TransformCompileOptions {
            approved_import_source_ids: approved_import_source_ids.clone(),
        },
    )
    .map_err(|error| CoreError::invalid(format!("transform pipeline is invalid: {error}")))?;
    // Runtime transform failures deliberately return the original input. The
    // structured report stays available for diagnostics while generation never
    // consumes a partial or ambiguous transform output.
    Ok(pipeline.apply(
        phase,
        input,
        TransformContext {
            variables,
            model_capabilities: supported_capabilities,
        },
        TransformApplyOptions::default(),
    ))
}

pub(super) fn apply_resolved_prompt_transforms(
    plan: &ResolvedPromptPlan,
    sets: &[TransformSet],
    variables: &VariableMap,
    supported_capabilities: &[CapabilityKey],
    approved_import_source_ids: &BTreeSet<String>,
) -> CoreResult<(ResolvedPromptPlan, Vec<String>)> {
    if !sets.iter().any(|set| {
        set.enabled
            && set.rules.iter().any(|rule| {
                rule.enabled && rule.phase == lorepia_domain::TransformPhase::ResolvedPrompt
            })
    }) {
        return Ok((plan.clone(), Vec::new()));
    }
    let pipeline = TransformPipeline::compile_with_options(
        sets,
        TransformLimits::default(),
        &TransformCompileOptions {
            approved_import_source_ids: approved_import_source_ids.clone(),
        },
    )
    .map_err(|error| CoreError::invalid(format!("transform pipeline is invalid: {error}")))?;
    let mut transformed_contents = Vec::with_capacity(plan.effective_messages.len());
    let mut warnings = Vec::new();
    let mut changed = false;
    for message in &plan.effective_messages {
        let result = pipeline.apply(
            lorepia_domain::TransformPhase::ResolvedPrompt,
            &message.content,
            TransformContext {
                variables,
                model_capabilities: supported_capabilities,
            },
            TransformApplyOptions {
                allow_resolved_prompt: true,
            },
        );
        if let Some(error) = &result.error {
            warnings.push(format!(
                "resolved-prompt transform failed for block {} and preserved the original text: {:?}",
                message.block_id.as_str(),
                error.code
            ));
        }
        changed |= result.changed;
        transformed_contents.push(result.output);
    }
    if !changed {
        return Ok((plan.clone(), warnings));
    }
    match reseal_resolved_prompt_plan(plan, &transformed_contents) {
        Ok(plan) => Ok((plan, warnings)),
        Err(error) => {
            warnings.push(format!(
                "resolved-prompt transform exceeded the reviewed plan boundary and was ignored: {error}"
            ));
            Ok((plan.clone(), warnings))
        }
    }
}
