#[test]
fn detached_generations_keep_process_admission_until_terminal() {
    let (_root, core, character) = imported_core();
    let mut generation_ids = Vec::with_capacity(MAX_ACTIVE_GENERATIONS_PER_PROCESS);

    for index in 0..MAX_ACTIVE_GENERATIONS_PER_PROCESS {
        let conversation = core.open_conversation(&character.id).expect("conversation");
        let (provider, provider_started) =
            StallingProvider::new(format!("detached partial {index}"));
        let generation_id = core
            .send_message_with_provider(
                &conversation.id,
                "start detached generation",
                format!("detached-model-{index}"),
                None,
                provider,
            )
            .expect("start generation within process admission");
        provider_started
            .recv_timeout(Duration::from_secs(2))
            .expect("detached provider started");
        generation_ids.push(generation_id);
    }
    assert_eq!(
        core.active_generation_count(),
        MAX_ACTIVE_GENERATIONS_PER_PROCESS
    );

    let overflow_conversation = core.open_conversation(&character.id).expect("conversation");
    let (overflow_provider, overflow_started) = StallingProvider::new("must not dispatch");
    let overflow = core
        .send_message_with_provider(
            &overflow_conversation.id,
            "overflow detached generations",
            "overflow-model".to_owned(),
            None,
            overflow_provider,
        )
        .expect_err("a recycled renderer stream must not bypass Core admission");
    assert_eq!(overflow.code, CoreErrorCode::ProviderRateLimited);
    assert!(
        overflow_started
            .recv_timeout(Duration::from_millis(100))
            .is_err(),
        "an over-capacity generation must not reach provider dispatch"
    );

    core.cancel_generation(&generation_ids[0])
        .expect("cancel one admitted generation");
    wait_for_generation_status(&core, &generation_ids[0], GenerationStatus::Cancelled);
    wait_for_active_generation_count(&core, MAX_ACTIVE_GENERATIONS_PER_PROCESS - 1);
    let (replacement_provider, replacement_started) = StallingProvider::new("replacement partial");
    core.send_message_with_provider(
        &overflow_conversation.id,
        "overflow detached generations",
        "overflow-model".to_owned(),
        None,
        replacement_provider,
    )
    .expect("terminal generation releases Core admission for the exact retry");
    replacement_started
        .recv_timeout(Duration::from_secs(2))
        .expect("replacement provider started");
    assert_eq!(
        core.active_generation_count(),
        MAX_ACTIVE_GENERATIONS_PER_PROCESS
    );
}

#[test]
fn dropping_last_core_from_a_runtime_worker_bounds_shutdown_and_releases_provider() {
    let (_root, core, character) = imported_core();
    let conversation = core.open_conversation(&character.id).expect("conversation");
    let (provider, provider_started) = StallingProvider::new("partial before shutdown");
    let provider_weak = Arc::downgrade(&provider);
    core.send_message_with_provider(
        &conversation.id,
        "start",
        "stalling".to_owned(),
        Some("ephemeral-credential".to_owned()),
        provider,
    )
    .expect("start generation");
    provider_started
        .recv_timeout(Duration::from_secs(2))
        .expect("provider started");

    let runtime_handle = core.inner.runtime.handle().clone();
    let (dropped_sender, dropped_receiver) = std_mpsc::channel();
    std::mem::drop(runtime_handle.spawn(async move {
        drop(core);
        let _ = dropped_sender.send(());
    }));

    dropped_receiver
        .recv_timeout(Duration::from_secs(4))
        .expect("core drop must not panic or deadlock on its runtime worker");
    assert!(
        provider_weak.upgrade().is_none(),
        "runtime shutdown must release the stalling provider and its captured state"
    );
}

