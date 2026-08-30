use super::*;

#[test]
fn event_commit_is_atomic_cas_with_exact_idempotent_replay() {
    let (_root, storage, conversation_id, branch_id) = interaction_storage();
    let key = InteractionStateKey {
        state_id: "interaction-state".to_owned(),
        conversation_id,
        branch_id,
    };
    storage
        .get_or_init_interaction_state(&key, &empty_state(0), &[], Utc::now())
        .expect("initialize state");
    let commit = InteractionEventCommit {
        event_id: "event-1".to_owned(),
        idempotency_key: "event-key-1".to_owned(),
        key: key.clone(),
        expected_state_revision: 0,
        event: InteractionEvent::ConversationOpened,
        generation_attempt_id: None,
        owner_message_id: None,
        policy: empty_policy(),
        evaluation_seal: None,
        deterministic_seed: None,
        next_state: empty_state(1),
        knowledge: Vec::new(),
        action_results: Vec::new(),
        effects: Vec::new(),
        derived_events: Vec::new(),
        proposals: Vec::new(),
        created_at: Utc::now(),
    };
    let first = storage
        .commit_interaction_event(&commit)
        .expect("commit event");
    assert!(!first.exact_replay);
    assert_eq!(first.resulting_state_revision, 1);

    let replay = storage
        .commit_interaction_event(&commit)
        .expect("exact replay");
    assert!(replay.exact_replay);
    assert_eq!(replay.event_id, first.event_id);
    let occurrence = storage
        .get_interaction_event_by_occurrence(&InteractionEventOccurrenceLookup {
            event_id: commit.event_id.clone(),
            idempotency_key: commit.idempotency_key.clone(),
            conversation_id: key.conversation_id.clone(),
            branch_id: key.branch_id.clone(),
            event: commit.event.clone(),
            generation_attempt_id: None,
            owner_message_id: None,
            occurred_at: commit.created_at,
        })
        .expect("look up committed occurrence")
        .expect("occurrence must exist");
    assert!(occurrence.exact_replay);
    assert_eq!(occurrence.resulting_state_revision, 1);
    let durable = storage
        .get_interaction_event(&commit.event_id)
        .expect("read immutable event evidence")
        .expect("event evidence must exist");
    assert!(durable.exact_replay);
    assert_eq!(durable.commit_sha256, first.commit_sha256);
    assert_eq!(
        durable.resulting_state_snapshot_sha256,
        first.resulting_state_snapshot_sha256
    );
    assert_eq!(
        durable.proposal_review_sha256s,
        first.proposal_review_sha256s
    );
    assert!(
        storage
            .get_interaction_event("event-that-does-not-exist")
            .expect("read missing event")
            .is_none()
    );
    assert!(
        storage
            .get_interaction_event_by_occurrence(&InteractionEventOccurrenceLookup {
                event_id: commit.event_id.clone(),
                idempotency_key: commit.idempotency_key.clone(),
                conversation_id: key.conversation_id.clone(),
                branch_id: key.branch_id.clone(),
                event: commit.event.clone(),
                generation_attempt_id: None,
                owner_message_id: None,
                occurred_at: commit.created_at + Duration::milliseconds(1),
            })
            .is_err(),
        "an occurrence timestamp mismatch must not alias the stored transition"
    );

    let mut conflict = commit.clone();
    conflict.event_id = "event-conflict".to_owned();
    assert!(
        storage.commit_interaction_event(&conflict).is_err(),
        "same idempotency key with different bytes must fail"
    );
    let mut stale = commit;
    stale.event_id = "event-stale".to_owned();
    stale.idempotency_key = "event-key-stale".to_owned();
    assert!(
        storage.commit_interaction_event(&stale).is_err(),
        "stale state revision must fail"
    );
}

