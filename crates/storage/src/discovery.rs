//! `SQLite` primitives for durable provider discovery.
//!
//! This module deliberately accepts already-serialized, redacted domain DTOs.
//! It never accepts credential material, HTTP headers, pasted cURL, or document
//! bodies. `database.rs` can wrap these primitives with domain mapping once the
//! public discovery API is integrated.

use std::{error::Error, fmt};

#[cfg(test)]
use lorepia_domain::discovery::SanitizedDiscoveryInput;
use lorepia_domain::discovery::{
    DiscoveryApprovalBinding, DiscoveryApprovalGrant, DiscoveryApprovalId, DiscoveryCommitPlan,
    DiscoveryCompensationKind, DiscoveryCompensationStatus, DiscoveryCompensationStep,
    DiscoveryFailure, DiscoveryReviewDiff, DiscoveryUnknownOutcomeResolution,
};
use rusqlite::{Connection, OptionalExtension, Transaction, TransactionBehavior, params};
use serde_json::Value;
use sha2::{Digest, Sha256};
use url::Url;

pub(crate) const DISCOVERY_STATE_MACHINE_MIGRATION: &str =
    include_str!("../migrations/0005_discovery_state_machine.sql");

#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg(test)]
pub struct NewDiscoverySession<'a> {
    pub id: &'a str,
    pub input: &'a SanitizedDiscoveryInput,
    pub created_at: &'a str,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DurableDiscoveryTransition<'a> {
    pub session_id: &'a str,
    pub expected_revision: u64,
    pub resulting_revision: u64,
    pub event_sequence: u64,
    pub next_event_sequence: u64,
    pub state: &'a str,
    pub draft_json: Option<&'a str>,
    pub review_diff_json: Option<&'a str>,
    pub error_json: Option<&'a str>,
    pub recovery_json: Option<&'a str>,
    pub unknown_operation: Option<&'a str>,
    pub manifest_sha256: Option<&'a str>,
    pub commit_plan_sha256: Option<&'a str>,
    pub commit_attempt_id: Option<&'a str>,
    pub committed_connection_id: Option<&'a str>,
    pub cancellation_pending: bool,
    pub event_id: &'a str,
    pub event_version: u32,
    pub event_json: &'a str,
    pub effect: DurableDiscoveryEffect,
    pub action_id: &'a str,
    pub action_kind: &'a str,
    /// Approval identifier carried by the action, when the action requires a
    /// persisted user decision.
    pub action_approval_id: Option<&'a str>,
    pub request_sha256: &'a str,
    pub response_json: &'a str,
    pub receipt_outcome: &'a str,
    pub audit_kind: &'a str,
    pub audit_summary_key: &'a str,
    pub occurred_at: &'a str,
    pub operation: Option<NewDiscoveryOperation<'a>>,
    pub completed_operation: Option<CompletedDiscoveryOperation<'a>>,
    pub approval: Option<NewDiscoveryApproval<'a>>,
    pub commit: Option<NewDiscoveryCommitAttempt<'a>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DurableDiscoveryEffect {
    None,
    ResolveKnownProvider,
    FetchDocuments,
    ExtractEvidence,
    BuildDeterministicManifestDraft,
    BuildAssistantManifestDraft,
    ValidateManifest,
    ListModels,
    ProbeCapabilities,
    CommitAtomically,
    RunCompensation,
    RequestCancellation,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DurableOperationOutcome {
    Succeeded,
    Failed,
    Interrupted,
    /// A trusted native host proved that a started persistent operation left
    /// no external effect, so recovery may durably interrupt it instead of
    /// conservatively classifying its outcome as unknown.
    AttestedNoExternalEffect,
    OutcomeUnknown,
}

impl DurableOperationOutcome {
    const fn as_str(self) -> &'static str {
        match self {
            Self::Succeeded => "succeeded",
            Self::Failed => "failed",
            Self::Interrupted | Self::AttestedNoExternalEffect => "interrupted",
            Self::OutcomeUnknown => "outcome_unknown",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompletedDiscoveryOperation<'a> {
    pub id: &'a str,
    pub outcome: DurableOperationOutcome,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewDiscoveryOperation<'a> {
    pub id: &'a str,
    pub operation_kind: &'a str,
    pub side_effect_class: &'a str,
    /// Immutable approval row authorizing this billable effect.
    pub approval_id: Option<&'a str>,
    /// SHA-256 of the exact typed `grant_json` stored in that approval row.
    pub approval_grant_sha256: Option<&'a str>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewDiscoveryApproval<'a> {
    pub id: &'a str,
    pub approval_kind: &'a str,
    pub candidate_id: Option<&'a str>,
    pub decision: &'a str,
    pub grant_json: &'a str,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewDiscoveryCommitAttempt<'a> {
    pub id: &'a str,
    pub attempt_number: u32,
    pub plan_sha256: &'a str,
    pub plan_json: &'a str,
    /// Reuse a previously prepared attempt after an explicit, reconciled restart.
    pub reuse_existing: bool,
    pub compensation_steps: &'a [NewDiscoveryCompensationStep<'a>],
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewDiscoveryCompensationStep<'a> {
    pub id: &'a str,
    pub ordinal: u32,
    pub action_id: &'a str,
    pub step_kind: &'a str,
    pub step_json: &'a str,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PersistDiscoveryTransition {
    Applied {
        revision: u64,
        event_sequence: u64,
    },
    Replayed {
        revision: u64,
        event_sequence: u64,
        response_json: String,
    },
}

#[derive(Debug)]
pub enum DiscoveryStorageError {
    Database(rusqlite::Error),
    SessionNotFound(String),
    RevisionConflict { expected: u64, actual: u64 },
    IdempotencyConflict { action_id: String },
    InvalidTransition(&'static str),
}

impl fmt::Display for DiscoveryStorageError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Database(error) => write!(formatter, "discovery database error: {error}"),
            Self::SessionNotFound(id) => write!(formatter, "discovery session not found: {id}"),
            Self::RevisionConflict { expected, actual } => write!(
                formatter,
                "discovery revision conflict: expected {expected}, current {actual}"
            ),
            Self::IdempotencyConflict { action_id } => write!(
                formatter,
                "discovery action id was reused with a different request: {action_id}"
            ),
            Self::InvalidTransition(reason) => {
                write!(formatter, "invalid durable discovery transition: {reason}")
            }
        }
    }
}

impl Error for DiscoveryStorageError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Database(error) => Some(error),
            _ => None,
        }
    }
}

impl From<rusqlite::Error> for DiscoveryStorageError {
    fn from(value: rusqlite::Error) -> Self {
        Self::Database(value)
    }
}