#[test]
fn hard_crash_generation_fixture_child() {
    let Some(root) = std::env::var_os(HARD_CRASH_GENERATION_ROOT_ENV) else {
        return;
    };
    let preserve_partial_generations =
        std::env::var(HARD_CRASH_GENERATION_PRESERVE_ENV).as_deref() == Ok("true");
    let reopen_preserve_partial_generations =
        std::env::var(HARD_CRASH_GENERATION_REOPEN_PRESERVE_ENV).as_deref() == Ok("true");
    let root = PathBuf::from(root);
    let core = Core::open(CoreConfig::new(&root)).expect("open hard-crash child Core");
    let mut settings = core.get_settings().expect("load hard-crash settings");
    settings.preserve_partial_generations = preserve_partial_generations;
    core.update_settings(&settings)
        .expect("configure hard-crash partial preservation");
    let mut card = NamedTempFile::new_in(&root).expect("hard-crash character card");
    write!(
        card,
        r#"{{"spec":"chara_card_v3","data":{{"name":"Crash","description":"Fixture"}}}}"#,
    )
    .expect("write hard-crash character card");
    let inspection = core
        .inspect_import(card.path())
        .expect("inspect hard-crash card");
    let character = core
        .commit_import(&inspection.id)
        .expect("commit hard-crash card");
    let conversation = core
        .open_conversation(&character.id)
        .expect("open hard-crash conversation");
    let partial = std::env::var(HARD_CRASH_GENERATION_PARTIAL_ENV)
        .unwrap_or_else(|_| "durable hard-crash checkpoint".to_owned());
    let (provider, provider_started) = StallingProvider::new(&partial);
    let generation_id = core
        .send_message_with_provider(
            &conversation.id,
            "start hard-crash fixture",
            "stalling".to_owned(),
            None,
            provider,
        )
        .expect("start hard-crash generation");
    provider_started
        .recv_timeout(Duration::from_secs(2))
        .expect("hard-crash provider started");
    if preserve_partial_generations {
        let checkpoint = wait_for_partial(&core, &conversation.id, &partial);
        assert_eq!(checkpoint.status, MessageStatus::Pending);
    }
    settings.preserve_partial_generations = reopen_preserve_partial_generations;
    core.update_settings(&settings)
        .expect("configure hard-crash reopen preservation");
    let generation = core
        .inner
        .storage
        .get_generation(&generation_id)
        .expect("read running hard-crash generation");
    let assistant_message_id = generation
        .assistant_message_id
        .clone()
        .expect("hard-crash generation assistant");
    let attempt = core
        .inner
        .storage
        .get_generation_attempt(&generation_id)
        .expect("read running hard-crash attempt");
    assert_eq!(attempt.status, GenerationAttemptStatus::Running);
    let fixture = HardCrashGenerationFixture {
        conversation_id: conversation.id.0,
        branch_id: generation.branch_id.0,
        user_message_id: generation.user_message_id.0,
        assistant_message_id: assistant_message_id.0,
        generation_id: generation_id.0,
        running_attempt_revision: attempt.revision,
        partial,
    };
    let encoded = serde_json::to_vec(&fixture).expect("encode hard-crash generation fixture");
    let mut sidecar = File::create(hard_crash_generation_fixture_path(&root))
        .expect("create hard-crash generation fixture");
    sidecar
        .write_all(&encoded)
        .expect("write hard-crash generation fixture");
    sidecar
        .flush()
        .expect("flush hard-crash generation fixture");
    sidecar
        .sync_all()
        .expect("sync hard-crash generation fixture");
    std::process::exit(HARD_CRASH_GENERATION_EXIT_CODE);
}

