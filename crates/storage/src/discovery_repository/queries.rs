//! Discovery read facades and their ordered `SQLite` queries.

use std::collections::BTreeSet;

use chrono::{DateTime, Utc};
use rusqlite::{Connection, OptionalExtension, Transaction, params};
use serde_json::Value;

use super::{
    CoreError, CoreErrorCode, CoreResult, DiscoveryActionId, DiscoveryActionReceipt,
    DiscoveryActionReplay, DiscoveryApprovalRecord, DiscoveryCommitAttemptId,
    DiscoveryCommitAttemptRecord, DiscoveryCompensationRecord, DiscoveryEvidenceRecord,
    DiscoveryNativeCredentialExecutionRecord, DiscoveryNativeNoEffectAttestationRecord,
    DiscoveryOperationId, DiscoveryOperationRecord, DiscoveryOutboxEvent,
    DiscoveryPreviousSelection, DiscoveryReviewDiff, DiscoverySessionId, DiscoverySessionSnapshot,
    DiscoveryTransition, Storage, StoredDiscoveryCandidate, corrupted, database_error, discovery,
    discovery_error, load_commit_attempt, load_discovery_credential_compensation_operation_id,
    load_discovery_previous_selection, load_native_no_effect_attestation,
};
use super::{
    repository_io::{
        decode_session_row, validate_missing_native_credential_execution,
        validate_native_credential_abandonment,
        validate_native_credential_execution_commit_binding,
    },
    row_mapping::{
        NativeCredentialExecutionRow, decode_approval_row, decode_candidate_row,
        decode_compensation_row, decode_evidence_row, decode_native_credential_execution_row,
        decode_operation_row, decode_outbox_row,
    },
    validation::{require_session, validate_identifier, validate_limit, validate_sha256},
};

impl Storage {
    pub fn get_discovery_session(
        &self,
        session_id: &DiscoverySessionId,
    ) -> CoreResult<DiscoverySessionSnapshot> {
        let connection = self.connection()?;
        load_session_snapshot(&connection, session_id.as_str())?.ok_or_else(|| {
            CoreError::new(
                CoreErrorCode::NotFound,
                "provider discovery session was not found",
                false,
            )
        })
    }

    pub fn list_discovery_sessions(&self, limit: u32) -> CoreResult<Vec<DiscoverySessionSnapshot>> {
        validate_limit(limit)?;
        let connection = self.connection()?;
        let ids = {
            let mut statement = connection
                .prepare(
                    "SELECT id
                     FROM provider_discovery_sessions
                     ORDER BY updated_at DESC, id
                     LIMIT ?1",
                )
                .map_err(database_error)?;
            statement
                .query_map([limit], |row| row.get::<_, String>(0))
                .map_err(database_error)?
                .collect::<Result<Vec<_>, _>>()
                .map_err(database_error)?
        };
        ids.into_iter()
            .map(|id| {
                load_session_snapshot(&connection, &id)?.ok_or_else(|| {
                    corrupted("discovery session disappeared while it was being listed")
                })
            })
            .collect()
    }

    /// Returns every session with an unfinished durable operation.
    ///
    /// Startup recovery must not infer completeness from the bounded
    /// user-facing history query. This complete internal scan uses the partial
    /// `provider_discovery_operations_recovery` index and de-duplicates session
    /// identifiers while preserving durable operation order.
    pub fn list_unfinished_discovery_sessions_for_recovery(
        &self,
    ) -> CoreResult<Vec<DiscoverySessionSnapshot>> {
        let connection = self.connection()?;
        let unfinished = discovery::list_unfinished_discovery_operations(&connection)
            .map_err(discovery_error)?;
        let mut seen = BTreeSet::new();
        unfinished
            .into_iter()
            .filter(|operation| seen.insert(operation.session_id.clone()))
            .map(|operation| operation.session_id)
            .map(|session_id| {
                load_session_snapshot(&connection, &session_id)?.ok_or_else(|| {
                    corrupted("unfinished discovery session disappeared during recovery scan")
                })
            })
            .collect()
    }