#[cfg(test)]
pub fn insert_discovery_session(
    connection: &mut Connection,
    session: &NewDiscoverySession<'_>,
) -> Result<(), DiscoveryStorageError> {
    session
        .input
        .validate()
        .map_err(|_| DiscoveryStorageError::InvalidTransition("invalid sanitized input"))?;
    let sanitized_input_json = serde_json::to_string(session.input).map_err(|_| {
        DiscoveryStorageError::InvalidTransition("sanitized input could not be serialized")
    })?;
    let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
    transaction.execute(
        "INSERT INTO provider_discovery_sessions (
             id,
             state,
             revision,
             next_event_sequence,
             sanitized_input_json,
             cancellation_pending,
             redaction_version,
             created_at,
             updated_at
         ) VALUES (?1, 'draft', 0, 1, ?2, 0, 1, ?3, ?3)",
        params![session.id, sanitized_input_json, session.created_at],
    )?;
    transaction.execute(
        "INSERT INTO provider_discovery_audit_log (
             session_id,
             audit_sequence,
             session_revision,
             audit_kind,
             action_id,
             subject_id,
             summary_key,
             created_at
         ) VALUES (?1, 1, 0, 'session_created', NULL, ?1,
             'discovery.audit.session_created', ?2)",
        params![session.id, session.created_at],
    )?;
    transaction.commit()?;
    Ok(())
}

#[cfg(test)]
#[allow(dead_code)] // Used when this source is included by the integration contract tests.
pub fn persist_discovery_transition(
    connection: &mut Connection,
    transition: &DurableDiscoveryTransition<'_>,
) -> Result<PersistDiscoveryTransition, DiscoveryStorageError> {
    let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let result = persist_discovery_transition_in_transaction(&transaction, transition)?;
    transaction.commit()?;
    Ok(result)
}

/// Persists the state CAS, optional commit plan, outbox event, action receipt,
/// and audit entry in the caller's transaction.
///
/// The transaction form lets `database.rs` include provider graph writes in the
/// same `SQLite` commit when the discovery reaches `committing`.
#[allow(clippy::too_many_lines)]
pub fn persist_discovery_transition_in_transaction(
    transaction: &Transaction<'_>,
    transition: &DurableDiscoveryTransition<'_>,
) -> Result<PersistDiscoveryTransition, DiscoveryStorageError> {
    validate_transition_redaction(transition)?;
    let prior_receipt = transaction
        .query_row(
            "SELECT
                 session_id,
                 request_sha256,
                 action_kind,
                 resulting_revision,
                 event_sequence,
                 response_json
             FROM provider_discovery_action_receipts
             WHERE action_id = ?1",
            [transition.action_id],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, u64>(3)?,
                    row.get::<_, u64>(4)?,
                    row.get::<_, String>(5)?,
                ))
            },
        )
        .optional()?;
    if let Some((
        session_id,
        request_sha256,
        action_kind,
        revision,
        event_sequence,
        response_json,
    )) = prior_receipt
    {
        if session_id != transition.session_id
            || request_sha256 != transition.request_sha256
            || action_kind != transition.action_kind
            || revision != transition.resulting_revision
        {
            return Err(DiscoveryStorageError::IdempotencyConflict {
                action_id: transition.action_id.to_owned(),
            });
        }
        return Ok(PersistDiscoveryTransition::Replayed {
            revision,
            event_sequence,
            response_json,
        });
    }

    validate_transition_shape(transition)?;

    let current = transaction
        .query_row(
            "SELECT
                 revision,
                 next_event_sequence,
                 state,
                 unknown_operation,
                 active_operation_id,
                 active_effect_approval_json
             FROM provider_discovery_sessions
             WHERE id = ?1",
            [transition.session_id],
            |row| {
                Ok((
                    row.get::<_, u64>(0)?,
                    row.get::<_, u64>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, Option<String>>(3)?,
                    row.get::<_, Option<String>>(4)?,
                    row.get::<_, Option<String>>(5)?,
                ))
            },
        )
        .optional()?
        .ok_or_else(|| DiscoveryStorageError::SessionNotFound(transition.session_id.to_owned()))?;
    if current.0 != transition.expected_revision {
        return Err(DiscoveryStorageError::RevisionConflict {
            expected: transition.expected_revision,
            actual: current.0,
        });
    }
    if current.1 != transition.event_sequence {
        return Err(DiscoveryStorageError::InvalidTransition(
            "event sequence does not match the session allocation",
        ));
    }
    if let Some(approval) = &transition.approval {
        validate_approval_grant_binding(transition, approval, current.3.as_deref())?;
    }
    let current_has_active_operation = current.4.is_some();
    let action_completes_operation = current_has_active_operation
        && !matches!(transition.action_kind, "cancel" | "assistant_checkpointed");
    if action_completes_operation != transition.completed_operation.is_some() {
        return Err(DiscoveryStorageError::InvalidTransition(
            "operation completion must be persisted with its session transition",
        ));
    }
    if let Some(completed_operation) = &transition.completed_operation {
        if current.4.as_deref() != Some(completed_operation.id) {
            return Err(DiscoveryStorageError::InvalidTransition(
                "operation completion must target the session's active operation",
            ));
        }
        complete_operation_in_transaction(
            transaction,
            transition,
            &current.2,
            current.4.as_deref(),
            completed_operation,
        )?;
    }

    if let Some(commit) = &transition.commit {
        insert_commit_attempt(transaction, transition, commit)?;
    }
    if let Some(approval) = &transition.approval {
        insert_approval(transaction, transition, approval)?;
    }
    if let Some(operation) = &transition.operation {
        insert_operation(transaction, transition, operation)?;
    }

    let resulting_active_operation_id = transition
        .operation
        .as_ref()
        .map(|operation| operation.id)
        .or_else(|| {
            if transition.completed_operation.is_some() {
                None
            } else {
                current.4.as_deref()
            }
        });
    let resulting_active_effect_approval_json =
        resulting_active_effect_approval_json(transition, current.5.as_deref())?;

    let changed = transaction.execute(
        "UPDATE provider_discovery_sessions
         SET
             state = ?2,
             revision = ?3,
             next_event_sequence = ?4,
             draft_json = ?5,
             review_diff_json = ?6,
             error_json = ?7,
             recovery_json = ?8,
             unknown_operation = ?9,
             manifest_sha256 = ?10,
             commit_plan_sha256 = ?11,
             commit_attempt_id = ?12,
             committed_connection_id = ?13,
             cancellation_pending = ?14,
             active_operation_id = ?15,
             active_effect_approval_json = ?16,
             updated_at = ?17
         WHERE id = ?1 AND revision = ?18 AND next_event_sequence = ?19",
        params![
            transition.session_id,
            transition.state,
            transition.resulting_revision,
            transition.next_event_sequence,
            transition.draft_json,
            transition.review_diff_json,
            transition.error_json,
            transition.recovery_json,
            transition.unknown_operation,
            transition.manifest_sha256,
            transition.commit_plan_sha256,
            transition.commit_attempt_id,
            transition.committed_connection_id,
            i64::from(transition.cancellation_pending),
            resulting_active_operation_id,
            resulting_active_effect_approval_json,
            transition.occurred_at,
            transition.expected_revision,
            transition.event_sequence,
        ],
    )?;
    if changed != 1 {
        let actual = transaction
            .query_row(
                "SELECT revision FROM provider_discovery_sessions WHERE id = ?1",
                [transition.session_id],
                |row| row.get::<_, u64>(0),
            )
            .optional()?
            .ok_or_else(|| {
                DiscoveryStorageError::SessionNotFound(transition.session_id.to_owned())
            })?;
        return Err(DiscoveryStorageError::RevisionConflict {
            expected: transition.expected_revision,
            actual,
        });
    }

    transaction.execute(
        "INSERT INTO provider_discovery_event_outbox (
             id,
             session_id,
             sequence,
             event_version,
             session_revision,
             state,
             event_json,
             redaction_version,
             delivery_attempts,
             available_at,
             delivered_at,
             created_at
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, 1, 0, ?8, NULL, ?8)",
        params![
            transition.event_id,
            transition.session_id,
            transition.event_sequence,
            transition.event_version,
            transition.resulting_revision,
            transition.state,
            transition.event_json,
            transition.occurred_at,
        ],
    )?;

    transaction.execute(
        "INSERT INTO provider_discovery_action_receipts (
             action_id,
             session_id,
             action_kind,
             request_sha256,
             expected_revision,
             resulting_revision,
             event_id,
             event_sequence,
             outcome,
             response_json,
             redaction_version,
             created_at
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, 1, ?11)",
        params![
            transition.action_id,
            transition.session_id,
            transition.action_kind,
            transition.request_sha256,
            transition.expected_revision,
            transition.resulting_revision,
            transition.event_id,
            transition.event_sequence,
            transition.receipt_outcome,
            transition.response_json,
            transition.occurred_at,
        ],
    )?;

    append_audit(
        transaction,
        transition.session_id,
        transition.resulting_revision,
        transition.audit_kind,
        Some(transition.action_id),
        Some(transition.event_id),
        transition.audit_summary_key,
        transition.occurred_at,
    )?;
    if let Some(approval) = &transition.approval {
        append_audit(
            transaction,
            transition.session_id,
            transition.resulting_revision,
            "approval_recorded",
            Some(transition.action_id),
            Some(approval.id),
            "discovery.audit.approval_recorded",
            transition.occurred_at,
        )?;
    }
    if let Some(commit) = &transition.commit {
        append_audit(
            transaction,
            transition.session_id,
            transition.resulting_revision,
            "commit_prepared",
            Some(transition.action_id),
            Some(commit.id),
            "discovery.audit.commit_prepared",
            transition.occurred_at,
        )?;
    }
    if let Some(completed_operation) = &transition.completed_operation
        && matches!(
            completed_operation.outcome,
            DurableOperationOutcome::Interrupted
                | DurableOperationOutcome::AttestedNoExternalEffect
                | DurableOperationOutcome::OutcomeUnknown
        )
    {
        append_audit(
            transaction,
            transition.session_id,
            transition.resulting_revision,
            "operation_interrupted",
            Some(transition.action_id),
            Some(completed_operation.id),
            "discovery.audit.operation_interrupted",
            transition.occurred_at,
        )?;
    }

    Ok(PersistDiscoveryTransition::Applied {
        revision: transition.resulting_revision,
        event_sequence: transition.event_sequence,
    })
}

