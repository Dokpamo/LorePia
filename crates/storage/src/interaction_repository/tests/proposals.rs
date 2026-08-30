use super::*;

#[test]
fn proposal_rejection_updates_state_and_rejects_decision_replay() {
    let (_root, storage, conversation_id, branch_id) = interaction_storage();
    let (rule_set_id, rule_id, _approve_rule_id, rule_set_revision_id) =
        install_approval_rules(&storage);
    let key = InteractionStateKey {
        state_id: "approval-state".to_owned(),
        conversation_id,
        branch_id,
    };
    let requested_state = persist_proposal_request(
        &storage,
        key,
        "proposal-record",
        &rule_set_id,
        &rule_id,
        &rule_set_revision_id,
    );

    let proposal_id = requested_state.proposals[0].id.clone();
    let mut rejected_state = requested_state;
    rejected_state.revision = 2;
    rejected_state.proposals[0].status = InteractionProposalStatus::Rejected;
    rejected_state.proposals[0].decided_at_epoch_seconds = Some(120);
    let rejection = InteractionProposalRejectionCommit {
        proposal_record_id: proposal_id.clone(),
        expected_state_revision: 1,
        expected_proposal_revision: 1,
        decided_at_epoch_seconds: 120,
        decision_state: rejected_state,
        updated_at: Utc::now(),
    };
    let rejected = storage
        .reject_interaction_proposal(&rejection)
        .expect("reject proposal");
    assert_eq!(rejected.record.status, InteractionProposalStatus::Rejected);
    assert_eq!(rejected.proposal_revision, 2);
    assert!(
        storage.reject_interaction_proposal(&rejection).is_err(),
        "proposal decision replay must be rejected"
    );
}

