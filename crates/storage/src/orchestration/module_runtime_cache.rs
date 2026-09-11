//! Pure materialization memo. Live DB, binding and CAS checks remain at every caller.

use lorepia_orchestration::{
    AppliedModuleRuntimePlan, ApprovedModuleActivationPlan, ModuleMergeError, ModuleMergeReview,
};
use std::sync::{Arc, Mutex, OnceLock};

static LAST_PLAN: OnceLock<Mutex<Option<Arc<AppliedModuleRuntimePlan>>>> = OnceLock::new();

pub(super) fn materialize(
    approval: &ApprovedModuleActivationPlan,
    review: &ModuleMergeReview,
) -> Result<AppliedModuleRuntimePlan, ModuleMergeError> {
    let cache = LAST_PLAN.get_or_init(Mutex::default);
    let cached = cache.lock().ok().and_then(|entry| entry.clone());
    if let Some(plan) = cached
        && plan.source_approval == *approval
        && plan.review == *review
    {
        return Ok((*plan).clone());
    }
    let plan = lorepia_orchestration::materialize_approved_module_runtime_plan(approval, review)?;
    if serde_json::to_vec(&plan).is_ok_and(|bytes| bytes.len() <= 32 * 1024 * 1024)
        && let Ok(mut entry) = cache.lock()
    {
        *entry = Some(Arc::new(plan.clone()));
    }
    Ok(plan)
}