#[test]
#[allow(clippy::too_many_lines)]
fn hard_crash_recovery_closes_attempt_and_terminal_lifecycle_once() {
    for preserve_partial_generations in [true, false] {
        let root = tempdir().expect("hard-crash recovery root");
        let fixture = run_hard_crash_generation_child(
            root.path(),
            preserve_partial_generations,
            preserve_partial_generations,
            "durable hard-crash checkpoint",
        );
        let generation_id = GenerationId(fixture.generation_id.clone());
        let reopened = Core::open(CoreConfig::new(root.path())).expect("recover hard crash");
        let generation = reopened
            .inner
            .storage
            .get_generation(&generation_id)
            .expect("recovered generation");
        assert_eq!(generation.status, GenerationStatus::Cancelled);
        assert_eq!(
            generation.error_code.as_deref(),
            Some(CoreErrorCode::Cancelled.as_str())
        );
        assert!(generation.finished_at.is_some());
        assert_eq!(
            reopened
                .get_conversation(&ConversationId(fixture.conversation_id.clone()))
                .expect("recovered conversation")
                .updated_at,
            generation.finished_at.expect("recovery finished_at")
        );
        let attempt = reopened
            .inner
            .storage
            .get_generation_attempt(&generation_id)
            .expect("recovered generation attempt");
        assert_eq!(attempt.status, GenerationAttemptStatus::Completed);
        assert_eq!(attempt.revision, fixture.running_attempt_revision + 1);

        let messages = reopened
            .list_messages(&ConversationId(fixture.conversation_id.clone()))
            .expect("recovered messages");
        let branch = reopened
            .inner
            .storage
            .get_conversation_branch(&ConversationBranchId(fixture.branch_id.clone()))
            .expect("recovered branch");
        if preserve_partial_generations {
            assert_eq!(messages.len(), 2);
            assert_eq!(messages[1].id.0, fixture.assistant_message_id);
            assert_eq!(messages[1].content, fixture.partial);
            assert_eq!(messages[1].status, MessageStatus::Cancelled);
            assert_eq!(
                branch.head_message_id.as_ref().map(|id| id.0.as_str()),
                Some(fixture.assistant_message_id.as_str())
            );
        } else {
            assert_eq!(messages.len(), 1);
            assert_eq!(messages[0].id.0, fixture.user_message_id);
            assert_eq!(
                branch.head_message_id.as_ref().map(|id| id.0.as_str()),
                Some(fixture.user_message_id.as_str())
            );
        }

        let rows = generation_lifecycle_rows(root.path(), &fixture.generation_id);
        let expected_count = if preserve_partial_generations { 2 } else { 1 };
        assert_eq!(rows.len(), expected_count);
        assert_eq!(
            rows[0],
            GenerationLifecycleRow {
                occurrence_id: format!("after-generation:{}", fixture.generation_id),
                event_kind: "after_generation".to_owned(),
                status: "pending".to_owned(),
                exact_head_message_id: Some(if preserve_partial_generations {
                    fixture.assistant_message_id.clone()
                } else {
                    fixture.user_message_id.clone()
                }),
                owner_message_id: preserve_partial_generations
                    .then(|| fixture.assistant_message_id.clone()),
            }
        );
        if preserve_partial_generations {
            assert_eq!(
                rows[1],
                GenerationLifecycleRow {
                    occurrence_id: format!("message-committed:{}", fixture.assistant_message_id),
                    event_kind: "message_committed".to_owned(),
                    status: "pending".to_owned(),
                    exact_head_message_id: Some(fixture.assistant_message_id.clone()),
                    owner_message_id: Some(fixture.assistant_message_id.clone()),
                }
            );
        }

        let receipt = reopened
            .drain_core_lifecycle_occurrences(64)
            .expect("drain recovered lifecycle");
        let recovered_events = receipt
            .deliveries
            .iter()
            .filter(|delivery| {
                delivery.generation_id.as_ref().map(|id| id.0.as_str())
                    == Some(fixture.generation_id.as_str())
            })
            .map(|delivery| delivery.event_kind)
            .collect::<Vec<_>>();
        let expected_events = if preserve_partial_generations {
            vec![
                LifecycleOccurrenceKind::AfterGeneration,
                LifecycleOccurrenceKind::MessageCommitted,
            ]
        } else {
            vec![LifecycleOccurrenceKind::AfterGeneration]
        };
        assert_eq!(recovered_events, expected_events);
        drop(reopened);

        let reopened =
            Core::open(CoreConfig::new(root.path())).expect("second hard-crash recovery open");
        assert_eq!(
            reopened
                .inner
                .storage
                .get_generation_attempt(&generation_id)
                .expect("idempotent attempt")
                .revision,
            fixture.running_attempt_revision + 1
        );
        let rows = generation_lifecycle_rows(root.path(), &fixture.generation_id);
        assert_eq!(rows.len(), expected_count);
        assert!(rows.iter().all(|row| row.status == "acknowledged"));
        let second = reopened
            .drain_core_lifecycle_occurrences(64)
            .expect("second lifecycle drain");
        assert!(second.deliveries.iter().all(|delivery| {
            delivery.generation_id.as_ref().map(|id| id.0.as_str())
                != Some(fixture.generation_id.as_str())
        }));
    }
}

