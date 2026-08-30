fn assert_directory_does_not_contain(root: &Path, needle: &[u8]) {
    for entry in fs::read_dir(root).expect("read data directory") {
        let entry = entry.expect("data directory entry");
        let path = entry.path();
        if path.is_dir() {
            assert_directory_does_not_contain(&path, needle);
        } else if path.is_file() {
            let contents = fs::read(&path).expect("read persisted data");
            assert!(
                !contents
                    .windows(needle.len())
                    .any(|window| window == needle),
                "secret material was persisted in {}",
                path.display()
            );
        }
    }
}

impl CapturingProvider {
    fn new(response: impl Into<String>) -> (Arc<Self>, std_mpsc::Receiver<Vec<String>>) {
        let (sender, receiver) = std_mpsc::channel();
        (
            Arc::new(Self {
                response: response.into(),
                captured: Mutex::new(Some(sender)),
                captured_temperature: Mutex::new(None),
            }),
            receiver,
        )
    }

    fn new_with_temperature_capture(
        response: impl Into<String>,
    ) -> (
        Arc<Self>,
        std_mpsc::Receiver<Vec<String>>,
        std_mpsc::Receiver<Option<f64>>,
    ) {
        let (message_sender, message_receiver) = std_mpsc::channel();
        let (temperature_sender, temperature_receiver) = std_mpsc::channel();
        (
            Arc::new(Self {
                response: response.into(),
                captured: Mutex::new(Some(message_sender)),
                captured_temperature: Mutex::new(Some(temperature_sender)),
            }),
            message_receiver,
            temperature_receiver,
        )
    }
}

#[async_trait]
impl Provider for CapturingProvider {
    fn capabilities(&self) -> ProviderCapabilities {
        ProviderCapabilities {
            streaming: true,
            reasoning: false,
            max_context_tokens: None,
        }
    }

    async fn generate(
        &self,
        request: GenerationRequest,
        _credential: Option<&str>,
        sink: ProviderEventSender,
        _cancelled: watch::Receiver<bool>,
    ) -> CoreResult<GenerationUsage> {
        if let Some(sender) = self
            .captured_temperature
            .lock()
            .expect("temperature capture lock")
            .take()
        {
            let _ = sender.send(request.temperature);
        }
        if let Some(sender) = self.captured.lock().expect("capture lock").take() {
            let _ = sender.send(
                request
                    .messages
                    .into_iter()
                    .map(|message| message.content)
                    .collect(),
            );
        }
        sink.send(ProviderEvent::TextDelta(self.response.clone()))
            .await
            .map_err(|_| CoreError::internal("chat event receiver closed"))?;
        Ok(GenerationUsage::default())
    }
}

#[async_trait]
impl Provider for OpaqueContinuityProvider {
    fn capabilities(&self) -> ProviderCapabilities {
        ProviderCapabilities {
            streaming: true,
            reasoning: true,
            max_context_tokens: None,
        }
    }

    fn snapshot_request(&self, request: &GenerationRequest) -> CoreResult<serde_json::Value> {
        if request.preserve_opaque_reasoning_state || !request.opaque_reasoning_context.is_empty() {
            return Err(CoreError::new(
                CoreErrorCode::UnsupportedContent,
                "opaque reasoning continuity cannot be stored in a plaintext request snapshot",
                false,
            ));
        }
        serde_json::to_value(request).map_err(|error| {
            CoreError::internal(format!(
                "cannot encode synthetic opaque-continuity request snapshot: {error}"
            ))
        })
    }

    async fn generate(
        &self,
        request: GenerationRequest,
        _credential: Option<&str>,
        sink: ProviderEventSender,
        _cancelled: watch::Receiver<bool>,
    ) -> CoreResult<GenerationUsage> {
        if let Some(sender) = self
            .captured_request
            .lock()
            .expect("opaque request capture lock")
            .take()
        {
            let _ = sender.send((
                request.preserve_opaque_reasoning_state,
                request.opaque_reasoning_context,
                request.provider_provenance,
            ));
        }
        if let Some(state) = self.emitted_state.clone() {
            sink.send(ProviderEvent::OpaqueReasoningState(state))
                .await
                .map_err(|_| CoreError::internal("chat event receiver closed"))?;
        }
        sink.send(ProviderEvent::TextDelta(self.response.clone()))
            .await
            .map_err(|_| CoreError::internal("chat event receiver closed"))?;
        Ok(GenerationUsage::default())
    }
}

#[async_trait]
impl Provider for OverflowUsageProvider {
    fn capabilities(&self) -> ProviderCapabilities {
        ProviderCapabilities {
            streaming: true,
            reasoning: false,
            max_context_tokens: None,
        }
    }

    async fn generate(
        &self,
        _request: GenerationRequest,
        _credential: Option<&str>,
        sink: ProviderEventSender,
        _cancelled: watch::Receiver<bool>,
    ) -> CoreResult<GenerationUsage> {
        sink.send(ProviderEvent::TextDelta(
            "response before invalid usage".to_owned(),
        ))
        .await
        .map_err(|_| CoreError::internal("provider event receiver closed"))?;
        Ok(GenerationUsage {
            input_tokens: Some(i64::MAX as u64 + 1),
            output_tokens: Some(1),
            ..GenerationUsage::default()
        })
    }
}

