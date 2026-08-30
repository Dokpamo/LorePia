use super::generation_support::*;
use super::*;

#[test]
fn direct_generation_occurrence_commit_is_rejected_without_append_authority() {
    let (_root, storage, conversation_id, branch_id) = interaction_storage();
    let key = InteractionStateKey {
        state_id: "generation-occurrence-state".to_owned(),
        conversation_id: conversation_id.clone(),
        branch_id: branch_id.clone(),
    };
    let created_at = Utc::now();
    storage
        .get_or_init_interaction_state(&key, &empty_state(0), &[], created_at)
        .expect("initialize generation interaction state");
    let settings = storage.load_settings().expect("load module local user");
    let prompt_authority = synthetic_prompt_selection_authority(&storage, &conversation_id);
    let module_review = lorepia_orchestration::review_module_merge(
        0,
        &lorepia_orchestration::ModuleResolutionContext {
            local_user_id: settings.local_user_id,
            persona_id: None,
            character_id: Some(prompt_authority.character.id.clone()),
            conversation_id: Some(conversation_id.0.clone()),
            branch_id: Some(branch_id.0.clone()),
            supported_capabilities: Vec::new(),
        },
        &[],
        &[],
    )
    .expect("review direct-commit module authority");
    let generation_attempt_id = storage
        .prepare_generation_attempt(
            &GenerationAttemptInput {
                operation_id: "generation-operation-a".to_owned(),
                conversation_id: conversation_id.clone(),
                source_branch_id: branch_id.clone(),
                proposed_branch_id: branch_id.clone(),
                expected_head_message_id: None,
                context_head_message_id: None,
                module_plan_sha256: no_applied_module_runtime_plan_sha256(),
                base_request_fingerprint_sha256: Sha256Digest::parse(sha256_hex(
                    b"generation-prompt-input",
                ))
                .expect("direct-commit input hash"),
                prompt_selection_authority: Some(prompt_authority),
                module_runtime_review_authority: Some(module_review),
                applied_runtime_plan_authority: None,
            },
            created_at,
        )
        .expect("prepare exact generation attempt")
        .generation_id;
    let commit = InteractionEventCommit {
        event_id: "before-generation-event".to_owned(),
        idempotency_key: "before-generation-event-key".to_owned(),
        key: key.clone(),
        expected_state_revision: 0,
        event: InteractionEvent::BeforeGeneration,
        generation_attempt_id: Some(generation_attempt_id.clone()),
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
        created_at,
    };
    let error = storage
        .commit_interaction_event(&commit)
        .expect_err("ordinary commits must not consume staged generation authority");
    assert_eq!(error.code, CoreErrorCode::InvalidInput);
    assert_eq!(
        storage
            .get_interaction_state_snapshot(&conversation_id, &branch_id)
            .expect("read unchanged generation interaction state")
            .state
            .revision,
        0,
        "rejected direct materialization must be atomic"
    );
    assert!(
        storage
            .get_interaction_event(&commit.event_id)
            .expect("read rejected generation event")
            .is_none()
    );
}

