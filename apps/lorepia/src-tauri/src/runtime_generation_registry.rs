use std::{
    collections::{HashMap, VecDeque},
    sync::{Arc, Mutex},
    time::Duration,
};

use tokio::sync::watch;
use uuid::Uuid;

use crate::error::{CommandError, CommandResult};

pub(crate) const MAXIMUM_RUNTIME_GENERATIONS: usize = 16;
const RUNTIME_GENERATION_TERMINAL_ACK_TIMEOUT: Duration = Duration::from_secs(5);

pub(crate) struct RuntimeGenerationRegistry {
    state: Mutex<RuntimeGenerationRegistryState>,
    capacity: usize,
    terminal_ack_timeout: Duration,
}

#[derive(Default)]
struct RuntimeGenerationRegistryState {
    slots: HashMap<String, RuntimeGenerationSlot>,
    cancellation_tombstones: VecDeque<RuntimeGenerationCancellationTombstone>,
}

struct RuntimeGenerationCancellationTombstone {
    request_id: String,
    terminal: watch::Sender<bool>,
}

struct RuntimeGenerationSlot {
    marker: Arc<()>,
    cancel: watch::Sender<bool>,
    terminal: watch::Sender<bool>,
}

pub(crate) struct RuntimeGenerationRegistration {
    request_id: String,
    marker: Arc<()>,
    cancelled: watch::Receiver<bool>,
    registry: Arc<RuntimeGenerationRegistry>,
}

impl RuntimeGenerationRegistry {
    pub(crate) fn new(capacity: usize) -> Self {
        Self {
            state: Mutex::new(RuntimeGenerationRegistryState::default()),
            capacity,
            terminal_ack_timeout: RUNTIME_GENERATION_TERMINAL_ACK_TIMEOUT,
        }
    }

    #[cfg(test)]
    fn new_with_terminal_ack_timeout(capacity: usize, terminal_ack_timeout: Duration) -> Self {
        Self {
            state: Mutex::new(RuntimeGenerationRegistryState::default()),
            capacity,
            terminal_ack_timeout,
        }
    }

    pub(crate) fn register(
        self: &Arc<Self>,
        request_id: &str,
    ) -> CommandResult<RuntimeGenerationRegistration> {
        validate_request_id(request_id)?;
        let mut state = self.state.lock().map_err(|_| CommandError::internal())?;
        if state.slots.contains_key(request_id) {
            return Err(CommandError::invalid_input());
        }
        if state.slots.len() >= self.capacity {
            return Err(CommandError::busy());
        }
        let cancellation_tombstone = state.take_cancellation_tombstone(request_id);
        let cancelled_before_registration = cancellation_tombstone.is_some();
        let marker = Arc::new(());
        let (cancel, cancelled) = watch::channel(cancelled_before_registration);
        let terminal = cancellation_tombstone.unwrap_or_else(|| {
            let (terminal, _terminal_receiver) = watch::channel(false);
            terminal
        });
        state.slots.insert(
            request_id.to_owned(),
            RuntimeGenerationSlot {
                marker: Arc::clone(&marker),
                cancel,
                terminal,
            },
        );
        Ok(RuntimeGenerationRegistration {
            request_id: request_id.to_owned(),
            marker,
            cancelled,
            registry: Arc::clone(self),
        })
    }

    /// Requests cancellation and acknowledges only after the exact native
    /// generation command has reached its terminal scope and released its
    /// registration. A pre-registration race is retained in a bounded FIFO;
    /// timeout returns `false` without discarding that bounded pending
    /// cancellation, while eviction or an unavailable slot also returns
    /// `false`.
    pub(crate) async fn cancel(&self, request_id: &str) -> CommandResult<bool> {
        validate_request_id(request_id)?;
        let mut terminal = {
            let mut state = self.state.lock().map_err(|_| CommandError::internal())?;
            if let Some(slot) = state.slots.get(request_id) {
                slot.cancel.send_replace(true);
                slot.terminal.subscribe()
            } else {
                let Some(terminal) =
                    state.remember_cancellation_tombstone(request_id, self.capacity)
                else {
                    return Ok(false);
                };
                terminal
            }
        };

        Ok(matches!(
            tokio::time::timeout(
                self.terminal_ack_timeout,
                terminal.wait_for(|is_terminal| *is_terminal),
            )
            .await,
            Ok(Ok(_))
        ))
    }

    fn finish(&self, request_id: &str, marker: &Arc<()>) {
        let Ok(mut state) = self.state.lock() else {
            return;
        };
        let matches_registration = state
            .slots
            .get(request_id)
            .is_some_and(|slot| Arc::ptr_eq(&slot.marker, marker));
        let terminal = if matches_registration {
            state.slots.remove(request_id).map(|slot| slot.terminal)
        } else {
            None
        };
        drop(state);
        if let Some(terminal) = terminal {
            terminal.send_replace(true);
        }
    }
}

