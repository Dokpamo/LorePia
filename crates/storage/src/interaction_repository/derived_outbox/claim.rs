use chrono::{DateTime, Utc};
use lorepia_domain::{CoreError, CoreResult};
use rusqlite::{TransactionBehavior, params};

use crate::Storage;

use super::super::state::validate_nonempty_id;
use super::super::{
    i64_from_u64, parse_datetime, revision_conflict, storage_corrupted, storage_db_error,
    u64_from_i64,
};
use super::recovery::quarantine_legacy_derived_outbox_rows;
use super::row_mapping::{decode_claimed_derived_outbox_row, read_derived_outbox_row};
use super::{
    InteractionDerivedEventSupervisorStatus, MAX_INTERACTION_DERIVED_CLAIM,
    StoredInteractionDerivedEvent,
};

impl Storage {
    pub fn interaction_derived_event_supervisor_status(
        &self,
    ) -> CoreResult<InteractionDerivedEventSupervisorStatus> {
        let connection = self.connection()?;
        let (pending_count, next_available_at) = connection
            .query_row(
                "WITH live AS (
                     SELECT occurrence.*
                     FROM interaction_derived_event_outbox AS occurrence
                     WHERE occurrence.status != 'acknowledged'
                       AND NOT EXISTS (
                           SELECT 1
                           FROM interaction_derived_event_quarantines AS quarantine
                           WHERE quarantine.occurrence_id = occurrence.occurrence_id
                       )
                 ), branch_heads AS (
                     SELECT candidate.*
                     FROM live AS candidate
                     WHERE NOT EXISTS (
                         SELECT 1
                         FROM live AS predecessor
                         WHERE predecessor.conversation_id = candidate.conversation_id
                           AND predecessor.branch_id = candidate.branch_id
                           AND (
                               predecessor.parent_resulting_state_revision
                                   < candidate.parent_resulting_state_revision
                               OR (
                                   predecessor.parent_resulting_state_revision
                                       = candidate.parent_resulting_state_revision
                                   AND predecessor.chain_id < candidate.chain_id
                               )
                               OR (
                                   predecessor.parent_resulting_state_revision
                                       = candidate.parent_resulting_state_revision
                                   AND predecessor.chain_id = candidate.chain_id
                                   AND predecessor.chain_ordinal < candidate.chain_ordinal
                               )
                           )
                     )
                 )
                 SELECT (SELECT COUNT(*) FROM live),
                        (SELECT MIN(CASE
                            WHEN branch_heads.status = 'pending'
                                THEN branch_heads.available_at
                            ELSE branch_heads.lease_until
                        END) FROM branch_heads)",
                [],
                |row| Ok((row.get::<_, i64>(0)?, row.get::<_, Option<String>>(1)?)),
            )
            .map_err(storage_db_error)?;
        Ok(InteractionDerivedEventSupervisorStatus {
            pending_count: u64_from_i64("pending derived interaction count", pending_count)?,
            next_available_at: next_available_at
                .as_deref()
                .map(|value| parse_datetime("next derived interaction availability", value))
                .transpose()?,
        })
    }

    /// Claims the earliest derived events without allowing two transitions on
    /// the same branch to race. Expired leases are reclaimed at least once;
    /// exact event/idempotency identities make materialization idempotent.
    pub fn claim_interaction_derived_events(
        &self,
        now: DateTime<Utc>,
        lease_until: DateTime<Utc>,
        limit: u32,
    ) -> CoreResult<Vec<StoredInteractionDerivedEvent>> {
        if limit == 0 || limit > MAX_INTERACTION_DERIVED_CLAIM {
            return Err(CoreError::invalid(
                "derived interaction claim limit must be between 1 and 64",
            ));
        }
        if lease_until <= now {
            return Err(CoreError::invalid(
                "derived interaction lease must end after its claim time",
            ));
        }
        let mut connection = self.connection()?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(storage_db_error)?;
        quarantine_legacy_derived_outbox_rows(&transaction, now, limit)?;
        let ids = {
            let mut statement = transaction
                .prepare(
                    "SELECT candidate.occurrence_id
                     FROM interaction_derived_event_outbox AS candidate
                     WHERE (
                         (candidate.status = 'pending' AND candidate.available_at <= ?1)
                         OR (candidate.status = 'claimed' AND candidate.lease_until <= ?1)
                     )
                     AND NOT EXISTS (
                         SELECT 1
                         FROM interaction_derived_event_quarantines AS quarantine
                         WHERE quarantine.occurrence_id = candidate.occurrence_id
                     )
                     AND NOT EXISTS (
                         SELECT 1
                         FROM interaction_derived_event_outbox AS predecessor
                         WHERE predecessor.conversation_id = candidate.conversation_id
                           AND predecessor.branch_id = candidate.branch_id
                           AND predecessor.status != 'acknowledged'
                           AND NOT EXISTS (
                               SELECT 1
                               FROM interaction_derived_event_quarantines AS quarantine
                               WHERE quarantine.occurrence_id = predecessor.occurrence_id
                           )
                           AND (
                               predecessor.parent_resulting_state_revision
                                   < candidate.parent_resulting_state_revision
                               OR (
                                   predecessor.parent_resulting_state_revision
                                       = candidate.parent_resulting_state_revision
                                   AND predecessor.chain_id < candidate.chain_id
                               )
                               OR (
                                   predecessor.parent_resulting_state_revision
                                       = candidate.parent_resulting_state_revision
                                   AND predecessor.chain_id = candidate.chain_id
                                   AND predecessor.chain_ordinal < candidate.chain_ordinal
                               )
                           )
                     )
                     ORDER BY candidate.parent_resulting_state_revision,
                              candidate.chain_id,
                              candidate.chain_ordinal
                     LIMIT ?2",
                )
                .map_err(storage_db_error)?;
            statement
                .query_map(params![now.to_rfc3339(), i64::from(limit)], |row| {
                    row.get::<_, String>(0)
                })
                .map_err(storage_db_error)?
                .collect::<Result<Vec<_>, _>>()
                .map_err(storage_db_error)?
        };
        let mut claimed = Vec::with_capacity(ids.len());
        for occurrence_id in ids {
            let changed = transaction
                .execute(
                    "UPDATE interaction_derived_event_outbox
                     SET status = 'claimed', delivery_attempts = delivery_attempts + 1,
                         lease_until = ?2, available_at = ?3
                     WHERE occurrence_id = ?1
                       AND ((status = 'pending' AND available_at <= ?3)
                            OR (status = 'claimed' AND lease_until <= ?3))",
                    params![occurrence_id, lease_until.to_rfc3339(), now.to_rfc3339()],
                )
                .map_err(storage_db_error)?;
            if changed != 1 {
                return Err(storage_corrupted(
                    "derived interaction occurrence changed during claim",
                ));
            }
            let row = read_derived_outbox_row(&transaction, &occurrence_id)?
                .ok_or_else(|| storage_corrupted("claimed derived occurrence disappeared"))?;
            claimed.push(decode_claimed_derived_outbox_row(&transaction, row)?);
        }
        transaction.commit().map_err(storage_db_error)?;
        Ok(claimed)
    }

    /// Defers one failed derived occurrence under its exact delivery token.
    pub fn retry_interaction_derived_event_after(
        &self,
        occurrence_id: &str,
        expected_delivery_attempts: u64,
        available_at: DateTime<Utc>,
    ) -> CoreResult<()> {
        validate_nonempty_id("derived interaction occurrence", occurrence_id)?;
        let changed = self
            .connection()?
            .execute(
                "UPDATE interaction_derived_event_outbox
                 SET status = 'pending', lease_until = NULL, available_at = ?3
                 WHERE occurrence_id = ?1 AND status = 'claimed'
                   AND delivery_attempts = ?2
                   AND NOT EXISTS (
                       SELECT 1
                       FROM interaction_derived_event_quarantines AS quarantine
                       WHERE quarantine.occurrence_id =
                             interaction_derived_event_outbox.occurrence_id
                   )",
                params![
                    occurrence_id,
                    i64_from_u64(
                        "derived interaction delivery attempts",
                        expected_delivery_attempts
                    )?,
                    available_at.to_rfc3339(),
                ],
            )
            .map_err(storage_db_error)?;
        if changed == 1 {
            Ok(())
        } else {
            Err(revision_conflict(
                "derived interaction occurrence delivery token is stale",
            ))
        }
    }
}