#[test]
fn proposal_approval_atomically_dispatches_derived_user_action() {
    let (_root, storage, conversation_id, branch_id) = interaction_storage();
    let (rule_set_id, request_rule_id, approve_rule_id, rule_set_revision_id) =
        install_approval_rules(&storage);
    let key = InteractionStateKey {
        state_id: "approved-state".to_owned(),
        conversation_id,
        branch_id,
    };
    storage
        .get_or_init_interaction_state(&key, &empty_state(0), &[], Utc::now())
        .expect("initialize state");
    let proposal = InteractionProposalRecord {
        id: interaction_proposal_record_id(&rule_set_id, &request_rule_id, "approve-change", 0)
            .expect("derive approved proposal record id"),
        rule_set_id: rule_set_id.clone(),
        rule_id: request_rule_id.clone(),
        proposal_id: "approve-change".to_owned(),
        title: "Approve change".to_owned(),
        body: "Allow this change?".to_owned(),
        status: InteractionProposalStatus::Pending,
        source_interaction_state_revision: 0,
        requested_at_epoch_seconds: 100,
        expires_at_epoch_seconds: Some(160),
        decided_at_epoch_seconds: None,
    };
    let mut requested_state = empty_state(1);
    requested_state.proposals.push(proposal.clone());
    let proposal_record_id = proposal.id.clone();
    storage
        .commit_interaction_event(&InteractionEventCommit {
            event_id: "approval-request-event".to_owned(),
            idempotency_key: "approval-request-event-key".to_owned(),
            key,
            expected_state_revision: 0,
            event: InteractionEvent::ConversationOpened,
            generation_attempt_id: None,
            owner_message_id: None,
            policy: policy_for_rule_set(&storage, &rule_set_id, &rule_set_revision_id),
            evaluation_seal: None,
            deterministic_seed: None,
            next_state: requested_state.clone(),
            knowledge: Vec::new(),
            action_results: vec![InteractionActionResultWrite {
                set_revision_id: rule_set_revision_id.clone(),
                rule_id: request_rule_id,
                action_ordinal: 0,
                status: InteractionActionResultStatus::Proposed,
                result: VersionedJson {
                    schema_version: 1,
                    value: json!({"status": "proposal_requested"}),
                },
            }],
            effects: vec![InteractionEffect::ApprovalRequested {
                rule_set_id: rule_set_id.clone(),
                rule_id: InteractionRuleId::from("request-rule"),
                proposal_id: "approve-change".to_owned(),
                title: "Approve change".to_owned(),
                body: "Allow this change?".to_owned(),
                expires_after_seconds: Some(60),
            }],
            derived_events: Vec::new(),
            proposals: vec![InteractionProposalWrite {
                review_payload_sha256: interaction_proposal_review_sha256(&proposal)
                    .expect("proposal digest"),
                record: proposal,
                rule_set_revision_id: rule_set_revision_id.clone(),
                action_ordinal: 0,
            }],
            created_at: Utc::now(),
        })
        .expect("commit proposal request");

    let mut decision_state = requested_state.clone();
    decision_state.revision = 2;
    decision_state.proposals[0].status = InteractionProposalStatus::Approved;
    decision_state.proposals[0].decided_at_epoch_seconds = Some(120);
    let mut derived_state = decision_state.clone();
    derived_state.revision = 3;
    let approval = InteractionProposalApprovalCommit {
        proposal_record_id,
        expected_state_revision: 1,
        expected_proposal_revision: 1,
        decided_at_epoch_seconds: 120,
        current_policy: policy_for_rule_set(&storage, &rule_set_id, &rule_set_revision_id),
        decision_state,
        derived: Some(InteractionDerivedEventCommit {
            event_id: "approval-derived-event".to_owned(),
            idempotency_key: "approval-derived-event-key".to_owned(),
            policy: policy_for_rule_set(&storage, &rule_set_id, &rule_set_revision_id),
            evaluation_seal: None,
            deterministic_seed: None,
            next_state: derived_state,
            knowledge: Vec::new(),
            action_results: vec![InteractionActionResultWrite {
                set_revision_id: rule_set_revision_id,
                rule_id: approve_rule_id,
                action_ordinal: 0,
                status: InteractionActionResultStatus::Applied,
                result: VersionedJson {
                    schema_version: 1,
                    value: json!({"status": "visible_event_created"}),
                },
            }],
            effects: vec![InteractionEffect::VisibleSystemEvent {
                text: "Change approved".to_owned(),
            }],
            derived_events: Vec::new(),
            proposals: Vec::new(),
            created_at: Utc::now(),
        }),
        updated_at: Utc::now(),
    };
    let receipt = storage
        .approve_interaction_proposal(&approval)
        .expect("approve and dispatch proposal");
    assert_eq!(receipt.resulting_state_revision, 3);
    assert_eq!(
        receipt.proposal.record.status,
        InteractionProposalStatus::Approved
    );
    assert_eq!(receipt.proposal.proposal_revision, 3);
    assert!(receipt.proposal.dispatched_at_epoch_seconds.is_some());
    assert_eq!(
        receipt.event.as_ref().map(|event| event.event_id.as_str()),
        Some("approval-derived-event")
    );
    assert!(
        storage.approve_interaction_proposal(&approval).is_err(),
        "approval decision replay must be rejected"
    );

    let effects = storage
        .list_pending_interaction_effects(Utc::now() + Duration::seconds(1), 8)
        .expect("list atomic approval effects");
    assert!(
        effects.iter().any(|effect| matches!(
            effect.effect,
            InteractionEffect::VisibleSystemEvent { ref text }
                if text == "Change approved"
        )),
        "derived UserAction effect must be durable in the same approval transaction"
    );
}

#[test]
fn proposal_approval_rejects_policy_update_after_request() {
    let (_root, storage, conversation_id, branch_id) = interaction_storage();
    let (rule_set_id, request_rule_id, _approve_rule_id, rule_set_revision_id) =
        install_approval_rules(&storage);
    let key = InteractionStateKey {
        state_id: "stale-proposal-policy-state".to_owned(),
        conversation_id,
        branch_id,
    };
    let requested_state = persist_proposal_request(
        &storage,
        key.clone(),
        "stale-proposal-policy-record",
        &rule_set_id,
        &request_rule_id,
        &rule_set_revision_id,
    );
    let proposal_record_id = requested_state.proposals[0].id.clone();
    let mut decision_state = requested_state;
    decision_state.revision = 2;
    decision_state.proposals[0].status = InteractionProposalStatus::Approved;
    decision_state.proposals[0].decided_at_epoch_seconds = Some(120);
    assert!(
        storage
            .approve_interaction_proposal(&InteractionProposalApprovalCommit {
                proposal_record_id: proposal_record_id.clone(),
                expected_state_revision: 1,
                expected_proposal_revision: 1,
                decided_at_epoch_seconds: 120,
                current_policy: empty_policy(),
                decision_state,
                derived: None,
                updated_at: Utc::now(),
            })
            .is_err(),
        "approval must reject a policy different from the immutable request policy"
    );
    let snapshot = storage
        .get_interaction_state_snapshot(&key.conversation_id, &key.branch_id)
        .expect("load state after stale-policy approval");
    assert_eq!(snapshot.state.revision, 1);
    assert_eq!(
        snapshot.state.proposals[0].status,
        InteractionProposalStatus::Pending
    );
    let proposal = storage
        .get_interaction_proposal(&proposal_record_id)
        .expect("load proposal after stale-policy approval");
    assert_eq!(proposal.record.status, InteractionProposalStatus::Pending);
    assert_eq!(proposal.proposal_revision, 1);
}