#[test]
fn message_committed_event_writes_one_exact_immutable_state_checkpoint() {
    let (_root, storage, conversation_id, branch_id) = interaction_storage();
    let key = InteractionStateKey {
        state_id: "message-checkpoint-state".to_owned(),
        conversation_id: conversation_id.clone(),
        branch_id: branch_id.clone(),
    };
    let created_at = Utc::now();
    storage
        .get_or_init_interaction_state(&key, &empty_state(0), &[], created_at)
        .expect("initialize checkpoint state");
    let message = Message::user(conversation_id.clone(), "checkpoint owner");
    storage
        .save_message(&message)
        .expect("save checkpoint owner");
    storage
        .connection()
        .expect("open checkpoint connection")
        .execute(
            "UPDATE conversation_branches
             SET head_message_id = ?3, updated_at = ?4
             WHERE conversation_id = ?1 AND id = ?2",
            params![
                conversation_id.0.as_str(),
                branch_id.0.as_str(),
                message.id.0.as_str(),
                created_at.to_rfc3339(),
            ],
        )
        .expect("advance checkpoint branch head");
    let commit = InteractionEventCommit {
        event_id: "message-checkpoint-event".to_owned(),
        idempotency_key: "message-checkpoint-key".to_owned(),
        key: key.clone(),
        expected_state_revision: 0,
        event: InteractionEvent::MessageCommitted,
        generation_attempt_id: None,
        owner_message_id: Some(message.id.clone()),
        policy: empty_policy(),
        evaluation_seal: None,
        deterministic_seed: None,
        next_state: empty_state(1),
        knowledge: Vec::new(),
        action_results: Vec::new(),
        effects: Vec::new(),
        derived_events: Vec::new(),
        proposals: Vec::new(),
        created_at,
    };
    let stored = storage
        .commit_interaction_event(&commit)
        .expect("commit message checkpoint event");
    let checkpoint = storage
        .get_interaction_state_checkpoint(&conversation_id, &branch_id, &message.id)
        .expect("load checkpoint");
    assert_eq!(checkpoint.state, commit.next_state);
    assert_eq!(
        checkpoint.checkpoint_sha256,
        stored.resulting_state_snapshot_sha256
    );
    assert_eq!(
        storage
            .get_interaction_event(&stored.event_id)
            .expect("read checkpoint event")
            .expect("checkpoint event exists")
            .owner_message_id,
        Some(message.id.clone())
    );
    let target_branch = storage
        .create_conversation_branch(&conversation_id, Some(&message.id), None)
        .expect("create checkpoint fork");
    let cloned = storage
        .get_interaction_state_snapshot(&conversation_id, &target_branch.id)
        .expect("read atomically cloned state");
    assert_eq!(cloned.state, checkpoint.state);
    assert_eq!(cloned.knowledge, checkpoint.knowledge);
    assert_eq!(
        cloned.key,
        interaction_state_key_for_branch(&conversation_id, &target_branch.id)
            .expect("derive target state identity")
    );

    let non_head = Message::user_after(
        conversation_id.clone(),
        Some(message.id.clone()),
        "not the branch head",
    );
    storage
        .save_message(&non_head)
        .expect("save non-head owner");
    let branches_before_invalid_fork = storage
        .list_conversation_branches(&conversation_id)
        .expect("list branches before invalid fork")
        .len();
    let error = storage
        .create_conversation_branch(&conversation_id, Some(&non_head.id), None)
        .expect_err("a boundary with no exact interaction evidence must not create a branch");
    assert_eq!(error.code, CoreErrorCode::NotFound);
    assert_eq!(
        storage
            .list_conversation_branches(&conversation_id)
            .expect("list branches after invalid fork")
            .len(),
        branches_before_invalid_fork,
        "branch insertion and interaction-state clone must roll back together"
    );
    let invalid = InteractionEventCommit {
        event_id: "invalid-message-checkpoint-event".to_owned(),
        idempotency_key: "invalid-message-checkpoint-key".to_owned(),
        expected_state_revision: 1,
        next_state: empty_state(2),
        owner_message_id: Some(non_head.id),
        created_at: created_at + Duration::seconds(1),
        ..commit
    };
    assert!(
        storage.commit_interaction_event(&invalid).is_err(),
        "a non-head message cannot own a checkpoint"
    );
    assert_eq!(
        storage
            .get_interaction_state_snapshot(&conversation_id, &branch_id)
            .expect("read rolled-back state")
            .state
            .revision,
        1,
        "invalid checkpoint ownership must roll back the state transition"
    );
}