fn validate_transition_shape(
    transition: &DurableDiscoveryTransition<'_>,
) -> Result<(), DiscoveryStorageError> {
    if transition.resulting_revision != transition.expected_revision.saturating_add(1) {
        return Err(DiscoveryStorageError::InvalidTransition(
            "resulting revision must increment exactly once",
        ));
    }
    if transition.next_event_sequence != transition.event_sequence.saturating_add(1) {
        return Err(DiscoveryStorageError::InvalidTransition(
            "next event sequence must increment exactly once",
        ));
    }
    if transition.event_version == 0 {
        return Err(DiscoveryStorageError::InvalidTransition(
            "event version must be positive",
        ));
    }
    match transition.state {
        "interrupted" if transition.recovery_json.is_none() => {
            return Err(DiscoveryStorageError::InvalidTransition(
                "interrupted transition requires a recovery checkpoint",
            ));
        }
        "unknown_outcome" if transition.unknown_operation.is_none() => {
            return Err(DiscoveryStorageError::InvalidTransition(
                "unknown outcome transition requires an operation",
            ));
        }
        _ => {}
    }
    match transition.effect {
        DurableDiscoveryEffect::CommitAtomically if transition.commit.is_none() => {
            return Err(DiscoveryStorageError::InvalidTransition(
                "committing transition must atomically prepare its commit attempt",
            ));
        }
        _ => {}
    }
    if transition.commit.is_some() && transition.effect != DurableDiscoveryEffect::CommitAtomically
    {
        return Err(DiscoveryStorageError::InvalidTransition(
            "commit attempt is only valid for an atomic commit effect",
        ));
    }
    let expected_operation = operation_kind_for_effect(transition.effect);
    if expected_operation.is_some() && transition.operation.is_none() {
        return Err(DiscoveryStorageError::InvalidTransition(
            "an external effect must atomically prepare its operation",
        ));
    }
    if let Some(operation) = &transition.operation
        && expected_operation != Some(operation.operation_kind)
    {
        return Err(DiscoveryStorageError::InvalidTransition(
            "prepared operation does not match the transition effect",
        ));
    }
    let required_approval = required_approval_for_action(transition.action_kind);
    if let Some((approval_kind, decision)) = required_approval {
        let approval =
            transition
                .approval
                .as_ref()
                .ok_or(DiscoveryStorageError::InvalidTransition(
                    "user-consent action must atomically persist its approval",
                ))?;
        if approval.approval_kind != approval_kind || approval.decision != decision {
            return Err(DiscoveryStorageError::InvalidTransition(
                "approval kind or decision does not match the action",
            ));
        }
        if transition.action_approval_id != Some(approval.id) {
            return Err(DiscoveryStorageError::InvalidTransition(
                "action approval identifier does not match the persisted approval",
            ));
        }
    } else if transition.approval.is_some() {
        return Err(DiscoveryStorageError::InvalidTransition(
            "action does not accept an approval record",
        ));
    } else if transition.action_approval_id.is_some() {
        return Err(DiscoveryStorageError::InvalidTransition(
            "action does not accept an approval identifier",
        ));
    }
    Ok(())
}

const MAX_PERSISTED_JSON_BYTES: usize = 1_048_576;
const MAX_PERSISTED_JSON_DEPTH: usize = 64;

fn validate_transition_redaction(
    transition: &DurableDiscoveryTransition<'_>,
) -> Result<(), DiscoveryStorageError> {
    for json in [
        transition.draft_json,
        transition.review_diff_json,
        transition.error_json,
        transition.recovery_json,
        Some(transition.event_json),
        Some(transition.response_json),
    ]
    .into_iter()
    .flatten()
    {
        validate_redacted_json_object(json)?;
    }
    if let Some(approval) = &transition.approval {
        validate_redacted_json_object(approval.grant_json)?;
    }
    let failure = transition
        .error_json
        .map(parse_discovery_failure)
        .transpose()?;
    if matches!(
        transition.action_kind,
        "fail" | "commit_failed_before_apply" | "compensation_failed"
    ) && failure.is_none()
    {
        return Err(DiscoveryStorageError::InvalidTransition(
            "failure action must persist its typed redacted failure",
        ));
    }
    let event = validate_redacted_json_object(transition.event_json)?;
    let event_failure = event.get("failure").filter(|value| !value.is_null());
    match (failure.as_ref(), event_failure) {
        (Some(failure), Some(event_failure))
            if serde_json::to_value(failure).ok().as_ref() == Some(event_failure) => {}
        (None, None) => {}
        _ => {
            return Err(DiscoveryStorageError::InvalidTransition(
                "event failure must exactly match the durable session failure",
            ));
        }
    }
    if let Some(commit) = &transition.commit {
        validate_redacted_json_object(commit.plan_json)?;
        if sha256_hex(commit.plan_json.as_bytes()) != commit.plan_sha256 {
            return Err(DiscoveryStorageError::InvalidTransition(
                "commit plan hash does not match the redacted plan",
            ));
        }
        validate_typed_commit_recipe(transition, commit)?;
    }
    Ok(())
}