#[test]
fn expired_proposal_decision_leaves_pending_state_unchanged() {
    let (_root, storage, conversation_id, branch_id) = interaction_storage();
    let (rule_set_id, request_rule_id, _approve_rule_id, rule_set_revision_id) =
        install_approval_rules(&storage);
    let key = InteractionStateKey {
        state_id: "expired-state".to_owned(),
        conversation_id: conversation_id.clone(),
        branch_id: branch_id.clone(),
    };
    let requested_state = persist_proposal_request(
        &storage,
        key,
        "expired-proposal-record",
        &rule_set_id,
        &request_rule_id,
        &rule_set_revision_id,
    );
    let proposal_record_id = requested_state.proposals[0].id.clone();
    let mut rejected_state = requested_state;
    rejected_state.revision = 2;
    rejected_state.proposals[0].status = InteractionProposalStatus::Rejected;
    rejected_state.proposals[0].decided_at_epoch_seconds = Some(160);
    let expired = storage.reject_interaction_proposal(&InteractionProposalRejectionCommit {
        proposal_record_id,
        expected_state_revision: 1,
        expected_proposal_revision: 1,
        decided_at_epoch_seconds: 160,
        decision_state: rejected_state,
        updated_at: Utc::now(),
    });
    assert!(expired.is_err(), "expiry must be checked again at commit");
    let current = storage
        .get_interaction_state(&conversation_id, &branch_id)
        .expect("load unchanged state");
    assert_eq!(current.revision, 1);
    assert_eq!(
        current.proposals[0].status,
        InteractionProposalStatus::Pending
    );
}

#[test]
fn concurrent_approve_and_reject_have_exactly_one_winner() {
    let (_root, storage, conversation_id, branch_id) = interaction_storage();
    let (rule_set_id, request_rule_id, _approve_rule_id, rule_set_revision_id) =
        install_approval_rules(&storage);
    let key = InteractionStateKey {
        state_id: "concurrent-decision-state".to_owned(),
        conversation_id,
        branch_id,
    };
    let requested_state = persist_proposal_request(
        &storage,
        key,
        "concurrent-proposal-record",
        &rule_set_id,
        &request_rule_id,
        &rule_set_revision_id,
    );
    let proposal_record_id = requested_state.proposals[0].id.clone();
    let mut approved_state = requested_state.clone();
    approved_state.revision = 2;
    approved_state.proposals[0].status = InteractionProposalStatus::Approved;
    approved_state.proposals[0].decided_at_epoch_seconds = Some(120);
    let mut rejected_state = requested_state;
    rejected_state.revision = 2;
    rejected_state.proposals[0].status = InteractionProposalStatus::Rejected;
    rejected_state.proposals[0].decided_at_epoch_seconds = Some(120);

    let approval_policy = policy_for_rule_set(&storage, &rule_set_id, &rule_set_revision_id);
    let storage = Arc::new(storage);
    let barrier = Arc::new(Barrier::new(3));
    let approving_storage = Arc::clone(&storage);
    let approving_barrier = Arc::clone(&barrier);
    let approving_proposal_id = proposal_record_id.clone();
    let approve = thread::spawn(move || {
        approving_barrier.wait();
        approving_storage.approve_interaction_proposal(&InteractionProposalApprovalCommit {
            proposal_record_id: approving_proposal_id,
            expected_state_revision: 1,
            expected_proposal_revision: 1,
            decided_at_epoch_seconds: 120,
            current_policy: approval_policy,
            decision_state: approved_state,
            derived: None,
            updated_at: Utc::now(),
        })
    });
    let rejecting_storage = Arc::clone(&storage);
    let rejecting_barrier = Arc::clone(&barrier);
    let rejecting_proposal_id = proposal_record_id.clone();
    let reject = thread::spawn(move || {
        rejecting_barrier.wait();
        rejecting_storage.reject_interaction_proposal(&InteractionProposalRejectionCommit {
            proposal_record_id: rejecting_proposal_id,
            expected_state_revision: 1,
            expected_proposal_revision: 1,
            decided_at_epoch_seconds: 120,
            decision_state: rejected_state,
            updated_at: Utc::now(),
        })
    });
    barrier.wait();
    let approve_succeeded = approve.join().expect("approve thread").is_ok();
    let reject_succeeded = reject.join().expect("reject thread").is_ok();
    assert_ne!(
        approve_succeeded, reject_succeeded,
        "exactly one pending-proposal CAS may win"
    );
    let durable = storage
        .get_interaction_proposal(&proposal_record_id)
        .expect("load decided proposal");
    assert_ne!(durable.record.status, InteractionProposalStatus::Pending);
}