    pub fn list_discovery_evidence(
        &self,
        session_id: &DiscoverySessionId,
        limit: u32,
    ) -> CoreResult<Vec<DiscoveryEvidenceRecord>> {
        validate_limit(limit)?;
        let connection = self.connection()?;
        require_session(&connection, session_id.as_str())?;
        let mut statement = connection
            .prepare(
                "SELECT id, session_id, kind, source_url, content_sha256,
                        extracted_json, fetched_at
                 FROM provider_discovery_evidence
                 WHERE session_id = ?1
                 ORDER BY fetched_at, id
                 LIMIT ?2",
            )
            .map_err(database_error)?;
        statement
            .query_map(params![session_id.as_str(), limit], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, String>(4)?,
                    row.get::<_, String>(5)?,
                    row.get::<_, String>(6)?,
                ))
            })
            .map_err(database_error)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(database_error)?
            .into_iter()
            .map(decode_evidence_row)
            .collect()
    }

    pub fn list_discovery_candidates(
        &self,
        session_id: &DiscoverySessionId,
        limit: u32,
    ) -> CoreResult<Vec<StoredDiscoveryCandidate>> {
        validate_limit(limit)?;
        let connection = self.connection()?;
        require_session(&connection, session_id.as_str())?;
        let mut statement = connection
            .prepare(
                "SELECT id, session_id, candidate_kind, summary_json, evidence_ids_json,
                        proposed_revision, created_at
                 FROM provider_discovery_candidates
                 WHERE session_id = ?1
                 ORDER BY proposed_revision, created_at, id
                 LIMIT ?2",
            )
            .map_err(database_error)?;
        statement
            .query_map(params![session_id.as_str(), limit], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, String>(4)?,
                    row.get::<_, u64>(5)?,
                    row.get::<_, String>(6)?,
                ))
            })
            .map_err(database_error)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(database_error)?
            .into_iter()
            .map(decode_candidate_row)
            .collect()
    }

    pub fn list_discovery_approvals(
        &self,
        session_id: &DiscoverySessionId,
        limit: u32,
    ) -> CoreResult<Vec<DiscoveryApprovalRecord>> {
        validate_limit(limit)?;
        let connection = self.connection()?;
        require_session(&connection, session_id.as_str())?;
        let mut statement = connection
            .prepare(
                "SELECT id, session_id, approval_kind, candidate_id, decision,
                        grant_json, session_revision, grant_sha256, created_at
                 FROM provider_discovery_approvals
                 WHERE session_id = ?1
                 ORDER BY session_revision, created_at, id
                 LIMIT ?2",
            )
            .map_err(database_error)?;
        statement
            .query_map(params![session_id.as_str(), limit], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, Option<String>>(3)?,
                    row.get::<_, String>(4)?,
                    row.get::<_, String>(5)?,
                    row.get::<_, u64>(6)?,
                    row.get::<_, String>(7)?,
                    row.get::<_, String>(8)?,
                ))
            })
            .map_err(database_error)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(database_error)?
            .into_iter()
            .map(decode_approval_row)
            .collect()
    }

    pub fn get_discovery_review(
        &self,
        session_id: &DiscoverySessionId,
    ) -> CoreResult<Option<DiscoveryReviewDiff>> {
        Ok(self.get_discovery_session(session_id)?.review)
    }

    pub fn get_current_discovery_operation(
        &self,
        session_id: &DiscoverySessionId,
    ) -> CoreResult<Option<DiscoveryOperationRecord>> {
        let connection = self.connection()?;
        let snapshot =
            load_session_snapshot(&connection, session_id.as_str())?.ok_or_else(|| {
                CoreError::new(
                    CoreErrorCode::NotFound,
                    "provider discovery session was not found",
                    false,
                )
            })?;
        snapshot
            .active_operation_id
            .as_ref()
            .map(|operation_id| load_operation_by_id(&connection, operation_id))
            .transpose()
    }

    pub fn get_discovery_native_no_effect_attestation(
        &self,
        operation_id: &DiscoveryOperationId,
    ) -> CoreResult<Option<DiscoveryNativeNoEffectAttestationRecord>> {
        let connection = self.connection()?;
        load_native_no_effect_attestation(&connection, operation_id.as_str())
    }

    /// Loads the immutable physical native authority for one discovery
    /// operation. An unreserved prepared operation returns `None`; a reserved
    /// prepared operation returns the row without a store-attempt timestamp,
    /// and a started operation must return the same row with that timestamp.
    pub fn get_discovery_native_credential_execution(
        &self,
        operation_id: &DiscoveryOperationId,
    ) -> CoreResult<Option<DiscoveryNativeCredentialExecutionRecord>> {
        let connection = self.connection()?;
        load_discovery_native_credential_execution(&connection, operation_id)
    }

    pub fn find_discovery_action_replay(
        &self,
        session_id: &DiscoverySessionId,
        action_id: &DiscoveryActionId,
        request_sha256: &str,
        action_kind: &str,
    ) -> CoreResult<Option<DiscoveryActionReplay>> {
        validate_sha256("discovery action request hash", request_sha256)?;
        validate_identifier("discovery action kind", action_kind, 128)?;
        let row = self
            .connection()?
            .query_row(
                "SELECT session_id, action_kind, request_sha256, expected_revision,
                        resulting_revision, event_sequence, outcome, response_json
                 FROM provider_discovery_action_receipts
                 WHERE action_id = ?1",
                [action_id.as_str()],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, u64>(3)?,
                        row.get::<_, u64>(4)?,
                        row.get::<_, u64>(5)?,
                        row.get::<_, String>(6)?,
                        row.get::<_, String>(7)?,
                    ))
                },
            )
            .optional()
            .map_err(database_error)?;
        let Some(row) = row else {
            return Ok(None);
        };
        if row.0 != session_id.as_str() || row.1 != action_kind || row.2 != request_sha256 {
            return Err(CoreError::invalid(
                "discovery action identifier was reused with a different request",
            ));
        }
        let transition = serde_json::from_str::<DiscoveryTransition>(&row.7)
            .map_err(|_| corrupted("stored discovery action response is invalid"))?;
        let receipt = DiscoveryActionReceipt {
            action_id: action_id.clone(),
            session_id: session_id.clone(),
            action_kind: row.1,
            request_sha256: row.2,
            expected_revision: row.3,
            resulting_revision: row.4,
            event_sequence: row.5,
            outcome: serde_json::from_value(Value::String(row.6))
                .map_err(|_| corrupted("stored discovery receipt outcome is invalid"))?,
        };
        if transition.receipt != receipt {
            return Err(corrupted(
                "stored discovery replay response does not match its receipt",
            ));
        }
        Ok(Some(DiscoveryActionReplay {
            receipt,
            transition,
        }))
    }

    pub fn get_discovery_commit_attempt(
        &self,
        attempt_id: &DiscoveryCommitAttemptId,
    ) -> CoreResult<DiscoveryCommitAttemptRecord> {
        let connection = self.connection()?;
        load_commit_attempt(&connection, attempt_id)
    }

    /// Returns the operation-scoped physical credential authority that led to
    /// the current compensation ledger. Selection follows the immutable
    /// receipt/recovery chain into `compensating`; it never guesses from the
    /// latest operation or from the reusable commit-attempt identifier.
    pub fn get_discovery_credential_compensation_operation_id(
        &self,
        session_id: &DiscoverySessionId,
        attempt_id: &DiscoveryCommitAttemptId,
        plan_sha256: &str,
    ) -> CoreResult<DiscoveryOperationId> {
        validate_sha256("discovery compensation plan hash", plan_sha256)?;
        let connection = self.connection()?;
        load_discovery_credential_compensation_operation_id(
            &connection,
            session_id,
            attempt_id,
            plan_sha256,
        )
    }

    /// Captures the exact route-and-preset selection for an immutable commit
    /// plan. Both identifiers are read under the same storage lock.
    pub fn current_discovery_previous_selection(&self) -> CoreResult<DiscoveryPreviousSelection> {
        let connection = self.connection()?;
        load_discovery_previous_selection(&connection)
    }

    pub fn list_discovery_compensation_steps(
        &self,
        attempt_id: &DiscoveryCommitAttemptId,
    ) -> CoreResult<Vec<DiscoveryCompensationRecord>> {
        let connection = self.connection()?;
        let attempt = load_commit_attempt(&connection, attempt_id)?;
        let mut statement = connection
            .prepare(
                "SELECT id, commit_attempt_id, ordinal, action_id, step_kind, step_json,
                        status, attempt_count, last_failure_json, created_at,
                        updated_at, completed_at
                 FROM provider_discovery_compensation_steps
                 WHERE commit_attempt_id = ?1
                 ORDER BY ordinal DESC, id",
            )
            .map_err(database_error)?;
        statement
            .query_map([attempt_id.as_str()], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, u32>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, String>(4)?,
                    row.get::<_, String>(5)?,
                    row.get::<_, String>(6)?,
                    row.get::<_, u32>(7)?,
                    row.get::<_, Option<String>>(8)?,
                    row.get::<_, String>(9)?,
                    row.get::<_, String>(10)?,
                    row.get::<_, Option<String>>(11)?,
                ))
            })
            .map_err(database_error)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(database_error)?
            .into_iter()
            .map(|row| decode_compensation_row(row, &attempt.plan))
            .collect()
    }
}