fn validate_typed_commit_recipe(
    transition: &DurableDiscoveryTransition<'_>,
    commit: &NewDiscoveryCommitAttempt<'_>,
) -> Result<(), DiscoveryStorageError> {
    let plan = serde_json::from_str::<DiscoveryCommitPlan>(commit.plan_json).map_err(|_| {
        DiscoveryStorageError::InvalidTransition("commit plan must use the typed domain contract")
    })?;
    plan.validate().map_err(|_| {
        DiscoveryStorageError::InvalidTransition("commit plan violates its typed domain contract")
    })?;
    if plan.attempt_id.as_str() != commit.id
        || plan.session_id.as_str() != transition.session_id
        || (!commit.reuse_existing && plan.expected_revision != transition.expected_revision)
        || (commit.reuse_existing && plan.expected_revision > transition.expected_revision)
    {
        return Err(DiscoveryStorageError::InvalidTransition(
            "commit plan must bind its attempt, session, and revision",
        ));
    }
    if commit.reuse_existing {
        return Ok(());
    }

    let mut ordinals = std::collections::BTreeSet::new();
    let mut credential_steps = 0_usize;
    let mut graph_steps = 0_usize;
    let mut selection_steps = 0_usize;
    for step in commit.compensation_steps {
        validate_redacted_json_object(step.step_json)?;
        let typed =
            serde_json::from_str::<DiscoveryCompensationStep>(step.step_json).map_err(|_| {
                DiscoveryStorageError::InvalidTransition(
                    "compensation step must use the typed domain contract",
                )
            })?;
        typed.validate_against(&plan).map_err(|_| {
            DiscoveryStorageError::InvalidTransition(
                "compensation target does not match the approved commit plan",
            )
        })?;
        let typed_kind = serde_json::to_value(typed.kind)
            .ok()
            .and_then(|value| value.as_str().map(str::to_owned));
        if typed.action_id.as_str() != step.action_id
            || typed.ordinal != step.ordinal
            || typed_kind.as_deref() != Some(step.step_kind)
            || typed.status != DiscoveryCompensationStatus::Pending
            || !ordinals.insert(typed.ordinal)
        {
            return Err(DiscoveryStorageError::InvalidTransition(
                "compensation columns must exactly match a unique pending typed step",
            ));
        }
        match typed.kind {
            DiscoveryCompensationKind::RemoveCredentialSlot => credential_steps += 1,
            DiscoveryCompensationKind::RemoveConnectionGraph => graph_steps += 1,
            DiscoveryCompensationKind::RestorePreviousSelection => selection_steps += 1,
        }
    }
    let expected_ordinals = (0..u32::try_from(commit.compensation_steps.len()).map_err(|_| {
        DiscoveryStorageError::InvalidTransition("compensation recipe exceeds the ordinal bound")
    })?)
        .collect::<std::collections::BTreeSet<_>>();
    if ordinals != expected_ordinals
        || credential_steps != usize::from(plan.credential_ref.is_some())
        || graph_steps != 1
        || selection_steps != 1
    {
        return Err(DiscoveryStorageError::InvalidTransition(
            "commit requires one exact graph and selection reversal plus any credential reversal",
        ));
    }
    Ok(())
}

fn parse_discovery_failure(json: &str) -> Result<DiscoveryFailure, DiscoveryStorageError> {
    let failure = serde_json::from_str::<DiscoveryFailure>(json).map_err(|_| {
        DiscoveryStorageError::InvalidTransition(
            "error JSON must be a typed redacted discovery failure",
        )
    })?;
    failure.validate().map_err(|_| {
        DiscoveryStorageError::InvalidTransition(
            "error JSON must be a valid redacted discovery failure",
        )
    })?;
    Ok(failure)
}

fn resulting_active_effect_approval_json(
    transition: &DurableDiscoveryTransition<'_>,
    current_json: Option<&str>,
) -> Result<Option<String>, DiscoveryStorageError> {
    let current = current_json.map(parse_active_effect_approval).transpose()?;
    if let Some(operation) = &transition.operation {
        return match (operation.approval_id, operation.approval_grant_sha256) {
            (Some(approval_id), Some(grant_sha256)) => {
                let binding = DiscoveryApprovalBinding {
                    approval_id: DiscoveryApprovalId::parse(approval_id).map_err(|_| {
                        DiscoveryStorageError::InvalidTransition(
                            "active effect approval id is invalid",
                        )
                    })?,
                    grant_sha256: grant_sha256.to_owned(),
                };
                binding.validate().map_err(|_| {
                    DiscoveryStorageError::InvalidTransition(
                        "active effect approval hash is invalid",
                    )
                })?;
                serde_json::to_string(&binding).map(Some).map_err(|_| {
                    DiscoveryStorageError::InvalidTransition(
                        "active effect approval could not be serialized",
                    )
                })
            }
            (None, None) => Ok(None),
            _ => Err(DiscoveryStorageError::InvalidTransition(
                "active effect approval binding is incomplete",
            )),
        };
    }
    if !matches!(
        transition.state,
        "building_assistant_manifest_draft"
            | "probing_capabilities"
            | "interrupted"
            | "unknown_outcome"
    ) {
        return Ok(None);
    }
    current
        .map(|binding| {
            serde_json::to_string(&binding).map_err(|_| {
                DiscoveryStorageError::InvalidTransition(
                    "active effect approval could not be serialized",
                )
            })
        })
        .transpose()
}

fn parse_active_effect_approval(
    json: &str,
) -> Result<DiscoveryApprovalBinding, DiscoveryStorageError> {
    let binding = serde_json::from_str::<DiscoveryApprovalBinding>(json).map_err(|_| {
        DiscoveryStorageError::InvalidTransition(
            "active effect approval JSON is not a typed binding",
        )
    })?;
    binding.validate().map_err(|_| {
        DiscoveryStorageError::InvalidTransition("active effect approval binding is invalid")
    })?;
    let canonical = serde_json::to_string(&binding).map_err(|_| {
        DiscoveryStorageError::InvalidTransition(
            "active effect approval could not be canonicalized",
        )
    })?;
    if canonical != json {
        return Err(DiscoveryStorageError::InvalidTransition(
            "active effect approval JSON must be canonical",
        ));
    }
    Ok(binding)
}

fn validate_redacted_json_object(json: &str) -> Result<Value, DiscoveryStorageError> {
    if json.len() > MAX_PERSISTED_JSON_BYTES {
        return Err(DiscoveryStorageError::InvalidTransition(
            "redacted JSON exceeds the persistence limit",
        ));
    }
    let value = serde_json::from_str::<Value>(json).map_err(|_| {
        DiscoveryStorageError::InvalidTransition("persisted redacted JSON is invalid")
    })?;
    if !value.is_object() {
        return Err(DiscoveryStorageError::InvalidTransition(
            "persisted redacted JSON must be an object",
        ));
    }
    validate_redacted_value(&value, 0)?;
    Ok(value)
}

