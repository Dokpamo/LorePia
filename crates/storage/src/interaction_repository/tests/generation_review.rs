use super::generation_support::*;
use super::*;

#[test]
fn generation_attempt_closure_rejects_self_rehashed_malformed_authority() {
    let generation_id = GenerationId("attempt-malformed-closure".to_owned());
    let policy = empty_policy();
    let seal = synthetic_evaluation_seal(&policy);
    let base = synthetic_closure(
        &generation_id,
        "malformed-closure-root",
        InteractionEvent::BeforeGeneration,
        &policy,
        &seal,
        &empty_state(0),
        &empty_state(1),
        &[],
        &[],
        &[],
        &[],
        &[],
    );

    let mut malformed = base.clone();
    malformed.transitions[0].depth = 1;
    malformed.chain_sha256 = crate::generation_attempt_derived_chain_sha256(&malformed)
        .expect("rehash malformed root depth");
    assert!(crate::generation_attempt_derived_closure_sha256(&malformed).is_err());

    let mut malformed = base.clone();
    malformed.final_knowledge = vec![InteractionKnowledgeBinding {
        book_revision_id: "malformed-book-revision".to_owned(),
        entry_id: KnowledgeEntryId::from("malformed-entry"),
    }];
    malformed.chain_sha256 = crate::generation_attempt_derived_chain_sha256(&malformed)
        .expect("rehash malformed final knowledge");
    assert!(crate::generation_attempt_derived_closure_sha256(&malformed).is_err());

    let mut malformed = base.clone();
    let mut guard = crate::GenerationAttemptDerivedGuardAudit {
        kind: crate::GenerationAttemptDerivedGuardKind::Cycle,
        candidate_event_sha256: Some(malformed.transitions[0].event_sha256.clone()),
        parent_ordinal: 0,
        depth: 1,
        suppressed_count: 0,
        evidence_sha256: Sha256Digest::parse("0".repeat(64)).expect("placeholder guard digest"),
    };
    guard.evidence_sha256 = crate::generation_attempt_derived_guard_evidence_sha256(&guard)
        .expect("rehash malformed guard");
    malformed.guard_audits.push(guard);
    malformed.guard_count = 1;
    malformed.chain_sha256 = crate::generation_attempt_derived_chain_sha256(&malformed)
        .expect("rehash malformed guard closure");
    assert!(crate::generation_attempt_derived_closure_sha256(&malformed).is_err());

    let mut malformed = base;
    malformed.transitions.clear();
    malformed.event_count = 0;
    malformed.chain_sha256 =
        crate::generation_attempt_derived_chain_sha256(&malformed).expect("rehash empty closure");
    assert!(crate::generation_attempt_derived_closure_sha256(&malformed).is_err());
}

#[test]
fn concurrent_same_boundary_reviews_receive_distinct_storage_identities() {
    let fixture = generation_approval_fixture(false);
    let second = parallel_generation_commit(
        &fixture,
        "generation-approval-operation-2",
        "generation-attempt-before-review-2",
        &fixture.source_key,
    );
    let first_review = fixture
        .storage
        .commit_generation_attempt_before_review(&fixture.commit)
        .expect("stage first concurrent review");
    let second_review = fixture
        .storage
        .commit_generation_attempt_before_review(&second)
        .expect("stage second concurrent review");
    let first = fixture
        .storage
        .list_generation_attempt_proposals(
            &fixture.commit.generation_id,
            InteractionProposalStatus::Pending,
            2,
        )
        .expect("list first concurrent proposal")
        .pop()
        .expect("first concurrent proposal");
    let second_proposal = fixture
        .storage
        .list_generation_attempt_proposals(
            &second.generation_id,
            InteractionProposalStatus::Pending,
            2,
        )
        .expect("list second concurrent proposal")
        .pop()
        .expect("second concurrent proposal");
    assert_eq!(
        first_review.domain_review_sha256,
        second_review.domain_review_sha256
    );
    assert_ne!(first_review.review_sha256, second_review.review_sha256);
    assert_eq!(
        first.domain_proposal_record_id,
        second_proposal.domain_proposal_record_id
    );
    assert_eq!(
        first.domain_proposal_review_sha256,
        second_proposal.domain_proposal_review_sha256
    );
    assert_ne!(first.record.id, second_proposal.record.id);
    assert_ne!(
        first.proposal_review_sha256,
        second_proposal.proposal_review_sha256
    );
    assert_eq!(first.storage_identity_version, 2);
    assert_eq!(second_proposal.storage_identity_version, 2);
}

