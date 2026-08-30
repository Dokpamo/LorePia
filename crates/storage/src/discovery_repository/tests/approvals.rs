use super::support::*;
use super::*;

#[test]
fn lan_session_authority_requires_an_active_immutable_creation_time_binding() {
    let mut input = draft_session("lan-authority-binding").input;
    input.connection_options.network_mode = ProviderNetworkMode::ApprovedLocalNetwork;
    input.connection_options.local_network_approval = Some(ProviderLocalNetworkApproval {
        origin: CanonicalOrigin::parse("http://models.lan:8080").unwrap(),
        addresses: vec!["192.168.10.20".parse().unwrap()],
    });
    let approved_at = now();

    assert!(
        validate_discovery_local_network_approval_binding(&input, approved_at, approved_at)
            .is_err(),
        "timestamp-less legacy LAN session must restart"
    );
    input
        .connection_options
        .issue_local_network_approval_at(approved_at)
        .unwrap();
    validate_discovery_local_network_approval_binding(&input, approved_at, approved_at).unwrap();
    assert!(
        validate_discovery_local_network_approval_binding(
            &input,
            approved_at + chrono::Duration::seconds(1),
            approved_at + chrono::Duration::seconds(1),
        )
        .is_err(),
        "session creation time mismatch must fail closed"
    );
    assert!(
        validate_discovery_local_network_approval_binding(
            &input,
            approved_at,
            approved_at + chrono::Duration::hours(25),
        )
        .is_err(),
        "expired session authority must fail closed"
    );
}

#[test]
fn confirmed_commit_completion_projection_fails_closed_after_approval_tamper() {
    let fixture = seed_completed_discovery_authority_with_mode(
        "tampered-confirmed-completion",
        CompletedDiscoveryAuthorityMode::PendingReconciled,
    );
    let approval_id = persist_pending_confirmed_commit_completion(
        &fixture,
        "tampered-confirmed-completion-action",
        "tampered-confirmed-completion-approval",
    );
    fixture
        .storage
        .ensure_provider_credential_access_settled(&fixture.connection_id)
        .expect("intact confirmed-completion authority is valid");

    let connection = fixture.storage.connection().expect("tamper database");
    let intact_error = connection
        .execute(
            "UPDATE provider_discovery_approvals SET grant_json = '{}' WHERE id = ?1",
            [approval_id.as_str()],
        )
        .expect_err("approval immutability blocks confirmed-completion tampering");
    assert!(
        intact_error
            .to_string()
            .contains("discovery approvals are immutable")
    );
    let approval_guard = suspend_test_trigger(&connection, "provider_discovery_approval_no_update");
    let forged_grant = DiscoveryApprovalGrant::UnknownOutcomeResolution {
        operation: DiscoveryOperationKind::AtomicCommit,
        resolution: DiscoveryUnknownOutcomeResolution::ConfirmedNoEffect,
    };
    let forged_json = encode_approval_grant(&forged_grant).expect("forged approval JSON");
    let forged_sha256 = sha256_hex(forged_json.as_bytes());
    connection
        .execute(
            "UPDATE provider_discovery_approvals
             SET grant_json = ?2, grant_sha256 = ?3
             WHERE id = ?1",
            rusqlite::params![approval_id.as_str(), forged_json, forged_sha256],
        )
        .expect("inject synthetic approval-history corruption");
    restore_test_trigger(&connection, &approval_guard);
    drop(connection);

    let error = fixture
        .storage
        .ensure_provider_credential_access_settled(&fixture.connection_id)
        .expect_err("tampered resolution approval must revoke settled authority");
    assert_eq!(error.code, CoreErrorCode::StorageCorrupted);

    let root_path = fixture.root.path().to_path_buf();
    let connection_id = fixture.connection_id.clone();
    drop(fixture.storage);
    let reopened = Storage::open(&root_path).expect("reopen tampered authority database");
    let reopened_error = reopened
        .ensure_provider_credential_access_settled(&connection_id)
        .expect_err("reopen must not trust tampered resolution approval");
    assert_eq!(reopened_error.code, CoreErrorCode::StorageCorrupted);
}