fn validate_redacted_value(value: &Value, depth: usize) -> Result<(), DiscoveryStorageError> {
    if depth > MAX_PERSISTED_JSON_DEPTH {
        return Err(DiscoveryStorageError::InvalidTransition(
            "persisted redacted JSON is too deeply nested",
        ));
    }
    match value {
        Value::Object(object) => {
            for (key, child) in object {
                let normalized = key
                    .bytes()
                    .filter(u8::is_ascii_alphanumeric)
                    .map(|byte| byte.to_ascii_lowercase())
                    .collect::<Vec<_>>();
                if matches!(
                    normalized.as_slice(),
                    b"apikey"
                        | b"authorization"
                        | b"proxyauthorization"
                        | b"cookie"
                        | b"setcookie"
                        | b"password"
                        | b"secret"
                        | b"accesstoken"
                        | b"refreshtoken"
                        | b"credentialvalue"
                        | b"rawcredential"
                        | b"requestheaders"
                        | b"responseheaders"
                        | b"documentbody"
                        | b"rawdocument"
                        | b"pastedcurl"
                ) {
                    return Err(DiscoveryStorageError::InvalidTransition(
                        "persisted JSON contains a forbidden sensitive field",
                    ));
                }
                if normalized.as_slice() == b"sourceurl"
                    && let Some(source_url) = child.as_str()
                {
                    let parsed = Url::parse(source_url).map_err(|_| {
                        DiscoveryStorageError::InvalidTransition(
                            "persisted source URL must be an absolute URL",
                        )
                    })?;
                    if parsed.query().is_some() || parsed.fragment().is_some() {
                        return Err(DiscoveryStorageError::InvalidTransition(
                            "persisted source URL must not contain a query or fragment",
                        ));
                    }
                }
                validate_redacted_value(child, depth + 1)?;
            }
        }
        Value::Array(array) => {
            for child in array {
                validate_redacted_value(child, depth + 1)?;
            }
        }
        _ => {}
    }
    Ok(())
}

#[allow(clippy::too_many_lines)]
fn validate_approval_grant_binding(
    transition: &DurableDiscoveryTransition<'_>,
    approval: &NewDiscoveryApproval<'_>,
    previous_unknown_operation: Option<&str>,
) -> Result<(), DiscoveryStorageError> {
    validate_redacted_json_object(approval.grant_json)?;
    let grant =
        serde_json::from_str::<DiscoveryApprovalGrant>(approval.grant_json).map_err(|_| {
            DiscoveryStorageError::InvalidTransition(
                "approval grant must use the typed grant contract",
            )
        })?;
    grant.validate().map_err(|_| {
        DiscoveryStorageError::InvalidTransition("approval grant violates its typed bounds")
    })?;
    let canonical = serde_json::to_string(&grant).map_err(|_| {
        DiscoveryStorageError::InvalidTransition("approval grant could not be canonicalized")
    })?;
    if canonical != approval.grant_json {
        return Err(DiscoveryStorageError::InvalidTransition(
            "approval grant must use canonical JSON before hashing",
        ));
    }

    match (approval.approval_kind, &grant) {
        ("template_selection", DiscoveryApprovalGrant::TemplateSelection { candidate_id }) => {
            if approval.candidate_id != Some(candidate_id.as_str()) {
                return Err(DiscoveryStorageError::InvalidTransition(
                    "template approval must bind the selected candidate",
                ));
            }
        }
        ("assistant_consent", DiscoveryApprovalGrant::AssistantConsent { .. })
        | ("capability_probe", DiscoveryApprovalGrant::CapabilityProbe { .. }) => {}
        (
            "credential_origin",
            DiscoveryApprovalGrant::CredentialOrigin {
                manifest_sha256, ..
            },
        ) => {
            if transition.manifest_sha256 != Some(manifest_sha256.as_str()) {
                return Err(DiscoveryStorageError::InvalidTransition(
                    "credential-origin approval must bind the validated manifest",
                ));
            }
        }
        (
            "review",
            DiscoveryApprovalGrant::Review {
                review_sha256,
                graph_sha256,
            },
        ) => {
            let persisted_review = transition
                .review_diff_json
                .ok_or(DiscoveryStorageError::InvalidTransition(
                    "review approval requires the persisted review diff",
                ))
                .and_then(|json| {
                    serde_json::from_str::<DiscoveryReviewDiff>(json).map_err(|_| {
                        DiscoveryStorageError::InvalidTransition(
                            "persisted review diff must use the typed contract",
                        )
                    })
                })?;
            persisted_review.validate().map_err(|_| {
                DiscoveryStorageError::InvalidTransition(
                    "persisted review diff violates its typed bounds",
                )
            })?;
            if persisted_review.sha256.as_str() != review_sha256.as_str() {
                return Err(DiscoveryStorageError::InvalidTransition(
                    "review approval must bind the persisted review diff",
                ));
            }
            if persisted_review.graph_sha256.as_str() != graph_sha256.as_str() {
                return Err(DiscoveryStorageError::InvalidTransition(
                    "review approval must bind the persisted provider graph",
                ));
            }
        }
        (
            "unknown_outcome_resolution",
            DiscoveryApprovalGrant::UnknownOutcomeResolution {
                operation,
                resolution,
            },
        ) => {
            let operation = serde_json::to_value(operation)
                .ok()
                .and_then(|value| value.as_str().map(str::to_owned));
            if operation.as_deref() != previous_unknown_operation {
                return Err(DiscoveryStorageError::InvalidTransition(
                    "unknown-outcome approval must bind the prior unknown operation",
                ));
            }
            let result_matches = match resolution {
                DiscoveryUnknownOutcomeResolution::ConfirmedNoEffect => {
                    matches!(transition.state, "interrupted" | "cancelled")
                }
                DiscoveryUnknownOutcomeResolution::ConfirmedCommitCompleted { connection_id } => {
                    (transition.state == "ready"
                        && transition.committed_connection_id == Some(connection_id.as_str()))
                        || transition.state == "compensating"
                }
                DiscoveryUnknownOutcomeResolution::ConfirmedCompensated
                | DiscoveryUnknownOutcomeResolution::ManuallyReconciledAsFailed => {
                    matches!(transition.state, "failed" | "cancelled")
                }
            };
            if !result_matches {
                return Err(DiscoveryStorageError::InvalidTransition(
                    "unknown-outcome approval does not match the resulting transition",
                ));
            }
        }
        _ => {
            return Err(DiscoveryStorageError::InvalidTransition(
                "approval kind does not match its typed grant",
            ));
        }
    }
    Ok(())
}

fn sha256_hex(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}

fn required_approval_for_action(action_kind: &str) -> Option<(&'static str, &'static str)> {
    match action_kind {
        "select_template" => Some(("template_selection", "approved")),
        "approve_assistant" => Some(("assistant_consent", "approved")),
        "decline_assistant" => Some(("assistant_consent", "rejected")),
        "approve_credential_origin" => Some(("credential_origin", "approved")),
        "approve_probes" => Some(("capability_probe", "approved")),
        "skip_probes" => Some(("capability_probe", "rejected")),
        "approve_review" => Some(("review", "approved")),
        "resolve_unknown_outcome" => Some(("unknown_outcome_resolution", "approved")),
        _ => None,
    }
}

