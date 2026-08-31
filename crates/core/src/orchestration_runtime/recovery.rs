use chrono::Utc;
use lorepia_domain::{
    ConversationBranchId, ConversationId, CoreError, CoreErrorCode, CoreResult, GenerationId,
    MemoryJob, MemoryJobId,
};
use lorepia_storage::{
    GenerationApprovalEvidence, GenerationBeforeEventEvidence, LifecycleOccurrenceKind,
    MemoryJobInterruption, MemoryQueryEmbeddingStatus, StoredMemoryQueryEmbedding,
};
use serde::{Deserialize, Serialize};

use super::{
    ProcessedCoreLifecycleOccurrence,
    auxiliary_tasks::{ClaimedMemoryJob, claimed_memory_job, queue_entry_as_revisioned},
};
use crate::{Core, Revisioned};

const MAX_CORE_LIFECYCLE_DRAIN: u32 = 256;
const CORE_LIFECYCLE_LEASE_SECONDS: i64 = 30;
const CORE_LIFECYCLE_APPROVAL_POLL_SECONDS: i64 = 1;
const MAX_CORE_LIFECYCLE_RETRY_SECONDS: i64 = 300;
/// One interrupted job offered to the user for an explicit retry decision.
///
/// The projection carries only identifiers, counters, and the bounded
/// interruption audit trail. Raw message text never crosses this seam.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct InterruptedMemoryJob {
    pub job: Revisioned<MemoryJob>,
    pub interruptions: Vec<MemoryJobInterruption>,
}
/// Durable disposition of one claimed Core lifecycle occurrence.
///
/// Errors expose only a stable code. The occurrence itself remains in the
/// local outbox with a bounded retry time, so a terminal event can never be
/// dropped because an interaction rule, storage read, or policy check failed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case", deny_unknown_fields)]
pub enum CoreLifecycleDeliveryStatus {
    Acknowledged,
    AwaitingApproval {
        retry_at: chrono::DateTime<Utc>,
    },
    Deferred {
        error_code: CoreErrorCode,
        retry_at: chrono::DateTime<Utc>,
    },
}
/// Redacted receipt for one exact lifecycle outbox delivery attempt.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CoreLifecycleDeliveryReceipt {
    pub occurrence_id: String,
    pub event_kind: LifecycleOccurrenceKind,
    pub generation_id: Option<GenerationId>,
    pub delivery_attempts: u64,
    pub status: CoreLifecycleDeliveryStatus,
    pub before_generation_evidence: Option<GenerationBeforeEventEvidence>,
    pub approval_evidence: Option<GenerationApprovalEvidence>,
}
/// Result of one bounded synchronous lifecycle drain pass.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CoreLifecycleDrainReceipt {
    pub deliveries: Vec<CoreLifecycleDeliveryReceipt>,
    /// True only when a claim found no currently available occurrence.
    pub queue_idle: bool,
}
/// Credential-free, content-free projection for an explicit native retry UI.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MemoryQueryEmbeddingRetryCandidate {
    pub id: String,
    pub status: MemoryQueryEmbeddingStatus,
    pub revision: u64,
    pub conversation_id: ConversationId,
    pub branch_id: ConversationBranchId,
    pub error_code: Option<String>,
    /// Ambiguous provider outcomes require a separate positive user
    /// acknowledgement before the CAS retry is admitted.
    pub requires_unknown_outcome_acknowledgement: bool,
}
impl Core {
    /// Explicitly authorizes one retry after an ambiguous query-embedding
    /// provider outcome. Ordinary prompt preparation never calls this seam.
    pub fn list_retryable_memory_query_embeddings(
        &self,
        conversation_id: &ConversationId,
        branch_id: &ConversationBranchId,
        limit: u32,
    ) -> CoreResult<Vec<MemoryQueryEmbeddingRetryCandidate>> {
        self.storage()
            .list_retryable_memory_query_embeddings(conversation_id, branch_id, limit)?
            .into_iter()
            .map(memory_query_retry_candidate)
            .collect()
    }

    pub fn retry_memory_query_embedding(
        &self,
        conversation_id: &ConversationId,
        branch_id: &ConversationBranchId,
        id: &str,
        expected_revision: u64,
        acknowledge_unknown_outcome: bool,
    ) -> CoreResult<MemoryQueryEmbeddingRetryCandidate> {
        let current = self.storage().get_memory_query_embedding(id)?;
        if current.intent.conversation_id != *conversation_id
            || current.intent.branch_id != *branch_id
        {
            return Err(CoreError::new(
                CoreErrorCode::NotFound,
                "memory query embedding was not found",
                false,
            ));
        }
        if current.revision != expected_revision {
            return Err(CoreError::new(
                CoreErrorCode::StorageUnavailable,
                "memory query embedding retry lost its expected revision",
                true,
            ));
        }
        if current.status == MemoryQueryEmbeddingStatus::Interrupted && !acknowledge_unknown_outcome
        {
            return Err(CoreError::new(
                CoreErrorCode::PermissionDenied,
                "unknown provider outcome must be acknowledged before explicit retry",
                true,
            ));
        }
        self.storage()
            .retry_memory_query_embedding(
                conversation_id,
                branch_id,
                id,
                expected_revision,
                Utc::now(),
            )
            .and_then(memory_query_retry_candidate)
    }
}