#[test]
fn hard_crash_recovery_uses_durable_checkpoint_instead_of_reopen_setting() {
    for (label, launch_preserve, reopen_preserve, partial, durable_content, keep_assistant) in [
        ("empty-before-first-checkpoint", true, true, "", "", false),
        (
            "checkpoint-survives-setting-disable",
            true,
            false,
            "durable checkpoint",
            "durable checkpoint",
            true,
        ),
        (
            "disabled-launch-cannot-be-enabled-on-reopen",
            false,
            true,
            "uncheckpointed delta",
            "",
            false,
        ),
    ] {
        let root = tempdir().expect("hard-crash policy root");
        let fixture =
            run_hard_crash_generation_child(root.path(), launch_preserve, reopen_preserve, partial);
        assert_eq!(
            hard_crash_assistant_content(root.path(), &fixture.assistant_message_id),
            durable_content,
            "{label}: launch policy must determine the durable checkpoint fact"
        );
        let reopened = Core::open(CoreConfig::new(root.path())).expect("recover hard crash");
        let messages = reopened
            .list_messages(&ConversationId(fixture.conversation_id.clone()))
            .expect("recovered messages");
        let retained = messages
            .iter()
            .find(|message| message.id.0 == fixture.assistant_message_id);
        assert_eq!(
            retained.is_some(),
            keep_assistant,
            "{label}: reopen setting must not reinterpret the durable checkpoint"
        );
        if let Some(retained) = retained {
            assert_eq!(retained.content, durable_content);
            assert_eq!(retained.status, MessageStatus::Cancelled);
        }
        let rows = generation_lifecycle_rows(root.path(), &fixture.generation_id);
        assert_eq!(rows.len(), if keep_assistant { 2 } else { 1 });
        assert_eq!(
            rows.iter().any(|row| row.event_kind == "message_committed"),
            keep_assistant,
            "{label}: MessageCommitted must match retained durable content"
        );
    }
}

#[test]
fn interrupted_generation_recovery_rolls_back_on_outbox_conflict() {
    let root = tempdir().expect("hard-crash rollback root");
    let fixture =
        run_hard_crash_generation_child(root.path(), true, true, "durable hard-crash checkpoint");
    let database_path = hard_crash_database_path(root.path());
    let database = rusqlite::Connection::open(&database_path).expect("open crash database");
    database
        .execute_batch(
            "CREATE TRIGGER test_reject_recovered_message_committed
                 BEFORE INSERT ON core_lifecycle_outbox
                 WHEN NEW.event_kind = 'message_committed'
                 BEGIN
                   SELECT RAISE(ABORT, 'synthetic recovered lifecycle failure');
                 END;",
        )
        .expect("inject recovered lifecycle failure");
    drop(database);

    let error = Core::open(CoreConfig::new(root.path()))
        .err()
        .expect("outbox conflict must reject recovery");
    assert_eq!(error.code, CoreErrorCode::StorageUnavailable);
    let database = rusqlite::Connection::open(&database_path).expect("inspect rollback database");
    assert_eq!(
        database
            .query_row(
                "SELECT status FROM generations WHERE id = ?1",
                [&fixture.generation_id],
                |row| row.get::<_, String>(0),
            )
            .expect("generation status after rollback"),
        "running"
    );
    assert_eq!(
        database
            .query_row(
                "SELECT status FROM messages WHERE id = ?1",
                [&fixture.assistant_message_id],
                |row| row.get::<_, String>(0),
            )
            .expect("message status after rollback"),
        "pending"
    );
    assert_eq!(
        database
            .query_row(
                "SELECT status, revision
                     FROM generation_attempt_intents WHERE generation_id = ?1",
                [&fixture.generation_id],
                |row| Ok((row.get::<_, String>(0)?, row.get::<_, u64>(1)?)),
            )
            .expect("attempt after rollback"),
        ("running".to_owned(), fixture.running_attempt_revision)
    );
    assert_eq!(
        database
            .query_row(
                "SELECT COUNT(*)
                     FROM core_lifecycle_outbox
                     WHERE generation_id = ?1
                       AND event_kind IN ('after_generation', 'message_committed')",
                [&fixture.generation_id],
                |row| row.get::<_, u64>(0),
            )
            .expect("terminal lifecycle count after rollback"),
        0,
        "the earlier AfterGeneration insert must roll back with MessageCommitted"
    );
    database
        .execute_batch("DROP TRIGGER test_reject_recovered_message_committed;")
        .expect("remove recovered lifecycle failure");
    drop(database);
    Core::open(CoreConfig::new(root.path())).expect("recover after removing outbox conflict");
}