pub(super) fn load_session_snapshot(
    connection: &Connection,
    session_id: &str,
) -> CoreResult<Option<DiscoverySessionSnapshot>> {
    let row = connection
        .query_row(
            "SELECT id, state, revision, next_event_sequence, sanitized_input_json,
                    draft_json, review_diff_json, error_json, recovery_json,
                    unknown_operation, manifest_sha256, commit_plan_sha256,
                    commit_attempt_id, committed_connection_id, cancellation_pending,
                    active_operation_id, active_effect_approval_json,
                    created_at, updated_at
             FROM provider_discovery_sessions
             WHERE id = ?1",
            [session_id],
            |row| {
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                    row.get(5)?,
                    row.get(6)?,
                    row.get(7)?,
                    row.get(8)?,
                    row.get(9)?,
                    row.get(10)?,
                    row.get(11)?,
                    row.get(12)?,
                    row.get(13)?,
                    row.get(14)?,
                    row.get(15)?,
                    row.get(16)?,
                    row.get(17)?,
                    row.get(18)?,
                ))
            },
        )
        .optional()
        .map_err(database_error)?;
    row.map(|row| decode_session_row(connection, row))
        .transpose()
}

fn load_native_credential_execution_row(
    connection: &Connection,
    operation_id: &DiscoveryOperationId,
) -> CoreResult<Option<NativeCredentialExecutionRow>> {
    connection
        .query_row(
            "SELECT execution.physical_authority_id, execution.operation_id,
                    execution.session_id, execution.commit_attempt_id,
                    execution.commit_plan_sha256, execution.connection_id,
                    execution.connection_binding_sha256, execution.reserved_at,
                    execution.schema_version, execution.redaction_version,
                    store_attempt.started_at, store_attempt.schema_version,
                    store_attempt.redaction_version
             FROM provider_discovery_native_credential_executions AS execution
             LEFT JOIN provider_discovery_native_credential_store_attempts AS store_attempt
               ON store_attempt.operation_id = execution.operation_id
              AND store_attempt.physical_authority_id = execution.physical_authority_id
             WHERE execution.operation_id = ?1",
            [operation_id.as_str()],
            |row| {
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                    row.get(5)?,
                    row.get(6)?,
                    row.get(7)?,
                    row.get(8)?,
                    row.get(9)?,
                    row.get(10)?,
                    row.get(11)?,
                    row.get(12)?,
                ))
            },
        )
        .optional()
        .map_err(database_error)
}

