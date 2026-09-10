use std::{
    sync::Arc,
    thread,
    time::{Duration, Instant},
};

use async_trait::async_trait;
use lorepia_domain::{
    CharacterContentV1, ConversationBranchId, ConversationId, CoreError, CoreResult, GenerationId,
    GenerationRequest, GenerationStatus, GenerationUsage, InteractionEvent, Message, MessageId,
    ProviderCapabilities, VariableId, VariableRef, VariableScope, VariableValue,
};
use lorepia_providers::{Provider, ProviderEventSender, StaticProvider};
use lorepia_storage::{InteractionEventCommit, InteractionPolicySnapshot, StoredInteractionState};
use tokio::sync::watch;

use super::{ConversationMode, Core, StallingProvider, hard_crash_database_path, imported_core};

struct FailingGreetingProvider;
#[async_trait]
impl Provider for FailingGreetingProvider {
    fn capabilities(&self) -> ProviderCapabilities {
        ProviderCapabilities {
            streaming: true,
            reasoning: false,
            max_context_tokens: None,
        }
    }
    async fn generate(
        &self,
        _: GenerationRequest,
        _: Option<&str>,
        _: ProviderEventSender,
        _: watch::Receiver<bool>,
    ) -> CoreResult<GenerationUsage> {
        Err(CoreError::internal("synthetic dispatched failure"))
    }
}

fn greeting_fixture() -> (
    tempfile::TempDir,
    Core,
    ConversationId,
    ConversationBranchId,
    MessageId,
) {
    let (root, core, character) = imported_core();
    core.inner
        .storage
        .save_character_content(
            &character.id,
            &CharacterContentV1 {
                first_message: "Durable greeting".into(),
                ..CharacterContentV1::default()
            },
            Some(1),
        )
        .expect("save greeting content");
    let conversation = core
        .create_conversation(&character.id, "Greeting history", ConversationMode::Chat)
        .expect("create conversation");
    let branch = core
        .get_conversation_state(&conversation.id)
        .expect("state")
        .active_branch_id;
    core.ensure_interaction_state_available(&conversation.id, &branch)
        .expect("drain initialization");
    let greeting = core.list_branch_messages(&branch).expect("greeting")[0]
        .id
        .clone();
    (root, core, conversation.id, branch, greeting)
}

fn mark_state(
    core: &Core,
    conversation: &ConversationId,
    branch: &ConversationBranchId,
) -> StoredInteractionState {
    let snapshot = core
        .inner
        .storage
        .get_interaction_state_snapshot(conversation, branch)
        .expect("state");
    let mut next = snapshot.state.clone();
    next.revision += 1;
    next.variables.insert(
        VariableRef {
            scope: VariableScope::Conversation,
            namespace: None,
            id: VariableId::from("greeting-marker"),
        },
        VariableValue::Text("Preserve this actual state".into()),
    );
    core.inner
        .storage
        .commit_interaction_event(&InteractionEventCommit {
            event_id: "greeting-marker-event".into(),
            idempotency_key: "greeting-marker-key".into(),
            key: snapshot.key,
            expected_state_revision: snapshot.state.revision,
            event: InteractionEvent::ConversationOpened,
            generation_attempt_id: None,
            owner_message_id: None,
            policy: InteractionPolicySnapshot {
                module_plan_sha256: None,
                rule_sets: vec![],
            },
            evaluation_seal: None,
            deterministic_seed: None,
            next_state: next,
            knowledge: snapshot.knowledge,
            action_results: vec![],
            effects: vec![],
            derived_events: vec![],
            proposals: vec![],
            created_at: chrono::Utc::now(),
        })
        .expect("persist nonempty state at greeting");
    core.inner
        .storage
        .get_interaction_state_snapshot(conversation, branch)
        .expect("marked state")
}

fn terminal_messages(
    core: &Core,
    branch: &ConversationBranchId,
    generation: &GenerationId,
) -> Vec<Message> {
    let deadline = Instant::now() + Duration::from_secs(3);
    loop {
        if core
            .inner
            .storage
            .get_generation(generation)
            .expect("generation")
            .status
            != GenerationStatus::Running
        {
            core.drain_available_core_lifecycle_occurrences()
                .expect("terminal lifecycle");
            return core.list_branch_messages(branch).expect("messages");
        }
        assert!(Instant::now() < deadline, "generation did not terminalize");
        thread::sleep(Duration::from_millis(10));
    }
}