#[test]
fn replaced_approval_receipt_event_and_evidence_fail_discovery_authority_closed() {
    let approval = seed_completed_discovery_authority("discovery-authority-approval-replace");
    assert_discovery_authority_history_replace_is_guarded_and_revalidated(
        &approval,
        "provider_discovery_approval_no_replace",
        "INSERT OR REPLACE INTO provider_discovery_approvals (
             id, session_id, approval_kind, candidate_id, decision,
             grant_json, session_revision, grant_sha256, redaction_version, created_at
         )
         SELECT id, session_id, approval_kind, candidate_id, decision,
                grant_json, session_revision, ?2, redaction_version, created_at
         FROM provider_discovery_approvals
         WHERE session_id = ?1 AND approval_kind = 'review'",
        approval.session_id.as_str(),
        &"f".repeat(64),
        false,
    );

    let receipt = seed_completed_discovery_authority("discovery-authority-receipt-replace");
    assert_discovery_authority_history_replace_is_guarded_and_revalidated(
        &receipt,
        "provider_discovery_receipt_no_replace",
        "INSERT OR REPLACE INTO provider_discovery_action_receipts (
             action_id, session_id, action_kind, request_sha256,
             expected_revision, resulting_revision, event_id,
             event_sequence, outcome, response_json, redaction_version, created_at
         )
         SELECT action_id, session_id, action_kind, ?2,
                expected_revision, resulting_revision, event_id,
                event_sequence, outcome, response_json, redaction_version, created_at
         FROM provider_discovery_action_receipts
         WHERE session_id = ?1 AND action_kind = 'commit_succeeded'",
        receipt.session_id.as_str(),
        &"f".repeat(64),
        false,
    );

    let event = seed_completed_discovery_authority("discovery-authority-outbox-replace");
    assert_discovery_authority_history_replace_is_guarded_and_revalidated(
        &event,
        "provider_discovery_outbox_no_replace",
        "INSERT OR REPLACE INTO provider_discovery_event_outbox (
             id, session_id, sequence, event_version, session_revision,
             state, event_json, redaction_version, delivery_attempts,
             available_at, delivered_at, created_at
         )
         SELECT id, session_id, sequence, event_version, session_revision,
                ?2, event_json, redaction_version, delivery_attempts,
                available_at, delivered_at, created_at
         FROM provider_discovery_event_outbox
         WHERE session_id = ?1 AND state = 'ready'",
        event.session_id.as_str(),
        "failed",
        true,
    );

    let evidence = seed_completed_discovery_authority("discovery-authority-evidence-replace");
    assert_discovery_authority_history_replace_is_guarded_and_revalidated(
        &evidence,
        "provider_discovery_evidence_no_replace",
        "INSERT OR REPLACE INTO provider_discovery_evidence (
             id, session_id, kind, source_url, content_sha256,
             extracted_json, redaction_version, fetched_at
         )
         SELECT id, session_id, ?2, source_url, content_sha256,
                extracted_json, redaction_version, fetched_at
         FROM provider_discovery_evidence WHERE id = ?1",
        evidence.evidence_id.as_str(),
        "forged_kind",
        false,
    );
}

#[test]
fn native_commit_start_requires_immutable_credential_approval() {
    let root = tempdir().expect("temp directory");
    let storage = Storage::open(root.path()).expect("open storage");
    let fixture =
        seed_started_native_credential_commit(&storage, "native-start-approval-authority");
    let credential_approval_id = storage
        .get_discovery_commit_attempt(&fixture.attempt_id)
        .expect("load credential commit attempt")
        .plan
        .credential_approval_id
        .expect("credential approval id");
    storage
        .validate_discovery_credential_install_operation_authority(
            &fixture.session.id,
            &fixture.attempt_id,
            &fixture.plan_sha256,
            &fixture.operation_id,
        )
        .expect("exact credential approvals grant native install authority");
    let (write, attestation) = native_no_effect_completion(&storage, &fixture, &fixture.session);

    let database = storage.connection().expect("database connection");
    let delete_approval = || {
        database.execute(
            "DELETE FROM provider_discovery_approvals WHERE id = ?1",
            [credential_approval_id.as_str()],
        )
    };
    let trigger_error =
        delete_approval().expect_err("immutable approval trigger must preserve native authority");
    assert!(
        trigger_error.to_string().contains("immutable"),
        "unexpected approval deletion rejection: {trigger_error}"
    );
    database
        .execute_batch("DROP TRIGGER provider_discovery_approval_no_delete;")
        .expect("drop approval deletion guard only for corruption fixture");
    delete_approval().expect("remove credential approval after bypassing history guard");
    assert_native_attestation_and_terminal_schema_rejected(
        &database,
        &attestation,
        write.occurred_at,
    );
    drop(database);

    let persist_error = storage
        .persist_native_no_effect_discovery_transition(&write, &attestation)
        .expect_err("public writer must reject detached credential approval");
    assert_eq!(persist_error.code, CoreErrorCode::StorageCorrupted);
    let error = storage
        .validate_discovery_credential_install_operation_authority(
            &fixture.session.id,
            &fixture.attempt_id,
            &fixture.plan_sha256,
            &fixture.operation_id,
        )
        .expect_err("detached credential approval must fail closed");
    assert_eq!(error.code, CoreErrorCode::StorageCorrupted);
}

