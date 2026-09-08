#[test]
fn generation_presentations_return_only_verified_terminal_pair_and_route() {
    let (_root, core, character) = imported_core();
    let conversation = core.open_conversation(&character.id).expect("conversation");
    let branch = core
        .get_conversation_state(&conversation.id)
        .expect("state")
        .active_branch_id;
    let transforms = display_only_generation_transform_set();
    install_generation_transform_fixture(
        &core,
        &conversation.id,
        &transforms,
        "synthetic.incremental.preset",
        "synthetic.incremental.binding",
    );
    let mut generations = Vec::new();
    for index in 0..3 {
        let generation = core
            .send_message_with_provider(
                &conversation.id,
                &format!("user {index}"),
                "static".to_owned(),
                None,
                Arc::new(StaticProvider::new("Synthetic reply")),
            )
            .expect("send");
        wait_for_generation_status(&core, &generation, GenerationStatus::Complete);
        wait_for_generation_registry_to_drain(&core);
        generations.push(generation);
    }
    let full = core
        .list_branch_message_presentations(&branch)
        .expect("full");
    assert_eq!(full.len(), 6);
    for (index, generation) in generations.iter().enumerate() {
        let pair = core
            .list_generation_message_presentations(&conversation.id, &branch, generation)
            .expect("terminal pair");
        assert_eq!(pair, full[index * 2..index * 2 + 2]);
        assert!(pair[1].projection_diagnostics_sha256.is_some());
        assert!(!pair[1].transform_diagnostics.is_empty());
    }
    assert!(
        core.list_generation_message_presentations(
            &ConversationId::new(),
            &branch,
            &generations[2]
        )
        .expect("wrong conversation")
        .is_empty()
    );
    assert!(
        core.list_generation_message_presentations(
            &conversation.id,
            &ConversationBranchId::new(),
            &generations[2]
        )
        .expect("wrong branch")
        .is_empty()
    );
    assert!(
        core.list_generation_message_presentations(&conversation.id, &branch, &GenerationId::new())
            .expect("missing generation")
            .is_empty()
    );
}

#[test]
fn generation_presentations_do_not_publish_pending_provider_output() {
    let (_root, core, character) = imported_core();
    let conversation = core.open_conversation(&character.id).expect("conversation");
    let branch = core
        .get_conversation_state(&conversation.id)
        .expect("state")
        .active_branch_id;
    let (provider, started, release) = CatchupSnapshotProvider::new();
    let generation = core
        .send_message_with_provider(
            &conversation.id,
            "hello",
            "static".to_owned(),
            None,
            provider,
        )
        .expect("send");
    started
        .recv_timeout(Duration::from_secs(2))
        .expect("provider running");
    assert!(
        core.list_generation_message_presentations(&conversation.id, &branch, &generation)
            .expect("pending pair is unavailable")
            .is_empty()
    );
    release.send(()).expect("release");
    wait_for_generation_status(&core, &generation, GenerationStatus::Complete);
    wait_for_generation_registry_to_drain(&core);
    let pair = core
        .list_generation_message_presentations(&conversation.id, &branch, &generation)
        .expect("terminal pair");
    assert_eq!(pair.len(), 2);
    assert_eq!(pair[1].message.content, "text-prefix+text-suffix");
}
