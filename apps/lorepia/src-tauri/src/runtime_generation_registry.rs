use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
};

use tokio::sync::watch;
use uuid::Uuid;

use crate::error::{CommandError, CommandResult};

pub(crate) const MAXIMUM_RUNTIME_GENERATIONS: usize = 16;

pub(crate) struct RuntimeGenerationRegistry {
    slots: Mutex<HashMap<String, RuntimeGenerationSlot>>,
    capacity: usize,
}

struct RuntimeGenerationSlot {
    marker: Arc<()>,
    cancel: watch::Sender<bool>,
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
            slots: Mutex::new(HashMap::new()),
            capacity,
        }
    }

    pub(crate) fn register(
        self: &Arc<Self>,
        request_id: &str,
    ) -> CommandResult<RuntimeGenerationRegistration> {
        validate_request_id(request_id)?;
        let mut slots = self.slots.lock().map_err(|_| CommandError::internal())?;
        if slots.contains_key(request_id) {
            return Err(CommandError::invalid_input());
        }
        if slots.len() >= self.capacity {
            return Err(CommandError::busy());
        }
        let marker = Arc::new(());
        let (cancel, cancelled) = watch::channel(false);
        slots.insert(
            request_id.to_owned(),
            RuntimeGenerationSlot {
                marker: Arc::clone(&marker),
                cancel,
            },
        );
        Ok(RuntimeGenerationRegistration {
            request_id: request_id.to_owned(),
            marker,
            cancelled,
            registry: Arc::clone(self),
        })
    }

    pub(crate) fn cancel(&self, request_id: &str) -> CommandResult<bool> {
        validate_request_id(request_id)?;
        let slots = self.slots.lock().map_err(|_| CommandError::internal())?;
        let Some(slot) = slots.get(request_id) else {
            return Ok(false);
        };
        Ok(slot.cancel.send(true).is_ok())
    }

    fn finish(&self, request_id: &str, marker: &Arc<()>) {
        let Ok(mut slots) = self.slots.lock() else {
            return;
        };
        if slots
            .get(request_id)
            .is_some_and(|slot| Arc::ptr_eq(&slot.marker, marker))
        {
            slots.remove(request_id);
        }
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
    use std::sync::Arc;

    use super::RuntimeGenerationRegistry;

    #[test]
    fn registry_is_bounded_and_cancels_exact_registration() {
        const FIRST_ID: &str = "00000000-0000-4000-8000-000000000011";
        const SECOND_ID: &str = "00000000-0000-4000-8000-000000000012";

        let registry = Arc::new(RuntimeGenerationRegistry::new(1));
        let first = registry.register(FIRST_ID).expect("first registration");
        let first_cancelled = first.cancelled();
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

        assert!(registry.cancel(FIRST_ID).expect("cancel first"));
        assert!(*first_cancelled.borrow());
        drop(first);
        assert!(!registry.cancel(FIRST_ID).expect("completed registration"));

        let second_lifetime = registry
            .register(FIRST_ID)
            .expect("identifier may be reused after completion");
        assert!(!*second_lifetime.cancelled().borrow());
    }

    #[test]
    fn registry_rejects_noncanonical_identifiers() {
        let registry = Arc::new(RuntimeGenerationRegistry::new(1));
        let Err(registration_error) = registry.register("not-an-opaque-request-id") else {
            panic!("invalid request identifier must be rejected");
        };
        assert_eq!(registration_error.code, "invalid_input");
        assert_eq!(
            registry
                .cancel("not-an-opaque-request-id")
                .expect_err("invalid cancellation identifier")
                .code,
            "invalid_input"
        );
    }
}
