use super::*;

pub(in crate::discovery_repository::tests) fn seed_started_native_credential_commit(
    storage: &Storage,
    id: &str,
) -> NativeNoEffectFixture {
    let fixture = seed_native_credential_commit(storage, id);
    reserve_and_start_test_native_execution(
        storage,
        &fixture.session,
        &fixture.attempt_id,
        &fixture.plan_sha256,
        &fixture.operation_id,
        now() + chrono::Duration::milliseconds(1),
    );
    fixture
}

pub(in crate::discovery_repository::tests) fn seed_prepared_native_credential_commit(
    storage: &Storage,
    id: &str,
) -> NativeNoEffectFixture {
    seed_native_credential_commit(storage, id)
}

pub(in crate::discovery_repository::tests) fn reserve_and_start_test_native_execution(
    storage: &Storage,
    session: &ProviderDiscoverySession,
    attempt_id: &DiscoveryCommitAttemptId,
    plan_sha256: &str,
    operation_id: &DiscoveryOperationId,
    started_at: chrono::DateTime<Utc>,
) -> super::super::super::DiscoveryNativeCredentialExecutionRecord {
    let reserved = storage
        .reserve_discovery_credential_install_execution(
            &super::super::super::DiscoveryNativeCredentialExecutionReservation {
                operation_id: operation_id.clone(),
                session_id: session.id.clone(),
                commit_attempt_id: attempt_id.clone(),
                commit_plan_sha256: plan_sha256.to_owned(),
                connection_id: session.input.connection_id.clone(),
                connection_binding_sha256: "b".repeat(64),
                reserved_at: started_at,
            },
        )
        .expect("reserve test native credential execution");
    storage
        .start_reserved_discovery_credential_install_execution(
            &super::super::super::DiscoveryNativeCredentialStoreAttemptStart {
                operation_id: operation_id.clone(),
                physical_authority_id: reserved.physical_authority_id,
                started_at,
            },
        )
        .expect("start exact test native credential execution")
}

pub(in crate::discovery_repository::tests) fn test_native_physical_authority_id(
    storage: &Storage,
    operation_id: &DiscoveryOperationId,
) -> String {
    storage
        .get_discovery_native_credential_execution(operation_id)
        .expect("load test native credential execution")
        .expect("test native credential execution exists")
        .physical_authority_id
}

pub(in crate::discovery_repository::tests) fn raw_test_native_physical_authority_id(
    storage: &Storage,
    operation_id: &DiscoveryOperationId,
) -> String {
    storage
        .connection()
        .expect("load raw test native execution database")
        .query_row(
            "SELECT physical_authority_id
             FROM provider_discovery_native_credential_executions
             WHERE operation_id = ?1",
            [operation_id.as_str()],
            |row| row.get(0),
        )
        .expect("load raw test native physical authority")
}

pub(in crate::discovery_repository::tests) fn assert_native_execution_table_is_append_only(
    database: &rusqlite::Connection,
    table: &str,
    operation_id: &DiscoveryOperationId,
) {
    database
        .execute(
            &format!(
                "INSERT OR REPLACE INTO {table} SELECT * FROM {table} WHERE operation_id = ?1"
            ),
            [operation_id.as_str()],
        )
        .expect_err("native execution history cannot be replaced");
    database
        .execute(
            &format!("UPDATE {table} SET schema_version = schema_version WHERE operation_id = ?1"),
            [operation_id.as_str()],
        )
        .expect_err("native execution history cannot be updated");
    database
        .execute(
            &format!("DELETE FROM {table} WHERE operation_id = ?1"),
            [operation_id.as_str()],
        )
        .expect_err("native execution history cannot be deleted");
}

pub(in crate::discovery_repository::tests) fn bypass_native_execution_table_version_guard(
    database: &rusqlite::Connection,
    table: &str,
    update_trigger: &str,
    operation_id: &DiscoveryOperationId,
) {
    let guard = suspend_test_trigger(database, update_trigger);
    database
        .execute_batch("PRAGMA ignore_check_constraints = ON;")
        .expect("suspend version CHECK only for corruption fixture");
    database
        .execute(
            &format!("UPDATE {table} SET schema_version = 2 WHERE operation_id = ?1"),
            [operation_id.as_str()],
        )
        .expect("inject unsupported native execution table version");
    database
        .execute_batch("PRAGMA ignore_check_constraints = OFF;")
        .expect("restore version CHECK enforcement");
    restore_test_trigger(database, &guard);
}

