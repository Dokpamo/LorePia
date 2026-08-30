use super::*;

pub(super) fn insert_test_discovery_approval(
    transaction: &rusqlite::Transaction<'_>,
    approval: &DiscoveryApprovalRecord,
) {
    let grant_json = encode_approval_grant(&approval.grant).expect("approval grant JSON");
    let grant_sha256 = sha256_hex(grant_json.as_bytes());
    transaction
        .execute(
            "INSERT INTO provider_discovery_approvals (
                 id, session_id, approval_kind, candidate_id, decision,
                 grant_json, session_revision, grant_sha256, redaction_version, created_at
             ) VALUES (?1, ?2, ?3, NULL, 'approved', ?4, ?5, ?6, 1, ?7)",
            rusqlite::params![
                approval.id.as_str(),
                approval.session_id.as_str(),
                super::super::super::approval_kind(&approval.grant),
                grant_json,
                approval.session_revision,
                grant_sha256,
                approval.created_at.to_rfc3339(),
            ],
        )
        .expect("insert authority approval");
}

pub(super) fn insert_test_discovery_receipt(
    transaction: &rusqlite::Transaction<'_>,
    transition: &lorepia_domain::discovery::DiscoveryTransition,
    occurred_at: chrono::DateTime<Utc>,
) {
    let event_json = super::super::super::encode_json_result(
        serde_json::to_value(&transition.event),
        "authority event JSON",
    )
    .expect("authority event JSON");
    let response_json = super::super::super::encode_json_result(
        serde_json::to_value(transition),
        "authority transition response JSON",
    )
    .expect("authority transition response JSON");
    let state = super::super::super::enum_wire_result(
        serde_json::to_value(transition.session.state),
        "authority event state",
    )
    .expect("authority state wire");
    let outcome = super::super::super::enum_wire_result(
        serde_json::to_value(transition.receipt.outcome),
        "authority receipt outcome",
    )
    .expect("authority outcome wire");
    transaction
        .execute(
            "INSERT INTO provider_discovery_event_outbox (
                 id, session_id, sequence, event_version, session_revision,
                 state, event_json, redaction_version, delivery_attempts,
                 available_at, delivered_at, created_at
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, 1, 0, ?8, NULL, ?8)",
            rusqlite::params![
                transition.event.id.as_str(),
                transition.event.session_id.as_str(),
                transition.event.sequence,
                transition.event.version,
                transition.event.session_revision,
                state,
                event_json,
                occurred_at.to_rfc3339(),
            ],
        )
        .expect("insert authority event");
    transaction
        .execute(
            "INSERT INTO provider_discovery_action_receipts (
                 action_id, session_id, action_kind, request_sha256,
                 expected_revision, resulting_revision, event_id,
                 event_sequence, outcome, response_json, redaction_version, created_at
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, 1, ?11)",
            rusqlite::params![
                transition.receipt.action_id.as_str(),
                transition.receipt.session_id.as_str(),
                transition.receipt.action_kind,
                transition.receipt.request_sha256,
                transition.receipt.expected_revision,
                transition.receipt.resulting_revision,
                transition.event.id.as_str(),
                transition.receipt.event_sequence,
                outcome,
                response_json,
                occurred_at.to_rfc3339(),
            ],
        )
        .expect("insert authority receipt");
    super::super::super::append_audit(
        transaction,
        transition.session.id.as_str(),
        transition.receipt.resulting_revision,
        super::super::super::audit_kind_for_action(&transition.receipt.action_kind),
        Some(transition.receipt.action_id.as_str()),
        Some(transition.event.id.as_str()),
        "discovery.audit.transition_applied",
        occurred_at,
    )
    .expect("insert authority transition audit");
}