#[test]
fn greeting_rewind_uses_history_after_complete_failed_and_cancelled_generations() {
    for outcome in ["complete", "failed", "cancelled"] {
        let (_root, core, conversation, branch, greeting) = greeting_fixture();
        let before = mark_state(&core, &conversation, &branch);
        let provider: Arc<dyn Provider> = match outcome {
            "failed" => Arc::new(FailingGreetingProvider),
            "cancelled" => StallingProvider::new("partial").0,
            _ => Arc::new(StaticProvider::new("answer")),
        };
        let generation = core
            .send_message_with_provider(
                &conversation,
                "first user turn",
                "static".into(),
                None,
                provider,
            )
            .expect("send after greeting");
        if outcome == "cancelled" {
            core.cancel_generation(&generation).expect("cancel");
        }
        let messages = terminal_messages(&core, &branch, &generation);
        assert_eq!(
            core.inner
                .storage
                .get_generation(&generation)
                .expect("generation")
                .status,
            match outcome {
                "failed" => GenerationStatus::Failed,
                "cancelled" => GenerationStatus::Cancelled,
                _ => GenerationStatus::Complete,
            }
        );
        if outcome == "complete" {
            let edited = core
                .edit_user_message_with_provider(
                    &conversation,
                    &branch,
                    Some(&messages[2].id),
                    &messages[1].id,
                    "edited first turn",
                    "static".into(),
                    None,
                    Arc::new(StaticProvider::new("edited answer")),
                )
                .expect("cross-branch generation review at historical greeting");
            terminal_messages(&core, &edited.branch.id, &edited.generation_id);
        }
        core.select_conversation_branch(&conversation, &branch)
            .expect("select source before rewind");
        core.remove_message_from_branch(
            &conversation,
            &branch,
            messages.last().map(|message| &message.id),
            &messages[1].id,
        )
        .expect("rewind to greeting");
        let current = core
            .inner
            .storage
            .get_interaction_state_snapshot(&conversation, &branch)
            .expect("later state remains");
        assert_ne!(current.state.revision, before.state.revision);
        core.select_conversation_branch(&conversation, &branch)
            .expect("select source");
        let fork = core
            .create_conversation_branch(&conversation, Some(&greeting), None)
            .expect("fork rewound greeting");
        let cloned = core
            .inner
            .storage
            .get_interaction_state_snapshot(&conversation, &fork.id)
            .expect("cloned boundary");
        assert_eq!(
            cloned.state, before.state,
            "{outcome} must use the real historical state"
        );
        assert_eq!(cloned.knowledge, before.knowledge);
    }
}

#[test]
fn greeting_history_corruption_or_absence_never_falls_back_to_rewound_current_state() {
    let (root, core, conversation, branch, greeting) = greeting_fixture();
    mark_state(&core, &conversation, &branch);
    let generation = core
        .send_message_with_provider(
            &conversation,
            "first turn",
            "static".into(),
            None,
            Arc::new(StaticProvider::new("answer")),
        )
        .expect("send");
    let messages = terminal_messages(&core, &branch, &generation);
    core.remove_message_from_branch(
        &conversation,
        &branch,
        Some(&messages[2].id),
        &messages[1].id,
    )
    .expect("rewind");
    let connection = rusqlite::Connection::open(hard_crash_database_path(root.path()))
        .expect("synthetic database");
    connection.execute_batch("DROP TRIGGER generation_attempt_before_snapshot_no_update;
        UPDATE generation_attempt_before_event_snapshots SET previous_state_snapshot_sha256 = 'aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa';")
        .expect("inject snapshot corruption in isolated test database");
    let branches = core
        .list_conversation_branches(&conversation)
        .expect("branches")
        .len();
    let error = core
        .create_conversation_branch(&conversation, Some(&greeting), None)
        .expect_err("corrupt history");
    assert_eq!(error.code, lorepia_domain::CoreErrorCode::StorageCorrupted);
    connection
        .execute_batch(
            "PRAGMA foreign_keys = OFF;
        DROP TRIGGER generation_attempt_before_snapshot_no_delete;
        DELETE FROM generation_attempt_before_event_snapshots;",
        )
        .expect("remove test snapshot");
    assert!(
        core.create_conversation_branch(&conversation, Some(&greeting), None)
            .is_err()
    );
    assert_eq!(
        core.list_conversation_branches(&conversation)
            .expect("branches")
            .len(),
        branches
    );
}
