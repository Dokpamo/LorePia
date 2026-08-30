use super::generation_support::*;
use super::*;

#[test]
fn generation_attempt_staging_survives_restart_and_replays_without_live_mutation() {
    let GenerationApprovalFixture {
        _root: root,
        storage,
        source_key: key,
        commit,
        ..
    } = generation_approval_fixture(false);
    let first = storage
        .commit_generation_attempt_before_review(&commit)
        .expect("stage generation BeforeGeneration review");
    assert!(!first.exact_replay);
    assert_eq!(first.pending_proposal_count, 1);
    assert_eq!(first.resulting_state_revision, 1);
    assert_eq!(
        storage
            .get_generation_attempt(&commit.generation_id)
            .expect("load staged generation attempt")
            .status,
        GenerationAttemptStatus::AwaitingApproval
    );
    assert_generation_attempt_has_no_live_mutation(&storage, &key);

    drop(storage);
    let reopened = Storage::open(root.path()).expect("reopen staged generation storage");
    let replay = reopened
        .commit_generation_attempt_before_review(&commit)
        .expect("replay exact generation BeforeGeneration review after restart");
    assert!(replay.exact_replay);
    assert_eq!(replay.event_sha256, first.event_sha256);
    assert_eq!(replay.evidence_sha256, first.evidence_sha256);
    let restored = reopened
        .list_generation_attempt_proposals(
            &commit.generation_id,
            InteractionProposalStatus::Pending,
            10,
        )
        .expect("restore pending generation proposal");
    assert_eq!(restored.len(), 1);
    assert_eq!(
        reopened
            .get_generation_attempt_proposal(&restored[0].record.id)
            .expect("load exact pending generation proposal"),
        restored[0]
    );

    let mut conflicting = commit.clone();
    conflicting.review_sha256 = sha256_hex(b"conflicting-generation-review");
    let error = reopened
        .commit_generation_attempt_before_review(&conflicting)
        .expect_err("conflicting staged review must not replay");
    assert_eq!(error.code, CoreErrorCode::InvalidInput);
    assert_generation_attempt_has_no_live_mutation(&reopened, &key);
}

#[test]
#[allow(clippy::too_many_lines)]
fn migrated_v1_generation_review_replays_without_resealing() {
    let fixture = generation_approval_fixture(false);
    let domain_review_sha256_by_record_id = fixture
        .commit
        .proposals
        .iter()
        .map(|proposal| {
            (
                proposal.record.id.as_str().to_owned(),
                proposal.review_payload_sha256.clone(),
            )
        })
        .collect::<BTreeMap<_, _>>();
    let domain_proposal_record_id = fixture.commit.proposals[0].record.id.clone();
    let domain_proposal_review_sha256 = fixture.commit.proposals[0].review_payload_sha256.clone();
    {
        let mut connection = fixture
            .storage
            .connection()
            .expect("open legacy generation review fixture");
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .expect("begin legacy generation review fixture");
        let snapshot_guard = transaction
            .query_row(
                "SELECT sql FROM sqlite_schema
                 WHERE type = 'trigger'
                   AND name = 'generation_attempt_before_snapshot_no_update'",
                [],
                |row| row.get::<_, String>(0),
            )
            .expect("load generation review immutability trigger");
        let proposal_guard = transaction
            .query_row(
                "SELECT sql FROM sqlite_schema
                 WHERE type = 'trigger'
                   AND name = 'generation_attempt_proposals_transition_guard'",
                [],
                |row| row.get::<_, String>(0),
            )
            .expect("load generation proposal transition trigger");
        let prepared = prepare_generation_attempt_before_review(
            &transaction,
            &fixture.commit,
            &fixture.commit.review_sha256,
            &domain_review_sha256_by_record_id,
        )
        .expect("prepare schema-twenty-four generation review");
        write_generation_attempt_before_review(&transaction, &fixture.commit, &prepared)
            .expect("write schema-twenty-four generation review shape");
        transaction
            .execute_batch(
                "DROP TRIGGER generation_attempt_before_snapshot_no_update;
                 DROP TRIGGER generation_attempt_proposals_transition_guard;",
            )
            .expect("open identity-version backfill fixture");
        transaction
            .execute(
                "UPDATE generation_attempt_before_event_snapshots
                 SET storage_identity_version = 1
                 WHERE generation_id = ?1",
                [fixture.commit.generation_id.0.as_str()],
            )
            .expect("mark migrated generation review identity v1");
        transaction
            .execute(
                "UPDATE generation_attempt_proposals
                 SET storage_identity_version = 1
                 WHERE generation_id = ?1",
                [fixture.commit.generation_id.0.as_str()],
            )
            .expect("mark migrated generation proposal identity v1");
        transaction
            .execute_batch(&format!("{snapshot_guard}; {proposal_guard};"))
            .expect("restore generation identity immutability triggers");
        transaction
            .commit()
            .expect("commit migrated generation review fixture");
    }

    let replay = fixture
        .storage
        .commit_generation_attempt_before_review(&fixture.commit)
        .expect("replay exact migrated generation review");
    assert!(replay.exact_replay);
    assert_eq!(replay.storage_identity_version, 1);
    assert_eq!(replay.review_sha256.as_str(), fixture.commit.review_sha256);
    assert_eq!(
        replay.domain_review_sha256.as_str(),
        fixture.commit.review_sha256
    );
    let proposal = fixture
        .storage
        .list_generation_attempt_proposals(
            &fixture.commit.generation_id,
            InteractionProposalStatus::Pending,
            1,
        )
        .expect("read migrated generation proposal")
        .pop()
        .expect("migrated generation proposal");
    assert_eq!(proposal.storage_identity_version, 1);
    assert_eq!(proposal.record.id, domain_proposal_record_id);
    assert_eq!(proposal.domain_proposal_record_id, proposal.record.id);
    assert_eq!(
        proposal.proposal_review_sha256.as_str(),
        domain_proposal_review_sha256
    );
    assert_eq!(
        proposal.domain_proposal_review_sha256,
        proposal.proposal_review_sha256
    );
    assert_generation_attempt_has_no_live_mutation(&fixture.storage, &fixture.source_key);
}

