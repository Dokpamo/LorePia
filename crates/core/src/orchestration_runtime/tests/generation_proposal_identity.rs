#[cfg(test)]
mod generation_proposal_identity_tests {
    use chrono::Utc;
    use lorepia_domain::{
        ConversationBranchId, ConversationId, GenerationId, InteractionProposalRecord,
        InteractionProposalRecordId, InteractionProposalStatus, InteractionRuleId,
        InteractionRuleSetId, InteractionState, Sha256Digest, VariableMap,
    };
    use lorepia_orchestration::InteractionLimits;
    use lorepia_storage::{
        InteractionEvaluationSeal, InteractionEvaluationTemplateValues, InteractionPolicySnapshot,
        StoredGenerationAttemptProposal, interaction_evaluation_seal_sha256,
        interaction_policy_sha256, interaction_proposal_review_sha256,
    };

    use super::{
        CoreErrorCode, interaction_evaluation_limits, remap_generation_attempt_proposal_ids,
        versioned_digest,
    };

    fn digest(value: &str) -> Sha256Digest {
        Sha256Digest::parse(versioned_digest(&("identity-test", value)).expect("test digest"))
            .expect("canonical test digest")
    }

    #[allow(clippy::too_many_lines)]
    fn proposal_fixture(
        status: InteractionProposalStatus,
    ) -> (
        GenerationId,
        InteractionState,
        StoredGenerationAttemptProposal,
    ) {
        let generation_id = GenerationId("generation-identity-test".to_owned());
        let domain_id = InteractionProposalRecordId::from("domain-proposal-record");
        let mut domain_record = InteractionProposalRecord {
            id: domain_id.clone(),
            rule_set_id: InteractionRuleSetId::from("identity-rule-set"),
            rule_id: InteractionRuleId::from("identity-rule"),
            proposal_id: "identity-proposal".to_owned(),
            title: "Identity proposal".to_owned(),
            body: "Verify proposal mapping".to_owned(),
            status: InteractionProposalStatus::Pending,
            source_interaction_state_revision: 0,
            requested_at_epoch_seconds: 1,
            expires_at_epoch_seconds: Some(60),
            decided_at_epoch_seconds: None,
        };
        let domain_review_sha256 = Sha256Digest::parse(
            interaction_proposal_review_sha256(&domain_record).expect("domain review digest"),
        )
        .expect("canonical domain review digest");
        let before_event_snapshot_sha256 = digest("before-event");
        let storage_id = InteractionProposalRecordId::from(format!(
            "attempt-proposal-{}",
            versioned_digest(&(
                "lorepia.generation-attempt-proposal-record.v1",
                &generation_id,
                &domain_id,
                domain_review_sha256.as_str(),
                before_event_snapshot_sha256.as_str(),
            ))
            .expect("storage proposal id")
        ));
        domain_record.id = storage_id;
        let storage_review_sha256 = Sha256Digest::parse(
            interaction_proposal_review_sha256(&domain_record).expect("storage review digest"),
        )
        .expect("canonical storage review digest");
        domain_record.status = status;
        domain_record.decided_at_epoch_seconds = match status {
            InteractionProposalStatus::Pending => None,
            InteractionProposalStatus::Expired => Some(60),
            InteractionProposalStatus::Approved | InteractionProposalStatus::Rejected => Some(2),
        };
        let now = Utc::now();
        let state = InteractionState {
            variables: VariableMap::default(),
            manually_active_knowledge: Vec::new(),
            proposals: vec![domain_record.clone()],
            revision: 1,
        };
        let origin_policy = InteractionPolicySnapshot {
            module_plan_sha256: None,
            rule_sets: Vec::new(),
        };
        let origin_policy_sha256 = Sha256Digest::parse(
            interaction_policy_sha256(&origin_policy).expect("origin policy digest"),
        )
        .expect("canonical origin policy digest");
        let origin_evaluation_seal = InteractionEvaluationSeal {
            schema_version: 1,
            engine_contract_version: 1,
            policy_sha256: origin_policy_sha256.clone(),
            executable_rule_sets_sha256: digest("executable-policy"),
            knowledge_revisions: Vec::new(),
            asset_action_diagnostics: Vec::new(),
            approved_import_source_ids: Vec::new(),
            policy_variables: VariableMap::default(),
            supported_capabilities: Vec::new(),
            template_values: InteractionEvaluationTemplateValues {
                character_name: Some("Identity".to_owned()),
                user_name: Some("User".to_owned()),
                persona_name: None,
                persona_description: None,
                current_date: Some("1970-01-01".to_owned()),
                current_time: Some("00:00:01+00:00".to_owned()),
            },
            event_epoch_seconds: 1,
            limits: interaction_evaluation_limits(InteractionLimits::default()),
            seed_contract_version: 1,
        };
        let origin_evaluation_seal_sha256 =
            interaction_evaluation_seal_sha256(&origin_evaluation_seal)
                .expect("origin evaluation seal digest");
        let proposal = StoredGenerationAttemptProposal {
            generation_id: generation_id.clone(),
            conversation_id: ConversationId("identity-conversation".to_owned()),
            source_branch_id: ConversationBranchId("identity-source".to_owned()),
            proposed_branch_id: ConversationBranchId("identity-target".to_owned()),
            ordinal: 0,
            record: domain_record,
            domain_proposal_record_id: domain_id,
            before_event_snapshot_sha256,
            origin_policy,
            origin_policy_sha256,
            origin_event_id: "identity-origin-event".to_owned(),
            origin_chain_ordinal: 0,
            origin_aggregate_revision: 1,
            origin_evaluation_seal,
            origin_evaluation_seal_sha256,
            rule_set_revision_id: "identity-rule-set-revision".to_owned(),
            action_ordinal: 0,
            action_payload_sha256: digest("action"),
            proposal_revision: if status == InteractionProposalStatus::Pending {
                1
            } else {
                2
            },
            proposal_review_sha256: storage_review_sha256,
            domain_proposal_review_sha256: domain_review_sha256,
            storage_identity_version: 2,
            decision_idempotency_key: None,
            decision_event_id: None,
            decision_event_sha256: None,
            resulting_aggregate_revision: None,
            decided_at_epoch_seconds: None,
            created_at: now,
            updated_at: now,
        };
        (generation_id, state, proposal)
    }

