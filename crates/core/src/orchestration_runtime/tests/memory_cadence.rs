#[cfg(test)]
mod memory_cadence_tests {
    use super::{memory_embedding_candidate_limit, next_memory_summary_turn_window};

    #[test]
    fn cadence_selects_the_earliest_contiguous_uncovered_window() {
        assert_eq!(
            next_memory_summary_turn_window(8, 2, &[(0, 1), (4, 5)]).expect("valid cadence"),
            Some((2, 3))
        );
        assert_eq!(
            next_memory_summary_turn_window(6, 2, &[(0, 1), (2, 3), (4, 5)])
                .expect("fully covered cadence"),
            None
        );
    }

    #[test]
    fn cadence_rejects_partial_and_overlapping_ranges() {
        assert!(next_memory_summary_turn_window(6, 2, &[(0, 2)]).is_err());
        assert!(next_memory_summary_turn_window(6, 2, &[(0, 1), (1, 2)]).is_err());
        assert!(next_memory_summary_turn_window(6, 2, &[(4, 3)]).is_err());
    }

    #[test]
    fn embedding_candidate_limit_respects_dimension_and_byte_budgets() {
        assert_eq!(
            memory_embedding_candidate_limit(10_000, 1).expect("minimum dimensions"),
            2_048
        );
        assert_eq!(
            memory_embedding_candidate_limit(10_000, 3_072).expect("common dimensions"),
            1_365
        );
        assert_eq!(
            memory_embedding_candidate_limit(10_000, 32_768).expect("maximum dimensions"),
            128
        );
        assert_eq!(
            memory_embedding_candidate_limit(7, 32_768).expect("record bound"),
            7
        );
        assert!(memory_embedding_candidate_limit(1, 0).is_err());
        assert!(memory_embedding_candidate_limit(1, u32::MAX).is_err());
    }
}
