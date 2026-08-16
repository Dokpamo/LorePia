use chrono::{DateTime, Utc};
use lorepia_domain::{
    ConversationBranchId, ConversationId, CoreError, CoreErrorCode, CoreResult, GenerationId,
    MessageId,
};
use rusqlite::{OptionalExtension, Transaction, TransactionBehavior, params};
use serde::{Deserialize, Serialize};

use crate::database::{Storage, storage_db_error};

const MAX_LIFECYCLE_CLAIM: usize = 256;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LifecycleOccurrenceKind {
    ConversationOpened,
    ConversationStarted,
    BeforeGeneration,
    AfterGeneration,
    MessageCommitted,
}

impl LifecycleOccurrenceKind {
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::ConversationOpened => "conversation_opened",
            Self::ConversationStarted => "conversation_started",
            Self::BeforeGeneration => "before_generation",
            Self::AfterGeneration => "after_generation",
            Self::MessageCommitted => "message_committed",
        }
    }

    fn parse(value: &str) -> CoreResult<Self> {
        match value {
            "conversation_opened" => Ok(Self::ConversationOpened),
            "conversation_started" => Ok(Self::ConversationStarted),
            "before_generation" => Ok(Self::BeforeGeneration),
            "after_generation" => Ok(Self::AfterGeneration),
            "message_committed" => Ok(Self::MessageCommitted),
            _ => Err(corrupted("stored lifecycle event kind is invalid")),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StoredLifecycleOccurrence {
    pub occurrence_id: String,
    pub event_kind: LifecycleOccurrenceKind,
    pub conversation_id: ConversationId,
    pub branch_id: ConversationBranchId,
    pub exact_head_message_id: Option<MessageId>,
    pub owner_message_id: Option<MessageId>,
    pub generation_id: Option<GenerationId>,
    pub occurred_at: DateTime<Utc>,
    pub available_at: DateTime<Utc>,
    pub delivery_attempts: u64,
    pub lease_until: Option<DateTime<Utc>>,
    pub acknowledged_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone)]
pub(crate) struct LifecycleOccurrenceWrite {
    pub occurrence_id: String,
    pub event_kind: LifecycleOccurrenceKind,
    pub conversation_id: ConversationId,
    pub branch_id: ConversationBranchId,
    pub exact_head_message_id: Option<MessageId>,
    pub owner_message_id: Option<MessageId>,
    pub generation_id: Option<GenerationId>,
    pub occurred_at: DateTime<Utc>,
}

impl Storage {
    pub fn claim_core_lifecycle_occurrences(
        &self,
        now: DateTime<Utc>,
        lease_until: DateTime<Utc>,
        limit: usize,
    ) -> CoreResult<Vec<StoredLifecycleOccurrence>> {
        if lease_until <= now {
            return Err(CoreError::invalid(
                "lifecycle occurrence lease must end after claim time",
            ));
        }
        let limit = bounded_limit(limit)?;
        if limit == 0 {
            return Ok(Vec::new());
        }
        let mut connection = self.connection()?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(storage_db_error)?;
        let occurrence_ids = {
            let mut statement = transaction
                .prepare(
                    "SELECT occurrence_id
                     FROM core_lifecycle_outbox
                     WHERE (
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
                     LIMIT ?2",
                )
                .map_err(storage_db_error)?;
            statement
                .query_map(
                    params![
                        now.to_rfc3339(),
                        i64::try_from(limit).map_err(|_| {
                            CoreError::invalid("lifecycle occurrence limit exceeds SQLite range")
                        })?
                    ],
                    |row| row.get::<_, String>(0),
                )
                .map_err(storage_db_error)?
                .collect::<Result<Vec<_>, _>>()
                .map_err(storage_db_error)?
        };
        let mut claimed = Vec::with_capacity(occurrence_ids.len());
        for occurrence_id in occurrence_ids {
            let changed = transaction
                .execute(
                    "UPDATE core_lifecycle_outbox
                     SET status = 'claimed',
                         delivery_attempts = delivery_attempts + 1,
                         lease_until = ?2,
                         available_at = ?3
                     WHERE occurrence_id = ?1
                       AND (
                         (status = 'pending' AND available_at <= ?3)
                         OR (status = 'claimed' AND lease_until <= ?3)
                       )",
                    params![occurrence_id, lease_until.to_rfc3339(), now.to_rfc3339()],
                )
                .map_err(storage_db_error)?;
            if changed != 1 {
                return Err(corrupted(
                    "lifecycle occurrence claim changed during an immediate transaction",
                ));
            }
            claimed.push(read_occurrence(&transaction, &occurrence_id)?);
        }
        transaction.commit().map_err(storage_db_error)?;
        Ok(claimed)
    }

    pub fn acknowledge_core_lifecycle_occurrence(
        &self,
        occurrence_id: &str,
        expected_delivery_attempts: u64,
        acknowledged_at: DateTime<Utc>,
    ) -> CoreResult<()> {
        validate_id("lifecycle occurrence", occurrence_id)?;
        let changed = self
            .connection()?
            .execute(
                "UPDATE core_lifecycle_outbox
                 SET status = 'acknowledged', lease_until = NULL,
                     acknowledged_at = ?3
                 WHERE occurrence_id = ?1 AND status = 'claimed'
                   AND delivery_attempts = ?2",
                params![
                    occurrence_id,
                    to_i64(expected_delivery_attempts)?,
                    acknowledged_at.to_rfc3339(),
                ],
            )
            .map_err(storage_db_error)?;
        if changed == 1 {
            Ok(())
        } else {
            Err(delivery_conflict(occurrence_id))
        }
    }

    pub fn retry_core_lifecycle_occurrence_after(
        &self,
        occurrence_id: &str,
        expected_delivery_attempts: u64,
        available_at: DateTime<Utc>,
    ) -> CoreResult<()> {
        validate_id("lifecycle occurrence", occurrence_id)?;
        let changed = self
            .connection()?
            .execute(
                "UPDATE core_lifecycle_outbox
                 SET status = 'pending', lease_until = NULL,
                     available_at = ?3
                 WHERE occurrence_id = ?1 AND status = 'claimed'
                   AND delivery_attempts = ?2",
                params![
                    occurrence_id,
                    to_i64(expected_delivery_attempts)?,
                    available_at.to_rfc3339(),
                ],
            )
            .map_err(storage_db_error)?;
        if changed == 1 {
            Ok(())
        } else {
            Err(delivery_conflict(occurrence_id))
        }
    }

    pub fn recover_core_lifecycle_occurrence_leases(&self, now: DateTime<Utc>) -> CoreResult<u64> {
        let changed = self
            .connection()?
            .execute(
                "UPDATE core_lifecycle_outbox
                 SET status = 'pending', lease_until = NULL,
                     available_at = ?1
                 WHERE status = 'claimed' AND lease_until <= ?1",
                [now.to_rfc3339()],
            )
            .map_err(storage_db_error)?;
        u64::try_from(changed).map_err(|_| CoreError::internal("lifecycle recovery count overflow"))
    }

    /// Startup-only recovery for the process-exclusive database owner.
    ///
    /// A crashed process may leave a claim whose wall-clock lease has not yet
    /// expired. `Storage::open` holds the data-root owner lock, so no live
    /// claimant can still own any stored lease at that point. Resetting every
    /// claim makes durable lifecycle delivery available immediately after a
    /// restart instead of silently delaying it until the old deadline.
    pub(crate) fn recover_all_core_lifecycle_occurrence_leases(
        &self,
        now: DateTime<Utc>,
    ) -> CoreResult<u64> {
        let changed = self
            .connection()?
            .execute(
                "UPDATE core_lifecycle_outbox
                 SET status = 'pending', lease_until = NULL,
                     available_at = ?1
                 WHERE status = 'claimed'",
                [now.to_rfc3339()],
            )
            .map_err(storage_db_error)?;
        u64::try_from(changed).map_err(|_| CoreError::internal("lifecycle recovery count overflow"))
    }

    /// Records a conversation-open occurrence without accepting raw action
    /// arguments or prompt text. Repeating the same exact snapshot is
    /// idempotent.
    pub fn enqueue_conversation_opened_occurrence(
        &self,
        open_occurrence_id: &str,
        conversation_id: &ConversationId,
        branch_id: &ConversationBranchId,
        exact_head_message_id: Option<&MessageId>,
        occurred_at: DateTime<Utc>,
    ) -> CoreResult<StoredLifecycleOccurrence> {
        validate_id("conversation open occurrence", open_occurrence_id)?;
        let write = LifecycleOccurrenceWrite {
            occurrence_id: format!("conversation-opened:{open_occurrence_id}"),
            event_kind: LifecycleOccurrenceKind::ConversationOpened,
            conversation_id: conversation_id.clone(),
            branch_id: branch_id.clone(),
            exact_head_message_id: exact_head_message_id.cloned(),
            owner_message_id: None,
            generation_id: None,
            occurred_at,
        };
        let mut connection = self.connection()?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(storage_db_error)?;
        validate_occurrence_snapshot(&transaction, &write)?;
        let inserted = insert_occurrence(&transaction, &write, true)?;
        let stored = read_occurrence(&transaction, &write.occurrence_id)?;
        if !inserted && !occurrence_matches_write(&stored, &write) {
            return Err(corrupted(
                "lifecycle occurrence idempotency identity was reused",
            ));
        }
        transaction.commit().map_err(storage_db_error)?;
        Ok(stored)
    }
}

pub(crate) fn insert_occurrence(
    transaction: &Transaction<'_>,
    occurrence: &LifecycleOccurrenceWrite,
    idempotent: bool,
) -> CoreResult<bool> {
    validate_write(occurrence)?;
    let sql = if idempotent {
        "INSERT OR IGNORE INTO core_lifecycle_outbox
         (occurrence_id, event_kind, conversation_id, branch_id,
          exact_head_message_id, owner_message_id, generation_id,
          occurred_at, available_at, status, delivery_attempts,
          lease_until, acknowledged_at, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?8,
                 'pending', 0, NULL, NULL, ?8)"
    } else {
        "INSERT INTO core_lifecycle_outbox
         (occurrence_id, event_kind, conversation_id, branch_id,
          exact_head_message_id, owner_message_id, generation_id,
          occurred_at, available_at, status, delivery_attempts,
          lease_until, acknowledged_at, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?8,
                 'pending', 0, NULL, NULL, ?8)"
    };
    transaction
        .execute(
            sql,
            params![
                occurrence.occurrence_id,
                occurrence.event_kind.as_str(),
                occurrence.conversation_id.0,
                occurrence.branch_id.0,
                occurrence
                    .exact_head_message_id
                    .as_ref()
                    .map(|id| id.0.as_str()),
                occurrence.owner_message_id.as_ref().map(|id| id.0.as_str()),
                occurrence.generation_id.as_ref().map(|id| id.0.as_str()),
                occurrence.occurred_at.to_rfc3339(),
            ],
        )
        .map(|changed| changed == 1)
        .map_err(storage_db_error)
}

fn validate_occurrence_snapshot(
    transaction: &Transaction<'_>,
    occurrence: &LifecycleOccurrenceWrite,
) -> CoreResult<()> {
    let valid = transaction
        .query_row(
            "SELECT EXISTS(
                SELECT 1
                FROM conversation_branches
                WHERE conversation_id = ?1 AND id = ?2
                  AND head_message_id IS ?3
             )",
            params![
                occurrence.conversation_id.0,
                occurrence.branch_id.0,
                occurrence
                    .exact_head_message_id
                    .as_ref()
                    .map(|id| id.0.as_str()),
            ],
            |row| row.get::<_, bool>(0),
        )
        .map_err(storage_db_error)?;
    if valid {
        Ok(())
    } else {
        Err(CoreError::invalid(
            "lifecycle occurrence does not match the exact branch head",
        ))
    }
}

fn read_occurrence(
    transaction: &Transaction<'_>,
    occurrence_id: &str,
) -> CoreResult<StoredLifecycleOccurrence> {
    transaction
        .query_row(
            "SELECT event_kind, conversation_id, branch_id,
                    exact_head_message_id, owner_message_id, generation_id,
                    occurred_at, available_at, delivery_attempts,
                    lease_until, acknowledged_at
             FROM core_lifecycle_outbox WHERE occurrence_id = ?1",
            [occurrence_id],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, Option<String>>(3)?,
                    row.get::<_, Option<String>>(4)?,
                    row.get::<_, Option<String>>(5)?,
                    row.get::<_, String>(6)?,
                    row.get::<_, String>(7)?,
                    row.get::<_, i64>(8)?,
                    row.get::<_, Option<String>>(9)?,
                    row.get::<_, Option<String>>(10)?,
                ))
            },
        )
        .optional()
        .map_err(storage_db_error)?
        .ok_or_else(|| not_found("lifecycle occurrence"))
        .and_then(|row| {
            Ok(StoredLifecycleOccurrence {
                occurrence_id: occurrence_id.to_owned(),
                event_kind: LifecycleOccurrenceKind::parse(&row.0)?,
                conversation_id: ConversationId(row.1),
                branch_id: ConversationBranchId(row.2),
                exact_head_message_id: row.3.map(MessageId),
                owner_message_id: row.4.map(MessageId),
                generation_id: row.5.map(GenerationId),
                occurred_at: parse_time("lifecycle occurred_at", &row.6)?,
                available_at: parse_time("lifecycle available_at", &row.7)?,
                delivery_attempts: to_u64(row.8)?,
                lease_until: row
                    .9
                    .as_deref()
                    .map(|value| parse_time("lifecycle lease_until", value))
                    .transpose()?,
                acknowledged_at: row
                    .10
                    .as_deref()
                    .map(|value| parse_time("lifecycle acknowledged_at", value))
                    .transpose()?,
            })
        })
}