impl RuntimeGenerationRegistryState {
    fn take_cancellation_tombstone(&mut self, request_id: &str) -> Option<watch::Sender<bool>> {
        let position = self
            .cancellation_tombstones
            .iter()
            .position(|candidate| candidate.request_id == request_id)?;
        self.cancellation_tombstones
            .remove(position)
            .map(|tombstone| tombstone.terminal)
    }

    fn remember_cancellation_tombstone(
        &mut self,
        request_id: &str,
        capacity: usize,
    ) -> Option<watch::Receiver<bool>> {
        if capacity == 0 {
            return None;
        }
        if let Some(existing) = self
            .cancellation_tombstones
            .iter()
            .find(|candidate| candidate.request_id == request_id)
        {
            return Some(existing.terminal.subscribe());
        }
        while self.cancellation_tombstones.len() >= capacity {
            self.cancellation_tombstones.pop_front();
        }
        let (terminal, receiver) = watch::channel(false);
        self.cancellation_tombstones
            .push_back(RuntimeGenerationCancellationTombstone {
                request_id: request_id.to_owned(),
                terminal,
            });
        Some(receiver)
    }
}

impl RuntimeGenerationRegistration {
    pub(crate) fn cancelled(&self) -> watch::Receiver<bool> {
        self.cancelled.clone()
    }
}

impl Drop for RuntimeGenerationRegistration {
    fn drop(&mut self) {
        self.registry.finish(&self.request_id, &self.marker);
    }
}

fn validate_request_id(request_id: &str) -> CommandResult<()> {
    if Uuid::parse_str(request_id).is_ok_and(|value| value.to_string() == request_id) {
        Ok(())
    } else {
        Err(CommandError::invalid_input())
    }
}

#[cfg(test)]
mod tests {
    use std::{sync::Arc, time::Duration};

    use super::RuntimeGenerationRegistry;

    #[tokio::test]
    async fn registry_is_bounded_and_acknowledges_only_a_finished_registration() {
        const FIRST_ID: &str = "00000000-0000-4000-8000-000000000011";
        const SECOND_ID: &str = "00000000-0000-4000-8000-000000000012";

        let registry = Arc::new(RuntimeGenerationRegistry::new(1));
        let first = registry.register(FIRST_ID).expect("first registration");
        let mut first_cancelled = first.cancelled();
        assert!(!*first_cancelled.borrow());
        assert_eq!(
            registry
                .register(FIRST_ID)
                .err()
                .expect("duplicate identifier")
                .code,
            "invalid_input"
        );
        assert_eq!(
            registry
                .register(SECOND_ID)
                .err()
                .expect("bounded registry")
                .code,
            "busy"
        );

        let cancellation_registry = Arc::clone(&registry);
        let cancellation = tokio::spawn(async move {
            cancellation_registry
                .cancel(FIRST_ID)
                .await
                .expect("cancel first")
        });
        first_cancelled
            .changed()
            .await
            .expect("registration observes cancellation");
        assert!(*first_cancelled.borrow());
        assert!(
            !cancellation.is_finished(),
            "cancellation must not acknowledge a live native future"
        );
        drop(first);
        assert!(
            cancellation
                .await
                .expect("join cancellation acknowledgement")
        );
        assert!(
            !registry
                .state
                .lock()
                .expect("registry state")
                .slots
                .contains_key(FIRST_ID),
            "terminal acknowledgement must follow exact slot removal"
        );

        let second_lifetime = registry
            .register(FIRST_ID)
            .expect("identifier may be reused after completion");
        assert!(!*second_lifetime.cancelled().borrow());
    }

    #[tokio::test]
    async fn cancellation_before_registration_is_consumed_once() {
        const REQUEST_ID: &str = "00000000-0000-4000-8000-000000000013";

        let registry = Arc::new(RuntimeGenerationRegistry::new(1));
        let cancellation_registry = Arc::clone(&registry);
        let cancellation = tokio::spawn(async move {
            cancellation_registry
                .cancel(REQUEST_ID)
                .await
                .expect("remember pre-registration cancellation")
        });
        wait_for_tombstone(&registry, REQUEST_ID).await;
        assert!(
            !cancellation.is_finished(),
            "pre-registration cancellation must await the exact native future"
        );
        let cancelled_registration = registry
            .register(REQUEST_ID)
            .expect("consume cancellation tombstone");
        assert!(*cancelled_registration.cancelled().borrow());
        assert!(
            !cancellation.is_finished(),
            "registration alone is not a terminal acknowledgement"
        );
        drop(cancelled_registration);
        assert!(
            cancellation
                .await
                .expect("join pre-registration cancellation")
        );

        let next_registration = registry
            .register(REQUEST_ID)
            .expect("tombstone is consumed exactly once");
        assert!(!*next_registration.cancelled().borrow());
    }

