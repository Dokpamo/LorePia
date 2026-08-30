fn install_protocol_state_target(
    storage: &Storage,
    id: &str,
    family: ApiFamily,
) -> (ModelRouteId, GenerationPresetId, String) {
    let profile = ProviderProfile {
        id: id.to_owned(),
        display_name: format!("Protocol State {id}"),
        base_url: "http://127.0.0.1:11434/v1".to_owned(),
        model: format!("{id}-model"),
        timeout_seconds: 30,
    };
    let (mut connection, mut route, preset) =
        legacy_provider_graph(&profile, Utc::now()).expect("protocol-state graph");
    let mut template = legacy_provider_template().expect("protocol-state template");
    template.id = ProviderTemplateId::from(format!("{id}-template"));
    template.display_name = format!("Protocol State {id}");
    template.source = TemplateSource::UserDiscovered;
    template.api_family = family;
    template.default_manifest.api_family = family;
    connection.template_id = template.id.clone();
    connection.template_version = template.manifest_version;
    route.api_family = family;
    storage
        .save_provider_template(&template)
        .expect("save protocol-state template");
    storage
        .insert_provider_connection(&connection)
        .expect("insert protocol-state connection");
    storage
        .save_model_route(&route)
        .expect("save protocol-state route");
    storage
        .save_generation_preset(&preset)
        .expect("save protocol-state preset");
    (route.id, preset.id, profile.model)
}

fn gemini_states_with_serialized_len(target: usize) -> Vec<OpaqueReasoningState> {
    fn build(counts: [usize; 4], append_plain_byte: bool) -> Vec<OpaqueReasoningState> {
        counts
            .into_iter()
            .enumerate()
            .map(|(part_index, count)| {
                let mut signature = "\\".repeat(count);
                if append_plain_byte && part_index == 0 {
                    signature.push('a');
                }
                OpaqueReasoningState::GeminiThoughtSignature {
                    part_index: u32::try_from(part_index).expect("bounded part index"),
                    signature: lorepia_domain::OpaqueReasoningData::parse(signature)
                        .expect("bounded backslash-heavy signature"),
                }
            })
            .collect()
    }

    let mut counts = [1_usize; 4];
    let baseline = serde_json::to_vec(&build(counts, false))
        .expect("serialize baseline opaque state")
        .len();
    let mut remaining = target
        .checked_sub(baseline)
        .expect("target must fit the fixed state envelope");
    let append_plain_byte = remaining % 2 == 1;
    remaining -= usize::from(append_plain_byte);
    let mut extra_backslashes = remaining / 2;
    for (index, count) in counts.iter_mut().enumerate() {
        let suffix_bytes = usize::from(append_plain_byte && index == 0);
        let capacity = lorepia_domain::MAX_OPAQUE_REASONING_ITEM_BYTES - *count - suffix_bytes;
        let added = capacity.min(extra_backslashes);
        *count += added;
        extra_backslashes -= added;
    }
    assert_eq!(extra_backslashes, 0, "target exceeds domain item bounds");
    let states = build(counts, append_plain_byte);
    assert_eq!(
        serde_json::to_vec(&states)
            .expect("serialize exact opaque state")
            .len(),
        target
    );
    states
}

