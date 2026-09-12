use super::State;

const MAX_ENTRIES: usize = 32;
const MAX_ID_BYTES: usize = 64 * 1024;

#[derive(Default)]
pub(in crate::database) struct CheckpointProofCache {
    epoch: Option<(u64, u64)>,
    entries: Vec<Entry>,
    bytes: usize,
    #[cfg(test)]
    pub(super) validations: usize,
}

struct Entry {
    message: String,
    generation: String,
    state: State,
}

impl CheckpointProofCache {
    pub(super) fn contains(
        &mut self,
        epoch: (u64, u64),
        message: &str,
        generation: &str,
        state: &State,
    ) -> bool {
        if self.epoch != Some(epoch) {
            self.entries.clear();
            self.bytes = 0;
            self.epoch = Some(epoch);
        }
        self.entries.iter().any(|entry| {
            entry.message == message && entry.generation == generation && entry.state == *state
        })
    }

    pub(super) fn record(
        &mut self,
        epoch: (u64, u64),
        message: &str,
        generation: &str,
        state: State,
    ) {
        // The caller has committed only this known append since contains(). Other
        // entries remain valid across that precise, owned journal transition.
        self.epoch = Some(epoch);
        if let Some(index) = self
            .entries
            .iter()
            .position(|entry| entry.message == message)
        {
            let old = self.entries.remove(index);
            self.bytes -= old.message.len() + old.generation.len();
        }
        let Some(bytes) = message
            .len()
            .checked_add(generation.len())
            .filter(|bytes| *bytes <= MAX_ID_BYTES)
        else {
            return;
        };
        while self.entries.len() >= MAX_ENTRIES || self.bytes + bytes > MAX_ID_BYTES {
            let old = self.entries.remove(0);
            self.bytes -= old.message.len() + old.generation.len();
        }
        self.bytes += bytes;
        self.entries.push(Entry {
            message: message.to_owned(),
            generation: generation.to_owned(),
            state,
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn state() -> State {
        State {
            base_bytes: 0,
            total_bytes: 1,
            next_sequence: 1,
        }
    }
    #[test]
    fn bounded_entries_ids_and_exact_state() {
        let mut cache = CheckpointProofCache::default();
        for index in 0..33 {
            cache.contains((0, 0), "absent", "g", &state());
            cache.record((0, 0), &index.to_string(), "g", state());
        }
        assert_eq!(cache.entries.len(), MAX_ENTRIES);
        assert!(!cache.contains((0, 0), "0", "g", &state()));
        assert!(cache.contains((0, 0), "32", "g", &state()));
        assert!(!cache.contains((0, 0), "32", "other", &state()));
        let oversized = "x".repeat(MAX_ID_BYTES);
        cache.record((0, 0), &oversized, "g", state());
        assert!(!cache.contains((0, 0), &oversized, "g", &state()));
        assert!(cache.bytes <= MAX_ID_BYTES);
        assert!(!cache.contains((1, 0), "32", "g", &state()));
        cache.record((1, 0), "32", "g", state());
        assert!(!cache.contains((1, 1), "32", "g", &state()));
    }
}