impl Core {
    /// Converts abandoned provider work to `Interrupted` at startup. It never
    /// requeues work: retry remains an explicit CAS operation.
    pub fn recover_running_memory_jobs(&self) -> CoreResult<Vec<ClaimedMemoryJob>> {
        self.storage()
            .recover_running_memory_jobs(Utc::now())?
            .iter()
            .map(claimed_memory_job)
            .collect()
    }

    /// Marks abandoned query-embedding dispatches as interrupted. No provider
    /// request is made and ordinary prompt preparation cannot requeue them.
    pub fn recover_running_memory_query_embeddings(&self) -> CoreResult<usize> {
        self.storage()
            .recover_running_memory_query_embeddings(Utc::now())
            .map(|entries| entries.len())
    }

    /// Lists interrupted jobs on one branch so the shell can offer an explicit
    /// retry decision. Interrupted jobs are never requeued automatically, so
    /// this read is the only way a user can discover them.
    pub fn list_interrupted_memory_jobs(
        &self,
        conversation_id: &ConversationId,
        branch_id: &ConversationBranchId,
        limit: u32,
    ) -> CoreResult<Vec<InterruptedMemoryJob>> {
        Ok(self
            .storage()
            .list_interrupted_memory_jobs(conversation_id, branch_id, limit)?
            .iter()
            .map(|entry| InterruptedMemoryJob {
                job: queue_entry_as_revisioned(entry),
                interruptions: entry.interruptions.clone(),
            })
            .collect())
    }

    /// Explicitly requeues one interrupted job. Unknown provider side effects
    /// are therefore never retried merely because the process restarted.
    pub fn retry_interrupted_memory_job(
        &self,
        conversation_id: &ConversationId,
        branch_id: &ConversationBranchId,
        id: &MemoryJobId,
        expected_revision: u64,
    ) -> CoreResult<ClaimedMemoryJob> {
        let now = Utc::now();
        let entry = self.storage().retry_interrupted_memory_job(
            conversation_id,
            branch_id,
            id,
            expected_revision,
            now,
            now,
        )?;
        claimed_memory_job(&entry)
    }
}