#[test]
fn native_commit_start_rejects_malformed_credential_approval_root() {
    for corruption in [
        "credential_grant_digest",
        "credential_origin",
        "credential_header",
        "review_grant_digest",
        "plan_digest",
    ] {
        assert_malformed_native_commit_root_rejected(corruption);
    }
}

#[test]
fn reconciled_retry_requires_immutable_no_effect_approval() {
    let root = tempdir().expect("temp directory");
    let storage = Storage::open(root.path()).expect("open storage");
    let fixture =
        seed_started_native_credential_commit(&storage, "native-retry-resolution-authority");
    let retry = restart_unknown_native_credential_commit(
        &storage,
        &fixture,
        DiscoveryOperationId::parse("operation-native-retry-resolution-authority-retry")
            .expect("retry operation id"),
    );
    storage
        .validate_discovery_credential_install_operation_authority(
            &fixture.session.id,
            &fixture.attempt_id,
            &fixture.plan_sha256,
            &retry.operation_id,
        )
        .expect("exact no-effect approval grants retry authority");
    let approval_id = storage
        .connection()
        .expect("database connection")
        .query_row(
            "SELECT id FROM provider_discovery_approvals
             WHERE session_id = ?1 AND approval_kind = 'unknown_outcome_resolution'",
            [fixture.session.id.as_str()],
            |row| row.get::<_, String>(0),
        )
        .expect("load no-effect approval id");
    let interrupted = apply(
        &retry.session,
        ProviderDiscoveryAction::Interrupt {
            operation: DiscoveryOperationKind::AtomicCommit,
            outcome: DiscoveryInterruptionOutcome::ConfirmedNoExternalEffect,
        },
        '8',
    );
    let attested_at = now() + chrono::Duration::milliseconds(6);
    let mut write = write(
        interrupted,
        None,
        Some(DiscoveryCompletedOperationWrite {
            id: retry.operation_id.clone(),
            outcome: DurableOperationOutcome::AttestedNoExternalEffect,
        }),
    );
    write.occurred_at = attested_at;
    let attestation = DiscoveryNativeNoEffectAttestationWrite::credential_slot_missing(
        retry.operation_id.clone(),
        test_native_physical_authority_id(&storage, &retry.operation_id),
        fixture.session.id.clone(),
        fixture.attempt_id.clone(),
        fixture.plan_sha256.clone(),
        fixture.session.input.connection_id.clone(),
    )
    .expect("retry native attestation");

    let database = storage.connection().expect("database connection");
    database
        .execute_batch("DROP TRIGGER provider_discovery_approval_no_delete;")
        .expect("drop approval deletion guard only for corruption fixture");
    database
        .execute(
            "DELETE FROM provider_discovery_approvals WHERE id = ?1",
            [approval_id],
        )
        .expect("remove no-effect approval after bypassing history guard");
    assert_native_attestation_and_terminal_schema_rejected(&database, &attestation, attested_at);
    drop(database);

    let persist_error = storage
        .persist_native_no_effect_discovery_transition(&write, &attestation)
        .expect_err("public writer must reject detached no-effect approval");
    assert_eq!(persist_error.code, CoreErrorCode::StorageCorrupted);
    let error = storage
        .validate_discovery_credential_install_operation_authority(
            &fixture.session.id,
            &fixture.attempt_id,
            &fixture.plan_sha256,
            &retry.operation_id,
        )
        .expect_err("detached no-effect approval must fail closed");
    assert_eq!(error.code, CoreErrorCode::StorageCorrupted);
}
