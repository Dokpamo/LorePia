pub(super) const SQL: &str = r"SELECT occurrence_id
                     FROM core_lifecycle_outbox
                     WHERE status != 'acknowledged' AND (
                         (status = 'pending' AND available_at <= ?1)
                         OR (status = 'claimed' AND lease_until <= ?1)
                     )
                     AND NOT EXISTS (
                         SELECT 1
                         FROM core_lifecycle_outbox AS predecessor
                         WHERE predecessor.conversation_id =
                                   core_lifecycle_outbox.conversation_id
                           AND predecessor.branch_id =
                                   core_lifecycle_outbox.branch_id
                           AND predecessor.status != 'acknowledged'
                           AND (
                               predecessor.occurred_at <
                                   core_lifecycle_outbox.occurred_at
                               OR (
                                   predecessor.occurred_at =
                                       core_lifecycle_outbox.occurred_at
                                   AND predecessor.occurrence_id <
                                       core_lifecycle_outbox.occurrence_id
                               )
                           )
                     )
                     AND NOT (
                         event_kind = 'message_committed'
                         AND generation_id IS NOT NULL
                         AND EXISTS (
                             SELECT 1
                             FROM core_lifecycle_outbox AS predecessor
                             WHERE predecessor.generation_id =
                                       core_lifecycle_outbox.generation_id
                               AND predecessor.event_kind = 'after_generation'
                               AND predecessor.status != 'acknowledged'
                         )
                     )
                     ORDER BY occurred_at, occurrence_id
                     LIMIT ?2";

#[cfg(test)]
mod tests;