#[test]
fn usage_overflow_can_be_compensated_and_the_branch_accepts_another_generation() {
    let (_root, storage, conversation, branch_id) = imported_storage();
    let (_user, pending, generation) =
        append_pending_generation(&storage, &conversation.id, &branch_id, None, "first");
    let mut assistant = pending.clone();
    assistant.content = "response before invalid usage".to_owned();
    assistant.status = MessageStatus::Complete;
    let error = storage
        .finalize_generation(
            &assistant,
            Some(&lorepia_domain::GenerationUsage {
                input_tokens: Some(1),
                cached_write_tokens: Some(i64::MAX as u64 + 1),
                output_tokens: Some(1),
                ..lorepia_domain::GenerationUsage::default()
            }),
            None,
            true,
        )
        .expect_err("overflow usage must reject normal finalization");
    assert_eq!(error.code, CoreErrorCode::InvalidInput);
    assert_eq!(
        storage
            .get_generation(&generation.id)
            .expect("running generation")
            .status,
        GenerationStatus::Running
    );
    assert_eq!(
        storage
            .list_branch_messages(&branch_id)
            .expect("pending lineage")[1]
            .status,
        MessageStatus::Pending
    );

    assistant.status = MessageStatus::Failed;
    storage
        .fail_generation_after_finalize_error(&assistant, true)
        .expect("compensate overflow");
    let failed = storage
        .get_generation(&generation.id)
        .expect("failed generation");
    assert_eq!(failed.status, GenerationStatus::Failed);
    assert_eq!(failed.input_tokens, None);
    assert_eq!(failed.cached_write_tokens, None);
    assert_eq!(failed.output_tokens, None);
    assert_eq!(
        failed.error_code.as_deref(),
        Some(CoreErrorCode::StorageUnavailable.as_str())
    );
    assert!(failed.finished_at.is_some());
    let messages = storage
        .list_branch_messages(&branch_id)
        .expect("failed lineage");
    assert_eq!(messages[1].status, MessageStatus::Failed);

    let (_, retry) = append_complete_generation(
        &storage,
        &conversation.id,
        &branch_id,
        Some(&assistant.id),
        "retry",
        "retry succeeded",
    );
    assert_eq!(retry.status, MessageStatus::Complete);
    assert!(
        storage
            .list_branch_messages(&branch_id)
            .expect("retried lineage")
            .iter()
            .all(|message| message.status != MessageStatus::Pending)
    );
}