#[test]
fn concurrent_approve_and_expire_have_exactly_one_winner() {
    let (_root, storage, conversation_id, branch_id) = interaction_storage();
    let (rule_set_id, request_rule_id, _approve_rule_id, rule_set_revision_id) =
        install_approval_rules(&storage);
    let key = InteractionStateKey {
        state_id: "concurrent-expiry-state".to_owned(),
        conversation_id: conversation_id.clone(),
        branch_id: branch_id.clone(),
    };
    let requested_state = persist_proposal_request(
        &storage,
        key,
        "concurrent-expiry-proposal",
        &rule_set_id,
        &request_rule_id,
        &rule_set_revision_id,
    );
    let proposal_record_id = requested_state.proposals[0].id.clone();
    let mut approved_state = requested_state;
    approved_state.revision = 2;
    approved_state.proposals[0].status = InteractionProposalStatus::Approved;
    approved_state.proposals[0].decided_at_epoch_seconds = Some(160);
    let approval_policy = policy_for_rule_set(&storage, &rule_set_id, &rule_set_revision_id);

    let storage = Arc::new(storage);
    let barrier = Arc::new(Barrier::new(3));
    let approving_storage = Arc::clone(&storage);
    let approving_barrier = Arc::clone(&barrier);
    let approving_proposal_id = proposal_record_id.clone();
    let approve = thread::spawn(move || {
        approving_barrier.wait();
        approving_storage.approve_interaction_proposal(&InteractionProposalApprovalCommit {
            proposal_record_id: approving_proposal_id,
            expected_state_revision: 1,
            expected_proposal_revision: 1,
            decided_at_epoch_seconds: 160,
            current_policy: approval_policy,
            decision_state: approved_state,
            derived: None,
            updated_at: Utc::now(),
        })
    });
    let expiring_storage = Arc::clone(&storage);
    let expiring_barrier = Arc::clone(&barrier);
    let expire = thread::spawn(move || {
        expiring_barrier.wait();
        expiring_storage.expire_due_interaction_proposals(&InteractionProposalExpiryCommit {
            conversation_id,
            branch_id,
            expected_state_revision: 1,
            now_epoch_seconds: 160,
            updated_at: Utc::now(),
        })
    });
    barrier.wait();
    let approve_succeeded = approve.join().expect("approve thread").is_ok();
    let expire_succeeded = expire.join().expect("expire thread").is_ok();
    assert_ne!(
        approve_succeeded, expire_succeeded,
        "approval and expiry must race through one pending/state CAS"
    );
    let durable = storage
        .get_interaction_proposal(&proposal_record_id)
        .expect("load proposal after expiry race");
    assert!(matches!(
        durable.record.status,
        InteractionProposalStatus::Approved | InteractionProposalStatus::Expired
    ));
}