    #[tokio::test]
    async fn cancellation_tombstones_evict_oldest_at_capacity() {
        const EVICTED_ID: &str = "00000000-0000-4000-8000-000000000014";
        const RETAINED_ID: &str = "00000000-0000-4000-8000-000000000015";

        let registry = Arc::new(RuntimeGenerationRegistry::new(1));
        let first_registry = Arc::clone(&registry);
        let first = tokio::spawn(async move {
            first_registry
                .cancel(EVICTED_ID)
                .await
                .expect("first tombstone")
        });
        wait_for_tombstone(&registry, EVICTED_ID).await;
        let second_registry = Arc::clone(&registry);
        let second = tokio::spawn(async move {
            second_registry
                .cancel(RETAINED_ID)
                .await
                .expect("second tombstone")
        });
        wait_for_tombstone(&registry, RETAINED_ID).await;
        assert!(!first.await.expect("evicted cancellation waiter"));

        let evicted = registry.register(EVICTED_ID).expect("register evicted id");
        assert!(!*evicted.cancelled().borrow());
        drop(evicted);
        let retained = registry
            .register(RETAINED_ID)
            .expect("register retained id");
        assert!(*retained.cancelled().borrow());
        drop(retained);
        assert!(second.await.expect("retained cancellation waiter"));
    }

    #[tokio::test]
    async fn cancellation_timeout_never_reports_terminal_success() {
        const REQUEST_ID: &str = "00000000-0000-4000-8000-000000000016";

        let registry = Arc::new(RuntimeGenerationRegistry::new_with_terminal_ack_timeout(
            1,
            Duration::from_millis(10),
        ));
        let registration = registry.register(REQUEST_ID).expect("registration");
        assert!(
            !registry
                .cancel(REQUEST_ID)
                .await
                .expect("bounded cancellation acknowledgement")
        );
        assert!(*registration.cancelled().borrow());
    }

    #[tokio::test]
    async fn terminal_acknowledgement_is_scoped_to_the_exact_request() {
        const FIRST_ID: &str = "00000000-0000-4000-8000-000000000018";
        const SECOND_ID: &str = "00000000-0000-4000-8000-000000000019";

        let registry = Arc::new(RuntimeGenerationRegistry::new(2));
        let first = registry.register(FIRST_ID).expect("first registration");
        let second = registry.register(SECOND_ID).expect("second registration");
        let mut first_cancelled = first.cancelled();
        let cancellation_registry = Arc::clone(&registry);
        let cancellation = tokio::spawn(async move {
            cancellation_registry
                .cancel(FIRST_ID)
                .await
                .expect("cancel first registration")
        });
        first_cancelled
            .changed()
            .await
            .expect("first registration observes cancellation");
        drop(first);
        assert!(cancellation.await.expect("join exact cancellation"));
        assert!(
            !*second.cancelled().borrow(),
            "another active registration must neither block nor inherit the acknowledgement"
        );
    }

    #[tokio::test]
    async fn pre_registration_timeout_retains_its_bounded_tombstone() {
        const REQUEST_ID: &str = "00000000-0000-4000-8000-000000000017";

        let registry = Arc::new(RuntimeGenerationRegistry::new_with_terminal_ack_timeout(
            1,
            Duration::from_millis(10),
        ));
        assert!(
            !registry
                .cancel(REQUEST_ID)
                .await
                .expect("pre-registration cancellation times out")
        );
        let later_registration = registry
            .register(REQUEST_ID)
            .expect("consume timed-out cancellation tombstone");
        assert!(
            *later_registration.cancelled().borrow(),
            "a false terminal acknowledgement must not discard the pending cancellation"
        );
    }

    #[tokio::test]
    async fn registry_rejects_noncanonical_identifiers() {
        let registry = Arc::new(RuntimeGenerationRegistry::new(1));
        let Err(registration_error) = registry.register("not-an-opaque-request-id") else {
            panic!("invalid request identifier must be rejected");
        };
        assert_eq!(registration_error.code, "invalid_input");
        assert_eq!(
            registry
                .cancel("not-an-opaque-request-id")
                .await
                .expect_err("invalid cancellation identifier")
                .code,
            "invalid_input"
        );
    }

    async fn wait_for_tombstone(registry: &RuntimeGenerationRegistry, request_id: &str) {
        tokio::time::timeout(Duration::from_secs(1), async {
            loop {
                let is_present = registry
                    .state
                    .lock()
                    .expect("registry state")
                    .cancellation_tombstones
                    .iter()
                    .any(|candidate| candidate.request_id == request_id);
                if is_present {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("cancellation tombstone was not installed");
    }
}