#[test]
fn same_second_cross_room_reviews_receive_distinct_storage_identities() {
    let fixture = generation_approval_fixture(false);
    let (character_id, character_name) = fixture
        .storage
        .connection()
        .expect("open cross-room character metadata")
        .query_row(
            "SELECT character.id, character.name
             FROM characters AS character
             JOIN conversations AS conversation
               ON conversation.character_id = character.id
             WHERE conversation.id = ?1",
            [fixture.source_key.conversation_id.0.as_str()],
            |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
        )
        .expect("load cross-room character");
    let conversation = Conversation::new(&character_id, &character_name);
    let (_, conversation_state) = fixture
        .storage
        .save_conversation_with_mode(&conversation, ConversationMode::Chat)
        .expect("save cross-room conversation");
    let cross_room_key = InteractionStateKey {
        state_id: "generation-attempt-cross-room-state".to_owned(),
        conversation_id: conversation.id,
        branch_id: conversation_state.active_branch_id,
    };
    let cross_room = parallel_generation_commit(
        &fixture,
        "generation-cross-room-operation",
        "generation-cross-room-before-review",
        &cross_room_key,
    );
    let first_review = fixture
        .storage
        .commit_generation_attempt_before_review(&fixture.commit)
        .expect("stage source-room review");
    let second_review = fixture
        .storage
        .commit_generation_attempt_before_review(&cross_room)
        .expect("stage cross-room review in same second");
    let first = fixture
        .storage
        .list_generation_attempt_proposals(
            &fixture.commit.generation_id,
            InteractionProposalStatus::Pending,
            2,
        )
        .expect("list source-room proposal")
        .pop()
        .expect("source-room proposal");
    let second = fixture
        .storage
        .list_generation_attempt_proposals(
            &cross_room.generation_id,
            InteractionProposalStatus::Pending,
            2,
        )
        .expect("list cross-room proposal")
        .pop()
        .expect("cross-room proposal");
    assert_eq!(
        first_review.domain_review_sha256,
        second_review.domain_review_sha256
    );
    assert_ne!(first_review.review_sha256, second_review.review_sha256);
    assert_eq!(
        first.domain_proposal_record_id,
        second.domain_proposal_record_id
    );
    assert_ne!(first.record.id, second.record.id);
    assert_ne!(
        first.before_event_snapshot_sha256,
        second.before_event_snapshot_sha256
    );
}

#[test]
fn generation_proposal_domain_identity_tampering_is_blocked_and_detected() {
    let fixture = generation_approval_fixture(false);
    fixture
        .storage
        .commit_generation_attempt_before_review(&fixture.commit)
        .expect("stage proposal for domain tamper test");
    let proposal = fixture
        .storage
        .list_generation_attempt_proposals(
            &fixture.commit.generation_id,
            InteractionProposalStatus::Pending,
            1,
        )
        .expect("list domain tamper proposal")
        .pop()
        .expect("domain tamper proposal");
    let tampered_domain_id = "ffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff";
    let connection = fixture
        .storage
        .connection()
        .expect("open domain tamper database");
    assert!(
        connection
            .execute(
                "UPDATE generation_attempt_proposals
                 SET domain_proposal_record_id = ?2
                 WHERE proposal_record_id = ?1",
                params![proposal.record.id.as_str(), tampered_domain_id],
            )
            .is_err(),
        "immutable identity trigger must block domain-ID tampering"
    );
    connection
        .execute_batch("DROP TRIGGER generation_attempt_proposals_transition_guard")
        .expect("disable guard to simulate on-disk domain corruption");
    connection
        .execute(
            "UPDATE generation_attempt_proposals
             SET domain_proposal_record_id = ?2
             WHERE proposal_record_id = ?1",
            params![proposal.record.id.as_str(), tampered_domain_id],
        )
        .expect("inject domain identity corruption");
    drop(connection);
    let error = fixture
        .storage
        .get_generation_attempt_proposal(&proposal.record.id)
        .expect_err("tampered domain identity must fail closed");
    assert_eq!(error.code, CoreErrorCode::StorageCorrupted);
}

