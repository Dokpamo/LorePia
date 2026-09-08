use std::{
    sync::{Arc, Condvar, Mutex},
    time::Duration,
};

use tauri::{
    AppHandle, Manager, UriSchemeResponder,
    http::{Request, Response},
};
use tokio::sync::{OwnedSemaphorePermit, Semaphore};

use super::{MAX_RENDERABLE_ASSET_BYTES, handle, overloaded_response, preflight_response};

const MAX_INFLIGHT_REQUESTS: usize = 4;
const MAX_INFLIGHT_BYTES: u64 = 2 * MAX_RENDERABLE_ASSET_BYTES;
// One complete supported message fan-out may wait without creating unbounded
// blocking jobs. The four work slots include replies awaiting native delivery.
const MAX_QUEUED_REQUESTS: usize = 128;
const ADMISSION_WAIT: Duration = Duration::from_secs(30);

#[derive(Clone)]
pub(crate) struct AssetProtocolAdmission {
    inner: Arc<AdmissionInner>,
}

struct AdmissionInner {
    work: Arc<Semaphore>,
    queue: Arc<Semaphore>,
    max_bytes: u64,
    bytes: Mutex<u64>,
    bytes_released: Condvar,
}

struct QueuedRequest {
    inner: Arc<AdmissionInner>,
    _queue: OwnedSemaphorePermit,
}

pub(super) struct AssetProtocolPermit {
    inner: Arc<AdmissionInner>,
    _work: OwnedSemaphorePermit,
    reserved_bytes: u64,
}

#[derive(Clone)]
struct ResponseLease {
    _permit: Arc<AssetProtocolPermit>,
}

impl AssetProtocolAdmission {
    fn new(max_requests: usize, max_bytes: u64) -> Self {
        Self {
            inner: Arc::new(AdmissionInner {
                work: Arc::new(Semaphore::new(max_requests)),
                queue: Arc::new(Semaphore::new(MAX_QUEUED_REQUESTS)),
                max_bytes,
                bytes: Mutex::new(0),
                bytes_released: Condvar::new(),
            }),
        }
    }

    fn try_queue(&self) -> Option<QueuedRequest> {
        Some(QueuedRequest {
            inner: Arc::clone(&self.inner),
            _queue: Arc::clone(&self.inner.queue).try_acquire_owned().ok()?,
        })
    }

    pub(crate) fn respond(
        &self,
        app: AppHandle,
        request: Request<Vec<u8>>,
        responder: UriSchemeResponder,
    ) {
        if let Some(response) = preflight_response(&request) {
            responder.respond(response);
            return;
        }
        let Some(queued) = self.try_queue() else {
            responder.respond(overloaded_response());
            return;
        };
        std::mem::drop(tauri::async_runtime::spawn(async move {
            let Some(mut permit) = queued.acquire().await else {
                responder.respond(overloaded_response());
                return;
            };
            std::mem::drop(tauri::async_runtime::spawn_blocking(move || {
                let mut response = handle(app.state(), request, &mut permit);
                retain_permit(&mut response, permit);
                responder.respond(response);
            }));
        }));
    }

    #[cfg(test)]
    pub(super) fn try_acquire(&self) -> Option<AssetProtocolPermit> {
        Some(AssetProtocolPermit {
            inner: Arc::clone(&self.inner),
            _work: Arc::clone(&self.inner.work).try_acquire_owned().ok()?,
            reserved_bytes: 0,
        })
    }

    #[cfg(test)]
    pub(super) fn reserved_bytes(&self) -> u64 {
        *self.inner.bytes.lock().expect("byte budget")
    }
}

impl Default for AssetProtocolAdmission {
    fn default() -> Self {
        Self::new(MAX_INFLIGHT_REQUESTS, MAX_INFLIGHT_BYTES)
    }
}

impl QueuedRequest {
    async fn acquire(self) -> Option<AssetProtocolPermit> {
        let work =
            tokio::time::timeout(ADMISSION_WAIT, Arc::clone(&self.inner.work).acquire_owned())
                .await
                .ok()?
                .ok()?;
        Some(AssetProtocolPermit {
            inner: Arc::clone(&self.inner),
            _work: work,
            reserved_bytes: 0,
        })
    }
}

impl AssetProtocolPermit {
    /// Called only after the approved descriptor/range establishes the exact
    /// body length, and before Storage allocates/reads any response bytes.
    pub(super) fn reserve_bytes(&mut self, bytes: u64) -> bool {
        if self.reserved_bytes != 0 || bytes > self.inner.max_bytes {
            return false;
        }
        let Ok(active) = self.inner.bytes.lock() else {
            return false;
        };
        let Ok((mut active, _timeout)) =
            self.inner
                .bytes_released
                .wait_timeout_while(active, ADMISSION_WAIT, |active| {
                    bytes > self.inner.max_bytes - *active
                })
        else {
            return false;
        };
        if bytes > self.inner.max_bytes - *active {
            return false;
        }
        *active += bytes;
        self.reserved_bytes = bytes;
        true
    }
}

impl Drop for AssetProtocolPermit {
    fn drop(&mut self) {
        if let Ok(mut active) = self.inner.bytes.lock() {
            *active -= self.reserved_bytes;
            self.inner.bytes_released.notify_all();
        }
    }
}

pub(super) fn retain_permit(response: &mut Response<Vec<u8>>, permit: AssetProtocolPermit) {
    // HTTP extensions survive Tauri/Wry Vec-to-Cow conversion and keep both
    // budgets charged until the native response is actually consumed.
    let previous = response.extensions_mut().insert(ResponseLease {
        _permit: Arc::new(permit),
    });
    debug_assert!(previous.is_none());
}

#[cfg(test)]
mod tests;
