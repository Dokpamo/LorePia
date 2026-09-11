//! Verification of immutable module reviews and runtime plans.

#[path = "verification_cache.rs"]
mod verification_cache;

use super::{
    AppliedModuleRuntimePlan, ApprovedModuleActivationPlan, ModuleMergeError, ModuleMergeReview,
    ModuleMergeReviewDigest, ResolvedModulePlan, applied_module_runtime_plan_sha256,
    module_activation_approval_sha256, module_merge_resolution_set_from_plan,
    module_merge_review_sha256, resolve_module_merge, resolved_module_plan_sha256,
    validate_activation_approval_id,
};

impl ModuleMergeReview {
    pub fn verify(&self) -> Result<(), ModuleMergeError> {
        verification_cache::verify(self, || self.verify_uncached())
    }

    fn verify_uncached(&self) -> Result<(), ModuleMergeError> {
        let expected = module_merge_review_sha256(ModuleMergeReviewDigest {
            state_revision: self.state_revision,
            context: &self.context,
            activation_binding_ids: &self.activation_binding_ids,
            ordered_bindings: &self.ordered_bindings,
            ignored_bindings: &self.ignored_bindings,
            components: &self.components,
            conflicts: &self.conflicts,
            import_approvals: &self.import_approvals,
            effective_variable_overrides: &self.effective_variable_overrides,
        })?;
        if expected != self.review_sha256 {
            return Err(ModuleMergeError::ReviewHashMismatch);
        }
        Ok(())
    }
}

impl ResolvedModulePlan {
    pub fn verify(&self) -> Result<(), ModuleMergeError> {
        verification_cache::verify(self, || self.verify_uncached())
    }

    fn verify_uncached(&self) -> Result<(), ModuleMergeError> {
        let expected = resolved_module_plan_sha256(
            &self.review_sha256,
            self.expected_state_revision,
            &self.activation_binding_ids,
            &self.ordered_binding_ids,
            &self.components,
            &self.omitted_components,
            &self.import_approvals,
            &self.effective_variable_overrides,
        )?;
        if expected != self.plan_sha256 {
            return Err(ModuleMergeError::PlanHashMismatch);
        }
        Ok(())
    }
}

impl ApprovedModuleActivationPlan {
    pub fn verify(&self) -> Result<(), ModuleMergeError> {
        verification_cache::verify(self, || self.verify_uncached())
    }

    fn verify_uncached(&self) -> Result<(), ModuleMergeError> {
        self.plan.verify()?;
        validate_activation_approval_id(&self.approval_id)?;
        if self.plan.activation_binding_ids.len() != 1 {
            return Err(ModuleMergeError::ActivationPlanRequired);
        }
        let expected = module_activation_approval_sha256(&self.approval_id, &self.plan)?;
        if expected != self.approval_sha256 {
            return Err(ModuleMergeError::ActivationApprovalHashMismatch);
        }
        Ok(())
    }
}

impl AppliedModuleRuntimePlan {
    pub fn verify(&self) -> Result<(), ModuleMergeError> {
        verification_cache::verify(self, || self.verify_uncached())
    }

    fn verify_uncached(&self) -> Result<(), ModuleMergeError> {
        self.source_approval.verify()?;
        self.review.verify()?;
        self.plan.verify()?;
        if !self.review.activation_binding_ids.is_empty()
            || !self.plan.activation_binding_ids.is_empty()
            || self.plan.review_sha256 != self.review.review_sha256
            || self.plan.expected_state_revision != self.review.state_revision
        {
            return Err(ModuleMergeError::InvalidRuntimeMaterialization(
                "runtime plans must resolve one no-pending-binding review".to_owned(),
            ));
        }
        let resolutions = module_merge_resolution_set_from_plan(&self.review, &self.plan)?;
        let reconstructed = resolve_module_merge(&self.review, &resolutions)?;
        if reconstructed != self.plan {
            return Err(ModuleMergeError::InvalidRuntimeMaterialization(
                "runtime plan differs from its reviewed resolution".to_owned(),
            ));
        }
        let expected = applied_module_runtime_plan_sha256(
            &self.source_approval,
            self.derived_from_plan_sha256.as_ref(),
            &self.review,
            &self.plan,
        )?;
        if expected != self.applied_plan_sha256 {
            return Err(ModuleMergeError::RuntimePlanHashMismatch);
        }
        Ok(())
    }
}