pub(in crate::discovery_repository::tests) fn native_credential_commit_plan(
    session: &ProviderDiscoverySession,
    id: &str,
    attempt_id: DiscoveryCommitAttemptId,
    credential_approval_id: DiscoveryApprovalId,
) -> DiscoveryCommitPlan {
    DiscoveryCommitPlan {
        attempt_id,
        session_id: session.id.clone(),
        expected_revision: 2,
        manifest_sha256: "1".repeat(64),
        graph_sha256: "2".repeat(64),
        template_id: ProviderTemplateId::from(format!("template-{id}")),
        template_version: 1,
        connection_id: session.input.connection_id.clone(),
        model_route_ids: vec![ModelRouteId::from(format!("route-{id}"))],
        credential_ref: session.input.credential_ref.clone(),
        credential_approval_id: Some(credential_approval_id),
        review_sha256: "3".repeat(64),
        catalog_authority: None,
        previous_selection: DiscoveryPreviousSelection::None,
    }
}

fn native_fixture_approvals(
    session: &ProviderDiscoverySession,
    plan: &DiscoveryCommitPlan,
    credential_approval_id: DiscoveryApprovalId,
    review_approval_id: DiscoveryApprovalId,
    prepared_at: chrono::DateTime<Utc>,
) -> (DiscoveryApprovalRecord, DiscoveryApprovalRecord) {
    let credential_approval = DiscoveryApprovalRecord {
        id: credential_approval_id,
        session_id: session.id.clone(),
        session_revision: 1,
        decision: DiscoveryApprovalDecision::Approved,
        grant: DiscoveryApprovalGrant::CredentialOrigin {
            origin: CanonicalOrigin::parse("https://provider.example/")
                .expect("credential approval origin"),
            auth_binding: lorepia_domain::AuthBinding::BearerHeader,
            manifest_sha256: plan.manifest_sha256.clone(),
        },
        created_at: prepared_at,
    };
    let review_approval = DiscoveryApprovalRecord {
        id: review_approval_id,
        session_id: session.id.clone(),
        session_revision: plan.expected_revision,
        decision: DiscoveryApprovalDecision::Approved,
        grant: DiscoveryApprovalGrant::Review {
            review_sha256: plan.review_sha256.clone(),
            graph_sha256: plan.graph_sha256.clone(),
        },
        created_at: prepared_at,
    };
    (credential_approval, review_approval)
}