#[test]
fn same_branch_generation_materialization_replays_exact_approved_chain() {
    let fixture = generation_approval_fixture(false);
    let (sealed, prompt_plan, decision) = seal_approved_generation_fixture(&fixture);
    assert_generation_attempt_has_no_live_mutation(&fixture.storage, &fixture.source_key);
    let materialized_at = fixture.commit.occurred_at + Duration::seconds(3);
    {
        let mut connection = fixture
            .storage
            .connection()
            .expect("open generation materialization transaction");
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .expect("begin generation materialization transaction");
        let receipt = materialize_generation_attempt_interaction_for_append(
            &fixture.storage,
            &transaction,
            &sealed,
            &fixture.target_key,
            &prompt_plan,
            materialized_at,
        )
        .expect("materialize exact generation interaction chain");
        assert_eq!(
            receipt.final_state_revision,
            decision.aggregate.state.revision
        );
        assert_eq!(
            receipt.final_state_snapshot_sha256,
            decision.aggregate.state_snapshot_sha256
        );
        crate::generation_attempt::mark_attempt_running_in_transaction(
            &transaction,
            &sealed,
            materialized_at,
        )
        .expect("mark materialized generation running");
        transaction
            .commit()
            .expect("commit generation materialization");
    }

    let live = fixture
        .storage
        .get_interaction_state_snapshot(
            &fixture.target_key.conversation_id,
            &fixture.target_key.branch_id,
        )
        .expect("load materialized interaction state");
    assert_eq!(live.state, decision.aggregate.state);
    assert_eq!(live.knowledge, decision.aggregate.knowledge);
    let proposal = fixture
        .storage
        .get_interaction_proposal(&decision.proposal.record.id)
        .expect("load materialized terminal proposal");
    assert_eq!(proposal.record.status, InteractionProposalStatus::Approved);
    assert!(proposal.dispatched_at_epoch_seconds.is_some());
    let pending = fixture
        .storage
        .list_pending_interaction_effects(materialized_at + Duration::seconds(1), 8)
        .expect("list materialized effects");
    assert_eq!(pending.len(), 1);
    assert!(matches!(
        pending[0].effect,
        InteractionEffect::VisibleSystemEvent { .. }
    ));
    let connection = fixture
        .storage
        .connection()
        .expect("open materialization assertions");
    assert_eq!(
        connection
            .query_row("SELECT COUNT(*) FROM interaction_events", [], |row| {
                row.get::<_, u64>(0)
            })
            .expect("count materialized interaction events"),
        2,
        "BeforeGeneration and its approved UserAction must each materialize once"
    );
    assert_eq!(
        connection
            .query_row(
                "SELECT COUNT(*)
                 FROM interaction_effect_outbox
                 WHERE effect_kind = 'approval_requested'
                   AND delivered_at IS NOT NULL",
                [],
                |row| row.get::<_, u64>(0),
            )
            .expect("count suppressed approval effects"),
        1,
        "an already-decided approval prompt must remain audit-visible but not redeliver"
    );
    drop(connection);
    assert_eq!(
        fixture
            .storage
            .get_generation_attempt(&sealed.generation_id)
            .expect("load running materialized attempt")
            .status,
        GenerationAttemptStatus::Running
    );
}

#[test]
fn fork_generation_materialization_clones_source_boundary_atomically() {
    let fixture = generation_approval_fixture(true);
    let (sealed, prompt_plan, decision) = seal_approved_generation_fixture(&fixture);
    let materialized_at = fixture.commit.occurred_at + Duration::seconds(3);
    {
        let mut connection = fixture
            .storage
            .connection()
            .expect("open fork materialization transaction");
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .expect("begin fork materialization transaction");
        transaction
            .execute(
                "INSERT INTO conversation_branches
                 (id, conversation_id, title, fork_message_id,
                  head_message_id, created_at, updated_at)
                 VALUES (?1, ?2, ?3, NULL, NULL, ?4, ?4)",
                params![
                    fixture.target_key.branch_id.0.as_str(),
                    fixture.target_key.conversation_id.0.as_str(),
                    "Reviewed generation fork",
                    materialized_at.to_rfc3339(),
                ],
            )
            .expect("insert reviewed target branch");
        materialize_generation_attempt_interaction_for_append(
            &fixture.storage,
            &transaction,
            &sealed,
            &fixture.target_key,
            &prompt_plan,
            materialized_at,
        )
        .expect("materialize reviewed fork interaction chain");
        crate::generation_attempt::mark_attempt_running_in_transaction(
            &transaction,
            &sealed,
            materialized_at,
        )
        .expect("mark fork attempt running");
        transaction.commit().expect("commit fork materialization");
    }

    let source = fixture
        .storage
        .get_interaction_state_snapshot(
            &fixture.source_key.conversation_id,
            &fixture.source_key.branch_id,
        )
        .expect("load unchanged source interaction state");
    assert_eq!(source.state, empty_state(0));
    let target = fixture
        .storage
        .get_interaction_state_snapshot(
            &fixture.target_key.conversation_id,
            &fixture.target_key.branch_id,
        )
        .expect("load cloned target interaction state");
    assert_eq!(target.key, fixture.target_key);
    assert_eq!(target.state, decision.aggregate.state);
    assert_eq!(target.knowledge, decision.aggregate.knowledge);
    let connection = fixture
        .storage
        .connection()
        .expect("open fork materialization assertions");
    let event_branches = connection
        .prepare(
            "SELECT DISTINCT branch_id
             FROM interaction_events
             ORDER BY branch_id",
        )
        .expect("prepare fork event branch query")
        .query_map([], |row| row.get::<_, String>(0))
        .expect("query fork event branches")
        .collect::<Result<Vec<_>, _>>()
        .expect("collect fork event branches");
    assert_eq!(event_branches, vec![fixture.target_key.branch_id.0.clone()]);
}
