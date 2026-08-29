use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
};

use tokio::sync::oneshot;
use uuid::Uuid;

use crate::error::{CommandError, CommandResult};

pub(crate) const MAXIMUM_CHAT_STREAMS: usize = 32;

pub(crate) struct ChatStreamRegistry {
    slots: Mutex<HashMap<String, ChatStreamSlot>>,
    capacity: usize,
}

struct ChatStreamSlot {
    marker: Arc<()>,
    dispose: Option<oneshot::Sender<()>>,
}

/// Owns one bounded renderer subscription without owning its Core generation.
///
/// Dropping this value unregisters only the forwarding receiver. Explicit Core
/// cancellation remains a separate command.
pub(crate) struct ChatStreamRegistration {
    stream_id: String,
    marker: Arc<()>,
    dispose: oneshot::Receiver<()>,
    registry: Arc<ChatStreamRegistry>,
}

impl ChatStreamRegistry {
    pub(crate) fn new(capacity: usize) -> Self {
        Self {
            slots: Mutex::new(HashMap::new()),
            capacity,
        }
    }

    pub(crate) fn register(
        self: &Arc<Self>,
        stream_id: &str,
    ) -> CommandResult<ChatStreamRegistration> {
        validate_stream_id(stream_id)?;
        let mut slots = self.slots.lock().map_err(|_| CommandError::internal())?;
        if slots.contains_key(stream_id) {
            return Err(CommandError::invalid_input());
        }
        if slots.len() >= self.capacity {
            return Err(CommandError::busy());
        }

        let marker = Arc::new(());
        let (dispose, dispose_receiver) = oneshot::channel();
        slots.insert(
            stream_id.to_owned(),
            ChatStreamSlot {
                marker: Arc::clone(&marker),
                dispose: Some(dispose),
            },
        );
        Ok(ChatStreamRegistration {
            stream_id: stream_id.to_owned(),
            marker,
            dispose: dispose_receiver,
            registry: Arc::clone(self),
        })
    }

    pub(crate) fn dispose(&self, stream_id: &str) -> CommandResult<bool> {
        validate_stream_id(stream_id)?;
        let mut slots = self.slots.lock().map_err(|_| CommandError::internal())?;
        let Some(slot) = slots.get_mut(stream_id) else {
            return Ok(false);
        };
        let Some(dispose) = slot.dispose.take() else {
            return Ok(false);
        };
        let _ = dispose.send(());
        Ok(true)
    }

    fn finish(&self, stream_id: &str, marker: &Arc<()>) {
        let Ok(mut slots) = self.slots.lock() else {
            return;
        };
        if slots
            .get(stream_id)
            .is_some_and(|slot| Arc::ptr_eq(&slot.marker, marker))
        {
            slots.remove(stream_id);
        }
    }
}

impl ChatStreamRegistration {
    pub(crate) async fn disposed(&mut self) {
        let _ = (&mut self.dispose).await;
    }
}

impl Drop for ChatStreamRegistration {
    fn drop(&mut self) {
        self.registry.finish(&self.stream_id, &self.marker);
    }
}

fn validate_stream_id(stream_id: &str) -> CommandResult<()> {
    if Uuid::parse_str(stream_id).is_ok_and(|value| value.to_string() == stream_id) {
        Ok(())
    } else {
        Err(CommandError::invalid_input())
    }
}