#[test]
#[allow(clippy::too_many_lines)]
fn expanded_usage_and_opaque_reasoning_state_survive_reopen() {
    let (root, storage, conversation, branch_id) = imported_storage();
    let cases = vec![
        (
            "openai-responses",
            ApiFamily::OpenAiResponses,
            serde_json::from_value(serde_json::json!({
            "kind": "open_ai_responses",
            "item": {
                "id": "opaque-openai-item-canary",
                "type": "reasoning",
                "summary": [{
                    "type": "summary_text",
                    "text": "opaque-openai-summary-canary"
                }],
                "content": [{
                    "type": "reasoning_text",
                    "text": "opaque-openai-reasoning-text-canary"
                }],
                "encrypted_content": "opaque-openai-content-canary",
                "status": "completed"
            }
            }))
            .expect("bounded OpenAI Responses reasoning item"),
        ),
        (
            "openrouter",
            ApiFamily::OpenAiChatCompletions,
            serde_json::from_value(serde_json::json!({
                "kind": "open_router_reasoning",
                "topology": {
                    "reasoning": "opaque-openrouter-reasoning-canary",
                    "reasoning_details": [concat!(
                        "{\"type\":\"reasoning.encrypted\",",
                        "\"data\":\"opaque-openrouter-data-canary\",",
                        "\"id\":\"opaque-openrouter-id-canary\",",
                        "\"format\":\"openai-responses-v1\"}"
                    ), concat!(
                        "{\"type\":\"reasoning.text\",",
                        "\"signature\":\"opaque-openrouter-signature-only-canary\"}"
                    ), concat!(
                        "{\"type\":\"reasoning.text\",",
                        "\"text\":null,",
                        "\"signature\":\"opaque-openrouter-null-text-canary\",",
                        "\"id\":null,",
                        "\"format\":null,",
                        "\"index\":null}"
                    )]
                }
            }))
            .expect("bounded OpenRouter reasoning topology"),
        ),
        (
            "anthropic",
            ApiFamily::AnthropicMessages,
            OpaqueReasoningState::AnthropicMessages {
                content_blocks: lorepia_domain::AnthropicContentBlockTopology::new(vec![
                    lorepia_domain::AnthropicContentBlock::Thinking {
                        thinking: lorepia_domain::AnthropicBlockText::parse(
                            "opaque-anthropic-thinking-canary",
                        )
                        .expect("bounded Anthropic thinking"),
                        signature: lorepia_domain::OpaqueReasoningData::parse(
                            "opaque-anthropic-signature-canary",
                        )
                        .expect("bounded Anthropic signature"),
                    },
                    lorepia_domain::AnthropicContentBlock::RedactedThinking {
                        data: lorepia_domain::OpaqueReasoningData::parse(
                            "opaque-anthropic-redacted-canary",
                        )
                        .expect("bounded Anthropic redacted thinking"),
                    },
                    lorepia_domain::AnthropicContentBlock::Text {
                        text: lorepia_domain::AnthropicBlockText::parse(
                            "opaque-anthropic-text-canary",
                        )
                        .expect("bounded Anthropic text"),
                    },
                    lorepia_domain::AnthropicContentBlock::ToolUse {
                        id: lorepia_domain::ToolCallId::parse("opaque-anthropic-tool-id-canary")
                            .expect("bounded Anthropic tool ID"),
                        name: lorepia_domain::ToolName::parse("lookup")
                            .expect("bounded Anthropic tool name"),
                        input: lorepia_domain::AnthropicToolInput::from_value(&serde_json::json!({
                            "query": "opaque-anthropic-tool-input-canary"
                        }))
                        .expect("bounded Anthropic tool input"),
                    },
                ])
                .expect("bounded Anthropic content topology"),
            },
        ),
        (
            "gemini",
            ApiFamily::GeminiGenerateContent,
            OpaqueReasoningState::GeminiThoughtSignature {
                part_index: 0,
                signature: lorepia_domain::OpaqueReasoningData::parse(
                    "opaque-gemini-signature-canary",
                )
                .expect("bounded signature"),
            },
        ),
    ];
    let opaque_debug = format!("{cases:?}");
    for canary in [
        "opaque-openai-item-canary",
        "opaque-openai-summary-canary",
        "opaque-openai-reasoning-text-canary",
        "opaque-openai-content-canary",
        "opaque-gemini-signature-canary",
        "opaque-openrouter-reasoning-canary",
        "opaque-openrouter-data-canary",
        "opaque-openrouter-id-canary",
        "opaque-openrouter-signature-only-canary",
        "opaque-openrouter-null-text-canary",
        "opaque-anthropic-text-canary",
        "opaque-anthropic-thinking-canary",
        "opaque-anthropic-signature-canary",
        "opaque-anthropic-redacted-canary",
        "opaque-anthropic-tool-id-canary",
        "opaque-anthropic-tool-input-canary",
    ] {
        assert!(
            !opaque_debug.contains(canary),
            "opaque state Debug output exposed {canary}"
        );
    }

    let usage = GenerationUsage {
        input_tokens: Some(101),
        cached_read_tokens: Some(11),
        cached_write_tokens: Some(12),
        output_tokens: Some(202),
        reasoning_tokens: Some(21),
        tool_tokens: Some(22),
        provider_raw_summary: Some(
            BoundedJson::parse(r#"{"total_tokens":303}"#).expect("bounded summary"),
        ),
    };
    let mut expected_head = None;
    let mut persisted = Vec::new();
    for (id, family, state) in cases {
        let (route_id, preset_id, model) = install_protocol_state_target(&storage, id, family);
        let user = Message::user_after(
            conversation.id.clone(),
            expected_head.clone(),
            format!("persist {id} protocol state"),
        );
        let generation_id = GenerationId::new();
        let pending = Message::pending_assistant(
            conversation.id.clone(),
            user.id.clone(),
            generation_id.clone(),
        );
        let generation = GenerationRecord {
            id: generation_id.clone(),
            conversation_id: conversation.id.clone(),
            branch_id: branch_id.clone(),
            user_message_id: user.id.clone(),
            assistant_message_id: Some(pending.id.clone()),
            mode: ConversationMode::Chat,
            model,
            model_route_id: Some(route_id),
            generation_preset_id: Some(preset_id),
            provider_family: Some(family),
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
            .append_generation(
                &branch_id,
                expected_head.as_ref(),
                &user,
                &pending,
                &generation,
            )
            .expect("append protocol-state generation");
        let mut assistant = pending;
        assistant.content = "complete".to_owned();
        assistant.status = MessageStatus::Complete;
        storage
            .finalize_generation_with_protocol_state(
                &assistant,
                Some(&usage),
                std::slice::from_ref(&state),
                None,
                true,
            )
            .expect("finalize protocol-state generation");
        expected_head = Some(assistant.id);
        persisted.push((generation_id, family, state));
    }

    drop(storage);
    let reopened = Storage::open(root.path()).expect("reopen storage");
    for (generation_id, family, state) in &persisted {
        let restored = reopened
            .get_generation(generation_id)
            .expect("restore generation");
        assert_eq!(restored.provider_family, Some(*family));
        assert_eq!(restored.input_tokens, usage.input_tokens);
        assert_eq!(restored.cached_read_tokens, usage.cached_read_tokens);
        assert_eq!(restored.cached_write_tokens, usage.cached_write_tokens);
        assert_eq!(restored.output_tokens, usage.output_tokens);
        assert_eq!(restored.reasoning_tokens, usage.reasoning_tokens);
        assert_eq!(restored.tool_tokens, usage.tool_tokens);
        assert_eq!(restored.provider_raw_summary, usage.provider_raw_summary);
        assert_eq!(restored.opaque_reasoning_state, std::slice::from_ref(state));
    }
    let first_generation_id = &persisted.first().expect("persisted generation").0;
    let error = reopened
        .connection()
        .expect("reopened connection")
        .execute(
            "UPDATE generations SET status = 'running' WHERE id = ?1",
            [&first_generation_id.0],
        )
        .expect_err("opaque state must remain terminal-only");
    assert!(
        error
            .to_string()
            .contains("generation protocol-state provenance is inconsistent")
    );
}

#[test]
#[allow(clippy::too_many_lines)]
fn exact_serialized_opaque_reasoning_limit_survives_storage_and_reopen() {
    let (root, storage, conversation, branch_id) = imported_storage();
    let (route_id, preset_id, model) = install_protocol_state_target(
        &storage,
        "opaque-serialized-envelope",
        ApiFamily::GeminiGenerateContent,
    );
    let user = Message::user(
        conversation.id.clone(),
        "persist the exact opaque-state JSON envelope",
    );
    let generation_id = GenerationId::new();
    let pending = Message::pending_assistant(
        conversation.id.clone(),
        user.id.clone(),
        generation_id.clone(),
    );
    let generation = GenerationRecord {
        id: generation_id.clone(),
        conversation_id: conversation.id.clone(),
        branch_id: branch_id.clone(),
        user_message_id: user.id.clone(),
        assistant_message_id: Some(pending.id.clone()),
        mode: ConversationMode::Chat,
        model,
        model_route_id: Some(route_id),
        generation_preset_id: Some(preset_id),
        provider_family: Some(ApiFamily::GeminiGenerateContent),
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
        .append_generation(&branch_id, None, &user, &pending, &generation)
        .expect("append exact-envelope generation");

    let states = gemini_states_with_serialized_len(MAX_OPAQUE_REASONING_SERIALIZED_BYTES);
    validate_opaque_reasoning_states(&states).expect("domain accepts the exact JSON envelope");
    let encoded = serde_json::to_string(&states).expect("serialize exact JSON envelope");
    assert_eq!(encoded.len(), MAX_OPAQUE_REASONING_SERIALIZED_BYTES);

    let mut assistant = pending;
    assistant.content = "complete".to_owned();
    assistant.status = MessageStatus::Complete;
    storage
        .finalize_generation_with_protocol_state(&assistant, None, &states, None, true)
        .expect("store the exact opaque-state JSON envelope");
    let stored_len = storage
        .connection()
        .expect("database")
        .query_row(
            "SELECT length(CAST(opaque_reasoning_state_json AS BLOB))
                 FROM generations
                 WHERE id = ?1",
            [&generation_id.0],
            |row| row.get::<_, usize>(0),
        )
        .expect("stored opaque-state byte length");
    assert_eq!(stored_len, MAX_OPAQUE_REASONING_SERIALIZED_BYTES);

    drop(storage);
    let reopened = Storage::open(root.path()).expect("reopen exact opaque-state envelope");
    assert_eq!(
        reopened
            .get_generation(&generation_id)
            .expect("restore exact opaque-state envelope")
            .opaque_reasoning_state,
        states
    );
}

#[test]
#[allow(clippy::too_many_lines)]
fn opaque_reasoning_bounds_and_corruption_fail_closed_without_payloads_in_errors() {
    let (root, storage, conversation, branch_id) = imported_storage();
    let profile = ProviderProfile {
        id: "opaque-state-validation-provider".to_owned(),
        display_name: "Opaque State Validation Provider".to_owned(),
        base_url: "http://127.0.0.1:11434/v1".to_owned(),
        model: "synthetic".to_owned(),
        timeout_seconds: 30,
    };
    storage
        .save_provider_profile(&profile)
        .expect("save opaque-state route");

    let user = Message::user(conversation.id.clone(), "validate opaque protocol state");
    let generation_id = GenerationId::new();
    let pending = Message::pending_assistant(
        conversation.id.clone(),
        user.id.clone(),
        generation_id.clone(),
    );
    let generation = GenerationRecord {
        id: generation_id.clone(),
        conversation_id: conversation.id.clone(),
        branch_id: branch_id.clone(),
        user_message_id: user.id.clone(),
        assistant_message_id: Some(pending.id.clone()),
        mode: ConversationMode::Chat,
        model: profile.model.clone(),
        model_route_id: Some(ModelRouteId::from(profile.id.as_str())),
        generation_preset_id: Some(GenerationPresetId::from(profile.id.as_str())),
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
    storage
        .append_generation(&branch_id, None, &user, &pending, &generation)
        .expect("append opaque-state generation");

    let individual_canary = "opaque-individual-bound-canary";
    let oversized_individual = format!(
        "{individual_canary}{}",
        "i".repeat(lorepia_domain::MAX_OPAQUE_REASONING_ITEM_BYTES)
    );
    let individual_error = lorepia_domain::OpaqueReasoningData::parse(oversized_individual)
        .expect_err("individual opaque payload must be bounded before storage");
    assert!(!individual_error.contains(individual_canary));

    let mut terminal = pending.clone();
    terminal.content = "complete".to_owned();
    terminal.status = MessageStatus::Complete;
    let mismatch_canary = "opaque-family-mismatch-canary";
    let mismatched_state = OpaqueReasoningState::GeminiThoughtSignature {
        part_index: 0,
        signature: lorepia_domain::OpaqueReasoningData::parse(mismatch_canary)
            .expect("bounded mismatch fixture"),
    };
    let matching_state: OpaqueReasoningState = serde_json::from_value(serde_json::json!({
        "kind": "open_router_reasoning",
        "topology": {
            "reasoning_details": [serde_json::json!({
                "type": "reasoning.encrypted",
                "data": "matching-state",
                "id": "matching-detail",
                "format": "openai-responses-v1"
            }).to_string()]
        }
    }))
    .expect("bounded matching fixture");
    let mismatch_error = storage
        .finalize_generation_with_protocol_state(
            &terminal,
            None,
            &[matching_state, mismatched_state],
            None,
            true,
        )
        .expect_err("mixed provider-family state must fail before persistence");
    assert_eq!(mismatch_error.code, CoreErrorCode::InvalidInput);
    assert!(!mismatch_error.message.contains(mismatch_canary));
    assert!(!format!("{mismatch_error:?}").contains(mismatch_canary));

    let aggregate_canary = "opaque-aggregate-bound-canary";
    let aggregate_item: OpaqueReasoningState = serde_json::from_value(serde_json::json!({
        "kind": "open_router_reasoning",
        "topology": {
            "reasoning_details": [serde_json::json!({
                "type": "reasoning.encrypted",
                "data": format!("{aggregate_canary}{}", "a".repeat(60 * 1024)),
                "id": "aggregate-detail",
                "format": "openai-responses-v1"
            }).to_string()]
        }
    }))
    .expect("individually bounded aggregate fixture");
    let aggregate_error = storage
        .finalize_generation_with_protocol_state(
            &terminal,
            None,
            &vec![aggregate_item.clone(); 5],
            None,
            true,
        )
        .expect_err("aggregate opaque payload must be rejected before write");
    assert_eq!(aggregate_error.code, CoreErrorCode::InvalidInput);
    assert!(!aggregate_error.message.contains(aggregate_canary));
    assert!(!format!("{aggregate_error:?}").contains(aggregate_canary));

    let count_item: OpaqueReasoningState = serde_json::from_value(serde_json::json!({
        "kind": "open_router_reasoning",
        "topology": {
            "reasoning_details": [serde_json::json!({
                "type": "reasoning.encrypted",
                "data": "bounded",
                "id": "count-detail",
                "format": "openai-responses-v1"
            }).to_string()]
        }
    }))
    .expect("bounded count fixture");
    let count_error = storage
        .finalize_generation_with_protocol_state(
            &terminal,
            None,
            &vec![count_item; lorepia_domain::MAX_OPAQUE_REASONING_STATE_COUNT + 1],
            None,
            true,
        )
        .expect_err("opaque item count must be rejected before write");
    assert_eq!(count_error.code, CoreErrorCode::InvalidInput);
    assert_eq!(
        storage
            .get_generation(&generation_id)
            .expect("generation remains running after rejected writes")
            .status,
        GenerationStatus::Running
    );
    assert_eq!(
        storage
            .list_branch_messages(&branch_id)
            .expect("messages remain unchanged after rejected writes")
            .last()
            .expect("pending assistant")
            .status,
        MessageStatus::Pending
    );

    let valid_canary = "opaque-valid-storage-canary";
    let valid_state = serde_json::from_value(serde_json::json!({
        "kind": "open_router_reasoning",
        "topology": {
            "reasoning_details": [serde_json::json!({
                "type": "reasoning.encrypted",
                "data": valid_canary,
                "id": "valid-detail",
                "format": "openai-responses-v1"
            }).to_string()]
        }
    }))
    .expect("valid stored state");
    storage
        .finalize_generation_with_protocol_state(
            &terminal,
            None,
            std::slice::from_ref(&valid_state),
            None,
            true,
        )
        .expect("store valid opaque state");

    let malformed = storage
        .connection()
        .expect("database")
        .execute(
            "UPDATE generations
                 SET opaque_reasoning_state_json = '[{\"kind\":'
                 WHERE id = ?1",
            [&generation_id.0],
        )
        .expect_err("schema must reject malformed opaque-state JSON");
    assert!(!malformed.to_string().contains(valid_canary));
    assert_eq!(
        storage
            .get_generation(&generation_id)
            .expect("malformed update did not replace valid state")
            .opaque_reasoning_state,
        vec![valid_state]
    );

    let unknown_canary = "opaque-unknown-payload-canary";
    let unknown_discriminator_canary = "opaque-unknown-kind-canary";
    let unknown_json = serde_json::json!([{
        "kind": unknown_discriminator_canary,
        "payload": unknown_canary
    }])
    .to_string();
    storage
        .connection()
        .expect("database")
        .execute(
            "UPDATE generations
                 SET opaque_reasoning_state_json = ?2
                 WHERE id = ?1",
            params![generation_id.0, unknown_json],
        )
        .expect("inject structurally valid unknown state");
    drop(storage);

    let reopened = Storage::open(root.path()).expect("reopen storage with unknown state");
    let unknown_error = reopened
        .get_generation(&generation_id)
        .expect_err("unknown opaque state must fail closed");
    assert_eq!(unknown_error.code, CoreErrorCode::StorageCorrupted);
    assert!(!unknown_error.message.contains(unknown_canary));
    assert!(!format!("{unknown_error:?}").contains(unknown_canary));
    assert!(!unknown_error.message.contains(unknown_discriminator_canary));
    assert!(!format!("{unknown_error:?}").contains(unknown_discriminator_canary));

    let anthropic_type_canary = "opaque-anthropic-unknown-type-canary";
    let anthropic_unknown_type_json = serde_json::json!([{
        "kind": "anthropic_messages",
        "content_blocks": [{
            "type": anthropic_type_canary,
            "text": "bounded"
        }]
    }])
    .to_string();
    reopened
        .connection()
        .expect("database")
        .execute(
            "UPDATE generations
                 SET opaque_reasoning_state_json = ?2
                 WHERE id = ?1",
            params![generation_id.0, anthropic_unknown_type_json],
        )
        .expect("inject Anthropic topology with unknown block type");
    let anthropic_type_error = reopened
        .get_generation(&generation_id)
        .expect_err("unknown Anthropic block type must fail closed");
    assert_eq!(anthropic_type_error.code, CoreErrorCode::StorageCorrupted);
    assert!(!anthropic_type_error.message.contains(anthropic_type_canary));
    assert!(!format!("{anthropic_type_error:?}").contains(anthropic_type_canary));

    let stored_family_canary = "opaque-stored-family-mismatch-canary";
    let stored_family_mismatch_json = serde_json::json!([{
        "kind": "gemini_thought_signature",
        "part_index": 0,
        "signature": stored_family_canary
    }])
    .to_string();
    reopened
        .connection()
        .expect("database")
        .execute(
            "UPDATE generations
                 SET opaque_reasoning_state_json = ?2
                 WHERE id = ?1",
            params![generation_id.0, stored_family_mismatch_json],
        )
        .expect("inject valid state bound to the wrong provider family");
    let stored_family_error = reopened
        .get_generation(&generation_id)
        .expect_err("stored provider-family mismatch must fail closed");
    assert_eq!(stored_family_error.code, CoreErrorCode::StorageCorrupted);
    assert!(!stored_family_error.message.contains(stored_family_canary));
    assert!(!format!("{stored_family_error:?}").contains(stored_family_canary));

    let openai_corruption_canary = "opaque-openai-corrupt-canary";
    let openai_corrupt_json = serde_json::json!([{
        "kind": "open_ai_responses",
        "item": {
            "id": "bounded-openai-item",
            "type": "reasoning",
            "summary": [{
                "type": "summary_text",
                "text": format!(
                    "{openai_corruption_canary}{}",
                    "o".repeat(lorepia_domain::MAX_OPAQUE_REASONING_ITEM_BYTES)
                )
            }],
            "status": "completed"
        }
    }])
    .to_string();
    reopened
        .connection()
        .expect("database")
        .execute(
            "UPDATE generations
                 SET opaque_reasoning_state_json = ?2
                 WHERE id = ?1",
            params![generation_id.0, openai_corrupt_json],
        )
        .expect("inject invalid OpenAI Responses reasoning item");
    let openai_corruption_error = reopened
        .get_generation(&generation_id)
        .expect_err("oversized OpenAI Responses reasoning item must fail closed");
    assert_eq!(
        openai_corruption_error.code,
        CoreErrorCode::StorageCorrupted
    );
    assert!(
        !openai_corruption_error
            .message
            .contains(openai_corruption_canary)
    );
    assert!(!format!("{openai_corruption_error:?}").contains(openai_corruption_canary));

    let anthropic_topology_canary = "opaque-anthropic-topology-canary";
    let anthropic_invalid_topology_json = serde_json::json!([{
        "kind": "anthropic_messages",
        "content_blocks": [{
            "type": "text",
            "text": anthropic_topology_canary
        }]
    }])
    .to_string();
    reopened
        .connection()
        .expect("database")
        .execute(
            "UPDATE generations
                 SET opaque_reasoning_state_json = ?2
                 WHERE id = ?1",
            params![generation_id.0, anthropic_invalid_topology_json],
        )
        .expect("inject Anthropic topology without thinking state");
    let anthropic_topology_error = reopened
        .get_generation(&generation_id)
        .expect_err("Anthropic topology without thinking must fail closed");
    assert!(
        !anthropic_topology_error
            .message
            .contains(anthropic_topology_canary)
    );
    assert!(!format!("{anthropic_topology_error:?}").contains(anthropic_topology_canary));

    let anthropic_corruption_canary = "opaque-anthropic-corrupt-canary";
    let anthropic_corrupt_json = serde_json::json!([{
        "kind": "anthropic_messages",
        "content_blocks": [{
            "type": "thinking",
            "thinking": "bounded",
            "signature": format!(
                "{anthropic_corruption_canary}{}",
                "s".repeat(lorepia_domain::MAX_OPAQUE_REASONING_ITEM_BYTES)
            )
        }]
    }])
    .to_string();
    reopened
        .connection()
        .expect("database")
        .execute(
            "UPDATE generations
                 SET opaque_reasoning_state_json = ?2
                 WHERE id = ?1",
            params![generation_id.0, anthropic_corrupt_json],
        )
        .expect("inject invalid Anthropic opaque topology");
    let anthropic_corruption_error = reopened
        .get_generation(&generation_id)
        .expect_err("oversized Anthropic stored topology must fail closed");
    assert!(
        !anthropic_corruption_error
            .message
            .contains(anthropic_corruption_canary)
    );
    assert!(!format!("{anthropic_corruption_error:?}").contains(anthropic_corruption_canary));

    let corruption_canary = "opaque-corrupt-payload-canary";
    let corrupt_json = serde_json::json!([{
        "kind": "gemini_thought_signature",
        "part_index": 0,
        "signature": format!(
            "{corruption_canary}{}",
            "c".repeat(lorepia_domain::MAX_OPAQUE_REASONING_ITEM_BYTES)
        )
    }])
    .to_string();
    reopened
        .connection()
        .expect("database")
        .execute(
            "UPDATE generations
                 SET opaque_reasoning_state_json = ?2
                 WHERE id = ?1",
            params![generation_id.0, corrupt_json],
        )
        .expect("inject bounded-JSON but invalid opaque payload");
    let corruption_error = reopened
        .get_generation(&generation_id)
        .expect_err("oversized stored opaque payload must fail closed");
    assert!(!corruption_error.message.contains(corruption_canary));
    assert!(!format!("{corruption_error:?}").contains(corruption_canary));
}

#[test]
fn terminal_database_failure_is_compensated_without_raw_error_text() {
    let (_root, storage, conversation, branch_id) = imported_storage();
    let (_user, pending, generation) = append_pending_generation(
        &storage,
        &conversation.id,
        &branch_id,
        None,
        "trigger failure",
    );
    storage
        .connection()
        .expect("connection")
        .execute_batch(
            "CREATE TEMP TRIGGER reject_complete_generation
                 BEFORE UPDATE OF status ON generations
                 WHEN NEW.status = 'complete'
                 BEGIN
                   SELECT RAISE(ABORT, 'synthetic terminal database failure');
                 END;",
        )
        .expect("install synthetic failure");

    let mut assistant = pending;
    assistant.content = "completed provider response".to_owned();
    assistant.status = MessageStatus::Complete;
    let error = storage
        .finalize_generation(&assistant, None, None, true)
        .expect_err("synthetic terminal update must fail");
    assert_eq!(error.code, CoreErrorCode::StorageUnavailable);

    assistant.status = MessageStatus::Failed;
    storage
        .fail_generation_after_finalize_error(&assistant, true)
        .expect("compensate terminal database failure");
    let failed = storage
        .get_generation(&generation.id)
        .expect("failed generation");
    assert_eq!(failed.status, GenerationStatus::Failed);
    assert_eq!(
        failed.error_code.as_deref(),
        Some(CoreErrorCode::StorageUnavailable.as_str())
    );
    assert!(
        !failed
            .error_code
            .as_deref()
            .unwrap_or_default()
            .contains("synthetic")
    );
    let messages = storage
        .list_branch_messages(&branch_id)
        .expect("failed lineage");
    assert_eq!(messages[1].status, MessageStatus::Failed);
    assert_eq!(messages[1].content, "completed provider response");
}

#[test]
fn compensation_never_regresses_an_already_terminal_generation() {
    let (_root, storage, conversation, branch_id) = imported_storage();
    let (_, complete) = append_complete_generation(
        &storage,
        &conversation.id,
        &branch_id,
        None,
        "already complete",
        "durable response",
    );
    let generation_id = complete
        .generation_id
        .clone()
        .expect("assistant generation id");
    let mut attempted_compensation = complete.clone();
    attempted_compensation.status = MessageStatus::Failed;
    let error = storage
        .fail_generation_after_finalize_error(&attempted_compensation, true)
        .expect_err("terminal generation must reject compensation");
    assert_eq!(error.code, CoreErrorCode::NotFound);

    let generation = storage
        .get_generation(&generation_id)
        .expect("terminal generation");
    assert_eq!(generation.status, GenerationStatus::Complete);
    assert_eq!(generation.error_code, None);
    let messages = storage
        .list_branch_messages(&branch_id)
        .expect("terminal lineage");
    assert_eq!(messages[1].status, MessageStatus::Complete);
    assert_eq!(messages[1].content, "durable response");
}

#[test]
fn discarded_partial_compensation_rewinds_the_branch_head() {
    let (_root, storage, conversation, branch_id) = imported_storage();
    let (user, pending, generation) = append_pending_generation(
        &storage,
        &conversation.id,
        &branch_id,
        None,
        "discard partial",
    );
    let mut assistant = pending;
    assistant.content = "partial response".to_owned();
    assistant.status = MessageStatus::Failed;
    let error = storage
        .finalize_generation(
            &assistant,
            Some(&lorepia_domain::GenerationUsage {
                input_tokens: Some(i64::MAX as u64 + 1),
                output_tokens: None,
                ..lorepia_domain::GenerationUsage::default()
            }),
            Some(CoreErrorCode::ProviderUnavailable.as_str()),
            false,
        )
        .expect_err("overflow usage must reject normal finalization");
    assert_eq!(error.code, CoreErrorCode::InvalidInput);

    storage
        .fail_generation_after_finalize_error(&assistant, false)
        .expect("discard compensated partial");
    assert_eq!(
        storage
            .get_generation(&generation.id)
            .expect("failed generation")
            .status,
        GenerationStatus::Failed
    );
    let branch = storage
        .get_conversation_branch(&branch_id)
        .expect("compensated branch");
    assert_eq!(branch.head_message_id, Some(user.id.clone()));
    assert_eq!(
        storage
            .list_branch_messages(&branch_id)
            .expect("rewound lineage"),
        vec![user]
    );
}
