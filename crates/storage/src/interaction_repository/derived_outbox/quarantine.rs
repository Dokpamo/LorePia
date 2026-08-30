use chrono::{DateTime, Utc};
use lorepia_domain::{CoreError, CoreResult, Sha256Digest};
use rusqlite::{Connection, OptionalExtension, TransactionBehavior, params};
use serde::Serialize;

use crate::Storage;

use super::super::state::validate_nonempty_id;
use super::super::types::InteractionPolicySnapshot;
use super::super::{
    MAX_AUDIT_JSON_BYTES, encode_json, i64_from_u64, interaction_policy_sha256, not_found,
    parse_datetime, revision_conflict, sha256_hex, storage_corrupted, storage_db_error,
    u64_from_i64,
};
use super::StoredInteractionDerivedEventQuarantine;
use super::row_mapping::{decode_derived_outbox_row, read_derived_outbox_row};

#[derive(Serialize)]
pub(super) struct DerivedQuarantineEvidence<'a> {
    pub(super) schema_version: u32,
    pub(super) occurrence_id: &'a str,
    pub(super) delivery_attempts: u64,
    pub(super) sealed_policy_sha256: &'a Sha256Digest,
    pub(super) active_policy_sha256: Option<&'a Sha256Digest>,
    pub(super) source_effect_sha256: &'a Sha256Digest,
    pub(super) source_action_sha256: &'a Sha256Digest,
    pub(super) reason_kind: &'a str,
}

impl Storage {
    /// Atomically records a terminal, non-successful outcome when Core cannot
    /// reconstruct the occurrence's sealed evaluation authority. Repeating
    /// the same evidence after response loss returns an exact replay.
    pub fn quarantine_interaction_derived_event_authority_failure(
        &self,
        occurrence_id: &str,
        expected_delivery_attempts: u64,
        active_policy: Option<&InteractionPolicySnapshot>,
        quarantined_at: DateTime<Utc>,
    ) -> CoreResult<StoredInteractionDerivedEventQuarantine> {
        validate_nonempty_id("derived interaction occurrence", occurrence_id)?;
        let mut connection = self.connection()?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(storage_db_error)?;
        let raw = read_derived_outbox_row(&transaction, occurrence_id)?
            .ok_or_else(|| not_found("derived interaction occurrence"))?;
        let status = raw.status.clone();
        let occurrence = decode_derived_outbox_row(&transaction, raw)?;
        let active_policy_sha256 = active_policy
            .map(interaction_policy_sha256)
            .transpose()?
            .map(Sha256Digest::parse)
            .transpose()
            .map_err(CoreError::invalid)?;
        let evidence = DerivedQuarantineEvidence {
            schema_version: 1,
            occurrence_id,
            delivery_attempts: expected_delivery_attempts,
            sealed_policy_sha256: &occurrence.policy_sha256,
            active_policy_sha256: active_policy_sha256.as_ref(),
            source_effect_sha256: &occurrence.source_effect_sha256,
            source_action_sha256: &occurrence.source_action_sha256,
            reason_kind: "sealed_policy_recovery_failed",
        };
        let evidence_json = encode_json(
            "derived interaction quarantine evidence",
            &evidence,
            MAX_AUDIT_JSON_BYTES,
        )?;
        let evidence_sha256 = Sha256Digest::parse(sha256_hex(evidence_json.as_bytes()))
            .map_err(CoreError::invalid)?;
        if let Some(stored) = read_derived_event_quarantine(&transaction, occurrence_id)? {
            if stored.delivery_attempts != expected_delivery_attempts
                || stored.sealed_policy_sha256 != occurrence.policy_sha256
                || stored.active_policy_sha256 != active_policy_sha256
                || stored.source_effect_sha256 != occurrence.source_effect_sha256
                || stored.source_action_sha256 != occurrence.source_action_sha256
                || stored.evidence_sha256 != evidence_sha256
            {
                return Err(revision_conflict(
                    "derived interaction quarantine evidence changed",
                ));
            }
            transaction.commit().map_err(storage_db_error)?;
            return Ok(StoredInteractionDerivedEventQuarantine {
                exact_replay: true,
                ..stored
            });
        }
        if status != "claimed" || occurrence.delivery_attempts != expected_delivery_attempts {
            return Err(revision_conflict(
                "derived interaction occurrence delivery token is stale",
            ));
        }
        transaction
            .execute(
                "INSERT INTO interaction_derived_event_quarantines
                 (occurrence_id, reason_kind, delivery_attempts,
                  sealed_policy_sha256, active_policy_sha256,
                  source_effect_sha256, source_action_sha256,
                  evidence_json, evidence_sha256, quarantined_at)
                 VALUES (?1, 'sealed_policy_recovery_failed', ?2, ?3, ?4,
                         ?5, ?6, ?7, ?8, ?9)",
                params![
                    occurrence_id,
                    i64_from_u64(
                        "derived interaction delivery attempts",
                        expected_delivery_attempts,
                    )?,
                    occurrence.policy_sha256.as_str(),
                    active_policy_sha256.as_ref().map(Sha256Digest::as_str),
                    occurrence.source_effect_sha256.as_str(),
                    occurrence.source_action_sha256.as_str(),
                    evidence_json,
                    evidence_sha256.as_str(),
                    quarantined_at.to_rfc3339(),
                ],
            )
            .map_err(storage_db_error)?;
        transaction.commit().map_err(storage_db_error)?;
        Ok(StoredInteractionDerivedEventQuarantine {
            occurrence_id: occurrence_id.to_owned(),
            delivery_attempts: expected_delivery_attempts,
            sealed_policy_sha256: occurrence.policy_sha256,
            active_policy_sha256,
            source_effect_sha256: occurrence.source_effect_sha256,
            source_action_sha256: occurrence.source_action_sha256,
            evidence_sha256,
            quarantined_at,
            exact_replay: false,
        })
    }
}

