use chrono::{Duration, Utc};
use lorepia_domain::CoreResult;

use crate::{Core, orchestration_runtime::core_lifecycle_retry_seconds};

const MAX_INTERACTION_DERIVED_DRAIN: u32 = 256;
const INTERACTION_DERIVED_LEASE_SECONDS: i64 = 30;

/// Content-free summary of one bounded derived-event drain pass.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InteractionDerivedDrainReceipt {
    pub committed_count: u32,
}

impl InteractionDerivedDrainReceipt {
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.committed_count == 0
    }
}

impl Core {
    /// Drains durable VariableChanged/KnowledgeActivated occurrences through
    /// the same compiled policy and state-CAS path as ordinary events.
    ///
    /// Each occurrence is claimed at least once. Storage commits the derived
    /// transition, any child occurrences, and the acknowledgement in one
    /// transaction, so response loss or restart cannot duplicate an action.
    pub fn drain_interaction_derived_events(&self) -> CoreResult<InteractionDerivedDrainReceipt> {
        let mut committed_count = 0;
        for _ in 0..MAX_INTERACTION_DERIVED_DRAIN {
            let now = Utc::now();
            let mut claimed = self.storage().claim_interaction_derived_events(
                now,
                now + Duration::seconds(INTERACTION_DERIVED_LEASE_SECONDS),
                1,
            )?;
            let Some(occurrence) = claimed.pop() else {
                break;
            };
            match self.process_interaction_derived_occurrence(&occurrence) {
                Ok(Some(_)) => committed_count += 1,
                Ok(None) => {}
                Err(error) => {
                    let retry_at = Utc::now()
                        + Duration::seconds(core_lifecycle_retry_seconds(
                            occurrence.delivery_attempts,
                        ));
                    self.storage().retry_interaction_derived_event_after(
                        &occurrence.occurrence_id,
                        occurrence.delivery_attempts,
                        retry_at,
                    )?;
                    return Err(error);
                }
            }
        }
        Ok(InteractionDerivedDrainReceipt { committed_count })
    }
}
