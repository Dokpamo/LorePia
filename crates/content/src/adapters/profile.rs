use lorepia_domain::CoreResult;
use serde_json::{Map, Value};

use super::{optional_text, unsupported, validate_metadata_text};

pub(super) fn creator(data: &Map<String, Value>) -> CoreResult<String> {
    let value = optional_text(data, "creator")?;
    validate_metadata_text("data.creator", &value, 1_024, 256)?;
    Ok(value)
}

pub(super) fn recommended_language(data: &Map<String, Value>) -> CoreResult<Option<String>> {
    let value = data
        .get("extensions")
        .and_then(|value| value.get("lorepia"))
        .and_then(|value| value.get("recommended_language"));
    let language = match value {
        None | Some(Value::Null) => return Ok(None),
        Some(Value::String(language)) => language.trim(),
        _ => {
            return Err(unsupported(
                "Recommended language must be a language tag or null",
            ));
        }
    };
    if language.is_empty() {
        return Ok(None);
    }
    let mut parts = language.split('-');
    let primary = parts.next().unwrap_or_default();
    if language.len() > 35
        || !(2..=8).contains(&primary.len())
        || !primary.bytes().all(|byte| byte.is_ascii_alphabetic())
        || !parts.all(|part| {
            (1..=8).contains(&part.len()) && part.bytes().all(|byte| byte.is_ascii_alphanumeric())
        })
    {
        return Err(unsupported(
            "Recommended language must be a bounded language tag",
        ));
    }
    Ok(Some(language.to_ascii_lowercase()))
}

pub(super) fn tags(data: &Map<String, Value>) -> CoreResult<Vec<String>> {
    let values = match data.get("tags") {
        None | Some(Value::Null) => return Ok(Vec::new()),
        Some(Value::Array(values)) if values.len() <= 128 => values,
        _ => {
            return Err(unsupported(
                "CCv3 data.tags must be an array of at most 128 strings",
            ));
        }
    };
    values
        .iter()
        .map(|value| {
            let tag = value
                .as_str()
                .ok_or_else(|| unsupported("CCv3 tags must be strings"))?;
            validate_metadata_text("data.tags entry", tag, 512, 128)?;
            Ok(tag.to_owned())
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use crate::adapters::{NonPortableContentPolicy, parse_card_json_with_source_and_policy};
    use lorepia_domain::CharacterContentV1;
    use serde_json::{Value, json};

    #[test]
    fn standard_profile_survives_safe_import_without_enabling_markup() {
        for spec in ["chara_card_v2", "chara_card_v3"] {
            let bytes = serde_json::to_vec(&json!({"spec": spec, "data": {
                "name": "Synthetic", "creator": "Author", "tags": ["world", "survival"],
                "creator_notes": "<script>inert note</script>", "unsupported": true
            }}))
            .unwrap();
            let card = parse_card_json_with_source_and_policy(
                &bytes,
                &"a".repeat(64),
                NonPortableContentPolicy::Omit,
            )
            .unwrap();
            assert_eq!(card.content.creator, "Author");
            assert_eq!(card.content.creator_notes, "<script>inert note</script>");
            assert_eq!(card.content.tags, ["world", "survival"]);
            assert_eq!(card.unsupported_optional_fields, ["unsupported"]);
            assert!(card.content.unknown_extensions.entries.is_empty());
            assert!(card.content.runtime.scripts.is_empty());
            assert!(card.content.system_instruction.is_empty());
        }
    }

    #[test]
    fn profile_rejects_wrong_types_and_excessive_values() {
        for (key, value) in [
            ("creator", json!(42)),
            ("creator", json!("a".repeat(257))),
            ("creator_notes", json!(false)),
            ("creator_notes", json!("a".repeat(65_537))),
            ("tags", json!("world")),
            ("tags", json!([false])),
            ("tags", json!(vec!["world"; 129])),
            ("tags", json!(["a".repeat(129)])),
        ] {
            let mut data = json!({"name": "Synthetic"});
            data[key] = value;
            let bytes =
                serde_json::to_vec(&json!({"spec": "chara_card_v3", "data": data})).unwrap();
            assert!(
                parse_card_json_with_source_and_policy(
                    &bytes,
                    &"a".repeat(64),
                    NonPortableContentPolicy::Omit
                )
                .is_err(),
                "{key}"
            );
        }
    }

    #[test]
    fn old_content_defaults_empty_without_changing_its_serialized_shape() {
        let content: CharacterContentV1 = serde_json::from_value(json!({})).unwrap();
        let encoded = serde_json::to_value(&content).unwrap();
        for key in ["creator", "creator_notes", "tags", "recommended_language"] {
            assert_eq!(encoded.get(key), None);
        }
        assert_eq!(
            content,
            serde_json::from_value::<CharacterContentV1>(encoded).unwrap()
        );
        let values = json!({"creator":"A", "creator_notes":"N", "tags":["x"]});
        let decoded: CharacterContentV1 = serde_json::from_value(values.clone()).unwrap();
        let roundtrip: Value = serde_json::to_value(decoded).unwrap();
        for key in ["creator", "creator_notes", "tags"] {
            assert_eq!(roundtrip[key], values[key]);
        }
    }

    #[test]
    fn recommendation_is_explicit_bounded_and_survives_both_import_policies() {
        for policy in [
            NonPortableContentPolicy::Omit,
            NonPortableContentPolicy::PreserveForRoundTrip,
        ] {
            let bytes = serde_json::to_vec(&json!({ "spec":"chara_card_v3", "data": {
                "name":"Synthetic", "extensions": { "lorepia": { "recommended_language":"JA-jp" } }
            }}))
            .unwrap();
            let card =
                parse_card_json_with_source_and_policy(&bytes, &"a".repeat(64), policy).unwrap();
            assert_eq!(card.content.recommended_language.as_deref(), Some("ja-jp"));
            assert!(card.content.runtime.scripts.is_empty());
            let encoded = serde_json::to_value(&card.content).unwrap();
            assert_eq!(
                serde_json::from_value::<CharacterContentV1>(encoded).unwrap(),
                card.content
            );
        }
        let prose = json!({ "creator_notes":"Recommended language: Japanese", "language":"ja" });
        assert_eq!(
            super::recommended_language(prose.as_object().unwrap()).unwrap(),
            None
        );
        for language in [json!(null), json!(""), json!("   ")] {
            let data = json!({ "extensions": { "lorepia": { "recommended_language":language } } });
            assert_eq!(
                super::recommended_language(data.as_object().unwrap()).unwrap(),
                None
            );
        }
        for language in [
            json!(42),
            json!(["ko"]),
            json!("j"),
            json!("en--US"),
            json!("ko-KR\n<script>"),
            json!("a".repeat(36)),
        ] {
            let data = json!({ "extensions": { "lorepia": { "recommended_language":language } } });
            assert!(super::recommended_language(data.as_object().unwrap()).is_err());
        }
    }
}
