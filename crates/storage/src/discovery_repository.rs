//! Typed repository facade for durable provider discovery.
//!
//! The lower-level [`crate::discovery`] module owns the `SQLite` state-machine
//! primitives. This module is the product-facing boundary: it hydrates domain
//! aggregates, validates bounded redacted payloads, and keeps provider graph
//! publication in the same transaction as discovery commit bookkeeping.

use std::collections::BTreeSet;

use chrono::{DateTime, Utc};
use lorepia_domain::{
    CapabilityObservation, Confidence, ConnectionConfigValue, CoreError, CoreErrorCode, CoreResult,
    CredentialRedirectPolicy, CredentialRef, DiscoverySessionId, EvidenceId, GenerationPreset,
    HttpUrl, ModelMetadataSource, ModelRoute, ObservationSource, ProviderConnection,
    ProviderConnectionId, ProviderNetworkMode, ProviderTemplate, SupportStatus, TemplateSource,
    discovery::{
        DiscoveryActionEnvelope, DiscoveryActionId, DiscoveryActionReceipt,
        DiscoveryActionRequired, DiscoveryApprovalBinding, DiscoveryApprovalDecision,
        DiscoveryApprovalGrant, DiscoveryApprovalId, DiscoveryApprovalRecord, DiscoveryCandidate,
        DiscoveryCandidateId, DiscoveryCommitAttemptId, DiscoveryCommitPlan,
        DiscoveryCompensationKind, DiscoveryCompensationStatus as DomainCompensationStatus,
        DiscoveryCompensationStep, DiscoveryCompensationTarget, DiscoveryEffect, DiscoveryEventId,
        DiscoveryInterruptionOutcome, DiscoveryOperationId, DiscoveryOperationKind,
        DiscoveryPreviousSelection, DiscoveryRecoveryCheckpoint, DiscoveryReviewDiff,
        DiscoverySideEffectClass, DiscoveryState, DiscoveryTransition,
        DiscoveryUnknownOutcomeResolution, PROVIDER_DISCOVERY_EVENT_VERSION,
        ProviderDiscoveryAction, ProviderDiscoveryEvent, ProviderDiscoverySession,
        SanitizedDiscoveryInput,
    },
};
use rusqlite::{Connection, OptionalExtension, Transaction, TransactionBehavior, params};
use serde::Serialize;
use serde_json::Value;
use uuid::Uuid;

use crate::{
    ProviderCredentialAccessAuthority, Storage,
    database::{
        StoredDiscoveredProviderGraphRows, clear_provider_selections_for_discovery_compensation,
        load_discovered_provider_graph_rows, load_discovery_previous_selection,
        restore_discovery_provider_selection, write_discovered_provider_graph_rows,
    },
    discovery::{
        self, CompletedDiscoveryOperation, DiscoveryRecoveryDisposition, DurableDiscoveryEffect,
        DurableDiscoveryTransition, DurableOperationOutcome, NewDiscoveryApproval,
        NewDiscoveryCommitAttempt, NewDiscoveryCompensationStep, NewDiscoveryOperation,
        PersistDiscoveryTransition,
    },
    generation_attempt::validate_provider_credential_access_authority_in_transaction,
    validate_provider_api_route_metadata,
};

mod approval_store;
mod commit_store;
pub(crate) mod contract_codec;
mod credential_execution;
mod errors;
mod event_outbox;
mod queries;
mod recovery;
mod repository_io;
mod row_mapping;
mod semantic_view;
mod transition_store;
mod types;
mod validation;

pub use semantic_view::{DiscoveryCandidateSnapshot, StoredDiscoveryCandidate};
pub use types::{
    DiscoveredProviderGraph, DiscoveryActionReplay, DiscoveryCommitAttemptRecord,
    DiscoveryCommitPhase, DiscoveryCompensationRecord, DiscoveryCompensationStatus,
    DiscoveryCompletedOperationWrite, DiscoveryEvidenceKind, DiscoveryEvidenceRecord,
    DiscoveryJsonUpdate, DiscoveryNativeCredentialExecutionRecord,
    DiscoveryNativeCredentialExecutionReservation, DiscoveryNativeCredentialStoreAttemptStart,
    DiscoveryNativeNoEffectAttestationKind, DiscoveryNativeNoEffectAttestationRecord,
    DiscoveryNativeNoEffectAttestationWrite, DiscoveryNativeRecoveryOwner,
    DiscoveryOperationRecord, DiscoveryOperationStatus, DiscoveryOutboxEvent,
    DiscoveryRecoveryResult, DiscoverySessionSnapshot, DiscoveryTransitionWrite,
    PreparedDiscoveryCommit, PreparedDiscoveryCompensationStep,
};

