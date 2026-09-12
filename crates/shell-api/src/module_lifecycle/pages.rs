//! Bounded views of complete module reviews. No page is itself authority:
//! every request recreates and verifies the complete Core review or plan.

use lorepia_core::{ModuleConflict, ModuleImportApprovalEvidence, ReviewedModuleComponent};

use super::{
    ActivateContentModuleInput, ContentModuleActivationRevisionReview,
    ContentModuleImportApprovalCandidateDto, ContentModuleLifecycleBindingDto,
    MAX_JAVASCRIPT_SAFE_INTEGER, MAX_LIFECYCLE_BINDINGS, MAX_LIFECYCLE_CANDIDATES_PER_COMPONENT,
    MAX_LIFECYCLE_COMPONENTS, MAX_LIFECYCLE_IMPORT_APPROVALS, MAX_LIFECYCLE_SOURCES_PER_CANDIDATE,
    MAX_LIFECYCLE_VARIABLE_OVERRIDES, MAX_UTC_RFC3339_TIMESTAMP,
    ResolveContentModuleActivationInput, invalid_lifecycle, validate_activation_request,
    validate_approval, validate_javascript_safe_integer, validate_proposed_import_authority,
    validate_resolution_set, validate_serialized,
};
use crate::{ShellApi, ShellError, ShellResult};
use lorepia_core::{ModuleActivationPlan, Sha256Digest};
use serde::{Deserialize, Serialize};

const PAGE_SIZE: usize = 64;