#[test]
fn simultaneous_duplicate_pending_creator_id_has_exactly_one_winner() {
    let (_root, storage, conversation_id, branch_id) = interaction_storage();
    let (rule_set_id, request_rule_id, _approve_rule_id, rule_set_revision_id) =
        install_approval_rules(&storage);
    let key = InteractionStateKey {
        state_id: "duplicate-pending-state".to_owned(),
        conversation_id,
        branch_id,
    };
    storage
        .get_or_init_interaction_state(&key, &empty_state(0), &[], Utc::now())
        .expect("initialize duplicate proposal state");

    let commit_for = |suffix: &str| {
        let record = InteractionProposalRecord {
            id: interaction_proposal_record_id(&rule_set_id, &request_rule_id, "approve-change", 0)
                .expect("derive duplicate proposal record id"),
            rule_set_id: rule_set_id.clone(),
            rule_id: request_rule_id.clone(),
            proposal_id: "approve-change".to_owned(),
            title: "Approve change".to_owned(),
            body: "Allow this change?".to_owned(),
            status: InteractionProposalStatus::Pending,
            source_interaction_state_revision: 0,
            requested_at_epoch_seconds: 100,
            expires_at_epoch_seconds: Some(160),
            decided_at_epoch_seconds: None,
        };
        let mut next_state = empty_state(1);
        next_state.proposals.push(record.clone());
        InteractionEventCommit {
            event_id: format!("duplicate-pending-event-{suffix}"),
            idempotency_key: format!("duplicate-pending-event-key-{suffix}"),
            key: key.clone(),
            expected_state_revision: 0,
            event: InteractionEvent::ConversationOpened,
            generation_attempt_id: None,
            owner_message_id: None,
            policy: policy_for_rule_set(&storage, &rule_set_id, &rule_set_revision_id),
            evaluation_seal: None,
            deterministic_seed: None,
            next_state,
            knowledge: Vec::new(),
            action_results: vec![InteractionActionResultWrite {
                set_revision_id: rule_set_revision_id.clone(),
                rule_id: request_rule_id.clone(),
                action_ordinal: 0,
                status: InteractionActionResultStatus::Proposed,
                result: VersionedJson {
                    schema_version: 1,
                    value: json!({"status": "proposal_requested"}),
                },
            }],
            effects: vec![InteractionEffect::ApprovalRequested {
                rule_set_id: rule_set_id.clone(),
                rule_id: request_rule_id.clone(),
                proposal_id: "approve-change".to_owned(),
                title: "Approve change".to_owned(),
                body: "Allow this change?".to_owned(),
                expires_after_seconds: Some(60),
            }],
            derived_events: Vec::new(),
            proposals: vec![InteractionProposalWrite {
                review_payload_sha256: interaction_proposal_review_sha256(&record)
                    .expect("duplicate proposal digest"),
                record,
                rule_set_revision_id: rule_set_revision_id.clone(),
                action_ordinal: 0,
            }],
            created_at: Utc::now(),
        }
    };
    let first_commit = commit_for("first");
    let second_commit = commit_for("second");
    let storage = Arc::new(storage);
    let barrier = Arc::new(Barrier::new(3));
    let first_storage = Arc::clone(&storage);
    let first_barrier = Arc::clone(&barrier);
    let first = thread::spawn(move || {
        first_barrier.wait();
        first_storage.commit_interaction_event(&first_commit)
    });
    let second_storage = Arc::clone(&storage);
    let second_barrier = Arc::clone(&barrier);
    let second = thread::spawn(move || {
        second_barrier.wait();
        second_storage.commit_interaction_event(&second_commit)
    });
    barrier.wait();
    let first_succeeded = first.join().expect("first proposal thread").is_ok();
    let second_succeeded = second.join().expect("second proposal thread").is_ok();
    assert_ne!(
        first_succeeded, second_succeeded,
        "state CAS and pending creator-id uniqueness permit exactly one winner"
    );
    let pending = storage
        .list_interaction_proposals(
            &key.conversation_id,
            &key.branch_id,
            InteractionProposalStatus::Pending,
            8,
        )
        .expect("list winning pending proposal");
    assert_eq!(pending.len(), 1);
    assert_eq!(pending[0].record.proposal_id, "approve-change");
    assert_eq!(pending[0].state_revision, 1);
    assert_eq!(pending[0].proposal_revision, 1);
}

