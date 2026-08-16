use lorepia_domain::TemplateSlot;
use lorepia_storage::PromptPresetBinding;
use sha2::{Digest, Sha256};

const LEGACY_BINDING_JSON: &str = r#"{"id":"legacy-binding","prompt_preset_id":"legacy-preset","scope":"branch","target_id":"legacy-branch","conversation_id":"legacy-conversation","pinned_revision_id":null,"priority":0,"enabled":true,"response_length":"balanced","creativity":50,"reasoning_effort":null,"memory_enabled":true,"knowledge_enabled":true,"variable_overrides":{"values":[]},"generation_preset_override_id":null,"created_at":"2026-08-09T00:00:00Z","updated_at":"2026-08-09T00:00:00Z"}"#;

#[test]
fn legacy_binding_round_trip_preserves_canonical_bytes_and_hash() {
    let binding: PromptPresetBinding =
        serde_json::from_str(LEGACY_BINDING_JSON).expect("decode legacy prompt binding");
    assert_eq!(binding.user_name_override, None);
    assert_eq!(binding.author_note, None);
    assert_eq!(binding.group_context, None);
    assert!(binding.template_slots.is_empty());

    let reencoded = serde_json::to_string(&binding).expect("re-encode legacy prompt binding");
    assert_eq!(reencoded, LEGACY_BINDING_JSON);
    assert_eq!(
        binding
            .canonical_document_sha256()
            .expect("hash legacy binding"),
        hex::encode(Sha256::digest(LEGACY_BINDING_JSON.as_bytes()))
    );
}

#[test]
fn binding_context_fields_are_bounded_unique_and_reserve_block_content() {
    let mut binding: PromptPresetBinding =
        serde_json::from_str(LEGACY_BINDING_JSON).expect("decode legacy prompt binding");
    binding.author_note = Some("Synthetic author note".to_owned());
    binding.group_context = Some("Synthetic group context".to_owned());
    binding.user_name_override = Some("Synthetic User".to_owned());
    binding.template_slots = vec![TemplateSlot {
        name: "scene_tone".to_owned(),
        value: "quiet".to_owned(),
    }];
    binding
        .canonical_document_sha256()
        .expect("valid source binding");

    binding.template_slots.push(TemplateSlot {
        name: "scene_tone".to_owned(),
        value: "loud".to_owned(),
    });
    assert!(binding.canonical_document_sha256().is_err());
    binding.template_slots[1].name = "block_content".to_owned();
    assert!(binding.canonical_document_sha256().is_err());
    binding.template_slots.truncate(1);
    binding.author_note = Some("\0".to_owned());
    assert!(binding.canonical_document_sha256().is_err());
}
