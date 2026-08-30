use super::support::*;
use super::*;

#[test]
fn assistant_evidence_boundary_actions_complete_the_billable_operation() {
    let root = tempdir().expect("temp directory");
    let storage = Storage::open(root.path()).expect("open storage");
    let draft = draft_session("session-assistant-more-evidence");
    storage
        .create_discovery_session(&draft, now())
        .expect("create draft");
    let operation_id =
        DiscoveryOperationId::parse("operation-assistant-more-evidence").expect("operation id");
    let mut connection = storage.connection().expect("database connection");
    let operation_guard = suspend_test_trigger(
        &connection,
        "provider_discovery_operation_initial_state_guard",
    );
    let transaction = connection.transaction().expect("transaction");
    transaction
        .execute(
            "INSERT INTO provider_discovery_operations (
                 id, session_id, operation_kind, side_effect_class, status,
                 action_id, expected_revision, request_sha256, approval_id,
                 approval_grant_sha256, started_at, finished_at, created_at, updated_at
             ) VALUES (
                 ?1, ?2, 'build_assistant_manifest_draft', 'billable_external', 'started',
                 'action-assistant-more-evidence', 0, ?3, NULL, NULL,
                 ?4, NULL, ?4, ?4
             )",
            rusqlite::params![
                operation_id.as_str(),
                draft.id.as_str(),
                "d".repeat(64),
                now().to_rfc3339(),
            ],
        )
        .expect("insert started assistant operation");
    restore_test_trigger(&transaction, &operation_guard);
    transaction
        .execute(
            "UPDATE provider_discovery_sessions
             SET state = 'building_assistant_manifest_draft',
                 revision = 1,
                 next_event_sequence = 2,
                 active_operation_id = ?2,
                 updated_at = ?3
             WHERE id = ?1",
            rusqlite::params![draft.id.as_str(), operation_id.as_str(), now().to_rfc3339(),],
        )
        .expect("activate assistant operation");

    let mut completion = write(
        apply(&draft, ProviderDiscoveryAction::Begin, 'd'),
        None,
        Some(DiscoveryCompletedOperationWrite {
            id: operation_id,
            outcome: super::super::DurableOperationOutcome::Succeeded,
        }),
    );
    completion.transition.receipt.action_kind = "assistant_requested_more_evidence".to_owned();
    super::super::validate_completed_operation_binding(&transaction, &completion)
        .expect("more-evidence action must complete the billable operation");

    completion.transition.receipt.action_kind = "assistant_resumed_with_evidence".to_owned();
    super::super::validate_completed_operation_binding(&transaction, &completion)
        .expect("resumed-with-evidence action must complete the billable operation");
}

#[test]
fn evidence_rejects_known_credential_markers_without_blocking_model_ids() {
    let root = tempdir().expect("temp directory");
    let storage = Storage::open(root.path()).expect("open storage");
    let draft = draft_session("session-evidence");
    storage
        .create_discovery_session(&draft, now())
        .expect("create draft");
    let query_url = DiscoveryEvidenceRecord {
        id: "evidence-query".into(),
        session_id: draft.id.clone(),
        kind: DiscoveryEvidenceKind::JsonDocument,
        source_url: HttpUrl::parse("https://provider.example/docs?token=secret")
            .expect("URL parser permits query"),
        content_sha256: "a".repeat(64),
        extracted_json: json!({"endpoint": "/v1/models"}),
        fetched_at: now(),
    };
    assert!(storage.save_discovery_evidence(&query_url).is_err());

    let sensitive = DiscoveryEvidenceRecord {
        id: "evidence-sensitive".into(),
        source_url: HttpUrl::parse("https://provider.example/docs").expect("source URL"),
        extracted_json: json!({"example_value": "sk-proj-must-not-persist"}),
        ..query_url
    };
    assert!(storage.save_discovery_evidence(&sensitive).is_err());
    let legitimate_model_id = "Qwen/Qwen2.5-Coder-32B-Instruct";
    let model_evidence = DiscoveryEvidenceRecord {
        id: "evidence-model-identifier".into(),
        extracted_json: json!({"model_id": legitimate_model_id}),
        ..sensitive.clone()
    };
    storage
        .save_discovery_evidence(&model_evidence)
        .expect("ordinary mixed-case model identifiers must remain persistable");
    assert!(!super::super::looks_like_secret(legitimate_model_id));
    assert!(!super::super::looks_like_secret(
        "provider.parameter.temperature"
    ));

    let known_secret = "sk-proj-must-not-persist-in-path";
    let secret_path = DiscoveryEvidenceRecord {
        id: "evidence-secret-path".into(),
        source_url: HttpUrl::parse(&format!("https://provider.example/docs/{known_secret}"))
            .expect("URL parser permits path material"),
        extracted_json: json!({"endpoint": "/v1/models"}),
        ..sensitive
    };
    assert!(storage.save_discovery_evidence(&secret_path).is_err());
    let mut unsafe_label = draft_session("session-secret-label");
    unsafe_label.input.display_name = known_secret.to_owned();
    assert!(
        storage
            .create_discovery_session(&unsafe_label, now())
            .is_err()
    );
    let mut unsafe_connection_id = draft_session("session-secret-connection-id");
    unsafe_connection_id.input.connection_id = ProviderConnectionId::from(known_secret);
    assert!(
        storage
            .create_discovery_session(&unsafe_connection_id, now())
            .is_err()
    );

    let mut unsafe_draft = draft_session("session-secret-ref");
    unsafe_draft.input.connection_id = ProviderConnectionId::from(known_secret);
    unsafe_draft.input.credential_ref = Some(CredentialRef(known_secret.to_owned()));
    assert!(
        storage
            .create_discovery_session(&unsafe_draft, now())
            .is_err()
    );
}