#[test]
fn generation_proposal_storage_identity_tampering_is_detected() {
    let fixture = generation_approval_fixture(false);
    fixture
        .storage
        .commit_generation_attempt_before_review(&fixture.commit)
        .expect("stage proposal for storage tamper test");
    let proposal = fixture
        .storage
        .list_generation_attempt_proposals(
            &fixture.commit.generation_id,
            InteractionProposalStatus::Pending,
            1,
        )
        .expect("list storage tamper proposal")
        .pop()
        .expect("storage tamper proposal");
    let tampered_storage_id =
        InteractionProposalRecordId::from(format!("attempt-proposal-{}", "0".repeat(64)));
    let connection = fixture
        .storage
        .connection()
        .expect("open storage tamper database");
    connection
        .execute_batch("DROP TRIGGER generation_attempt_proposals_transition_guard")
        .expect("disable guard to simulate on-disk storage corruption");
    connection
        .execute(
            "UPDATE generation_attempt_proposals
             SET proposal_record_id = ?2
             WHERE proposal_record_id = ?1",
            params![proposal.record.id.as_str(), tampered_storage_id.as_str()],
        )
        .expect("inject storage identity corruption");
    drop(connection);
    let error = fixture
        .storage
        .get_generation_attempt_proposal(&tampered_storage_id)
        .expect_err("tampered storage identity must fail closed");
    assert_eq!(error.code, CoreErrorCode::StorageCorrupted);
}

#[test]
#[allow(clippy::too_many_lines)]
fn generation_attempt_decision_handshake_rejects_partial_and_mismatched_sql() {
    let fixture = generation_approval_fixture(false);
    fixture
        .storage
        .commit_generation_attempt_before_review(&fixture.commit)
        .expect("stage proposal for decision handshake tampering");
    let proposal = fixture
        .storage
        .list_generation_attempt_proposals(
            &fixture.commit.generation_id,
            InteractionProposalStatus::Pending,
            1,
        )
        .expect("list decision handshake proposal")
        .pop()
        .expect("decision handshake proposal");
    let aggregate = fixture
        .storage
        .get_generation_attempt_interaction_aggregate(&fixture.commit.generation_id)
        .expect("load decision handshake aggregate");
    assert_eq!(
        generation_decision_handshake_counts(&fixture.storage, &fixture.commit.generation_id),
        (0, 0)
    );

    let resulting_state_revision = aggregate
        .state
        .revision
        .checked_add(1)
        .expect("direct resulting state revision");
    let state_snapshot_sha256 = sha256_hex(b"direct generation decision state");
    {
        let mut connection = fixture
            .storage
            .connection()
            .expect("open proposal-only tamper transaction");
        let transaction = connection
            .transaction()
            .expect("begin proposal-only tamper transaction");
        assert_eq!(
            direct_terminalize_generation_proposal(
                &transaction,
                &proposal.record.id,
                2,
                resulting_state_revision,
                &state_snapshot_sha256,
                "2026-08-09T01:00:00Z",
            )
            .expect("stage proposal-only terminal write"),
            1
        );
        assert!(
            transaction.commit().is_err(),
            "a proposal decision without its aggregate binding must fail at commit"
        );
    }
    assert_pending_generation_handshake_unchanged(&fixture, &proposal, &aggregate);

    {
        let connection = fixture
            .storage
            .connection()
            .expect("open aggregate-only tamper connection");
        let error = direct_advance_generation_aggregate(
            &connection,
            &fixture.commit.generation_id,
            2,
            resulting_state_revision,
            &state_snapshot_sha256,
            "2026-08-09T01:01:00Z",
        )
        .expect_err("aggregate-only decision write must fail");
        assert!(
            error
                .to_string()
                .contains("generation attempt aggregate transition is invalid"),
            "unexpected aggregate-only rejection: {error}"
        );
    }
    assert_pending_generation_handshake_unchanged(&fixture, &proposal, &aggregate);

    {
        let mut connection = fixture
            .storage
            .connection()
            .expect("open mismatched decision transaction");
        let transaction = connection
            .transaction()
            .expect("begin mismatched decision transaction");
        assert_eq!(
            direct_terminalize_generation_proposal(
                &transaction,
                &proposal.record.id,
                3,
                resulting_state_revision,
                &state_snapshot_sha256,
                "2026-08-09T01:02:00Z",
            )
            .expect("stage mismatched proposal terminal write"),
            1
        );
        let error = direct_advance_generation_aggregate(
            &transaction,
            &fixture.commit.generation_id,
            2,
            resulting_state_revision,
            &state_snapshot_sha256,
            "2026-08-09T01:02:00Z",
        )
        .expect_err("mismatched resulting aggregate revision must fail");
        assert!(
            error
                .to_string()
                .contains("aggregate update has no exact proposal decision"),
            "unexpected mismatched-decision rejection: {error}"
        );
        transaction
            .rollback()
            .expect("roll back mismatched decision transaction");
    }
    assert_pending_generation_handshake_unchanged(&fixture, &proposal, &aggregate);
}

