use super::*;

fn assert_unsupported(bytes: &[u8]) {
    let error = parse_card_json(bytes).expect_err("metadata must be rejected");
    assert_eq!(error.code, CoreErrorCode::UnsupportedContent);
    assert!(!error.recoverable);
}

#[test]
fn requires_an_object_with_v3_spec_data_and_name() {
    for bytes in [
        b"".as_slice(),
        b"null".as_slice(),
        br"[]".as_slice(),
        br"{}".as_slice(),
        br#"{"spec":3,"data":{"name":"Segu"}}"#.as_slice(),
        br#"{"spec":"chara_card_v3"}"#.as_slice(),
        br#"{"spec":"chara_card_v3","data":[]}"#.as_slice(),
        br#"{"spec":"chara_card_v3","data":{}}"#.as_slice(),
        br#"{"spec":"chara_card_v3","data":{"name":3}}"#.as_slice(),
        br#"{"spec":"chara_card_v3","data":{"name":"  "}}"#.as_slice(),
    ] {
        assert_unsupported(bytes);
    }
}

#[test]
fn parses_required_and_optional_fields() {
    let metadata = parse_card_json(
        br#"{"spec":"chara_card_v3","data":{"name":" Segu ","description":" Guide "}}"#,
    )
    .expect("valid CCv3 metadata");

    assert_eq!(metadata.name, "Segu");
    assert_eq!(metadata.description, "Guide");
    assert!(metadata.unsupported_optional_fields.is_empty());
}

#[test]
fn reports_sorted_unsupported_fields_and_excludes_only_consumed_text() {
    let description = parse_card_json(
        br#"{
            "spec":"chara_card_v3",
            "data":{
                "name":"Segu",
                "description":"Guide",
                "personality":"Unused fallback",
                "z_unknown":true,
                "alternate_greetings":[],
                "creator":"Synthetic"
            }
        }"#,
    )
    .expect("valid CCv3 metadata");
    assert_eq!(description.unsupported_optional_fields, ["z_unknown"]);

    let fallback = parse_card_json(
        br#"{
            "spec":"chara_card_v3",
            "data":{
                "name":"Segu",
                "description":null,
                "personality":"Fallback",
                "scenario":"Synthetic"
            }
        }"#,
    )
    .expect("valid fallback metadata");
    assert_eq!(fallback.description, "Fallback");
    assert!(fallback.unsupported_optional_fields.is_empty());

    let duplicate = parse_card_json(
        br#"{
            "spec":"chara_card_v3",
            "data":{
                "name":"Segu",
                "creator":"First",
                "creator":"Last"
            }
        }"#,
    )
    .expect("duplicate optional keys remain bounded");
    assert!(duplicate.unsupported_optional_fields.is_empty());
    assert_eq!(duplicate.content.creator, "Last");
}

#[test]
fn bounds_optional_field_names_and_count() {
    let oversized_key = "k".repeat(MAX_OPTIONAL_FIELD_KEY_BYTES + 1);
    let oversized = serde_json::json!({
        "spec": CHARACTER_CARD_V3_SPEC,
        "data": {
            "name": "Segu",
            oversized_key: true,
        }
    });
    assert_unsupported(&serde_json::to_vec(&oversized).expect("encode"));

    let mut data = serde_json::Map::new();
    data.insert("name".to_owned(), Value::String("Segu".to_owned()));
    for index in 0..=MAX_UNSUPPORTED_OPTIONAL_FIELDS {
        data.insert(format!("optional_{index:03}"), Value::Bool(true));
    }
    let too_many = serde_json::json!({
        "spec": CHARACTER_CARD_V3_SPEC,
        "data": data,
    });
    assert_unsupported(&serde_json::to_vec(&too_many).expect("encode"));

    let control_key = serde_json::json!({
        "spec": CHARACTER_CARD_V3_SPEC,
        "data": {
            "name": "Segu",
            "unsafe\nlabel": true,
        }
    });
    assert_unsupported(&serde_json::to_vec(&control_key).expect("encode"));
}

