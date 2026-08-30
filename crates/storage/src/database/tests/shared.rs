fn append_pending_generation(
    storage: &Storage,
    conversation_id: &ConversationId,
    branch_id: &ConversationBranchId,
    expected_head: Option<&MessageId>,
    user_text: &str,
) -> (Message, Message, GenerationRecord) {
    let user = Message::user_after(conversation_id.clone(), expected_head.cloned(), user_text);
    let generation_id = GenerationId::new();
    let pending = Message::pending_assistant(
        conversation_id.clone(),
        user.id.clone(),
        generation_id.clone(),
    );
    let generation = GenerationRecord {
        id: generation_id,
        conversation_id: conversation_id.clone(),
        branch_id: branch_id.clone(),
        user_message_id: user.id.clone(),
        assistant_message_id: Some(pending.id.clone()),
        mode: ConversationMode::Chat,
        model: "synthetic".to_owned(),
        model_route_id: None,
        generation_preset_id: None,
        provider_family: None,
        status: GenerationStatus::Running,
        input_tokens: None,
        cached_read_tokens: None,
        cached_write_tokens: None,
        output_tokens: None,
        reasoning_tokens: None,
        tool_tokens: None,
        provider_raw_summary: None,
        opaque_reasoning_state: Vec::new(),
        error_code: None,
        started_at: pending.created_at,
        finished_at: None,
    };
    storage
        .append_generation(branch_id, expected_head, &user, &pending, &generation)
        .expect("append generation");
    (user, pending, generation)
}

fn provider_generation_record(
    conversation: &Conversation,
    branch_id: &ConversationBranchId,
    route_id: ModelRouteId,
    preset_id: GenerationPresetId,
    model: &str,
    user_text: &str,
) -> (Message, Message, GenerationRecord) {
    let user = Message::user(conversation.id.clone(), user_text);
    let generation_id = GenerationId::new();
    let pending = Message::pending_assistant(
        conversation.id.clone(),
        user.id.clone(),
        generation_id.clone(),
    );
    let generation = GenerationRecord {
        id: generation_id,
        conversation_id: conversation.id.clone(),
        branch_id: branch_id.clone(),
        user_message_id: user.id.clone(),
        assistant_message_id: Some(pending.id.clone()),
        mode: ConversationMode::Chat,
        model: model.to_owned(),
        model_route_id: Some(route_id),
        generation_preset_id: Some(preset_id),
        provider_family: Some(ApiFamily::OpenAiChatCompletions),
        status: GenerationStatus::Running,
        input_tokens: None,
        cached_read_tokens: None,
        cached_write_tokens: None,
        output_tokens: None,
        reasoning_tokens: None,
        tool_tokens: None,
        provider_raw_summary: None,
        opaque_reasoning_state: Vec::new(),
        error_code: None,
        started_at: pending.created_at,
        finished_at: None,
    };
    (user, pending, generation)
}

fn append_complete_generation(
    storage: &Storage,
    conversation_id: &ConversationId,
    branch_id: &ConversationBranchId,
    expected_head: Option<&MessageId>,
    user_text: &str,
    assistant_text: &str,
) -> (Message, Message) {
    let (user, pending, _) = append_pending_generation(
        storage,
        conversation_id,
        branch_id,
        expected_head,
        user_text,
    );
    let mut assistant = pending;
    assistant.content = assistant_text.to_owned();
    assistant.status = MessageStatus::Complete;
    storage
        .finalize_generation(&assistant, None, None, true)
        .expect("finalize generation");
    (user, assistant)
}

fn imported_storage() -> (
    tempfile::TempDir,
    Storage,
    Conversation,
    ConversationBranchId,
) {
    let root = tempdir().expect("temp root");
    let mut staged = NamedTempFile::new_in(root.path()).expect("staging");
    staged.write_all(b"character").expect("source");
    let character = Character::new("Segu", "Guide", hex::encode(Sha256::digest(b"character")));
    let storage = Storage::open(root.path()).expect("open storage");
    storage
        .commit_character_import(
            staged.path(),
            &character,
            9,
            &Uuid::new_v4().to_string(),
            &[],
        )
        .expect("commit import");
    let conversation = Conversation::new(&character.id, &character.name);
    let (_, state) = storage
        .save_conversation_with_mode(&conversation, ConversationMode::Chat)
        .expect("save conversation");
    (root, storage, conversation, state.active_branch_id)
}
