use super::support::*;
use super::*;

fn initial_working_draft(source: Value) -> Value {
    json!({
        "schema_version": 1,
        "source": source,
        "deterministic": null,
        "evidence_ids": [],
        "extra_evidence_ids": [],
        "selected_candidate_id": null,
        "template": null,
        "connection": null,
        "routes": [],
        "observations": [],
        "presets": [],
        "credential_approval_id": null,
        "probe_route_ids": [],
        "probe_failure_count": 0,
        "assistant": null
    })
}

fn initial_sanitized_curl_output() -> Value {
    let extracted = json!({
        "method": "POST",
        "origin": "https://provider.example",
        "source_path_sha256": "1".repeat(64),
        "source_path_is_root": false,
        "query_parameter_names": [],
        "header_names": [],
        "auth_hints": [],
        "body_json_shape": null,
        "stream_hint": null,
        "api_family_candidates": [],
        "trust": "sanitized_curl_structure"
    });
    let content_sha256 = sha256_hex(&serde_json::to_vec(&extracted).expect("sanitized cURL JSON"));
    json!({
        "schema_version": 1,
        "selected_template": null,
        "evidence": [{
            "kind": "sanitized_curl_request",
            "source_origin": "https://provider.example",
            "content_sha256": content_sha256,
            "extracted_json": extracted,
            "redaction_version": 1
        }],
        "family_candidates": [],
        "manifest_candidates": [],
        "connection_hints": [],
        "fetch_issues": [],
        "fetch_stopped_by_budget": false
    })
}

#[test]
fn authority_checked_begin_allows_new_credentialless_discovery() {
    let root = tempdir().expect("temp directory");
    let storage = Storage::open(root.path()).expect("open storage");
    let draft = draft_session("session-new-credentialless-authority-check");
    let begin = apply(&draft, ProviderDiscoveryAction::Begin, 'b');
    let operation_id = DiscoveryOperationId::parse("operation-new-credentialless-authority-check")
        .expect("operation id");

    storage
        .begin_discovery_session_with_credential_authority(
            &draft,
            &write(begin, Some(operation_id), None),
            None,
        )
        .expect("new credentialless discovery needs no prior authority");
    assert_eq!(
        storage
            .get_discovery_session(&draft.id)
            .expect("load begun discovery")
            .session
            .state,
        DiscoveryState::ResolvingKnownProvider
    );
}

#[test]
fn authority_checked_begin_allows_existing_credentialless_connection() {
    let root = tempdir().expect("temp directory");
    let storage = Storage::open(root.path()).expect("open storage");
    let draft = draft_session("session-existing-credentialless-authority-check");
    storage
        .save_provider_profile(&ProviderProfile {
            id: "credentialless-template-seed".to_owned(),
            display_name: "Credentialless template seed".to_owned(),
            base_url: "https://provider.example/v1".to_owned(),
            model: "synthetic".to_owned(),
            timeout_seconds: 30,
        })
        .expect("seed a valid provider template");
    let mut credentialless = storage
        .get_provider_connection(&ProviderConnectionId::from("credentialless-template-seed"))
        .expect("load template seed connection");
    credentialless.id = draft.input.connection_id.clone();
    credentialless.display_name = "Existing credentialless provider".to_owned();
    credentialless.credential_ref = None;
    credentialless.credential_scope = None;
    storage
        .insert_provider_connection(&credentialless)
        .expect("insert existing credentialless connection");

    let begin = apply(&draft, ProviderDiscoveryAction::Begin, 'e');
    let operation_id =
        DiscoveryOperationId::parse("operation-existing-credentialless-authority-check")
            .expect("operation id");
    storage
        .begin_discovery_session_with_credential_authority(
            &draft,
            &write(begin, Some(operation_id), None),
            None,
        )
        .expect("existing credentialless connection needs no authority");
    assert_eq!(
        storage
            .get_discovery_session(&draft.id)
            .expect("load begun discovery")
            .session
            .state,
        DiscoveryState::ResolvingKnownProvider
    );
}

#[test]
fn begin_and_action_receipt_are_idempotently_replayable() {
    let root = tempdir().expect("temp directory");
    let storage = Storage::open(root.path()).expect("open storage");
    let draft = draft_session("session-replay");
    let begin = apply(&draft, ProviderDiscoveryAction::Begin, 'e');
    let operation_id = DiscoveryOperationId::parse("operation-replay").expect("operation id");
    let mut begin_write = write(begin.clone(), Some(operation_id), None);
    begin_write.draft =
        DiscoveryJsonUpdate::Replace(initial_working_draft(json!({"kind": "site"})));
    begin_write.review = DiscoveryJsonUpdate::Clear;
    assert!(matches!(
        storage
            .begin_discovery_session(&draft, &begin_write)
            .expect("persist begin"),
        PersistDiscoveryTransition::Applied { .. }
    ));
    let replay = storage
        .find_discovery_action_replay(
            &draft.id,
            &begin.receipt.action_id,
            &begin.receipt.request_sha256,
            &begin.receipt.action_kind,
        )
        .expect("find replay")
        .expect("stored replay");
    assert_eq!(replay.transition, begin);
    assert_eq!(
        storage
            .get_discovery_session(&draft.id)
            .expect("load begun session")
            .draft_json,
        match &begin_write.draft {
            DiscoveryJsonUpdate::Replace(value) => Some(value.clone()),
            _ => None,
        }
    );
    assert!(matches!(
        storage
            .begin_discovery_session(&draft, &begin_write)
            .expect("replay begin"),
        PersistDiscoveryTransition::Replayed { .. }
    ));
    assert!(
        storage
            .find_discovery_action_replay(
                &draft.id,
                &begin_write.transition.receipt.action_id,
                &"f".repeat(64),
                &begin_write.transition.receipt.action_kind,
            )
            .is_err()
    );
}