fn operation_kind_for_effect(effect: DurableDiscoveryEffect) -> Option<&'static str> {
    match effect {
        DurableDiscoveryEffect::ResolveKnownProvider => Some("resolve_known_provider"),
        DurableDiscoveryEffect::FetchDocuments => Some("fetch_documents"),
        DurableDiscoveryEffect::ExtractEvidence => Some("extract_evidence"),
        DurableDiscoveryEffect::BuildDeterministicManifestDraft => {
            Some("build_deterministic_manifest_draft")
        }
        DurableDiscoveryEffect::BuildAssistantManifestDraft => {
            Some("build_assistant_manifest_draft")
        }
        DurableDiscoveryEffect::ValidateManifest => Some("validate_manifest"),
        DurableDiscoveryEffect::ListModels => Some("list_models"),
        DurableDiscoveryEffect::ProbeCapabilities => Some("probe_capabilities"),
        DurableDiscoveryEffect::CommitAtomically => Some("atomic_commit"),
        DurableDiscoveryEffect::RunCompensation => Some("compensation"),
        DurableDiscoveryEffect::None | DurableDiscoveryEffect::RequestCancellation => None,
    }
}

fn operation_kind_for_state(state: &str) -> Option<&'static str> {
    match state {
        "resolving_known_provider" => Some("resolve_known_provider"),
        "fetching_documents" => Some("fetch_documents"),
        "extracting_evidence" => Some("extract_evidence"),
        "building_deterministic_manifest_draft" => Some("build_deterministic_manifest_draft"),
        "building_assistant_manifest_draft" => Some("build_assistant_manifest_draft"),
        "validating_manifest" => Some("validate_manifest"),
        "listing_models" => Some("list_models"),
        "probing_capabilities" => Some("probe_capabilities"),
        "committing" => Some("atomic_commit"),
        "compensating" => Some("compensation"),
        _ => None,
    }
}

fn expected_side_effect_class(operation_kind: &str) -> Option<&'static str> {
    match operation_kind {
        "build_deterministic_manifest_draft" => Some("local_deterministic"),
        "resolve_known_provider"
        | "fetch_documents"
        | "extract_evidence"
        | "validate_manifest"
        | "list_models" => Some("read_only"),
        "build_assistant_manifest_draft" | "probe_capabilities" => Some("billable_external"),
        "atomic_commit" | "compensation" => Some("persistent"),
        _ => None,
    }
}

#[allow(clippy::too_many_lines)]
fn insert_operation(
    transaction: &Transaction<'_>,
    transition: &DurableDiscoveryTransition<'_>,
    operation: &NewDiscoveryOperation<'_>,
) -> Result<(), DiscoveryStorageError> {
    if expected_side_effect_class(operation.operation_kind) != Some(operation.side_effect_class) {
        return Err(DiscoveryStorageError::InvalidTransition(
            "operation side-effect class does not match its kind",
        ));
    }
    let required_approval_kind = match operation.operation_kind {
        "build_assistant_manifest_draft" => Some("assistant_consent"),
        "probe_capabilities" => Some("capability_probe"),
        _ => None,
    };
    match (
        required_approval_kind,
        operation.approval_id,
        operation.approval_grant_sha256,
    ) {
        (Some(approval_kind), Some(approval_id), Some(grant_sha256)) => {
            let approval = transaction
                .query_row(
                    "SELECT approval_kind, decision, grant_json, grant_sha256
                     FROM provider_discovery_approvals
                     WHERE id = ?1 AND session_id = ?2",
                    params![approval_id, transition.session_id],
                    |row| {
                        Ok((
                            row.get::<_, String>(0)?,
                            row.get::<_, String>(1)?,
                            row.get::<_, String>(2)?,
                            row.get::<_, String>(3)?,
                        ))
                    },
                )
                .optional()?
                .ok_or(DiscoveryStorageError::InvalidTransition(
                    "billable operation approval does not exist in this session",
                ))?;
            if approval.0 != approval_kind
                || approval.1 != "approved"
                || approval.3 != grant_sha256
                || sha256_hex(approval.2.as_bytes()) != grant_sha256
            {
                return Err(DiscoveryStorageError::InvalidTransition(
                    "billable operation is not bound to the exact approved grant",
                ));
            }
            let typed =
                serde_json::from_str::<DiscoveryApprovalGrant>(&approval.2).map_err(|_| {
                    DiscoveryStorageError::InvalidTransition(
                        "billable operation approval grant is not typed",
                    )
                })?;
            let typed_kind_matches = matches!(
                (approval_kind, &typed),
                (
                    "assistant_consent",
                    DiscoveryApprovalGrant::AssistantConsent { .. }
                ) | (
                    "capability_probe",
                    DiscoveryApprovalGrant::CapabilityProbe { .. }
                )
            );
            let canonical_grant = serde_json::to_string(&typed).ok();
            if !typed_kind_matches
                || typed.validate().is_err()
                || canonical_grant.as_deref() != Some(approval.2.as_str())
            {
                return Err(DiscoveryStorageError::InvalidTransition(
                    "billable operation approval grant does not match its effect",
                ));
            }
        }
        (Some(_), _, _) => {
            return Err(DiscoveryStorageError::InvalidTransition(
                "billable operation must bind an approval id and grant hash",
            ));
        }
        (None, None, None) => {}
        (None, _, _) => {
            return Err(DiscoveryStorageError::InvalidTransition(
                "non-billable operation cannot carry a billable approval binding",
            ));
        }
    }
    transaction.execute(
        "INSERT INTO provider_discovery_operations (
             id,
             session_id,
             operation_kind,
             side_effect_class,
             status,
             action_id,
             expected_revision,
             request_sha256,
             approval_id,
             approval_grant_sha256,
             started_at,
             finished_at,
             created_at,
             updated_at
         ) VALUES (
             ?1, ?2, ?3, ?4, 'prepared', ?5, ?6, ?7, ?8, ?9,
             NULL, NULL, ?10, ?10
         )",
        params![
            operation.id,
            transition.session_id,
            operation.operation_kind,
            operation.side_effect_class,
            transition.action_id,
            transition.resulting_revision,
            transition.request_sha256,
            operation.approval_id,
            operation.approval_grant_sha256,
            transition.occurred_at,
        ],
    )?;
    Ok(())
}

fn insert_approval(
    transaction: &Transaction<'_>,
    transition: &DurableDiscoveryTransition<'_>,
    approval: &NewDiscoveryApproval<'_>,
) -> Result<(), DiscoveryStorageError> {
    let grant_sha256 = sha256_hex(approval.grant_json.as_bytes());
    transaction.execute(
        "INSERT INTO provider_discovery_approvals (
             id,
             session_id,
             approval_kind,
             candidate_id,
             decision,
             grant_json,
             session_revision,
             grant_sha256,
             redaction_version,
             created_at
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, 1, ?9)",
        params![
            approval.id,
            transition.session_id,
            approval.approval_kind,
            approval.candidate_id,
            approval.decision,
            approval.grant_json,
            transition.expected_revision,
            grant_sha256,
            transition.occurred_at,
        ],
    )?;
    Ok(())
}

