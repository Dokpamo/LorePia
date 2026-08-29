use std::future::Future;
use std::time::Duration;

use lorepia_domain::{CoreError, CoreResult};
use tokio::runtime::{Builder, Handle};

const RUNTIME_SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(1);

pub(super) struct RuntimeControl {
    handle: Handle,
    shutdown_sender: Option<tokio::sync::oneshot::Sender<()>>,
    owner_thread: Option<std::thread::JoinHandle<()>>,
}

impl RuntimeControl {
    pub(super) fn start() -> CoreResult<Self> {
        let (ready_sender, ready_receiver) =
            std::sync::mpsc::sync_channel::<Result<Handle, String>>(1);
        let (shutdown_sender, shutdown_receiver) = tokio::sync::oneshot::channel();
        let owner_thread = std::thread::Builder::new()
            .name("lorepia-core-owner".to_owned())
            .spawn(move || {
                let runtime = match Builder::new_multi_thread()
                    .worker_threads(2)
                    .enable_all()
                    .thread_name("lorepia-core-worker")
                    .build()
                {
                    Ok(runtime) => runtime,
                    Err(error) => {
                        let _ = ready_sender
                            .send(Err(format!("cannot create core async runtime: {error}")));
                        return;
                    }
                };
                if ready_sender.send(Ok(runtime.handle().clone())).is_err() {
                    runtime.shutdown_timeout(RUNTIME_SHUTDOWN_TIMEOUT);
                    return;
                }
                let _ = runtime.block_on(shutdown_receiver);
                runtime.shutdown_timeout(RUNTIME_SHUTDOWN_TIMEOUT);
            })
            .map_err(|error| {
                CoreError::internal(format!("cannot start core runtime owner: {error}"))
            })?;

        match ready_receiver.recv() {
            Ok(Ok(handle)) => Ok(Self {
                handle,
                shutdown_sender: Some(shutdown_sender),
                owner_thread: Some(owner_thread),
            }),
            Ok(Err(message)) => {
                let _ = owner_thread.join();
                Err(CoreError::internal(message))
            }
            Err(error) => {
                let _ = owner_thread.join();
                Err(CoreError::internal(format!(
                    "core runtime owner stopped during startup: {error}"
                )))
            }
        }
    }

    pub(super) fn spawn(&self, future: impl Future<Output = ()> + Send + 'static) {
        std::mem::drop(self.handle.spawn(future));
    }

    pub(super) fn handle(&self) -> &Handle {
        &self.handle
    }

    pub(super) fn shutdown(&mut self) {
        if let Some(sender) = self.shutdown_sender.take() {
            let _ = sender.send(());
        }
        if let Some(owner_thread) = self.owner_thread.take() {
            let _ = owner_thread.join();
        }
    }
}
