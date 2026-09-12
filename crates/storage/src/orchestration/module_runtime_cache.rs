//! Pure materialization memo. Live DB, binding and CAS checks remain at every caller.

use lorepia_orchestration::{
    AppliedModuleRuntimePlan, ApprovedModuleActivationPlan, ModuleMergeError, ModuleMergeReview,
};
use std::{
    io::{self, Write},
    sync::{Arc, Mutex, OnceLock},
};

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
    if fits_serialized_budget(&plan, 32 * 1024 * 1024)
        && let Ok(mut entry) = cache.lock()
    {
        *entry = Some(Arc::new(plan.clone()));
    }
    Ok(plan)
}

// Admission needs the encoded length, not an additional full plan buffer.
fn fits_serialized_budget(value: &impl serde::Serialize, maximum: usize) -> bool {
    let mut count = SerializedByteCount(0);
    serde_json::to_writer(&mut count, value).is_ok() && count.0 <= maximum
}

struct SerializedByteCount(usize);

impl Write for SerializedByteCount {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.0 = self
            .0
            .checked_add(bytes.len())
            .ok_or_else(|| io::Error::other("serialized byte count overflow"))?;
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::fits_serialized_budget;

    #[test]
    fn counting_admission_preserves_exact_json_byte_boundaries() {
        for value in [
            serde_json::json!(null),
            serde_json::json!({"text": "한글\n\"\\", "values": [true, -42, 1.5]}),
            serde_json::json!({"text": "x".repeat(1024 * 1024)}),
        ] {
            let bytes = serde_json::to_vec(&value).unwrap();
            assert!(fits_serialized_budget(&value, bytes.len()));
            assert!(!fits_serialized_budget(&value, bytes.len() - 1));
        }
    }

    #[test]
    fn unserializable_values_are_not_admitted() {
        let value = std::collections::BTreeMap::from([(vec![1, 2], "invalid JSON key")]);
        assert!(serde_json::to_vec(&value).is_err());
        assert!(!fits_serialized_budget(&value, usize::MAX));
    }
}