fn seed_native_credential_commit(storage: &Storage, id: &str) -> NativeNoEffectFixture {
    let mut session = draft_session(id);
    session.input.credential_ref = Some(CredentialRef(
        session.input.connection_id.as_str().to_owned(),
    ));
    session.validate().expect("valid credential draft");
    storage
        .create_discovery_session(&session, now())
        .expect("create credential discovery session");

    let attempt_id =
        DiscoveryCommitAttemptId::parse(format!("attempt-{id}")).expect("commit attempt id");
    let action_id = DiscoveryActionId::parse(format!("action-{id}")).expect("commit action id");
    let operation_id =
        DiscoveryOperationId::parse(format!("operation-{id}")).expect("operation id");
    let credential_approval_id =
        DiscoveryApprovalId::parse(format!("approval-{id}")).expect("approval id");
    let plan = native_credential_commit_plan(
        &session,
        id,
        attempt_id.clone(),
        credential_approval_id.clone(),
    );
    plan.validate().expect("valid credential commit plan");
    let plan_json = encode_commit_plan_json(&plan).expect("commit plan JSON");
    let plan_sha256 = sha256_hex(plan_json.as_bytes());
    let mut awaiting_credential = session.clone();
    awaiting_credential.state = DiscoveryState::AwaitingCredentialOriginApproval;
    awaiting_credential.revision = 1;
    awaiting_credential.next_event_sequence = 2;
    awaiting_credential.manifest_sha256 = Some(plan.manifest_sha256.clone());
    awaiting_credential
        .validate()
        .expect("valid credential-origin approval session");
    let credential_approved = awaiting_credential
        .apply(&DiscoveryActionEnvelope {
            id: DiscoveryActionId::parse(format!("credential-action-{id}"))
                .expect("credential approval action id"),
            expected_revision: awaiting_credential.revision,
            request_sha256: "3".repeat(64),
            action: ProviderDiscoveryAction::ApproveCredentialOrigin {
                approval_id: credential_approval_id.clone(),
            },
        })
        .expect("approve native credential origin");
    let mut awaiting_review = credential_approved.session.clone();
    awaiting_review.state = DiscoveryState::AwaitingReview;
    awaiting_review.manifest_sha256 = Some(plan.manifest_sha256.clone());
    awaiting_review
        .validate()
        .expect("valid awaiting-review credential session");
    let review_approval_id =
        DiscoveryApprovalId::parse(format!("review-approval-{id}")).expect("review approval id");
    let prepare = awaiting_review
        .apply(&DiscoveryActionEnvelope {
            id: action_id,
            expected_revision: awaiting_review.revision,
            request_sha256: "4".repeat(64),
            action: ProviderDiscoveryAction::ApproveReview {
                approval_id: review_approval_id.clone(),
                commit_attempt_id: attempt_id.clone(),
                commit_plan_sha256: plan_sha256.clone(),
                graph_sha256: plan.graph_sha256.clone(),
            },
        })
        .expect("prepare native credential commit");
    let prepared_at = now();
    let (credential_approval, review_approval) = native_fixture_approvals(
        &session,
        &plan,
        credential_approval_id,
        review_approval_id,
        prepared_at,
    );
    persist_native_fixture_authority_history(
        storage,
        &credential_approved,
        &credential_approval,
        &prepare,
        &review_approval,
        &attempt_id,
        prepared_at,
    );
    persist_native_fixture_rows(
        storage,
        &prepare,
        &attempt_id,
        &operation_id,
        &plan_sha256,
        &plan_json,
        prepared_at,
    );

    session = prepare.session;
    NativeNoEffectFixture {
        session,
        operation_id,
        attempt_id,
        plan_sha256,
    }
}

fn persist_native_fixture_authority_history(
    storage: &Storage,
    credential_approved: &lorepia_domain::discovery::DiscoveryTransition,
    credential_approval: &DiscoveryApprovalRecord,
    prepare: &lorepia_domain::discovery::DiscoveryTransition,
    review_approval: &DiscoveryApprovalRecord,
    attempt_id: &DiscoveryCommitAttemptId,
    prepared_at: chrono::DateTime<Utc>,
) {
    let mut connection = storage.connection().expect("database connection");
    let transaction = connection.transaction().expect("authority transaction");
    insert_test_discovery_approval(&transaction, credential_approval);
    insert_test_discovery_approval(&transaction, review_approval);
    insert_test_discovery_receipt(&transaction, credential_approved, prepared_at);
    super::super::super::append_audit(
        &transaction,
        prepare.session.id.as_str(),
        credential_approved.receipt.resulting_revision,
        "approval_recorded",
        Some(credential_approved.receipt.action_id.as_str()),
        Some(credential_approval.id.as_str()),
        "discovery.audit.approval_recorded",
        prepared_at,
    )
    .expect("insert credential-origin approval audit");
    insert_test_discovery_receipt(&transaction, prepare, prepared_at);
    super::super::super::append_audit(
        &transaction,
        prepare.session.id.as_str(),
        prepare.receipt.resulting_revision,
        "approval_recorded",
        Some(prepare.receipt.action_id.as_str()),
        Some(review_approval.id.as_str()),
        "discovery.audit.approval_recorded",
        prepared_at,
    )
    .expect("insert review approval audit");
    super::super::super::append_audit(
        &transaction,
        prepare.session.id.as_str(),
        prepare.receipt.resulting_revision,
        "commit_prepared",
        Some(prepare.receipt.action_id.as_str()),
        Some(attempt_id.as_str()),
        "discovery.audit.commit_prepared",
        prepared_at,
    )
    .expect("insert native commit-prepared audit");
    transaction.commit().expect("commit authority history");
}