fn occurrence_matches_write(
    stored: &StoredLifecycleOccurrence,
    write: &LifecycleOccurrenceWrite,
) -> bool {
    stored.event_kind == write.event_kind
        && stored.conversation_id == write.conversation_id
        && stored.branch_id == write.branch_id
        && stored.exact_head_message_id == write.exact_head_message_id
        && stored.owner_message_id == write.owner_message_id
        && stored.generation_id == write.generation_id
        && stored.occurred_at == write.occurred_at
}

fn validate_write(write: &LifecycleOccurrenceWrite) -> CoreResult<()> {
    validate_id("lifecycle occurrence", &write.occurrence_id)?;
    validate_id("conversation", &write.conversation_id.0)?;
    validate_id("branch", &write.branch_id.0)
}

fn bounded_limit(limit: usize) -> CoreResult<usize> {
    if limit > MAX_LIFECYCLE_CLAIM {
        Err(CoreError::invalid(format!(
            "lifecycle occurrence claim limit exceeds {MAX_LIFECYCLE_CLAIM}"
        )))
    } else {
        Ok(limit)
    }
}

fn validate_id(label: &str, value: &str) -> CoreResult<()> {
    if value.is_empty()
        || value.len() > 256
        || value.trim() != value
        || value.chars().any(char::is_control)
    {
        Err(CoreError::invalid(format!("{label} id is invalid")))
    } else {
        Ok(())
    }
}

fn parse_time(label: &str, value: &str) -> CoreResult<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(value)
        .map(|value| value.with_timezone(&Utc))
        .map_err(|error| corrupted(format!("{label} is invalid: {error}")))
}

fn to_i64(value: u64) -> CoreResult<i64> {
    i64::try_from(value).map_err(|_| CoreError::invalid("delivery attempts exceed SQLite range"))
}

fn to_u64(value: i64) -> CoreResult<u64> {
    u64::try_from(value).map_err(|_| corrupted("stored delivery attempts are negative"))
}

fn delivery_conflict(occurrence_id: &str) -> CoreError {
    CoreError::invalid(format!(
        "lifecycle occurrence delivery CAS conflict for {occurrence_id}"
    ))
}

fn not_found(kind: &str) -> CoreError {
    CoreError::new(
        CoreErrorCode::NotFound,
        format!("{kind} was not found"),
        false,
    )
}

fn corrupted(message: impl Into<String>) -> CoreError {
    CoreError::new(CoreErrorCode::StorageCorrupted, message, false)
}
