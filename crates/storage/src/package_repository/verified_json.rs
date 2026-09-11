//! Bounded memoization of pure JSON validation, never repository authority.

use std::{
    any::Any,
    collections::VecDeque,
    sync::{Arc, Mutex, OnceLock},
};

use super::{
    CompactPackageApprovalPayload, CoreResult, PackageApprovalPayload, PackageReview,
    SelectiveImportPlan, VersionedJson, decode_json, storage_corrupted,
};

const MAX_ENTRIES: usize = 8;
const MAX_INPUT_BYTES: usize = 16 * 1024 * 1024;

struct Entry {
    kind: &'static str,
    inputs: Vec<String>,
    selection: Option<VersionedJson>,
    bytes: usize,
    value: Arc<dyn Any + Send + Sync>,
}

#[derive(Default)]
struct Cache {
    entries: VecDeque<Entry>,
    bytes: usize,
}

static CACHE: OnceLock<Mutex<Cache>> = OnceLock::new();

// The exact input bytes, including the full selection for compact approvals,
// are compared. A hit proves only that this pure decoder accepted these bytes
// before; callers still query and validate every live DB/CAS relationship.
fn memoize<T: Clone + Send + Sync + 'static>(
    kind: &'static str,
    inputs: &[&str],
    compute: impl FnOnce() -> CoreResult<T>,
) -> CoreResult<T> {
    memoize_with_selection(kind, inputs, None, compute)
}

fn memoize_with_selection<T: Clone + Send + Sync + 'static>(
    kind: &'static str,
    inputs: &[&str],
    selection: Option<&VersionedJson>,
    compute: impl FnOnce() -> CoreResult<T>,
) -> CoreResult<T> {
    let cache = CACHE.get_or_init(Mutex::default);
    {
        let mut guard = cache
            .lock()
            .map_err(|_| storage_corrupted("package validation cache is poisoned"))?;
        if let Some(index) = guard.entries.iter().position(|entry| {
            entry.kind == kind
                && entry.selection.as_ref() == selection
                && entry
                    .inputs
                    .iter()
                    .map(String::as_str)
                    .eq(inputs.iter().copied())
        }) {
            let entry = guard.entries.remove(index).expect("located cache entry");
            let result = entry.value.downcast_ref::<T>().cloned();
            guard.entries.push_back(entry);
            if let Some(result) = result {
                return Ok(result);
            }
        }
    }
    let value = compute()?;
    let selection_bytes = selection
        .map(serde_json::to_vec)
        .transpose()
        .map_err(|error| storage_corrupted(format!("stored selection cannot be encoded: {error}")))?
        .map_or(0, |bytes| bytes.len());
    let bytes = inputs.iter().map(|input| input.len()).sum::<usize>() + selection_bytes;
    if bytes <= MAX_INPUT_BYTES {
        let mut guard = cache
            .lock()
            .map_err(|_| storage_corrupted("package validation cache is poisoned"))?;
        while guard.entries.len() >= MAX_ENTRIES || guard.bytes + bytes > MAX_INPUT_BYTES {
            if let Some(oldest) = guard.entries.pop_front() {
                guard.bytes -= oldest.bytes;
            }
        }
        guard.bytes += bytes;
        guard.entries.push_back(Entry {
            kind,
            inputs: inputs.iter().map(|input| (*input).to_owned()).collect(),
            selection: selection.cloned(),
            bytes,
            value: Arc::new(value.clone()),
        });
    }
    Ok(value)
}

pub(super) fn inspection(json: &str) -> CoreResult<(VersionedJson, PackageReview)> {
    memoize("inspection", &[json], || {
        let wrapper: VersionedJson = decode_json("package inspection", json)?;
        if wrapper.schema_version != 1 {
            return Err(storage_corrupted(
                "package inspection wrapper schema is unsupported",
            ));
        }
        let review: PackageReview =
            serde_json::from_value(wrapper.value.clone()).map_err(|error| {
                storage_corrupted(format!("stored package review is invalid: {error}"))
            })?;
        review.verify().map_err(|error| {
            storage_corrupted(format!("stored package review is invalid: {error}"))
        })?;
        Ok((wrapper, review))
    })
}

pub(super) fn selection(json: &str) -> CoreResult<(VersionedJson, SelectiveImportPlan)> {
    memoize("selection", &[json], || {
        let wrapper: VersionedJson = decode_json("package selection", json)?;
        if wrapper.schema_version != 1 {
            return Err(storage_corrupted(
                "package selection wrapper schema is unsupported",
            ));
        }
        let plan: SelectiveImportPlan =
            serde_json::from_value(wrapper.value.clone()).map_err(|error| {
                storage_corrupted(format!("stored package selection is invalid: {error}"))
            })?;
        plan.verify().map_err(|error| {
            storage_corrupted(format!("stored package selection is invalid: {error}"))
        })?;
        Ok((wrapper, plan))
    })
}

pub(super) fn approval(
    json: &str,
    selection: Option<&VersionedJson>,
) -> CoreResult<PackageApprovalPayload> {
    memoize_with_selection("approval", &[json], selection, || {
        let wrapper: VersionedJson = decode_json("package approval", json)?;
        let approved: PackageApprovalPayload = match wrapper.schema_version {
            1 => serde_json::from_value(wrapper.value).map_err(|error| {
                storage_corrupted(format!("stored package approval is invalid: {error}"))
            })?,
            2 => {
                let compact: CompactPackageApprovalPayload = serde_json::from_value(wrapper.value)
                    .map_err(|error| {
                        storage_corrupted(format!("stored package approval is invalid: {error}"))
                    })?;
                let selection =
                    selection.ok_or_else(|| storage_corrupted("package selection is missing"))?;
                let plan: SelectiveImportPlan = serde_json::from_value(selection.value.clone())
                    .map_err(|error| {
                        storage_corrupted(format!("stored package selection is invalid: {error}"))
                    })?;
                compact.into_payload(&plan)?
            }
            _ => {
                return Err(storage_corrupted(
                    "stored package approval wrapper schema is unsupported",
                ));
            }
        };
        approved.plan.verify().map_err(|error| {
            storage_corrupted(format!("stored package approval is invalid: {error}"))
        })?;
        Ok(approved)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[test]
    fn only_exact_successful_inputs_are_memoized() {
        let calls = AtomicUsize::new(0);
        let run = |input: &str| {
            memoize("test-exact", &[input], || {
                calls.fetch_add(1, Ordering::Relaxed);
                if input == "corrupt" {
                    return Err(storage_corrupted("invalid"));
                }
                Ok(input.len())
            })
        };
        assert_eq!(run("accepted").unwrap(), 8);
        assert_eq!(run("accepted").unwrap(), 8);
        assert!(run("corrupt").is_err());
        assert!(run("corrupt").is_err());
        assert_eq!(run("accepted ").unwrap(), 9);
        assert_eq!(calls.load(Ordering::Relaxed), 4);
    }
}
