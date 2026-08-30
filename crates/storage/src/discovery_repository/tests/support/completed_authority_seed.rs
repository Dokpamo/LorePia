use super::*;

struct TestNativeExecution<'a> {
    operation_id: &'a DiscoveryOperationId,
    physical_authority_id: &'a str,
    session_id: &'a DiscoverySessionId,
    attempt_id: &'a DiscoveryCommitAttemptId,
    plan_sha256: &'a str,
    connection_id: &'a ProviderConnectionId,
    connection_binding_sha256: &'a str,
    reserved_at: chrono::DateTime<Utc>,
    store_started_at: chrono::DateTime<Utc>,
}

fn insert_test_native_execution(
    transaction: &rusqlite::Transaction<'_>,
    execution: &TestNativeExecution<'_>,
) {
    transaction
        .execute(
            "INSERT INTO provider_discovery_native_credential_executions (
                 physical_authority_id, operation_id, session_id,
                 commit_attempt_id, commit_plan_sha256, connection_id,
                 connection_binding_sha256, reserved_at,
                 schema_version, redaction_version
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, 1, 1)",
            rusqlite::params![
                execution.physical_authority_id,
                execution.operation_id.as_str(),
                execution.session_id.as_str(),
                execution.attempt_id.as_str(),
                execution.plan_sha256,
                execution.connection_id.as_str(),
                execution.connection_binding_sha256,
                execution.reserved_at.to_rfc3339(),
            ],
        )
        .expect("insert test native execution reservation");
    transaction
        .execute(
            "INSERT INTO provider_discovery_native_credential_store_attempts (
                 operation_id, physical_authority_id, started_at,
                 schema_version, redaction_version
             ) VALUES (?1, ?2, ?3, 1, 1)",
            rusqlite::params![
                execution.operation_id.as_str(),
                execution.physical_authority_id,
                execution.store_started_at.to_rfc3339(),
            ],
        )
        .expect("insert test native store attempt");
}

pub(in crate::discovery_repository::tests) fn project_completed_discovery_credential_authority(
    fixture: &CompletedDiscoveryAuthorityFixture,
) -> u64 {
    project_completed_discovery_credential_authority_at(
        fixture,
        now() + chrono::Duration::seconds(3),
    )
}

pub(in crate::discovery_repository::tests) fn project_completed_discovery_credential_authority_at(
    fixture: &CompletedDiscoveryAuthorityFixture,
    occurred_at: chrono::DateTime<Utc>,
) -> u64 {
    let mut database = fixture.storage.connection().expect("authority database");
    let transaction = database
        .transaction()
        .expect("begin discovery ownership transaction");
    let authority_sequence = super::super::super::insert_discovery_credential_ownership_event(
        &transaction,
        &fixture.connection_id,
        &fixture.binding_sha256,
        &fixture.physical_authority_id,
        &fixture.authority_operation_id,
        occurred_at,
    )
    .expect("insert exact discovery execution ownership event");
    let changed = transaction
        .execute(
            "UPDATE provider_credential_ownership
             SET ownership_state = 'discovery_owned',
                 connection_binding_sha256 = ?2,
                 authority_id = ?3,
                 authority_sequence = ?4,
                 updated_at = ?5
             WHERE connection_id = ?1 AND credential_ref = ?1",
            rusqlite::params![
                fixture.connection_id.as_str(),
                fixture.binding_sha256,
                fixture.physical_authority_id,
                authority_sequence,
                occurred_at.to_rfc3339(),
            ],
        )
        .expect("project exact discovery operation ownership");
    assert_eq!(changed, 1);
    transaction
        .commit()
        .expect("commit discovery operation ownership");
    authority_sequence
}

#[allow(clippy::too_many_lines)]
pub(in crate::discovery_repository::tests) fn seed_completed_discovery_authority(
    id: &str,
) -> CompletedDiscoveryAuthorityFixture {
    seed_completed_discovery_authority_with_mode(id, CompletedDiscoveryAuthorityMode::Direct)
}