#[test]
fn timed_partial_checkpoint_survives_restart_when_preservation_is_enabled() {
    let (root, core, character) = imported_core();
    core.update_settings(&AppSettings {
        preserve_partial_generations: true,
        selected_provider_profile_id: None,
        selected_model_route_id: None,
        selected_generation_preset_id: None,
        ..AppSettings::default()
    })
    .expect("enable partial preservation");
    let conversation = core.open_conversation(&character.id).expect("conversation");
    let partial = "latest timer checkpoint";
    let (provider, provider_started) = StallingProvider::new(partial);
    core.send_message_with_provider(
        &conversation.id,
        "start",
        "stalling".to_owned(),
        None,
        provider,
    )
    .expect("start generation");
    provider_started
        .recv_timeout(Duration::from_secs(2))
        .expect("provider started");

    let checkpoint = wait_for_partial(&core, &conversation.id, partial);
    assert_eq!(checkpoint.status, MessageStatus::Pending);
    drop(core);

    let reopened = Core::open(CoreConfig::new(root.path())).expect("reopen");
    let messages = reopened
        .list_messages(&conversation.id)
        .expect("restored messages");
    assert_eq!(messages.len(), 2);
    assert_eq!(messages[1].content, partial);
    assert_eq!(messages[1].status, MessageStatus::Cancelled);
}

#[test]
fn partial_checkpoint_is_never_written_when_preservation_is_disabled() {
    let (root, core, character) = imported_core();
    core.update_settings(&AppSettings {
        preserve_partial_generations: false,
        selected_provider_profile_id: None,
        selected_model_route_id: None,
        selected_generation_preset_id: None,
        ..AppSettings::default()
    })
    .expect("disable partial preservation");
    let conversation = core.open_conversation(&character.id).expect("conversation");
    let partial = "must not persist";
    let (provider, provider_started) = StallingProvider::new(partial);
    core.send_message_with_provider(
        &conversation.id,
        "start",
        "stalling".to_owned(),
        None,
        provider,
    )
    .expect("start generation");
    provider_started
        .recv_timeout(Duration::from_secs(2))
        .expect("provider started");
    thread::sleep(PARTIAL_CHECKPOINT_INTERVAL + Duration::from_millis(150));

    let messages = core.list_messages(&conversation.id).expect("messages");
    assert_eq!(messages.len(), 2);
    assert_eq!(messages[1].status, MessageStatus::Pending);
    assert!(messages[1].content.is_empty());
    drop(core);

    let reopened = Core::open(CoreConfig::new(root.path())).expect("reopen");
    let restored = reopened
        .list_messages(&conversation.id)
        .expect("restored messages");
    assert_eq!(restored.len(), 1);
    assert_eq!(restored[0].content, "start");
}

#[test]
fn partial_checkpoint_byte_threshold_is_inclusive() {
    assert!(!partial_checkpoint_due(PARTIAL_CHECKPOINT_BYTES - 1, 0));
    assert!(partial_checkpoint_due(PARTIAL_CHECKPOINT_BYTES, 0));
    assert!(partial_checkpoint_due(
        PARTIAL_CHECKPOINT_BYTES * 2,
        PARTIAL_CHECKPOINT_BYTES
    ));
}