#[cfg(test)]
mod tests;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReviewContentModuleActivationPageInput {
    pub activation: lorepia_core::ContentModuleActivationRequest,
    pub offset: u32,
    /// Required beyond the first page. A changed workspace invalidates paging.
    pub expected_review_sha256: Option<Sha256Digest>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ContentModuleActivationReviewPageDto {
    pub review_sha256: Sha256Digest,
    pub state_revision: u64,
    pub activation_binding_id: String,
    pub proposed_revision: ContentModuleActivationRevisionReview,
    pub package_approval: Option<ContentModuleImportApprovalCandidateDto>,
    pub component_count: u32,
    pub offset: u32,
    pub next_offset: Option<u32>,
    pub components: Vec<ReviewedModuleComponent>,
    /// Complete conflict set, bounded by the existing resolution limit.
    pub conflicts: Vec<ModuleConflict>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ContentModuleActivationPlanSummaryDto {
    pub review_sha256: Sha256Digest,
    pub plan_sha256: Sha256Digest,
    pub expected_state_revision: u64,
    pub activation_binding_id: String,
    pub component_count: u32,
    pub runtime_enabled_count: u32,
    pub omitted_component_count: u32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ContentModuleActivationReceiptSummaryDto {
    pub verified: bool,
    pub binding: ContentModuleLifecycleBindingDto,
    pub approval_id: String,
    pub approval_sha256: Sha256Digest,
    pub plan: ContentModuleActivationPlanSummaryDto,
}

impl ShellApi {
    pub fn review_content_module_activation_page(
        &self,
        input: ReviewContentModuleActivationPageInput,
    ) -> ShellResult<ContentModuleActivationReviewPageDto> {
        validate_activation_request(&input.activation)?;
        if input.offset != 0 && input.expected_review_sha256.is_none() {
            return Err(invalid_lifecycle(
                "continued module review requires its exact hash",
            ));
        }
        let presentation = self
            .core
            .review_content_module_activation_presentation(&input.activation)
            .map_err(ShellError::from)?;
        let review = &presentation.review;
        review
            .verify()
            .map_err(|error| invalid_lifecycle(error.to_string()))?;
        if input
            .expected_review_sha256
            .as_ref()
            .is_some_and(|hash| hash != &review.review_sha256)
        {
            return Err(invalid_lifecycle("module review page is stale"));
        }
        validate_proposed_import_authority(review, &presentation.proposed_revision)?;
        validate_javascript_safe_integer("module review state revision", review.state_revision)?;
        // Only the total component and import-evidence payloads stay native.
        // All existing bounds on bindings, candidates, sources, choices and
        // overrides still apply to the complete review, not just this page.
        if review.ordered_bindings.len() > MAX_LIFECYCLE_BINDINGS
            || review.ignored_bindings.len() > MAX_LIFECYCLE_BINDINGS
            || review.conflicts.len() > MAX_LIFECYCLE_COMPONENTS
            || review.import_approvals.len() > MAX_LIFECYCLE_IMPORT_APPROVALS
            || review.effective_variable_overrides.values.len() > MAX_LIFECYCLE_VARIABLE_OVERRIDES
            || review.components.iter().any(|component| {
                component.candidates.len() > MAX_LIFECYCLE_CANDIDATES_PER_COMPONENT
                    || component.candidates.iter().any(|candidate| {
                        candidate.sources.len() > MAX_LIFECYCLE_SOURCES_PER_CANDIDATE
                    })
            })
            || review
                .conflicts
                .iter()
                .any(|conflict| conflict.candidates.len() > MAX_LIFECYCLE_CANDIDATES_PER_COMPONENT)
        {
            return Err(invalid_lifecycle(
                "module review exceeds bounded metadata or choices",
            ));
        }
        let offset = input.offset as usize;
        if offset > review.components.len() || (offset == review.components.len() && offset != 0) {
            return Err(invalid_lifecycle(
                "module review page is outside the component set",
            ));
        }
        let end = (offset + PAGE_SIZE).min(review.components.len());
        let binding_id = &input.activation.binding.id;
        let package_approval = review
            .import_approvals
            .iter()
            .find(|approval| &approval.binding_id == binding_id)
            .map(|approval| approval_summary(&approval.evidence))
            .transpose()?;
        let page = ContentModuleActivationReviewPageDto {
            review_sha256: review.review_sha256.clone(),
            state_revision: review.state_revision,
            activation_binding_id: binding_id.as_str().to_owned(),
            proposed_revision: presentation.proposed_revision,
            package_approval,
            component_count: count(review.components.len())?,
            offset: input.offset,
            next_offset: (end < review.components.len())
                .then(|| count(end))
                .transpose()?,
            components: review.components[offset..end].to_vec(),
            conflicts: review.conflicts.clone(),
        };
        validate_serialized("module review page", &page)?;
        Ok(page)
    }

    pub fn resolve_content_module_activation_summary(
        &self,
        input: ResolveContentModuleActivationInput,
    ) -> ShellResult<ContentModuleActivationPlanSummaryDto> {
        validate_activation_request(&input.activation)?;
        validate_resolution_set(&input.resolutions)?;
        let plan = self
            .core
            .resolve_content_module_activation(&input.activation, &input.resolutions)
            .map_err(ShellError::from)?;
        summarize_plan(&plan)
    }

    pub fn activate_content_module_summary(
        &self,
        input: ActivateContentModuleInput,
    ) -> ShellResult<ContentModuleActivationReceiptSummaryDto> {
        validate_activation_request(&input.activation)?;
        validate_resolution_set(&input.resolutions)?;
        validate_approval(&input.approval)?;
        let preflight = self
            .core
            .preflight_content_module_activation(
                &input.activation,
                &input.resolutions,
                &input.approval,
            )
            .map_err(ShellError::from)?;
        preflight
            .verify()
            .map_err(|error| invalid_lifecycle(error.to_string()))?;
        validate_javascript_safe_integer(
            "module resulting revision",
            preflight.resulting_state_revision,
        )?;
        // Check the exact compact receipt shape before any durable mutation.
        // The longest supported UTC timestamp bounds the storage-authored value.
        let mut projected = ContentModuleActivationReceiptSummaryDto {
            verified: true,
            binding: ContentModuleLifecycleBindingDto {
                binding: preflight.binding,
                state_revision: preflight.resulting_state_revision,
                updated_at: MAX_UTC_RFC3339_TIMESTAMP
                    .parse()
                    .map_err(|_| invalid_lifecycle("invalid receipt timestamp bound"))?,
            },
            approval_id: preflight.approved_plan.approval_id,
            approval_sha256: preflight.approved_plan.approval_sha256,
            plan: summarize_plan(&preflight.approved_plan.plan)?,
        };
        validate_serialized("module activation summary preflight", &projected)?;
        let receipt = self
            .core
            .activate_content_module(&input.activation, &input.resolutions, &input.approval)
            .map_err(activation_error)?;
        receipt
            .verify()
            .map_err(|error| invalid_lifecycle(error.to_string()))?;
        if receipt.approved_plan.approval_sha256 != projected.approval_sha256
            || receipt.approved_plan.approval_id != projected.approval_id
            || summarize_plan(&receipt.approved_plan.plan)? != projected.plan
            || receipt.binding.value != projected.binding.binding
            || receipt.binding.revision != projected.binding.state_revision
        {
            return Err(invalid_lifecycle(
                "module receipt changed from verified preflight",
            ));
        }
        projected.binding.updated_at = receipt.binding.updated_at;
        validate_serialized("module activation summary receipt", &projected)?;
        Ok(projected)
    }
}

fn count(value: usize) -> ShellResult<u32> {
    u32::try_from(value).map_err(|_| invalid_lifecycle("module count exceeds safe projection"))
}

fn activation_error(error: lorepia_core::CoreError) -> ShellError {
    let too_large = error.code == lorepia_core::CoreErrorCode::InvalidInput
        && matches!(
            error.message.as_str(),
            "module activation review exceeds its JSON storage limit"
                | "approved module activation exceeds its JSON storage limit"
                | "applied module runtime plan exceeds its JSON storage limit"
                | "module authority composition exceeds its JSON storage limit"
                | "module authority composition exceeds JSON nesting or node limits"
        );
    let mut projected = ShellError::from(error);
    if too_large {
        "error.module_plan_too_large".clone_into(&mut projected.message_key);
    }
    projected
}

fn approval_summary(
    evidence: &ModuleImportApprovalEvidence,
) -> ShellResult<ContentModuleImportApprovalCandidateDto> {
    validate_javascript_safe_integer("module import revision", evidence.import_revision)?;
    Ok(ContentModuleImportApprovalCandidateDto {
        approval_id: evidence.approval_id.clone(),
        approval_sha256: evidence.approval_sha256.to_string(),
        import_id: evidence.import_id.clone(),
        import_revision: evidence.import_revision,
        package_id: evidence.package_id.0.clone(),
        package_source_sha256: evidence.package_source_sha256.to_string(),
        selection_sha256: evidence.selection_sha256.to_string(),
        capability_review_sha256: evidence.capability_review_sha256.to_string(),
        module_id: evidence.module_id.0.clone(),
        module_revision_id: evidence.module_revision_id.0.clone(),
        module_revision_source_sha256: evidence.module_revision_source_sha256.to_string(),
    })
}

fn summarize_plan(
    plan: &ModuleActivationPlan,
) -> ShellResult<ContentModuleActivationPlanSummaryDto> {
    plan.verify()
        .map_err(|error| invalid_lifecycle(error.to_string()))?;
    let [binding_id] = plan.activation_binding_ids.as_slice() else {
        return Err(invalid_lifecycle(
            "module plan requires exactly one activation",
        ));
    };
    if plan.expected_state_revision > MAX_JAVASCRIPT_SAFE_INTEGER
        || plan.ordered_binding_ids.len() > MAX_LIFECYCLE_BINDINGS
        || plan.import_approvals.len() > MAX_LIFECYCLE_IMPORT_APPROVALS
        || plan.effective_variable_overrides.values.len() > MAX_LIFECYCLE_VARIABLE_OVERRIDES
        || plan.omitted_components.len() > MAX_LIFECYCLE_COMPONENTS
        || plan.components.iter().any(|component| {
            component.coalesced_sources.len() > MAX_LIFECYCLE_SOURCES_PER_CANDIDATE
        })
    {
        return Err(invalid_lifecycle(
            "module plan exceeds bounded metadata or choices",
        ));
    }
    let result = ContentModuleActivationPlanSummaryDto {
        review_sha256: plan.review_sha256.clone(),
        plan_sha256: plan.plan_sha256.clone(),
        expected_state_revision: plan.expected_state_revision,
        activation_binding_id: binding_id.as_str().to_owned(),
        component_count: count(plan.components.len())?,
        runtime_enabled_count: count(
            plan.components
                .iter()
                .filter(|component| component.runtime_enabled)
                .count(),
        )?,
        omitted_component_count: count(plan.omitted_components.len())?,
    };
    validate_serialized("module plan summary", &result)?;
    Ok(result)
}
