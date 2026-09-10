use super::*;

#[tokio::test]
async fn enforces_a_cumulative_non_progress_event_limit() {
    let exact = ProtocolProvider {
        protocol_events: vec![
            ProviderEvent::ReasoningDelta(String::new());
            MAX_GENERATED_PROVIDER_EVENTS
        ],
    };
    let (events, _receiver) = mpsc::channel(MAX_GENERATED_PROVIDER_EVENTS + 2);
    let (_cancel, cancelled) = watch::channel(false);
    run_generation(&exact, request(), None, events, cancelled)
        .await
        .expect("exact provider-event count");

    let overflow = ProtocolProvider {
        protocol_events: vec![
            ProviderEvent::ReasoningDelta(String::new());
            MAX_GENERATED_PROVIDER_EVENTS + 1
        ],
    };
    let (events, _receiver) = mpsc::channel(MAX_GENERATED_PROVIDER_EVENTS + 2);
    let (_cancel, cancelled) = watch::channel(false);
    let failure = run_generation(&overflow, request(), None, events, cancelled)
        .await
        .expect_err("provider-event overflow must fail");
    assert_eq!(failure.error.code, CoreErrorCode::ProviderUnavailable);
    assert_eq!(
        failure.error.message,
        "provider exceeded the 8192 non-progress event limit"
    );
    assert!(!failure.error.recoverable);
    assert_eq!(
        failure.last_sequence,
        u64::try_from(MAX_GENERATED_PROVIDER_EVENTS + 1).expect("bounded sequence")
    );
}

async fn generate_events(
    protocol_events: Vec<ProviderEvent>,
) -> (Result<GenerationOutcome, GenerationFailure>, Vec<ChatEvent>) {
    let provider = ProtocolProvider { protocol_events };
    let (events, mut receiver) = mpsc::channel(128);
    let drain = tokio::spawn(async move {
        let mut events = Vec::new();
        while let Some(event) = receiver.recv().await {
            events.push(event);
        }
        events
    });
    let (_cancel, cancelled) = watch::channel(false);
    let result = run_generation(&provider, request(), None, events, cancelled).await;
    (result, drain.await.expect("event drain"))
}

#[tokio::test]
async fn identical_text_and_reasoning_survive_single_scalar_fragmentation() {
    for reasoning in [false, true] {
        let content = "😀".repeat(MAX_GENERATED_OUTPUT_CHARS);
        let event = |delta: String| {
            if reasoning {
                ProviderEvent::ReasoningDelta(delta)
            } else {
                ProviderEvent::TextDelta(delta)
            }
        };
        let (whole, whole_events) = generate_events(vec![event(content.clone())]).await;
        let (split, split_events) =
            generate_events(content.chars().map(|c| event(c.to_string())).collect()).await;
        assert_eq!(
            whole.expect("whole output").text,
            split.expect("split output").text
        );
        let text = |events: Vec<ChatEvent>| {
            events
                .into_iter()
                .filter_map(|e| match e.kind {
                    ChatEventKind::TextDelta(s) | ChatEventKind::ReasoningDelta(s) => Some(s),
                    _ => None,
                })
                .collect::<String>()
        };
        assert_eq!(text(whole_events), content);
        assert_eq!(text(split_events), content);
    }
}

#[tokio::test]
async fn interleaved_progress_cannot_reset_the_empty_event_flood_budget() {
    let mut protocol = Vec::new();
    for _ in 0..=MAX_GENERATED_PROVIDER_EVENTS {
        protocol.push(ProviderEvent::TextDelta("x".to_owned()));
        protocol.push(ProviderEvent::ReasoningDelta(String::new()));
    }
    let failure = generate_events(protocol)
        .await
        .0
        .expect_err("empty event flood");
    assert_eq!(
        failure.partial_text.len(),
        MAX_GENERATED_PROVIDER_EVENTS + 1
    );
    assert_eq!(
        failure.error.message,
        "provider exceeded the 8192 non-progress event limit"
    );
}

#[tokio::test]
async fn bounded_tool_arguments_survive_single_scalar_fragmentation() {
    let fragments = vec!["x".to_owned(); MAX_GENERATED_PROVIDER_EVENTS + 1];
    let (result, events) = generate_events(tool_protocol_with_fragments(fragments)).await;
    result.expect("valid fragmented arguments");
    let arguments = events
        .into_iter()
        .filter_map(|event| match event.kind {
            ChatEventKind::ToolCallArgumentsDelta { delta, .. } => Some(delta.as_str().to_owned()),
            _ => None,
        })
        .collect::<String>();
    assert_eq!(arguments, "x".repeat(MAX_GENERATED_PROVIDER_EVENTS + 1));
}