fn persist_native_fixture_rows(
    storage: &Storage,
    prepare: &lorepia_domain::discovery::DiscoveryTransition,
    attempt_id: &DiscoveryCommitAttemptId,
    operation_id: &DiscoveryOperationId,
    plan_sha256: &str,
    plan_json: &str,
    prepared_at: chrono::DateTime<Utc>,
) {
    let mut connection = storage.connection().expect("database connection");
    let session_guard =
        suspend_test_trigger(&connection, "provider_discovery_session_revision_guard");
    let transaction = connection.transaction().expect("fixture transaction");
    transaction
        .execute(
            "INSERT INTO provider_discovery_commit_attempts (
                 id, session_id, attempt_number, action_id, expected_revision,
                 plan_sha256, plan_json, phase, redaction_version,
                 created_at, updated_at, completed_at
             ) VALUES (?1, ?2, 1, ?3, ?4, ?5, ?6, 'prepared', 1, ?7, ?7, NULL)",
            rusqlite::params![
                attempt_id.as_str(),
                prepare.session.id.as_str(),
                prepare.receipt.action_id.as_str(),
                prepare.receipt.expected_revision,
                plan_sha256,
                plan_json,
                prepared_at.to_rfc3339(),
            ],
        )
        .expect("insert prepared credential commit");
    transaction
        .execute(
            "INSERT INTO provider_discovery_operations (
                 id, session_id, operation_kind, side_effect_class, status,
                 action_id, expected_revision, request_sha256, approval_id,
                 approval_grant_sha256, started_at, finished_at, created_at, updated_at
             ) VALUES (
                 ?1, ?2, 'atomic_commit', 'persistent', 'prepared',
                 ?3, ?4, ?5, NULL, NULL, NULL, NULL, ?6, ?6
             )",
            rusqlite::params![
                operation_id.as_str(),
                prepare.session.id.as_str(),
                prepare.receipt.action_id.as_str(),
                prepare.receipt.resulting_revision,
                prepare.receipt.request_sha256,
                prepared_at.to_rfc3339(),
            ],
        )
        .expect("insert started credential operation");
    transaction
        .execute(
            "UPDATE provider_discovery_sessions
             SET state = 'committing',
                 revision = ?2,
                 next_event_sequence = ?3,
                 manifest_sha256 = ?4,
                 commit_plan_sha256 = ?5,
                 commit_attempt_id = ?6,
                 active_operation_id = ?7,
                 updated_at = ?8
             WHERE id = ?1",
            rusqlite::params![
                prepare.session.id.as_str(),
                prepare.session.revision,
                prepare.session.next_event_sequence,
                prepare.session.manifest_sha256.as_deref(),
                plan_sha256,
                attempt_id.as_str(),
                operation_id.as_str(),
                prepared_at.to_rfc3339(),
            ],
        )
        .expect("activate credential commit fixture");
    restore_test_trigger(&transaction, &session_guard);
    transaction.commit().expect("commit credential fixture");
}

pub(in crate::discovery_repository::tests) fn native_no_effect_completion(
    storage: &Storage,
    fixture: &NativeNoEffectFixture,
    session: &ProviderDiscoverySession,
) -> (
    DiscoveryTransitionWrite,
    DiscoveryNativeNoEffectAttestationWrite,
) {
    let transition = apply(
        session,
        ProviderDiscoveryAction::Interrupt {
            operation: DiscoveryOperationKind::AtomicCommit,
            outcome: DiscoveryInterruptionOutcome::ConfirmedNoExternalEffect,
        },
        '5',
    );
    let mut write = write(
        transition,
        None,
        Some(DiscoveryCompletedOperationWrite {
            id: fixture.operation_id.clone(),
            outcome: DurableOperationOutcome::AttestedNoExternalEffect,
        }),
    );
    write.occurred_at = now() + chrono::Duration::milliseconds(2);
    let attestation = DiscoveryNativeNoEffectAttestationWrite::credential_slot_missing(
        fixture.operation_id.clone(),
        test_native_physical_authority_id(storage, &fixture.operation_id),
        fixture.session.id.clone(),
        fixture.attempt_id.clone(),
        fixture.plan_sha256.clone(),
        fixture.session.input.connection_id.clone(),
    )
    .expect("native no-effect attestation");
    (write, attestation)
}
