use super::{DraftMessage, MESSAGE_OVERHEAD_TOKENS, TokenEstimator};

/// Select the same contiguous suffix as repeatedly removing the oldest item,
/// estimating each retained item once and moving the prefix only once.
pub(super) fn keep_latest_items<E: TokenEstimator>(
    messages: &mut Vec<DraftMessage>,
    target_tokens: u32,
    estimator: &E,
) {
    let mut remaining = target_tokens;
    let mut keep_from = messages.len();
    for (index, message) in messages.iter().enumerate().rev() {
        let cost = estimator
            .estimate_text(&message.content)
            .saturating_add(MESSAGE_OVERHEAD_TOKENS);
        if cost > remaining {
            break;
        }
        remaining -= cost;
        keep_from = index;
    }
    messages.drain(..keep_from);
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;

    use super::*;
    use crate::resolver::Utf8TokenEstimator;
    use lorepia_domain::{InstructionAuthority, Provenance, RoleHint, SourceKind};

    fn message(content: &str) -> DraftMessage {
        DraftMessage {
            requested_role: RoleHint::User,
            authority: InstructionAuthority::User,
            content: content.to_owned(),
            source_message_ids: Vec::new(),
            source_memory_record_ids: Vec::new(),
            provenance: Provenance {
                source_kind: SourceKind::UserCreated,
                source_id: None,
                source_hash: None,
                author: None,
                license: None,
                imported_at: None,
            },
        }
    }

    struct CountingEstimator(Cell<usize>);

    impl TokenEstimator for CountingEstimator {
        fn id(&self) -> &'static str {
            "counting"
        }
        fn estimate_text(&self, text: &str) -> u32 {
            self.0.set(self.0.get() + 1);
            Utf8TokenEstimator.estimate_text(text)
        }
        fn keep_prefix(&self, text: &str, budget: u32) -> String {
            Utf8TokenEstimator.keep_prefix(text, budget)
        }
        fn keep_suffix(&self, text: &str, budget: u32) -> String {
            Utf8TokenEstimator.keep_suffix(text, budget)
        }
    }

    #[test]
    fn keeps_identical_suffix_for_empty_exact_and_partial_budgets() {
        let original = vec![
            message("old"),
            message(""),
            message("새로운😀"),
            message("last"),
        ];
        for budget in 0..64 {
            let mut expected = original.clone();
            while expected
                .iter()
                .map(|m| Utf8TokenEstimator.estimate_text(&m.content) + MESSAGE_OVERHEAD_TOKENS)
                .sum::<u32>()
                > budget
            {
                expected.remove(0);
            }
            let mut actual = original.clone();
            keep_latest_items(&mut actual, budget, &Utf8TokenEstimator);
            assert_eq!(
                actual.iter().map(|m| &m.content).collect::<Vec<_>>(),
                expected.iter().map(|m| &m.content).collect::<Vec<_>>(),
                "budget={budget}"
            );
        }
        let mut empty = Vec::new();
        keep_latest_items(&mut empty, 0, &Utf8TokenEstimator);
        assert!(empty.is_empty());
    }

    #[test]
    fn large_history_estimates_at_most_once_per_message() {
        let estimator = CountingEstimator(Cell::new(0));
        let original = vec![message("text"); 16_384];
        let mut messages = original.clone();
        keep_latest_items(&mut messages, 5, &estimator);
        assert_eq!(messages.len(), 1);
        assert_eq!(estimator.0.get(), 2);
        estimator.0.set(0);
        messages = original;
        keep_latest_items(&mut messages, u32::MAX, &estimator);
        assert_eq!(messages.len(), 16_384);
        assert_eq!(estimator.0.get(), 16_384);
    }
}
