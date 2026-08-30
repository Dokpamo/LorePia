//! At-least-once discovery event outbox polling and acknowledgement.

use super::{
    CoreResult, DateTime, DiscoveryEventId, DiscoveryOutboxEvent, DiscoverySessionId, Storage,
    TransactionBehavior, Utc, database_error, load_pollable_outbox_rows,
    load_pollable_outbox_rows_for_session, params, validate_identifier, validate_limit,
};

impl Storage {
    pub fn poll_discovery_events(
        &self,
        limit: u32,
        available_at: DateTime<Utc>,
    ) -> CoreResult<Vec<DiscoveryOutboxEvent>> {
        validate_limit(limit)?;
        let mut connection = self.connection()?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(database_error)?;
        let rows = load_pollable_outbox_rows(&transaction, limit, available_at)?;
        for row in &rows {
            transaction
                .execute(
                    "UPDATE provider_discovery_event_outbox
                     SET delivery_attempts = delivery_attempts + 1
                     WHERE id = ?1 AND delivered_at IS NULL",
                    [row.event.id.as_str()],
                )
                .map_err(database_error)?;
        }
        transaction.commit().map_err(database_error)?;
        Ok(rows
            .into_iter()
            .map(|mut row| {
                row.delivery_attempts += 1;
                row
            })
            .collect())
    }

    /// Polls only the earliest deliverable event for one discovery session.
    ///
    /// Delivery remains at-least-once until the caller acknowledges the event;
    /// pending events for other sessions are neither attempted nor discarded.
    pub fn poll_discovery_events_for_session(
        &self,
        session_id: &DiscoverySessionId,
        limit: u32,
        available_at: DateTime<Utc>,
    ) -> CoreResult<Vec<DiscoveryOutboxEvent>> {
        validate_identifier("discovery session id", session_id.as_str(), 128)?;
        validate_limit(limit)?;
        let mut connection = self.connection()?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(database_error)?;
        let rows =
            load_pollable_outbox_rows_for_session(&transaction, session_id, limit, available_at)?;
        for row in &rows {
            transaction
                .execute(
                    "UPDATE provider_discovery_event_outbox
                     SET delivery_attempts = delivery_attempts + 1
                     WHERE id = ?1 AND delivered_at IS NULL",
                    [row.event.id.as_str()],
                )
                .map_err(database_error)?;
        }
        transaction.commit().map_err(database_error)?;
        Ok(rows
            .into_iter()
            .map(|mut row| {
                row.delivery_attempts += 1;
                row
            })
            .collect())
    }

    pub fn ack_discovery_event(
        &self,
        event_id: &DiscoveryEventId,
        delivered_at: DateTime<Utc>,
    ) -> CoreResult<bool> {
        let changed = self
            .connection()?
            .execute(
                "UPDATE provider_discovery_event_outbox
                 SET delivered_at = ?2
                 WHERE id = ?1
                   AND delivered_at IS NULL
                   AND delivery_attempts > 0
                   AND ?2 >= available_at
                   AND NOT EXISTS (
                       SELECT 1
                       FROM provider_discovery_event_outbox AS earlier
                       WHERE earlier.session_id =
                             provider_discovery_event_outbox.session_id
                         AND earlier.delivered_at IS NULL
                         AND earlier.sequence <
                             provider_discovery_event_outbox.sequence
                   )",
                params![event_id.as_str(), delivered_at.to_rfc3339()],
            )
            .map_err(database_error)?;
        Ok(changed == 1)
    }
}