#[allow(clippy::too_many_lines)]
fn insert_commit_attempt(
    transaction: &Transaction<'_>,
    transition: &DurableDiscoveryTransition<'_>,
    commit: &NewDiscoveryCommitAttempt<'_>,
) -> Result<(), DiscoveryStorageError> {
    if transition.commit_attempt_id != Some(commit.id)
        || transition.commit_plan_sha256 != Some(commit.plan_sha256)
    {
        return Err(DiscoveryStorageError::InvalidTransition(
            "commit attempt must match the session transition",
        ));
    }
    if commit.reuse_existing {
        if !commit.compensation_steps.is_empty() {
            return Err(DiscoveryStorageError::InvalidTransition(
                "reused commit attempt cannot replace compensation steps",
            ));
        }
        let existing = transaction
            .query_row(
                "SELECT
                     session_id,
                     attempt_number,
                     plan_sha256,
                     plan_json,
                     phase
                 FROM provider_discovery_commit_attempts
                 WHERE id = ?1",
                [commit.id],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, u32>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, String>(3)?,
                        row.get::<_, String>(4)?,
                    ))
                },
            )
            .optional()?
            .ok_or(DiscoveryStorageError::InvalidTransition(
                "commit attempt to restart does not exist",
            ))?;
        if existing.0 != transition.session_id
            || existing.1 != commit.attempt_number
            || existing.2 != commit.plan_sha256
            || existing.3 != commit.plan_json
            || existing.4 != "prepared"
        {
            return Err(DiscoveryStorageError::InvalidTransition(
                "commit restart does not match the prepared attempt",
            ));
        }
        return Ok(());
    }
    transaction.execute(
        "INSERT INTO provider_discovery_commit_attempts (
             id,
             session_id,
             attempt_number,
             action_id,
             expected_revision,
             plan_sha256,
             plan_json,
             phase,
             redaction_version,
             created_at,
             updated_at,
             completed_at
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, 'prepared', 1, ?8, ?8, NULL)",
        params![
            commit.id,
            transition.session_id,
            commit.attempt_number,
            transition.action_id,
            transition.expected_revision,
            commit.plan_sha256,
            commit.plan_json,
            transition.occurred_at,
        ],
    )?;
    for step in commit.compensation_steps {
        transaction.execute(
            "INSERT INTO provider_discovery_compensation_steps (
                 id,
                 commit_attempt_id,
                 ordinal,
                 action_id,
                 step_kind,
                 step_json,
                 status,
                 attempt_count,
                 last_failure_json,
                 redaction_version,
                 created_at,
                 updated_at,
                 completed_at
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, 'pending', 0, NULL, 1, ?7, ?7, NULL)",
            params![
                step.id,
                commit.id,
                step.ordinal,
                step.action_id,
                step.step_kind,
                step.step_json,
                transition.occurred_at,
            ],
        )?;
    }
    Ok(())
}

fn complete_operation_in_transaction(
    transaction: &Transaction<'_>,
    transition: &DurableDiscoveryTransition<'_>,
    current_state: &str,
    active_operation_id: Option<&str>,
    completed: &CompletedDiscoveryOperation<'_>,
) -> Result<(), DiscoveryStorageError> {
    let expected_kind = operation_kind_for_state(current_state).ok_or(
        DiscoveryStorageError::InvalidTransition("session state has no operation to complete"),
    )?;
    let operation = transaction
        .query_row(
            "SELECT
                 session_id,
                 operation_kind,
                 side_effect_class,
                 status
             FROM provider_discovery_operations
             WHERE id = ?1",
            [completed.id],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                ))
            },
        )
        .optional()?
        .ok_or(DiscoveryStorageError::InvalidTransition(
            "operation to complete does not exist",
        ))?;
    if active_operation_id != Some(completed.id)
        || operation.0 != transition.session_id
        || operation.1 != expected_kind
    {
        return Err(DiscoveryStorageError::InvalidTransition(
            "operation does not match the active session operation and state",
        ));
    }

    let allowed = match completed.outcome {
        DurableOperationOutcome::Succeeded | DurableOperationOutcome::Failed => {
            operation.3 == "started"
        }
        DurableOperationOutcome::Interrupted => {
            operation.3 == "prepared"
                || (operation.3 == "started"
                    && matches!(operation.2.as_str(), "local_deterministic" | "read_only"))
        }
        DurableOperationOutcome::AttestedNoExternalEffect => {
            operation.1 == "atomic_commit"
                && operation.2 == "persistent"
                && operation.3 == "started"
        }
        DurableOperationOutcome::OutcomeUnknown => operation.3 == "started",
    };
    if !allowed {
        return Err(DiscoveryStorageError::InvalidTransition(
            "operation status or side-effect class cannot produce that outcome",
        ));
    }

    let changed = transaction.execute(
        "UPDATE provider_discovery_operations
         SET
             status = ?2,
             started_at = CASE
                 WHEN ?2 = 'interrupted' AND status = 'prepared'
                     THEN COALESCE(started_at, ?3)
                 ELSE started_at
             END,
             finished_at = ?3,
             updated_at = ?3
         WHERE id = ?1 AND status = ?4",
        params![
            completed.id,
            completed.outcome.as_str(),
            transition.occurred_at,
            operation.3,
        ],
    )?;
    if changed != 1 {
        return Err(DiscoveryStorageError::InvalidTransition(
            "operation changed concurrently before completion",
        ));
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn append_audit(
    transaction: &Transaction<'_>,
    session_id: &str,
    session_revision: u64,
    audit_kind: &str,
    action_id: Option<&str>,
    subject_id: Option<&str>,
    summary_key: &str,
    created_at: &str,
) -> Result<(), DiscoveryStorageError> {
    let next_audit_sequence = transaction.query_row(
        "SELECT COALESCE(MAX(audit_sequence), 0) + 1
         FROM provider_discovery_audit_log
         WHERE session_id = ?1",
        [session_id],
        |row| row.get::<_, u64>(0),
    )?;
    transaction.execute(
        "INSERT INTO provider_discovery_audit_log (
             session_id,
             audit_sequence,
             session_revision,
             audit_kind,
             action_id,
             subject_id,
             summary_key,
             created_at
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
        params![
            session_id,
            next_audit_sequence,
            session_revision,
            audit_kind,
            action_id,
            subject_id,
            summary_key,
            created_at,
        ],
    )?;
    Ok(())
}

pub fn mark_discovery_operation_started(
    connection: &mut Connection,
    operation_id: &str,
    started_at: &str,
) -> Result<bool, DiscoveryStorageError> {
    let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let changed = transaction.execute(
        "UPDATE provider_discovery_operations
         SET status = 'started', started_at = ?2, updated_at = ?2
         WHERE id = ?1
           AND status = 'prepared'
           AND EXISTS (
               SELECT 1
               FROM provider_discovery_sessions AS session
               WHERE session.id = provider_discovery_operations.session_id
                 AND session.active_operation_id =
                     provider_discovery_operations.id
                 AND (
                     session.cancellation_pending = 0
                     OR (
                         provider_discovery_operations.operation_kind =
                             'compensation'
                         AND session.state = 'compensating'
                     )
                 )
                 AND (
                     (provider_discovery_operations.operation_kind =
                         'resolve_known_provider'
                         AND session.state = 'resolving_known_provider')
                     OR (provider_discovery_operations.operation_kind =
                         'fetch_documents'
                         AND session.state = 'fetching_documents')
                     OR (provider_discovery_operations.operation_kind =
                         'extract_evidence'
                         AND session.state = 'extracting_evidence')
                     OR (provider_discovery_operations.operation_kind =
                         'build_deterministic_manifest_draft'
                         AND session.state =
                             'building_deterministic_manifest_draft')
                     OR (provider_discovery_operations.operation_kind =
                         'build_assistant_manifest_draft'
                         AND session.state =
                             'building_assistant_manifest_draft')
                     OR (provider_discovery_operations.operation_kind =
                         'validate_manifest'
                         AND session.state = 'validating_manifest')
                     OR (provider_discovery_operations.operation_kind =
                         'list_models'
                         AND session.state = 'listing_models')
                     OR (provider_discovery_operations.operation_kind =
                         'probe_capabilities'
                         AND session.state = 'probing_capabilities')
                     OR (provider_discovery_operations.operation_kind =
                         'atomic_commit'
                         AND session.state = 'committing')
                     OR (provider_discovery_operations.operation_kind =
                         'compensation'
                         AND session.state = 'compensating')
                 )
           )",
        params![operation_id, started_at],
    )?;
    if changed == 1 {
        let (session_id, session_revision, action_id, operation_kind) = transaction.query_row(
            "SELECT session_id, expected_revision, action_id, operation_kind
             FROM provider_discovery_operations
             WHERE id = ?1",
            [operation_id],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, u64>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                ))
            },
        )?;
        if operation_kind == "compensation" {
            mark_compensation_commit_started(&transaction, &session_id, started_at)?;
        }
        append_audit(
            &transaction,
            &session_id,
            session_revision,
            "operation_started",
            Some(&action_id),
            Some(operation_id),
            "discovery.audit.operation_started",
            started_at,
        )?;
    }
    transaction.commit()?;
    Ok(changed == 1)
}