#[test]
fn generation_materialization_rollback_is_cleanly_retryable() {
    let fixture = generation_approval_fixture(false);
    let (sealed, prompt_plan, decision) = seal_approved_generation_fixture(&fixture);
    let materialized_at = fixture.commit.occurred_at + Duration::seconds(3);
    {
        let mut connection = fixture
            .storage
            .connection()
            .expect("open rollback materialization transaction");
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .expect("begin rollback materialization transaction");
        materialize_generation_attempt_interaction_for_append(
            &fixture.storage,
            &transaction,
            &sealed,
            &fixture.target_key,
            &prompt_plan,
            materialized_at,
        )
        .expect("materialize before rollback");
        crate::generation_attempt::mark_attempt_running_in_transaction(
            &transaction,
            &sealed,
            materialized_at,
        )
        .expect("mark running before rollback");
        transaction.rollback().expect("roll back materialization");
    }
    assert_generation_attempt_has_no_live_mutation(&fixture.storage, &fixture.source_key);
    assert_eq!(
        fixture
            .storage
            .get_generation_attempt(&sealed.generation_id)
            .expect("load rolled-back attempt")
            .status,
        GenerationAttemptStatus::DispatchReady
    );

    {
        let mut connection = fixture
            .storage
            .connection()
            .expect("open retry materialization transaction");
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .expect("begin retry materialization transaction");
        materialize_generation_attempt_interaction_for_append(
            &fixture.storage,
            &transaction,
            &sealed,
            &fixture.target_key,
            &prompt_plan,
            materialized_at,
        )
        .expect("retry exact materialization");
        crate::generation_attempt::mark_attempt_running_in_transaction(
            &transaction,
            &sealed,
            materialized_at,
        )
        .expect("mark retried attempt running");
        transaction
            .commit()
            .expect("commit retried materialization");
    }
    assert_eq!(
        fixture
            .storage
            .get_interaction_state_snapshot(
                &fixture.target_key.conversation_id,
                &fixture.target_key.branch_id,
            )
            .expect("load retried materialized state")
            .state,
        decision.aggregate.state
    );
}

#[test]
fn due_proposal_expiry_is_atomic_restart_safe_and_dispatches_no_action() {
    let (_root, storage, conversation_id, branch_id) = interaction_storage();
    let (rule_set_id, request_rule_id, _approve_rule_id, rule_set_revision_id) =
        install_approval_rules(&storage);
    let key = InteractionStateKey {
        state_id: "due-expiry-state".to_owned(),
        conversation_id: conversation_id.clone(),
        branch_id: branch_id.clone(),
    };
    persist_proposal_request(
        &storage,
        key,
        "due-expiry-proposal",
        &rule_set_id,
        &request_rule_id,
        &rule_set_revision_id,
    );
    let expired = storage
        .expire_due_interaction_proposals(&InteractionProposalExpiryCommit {
            conversation_id: conversation_id.clone(),
            branch_id: branch_id.clone(),
            expected_state_revision: 1,
            now_epoch_seconds: 160,
            updated_at: Utc::now(),
        })
        .expect("expire due proposal");
    assert_eq!(expired.state.revision, 2);
    assert_eq!(expired.expired_proposals.len(), 1);
    assert_eq!(
        expired.expired_proposals[0].record.status,
        InteractionProposalStatus::Expired
    );
    assert_eq!(expired.expired_proposals[0].proposal_revision, 2);
    assert_eq!(
        expired.expired_proposals[0].record.decided_at_epoch_seconds,
        Some(160)
    );
    let replay = storage
        .expire_due_interaction_proposals(&InteractionProposalExpiryCommit {
            conversation_id: conversation_id.clone(),
            branch_id: branch_id.clone(),
            expected_state_revision: 2,
            now_epoch_seconds: 160,
            updated_at: Utc::now(),
        })
        .expect("repeat expiry after restart");
    assert!(replay.expired_proposals.is_empty());
    assert_eq!(replay.state.revision, 2);
    let user_action_events = storage
        .connection()
        .expect("open expiry test connection")
        .query_row(
            "SELECT COUNT(*) FROM interaction_events WHERE event_kind = 'user_action'",
            [],
            |row| row.get::<_, i64>(0),
        )
        .expect("count expiry user actions");
    assert_eq!(
        user_action_events, 0,
        "proposal expiry must never dispatch a UserAction"
    );
    let listed = storage
        .list_interaction_proposals(
            &conversation_id,
            &branch_id,
            InteractionProposalStatus::Expired,
            8,
        )
        .expect("list expired proposals");
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].state_revision, 2);
}