#[test]
fn decided_creator_proposal_id_can_be_requested_again() {
    let (_root, storage, conversation_id, branch_id) = interaction_storage();
    let (rule_set_id, request_rule_id, _approve_rule_id, rule_set_revision_id) =
        install_approval_rules(&storage);
    let key = InteractionStateKey {
        state_id: "repeat-proposal-state".to_owned(),
        conversation_id,
        branch_id,
    };
    let requested_state = persist_proposal_request(
        &storage,
        key.clone(),
        "first-proposal-record",
        &rule_set_id,
        &request_rule_id,
        &rule_set_revision_id,
    );
    let first_proposal_record_id = requested_state.proposals[0].id.clone();
    let mut rejected_state = requested_state;
    rejected_state.revision = 2;
    rejected_state.proposals[0].status = InteractionProposalStatus::Rejected;
    rejected_state.proposals[0].decided_at_epoch_seconds = Some(120);
    storage
        .reject_interaction_proposal(&InteractionProposalRejectionCommit {
            proposal_record_id: first_proposal_record_id.clone(),
            expected_state_revision: 1,
            expected_proposal_revision: 1,
            decided_at_epoch_seconds: 120,
            decision_state: rejected_state.clone(),
            updated_at: Utc::now(),
        })
        .expect("reject first proposal");

    let second_proposal_record_id =
        interaction_proposal_record_id(&rule_set_id, &request_rule_id, "approve-change", 2)
            .expect("derive repeated proposal record id");
    let repeated = InteractionProposalRecord {
        id: second_proposal_record_id.clone(),
        rule_set_id: rule_set_id.clone(),
        rule_id: request_rule_id.clone(),
        proposal_id: "approve-change".to_owned(),
        title: "Approve change".to_owned(),
        body: "Allow this change?".to_owned(),
        status: InteractionProposalStatus::Pending,
        source_interaction_state_revision: 2,
        requested_at_epoch_seconds: 200,
        expires_at_epoch_seconds: Some(260),
        decided_at_epoch_seconds: None,
    };
    let mut repeated_state = rejected_state;
    repeated_state.revision = 3;
    repeated_state.proposals.push(repeated.clone());
    storage
        .commit_interaction_event(&InteractionEventCommit {
            event_id: "repeat-proposal-event".to_owned(),
            idempotency_key: "repeat-proposal-key".to_owned(),
            key: key.clone(),
            expected_state_revision: 2,
            event: InteractionEvent::ConversationOpened,
            generation_attempt_id: None,
            owner_message_id: None,
            policy: policy_for_rule_set(&storage, &rule_set_id, &rule_set_revision_id),
            evaluation_seal: None,
            deterministic_seed: None,
            next_state: repeated_state,
            knowledge: Vec::new(),
            action_results: vec![InteractionActionResultWrite {
                set_revision_id: rule_set_revision_id.clone(),
                rule_id: request_rule_id.clone(),
                action_ordinal: 0,
                status: InteractionActionResultStatus::Proposed,
                result: VersionedJson {
                    schema_version: 1,
                    value: json!({"status": "proposal_requested_again"}),
                },
            }],
            effects: vec![InteractionEffect::ApprovalRequested {
                rule_set_id,
                rule_id: request_rule_id,
                proposal_id: "approve-change".to_owned(),
                title: "Approve change".to_owned(),
                body: "Allow this change?".to_owned(),
                expires_after_seconds: Some(60),
            }],
            derived_events: Vec::new(),
            proposals: vec![InteractionProposalWrite {
                review_payload_sha256: interaction_proposal_review_sha256(&repeated)
                    .expect("repeat proposal digest"),
                record: repeated,
                rule_set_revision_id,
                action_ordinal: 0,
            }],
            created_at: Utc::now(),
        })
        .expect("request same creator proposal id after decision");
    let snapshot = storage
        .get_interaction_state_snapshot(&key.conversation_id, &key.branch_id)
        .expect("load repeated proposal state");
    assert_eq!(snapshot.state.proposals.len(), 2);
    assert_eq!(
        snapshot.state.proposals[0].status,
        InteractionProposalStatus::Rejected
    );
    assert_eq!(
        snapshot.state.proposals[1].status,
        InteractionProposalStatus::Pending
    );
    let pending = storage
        .list_interaction_proposals(
            &key.conversation_id,
            &key.branch_id,
            InteractionProposalStatus::Pending,
            8,
        )
        .expect("list pending proposals");
    assert_eq!(pending.len(), 1);
    assert_eq!(pending[0].record.id, second_proposal_record_id);
    assert_eq!(pending[0].state_revision, 3);
    assert_eq!(pending[0].proposal_revision, 1);
    let rejected = storage
        .list_interaction_proposals(
            &key.conversation_id,
            &key.branch_id,
            InteractionProposalStatus::Rejected,
            8,
        )
        .expect("list rejected proposals");
    assert_eq!(rejected.len(), 1);
    assert_eq!(rejected[0].record.id, first_proposal_record_id);
    assert_eq!(rejected[0].state_revision, 3);
    assert_eq!(rejected[0].proposal_revision, 2);
    assert!(
        storage
            .list_interaction_proposals(
                &key.conversation_id,
                &key.branch_id,
                InteractionProposalStatus::Pending,
                0,
            )
            .is_err(),
        "proposal listing must reject an unbounded zero limit"
    );
}
