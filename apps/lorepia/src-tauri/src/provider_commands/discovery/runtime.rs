use std::{
    collections::HashMap,
    sync::{Mutex, OnceLock},
};

use uuid::Uuid;

use crate::error::{CommandError, CommandResult};

struct ActiveDiscoveryRequest {
    request_id: Uuid,
    cancel: tokio::sync::watch::Sender<bool>,
}

pub(in crate::provider_commands) struct ActiveDiscoveryRequestRegistration {
    session_id: String,
    request_id: Uuid,
}

impl Drop for ActiveDiscoveryRequestRegistration {
    fn drop(&mut self) {
        if let Ok(mut requests) = active_discovery_requests().lock()
            && requests
                .get(&self.session_id)
                .is_some_and(|request| request.request_id == self.request_id)
        {
            requests.remove(&self.session_id);
        }
    }
}

fn active_discovery_requests() -> &'static Mutex<HashMap<String, ActiveDiscoveryRequest>> {
    static REQUESTS: OnceLock<Mutex<HashMap<String, ActiveDiscoveryRequest>>> = OnceLock::new();
    REQUESTS.get_or_init(|| Mutex::new(HashMap::new()))
}

pub(in crate::provider_commands) fn register_active_discovery_request(
    session_id: &str,
) -> CommandResult<(
    ActiveDiscoveryRequestRegistration,
    tokio::sync::watch::Receiver<bool>,
)> {
    let mut requests = active_discovery_requests()
        .lock()
        .map_err(|_| CommandError::internal())?;
    if requests.contains_key(session_id) {
        return Err(CommandError::busy());
    }
    let request_id = Uuid::new_v4();
    let (cancel, cancelled) = tokio::sync::watch::channel(false);
    requests.insert(
        session_id.to_owned(),
        ActiveDiscoveryRequest { request_id, cancel },
    );
    Ok((
        ActiveDiscoveryRequestRegistration {
            session_id: session_id.to_owned(),
            request_id,
        },
        cancelled,
    ))
}

pub(super) fn signal_active_discovery_request_cancellation(session_id: &str) -> CommandResult<()> {
    let requests = active_discovery_requests()
        .lock()
        .map_err(|_| CommandError::internal())?;
    if let Some(request) = requests.get(session_id) {
        let _ = request.cancel.send(true);
    }
    Ok(())
}
