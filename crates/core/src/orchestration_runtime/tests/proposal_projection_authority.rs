#[cfg(test)]
mod proposal_projection_authority_tests {
    use lorepia_domain::{
        InteractionProposalDecision, InteractionProposalRecord, InteractionProposalRecordId,
        InteractionProposalStatus, InteractionRuleId, InteractionRuleSetId,
    };
    use lorepia_storage::GenerationAttemptProposalDecision;

    use super::{
        generation_proposal_decision_requires_reviewable_text,
        interaction_proposal_decision_requires_reviewable_text,
        require_reviewable_interaction_proposal_text,
    };

    #[test]
    fn only_approval_requires_reviewable_text_and_normal_text_is_accepted() {
        assert!(interaction_proposal_decision_requires_reviewable_text(
            InteractionProposalDecision::Approve
        ));
        assert!(!interaction_proposal_decision_requires_reviewable_text(
            InteractionProposalDecision::Reject
        ));
        assert!(generation_proposal_decision_requires_reviewable_text(
            GenerationAttemptProposalDecision::Approve
        ));
        assert!(!generation_proposal_decision_requires_reviewable_text(
            GenerationAttemptProposalDecision::Reject
        ));
        assert!(!generation_proposal_decision_requires_reviewable_text(
            GenerationAttemptProposalDecision::Expire
        ));

        let proposal = InteractionProposalRecord {
            id: InteractionProposalRecordId::from("proposal-safe-review"),
            rule_set_id: InteractionRuleSetId::from("rules-safe-review"),
            rule_id: InteractionRuleId::from("rule-safe-review"),
            proposal_id: "action-safe-review".to_owned(),
            title: "검토 가능한 제안".to_owned(),
            body: "정상 크기의 안전한 제안 본문입니다.".to_owned(),
            status: InteractionProposalStatus::Pending,
            source_interaction_state_revision: 1,
            requested_at_epoch_seconds: 1,
            expires_at_epoch_seconds: None,
            decided_at_epoch_seconds: None,
        };
        require_reviewable_interaction_proposal_text(&proposal)
            .expect("normal proposal text remains approvable");
    }
}