#[test]
fn generation_attempt_decision_handshake_backfills_terminal_history() {
    let fixture = generation_approval_fixture(false);
    let (_, _, receipt) = seal_approved_generation_fixture(&fixture);
    let proposal_before = fixture
        .storage
        .get_generation_attempt_proposal(&receipt.proposal.record.id)
        .expect("load terminal proposal before handshake rebuild");
    let aggregate_before = fixture
        .storage
        .get_generation_attempt_interaction_aggregate(&fixture.commit.generation_id)
        .expect("load terminal aggregate before handshake rebuild");
    assert_eq!(
        generation_decision_handshake_counts(&fixture.storage, &fixture.commit.generation_id),
        (1, 1)
    );

    {
        let mut connection = fixture
            .storage
            .connection()
            .expect("open handshake rebuild transaction");
        let transaction = connection
            .transaction()
            .expect("begin handshake rebuild transaction");
        transaction
            .execute_batch(
                "DROP TRIGGER generation_attempt_decision_binding_insert_guard;
                 DROP TRIGGER generation_attempt_decision_binding_no_update;
                 DROP TRIGGER generation_attempt_decision_binding_no_delete;
                 DROP TRIGGER generation_attempt_decision_commit_insert_guard;
                 DROP TRIGGER generation_attempt_decision_commit_no_update;
                 DROP TRIGGER generation_attempt_decision_commit_no_delete;
                 DROP TRIGGER generation_attempt_proposals_terminal_insert_guard;
                 DROP TRIGGER generation_attempt_aggregate_insert_guard_v2;
                 DROP TRIGGER generation_attempt_proposal_decision_commit;
                 DROP TRIGGER generation_attempt_aggregate_decision_bind;
                 DROP TABLE generation_attempt_proposal_decision_commits;
                 DROP TABLE generation_attempt_aggregate_decision_bindings;",
            )
            .expect("remove only the version-29 handshake layer");
        transaction
            .execute_batch(include_str!(
                "../../../migrations/0029_generation_attempt_decision_handshake.sql"
            ))
            .expect("reapply version-29 handshake migration");
        let foreign_key_violation = transaction
            .query_row("PRAGMA foreign_key_check", [], |_| Ok(()))
            .optional()
            .expect("check rebuilt handshake foreign keys");
        assert!(foreign_key_violation.is_none());
        transaction
            .commit()
            .expect("commit rebuilt generation decision handshake");
    }

    assert_eq!(
        fixture
            .storage
            .get_generation_attempt_proposal(&proposal_before.record.id)
            .expect("reload terminal proposal after handshake rebuild"),
        proposal_before
    );
    assert_eq!(
        fixture
            .storage
            .get_generation_attempt_interaction_aggregate(&fixture.commit.generation_id)
            .expect("reload terminal aggregate after handshake rebuild"),
        aggregate_before
    );
    assert_eq!(
        generation_decision_handshake_counts(&fixture.storage, &fixture.commit.generation_id),
        (1, 1)
    );
    assert_generation_attempt_has_no_live_mutation(&fixture.storage, &fixture.source_key);
}

