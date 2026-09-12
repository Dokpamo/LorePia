use super::*;

fn database() -> Connection {
    let connection = Connection::open_in_memory().expect("database");
    connection
        .execute_batch("PRAGMA foreign_keys = ON")
        .expect("foreign keys");
    connection
        .execute_batch(include_str!(
            "../../../migrations/0042_module_plan_documents.sql"
        ))
        .expect("schema");
    connection
}

fn large_json() -> String {
    // Multibyte codepoints cross the byte-oriented part boundary.
    serde_json::json!({"components": (0..12_000).map(|id| {
        serde_json::json!({"id": id, "text": "가나다라마바사".repeat(12)})
    }).collect::<Vec<_>>()})
    .to_string()
}

#[test]
fn legacy_inline_and_chunked_documents_round_trip_without_changing_logical_hashes() {
    let mut connection = database();
    let transaction = connection.transaction().expect("transaction");
    let legacy = r#"{"components":[]}"#;
    assert_eq!(store(&transaction, Kind::Review, legacy).unwrap(), legacy);
    let json = large_json();
    assert!(validate_json_bounds("module review", &json).is_err());
    assert_eq!(expand(&transaction, Kind::Review, &json).unwrap(), json);
    let manifest = store(&transaction, Kind::Review, &json).unwrap();
    assert!(manifest.len() < 256);
    assert_eq!(expand(&transaction, Kind::Review, &manifest).unwrap(), json);
    assert_eq!(store(&transaction, Kind::Review, &json).unwrap(), manifest);
    let expected = sha256_hex(json.as_bytes());
    let metadata: Manifest = serde_json::from_str(&manifest).unwrap();
    assert_eq!(metadata.sha256, expected);
    transaction.commit().unwrap();
    assert_eq!(
        decode::<serde_json::Value>(&connection, Kind::Review, &manifest).unwrap(),
        serde_json::from_str::<serde_json::Value>(&json).unwrap()
    );
    assert!(expand(&connection, Kind::Runtime, &manifest).is_err());
    assert!(
        connection
            .execute(
                "UPDATE module_plan_documents SET part_count = part_count",
                []
            )
            .is_err()
    );
    assert!(
        connection
            .execute("DELETE FROM module_plan_document_parts", [])
            .is_err()
    );
}

#[test]
fn document_parts_and_parent_publication_rollback_together() {
    let mut connection = database();
    connection
        .execute_batch("CREATE TABLE parent (id INTEGER PRIMARY KEY, document TEXT)")
        .unwrap();
    {
        let transaction = connection.transaction().unwrap();
        let manifest = store(&transaction, Kind::Approval, &large_json()).unwrap();
        transaction
            .execute("INSERT INTO parent VALUES (1, ?1)", [&manifest])
            .unwrap();
        assert!(
            transaction
                .execute("INSERT INTO parent VALUES (1, ?1)", [&manifest])
                .is_err()
        );
        // Dropping the caller's transaction must leave neither parent nor chunks.
    }
    for table in [
        "parent",
        "module_plan_documents",
        "module_plan_document_parts",
    ] {
        let count: i64 = connection
            .query_row(&format!("SELECT count(*) FROM {table}"), [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(count, 0);
    }
}

#[test]
fn missing_reordered_and_mutated_parts_fail_closed() {
    for mutation in [
        "DELETE FROM module_plan_document_parts WHERE ordinal = 1",
        "UPDATE module_plan_document_parts SET ordinal = 100 WHERE ordinal = 1",
        "UPDATE module_plan_document_parts SET bytes = zeroblob(length(bytes)) WHERE ordinal = 1",
        "UPDATE module_plan_document_parts SET sha256 = printf('%064d', 0) WHERE ordinal = 1",
        "UPDATE module_plan_documents SET byte_length = byte_length - 1",
    ] {
        let mut connection = database();
        let transaction = connection.transaction().unwrap();
        let manifest = store(&transaction, Kind::Runtime, &large_json()).unwrap();
        transaction.commit().unwrap();
        connection.execute_batch("DROP TRIGGER module_plan_parts_no_update; DROP TRIGGER module_plan_parts_no_delete; DROP TRIGGER module_plan_documents_no_update;").unwrap();
        connection.execute(mutation, []).unwrap();
        assert!(
            expand(&connection, Kind::Runtime, &manifest).is_err(),
            "{mutation}"
        );
        let transaction = connection.transaction().unwrap();
        assert!(
            store(&transaction, Kind::Runtime, &large_json()).is_err(),
            "cannot silently repair corrupt immutable authority"
        );
    }
}

#[test]
fn manifest_version_count_depth_and_aggregate_size_remain_bounded() {
    let mut connection = database();
    let transaction = connection.transaction().unwrap();
    let manifest = store(&transaction, Kind::Review, &large_json()).unwrap();
    for (key, value) in [
        ("part_count", 129),
        ("byte_length", MAX_BYTES + 1),
        ("lorepia_module_document", 2),
    ] {
        let mut value_json: serde_json::Value = serde_json::from_str(&manifest).unwrap();
        value_json[key] = value.into();
        assert!(expand(&transaction, Kind::Review, &value_json.to_string()).is_err());
    }
    assert!(validate(&format!("\"{}\"", "x".repeat(MAX_BYTES))).is_err());
    assert!(validate(&format!("{}0{}", "[".repeat(40), "]".repeat(40))).is_err());
    assert!(validate(r#"{"api_key":"forbidden"}"#).is_err());
}

#[test]
fn validated_inline_document_preserves_original_bytes_and_rejects_invalid_authority() {
    let connection = database();
    let json = " {\"text\": \"한글\\n\", \"value\": 1, \"value\": 2} ";
    let expanded = expand(&connection, Kind::Runtime, json).unwrap();
    assert!(matches!(expanded, Cow::Borrowed(_)));
    assert_eq!(expanded, json);
    for invalid in ["{", r#"{"nested":{"api_key":"secret"}}"#] {
        let error = expand(&connection, Kind::Runtime, invalid).unwrap_err();
        assert_eq!(error.message, "stored module document exceeds limits");
        assert_eq!(error.code, lorepia_domain::CoreErrorCode::StorageCorrupted);
    }
}