pub(super) fn load_discovery_native_credential_execution(
    connection: &Connection,
    operation_id: &DiscoveryOperationId,
) -> CoreResult<Option<DiscoveryNativeCredentialExecutionRecord>> {
    let row = load_native_credential_execution_row(connection, operation_id)?;
    let operation = load_operation_by_id(connection, operation_id)?;
    let Some(row) = row else {
        validate_missing_native_credential_execution(connection, &operation)?;
        return Ok(None);
    };
    let (execution, reserved_at_raw) = decode_native_credential_execution_row(row)?;
    let attempt = load_commit_attempt(connection, &execution.commit_attempt_id)?;
    let valid_abandonment = validate_native_credential_abandonment(
        connection,
        &operation,
        &execution,
        &reserved_at_raw,
    )?;
    validate_native_credential_execution_commit_binding(
        connection,
        &operation,
        &attempt,
        &execution,
        valid_abandonment,
    )?;
    Ok(Some(execution))
}

pub(super) fn load_operation_by_id(
    connection: &Connection,
    operation_id: &DiscoveryOperationId,
) -> CoreResult<DiscoveryOperationRecord> {
    let row = connection
        .query_row(
            "SELECT id, session_id, operation_kind, side_effect_class, status,
                    action_id, expected_revision, request_sha256, approval_id,
                    approval_grant_sha256, started_at, finished_at, created_at, updated_at
             FROM provider_discovery_operations
             WHERE id = ?1",
            [operation_id.as_str()],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, String>(4)?,
                    row.get::<_, String>(5)?,
                    row.get::<_, u64>(6)?,
                    row.get::<_, String>(7)?,
                    row.get::<_, Option<String>>(8)?,
                    row.get::<_, Option<String>>(9)?,
                    row.get::<_, Option<String>>(10)?,
                    row.get::<_, Option<String>>(11)?,
                    row.get::<_, String>(12)?,
                    row.get::<_, String>(13)?,
                ))
            },
        )
        .optional()
        .map_err(database_error)?
        .ok_or_else(|| corrupted("active discovery operation is missing"))?;
    decode_operation_row(row)
}