impl Core {
    /// Claims, processes, and settles a bounded batch of durable Core
    /// lifecycle occurrences.
    ///
    /// Claiming is deliberately one-at-a-time. A failed or approval-blocked
    /// occurrence is returned to `pending` before another claim is attempted,
    /// which avoids holding unused leases and lets unrelated rooms continue.
    /// Every successful acknowledgement happens only after all local,
    /// idempotent consequences have committed.
    pub fn drain_core_lifecycle_occurrences(
        &self,
        max_occurrences: u32,
    ) -> CoreResult<CoreLifecycleDrainReceipt> {
        if !(1..=MAX_CORE_LIFECYCLE_DRAIN).contains(&max_occurrences) {
            return Err(CoreError::invalid(format!(
                "lifecycle drain limit must be between 1 and {MAX_CORE_LIFECYCLE_DRAIN}",
            )));
        }

        let mut deliveries = Vec::with_capacity(max_occurrences as usize);
        let mut queue_idle = false;
        while deliveries.len() < max_occurrences as usize {
            let claimed_at = Utc::now();
            let lease_until = claimed_at + chrono::Duration::seconds(CORE_LIFECYCLE_LEASE_SECONDS);
            let mut claimed =
                self.storage()
                    .claim_core_lifecycle_occurrences(claimed_at, lease_until, 1)?;
            let Some(occurrence) = claimed.pop() else {
                queue_idle = true;
                break;
            };

            match self.process_core_lifecycle_occurrence(&occurrence) {
                Ok(ProcessedCoreLifecycleOccurrence::Acknowledged {
                    before_generation_evidence,
                    approval_evidence,
                }) => {
                    self.storage().acknowledge_core_lifecycle_occurrence(
                        &occurrence.occurrence_id,
                        occurrence.delivery_attempts,
                        Utc::now(),
                    )?;
                    deliveries.push(CoreLifecycleDeliveryReceipt {
                        occurrence_id: occurrence.occurrence_id,
                        event_kind: occurrence.event_kind,
                        generation_id: occurrence.generation_id,
                        delivery_attempts: occurrence.delivery_attempts,
                        status: CoreLifecycleDeliveryStatus::Acknowledged,
                        before_generation_evidence,
                        approval_evidence,
                    });
                }
                Ok(ProcessedCoreLifecycleOccurrence::AwaitingApproval {
                    before_generation_evidence,
                }) => {
                    let retry_at = Utc::now()
                        + chrono::Duration::seconds(CORE_LIFECYCLE_APPROVAL_POLL_SECONDS);
                    self.storage().retry_core_lifecycle_occurrence_after(
                        &occurrence.occurrence_id,
                        occurrence.delivery_attempts,
                        retry_at,
                    )?;
                    deliveries.push(CoreLifecycleDeliveryReceipt {
                        occurrence_id: occurrence.occurrence_id,
                        event_kind: occurrence.event_kind,
                        generation_id: occurrence.generation_id,
                        delivery_attempts: occurrence.delivery_attempts,
                        status: CoreLifecycleDeliveryStatus::AwaitingApproval { retry_at },
                        before_generation_evidence,
                        approval_evidence: None,
                    });
                }
                Err(error) => {
                    let retry_at = Utc::now()
                        + chrono::Duration::seconds(core_lifecycle_retry_seconds(
                            occurrence.delivery_attempts,
                        ));
                    self.storage().retry_core_lifecycle_occurrence_after(
                        &occurrence.occurrence_id,
                        occurrence.delivery_attempts,
                        retry_at,
                    )?;
                    deliveries.push(CoreLifecycleDeliveryReceipt {
                        occurrence_id: occurrence.occurrence_id,
                        event_kind: occurrence.event_kind,
                        generation_id: occurrence.generation_id,
                        delivery_attempts: occurrence.delivery_attempts,
                        status: CoreLifecycleDeliveryStatus::Deferred {
                            error_code: error.code,
                            retry_at,
                        },
                        before_generation_evidence: None,
                        approval_evidence: None,
                    });
                }
            }
        }

        Ok(CoreLifecycleDrainReceipt {
            deliveries,
            queue_idle,
        })
    }

    /// Brings synchronous Core boundaries up to the available durable
    /// lifecycle frontier without waiting for future retries.
    ///
    /// Generation completion writes terminal occurrences atomically with the
    /// terminal message. Branch forks, historical actions, and room reopen
    /// projections must consume that already-durable work before reading the
    /// checkpoint or effect projection it owns. The bounded passes preserve
    /// backpressure if a process has accumulated an unusually large backlog.
    pub(crate) fn drain_available_core_lifecycle_occurrences(&self) -> CoreResult<()> {
        for _ in 0..8 {
            if self.drain_core_lifecycle_occurrences(64)?.queue_idle {
                return Ok(());
            }
        }
        Err(CoreError::new(
            CoreErrorCode::StorageUnavailable,
            "available interaction lifecycle backlog exceeds the synchronous drain bound",
            true,
        ))
    }

    /// Recovers only expired lifecycle leases during a live process.
    ///
    /// `Storage::open` separately resets every abandoned claim while holding
    /// the process-exclusive data-root owner lock.
    pub fn recover_expired_core_lifecycle_occurrence_leases(&self) -> CoreResult<u64> {
        self.storage()
            .recover_core_lifecycle_occurrence_leases(Utc::now())
    }
}

pub(crate) fn core_lifecycle_retry_seconds(delivery_attempts: u64) -> i64 {
    let exponent = delivery_attempts.saturating_sub(1).min(8) as u32;
    1_i64
        .checked_shl(exponent)
        .unwrap_or(MAX_CORE_LIFECYCLE_RETRY_SECONDS)
        .min(MAX_CORE_LIFECYCLE_RETRY_SECONDS)
}
fn memory_query_retry_candidate(
    stored: StoredMemoryQueryEmbedding,
) -> CoreResult<MemoryQueryEmbeddingRetryCandidate> {
    if !matches!(
        stored.status,
        MemoryQueryEmbeddingStatus::Interrupted
            | MemoryQueryEmbeddingStatus::Failed
            | MemoryQueryEmbeddingStatus::Cancelled
            | MemoryQueryEmbeddingStatus::Queued
    ) {
        return Err(CoreError::invalid(
            "memory query embedding is not in a retryable or explicitly requeued state",
        ));
    }
    Ok(MemoryQueryEmbeddingRetryCandidate {
        id: stored.intent.id,
        status: stored.status,
        revision: stored.revision,
        conversation_id: stored.intent.conversation_id,
        branch_id: stored.intent.branch_id,
        error_code: stored.error_code,
        requires_unknown_outcome_acknowledgement: stored.status
            == MemoryQueryEmbeddingStatus::Interrupted,
    })
}