use approval_store::{
    approval_kind, validate_credential_approval, validate_discovery_authority_approval_rows,
    validate_discovery_unknown_outcome_resolution, validate_review_approval,
};
use commit_store::{
    apply_provider_graph_in_transaction, complete_commit_attempt_for_ready_transition,
    ensure_discovery_attempt_graph_absent, finalize_commit_failed_before_apply,
    graph_ownership_audit_hash, graph_template_was_created, load_commit_attempt,
    load_discovery_selection_restore_revision, provider_graph_ownership_hash,
    record_discovery_selection_restore_authority, require_started_session_operation,
    stored_provider_graph_ownership_hash, validate_commit_phase_preconditions,
    validate_discovery_authority_graph_audits, validate_graph_component, validate_provider_graph,
    verify_discovery_attempt_graph,
};
use contract_codec::{
    append_audit, candidate_kind, canonical_json_result, canonical_typed_json_result,
    decode_redacted_json, encode_approval_grant, encode_commit_plan_json, encode_json_result,
    encode_redacted_json, enum_wire_result, parse_approval_decision, parse_discovery_state,
    parse_operation_kind, parse_side_effect_class, parse_timestamp, sha256_hex,
};
#[cfg(test)]
use credential_execution::native_no_effect_execution_binding_sha256;
use credential_execution::{
    DISCOVERY_REDACTION_VERSION, DiscoveryAuthorityReceiptRecord,
    insert_discovery_credential_ownership_event, load_discovery_authority_receipt_by_action,
    load_discovery_authority_receipt_by_revision, load_native_no_effect_attestation,
    native_no_effect_evidence_sha256, project_reconciled_discovery_credential_ownership,
    validate_cancelled_pre_store_interruption_receipt,
    validate_discovery_operation_interrupted_audit, validate_discovery_operation_start_audit,
    validate_discovery_operation_terminal_audit_order_for_receipt,
    validate_discovery_receipt_follows, validate_exact_discovery_authority_audit,
    validate_interrupted_discovery_authority_receipt,
    validate_interrupted_discovery_operation_evidence,
    validate_native_no_effect_operation_start_receipt,
};
pub(crate) use credential_execution::{
    validate_archived_discovery_credential_ownership_authority_for_slot_gc,
    validate_discovery_credential_ownership_authority,
    validate_native_no_effect_attestation_integrity,
};
use errors::{contract_error, corrupted, database_error, discovery_error};
use queries::{
    load_discovery_native_credential_execution, load_operation_by_id, load_pollable_outbox_rows,
    load_pollable_outbox_rows_for_session, load_session_snapshot,
};
use recovery::{
    load_discovery_credential_compensation_operation_id, reconcile_discovery_saga_ledger,
    validate_active_discovery_credential_cancellation_chain,
    validate_legacy_unbound_started_credential_execution,
    validate_native_no_effect_retry_predecessor, validate_pre_store_native_credential_interruption,
    validate_terminal_compensation_transition,
};
#[cfg(test)]
use recovery::{prepare_compensation_ledger, validate_failed_compensation_ledger};
use repository_io::{
    compensation_status_transition_allowed, ensure_foreign_keys_clean,
    validate_discovery_native_physical_authority_id,
};
use row_mapping::{
    ApprovalRow, CompensationRow, decode_approval_row, decode_compensation_row, decode_evidence_row,
};
#[cfg(test)]
use transition_store::validate_completed_operation_binding;
use transition_store::{
    audit_kind_for_action, map_discovery_effect, persist_transition_in_transaction,
    validate_discovery_local_network_approval_binding, validate_prepared_commit,
};
#[cfg(test)]
use validation::is_pristine_discovery_session;
use validation::{
    ensure_provider_credential_operation_settled_for_discovery, looks_like_secret, require_session,
    validate_approval_references, validate_atomic_discovery_begin,
    validate_candidate_evidence_references, validate_capability_probe_grant,
    validate_discovery_evidence, validate_identifier, validate_limit,
    validate_opaque_credential_reference, validate_persistable_discovery_url,
    validate_redacted_value, validate_review_evidence_references, validate_sanitized_input,
    validate_session_evidence_ids, validate_sha256, validate_transition_write,
};
#[cfg(test)]
mod tests;
