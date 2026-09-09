use super::*;

#[test]
fn terminal_lookahead_is_consumed_and_reinserted_for_native_regex() {
    assert_eq!(
        rewrite_terminal_positive_lookahead("(post)(?=\\nnext|\\nend)", "<$1>"),
        Some(("(post)(\\nnext|\\nend)".to_owned(), "<$1>$2".to_owned()))
    );
}

#[test]
fn named_knowledge_positions_are_expanded_with_the_portable_alias() {
    let positions = BTreeMap::from([(
        "Do-yoon_option1".to_owned(),
        vec!["childhood friend".to_owned()],
    )]);
    assert_eq!(
        resolve_named_knowledge_positions(
            "profile\n{{position::Do-yoon_option1}}\nend",
            &positions,
        ),
        "profile\nchildhood friend\nend"
    );
    assert_eq!(
        resolve_named_knowledge_positions("{{position::missing}}", &positions),
        ""
    );
}

fn extension_entry(path: &str, size_bytes: u64) -> UnknownExtensionEntry {
    UnknownExtensionEntry {
        key: "synthetic".into(),
        source_path: path.into(),
        sha256: Sha256Digest::parse("ab".repeat(32)).expect("digest"),
        size_bytes,
        quarantine: None,
    }
}

#[test]
fn legacy_character_wire_shape_remains_unchanged() {
    let character = Character {
        id: "character".into(),
        name: "Segu".into(),
        description: "Guide".into(),
        source_hash: "12".repeat(32),
        avatar_asset_hash: None,
        created_at: DateTime::parse_from_rfc3339("2026-08-03T00:00:00Z")
            .expect("timestamp")
            .with_timezone(&Utc),
    };

    let value = serde_json::to_value(character).expect("serialize character");
    assert_eq!(
        value.as_object().expect("object").keys().count(),
        6,
        "the existing Character contract must not gain companion fields"
    );
    assert!(value.get("content").is_none());
}

#[test]
fn character_content_defaults_and_accepts_standard_card_aliases() {
    let empty: CharacterContentV1 = serde_json::from_str("{}").expect("default content");
    assert_eq!(empty, CharacterContentV1::default());

    let content: CharacterContentV1 = serde_json::from_str(
        r#"{
            "first_mes":"Hello",
            "mes_example":"User: Hi\nSegu: Welcome",
            "system_prompt":"Stay in character",
            "post_history_instructions":"Answer briefly"
        }"#,
    )
    .expect("standard aliases");
    assert_eq!(content.first_message, "Hello");
    assert_eq!(
        content.example_dialogs,
        ["User: Hi\nSegu: Welcome"],
        "the standard single string must remain intact"
    );
    assert_eq!(content.system_instruction, "Stay in character");
    assert_eq!(content.post_history_instruction, "Answer briefly");
}

#[test]
fn portable_runtime_capability_wire_values_and_legacy_absence_are_stable() {
    let capabilities = [
        (
            PortableRuntimeCapability::RuntimeCallbacks,
            "runtime:callbacks",
        ),
        (PortableRuntimeCapability::ChatRead, "chat:read"),
        (PortableRuntimeCapability::ChatWrite, "chat:write"),
        (PortableRuntimeCapability::StateReadWrite, "state:readwrite"),
        (PortableRuntimeCapability::ProfileRead, "profile:read"),
        (PortableRuntimeCapability::LoreRead, "lore:read"),
        (PortableRuntimeCapability::UiWrite, "ui:write"),
        (PortableRuntimeCapability::ModelPrimary, "model:primary"),
        (PortableRuntimeCapability::ModelAuxiliary, "model:auxiliary"),
        (PortableRuntimeCapability::Elevated, "elevated"),
    ];
    for (capability, wire) in capabilities {
        assert_eq!(capability.as_str(), wire);
        assert_eq!(
            serde_json::to_value(capability).expect("serialize capability"),
            serde_json::Value::String(wire.to_owned())
        );
        assert_eq!(
            serde_json::from_value::<PortableRuntimeCapability>(serde_json::Value::String(
                wire.to_owned()
            ))
            .expect("deserialize capability"),
            capability
        );
    }

    let legacy: CharacterRuntimeProfile =
        serde_json::from_str("{}").expect("legacy runtime profile");
    assert_eq!(legacy.required_capabilities, None);
    assert!(
        serde_json::to_value(legacy)
            .expect("serialize legacy profile")
            .get("required_capabilities")
            .is_none(),
        "legacy absence must remain omitted from trusted stored wire data"
    );
}

#[test]
fn unknown_extension_index_is_sorted_bounded_and_source_backed() {
    let source = Sha256Digest::parse("cd".repeat(32)).expect("source digest");
    let index = UnknownExtensionIndex::try_new(
        Some(source),
        vec![
            extension_entry("/data/extensions/zeta", 7),
            extension_entry("/data/extensions/alpha", 5),
        ],
    )
    .expect("valid index");

    assert_eq!(index.entry_count, 2);
    assert_eq!(index.total_size_bytes, 12);
    assert_eq!(index.entries[0].source_path, "/data/extensions/alpha");
    assert!(
        UnknownExtensionIndex::try_new(None, vec![extension_entry("/data/extensions/value", 1)])
            .is_err()
    );
    assert!(
        UnknownExtensionIndex::try_new(
            Some(Sha256Digest::parse("ef".repeat(32)).expect("digest")),
            vec![
                extension_entry("/data/extensions/duplicate", 1),
                extension_entry("/data/extensions/duplicate", 1),
            ],
        )
        .is_err()
    );
}

#[test]
fn unknown_extension_wire_data_cannot_bypass_bounds_or_quarantine() {
    let oversized_entries = (0..=MAX_UNKNOWN_EXTENSION_ENTRIES)
        .map(|index| extension_entry(&format!("/data/extensions/{index}"), 1))
        .collect::<Vec<_>>();
    let oversized_json = serde_json::json!({
        "raw_source_sha256": "12".repeat(32),
        "entries": oversized_entries,
    });
    assert!(
        serde_json::from_value::<UnknownExtensionIndex>(oversized_json).is_err(),
        "deserialization must enforce the entry limit"
    );

    let mut active = extension_entry("/data/extensions/script", 1);
    active.quarantine = Some(ExtensionQuarantine {
        kind: ExtensionQuarantineKind::Script,
        reason: "scripts are inert on import".into(),
        active: true,
    });
    let active_json = serde_json::json!({
        "raw_source_sha256": "34".repeat(32),
        "entries": [active],
    });
    assert!(
        serde_json::from_value::<UnknownExtensionIndex>(active_json).is_err(),
        "quarantined extensions cannot become active through serialized input"
    );

    let mismatched_totals = serde_json::json!({
        "raw_source_sha256": "56".repeat(32),
        "entries": [extension_entry("/data/extensions/value", 3)],
        "entry_count": 1,
        "total_size_bytes": 4,
    });
    assert!(serde_json::from_value::<UnknownExtensionIndex>(mismatched_totals).is_err());
}