#[allow(clippy::too_many_lines)]
pub(in crate::discovery_repository::tests) fn seed_completed_discovery_authority_with_mode(
    id: &str,
    mode: CompletedDiscoveryAuthorityMode,
) -> CompletedDiscoveryAuthorityFixture {
    let root = tempdir().expect("temp directory");
    let storage = Storage::open(root.path()).expect("open storage");
    let mut awaiting_review = draft_session(id);
    let connection_id = awaiting_review.input.connection_id.clone();
    awaiting_review.input.credential_ref = Some(CredentialRef(connection_id.as_str().to_owned()));
    storage
        .save_provider_profile(&ProviderProfile {
            id: connection_id.as_str().to_owned(),
            display_name: awaiting_review.input.display_name.clone(),
            base_url: "https://provider.example/v1".to_owned(),
            model: "synthetic".to_owned(),
            timeout_seconds: 30,
        })
        .expect("save authority provider graph");
    let connection = storage
        .get_provider_connection(&connection_id)
        .expect("load authority connection");
    let binding_sha256 =
        crate::provider_credential_repository::provider_credential_connection_binding_sha256(
            &storage.connection().expect("binding database"),
            &connection_id,
        )
        .expect("authority binding hash");
    let template = storage
        .get_provider_template(&connection.template_id, connection.template_version)
        .expect("load authority template");
    let routes = storage
        .list_model_routes(&connection_id)
        .expect("load authority routes");
    let observations = routes
        .iter()
        .flat_map(|route| {
            storage
                .list_capability_observations(&route.id)
                .expect("load authority observations")
        })
        .collect::<Vec<_>>();
    let presets = routes
        .iter()
        .flat_map(|route| {
            storage
                .list_generation_presets(&route.id)
                .expect("load authority presets")
        })
        .collect::<Vec<_>>();
    let graph_sha256 =
        provider_graph_ownership_hash(&template, &connection, &routes, &observations, &presets)
            .expect("authority graph hash");
    let evidence_id = EvidenceId::from(format!("evidence-{id}"));
    let review = DiscoveryReviewDiff::new(
        graph_sha256.clone(),
        vec![DiscoveryReviewChange {
            kind: DiscoveryReviewChangeKind::Add,
            target_kind: "provider_connection".to_owned(),
            target_id: connection_id.as_str().to_owned(),
            summary_key: "discovery.review.authority_fixture".to_owned(),
            evidence_ids: vec![evidence_id.clone()],
        }],
        0,
        0,
    )
    .expect("authority review");
    let manifest_json = canonical_json_result(
        serde_json::to_value(&template.default_manifest),
        "authority provider manifest",
    )
    .expect("authority manifest JSON");
    let manifest_sha256 = sha256_hex(manifest_json.as_bytes());
    let attempt_id =
        DiscoveryCommitAttemptId::parse(format!("attempt-{id}")).expect("authority attempt id");
    let credential_approval_id = DiscoveryApprovalId::parse(format!("credential-approval-{id}"))
        .expect("credential approval id");
    let review_approval_id =
        DiscoveryApprovalId::parse(format!("review-approval-{id}")).expect("review approval id");
    let expected_revision = 10;
    let plan = DiscoveryCommitPlan {
        attempt_id: attempt_id.clone(),
        session_id: awaiting_review.id.clone(),
        expected_revision,
        manifest_sha256: manifest_sha256.clone(),
        graph_sha256: graph_sha256.clone(),
        template_id: template.id.clone(),
        template_version: template.manifest_version,
        connection_id: connection_id.clone(),
        model_route_ids: routes.iter().map(|route| route.id.clone()).collect(),
        credential_ref: awaiting_review.input.credential_ref.clone(),
        credential_approval_id: Some(credential_approval_id.clone()),
        review_sha256: review.sha256.clone(),
        catalog_authority: None,
        previous_selection: DiscoveryPreviousSelection::None,
    };
    let plan_json = encode_commit_plan_json(&plan).expect("authority plan JSON");
    let plan_sha256 = sha256_hex(plan_json.as_bytes());
    let mut awaiting_credential = awaiting_review.clone();
    awaiting_credential.state = DiscoveryState::AwaitingCredentialOriginApproval;
    awaiting_credential.revision = expected_revision.saturating_sub(1);
    awaiting_credential.next_event_sequence = 20;
    awaiting_credential.manifest_sha256 = Some(manifest_sha256.clone());
    awaiting_credential
        .validate()
        .expect("awaiting credential authority session");
    let credential_approved = awaiting_credential
        .apply(&DiscoveryActionEnvelope {
            id: DiscoveryActionId::parse(format!("credential-action-{id}"))
                .expect("credential action id"),
            expected_revision: awaiting_credential.revision,
            request_sha256: "9".repeat(64),
            action: ProviderDiscoveryAction::ApproveCredentialOrigin {
                approval_id: credential_approval_id.clone(),
            },
        })
        .expect("approve credential origin authority");
    awaiting_review.state = DiscoveryState::AwaitingReview;
    awaiting_review.revision = expected_revision;
    awaiting_review.next_event_sequence = 21;
    awaiting_review.manifest_sha256 = Some(manifest_sha256.clone());
    awaiting_review
        .validate()
        .expect("awaiting review authority session");
    let prepare = awaiting_review
        .apply(&DiscoveryActionEnvelope {
            id: DiscoveryActionId::parse(format!("prepare-action-{id}"))
                .expect("prepare action id"),
            expected_revision,
            request_sha256: "a".repeat(64),
            action: ProviderDiscoveryAction::ApproveReview {
                approval_id: review_approval_id.clone(),
                commit_attempt_id: attempt_id.clone(),
                commit_plan_sha256: plan_sha256.clone(),
                graph_sha256: graph_sha256.clone(),
            },
        })
        .expect("prepare authority commit");
    let resolution_approval_id = DiscoveryApprovalId::parse(format!("resolution-approval-{id}"))
        .expect("resolution approval id");
    let (cancel, unknown, interrupted, restart, terminal) = match mode {
        CompletedDiscoveryAuthorityMode::Direct => {
            let terminal = prepare
                .session
                .apply(&DiscoveryActionEnvelope {
                    id: DiscoveryActionId::parse(format!("terminal-action-{id}"))
                        .expect("terminal action id"),
                    expected_revision: prepare.session.revision,
                    request_sha256: "b".repeat(64),
                    action: ProviderDiscoveryAction::CommitSucceeded {
                        connection_id: connection_id.clone(),
                    },
                })
                .expect("complete authority commit");
            (None, None, None, None, terminal)
        }
        CompletedDiscoveryAuthorityMode::Reconciled
        | CompletedDiscoveryAuthorityMode::PendingReconciled => {
            let unknown = prepare
                .session
                .apply(&DiscoveryActionEnvelope {
                    id: DiscoveryActionId::parse(format!("unknown-action-{id}"))
                        .expect("unknown action id"),
                    expected_revision: prepare.session.revision,
                    request_sha256: "c".repeat(64),
                    action: ProviderDiscoveryAction::ExternalOutcomeBecameUnknown,
                })
                .expect("record unknown authority outcome");
            let terminal = unknown
                .session
                .apply(&DiscoveryActionEnvelope {
                    id: DiscoveryActionId::parse(format!("resolution-action-{id}"))
                        .expect("resolution action id"),
                    expected_revision: unknown.session.revision,
                    request_sha256: "d".repeat(64),
                    action: ProviderDiscoveryAction::ResolveUnknownOutcome {
                        approval_id: resolution_approval_id.clone(),
                        resolution: DiscoveryUnknownOutcomeResolution::ConfirmedCommitCompleted {
                            connection_id: connection_id.clone(),
                        },
                    },
                })
                .expect("reconcile authority commit");
            (None, Some(unknown), None, None, terminal)
        }
        CompletedDiscoveryAuthorityMode::PreparedInterruptedRetry => {
            let interrupted = prepare
                .session
                .apply(&DiscoveryActionEnvelope {
                    id: DiscoveryActionId::parse(format!("interrupt-action-{id}"))
                        .expect("interrupt action id"),
                    expected_revision: prepare.session.revision,
                    request_sha256: "c".repeat(64),
                    action: ProviderDiscoveryAction::Interrupt {
                        operation: DiscoveryOperationKind::AtomicCommit,
                        outcome: DiscoveryInterruptionOutcome::ConfirmedNoExternalEffect,
                    },
                })
                .expect("interrupt prepared authority commit");
            let restart = interrupted
                .session
                .apply(&DiscoveryActionEnvelope {
                    id: DiscoveryActionId::parse(format!("restart-action-{id}"))
                        .expect("restart action id"),
                    expected_revision: interrupted.session.revision,
                    request_sha256: "d".repeat(64),
                    action: ProviderDiscoveryAction::RestartInterrupted,
                })
                .expect("restart prepared authority commit");
            let terminal = restart
                .session
                .apply(&DiscoveryActionEnvelope {
                    id: DiscoveryActionId::parse(format!("terminal-action-{id}"))
                        .expect("terminal action id"),
                    expected_revision: restart.session.revision,
                    request_sha256: "e".repeat(64),
                    action: ProviderDiscoveryAction::CommitSucceeded {
                        connection_id: connection_id.clone(),
                    },
                })
                .expect("complete prepared-interrupted authority commit");
            (None, None, Some(interrupted), Some(restart), terminal)
        }
        CompletedDiscoveryAuthorityMode::UnknownNoEffectRetry => {
            let unknown = prepare
                .session
                .apply(&DiscoveryActionEnvelope {
                    id: DiscoveryActionId::parse(format!("unknown-action-{id}"))
                        .expect("unknown action id"),
                    expected_revision: prepare.session.revision,
                    request_sha256: "c".repeat(64),
                    action: ProviderDiscoveryAction::ExternalOutcomeBecameUnknown,
                })
                .expect("record retryable unknown authority outcome");
            let interrupted = unknown
                .session
                .apply(&DiscoveryActionEnvelope {
                    id: DiscoveryActionId::parse(format!("resolution-action-{id}"))
                        .expect("resolution action id"),
                    expected_revision: unknown.session.revision,
                    request_sha256: "d".repeat(64),
                    action: ProviderDiscoveryAction::ResolveUnknownOutcome {
                        approval_id: resolution_approval_id.clone(),
                        resolution: DiscoveryUnknownOutcomeResolution::ConfirmedNoEffect,
                    },
                })
                .expect("confirm retryable no-effect authority outcome");
            let restart = interrupted
                .session
                .apply(&DiscoveryActionEnvelope {
                    id: DiscoveryActionId::parse(format!("restart-action-{id}"))
                        .expect("restart action id"),
                    expected_revision: interrupted.session.revision,
                    request_sha256: "e".repeat(64),
                    action: ProviderDiscoveryAction::RestartInterrupted,
                })
                .expect("restart interrupted authority commit");
            let terminal = restart
                .session
                .apply(&DiscoveryActionEnvelope {
                    id: DiscoveryActionId::parse(format!("terminal-action-{id}"))
                        .expect("terminal action id"),
                    expected_revision: restart.session.revision,
                    request_sha256: "f".repeat(64),
                    action: ProviderDiscoveryAction::CommitSucceeded {
                        connection_id: connection_id.clone(),
                    },
                })
                .expect("complete restarted authority commit");
            (
                None,
                Some(unknown),
                Some(interrupted),
                Some(restart),
                terminal,
            )
        }
        CompletedDiscoveryAuthorityMode::ConfirmedCommitCompensation => {
            let cancel = prepare
                .session
                .apply(&DiscoveryActionEnvelope {
                    id: DiscoveryActionId::parse(format!("cancel-action-{id}"))
                        .expect("cancel action id"),
                    expected_revision: prepare.session.revision,
                    request_sha256: "c".repeat(64),
                    action: ProviderDiscoveryAction::Cancel,
                })
                .expect("request authority commit cancellation");
            let unknown = cancel
                .session
                .apply(&DiscoveryActionEnvelope {
                    id: DiscoveryActionId::parse(format!("unknown-action-{id}"))
                        .expect("unknown action id"),
                    expected_revision: cancel.session.revision,
                    request_sha256: "d".repeat(64),
                    action: ProviderDiscoveryAction::ExternalOutcomeBecameUnknown,
                })
                .expect("record cancelling unknown authority outcome");
            let terminal = unknown
                .session
                .apply(&DiscoveryActionEnvelope {
                    id: DiscoveryActionId::parse(format!("resolution-action-{id}"))
                        .expect("resolution action id"),
                    expected_revision: unknown.session.revision,
                    request_sha256: "e".repeat(64),
                    action: ProviderDiscoveryAction::ResolveUnknownOutcome {
                        approval_id: resolution_approval_id.clone(),
                        resolution: DiscoveryUnknownOutcomeResolution::ConfirmedCommitCompleted {
                            connection_id: connection_id.clone(),
                        },
                    },
                })
                .expect("confirm cancelling commit completion");
            (Some(cancel), Some(unknown), None, None, terminal)
        }
        CompletedDiscoveryAuthorityMode::ConfirmedNoEffectCompensation => {
            let cancel = prepare
                .session
                .apply(&DiscoveryActionEnvelope {
                    id: DiscoveryActionId::parse(format!("cancel-action-{id}"))
                        .expect("cancel action id"),
                    expected_revision: prepare.session.revision,
                    request_sha256: "c".repeat(64),
                    action: ProviderDiscoveryAction::Cancel,
                })
                .expect("request no-effect authority cancellation");
            let unknown = cancel
                .session
                .apply(&DiscoveryActionEnvelope {
                    id: DiscoveryActionId::parse(format!("unknown-action-{id}"))
                        .expect("unknown action id"),
                    expected_revision: cancel.session.revision,
                    request_sha256: "d".repeat(64),
                    action: ProviderDiscoveryAction::ExternalOutcomeBecameUnknown,
                })
                .expect("record cancelling no-effect unknown outcome");
            let interrupted = unknown
                .session
                .apply(&DiscoveryActionEnvelope {
                    id: DiscoveryActionId::parse(format!("resolution-action-{id}"))
                        .expect("resolution action id"),
                    expected_revision: unknown.session.revision,
                    request_sha256: "e".repeat(64),
                    action: ProviderDiscoveryAction::ResolveUnknownOutcome {
                        approval_id: resolution_approval_id.clone(),
                        resolution: DiscoveryUnknownOutcomeResolution::ConfirmedNoEffect,
                    },
                })
                .expect("confirm cancelling no-effect authority outcome");
            let terminal = interrupted
                .session
                .apply(&DiscoveryActionEnvelope {
                    id: DiscoveryActionId::parse(format!("restart-action-{id}"))
                        .expect("compensation restart action id"),
                    expected_revision: interrupted.session.revision,
                    request_sha256: "f".repeat(64),
                    action: ProviderDiscoveryAction::RestartInterrupted,
                })
                .expect("restart interrupted compensation authority");
            (
                Some(cancel),
                Some(unknown),
                Some(interrupted),
                None,
                terminal,
            )
        }
    };
    let operation_id =
        DiscoveryOperationId::parse(format!("operation-{id}")).expect("authority operation id");
    let retry_operation_id = restart.as_ref().map(|_| {
        DiscoveryOperationId::parse(format!("retry-operation-{id}")).expect("retry operation id")
    });
    let compensation_operation_id = matches!(
        mode,
        CompletedDiscoveryAuthorityMode::ConfirmedCommitCompensation
            | CompletedDiscoveryAuthorityMode::ConfirmedNoEffectCompensation
    )
    .then(|| {
        DiscoveryOperationId::parse(format!("compensation-operation-{id}"))
            .expect("compensation operation id")
    });
    let authority_operation_id = retry_operation_id.as_ref().unwrap_or(&operation_id).clone();
    let initial_physical_authority_id = format!("discovery-native-{}", uuid::Uuid::new_v4());
    let retry_physical_authority_id = retry_operation_id
        .as_ref()
        .map(|_| format!("discovery-native-{}", uuid::Uuid::new_v4()));
    let physical_authority_id = retry_physical_authority_id
        .as_ref()
        .unwrap_or(&initial_physical_authority_id)
        .clone();
    let pending_reconciled = mode == CompletedDiscoveryAuthorityMode::PendingReconciled;
    let persisted_session = if pending_reconciled {
        &unknown
            .as_ref()
            .expect("pending reconciliation has an unknown transition")
            .session
    } else {
        &terminal.session
    };
    let prepared_at = now();
    let compensation_mode = compensation_operation_id.is_some();
    let cancel_at = prepared_at + chrono::Duration::seconds(1);
    let operation_finished_at = if compensation_mode {
        prepared_at + chrono::Duration::seconds(2)
    } else if unknown.is_some() || interrupted.is_some() {
        prepared_at + chrono::Duration::seconds(1)
    } else {
        prepared_at + chrono::Duration::seconds(2)
    };
    let resolution_at =
        prepared_at + chrono::Duration::seconds(if compensation_mode { 3 } else { 2 });
    let restart_at = prepared_at + chrono::Duration::seconds(if compensation_mode { 4 } else { 3 });
    let interrupted_at = if unknown.is_some() {
        resolution_at
    } else {
        operation_finished_at
    };
    let completed_at = match mode {
        CompletedDiscoveryAuthorityMode::ConfirmedCommitCompensation => resolution_at,
        CompletedDiscoveryAuthorityMode::ConfirmedNoEffectCompensation => restart_at,
        _ => prepared_at + chrono::Duration::seconds(if restart.is_some() { 4 } else { 2 }),
    };
    let compensation_started_at = completed_at + chrono::Duration::seconds(1);
    let input_json = canonical_json_result(
        serde_json::to_value(&persisted_session.input),
        "authority discovery input",
    )
    .expect("authority input JSON");
    let review_json = serde_json::to_string(&review).expect("authority review JSON");

    let mut database = storage.connection().expect("authority database");
    database
        .execute_batch(
            "DROP TRIGGER provider_discovery_session_initial_state_guard;
             DROP TRIGGER provider_discovery_commit_attempt_initial_state_guard;
             DROP TRIGGER provider_discovery_operation_initial_state_guard;
             DROP TRIGGER provider_discovery_native_credential_execution_insert_guard;
             DROP TRIGGER provider_discovery_native_credential_store_attempt_insert_guard;",
        )
        .expect("drop initial-state guards only for the completed-history fixture");
    let transaction = database.transaction().expect("authority transaction");
    transaction
        .execute(
            "INSERT INTO provider_discovery_sessions (
                 id, state, revision, next_event_sequence, sanitized_input_json,
                 draft_json, review_diff_json, error_json, recovery_json,
                 unknown_operation, manifest_sha256, commit_plan_sha256,
                 commit_attempt_id, committed_connection_id, cancellation_pending,
                 active_operation_id, active_effect_approval_json, redaction_version,
                 created_at, updated_at
             ) VALUES (
                 ?1, ?2, ?3, ?4, ?5, NULL, ?6, NULL, NULL, ?7,
                 ?8, ?9, ?10, ?11, ?12, ?13, NULL, 1, ?14, ?15
             )",
            rusqlite::params![
                persisted_session.id.as_str(),
                if compensation_mode {
                    "compensating"
                } else if pending_reconciled {
                    "unknown_outcome"
                } else {
                    "ready"
                },
                persisted_session.revision,
                persisted_session.next_event_sequence,
                input_json,
                review_json,
                persisted_session.unknown_operation.map(|operation| {
                    super::super::super::enum_wire_result(
                        serde_json::to_value(operation),
                        "pending reconciliation unknown operation",
                    )
                    .expect("unknown operation wire")
                }),
                manifest_sha256,
                plan_sha256,
                attempt_id.as_str(),
                persisted_session
                    .committed_connection_id
                    .as_ref()
                    .map(ProviderConnectionId::as_str),
                persisted_session.cancellation_pending,
                compensation_operation_id
                    .as_ref()
                    .map(DiscoveryOperationId::as_str),
                prepared_at.to_rfc3339(),
                if pending_reconciled {
                    operation_finished_at.to_rfc3339()
                } else {
                    completed_at.to_rfc3339()
                },
            ],
        )
        .expect("insert authority session");
    transaction
        .execute(
            "INSERT INTO provider_discovery_evidence (
                 id, session_id, kind, source_url, content_sha256,
                 extracted_json, redaction_version, fetched_at
             ) VALUES (?1, ?2, 'json_document', ?3, ?4, ?5, 1, ?6)",
            rusqlite::params![
                evidence_id.as_str(),
                terminal.session.id.as_str(),
                "https://provider.example/docs",
                "e".repeat(64),
                r#"{"shape":"object"}"#,
                prepared_at.to_rfc3339(),
            ],
        )
        .expect("insert authority evidence");
    insert_test_discovery_approval(
        &transaction,
        &DiscoveryApprovalRecord {
            id: credential_approval_id.clone(),
            session_id: terminal.session.id.clone(),
            session_revision: expected_revision.saturating_sub(1),
            decision: DiscoveryApprovalDecision::Approved,
            grant: DiscoveryApprovalGrant::CredentialOrigin {
                origin: connection.api_origin.clone(),
                auth_binding: connection
                    .credential_scope
                    .as_ref()
                    .expect("authority credential scope")
                    .auth_binding
                    .clone(),
                manifest_sha256: manifest_sha256.clone(),
            },
            created_at: prepared_at,
        },
    );
    if let Some(unknown) = &unknown
        && !pending_reconciled
    {
        let (resolution, created_at) = if matches!(
            mode,
            CompletedDiscoveryAuthorityMode::Reconciled
                | CompletedDiscoveryAuthorityMode::ConfirmedCommitCompensation
        ) {
            (
                DiscoveryUnknownOutcomeResolution::ConfirmedCommitCompleted {
                    connection_id: connection_id.clone(),
                },
                completed_at,
            )
        } else {
            (
                DiscoveryUnknownOutcomeResolution::ConfirmedNoEffect,
                resolution_at,
            )
        };
        insert_test_discovery_approval(
            &transaction,
            &DiscoveryApprovalRecord {
                id: resolution_approval_id.clone(),
                session_id: terminal.session.id.clone(),
                session_revision: unknown.session.revision,
                decision: DiscoveryApprovalDecision::Approved,
                grant: DiscoveryApprovalGrant::UnknownOutcomeResolution {
                    operation: DiscoveryOperationKind::AtomicCommit,
                    resolution,
                },
                created_at,
            },
        );
    }
    insert_test_discovery_approval(
        &transaction,
        &DiscoveryApprovalRecord {
            id: review_approval_id.clone(),
            session_id: terminal.session.id.clone(),
            session_revision: expected_revision,
            decision: DiscoveryApprovalDecision::Approved,
            grant: DiscoveryApprovalGrant::Review {
                review_sha256: review.sha256.clone(),
                graph_sha256: graph_sha256.clone(),
            },
            created_at: prepared_at,
        },
    );
    insert_test_discovery_receipt(&transaction, &credential_approved, prepared_at);
    super::super::super::append_audit(
        &transaction,
        terminal.session.id.as_str(),
        expected_revision,
        "approval_recorded",
        Some(credential_approved.receipt.action_id.as_str()),
        Some(credential_approval_id.as_str()),
        "discovery.audit.approval_recorded",
        prepared_at,
    )
    .expect("insert credential approval audit");
    insert_test_discovery_receipt(&transaction, &prepare, prepared_at);
    super::super::super::append_audit(
        &transaction,
        terminal.session.id.as_str(),
        prepare.receipt.resulting_revision,
        "approval_recorded",
        Some(prepare.receipt.action_id.as_str()),
        Some(review_approval_id.as_str()),
        "discovery.audit.approval_recorded",
        prepared_at,
    )
    .expect("insert review approval audit");
    transaction
        .execute(
            "INSERT INTO provider_discovery_commit_attempts (
                 id, session_id, attempt_number, action_id, expected_revision,
                 plan_sha256, plan_json, phase, redaction_version,
                 created_at, updated_at, completed_at
             ) VALUES (?1, ?2, 1, ?3, ?4, ?5, ?6, ?7, 1, ?8, ?9, ?10)",
            rusqlite::params![
                attempt_id.as_str(),
                terminal.session.id.as_str(),
                prepare.receipt.action_id.as_str(),
                expected_revision,
                plan_sha256,
                plan_json,
                if compensation_mode {
                    "compensating"
                } else if pending_reconciled {
                    "outcome_unknown"
                } else {
                    "completed"
                },
                prepared_at.to_rfc3339(),
                if compensation_mode {
                    compensation_started_at.to_rfc3339()
                } else if pending_reconciled {
                    operation_finished_at.to_rfc3339()
                } else {
                    completed_at.to_rfc3339()
                },
                (!compensation_mode && !pending_reconciled).then(|| completed_at.to_rfc3339()),
            ],
        )
        .expect("insert authority attempt");
    let initial_started_at = if mode == CompletedDiscoveryAuthorityMode::PreparedInterruptedRetry {
        operation_finished_at
    } else {
        prepared_at
    };
    transaction
        .execute(
            &format!(
                "INSERT INTO provider_discovery_operations (
                 id, session_id, operation_kind, side_effect_class, status,
                 action_id, expected_revision, request_sha256, approval_id,
                 approval_grant_sha256, started_at, finished_at, created_at, updated_at
             ) VALUES (
                 ?1, ?2, 'atomic_commit', 'persistent', '{}',
                 ?3, ?4, ?5, NULL, NULL, ?6, ?7, ?8, ?7
             )",
                if unknown.is_some() {
                    "outcome_unknown"
                } else if interrupted.is_some() {
                    "interrupted"
                } else {
                    "succeeded"
                }
            ),
            rusqlite::params![
                operation_id.as_str(),
                terminal.session.id.as_str(),
                prepare.receipt.action_id.as_str(),
                prepare.receipt.resulting_revision,
                prepare.receipt.request_sha256,
                initial_started_at.to_rfc3339(),
                operation_finished_at.to_rfc3339(),
                prepared_at.to_rfc3339(),
            ],
        )
        .expect("insert authority operation");
    super::super::super::append_audit(
        &transaction,
        terminal.session.id.as_str(),
        prepare.receipt.resulting_revision,
        "commit_prepared",
        Some(prepare.receipt.action_id.as_str()),
        Some(attempt_id.as_str()),
        "discovery.audit.commit_prepared",
        prepared_at,
    )
    .expect("insert authority commit-prepared audit");
    if mode != CompletedDiscoveryAuthorityMode::PreparedInterruptedRetry {
        super::super::super::append_audit(
            &transaction,
            terminal.session.id.as_str(),
            prepare.receipt.resulting_revision,
            "operation_started",
            Some(prepare.receipt.action_id.as_str()),
            Some(operation_id.as_str()),
            "discovery.audit.operation_started",
            initial_started_at,
        )
        .expect("insert authority operation-started audit");
    }
    if let Some(cancel) = &cancel {
        insert_test_discovery_receipt(&transaction, cancel, cancel_at);
    }
    if matches!(
        mode,
        CompletedDiscoveryAuthorityMode::Reconciled
            | CompletedDiscoveryAuthorityMode::PendingReconciled
            | CompletedDiscoveryAuthorityMode::ConfirmedCommitCompensation
    ) {
        super::super::super::append_audit(
            &transaction,
            terminal.session.id.as_str(),
            prepare.receipt.resulting_revision,
            "transition_applied",
            None,
            Some(&graph_sha256),
            "discovery.audit.provider_graph_applied",
            operation_finished_at,
        )
        .expect("insert reconciled authority graph audit");
        super::super::super::append_audit(
            &transaction,
            terminal.session.id.as_str(),
            prepare.receipt.resulting_revision,
            "transition_applied",
            None,
            Some("reused"),
            "discovery.audit.provider_template_ownership",
            operation_finished_at,
        )
        .expect("insert reconciled authority template audit");
    }
    if let Some(unknown) = &unknown {
        insert_test_discovery_receipt(&transaction, unknown, operation_finished_at);
        super::super::super::append_audit(
            &transaction,
            terminal.session.id.as_str(),
            unknown.receipt.resulting_revision,
            "operation_interrupted",
            Some(unknown.receipt.action_id.as_str()),
            Some(operation_id.as_str()),
            "discovery.audit.operation_interrupted",
            operation_finished_at,
        )
        .expect("insert authority outcome-unknown audit");
    }
    if let Some(interrupted) = &interrupted {
        insert_test_discovery_receipt(&transaction, interrupted, interrupted_at);
        if unknown.is_none() {
            super::super::super::append_audit(
                &transaction,
                terminal.session.id.as_str(),
                interrupted.receipt.resulting_revision,
                "operation_interrupted",
                Some(interrupted.receipt.action_id.as_str()),
                Some(operation_id.as_str()),
                "discovery.audit.operation_interrupted",
                interrupted_at,
            )
            .expect("insert authority prepared-interruption audit");
        } else {
            super::super::super::append_audit(
                &transaction,
                terminal.session.id.as_str(),
                interrupted.receipt.resulting_revision,
                "approval_recorded",
                Some(interrupted.receipt.action_id.as_str()),
                Some(resolution_approval_id.as_str()),
                "discovery.audit.approval_recorded",
                resolution_at,
            )
            .expect("insert retry resolution approval audit");
        }
    }
    if let Some(restart) = &restart {
        insert_test_discovery_receipt(&transaction, restart, restart_at);
        super::super::super::append_audit(
            &transaction,
            terminal.session.id.as_str(),
            restart.receipt.resulting_revision,
            "commit_prepared",
            Some(restart.receipt.action_id.as_str()),
            Some(attempt_id.as_str()),
            "discovery.audit.commit_prepared",
            restart_at,
        )
        .expect("insert authority retry commit-prepared audit");
        let retry_operation_id = retry_operation_id
            .as_ref()
            .expect("restart has a retry operation id");
        transaction
            .execute(
                "INSERT INTO provider_discovery_operations (
                     id, session_id, operation_kind, side_effect_class, status,
                     action_id, expected_revision, request_sha256, approval_id,
                     approval_grant_sha256, started_at, finished_at, created_at, updated_at
                 ) VALUES (
                     ?1, ?2, 'atomic_commit', 'persistent', 'succeeded',
                     ?3, ?4, ?5, NULL, NULL, ?6, ?7, ?6, ?7
                 )",
                rusqlite::params![
                    retry_operation_id.as_str(),
                    terminal.session.id.as_str(),
                    restart.receipt.action_id.as_str(),
                    restart.receipt.resulting_revision,
                    restart.receipt.request_sha256,
                    restart_at.to_rfc3339(),
                    completed_at.to_rfc3339(),
                ],
            )
            .expect("insert successful retry operation");
        super::super::super::append_audit(
            &transaction,
            terminal.session.id.as_str(),
            restart.receipt.resulting_revision,
            "operation_started",
            Some(restart.receipt.action_id.as_str()),
            Some(retry_operation_id.as_str()),
            "discovery.audit.operation_started",
            restart_at,
        )
        .expect("insert authority retry operation-started audit");
    }
    if matches!(
        mode,
        CompletedDiscoveryAuthorityMode::Direct
            | CompletedDiscoveryAuthorityMode::PreparedInterruptedRetry
            | CompletedDiscoveryAuthorityMode::UnknownNoEffectRetry
    ) {
        super::super::super::append_audit(
            &transaction,
            terminal.session.id.as_str(),
            terminal.previous_revision,
            "transition_applied",
            None,
            Some(&graph_sha256),
            "discovery.audit.provider_graph_applied",
            completed_at,
        )
        .expect("insert authority graph audit");
        super::super::super::append_audit(
            &transaction,
            terminal.session.id.as_str(),
            terminal.previous_revision,
            "transition_applied",
            None,
            Some("reused"),
            "discovery.audit.provider_template_ownership",
            completed_at,
        )
        .expect("insert authority template audit");
    }
    if !pending_reconciled {
        insert_test_discovery_receipt(&transaction, &terminal, completed_at);
    }
    if !pending_reconciled
        && matches!(
            mode,
            CompletedDiscoveryAuthorityMode::Reconciled
                | CompletedDiscoveryAuthorityMode::ConfirmedCommitCompensation
        )
    {
        super::super::super::append_audit(
            &transaction,
            terminal.session.id.as_str(),
            terminal.receipt.resulting_revision,
            "approval_recorded",
            Some(terminal.receipt.action_id.as_str()),
            Some(resolution_approval_id.as_str()),
            "discovery.audit.approval_recorded",
            completed_at,
        )
        .expect("insert reconciled resolution approval audit");
    }
    if let Some(compensation_operation_id) = &compensation_operation_id {
        let credential_ref = plan
            .credential_ref
            .as_ref()
            .expect("compensation authority plan has a credential reference");
        let step = DiscoveryCompensationStep {
            action_id: DiscoveryActionId::parse(format!("slot-removal-action-{id}"))
                .expect("slot-removal action id"),
            ordinal: 0,
            kind: DiscoveryCompensationKind::RemoveCredentialSlot,
            target: DiscoveryCompensationTarget::RemoveCredentialSlot {
                connection_id: connection_id.clone(),
                credential_ref: credential_ref.clone(),
            },
            status: DiscoveryCompensationStatus::Pending,
        };
        step.validate_against(&plan)
            .expect("valid credential compensation step");
        transaction
            .execute(
                "INSERT INTO provider_discovery_compensation_steps (
                     id, commit_attempt_id, ordinal, action_id, step_kind,
                     step_json, status, attempt_count, last_failure_json,
                     redaction_version, created_at, updated_at, completed_at
                 ) VALUES (
                     ?1, ?2, 0, ?3, 'remove_credential_slot',
                     ?4, 'pending', 0, NULL, 1, ?5, ?5, NULL
                 )",
                rusqlite::params![
                    format!("slot-removal-step-{id}"),
                    attempt_id.as_str(),
                    step.action_id.as_str(),
                    serde_json::to_string(&step).expect("slot-removal step JSON"),
                    completed_at.to_rfc3339(),
                ],
            )
            .expect("insert credential compensation step");
        transaction
            .execute(
                "INSERT INTO provider_discovery_operations (
                     id, session_id, operation_kind, side_effect_class, status,
                     action_id, expected_revision, request_sha256, approval_id,
                     approval_grant_sha256, started_at, finished_at, created_at, updated_at
                 ) VALUES (
                     ?1, ?2, 'compensation', 'persistent', 'started',
                     ?3, ?4, ?5, NULL, NULL, ?6, NULL, ?7, ?6
                 )",
                rusqlite::params![
                    compensation_operation_id.as_str(),
                    terminal.session.id.as_str(),
                    terminal.receipt.action_id.as_str(),
                    terminal.receipt.resulting_revision,
                    terminal.receipt.request_sha256,
                    compensation_started_at.to_rfc3339(),
                    completed_at.to_rfc3339(),
                ],
            )
            .expect("insert started credential compensation operation");
        super::super::super::append_audit(
            &transaction,
            terminal.session.id.as_str(),
            terminal.receipt.resulting_revision,
            "operation_started",
            Some(terminal.receipt.action_id.as_str()),
            Some(compensation_operation_id.as_str()),
            "discovery.audit.operation_started",
            compensation_started_at,
        )
        .expect("insert credential compensation operation-started audit");
    }
    if mode == CompletedDiscoveryAuthorityMode::PreparedInterruptedRetry {
        transaction
            .execute(
                "INSERT INTO provider_discovery_native_credential_executions (
                     physical_authority_id, operation_id, session_id,
                     commit_attempt_id, commit_plan_sha256, connection_id,
                     connection_binding_sha256, reserved_at,
                     schema_version, redaction_version
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, 1, 1)",
                rusqlite::params![
                    initial_physical_authority_id,
                    operation_id.as_str(),
                    terminal.session.id.as_str(),
                    attempt_id.as_str(),
                    plan_sha256,
                    connection_id.as_str(),
                    binding_sha256,
                    prepared_at.to_rfc3339(),
                ],
            )
            .expect("insert abandoned native execution reservation");
        transaction
            .execute(
                "INSERT INTO provider_discovery_native_credential_abandoned_reservations (
                     operation_id, physical_authority_id, session_id,
                     commit_attempt_id, commit_plan_sha256, connection_id,
                     connection_binding_sha256, reserved_at,
                     abandonment_kind, abandoned_at,
                     schema_version, redaction_version
                 ) VALUES (
                     ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8,
                     'prepared_interrupted_before_native_store', ?9, 1, 1
                 )",
                rusqlite::params![
                    operation_id.as_str(),
                    initial_physical_authority_id,
                    terminal.session.id.as_str(),
                    attempt_id.as_str(),
                    plan_sha256,
                    connection_id.as_str(),
                    binding_sha256,
                    prepared_at.to_rfc3339(),
                    operation_finished_at.to_rfc3339(),
                ],
            )
            .expect("insert exact abandoned reservation evidence");
    } else {
        insert_test_native_execution(
            &transaction,
            &TestNativeExecution {
                operation_id: &operation_id,
                physical_authority_id: &initial_physical_authority_id,
                session_id: &terminal.session.id,
                attempt_id: &attempt_id,
                plan_sha256: &plan_sha256,
                connection_id: &connection_id,
                connection_binding_sha256: &binding_sha256,
                reserved_at: prepared_at,
                store_started_at: initial_started_at,
            },
        );
    }
    if let (Some(retry_operation_id), Some(retry_physical_authority_id)) =
        (&retry_operation_id, &retry_physical_authority_id)
    {
        insert_test_native_execution(
            &transaction,
            &TestNativeExecution {
                operation_id: retry_operation_id,
                physical_authority_id: retry_physical_authority_id,
                session_id: &terminal.session.id,
                attempt_id: &attempt_id,
                plan_sha256: &plan_sha256,
                connection_id: &connection_id,
                connection_binding_sha256: &binding_sha256,
                reserved_at: restart_at,
                store_started_at: restart_at,
            },
        );
    }
    transaction.commit().expect("commit authority fixture");
    drop(database);
    CompletedDiscoveryAuthorityFixture {
        root,
        storage,
        session_id: terminal.session.id,
        connection_id,
        attempt_id,
        operation_id,
        authority_operation_id,
        physical_authority_id,
        evidence_id,
        binding_sha256,
    }
}