#[test]
fn generation_attempt_approval_is_idempotent_and_cas_isolated() {
    let fixture = generation_approval_fixture(false);
    fixture
        .storage
        .commit_generation_attempt_before_review(&fixture.commit)
        .expect("stage generation proposal for approval");
    let aggregate = fixture
        .storage
        .get_generation_attempt_interaction_aggregate(&fixture.commit.generation_id)
        .expect("load pending generation aggregate");
    let proposal = fixture
        .storage
        .list_generation_attempt_proposals(
            &fixture.commit.generation_id,
            InteractionProposalStatus::Pending,
            1,
        )
        .expect("list pending generation proposal")
        .pop()
        .expect("load pending generation proposal");
    let decided_at_epoch_seconds = proposal.record.requested_at_epoch_seconds + 1;
    let domain_state = remap_generation_attempt_test_state(
        &fixture.storage,
        &fixture.commit.generation_id,
        &aggregate.state,
        true,
    );
    let domain_decision_state = approve_pending(
        &domain_state,
        &proposal.record.proposal_id,
        domain_state.revision,
        decided_at_epoch_seconds,
    )
    .expect("derive approved proposal state")
    .state;
    let decision_state = remap_generation_attempt_test_state(
        &fixture.storage,
        &fixture.commit.generation_id,
        &domain_decision_state,
        false,
    );
    let mut derived_next_state = decision_state.clone();
    derived_next_state.revision += 1;
    let updated_at = fixture.commit.occurred_at + Duration::seconds(1);
    let evaluation_seal = proposal.origin_evaluation_seal.clone();
    let user_action = InteractionEvent::UserAction {
        action_id: proposal.record.proposal_id.clone(),
    };
    let derived = InteractionDerivedEventCommit {
        event_id: "generation-approval-user-action".to_owned(),
        idempotency_key: "generation-approval-user-action-key".to_owned(),
        policy: fixture.policy.clone(),
        evaluation_seal: None,
        deterministic_seed: None,
        next_state: derived_next_state.clone(),
        knowledge: Vec::new(),
        action_results: Vec::new(),
        effects: Vec::new(),
        derived_events: Vec::new(),
        proposals: Vec::new(),
        created_at: updated_at,
    };
    let derived_closure = synthetic_closure(
        &fixture.commit.generation_id,
        &derived.event_id,
        user_action,
        &fixture.policy,
        &evaluation_seal,
        &decision_state,
        &derived_next_state,
        &derived.knowledge,
        &derived.action_results,
        &derived.effects,
        &derived.derived_events,
        &derived.proposals,
    );
    let commit = GenerationAttemptProposalDecisionCommit {
        proposal_record_id: proposal.record.id.clone(),
        expected_proposal_revision: proposal.proposal_revision,
        expected_aggregate_revision: aggregate.aggregate_revision,
        decision: GenerationAttemptProposalDecision::Approve,
        decision_idempotency_key: "generation-approval-decision".to_owned(),
        decided_at_epoch_seconds,
        decision_state,
        current_policy: Some(fixture.policy.clone()),
        evaluation_seal: Some(evaluation_seal),
        derived_closure: Some(derived_closure),
        derived: Some(derived),
        updated_at,
    };

    let first = fixture
        .storage
        .decide_generation_attempt_proposal(&commit)
        .expect("approve isolated generation proposal");
    assert!(!first.exact_replay);
    assert_eq!(
        first.proposal.record.status,
        InteractionProposalStatus::Approved
    );
    assert_eq!(first.proposal.proposal_revision, 2);
    assert_eq!(first.aggregate.aggregate_revision, 2);
    assert_eq!(first.aggregate.pending_proposal_count, 0);
    assert_eq!(first.aggregate.terminal_decision_count, 1);
    assert_eq!(first.aggregate.state.revision, 3);
    assert_eq!(
        first.aggregate.decision_event_ids,
        vec!["generation-approval-user-action"]
    );
    let evidence = first
        .approval_evidence
        .as_ref()
        .expect("approval must seal generation evidence");
    assert_eq!(
        evidence.decision_event_ids,
        first.aggregate.decision_event_ids
    );
    assert_eq!(evidence.resulting_state_revision, 3);
    assert_eq!(
        fixture
            .storage
            .get_generation_attempt(&fixture.commit.generation_id)
            .expect("load approved generation attempt")
            .status,
        GenerationAttemptStatus::BeforeGenerationApplied
    );

    let replay = fixture
        .storage
        .decide_generation_attempt_proposal(&commit)
        .expect("replay exact generation approval");
    assert!(replay.exact_replay);
    assert_eq!(
        replay.approval_evidence_sha256,
        first.approval_evidence_sha256
    );

    let mut stale = commit.clone();
    stale.decision_idempotency_key = "generation-approval-stale-cas".to_owned();
    let error = fixture
        .storage
        .decide_generation_attempt_proposal(&stale)
        .expect_err("stale generation approval CAS must fail");
    assert_eq!(error.code, CoreErrorCode::InvalidInput);
    assert_generation_attempt_has_no_live_mutation(&fixture.storage, &fixture.source_key);
}