fn mark_compensation_commit_started(
    transaction: &Transaction<'_>,
    session_id: &str,
    started_at: &str,
) -> Result<(), DiscoveryStorageError> {
    let phase_changed = transaction.execute(
        "UPDATE provider_discovery_commit_attempts
         SET phase = 'compensating', updated_at = ?2, completed_at = NULL
         WHERE id = (
             SELECT commit_attempt_id
             FROM provider_discovery_sessions
             WHERE id = ?1 AND state = 'compensating'
         )
           AND phase IN ('compensation_required', 'compensating')",
        params![session_id, started_at],
    )?;
    if phase_changed != 1 {
        return Err(DiscoveryStorageError::InvalidTransition(
            "compensation operation cannot start outside its durable phase",
        ));
    }
    Ok(())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiscoveryRecoveryDisposition {
    MarkInterrupted,
    MarkUnknownOutcome,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnfinishedDiscoveryOperation {
    pub id: String,
    pub session_id: String,
    pub operation_kind: String,
    pub status: String,
    pub disposition: DiscoveryRecoveryDisposition,
}

/// Returns crash recovery work without mutating or replaying it.
///
/// The caller must create a versioned `Interrupt` action and pass it through
/// the same CAS/outbox path. This read-only scan is what prevents startup from
/// silently retrying unknown native or billable side effects.
pub fn list_unfinished_discovery_operations(
    connection: &Connection,
) -> Result<Vec<UnfinishedDiscoveryOperation>, DiscoveryStorageError> {
    let mut statement = connection.prepare(
        "SELECT id, session_id, operation_kind, side_effect_class, status
         FROM provider_discovery_operations
         WHERE status IN ('prepared', 'started')
         ORDER BY created_at, id",
    )?;
    let rows = statement.query_map([], |row| {
        let side_effect_class = row.get::<_, String>(3)?;
        let status = row.get::<_, String>(4)?;
        let disposition = if status == "prepared"
            || matches!(
                side_effect_class.as_str(),
                "local_deterministic" | "read_only"
            ) {
            DiscoveryRecoveryDisposition::MarkInterrupted
        } else {
            DiscoveryRecoveryDisposition::MarkUnknownOutcome
        };
        Ok(UnfinishedDiscoveryOperation {
            id: row.get(0)?,
            session_id: row.get(1)?,
            operation_kind: row.get(2)?,
            status,
            disposition,
        })
    })?;
    rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
}

/// Reloads the stable redacted failure stored on a discovery session.
///
/// This deliberately parses the durable typed contract so callers never need
/// to scrape a transient log or expose a raw provider response after restart.
#[cfg(test)]
#[allow(dead_code)] // Used when this source is included by the integration contract tests.
pub fn load_discovery_failure(
    connection: &Connection,
    session_id: &str,
) -> Result<Option<DiscoveryFailure>, DiscoveryStorageError> {
    let error_json = connection
        .query_row(
            "SELECT error_json
             FROM provider_discovery_sessions
             WHERE id = ?1",
            [session_id],
            |row| row.get::<_, Option<String>>(0),
        )
        .optional()?
        .ok_or_else(|| DiscoveryStorageError::SessionNotFound(session_id.to_owned()))?;
    error_json
        .as_deref()
        .map(parse_discovery_failure)
        .transpose()
}

#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg(test)]
#[allow(dead_code)] // Used when this source is included by the integration contract tests.
pub struct PendingDiscoveryEvent {
    pub id: String,
    pub session_id: String,
    pub sequence: u64,
    pub event_json: String,
}

#[cfg(test)]
#[allow(dead_code)] // Used when this source is included by the integration contract tests.
pub fn list_pending_discovery_events(
    connection: &Connection,
    limit: u32,
) -> Result<Vec<PendingDiscoveryEvent>, DiscoveryStorageError> {
    if limit == 0 || limit > 1_000 {
        return Err(DiscoveryStorageError::InvalidTransition(
            "outbox batch limit must be 1..=1000",
        ));
    }
    let mut statement = connection.prepare(
        "SELECT id, session_id, sequence, event_json
         FROM provider_discovery_event_outbox
         WHERE delivered_at IS NULL
         ORDER BY available_at, session_id, sequence
         LIMIT ?1",
    )?;
    let rows = statement.query_map([limit], |row| {
        Ok(PendingDiscoveryEvent {
            id: row.get(0)?,
            session_id: row.get(1)?,
            sequence: row.get(2)?,
            event_json: row.get(3)?,
        })
    })?;
    rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
}

#[cfg(test)]
#[allow(dead_code)] // Used when this source is included by the integration contract tests.
pub fn mark_discovery_event_delivered(
    connection: &Connection,
    event_id: &str,
    delivered_at: &str,
) -> Result<bool, DiscoveryStorageError> {
    let changed = connection.execute(
        "UPDATE provider_discovery_event_outbox
         SET delivered_at = ?2, delivery_attempts = delivery_attempts + 1
         WHERE id = ?1 AND delivered_at IS NULL",
        params![event_id, delivered_at],
    )?;
    Ok(changed == 1)
}
