//! Keep synchronous Storage transactions and output transforms off async
//! network workers. Generation admission bounds awaiters; only two closures
//! can be submitted to Tokio's blocking pool for this Core at once.

use std::sync::Arc;

use lorepia_domain::{CoreError, CoreResult};
use tokio::sync::Semaphore;

#[derive(Clone)]
pub(in crate::app) struct BlockingWork {
    permits: Arc<Semaphore>,
}

impl Default for BlockingWork {
    fn default() -> Self {
        Self {
            permits: Arc::new(Semaphore::new(2)),
        }
    }
}

impl BlockingWork {
    pub(in crate::app) async fn run<T: Send + 'static>(
        &self,
        operation: impl FnOnce() -> T + Send + 'static,
    ) -> CoreResult<T> {
        let permit = Arc::clone(&self.permits)
            .acquire_owned()
            .await
            .map_err(|_| CoreError::internal("Core blocking execution is closed"))?;
        tokio::task::spawn_blocking(move || {
            // A cancelled awaiter cannot recycle admission while native work
            // still runs. The operation remains bounded by its owning use case.
            let _permit = permit;
            operation()
        })
        .await
        .map_err(|_| CoreError::internal("Core blocking operation stopped unexpectedly"))
    }
}

impl crate::Core {
    pub(in crate::app) async fn drain_interaction_derived_events_blocking(
        &self,
    ) -> std::time::Duration {
        self.run_blocking(|core| {
            let _ = core.drain_interaction_derived_events();
            Ok(super::super::interaction_derived_supervisor_delay(
                core.storage(),
                chrono::Utc::now(),
            ))
        })
        .await
        .unwrap_or(super::super::INTERACTION_DERIVED_SUPERVISOR_ERROR_POLL)
    }

    pub(crate) async fn run_blocking<T: Send + 'static>(
        &self,
        operation: impl FnOnce(&Self) -> CoreResult<T> + Send + 'static,
    ) -> CoreResult<T> {
        let worker = self.clone();
        self.inner
            .runtime
            .blocking()
            .run(move || operation(&worker))
            .await?
    }
}

#[cfg(test)]
mod tests;