#[test]
fn generation_attempt_rejection_uses_domain_identity_and_seals_storage_state() {
    let fixture = generation_approval_fixture(false);
    fixture
        .storage
        .commit_generation_attempt_before_review(&fixture.commit)
        .expect("stage generation proposal for rejection");
    let aggregate = fixture
        .storage
        .get_generation_attempt_interaction_aggregate(&fixture.commit.generation_id)
        .expect("load rejection aggregate");
    let proposal = fixture
        .storage
        .list_generation_attempt_proposals(
            &fixture.commit.generation_id,
            InteractionProposalStatus::Pending,
            1,
        )
        .expect("list rejection proposal")
        .pop()
        .expect("rejection proposal");
    let domain_state = remap_generation_attempt_test_state(
        &fixture.storage,
        &fixture.commit.generation_id,
        &aggregate.state,
        true,
    );
    let decided_at_epoch_seconds = proposal.record.requested_at_epoch_seconds + 1;
    let domain_decision_state = reject_pending(
        &domain_state,
        &proposal.record.proposal_id,
        domain_state.revision,
        decided_at_epoch_seconds,
    )
    .expect("derive rejected domain state")
    .state;
    let decision_state = remap_generation_attempt_test_state(
        &fixture.storage,
        &fixture.commit.generation_id,
        &domain_decision_state,
        false,
    );
    let receipt = fixture
        .storage
        .decide_generation_attempt_proposal(&GenerationAttemptProposalDecisionCommit {
            proposal_record_id: proposal.record.id.clone(),
            expected_proposal_revision: proposal.proposal_revision,
            expected_aggregate_revision: aggregate.aggregate_revision,
            decision: GenerationAttemptProposalDecision::Reject,
            decision_idempotency_key: "generation-rejection-decision".to_owned(),
            decided_at_epoch_seconds,
            decision_state,
            current_policy: None,
            evaluation_seal: None,
            derived_closure: None,
            derived: None,
            updated_at: fixture.commit.occurred_at + Duration::seconds(1),
        })
        .expect("reject isolated generation proposal");
    assert_eq!(
        receipt.proposal.record.status,
        InteractionProposalStatus::Rejected
    );
    assert_eq!(receipt.aggregate.pending_proposal_count, 0);
    assert_eq!(receipt.aggregate.terminal_decision_count, 1);
    assert!(receipt.aggregate.decision_event_ids.is_empty());
    assert_generation_attempt_has_no_live_mutation(&fixture.storage, &fixture.source_key);
}

