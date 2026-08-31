    use lorepia_core::{
        BoundedJson, CharacterContentV1, ChatEvent, ChatEventKind, ConversationId, GenerationId,
        GenerationUsage, Message, MessageId, MessagePresentation, MessageStatus,
        MessageTransformDiagnostic, MessageTransformDisposition, MessageTransformStage,
        Sha256Digest,
    };

    use super::{CharacterRenderProfileDto, ChatEventDto, ChatEventKindDto, MessageDto};

    #[test]
    fn character_render_profile_partitions_transforms_and_indexes_misleading_asset_names() {
        let content: CharacterContentV1 = serde_json::from_value(serde_json::json!({
            "knowledge_book": {
                "embedded": {
                    "id": "book-1",
                    "entries": [
                        {
                            "id": "regex-entry",
                            "content": "runtime lore",
                            "enabled": true,
                            "primary_keys": (0..130)
                                .map(|index| format!("rule-{index}"))
                                .collect::<Vec<_>>(),
                            "use_regex": true
                        },
                        {
                            "id": "folder-entry",
                            "content": "must not execute",
                            "enabled": true,
                            "primary_keys": ["(a+)+$"],
                            "use_regex": true,
                            "folder": true
                        }
                    ]
                }
            },
            "assets": [{
                "id": "asset-expression-1",
                "sha256": "a".repeat(64),
                "media_type": "image/webp",
                "role": "expression",
                "name": "pose.png.png",
                "size_bytes": 12,
                "source": {
                    "kind": "charx_package",
                    "logical_path": "assets/other/image/pose.png.png"
                }
            }],
            "runtime": {
                "transforms": [
                    {
                        "id": "output-rule",
                        "phase": "provider_output",
                        "pattern": "<lmg",
                        "replacement": "<img",
                        "flags": "g"
                    },
                    {
                        "id": "display-rule",
                        "phase": "display",
                        "pattern": "<Status>",
                        "replacement": "<section>",
                        "flags": "i"
                    },
                    {
                        "id": "disabled-display-rule",
                        "phase": "display",
                        "enabled": false,
                        "pattern": "never-run",
                        "replacement": "blocked",
                        "flags": ""
                    }
                ],
                "scripts": [{
                    "id": "script-1",
                    "event": "start",
                    "language": "lua",
                    "source": "return"
                }],
                "background_markup": "<style>.card{color:red}</style>",
                "initial_variables": {"mode": "0"}
            }
        }))
        .expect("decode synthetic character content");

        let profile = CharacterRenderProfileDto::from_content(
            "character-1".to_owned(),
            Some("revision-1".to_owned()),
            content,
        );

        assert_eq!(profile.assets.len(), 1);
        assert!(
            profile.assets[0]
                .aliases
                .iter()
                .any(|alias| alias == "pose")
        );
        assert!(
            profile.assets[0]
                .aliases
                .iter()
                .any(|alias| alias == "pose.png.png")
        );
        assert_eq!(profile.output_transforms[0].pattern, "<lmg");
        assert_eq!(profile.display_transforms[0].pattern, "<Status>");
        assert_eq!(profile.display_transforms.len(), 1);
        assert_eq!(profile.initial_variables["mode"], "0");
        assert_eq!(profile.runtime_script_count, 1);
        assert_eq!(profile.runtime_knowledge.len(), 1);
        assert_eq!(profile.runtime_knowledge[0].primary_keys.len(), 128);
    }

    #[test]
    fn event_projection_preserves_v4_wire_variant_and_omits_raw_usage_summary() {
        let canary = "provider-raw-summary-canary";
        let event = ChatEvent::new(
            GenerationId("generation-1".to_owned()),
            ConversationId("conversation-1".to_owned()),
            7,
            ChatEventKind::UsageUpdated(GenerationUsage {
                input_tokens: Some(11),
                cached_read_tokens: Some(3),
                cached_write_tokens: None,
                output_tokens: Some(5),
                reasoning_tokens: Some(2),
                tool_tokens: None,
                provider_raw_summary: Some(
                    BoundedJson::parse(format!(r#"{{"private":"{canary}"}}"#))
                        .expect("bounded metadata"),
                ),
            }),
        );

        let dto = ChatEventDto::from(event);
        let json = serde_json::to_string(&dto).expect("serialize event DTO");

        assert!(matches!(dto.kind, ChatEventKindDto::UsageUpdated(_)));
        assert!(json.contains(r#""type":"usage_updated""#));
        assert!(!json.contains("provider_raw_summary"));
        assert!(!json.contains(canary));
    }

    #[test]
    fn message_projection_uses_display_text_and_exposes_only_content_free_diagnostics() {
        let generation_id = GenerationId("generation-display-1".to_owned());
        let mut message = Message::pending_assistant(
            ConversationId("conversation-display-1".to_owned()),
            MessageId("user-display-1".to_owned()),
            generation_id,
        );
        message.content = "CANONICAL_CONTENT_CANARY".to_owned();
        message.status = MessageStatus::Complete;
        let recorded_at = message.created_at;
        let presentation = MessagePresentation {
            message,
            display_content: "Rendered display text".to_owned(),
            canonical_content_sha256: Sha256Digest::parse("a".repeat(64))
                .expect("canonical digest"),
            display_content_sha256: Sha256Digest::parse("b".repeat(64)).expect("display digest"),
            projection_diagnostics_sha256: Some(
                Sha256Digest::parse("c".repeat(64)).expect("diagnostic digest"),
            ),
            transform_diagnostics: vec![MessageTransformDiagnostic {
                set_revision_id: Some("transform-revision-1".to_owned()),
                rule_id: Some("display-rule-1".to_owned()),
                stage: MessageTransformStage::DisplayOnly,
                disposition: MessageTransformDisposition::Applied,
                code: None,
                before_sha256: Sha256Digest::parse("a".repeat(64)).expect("before digest"),
                after_sha256: Some(Sha256Digest::parse("b".repeat(64)).expect("after digest")),
                recorded_at,
            }],
        };

        let dto = MessageDto::from(presentation);
        let json = serde_json::to_string(&dto).expect("serialize projected message DTO");
        let projection = dto.display_projection.expect("display projection metadata");

        assert_eq!(dto.content, "Rendered display text");
        assert_eq!(projection.canonical_content_sha256, "a".repeat(64));
        assert_eq!(projection.display_content_sha256, "b".repeat(64));
        assert_eq!(projection.diagnostics_sha256, "c".repeat(64));
        assert_eq!(projection.diagnostics.len(), 1);
        assert!(!json.contains("CANONICAL_CONTENT_CANARY"));
        assert_eq!(json.matches("Rendered display text").count(), 1);
        assert!(!json.contains("\"pattern\""));
        assert!(!json.contains("\"replacement\""));
    }