#[test]
fn begin_rejects_forged_commit_metadata_on_initial_or_resulting_session() {
    let root = tempdir().expect("temp directory");
    let storage = Storage::open(root.path()).expect("open storage");

    let mut forged_initial = draft_session("session-forged-initial");
    forged_initial.commit_attempt_id =
        Some(DiscoveryCommitAttemptId::parse("foreign-attempt").expect("attempt id"));
    forged_initial.commit_plan_sha256 = Some("4".repeat(64));
    let begin = apply(&forged_initial, ProviderDiscoveryAction::Begin, '5');
    let error = storage
        .begin_discovery_session(
            &forged_initial,
            &write(
                begin,
                Some(
                    DiscoveryOperationId::parse("operation-forged-initial").expect("operation id"),
                ),
                None,
            ),
        )
        .expect_err("begin must reject a non-pristine initial session");
    assert_eq!(error.code, CoreErrorCode::InvalidInput);
    assert_eq!(
        storage
            .get_discovery_session(&forged_initial.id)
            .expect_err("rejected begin must not create a session")
            .code,
        CoreErrorCode::NotFound
    );

    let pristine = draft_session("session-forged-result");
    let mut begin = apply(&pristine, ProviderDiscoveryAction::Begin, '6');
    begin.session.commit_attempt_id =
        Some(DiscoveryCommitAttemptId::parse("foreign-result-attempt").expect("attempt id"));
    begin.session.commit_plan_sha256 = Some("7".repeat(64));
    let error = storage
        .begin_discovery_session(
            &pristine,
            &write(
                begin,
                Some(DiscoveryOperationId::parse("operation-forged-result").expect("operation id")),
                None,
            ),
        )
        .expect_err("begin must reject forged resulting session metadata");
    assert_eq!(error.code, CoreErrorCode::InvalidInput);
    assert_eq!(
        storage
            .get_discovery_session(&pristine.id)
            .expect_err("rejected begin must remain atomic")
            .code,
        CoreErrorCode::NotFound
    );

    let raw_curl = draft_session("session-raw-curl-draft");
    let mut raw_curl_write = write(
        apply(&raw_curl, ProviderDiscoveryAction::Begin, '8'),
        Some(DiscoveryOperationId::parse("operation-raw-curl-draft").expect("operation id")),
        None,
    );
    raw_curl_write.draft = DiscoveryJsonUpdate::Replace(initial_working_draft(json!({
        "kind": "curl",
        "raw_curl": "curl https://provider.example/v1/models"
    })));
    raw_curl_write.review = DiscoveryJsonUpdate::Clear;
    let error = storage
        .begin_discovery_session(&raw_curl, &raw_curl_write)
        .expect_err("raw cURL command text must not enter the durable initial draft");
    assert_eq!(error.code, CoreErrorCode::InvalidInput);
    assert_eq!(
        storage
            .get_discovery_session(&raw_curl.id)
            .expect_err("rejected raw cURL begin must remain atomic")
            .code,
        CoreErrorCode::NotFound
    );
}

#[test]
fn begin_accepts_only_canonical_sanitized_curl_output() {
    let root = tempdir().expect("temp directory");
    let storage = Storage::open(root.path()).expect("open storage");
    let draft = draft_session("session-sanitized-curl-draft");
    let mut working_draft = initial_working_draft(json!({"kind": "curl"}));
    working_draft["deterministic"] = initial_sanitized_curl_output();
    let mut begin_write = write(
        apply(&draft, ProviderDiscoveryAction::Begin, '9'),
        Some(DiscoveryOperationId::parse("operation-sanitized-curl-draft").expect("operation id")),
        None,
    );
    begin_write.draft = DiscoveryJsonUpdate::Replace(working_draft);
    begin_write.review = DiscoveryJsonUpdate::Clear;
    storage
        .begin_discovery_session(&draft, &begin_write)
        .expect("persist canonical sanitized cURL output");

    let forged = draft_session("session-forged-curl-output");
    let mut forged_output = initial_sanitized_curl_output();
    forged_output["evidence"][0]["extracted_json"]["raw_curl"] =
        Value::String("curl https://provider.example/v1/models".to_owned());
    let mut forged_draft = initial_working_draft(json!({"kind": "curl"}));
    forged_draft["deterministic"] = forged_output;
    let mut forged_write = write(
        apply(&forged, ProviderDiscoveryAction::Begin, 'a'),
        Some(DiscoveryOperationId::parse("operation-forged-curl-output").expect("operation id")),
        None,
    );
    forged_write.draft = DiscoveryJsonUpdate::Replace(forged_draft);
    forged_write.review = DiscoveryJsonUpdate::Clear;
    let error = storage
        .begin_discovery_session(&forged, &forged_write)
        .expect_err("non-canonical cURL payload must not enter durable state");
    assert_eq!(error.code, CoreErrorCode::InvalidInput);
    assert_eq!(
        storage
            .get_discovery_session(&forged.id)
            .expect_err("rejected cURL output must remain atomic")
            .code,
        CoreErrorCode::NotFound
    );
}