#[test]
fn metadata_limits_are_inclusive_at_multibyte_utf8_boundaries() {
    let name = "😀".repeat(MAX_CHARACTER_NAME_CHARS);
    assert_eq!(name.len(), MAX_CHARACTER_NAME_BYTES);
    let description = "😀".repeat(MAX_CHARACTER_DESCRIPTION_CHARS);
    assert_eq!(description.len(), MAX_CHARACTER_DESCRIPTION_BYTES);
    let json = serde_json::json!({
        "spec": CHARACTER_CARD_V3_SPEC,
        "data": {
            "name": name,
            "description": description,
        }
    });

    let metadata =
        parse_card_json(&serde_json::to_vec(&json).expect("encode")).expect("exact limits");
    assert_eq!(metadata.name.chars().count(), MAX_CHARACTER_NAME_CHARS);
    assert_eq!(
        metadata.description.chars().count(),
        MAX_CHARACTER_DESCRIPTION_CHARS
    );
}

#[test]
fn metadata_limits_reject_one_complete_multibyte_scalar_over_the_boundary() {
    let oversized_name = "😀".repeat(MAX_CHARACTER_NAME_CHARS + 1);
    let name_json = serde_json::json!({
        "spec": CHARACTER_CARD_V3_SPEC,
        "data": {"name": oversized_name}
    });
    let name_error =
        parse_card_json(&serde_json::to_vec(&name_json).expect("encode")).expect_err("name");
    assert_eq!(name_error.code, CoreErrorCode::UnsupportedContent);
    assert_eq!(
        name_error.message,
        "CCv3 data.name exceeds the 1024-byte or 256-character limit"
    );

    let oversized_description = "😀".repeat(MAX_CHARACTER_DESCRIPTION_CHARS + 1);
    let description_json = serde_json::json!({
        "spec": CHARACTER_CARD_V3_SPEC,
        "data": {
            "name": "Segu",
            "description": oversized_description,
        }
    });
    let description_error =
        parse_card_json(&serde_json::to_vec(&description_json).expect("encode"))
            .expect_err("description");
    assert_eq!(description_error.code, CoreErrorCode::UnsupportedContent);
    assert_eq!(
        description_error.message,
        "CCv3 data.description exceeds the 262144-byte or 65536-character limit"
    );
}

#[test]
fn card_runtime_capabilities_normalize_legacy_and_reject_null_or_implicit_elevation() {
    let card = |runtime: Value| {
        serde_json::to_vec(&serde_json::json!({
            "spec": CHARACTER_CARD_V3_SPEC,
            "data": {
                "name": "Segu",
                "extensions": { "runtime": runtime }
            }
        }))
        .expect("encode card")
    };

    let legacy = parse_card_json(&card(serde_json::json!({
        "virtualscript": "return true"
    })))
    .expect("safe legacy runtime");
    assert_eq!(
        legacy.content.runtime.required_capabilities,
        Some(vec![
            lorepia_domain::PortableRuntimeCapability::RuntimeCallbacks,
            lorepia_domain::PortableRuntimeCapability::UiWrite,
        ])
    );

    for field in ["requiredCapabilities", "required_capabilities"] {
        assert_unsupported(&card(serde_json::json!({
            "virtualscript": "return true",
            (field): null
        })));
    }
    assert_unsupported(&card(serde_json::json!({
        "virtualscript": "return true",
        "lowLevelAccess": true
    })));

    let declared = parse_card_json(&card(serde_json::json!({
        "virtualscript": "return true",
        "lowLevelAccess": true,
        "required_capabilities": ["runtime:callbacks", "elevated"]
    })))
    .expect("explicit elevated runtime");
    assert_eq!(
        declared.content.runtime.required_capabilities,
        Some(vec![
            lorepia_domain::PortableRuntimeCapability::RuntimeCallbacks,
            lorepia_domain::PortableRuntimeCapability::Elevated,
        ])
    );
}