pub(in crate::discovery_repository::tests) fn direct_completed_discovery_replay_write(
    fixture: &CompletedDiscoveryAuthorityFixture,
) -> DiscoveryTransitionWrite {
    let database = fixture.storage.connection().expect("replay database");
    let snapshot =
        super::super::super::load_session_snapshot(&database, fixture.session_id.as_str())
            .expect("load replay session")
            .expect("replay session exists");
    let terminal = super::super::super::load_discovery_authority_receipt_by_revision(
        &database,
        &fixture.session_id,
        snapshot.session.revision,
    )
    .expect("load direct terminal receipt");
    assert_eq!(terminal.receipt.action_kind, "commit_succeeded");
    let attempt = super::super::super::load_commit_attempt(&database, &fixture.attempt_id)
        .expect("load replay commit attempt");
    let graph = crate::database::load_discovered_provider_graph_rows(
        &database,
        &attempt.plan.template_id,
        attempt.plan.template_version,
        &fixture.connection_id,
    )
    .expect("load replay provider graph")
    .expect("replay provider graph exists");
    drop(database);
    let mut replay = write(
        terminal.transition,
        None,
        Some(DiscoveryCompletedOperationWrite {
            id: fixture.authority_operation_id.clone(),
            outcome: DurableOperationOutcome::Succeeded,
        }),
    );
    replay.provider_graph = Some(super::super::super::DiscoveredProviderGraph {
        plan: attempt.plan,
        plan_sha256: attempt.plan_sha256,
        template: graph.template,
        connection: graph.connection,
        routes: graph.routes,
        observations: graph.observations,
        presets: graph.presets,
    });
    replay.occurred_at = terminal.created_at;
    replay
}

pub(in crate::discovery_repository::tests) fn complete_ordinary_credential_successor(
    fixture: &CompletedDiscoveryAuthorityFixture,
) {
    let replacement_authority = fixture
        .storage
        .propose_provider_credential_install_authority(&fixture.connection_id)
        .expect("propose ordinary successor install authority");
    let replacement = fixture
        .storage
        .prepare_provider_credential_operation_with_install_authority(
            &fixture.connection_id,
            ProviderCredentialOperationKind::Install,
            ProviderCredentialObservedStatus::Missing,
            Some(&replacement_authority),
        )
        .expect("prepare ordinary successor install");
    fixture
        .storage
        .start_provider_credential_operation(
            &replacement.plan.operation_id,
            &replacement.plan_sha256,
        )
        .expect("start ordinary successor install");
    fixture
        .storage
        .attest_provider_credential_predecessor_delete_intent(
            &replacement.plan.operation_id,
            &replacement.plan_sha256,
            ProviderCredentialObservedStatus::Available,
        )
        .expect("record predecessor deletion intent");
    fixture
        .storage
        .attest_provider_credential_predecessor_missing(
            &replacement.plan.operation_id,
            &replacement.plan_sha256,
        )
        .expect("attest discovery predecessor missing");
    fixture
        .storage
        .finish_provider_credential_operation(
            &replacement.plan.operation_id,
            &replacement.plan_sha256,
            ProviderCredentialObservedStatus::Available,
        )
        .expect("complete ordinary ownership successor");
}

pub(in crate::discovery_repository::tests) fn active_credential_ownership_tail(
    fixture: &CompletedDiscoveryAuthorityFixture,
) -> (u64, u64) {
    fixture
        .storage
        .connection()
        .expect("ownership tail database")
        .query_row(
            "SELECT ownership.authority_sequence, COUNT(event.authority_sequence)
             FROM provider_credential_ownership AS ownership
             JOIN provider_credential_ownership_events AS event
               ON event.connection_id = ownership.connection_id
             WHERE ownership.connection_id = ?1
             GROUP BY ownership.authority_sequence",
            [fixture.connection_id.as_str()],
            |row| Ok((row.get::<_, u64>(0)?, row.get::<_, u64>(1)?)),
        )
        .expect("load active ownership tail")
}

