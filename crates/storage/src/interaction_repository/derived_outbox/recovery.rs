use chrono::{DateTime, Utc};
use lorepia_domain::{CoreError, CoreResult, Sha256Digest};
use rusqlite::{Transaction, params};

use crate::Storage;

use super::super::{
    MAX_AUDIT_JSON_BYTES, encode_json, i64_from_u64, sha256_hex, storage_corrupted,
    storage_db_error, u64_from_i64,
};
use super::quarantine::DerivedQuarantineEvidence;

impl Storage {
    /// Resets every abandoned derived-event claim while `Storage::open` still
    /// holds the process-exclusive data-root lock.
    pub(crate) fn recover_all_interaction_derived_event_leases(
        &self,
        available_at: DateTime<Utc>,
    ) -> CoreResult<u64> {
        let changed = self
            .connection()?
            .execute(
                "UPDATE interaction_derived_event_outbox
                 SET status = 'pending', lease_until = NULL, available_at = ?1
                 WHERE status = 'claimed'
                   AND NOT EXISTS (
                       SELECT 1
                       FROM interaction_derived_event_quarantines AS quarantine
                       WHERE quarantine.occurrence_id =
                             interaction_derived_event_outbox.occurrence_id
                   )",
                [available_at.to_rfc3339()],
            )
            .map_err(storage_db_error)?;
        u64::try_from(changed)
            .map_err(|_| CoreError::internal("derived-event recovery count overflowed"))
    }
}

pub(super) fn quarantine_legacy_derived_outbox_rows(
    transaction: &Transaction<'_>,
    quarantined_at: DateTime<Utc>,
    limit: u32,
) -> CoreResult<()> {
    let legacy_rows = {
        let mut statement = transaction
            .prepare(
                "SELECT occurrence.occurrence_id, occurrence.delivery_attempts,
                        occurrence.policy_sha256, occurrence.source_effect_sha256,
                        occurrence.source_action_sha256
                 FROM interaction_derived_event_outbox AS occurrence
                 WHERE occurrence.status != 'acknowledged'
                   AND (
                       occurrence.evaluation_seal_version != 1
                       OR occurrence.evaluation_seal_json IS NULL
                       OR occurrence.evaluation_seal_sha256 IS NULL
                       OR occurrence.deterministic_seed_hex IS NULL
                   )
                   AND NOT EXISTS (
                       SELECT 1
                       FROM interaction_derived_event_quarantines AS quarantine
                       WHERE quarantine.occurrence_id = occurrence.occurrence_id
                   )
                 ORDER BY occurrence.occurrence_id
                 LIMIT ?1",
            )
            .map_err(storage_db_error)?;
        statement
            .query_map([i64::from(limit)], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, String>(4)?,
                ))
            })
            .map_err(storage_db_error)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(storage_db_error)?
    };
    for (occurrence_id, delivery_attempts, policy_sha256, effect_sha256, action_sha256) in
        legacy_rows
    {
        let prior_delivery_attempts =
            u64_from_i64("legacy derived delivery attempts", delivery_attempts)?;
        let delivery_attempts = prior_delivery_attempts
            .checked_add(1)
            .ok_or_else(|| CoreError::invalid("legacy derived delivery attempts overflowed"))?;
        let claimed = transaction
            .execute(
                "UPDATE interaction_derived_event_outbox
                 SET status = 'claimed',
                     delivery_attempts = delivery_attempts + 1,
                     lease_until = ?3,
                     available_at = ?3
                 WHERE occurrence_id = ?1
                   AND status != 'acknowledged'
                   AND delivery_attempts = ?2
                   AND (
                       evaluation_seal_version != 1
                       OR evaluation_seal_json IS NULL
                       OR evaluation_seal_sha256 IS NULL
                       OR deterministic_seed_hex IS NULL
                   )",
                params![
                    occurrence_id,
                    i64_from_u64(
                        "legacy derived prior delivery attempts",
                        prior_delivery_attempts,
                    )?,
                    quarantined_at.to_rfc3339(),
                ],
            )
            .map_err(storage_db_error)?;
        if claimed != 1 {
            return Err(storage_corrupted(
                "legacy derived interaction claim was not exact",
            ));
        }
        let sealed_policy_sha256 =
            Sha256Digest::parse(policy_sha256).map_err(CoreError::invalid)?;
        let source_effect_sha256 =
            Sha256Digest::parse(effect_sha256).map_err(CoreError::invalid)?;
        let source_action_sha256 =
            Sha256Digest::parse(action_sha256).map_err(CoreError::invalid)?;
        let evidence = DerivedQuarantineEvidence {
            schema_version: 1,
            occurrence_id: &occurrence_id,
            delivery_attempts,
            sealed_policy_sha256: &sealed_policy_sha256,
            active_policy_sha256: None,
            source_effect_sha256: &source_effect_sha256,
            source_action_sha256: &source_action_sha256,
            reason_kind: "sealed_policy_recovery_failed",
        };
        let evidence_json = encode_json(
            "legacy derived interaction quarantine evidence",
            &evidence,
            MAX_AUDIT_JSON_BYTES,
        )?;
        let evidence_sha256 = sha256_hex(evidence_json.as_bytes());
        let inserted = transaction
            .execute(
                "INSERT INTO interaction_derived_event_quarantines
                 (occurrence_id, reason_kind, delivery_attempts,
                  sealed_policy_sha256, active_policy_sha256,
                  source_effect_sha256, source_action_sha256,
                  evidence_json, evidence_sha256, quarantined_at)
                 VALUES (?1, 'sealed_policy_recovery_failed', ?2, ?3, NULL,
                         ?4, ?5, ?6, ?7, ?8)",
                params![
                    occurrence_id,
                    i64_from_u64("legacy derived delivery attempts", delivery_attempts)?,
                    sealed_policy_sha256.as_str(),
                    source_effect_sha256.as_str(),
                    source_action_sha256.as_str(),
                    evidence_json,
                    evidence_sha256,
                    quarantined_at.to_rfc3339(),
                ],
            )
            .map_err(storage_db_error)?;
        if inserted != 1 {
            return Err(storage_corrupted(
                "legacy derived interaction quarantine insert was not exact",
            ));
        }
    }
    Ok(())
}
