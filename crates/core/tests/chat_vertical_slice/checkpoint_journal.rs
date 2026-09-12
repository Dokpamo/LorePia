use super::*;

#[test]
fn live_checkpoint_uses_a_suffix_and_cancellation_materializes_exact_text() {
    let (root, core, character_id) = imported_core();
    let (base_url, provider_ready, provider_stop) = spawn_stalling_provider();
    let profile = core
        .upsert_provider_profile(ProviderProfile {
            id: "checkpoint-journal-test".to_owned(),
            display_name: "Checkpoint journal test".to_owned(),
            base_url,
            model: "fixture".to_owned(),
            timeout_seconds: 5,
        })
        .expect("save provider profile");
    core.update_settings(&AppSettings {
        preserve_partial_generations: true,
        ..AppSettings::default()
    })
    .expect("preserve checkpoints");
    let conversation = core.open_conversation(&character_id).expect("conversation");
    let mut events = core.subscribe_events();
    let generation = core
        .send_message(
            &conversation.id,
            "증분 보존",
            GenerationOperationContext::New {
                operation_nonce: "checkpoint-journal-live-v1",
            },
            &profile.id,
            None,
        )
        .expect("start generation");
    provider_ready
        .recv_timeout(Duration::from_secs(2))
        .expect("provider started");
    let deadline = Instant::now() + Duration::from_secs(2);
    loop {
        let messages = core
            .list_messages(&conversation.id)
            .expect("pending messages");
        if messages[1].content == "부분" {
            assert_eq!(messages[1].status, MessageStatus::Pending);
            break;
        }
        assert!(
            Instant::now() < deadline,
            "partial checkpoint was not visible"
        );
        thread::sleep(Duration::from_millis(10));
    }
    let connection = rusqlite::Connection::open_with_flags(
        active_database_path(root.path()),
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
    )
    .expect("inspect temporary database");
    let raw_content = || {
        connection
            .query_row(
                "SELECT content FROM messages WHERE generation_id = ?1 AND role = 'assistant'",
                [generation.0.as_str()],
                |row| row.get::<_, String>(0),
            )
            .expect("raw assistant snapshot")
    };
    assert_eq!(
        raw_content(),
        "",
        "live suffix does not rewrite the base row"
    );

    core.cancel_generation(&generation).expect("cancel");
    let received = collect_generation_events_until_terminal(&mut events, &generation);
    assert!(matches!(
        received.last().map(|event| &event.kind),
        Some(ChatEventKind::GenerationCancelled)
    ));
    assert_eq!(
        raw_content(),
        "부분",
        "terminal publication follows materialization"
    );
    let _ = provider_stop.send(());
    drop(connection);
    drop(core);
    let storage = open_storage_after_core_drop(root.path());
    let restored = storage
        .list_messages(&conversation.id)
        .expect("restored messages");
    assert_eq!(restored[1].content, "부분");
    assert_eq!(restored[1].status, MessageStatus::Cancelled);
}