pub(in crate::discovery_repository::tests) fn persist_pending_confirmed_commit_completion(
    fixture: &CompletedDiscoveryAuthorityFixture,
    action_label: &str,
    approval_label: &str,
) -> DiscoveryApprovalId {
    let unknown = fixture
        .storage
        .get_discovery_session(&fixture.session_id)
        .expect("load pending reconciled discovery session");
    assert_eq!(unknown.session.state, DiscoveryState::UnknownOutcome);
    let approval_id = DiscoveryApprovalId::parse(approval_label).expect("resolution approval");
    let resolution = DiscoveryUnknownOutcomeResolution::ConfirmedCommitCompleted {
        connection_id: fixture.connection_id.clone(),
    };
    let occurred_at = now() + chrono::Duration::seconds(2);
    let transition = unknown
        .session
        .apply(&DiscoveryActionEnvelope {
            id: DiscoveryActionId::parse(action_label).expect("resolution action"),
            expected_revision: unknown.session.revision,
            request_sha256: "d".repeat(64),
            action: ProviderDiscoveryAction::ResolveUnknownOutcome {
                approval_id: approval_id.clone(),
                resolution: resolution.clone(),
            },
        })
        .expect("resolve graph-backed unknown commit");
    let mut transition_write = write(transition, None, None);
    transition_write.approval = Some(DiscoveryApprovalRecord {
        id: approval_id.clone(),
        session_id: fixture.session_id.clone(),
        session_revision: unknown.session.revision,
        decision: DiscoveryApprovalDecision::Approved,
        grant: DiscoveryApprovalGrant::UnknownOutcomeResolution {
            operation: DiscoveryOperationKind::AtomicCommit,
            resolution,
        },
        created_at: occurred_at,
    });
    transition_write.occurred_at = occurred_at;
    assert!(matches!(
        fixture
            .storage
            .persist_discovery_transition(&transition_write)
            .expect("persist public confirmed-completion transition"),
        PersistDiscoveryTransition::Applied { .. }
    ));
    approval_id
}

pub(in crate::discovery_repository::tests) fn assert_discovery_authority_history_replace_is_guarded_and_revalidated(
    fixture: &CompletedDiscoveryAuthorityFixture,
    trigger_name: &str,
    replacement_sql: &str,
    selector: &str,
    replacement: &str,
    disable_foreign_keys_for_corruption: bool,
) {
    let database = fixture.storage.connection().expect("authority database");
    let replace = || database.execute(replacement_sql, rusqlite::params![selector, replacement]);
    let trigger_error = replace().expect_err("authority history REPLACE guard must reject");
    assert!(
        trigger_error.to_string().contains("cannot replace history"),
        "replacement was not rejected by the expected no-REPLACE guard: {trigger_error}"
    );
    validate_discovery_credential_ownership_authority(
        &database,
        &fixture.connection_id,
        &fixture.physical_authority_id,
        fixture.authority_operation_id.as_str(),
        &fixture.binding_sha256,
    )
    .expect("rejected authority history replacement preserves the original authority");

    database
        .execute_batch(&format!("DROP TRIGGER {trigger_name};"))
        .expect("drop only the selected no-REPLACE guard for corruption fixture");
    if disable_foreign_keys_for_corruption {
        database
            .pragma_update(None, "foreign_keys", false)
            .expect("temporarily disable foreign keys for physical corruption fixture");
    }
    replace().expect("inject replaced authority history after dropping its test guard");
    if disable_foreign_keys_for_corruption {
        database
            .pragma_update(None, "foreign_keys", true)
            .expect("restore foreign keys after physical corruption fixture");
    }
    let error = validate_discovery_credential_ownership_authority(
        &database,
        &fixture.connection_id,
        &fixture.physical_authority_id,
        fixture.authority_operation_id.as_str(),
        &fixture.binding_sha256,
    )
    .expect_err("replaced authority history must fail runtime validation closed");
    assert_eq!(error.code, CoreErrorCode::StorageCorrupted);
}