pub(super) fn read_derived_event_quarantine(
    connection: &Connection,
    occurrence_id: &str,
) -> CoreResult<Option<StoredInteractionDerivedEventQuarantine>> {
    connection
        .query_row(
            "SELECT delivery_attempts, sealed_policy_sha256,
                    active_policy_sha256, source_effect_sha256,
                    source_action_sha256, reason_kind, evidence_json,
                    evidence_sha256, quarantined_at
             FROM interaction_derived_event_quarantines
             WHERE occurrence_id = ?1",
            [occurrence_id],
            |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, Option<String>>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, String>(4)?,
                    row.get::<_, String>(5)?,
                    row.get::<_, String>(6)?,
                    row.get::<_, String>(7)?,
                    row.get::<_, String>(8)?,
                ))
            },
        )
        .optional()
        .map_err(storage_db_error)?
        .map(|row| {
            let delivery_attempts = u64_from_i64("derived quarantine delivery attempts", row.0)?;
            let sealed_policy_sha256 = Sha256Digest::parse(row.1).map_err(CoreError::invalid)?;
            let active_policy_sha256 = row
                .2
                .map(Sha256Digest::parse)
                .transpose()
                .map_err(CoreError::invalid)?;
            let source_effect_sha256 = Sha256Digest::parse(row.3).map_err(CoreError::invalid)?;
            let source_action_sha256 = Sha256Digest::parse(row.4).map_err(CoreError::invalid)?;
            if row.5 != "sealed_policy_recovery_failed" {
                return Err(storage_corrupted(
                    "derived quarantine reason kind is invalid",
                ));
            }
            let canonical_evidence = encode_json(
                "derived interaction quarantine evidence",
                &DerivedQuarantineEvidence {
                    schema_version: 1,
                    occurrence_id,
                    delivery_attempts,
                    sealed_policy_sha256: &sealed_policy_sha256,
                    active_policy_sha256: active_policy_sha256.as_ref(),
                    source_effect_sha256: &source_effect_sha256,
                    source_action_sha256: &source_action_sha256,
                    reason_kind: "sealed_policy_recovery_failed",
                },
                MAX_AUDIT_JSON_BYTES,
            )?;
            if canonical_evidence != row.6 || sha256_hex(row.6.as_bytes()) != row.7 {
                return Err(storage_corrupted(
                    "derived quarantine evidence hash is inconsistent",
                ));
            }
            Ok(StoredInteractionDerivedEventQuarantine {
                occurrence_id: occurrence_id.to_owned(),
                delivery_attempts,
                sealed_policy_sha256,
                active_policy_sha256,
                source_effect_sha256,
                source_action_sha256,
                evidence_sha256: Sha256Digest::parse(row.7).map_err(CoreError::invalid)?,
                quarantined_at: parse_datetime("derived quarantine timestamp", &row.8)?,
                exact_replay: false,
            })
        })
        .transpose()
}
