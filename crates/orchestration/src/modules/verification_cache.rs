//! Pure verification memo: exact values only, no clocks, I/O, or authority state.

use serde::Serialize;
use std::{
    any::Any,
    collections::VecDeque,
    sync::{Arc, Mutex, OnceLock},
};

const MAX_ENTRIES: usize = 12;
const MAX_SERIALIZED_BYTES: usize = 64 * 1024 * 1024;

struct Entry {
    value: Arc<dyn Any + Send + Sync>,
    bytes: usize,
}

#[derive(Default)]
struct Cache {
    entries: VecDeque<Entry>,
    bytes: usize,
}
static CACHE: OnceLock<Mutex<Cache>> = OnceLock::new();

pub(super) fn verify<T, E>(value: &T, compute: impl FnOnce() -> Result<(), E>) -> Result<(), E>
where
    T: Clone + PartialEq + Serialize + Send + Sync + 'static,
{
    let cache = CACHE.get_or_init(Mutex::default);
    if let Ok(mut guard) = cache.lock()
        && let Some(index) = guard
            .entries
            .iter()
            .position(|entry| entry.value.downcast_ref::<T>() == Some(value))
    {
        if let Some(entry) = guard.entries.remove(index) {
            guard.entries.push_back(entry);
        }
        return Ok(());
    }
    // No cache lock is held across recursively verified child values. Failed
    // verification is never retained. The caller's complete value is compared,
    // not its claimed digest, revision, pointer, or serialization projection.
    compute()?;
    if let Ok(bytes) = serde_json::to_vec(value)
        && bytes.len() <= MAX_SERIALIZED_BYTES
        && let Ok(mut guard) = cache.lock()
    {
        while guard.entries.len() >= MAX_ENTRIES || guard.bytes + bytes.len() > MAX_SERIALIZED_BYTES
        {
            if let Some(oldest) = guard.entries.pop_front() {
                guard.bytes -= oldest.bytes;
            }
        }
        guard.bytes += bytes.len();
        guard.entries.push_back(Entry {
            value: Arc::new(value.clone()),
            bytes: bytes.len(),
        });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[derive(Clone, PartialEq, Serialize)]
    struct Fixture {
        claimed_hash: u8,
        nested: Vec<u8>,
    }

    #[test]
    fn changed_payload_with_same_claimed_hash_is_not_a_hit() {
        let calls = AtomicUsize::new(0);
        let run = |value: &Fixture| {
            verify(value, || {
                calls.fetch_add(1, Ordering::Relaxed);
                if value.nested == [1, 2, 3] {
                    Ok(())
                } else {
                    Err("tampered")
                }
            })
        };
        let mut value = Fixture {
            claimed_hash: 9,
            nested: vec![1, 2, 3],
        };
        assert_eq!(run(&value), Ok(()));
        assert_eq!(run(&value), Ok(()));
        value.nested[2] = 4;
        assert_eq!(run(&value), Err("tampered"));
        assert_eq!(run(&value), Err("tampered"));
        assert_eq!(calls.load(Ordering::Relaxed), 3);
    }
}