#[async_trait]
impl Provider for SnapshotFailingProvider {
    fn capabilities(&self) -> ProviderCapabilities {
        ProviderCapabilities {
            streaming: true,
            reasoning: false,
            max_context_tokens: None,
        }
    }

    fn snapshot_request(&self, _request: &GenerationRequest) -> CoreResult<serde_json::Value> {
        Err(CoreError::internal("injected provider snapshot failure"))
    }

    async fn generate(
        &self,
        _request: GenerationRequest,
        _credential: Option<&str>,
        _sink: ProviderEventSender,
        _cancelled: watch::Receiver<bool>,
    ) -> CoreResult<GenerationUsage> {
        panic!("generation must not start after a request snapshot failure")
    }
}

impl StallingProvider {
    fn new(partial: impl Into<String>) -> (Arc<Self>, std_mpsc::Receiver<()>) {
        let (started_sender, started_receiver) = std_mpsc::channel();
        (
            Arc::new(Self {
                partial: partial.into(),
                started: Mutex::new(Some(started_sender)),
            }),
            started_receiver,
        )
    }
}

#[async_trait]
impl Provider for StallingProvider {
    fn capabilities(&self) -> ProviderCapabilities {
        ProviderCapabilities {
            streaming: true,
            reasoning: false,
            max_context_tokens: None,
        }
    }

    async fn generate(
        &self,
        _request: GenerationRequest,
        _credential: Option<&str>,
        sink: ProviderEventSender,
        _cancelled: watch::Receiver<bool>,
    ) -> CoreResult<GenerationUsage> {
        sink.send(ProviderEvent::TextDelta(self.partial.clone()))
            .await
            .map_err(|_| CoreError::internal("provider event receiver closed"))?;
        if let Some(sender) = self.started.lock().expect("started lock").take() {
            let _ = sender.send(());
        }
        std::future::pending().await
    }
}

impl CatchupSnapshotProvider {
    fn new() -> (
        Arc<Self>,
        std_mpsc::Receiver<()>,
        tokio::sync::oneshot::Sender<()>,
    ) {
        let (started_sender, started_receiver) = std_mpsc::channel();
        let (release_sender, release_receiver) = tokio::sync::oneshot::channel();
        (
            Arc::new(Self {
                started: Mutex::new(Some(started_sender)),
                release: Mutex::new(Some(release_receiver)),
            }),
            started_receiver,
            release_sender,
        )
    }
}

#[async_trait]
impl Provider for CatchupSnapshotProvider {
    fn capabilities(&self) -> ProviderCapabilities {
        ProviderCapabilities {
            streaming: true,
            reasoning: true,
            max_context_tokens: None,
        }
    }

    async fn generate(
        &self,
        _request: GenerationRequest,
        _credential: Option<&str>,
        sink: ProviderEventSender,
        _cancelled: watch::Receiver<bool>,
    ) -> CoreResult<GenerationUsage> {
        sink.send(ProviderEvent::ReasoningDelta("reasoning-prefix".to_owned()))
            .await
            .map_err(|_| CoreError::internal("provider event receiver closed"))?;
        sink.send(ProviderEvent::TextDelta("text-prefix".to_owned()))
            .await
            .map_err(|_| CoreError::internal("provider event receiver closed"))?;
        if let Some(sender) = self.started.lock().expect("catch-up started lock").take() {
            let _ = sender.send(());
        }
        let release = self
            .release
            .lock()
            .expect("catch-up release lock")
            .take()
            .expect("catch-up release receiver");
        release
            .await
            .map_err(|_| CoreError::internal("catch-up release sender dropped"))?;
        sink.send(ProviderEvent::ReasoningDelta(
            "+reasoning-suffix".to_owned(),
        ))
        .await
        .map_err(|_| CoreError::internal("provider event receiver closed"))?;
        sink.send(ProviderEvent::TextDelta("+text-suffix".to_owned()))
            .await
            .map_err(|_| CoreError::internal("provider event receiver closed"))?;
        Ok(GenerationUsage::default())
    }
}

#[async_trait]
impl Provider for LeaseBarrierProvider {
    fn capabilities(&self) -> ProviderCapabilities {
        ProviderCapabilities {
            streaming: true,
            reasoning: false,
            max_context_tokens: None,
        }
    }

    async fn generate(
        &self,
        _request: GenerationRequest,
        _credential: Option<&str>,
        sink: ProviderEventSender,
        _cancelled: watch::Receiver<bool>,
    ) -> CoreResult<GenerationUsage> {
        if let Some(entered) = self.entered.lock().expect("lease entered lock").take() {
            let _ = entered.send(());
        }
        let release = self
            .release
            .lock()
            .expect("lease release lock")
            .take()
            .expect("lease release receiver");
        release
            .await
            .map_err(|_| CoreError::internal("lease test release dropped"))?;
        sink.send(ProviderEvent::TextDelta("completed".to_owned()))
            .await
            .map_err(|_| CoreError::internal("lease test event receiver closed"))?;
        Ok(GenerationUsage::default())
    }
}