pub(super) fn load_pollable_outbox_rows(
    transaction: &Transaction<'_>,
    limit: u32,
    available_at: DateTime<Utc>,
) -> CoreResult<Vec<DiscoveryOutboxEvent>> {
    let mut statement = transaction
        .prepare(
            "SELECT event.id, event.session_id, event.sequence, event.event_version,
                    event.session_revision, event.state, event.event_json,
                    event.delivery_attempts, event.available_at, event.created_at
             FROM provider_discovery_event_outbox AS event
             WHERE event.delivered_at IS NULL
               AND event.available_at <= ?1
               AND NOT EXISTS (
                   SELECT 1
                   FROM provider_discovery_event_outbox AS earlier
                   WHERE earlier.session_id = event.session_id
                     AND earlier.delivered_at IS NULL
                     AND earlier.sequence < event.sequence
               )
             ORDER BY event.available_at, event.session_id, event.sequence
             LIMIT ?2",
        )
        .map_err(database_error)?;
    statement
        .query_map(params![available_at.to_rfc3339(), limit], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, u64>(2)?,
                row.get::<_, u32>(3)?,
                row.get::<_, u64>(4)?,
                row.get::<_, String>(5)?,
                row.get::<_, String>(6)?,
                row.get::<_, u32>(7)?,
                row.get::<_, String>(8)?,
                row.get::<_, String>(9)?,
            ))
        })
        .map_err(database_error)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(database_error)?
        .into_iter()
        .map(decode_outbox_row)
        .collect()
}

pub(super) fn load_pollable_outbox_rows_for_session(
    transaction: &Transaction<'_>,
    session_id: &DiscoverySessionId,
    limit: u32,
    available_at: DateTime<Utc>,
) -> CoreResult<Vec<DiscoveryOutboxEvent>> {
    let mut statement = transaction
        .prepare(
            "SELECT event.id, event.session_id, event.sequence, event.event_version,
                    event.session_revision, event.state, event.event_json,
                    event.delivery_attempts, event.available_at, event.created_at
             FROM provider_discovery_event_outbox AS event
             WHERE event.session_id = ?2
               AND event.delivered_at IS NULL
               AND event.available_at <= ?1
               AND NOT EXISTS (
                   SELECT 1
                   FROM provider_discovery_event_outbox AS earlier
                   WHERE earlier.session_id = event.session_id
                     AND earlier.delivered_at IS NULL
                     AND earlier.sequence < event.sequence
               )
             ORDER BY event.available_at, event.session_id, event.sequence
             LIMIT ?3",
        )
        .map_err(database_error)?;
    statement
        .query_map(
            params![available_at.to_rfc3339(), session_id.as_str(), limit],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, u64>(2)?,
                    row.get::<_, u32>(3)?,
                    row.get::<_, u64>(4)?,
                    row.get::<_, String>(5)?,
                    row.get::<_, String>(6)?,
                    row.get::<_, u32>(7)?,
                    row.get::<_, String>(8)?,
                    row.get::<_, String>(9)?,
                ))
            },
        )
        .map_err(database_error)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(database_error)?
        .into_iter()
        .map(decode_outbox_row)
        .collect()
}