#[test]
fn generation_attempt_expiry_is_idempotent_and_cas_isolated() {
    let fixture = generation_approval_fixture(false);
    fixture
        .storage
        .commit_generation_attempt_before_review(&fixture.commit)
        .expect("stage generation proposal for expiry");
    let aggregate = fixture
        .storage
        .get_generation_attempt_interaction_aggregate(&fixture.commit.generation_id)
        .expect("load pending generation aggregate");
    let proposal = fixture
        .storage
        .list_generation_attempt_proposals(
            &fixture.commit.generation_id,
            InteractionProposalStatus::Pending,
            1,
        )
        .expect("list pending generation proposal")
        .pop()
        .expect("load pending generation proposal");
    let decided_at_epoch_seconds = proposal
        .record
        .expires_at_epoch_seconds
        .expect("fixture proposal expires");
    let domain_state = remap_generation_attempt_test_state(
        &fixture.storage,
        &fixture.commit.generation_id,
        &aggregate.state,
        true,
    );
    let domain_decision_state = expire_pending_proposal(
        &domain_state,
        &proposal.record.proposal_id,
        domain_state.revision,
        decided_at_epoch_seconds,
    )
    .expect("derive expired proposal state")
    .state;
    let decision_state = remap_generation_attempt_test_state(
        &fixture.storage,
        &fixture.commit.generation_id,
        &domain_decision_state,
        false,
    );
    let commit = GenerationAttemptProposalDecisionCommit {
        proposal_record_id: proposal.record.id.clone(),
        expected_proposal_revision: proposal.proposal_revision,
        expected_aggregate_revision: aggregate.aggregate_revision,
        decision: GenerationAttemptProposalDecision::Expire,
        decision_idempotency_key: "generation-expiry-decision".to_owned(),
        decided_at_epoch_seconds,
        decision_state,
        current_policy: None,
        evaluation_seal: None,
        derived_closure: None,
        derived: None,
        updated_at: fixture.commit.occurred_at + Duration::seconds(60),
    };

    let first = fixture
        .storage
        .decide_generation_attempt_proposal(&commit)
        .expect("expire isolated generation proposal");
    assert!(!first.exact_replay);
    assert_eq!(
        first.proposal.record.status,
        InteractionProposalStatus::Expired
    );
    assert_eq!(first.proposal.proposal_revision, 2);
    assert_eq!(first.aggregate.aggregate_revision, 2);
    assert_eq!(first.aggregate.pending_proposal_count, 0);
    assert_eq!(first.aggregate.terminal_decision_count, 1);
    assert_eq!(first.aggregate.state.revision, 2);
    assert!(first.aggregate.decision_event_ids.is_empty());
    let evidence = first
        .approval_evidence
        .as_ref()
        .expect("expiry must seal generation evidence");
    assert!(evidence.decision_event_ids.is_empty());
    assert_eq!(evidence.resulting_state_revision, 2);
    assert_eq!(
        fixture
            .storage
            .get_generation_attempt(&fixture.commit.generation_id)
            .expect("load expired generation attempt")
            .status,
        GenerationAttemptStatus::BeforeGenerationApplied
    );

    let replay = fixture
        .storage
        .decide_generation_attempt_proposal(&commit)
        .expect("replay exact generation expiry");
    assert!(replay.exact_replay);
    assert_eq!(
        replay.approval_evidence_sha256,
        first.approval_evidence_sha256
    );

    let mut stale = commit.clone();
    stale.decision_idempotency_key = "generation-expiry-stale-cas".to_owned();
    let error = fixture
        .storage
        .decide_generation_attempt_proposal(&stale)
        .expect_err("stale generation expiry CAS must fail");
    assert_eq!(error.code, CoreErrorCode::InvalidInput);
    assert_generation_attempt_has_no_live_mutation(&fixture.storage, &fixture.source_key);
}