    #[test]
    fn proposal_identity_mapping_is_total_for_pending_and_terminal_dispositions() {
        for status in [
            InteractionProposalStatus::Pending,
            InteractionProposalStatus::Approved,
            InteractionProposalStatus::Rejected,
            InteractionProposalStatus::Expired,
        ] {
            let (generation_id, storage_state, proposal) = proposal_fixture(status);
            let domain_state = remap_generation_attempt_proposal_ids(
                &generation_id,
                &storage_state,
                std::slice::from_ref(&proposal),
                true,
            )
            .expect("map exact storage proposal to its domain identity");
            assert_eq!(
                domain_state.proposals[0].id,
                proposal.domain_proposal_record_id
            );
            assert_eq!(domain_state.proposals[0].status, status);
            assert_eq!(
                remap_generation_attempt_proposal_ids(
                    &generation_id,
                    &domain_state,
                    std::slice::from_ref(&proposal),
                    false,
                )
                .expect("map exact domain proposal back to storage"),
                storage_state
            );
        }
    }

    #[test]
    fn proposal_identity_mapping_rejects_tampered_missing_and_extraneous_records() {
        let (generation_id, state, proposal) =
            proposal_fixture(InteractionProposalStatus::Approved);
        let mut tampered = proposal.clone();
        tampered.domain_proposal_record_id =
            InteractionProposalRecordId::from("tampered-domain-record");
        let error =
            remap_generation_attempt_proposal_ids(&generation_id, &state, &[tampered], true)
                .expect_err("tampered domain mapping must fail closed");
        assert_eq!(error.code, CoreErrorCode::StorageCorrupted);

        let error = remap_generation_attempt_proposal_ids(&generation_id, &state, &[], true)
            .expect_err("missing attempt proposal mapping must fail closed");
        assert_eq!(error.code, CoreErrorCode::StorageCorrupted);

        let mut extraneous = proposal.clone();
        extraneous.record.id =
            InteractionProposalRecordId::from(format!("attempt-proposal-{}", "0".repeat(64)));
        let error = remap_generation_attempt_proposal_ids(
            &generation_id,
            &state,
            &[proposal, extraneous],
            true,
        )
        .expect_err("extraneous storage mapping must fail closed");
        assert_eq!(error.code, CoreErrorCode::StorageCorrupted);
    }
}
