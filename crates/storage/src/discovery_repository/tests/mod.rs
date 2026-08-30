mod approvals;
mod begin;
mod commit;
mod credentials;
mod evidence;
mod interruption;
mod recovery;
mod support;
mod transitions;
mod unknown_outcome;
mod validation;

use chrono::{DateTime, TimeZone, Utc};
use lorepia_domain::{
    CanonicalOrigin, CoreErrorCode, CredentialRef, DiscoverySessionId, EvidenceId, HttpUrl,
    ModelRouteId, ProviderConnectionId, ProviderLocalNetworkApproval, ProviderNetworkMode,
    ProviderProfile, ProviderTemplateId,
    discovery::{
        DiscoveryActionEnvelope, DiscoveryActionId, DiscoveryApprovalDecision,
        DiscoveryApprovalGrant, DiscoveryApprovalId, DiscoveryApprovalRecord,
        DiscoveryCommitAttemptId, DiscoveryCommitPlan, DiscoveryCompensationKind,
        DiscoveryCompensationStatus, DiscoveryCompensationStep, DiscoveryCompensationTarget,
        DiscoveryFailure, DiscoveryInterruptionOutcome, DiscoveryOperationId,
        DiscoveryOperationKind, DiscoveryPreviousSelection, DiscoveryReviewChange,
        DiscoveryReviewChangeKind, DiscoveryReviewDiff, DiscoveryState,
        DiscoveryUnknownOutcomeResolution, ProviderDiscoveryAction,
        ProviderDiscoveryConnectionOptions, ProviderDiscoverySession, SanitizedDiscoveryInput,
    },
};
use serde_json::{Value, json};
use tempfile::{TempDir, tempdir};

use crate::{
    ProviderCredentialObservedStatus, ProviderCredentialOperationKind,
    ProviderCredentialSlotGarbageStatus,
};

use super::{
    DiscoveryCompletedOperationWrite, DiscoveryEvidenceKind, DiscoveryEvidenceRecord,
    DiscoveryJsonUpdate, DiscoveryNativeNoEffectAttestationWrite, DiscoveryTransitionWrite,
    DurableOperationOutcome, PersistDiscoveryTransition, PreparedDiscoveryCommit, Storage,
    canonical_json_result, encode_approval_grant, encode_commit_plan_json,
    provider_graph_ownership_hash, sha256_hex,
    validate_archived_discovery_credential_ownership_authority_for_slot_gc,
    validate_discovery_credential_ownership_authority,
    validate_discovery_local_network_approval_binding,
};

use super::{
    contract_error, database_error, discovery_error, encode_redacted_json,
    is_pristine_discovery_session, require_session, validate_discovery_evidence,
    validate_identifier, validate_sanitized_input,
};
use crate::discovery::{self, NewDiscoverySession};
use lorepia_domain::{CoreError, CoreResult};
use rusqlite::{OptionalExtension, TransactionBehavior, params};

impl Storage {
    fn create_discovery_session(
        &self,
        session: &ProviderDiscoverySession,
        created_at: DateTime<Utc>,
    ) -> CoreResult<()> {
        session.validate().map_err(contract_error)?;
        validate_sanitized_input(&session.input)?;
        if !is_pristine_discovery_session(session) {
            return Err(CoreError::invalid(
                "a new discovery session must be a pristine draft",
            ));
        }
        validate_identifier("discovery session id", session.id.as_str(), 128)?;
        let mut connection = self.connection()?;
        discovery::insert_discovery_session(
            &mut connection,
            &NewDiscoverySession {
                id: session.id.as_str(),
                input: &session.input,
                created_at: &created_at.to_rfc3339(),
            },
        )
        .map_err(discovery_error)
    }

    fn save_discovery_evidence(&self, evidence: &DiscoveryEvidenceRecord) -> CoreResult<()> {
        validate_discovery_evidence(evidence)?;
        let mut connection = self.connection()?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(database_error)?;
        require_session(&transaction, evidence.session_id.as_str())?;
        let extracted_json = encode_redacted_json(&evidence.extracted_json, "discovery evidence")?;
        let existing = transaction
            .query_row(
                "SELECT session_id, kind, source_url, content_sha256, extracted_json, fetched_at
                 FROM provider_discovery_evidence
                 WHERE id = ?1",
                [evidence.id.as_str()],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, String>(3)?,
                        row.get::<_, String>(4)?,
                        row.get::<_, String>(5)?,
                    ))
                },
            )
            .optional()
            .map_err(database_error)?;
        let expected = (
            evidence.session_id.as_str(),
            evidence.kind.as_str(),
            evidence.source_url.as_str(),
            evidence.content_sha256.as_str(),
            extracted_json.as_str(),
            evidence.fetched_at.to_rfc3339(),
        );
        if let Some(existing) = existing {
            if existing.0 == expected.0
                && existing.1 == expected.1
                && existing.2 == expected.2
                && existing.3 == expected.3
                && existing.4 == expected.4
                && existing.5 == expected.5
            {
                return Ok(());
            }
            return Err(CoreError::invalid(
                "discovery evidence identifiers are immutable",
            ));
        }
        transaction
            .execute(
                "INSERT INTO provider_discovery_evidence (
                     id, session_id, kind, source_url, content_sha256,
                     extracted_json, redaction_version, fetched_at
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, 1, ?7)",
                params![
                    evidence.id.as_str(),
                    evidence.session_id.as_str(),
                    evidence.kind.as_str(),
                    evidence.source_url.as_str(),
                    evidence.content_sha256,
                    extracted_json,
                    evidence.fetched_at.to_rfc3339(),
                ],
            )
            .map_err(database_error)?;
        transaction.commit().map_err(database_error)
    }
}
