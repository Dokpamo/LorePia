use lorepia_domain::{ConversationId, GenerationId, GenerationRequest};
use lorepia_providers::{OpenAiCompatibleProvider, Provider};
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicUsize, Ordering},
};
use std::time::Duration;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpListener,
    sync::{mpsc, watch},
};

fn request() -> GenerationRequest {
    GenerationRequest {
        generation_id: GenerationId::new(),
        conversation_id: ConversationId::new(),
        model: "fixture".to_owned(),
        messages: Vec::new(),
        resolved_prompt_plan: None,
        provider_execution_plan_hash: None,
        temperature: Some(1.0),
        max_output_tokens: None,
        provider_provenance: None,
        preserve_opaque_reasoning_state: false,
        opaque_reasoning_context: Vec::new(),
    }
}

fn spawn_fixture_server(
    listener: TcpListener,
    connection_count: Arc<AtomicUsize>,
    request_count: Arc<AtomicUsize>,
    authorization: Arc<Mutex<Vec<Vec<String>>>>,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        loop {
            let (mut socket, _) = listener.accept().await.unwrap();
            connection_count.fetch_add(1, Ordering::SeqCst);
            let request_count = Arc::clone(&request_count);
            let authorization = Arc::clone(&authorization);
            tokio::spawn(async move {
                loop {
                    let mut headers = Vec::new();
                    while !headers.ends_with(b"\r\n\r\n") {
                        let Ok(byte) = socket.read_u8().await else {
                            return;
                        };
                        headers.push(byte);
                        if headers.len() > 65536 {
                            return;
                        }
                    }
                    let headers = String::from_utf8(headers).unwrap();
                    let content_length = headers
                        .lines()
                        .find_map(|line| {
                            let (name, value) = line.split_once(':')?;
                            name.eq_ignore_ascii_case("content-length")
                                .then(|| value.trim().parse::<usize>().unwrap())
                        })
                        .unwrap_or(0);
                    let mut body = vec![0; content_length];
                    if socket.read_exact(&mut body).await.is_err() {
                        return;
                    }
                    let header = headers
                        .lines()
                        .filter_map(|line| {
                            let (name, value) = line.split_once(':')?;
                            name.eq_ignore_ascii_case("authorization")
                                .then(|| value.trim().to_owned())
                        })
                        .collect::<Vec<_>>();
                    authorization.lock().unwrap().push(header);
                    request_count.fetch_add(1, Ordering::SeqCst);
                    let body = "data: {\"choices\":[{\"index\":0,\"delta\":{\"content\":\"ok\"},\"finish_reason\":null}]}\n\ndata: {\"choices\":[{\"index\":0,\"delta\":{},\"finish_reason\":\"stop\"}]}\n\ndata: [DONE]\n\n";
                    let response = format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\nConnection: keep-alive\r\n\r\n{}",
                        body.len(),
                        body
                    );
                    if socket.write_all(response.as_bytes()).await.is_err() {
                        return;
                    }
                }
            });
        }
    })
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn separate_provider_instances_reuse_only_the_approved_transport()
-> Result<(), Box<dyn std::error::Error>> {
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let address = listener.local_addr()?;
    let connection_count = Arc::new(AtomicUsize::new(0));
    let request_count = Arc::new(AtomicUsize::new(0));
    let authorization = Arc::new(Mutex::new(Vec::new()));
    let server = spawn_fixture_server(
        listener,
        Arc::clone(&connection_count),
        Arc::clone(&request_count),
        Arc::clone(&authorization),
    );
    let base = format!("http://{address}/v1");
    let count = 20;
    let mut expected_authorization = Vec::new();
    for index in 0..count {
        // Synthetic values only: transport reuse must not retain request headers.
        let credential = match index % 3 {
            0 => Some("synthetic-pool-credential-A"),
            1 => Some("synthetic-pool-credential-B"),
            _ => None,
        };
        expected_authorization.push(
            credential
                .map(|value| format!("Bearer {value}"))
                .into_iter()
                .collect::<Vec<_>>(),
        );
        let (sink, mut receiver) = mpsc::channel(64);
        let (_cancel, cancelled) = watch::channel(false);
        let provider = OpenAiCompatibleProvider::new(&base, Duration::from_secs(5))?;
        provider
            .generate(request(), credential, sink, cancelled)
            .await?;
        let mut events = 0;
        while receiver.try_recv().is_ok() {
            events += 1;
        }
        assert!(events > 0);
    }
    let actual_connections = connection_count.load(Ordering::SeqCst);
    let actual_requests = request_count.load(Ordering::SeqCst);
    assert_eq!(
        *authorization.lock().unwrap(),
        expected_authorization,
        "a reused connection sends only each request's current credential, including absence"
    );
    // This control ONLY proves the loopback server supports connection reuse;
    // it is not a proposed provider implementation or comparable parser timing.
    let client = reqwest::Client::builder().no_proxy().build()?;
    for _ in 0..count {
        client
            .post(format!("{base}/chat/completions"))
            .json(&serde_json::json!({"fixture": true}))
            .send()
            .await?
            .bytes()
            .await?;
    }
    let control_connections = connection_count.load(Ordering::SeqCst) - actual_connections;
    let control_requests = request_count.load(Ordering::SeqCst) - actual_requests;
    // HTTP/1 idle publication runs asynchronously; a following checkout may
    // legitimately race a speculative connect even after the body is drained.
    // Exact transport construction/key reuse belongs to the pool unit tests.
    assert!(
        actual_connections > 0 && actual_connections < count,
        "separate adapters for an unchanged approved target reuse keep-alive"
    );
    assert_eq!(actual_requests, count);
    assert!(control_connections > 0 && control_connections < count);
    assert_eq!(control_requests, count);
    server.abort();
    Ok(())
}

#[test]
fn pooled_transport_reconnects_after_its_original_runtime_shuts_down()
-> Result<(), Box<dyn std::error::Error>> {
    let connection_count = Arc::new(AtomicUsize::new(0));
    let request_count = Arc::new(AtomicUsize::new(0));
    let authorization = Arc::new(Mutex::new(Vec::new()));
    let connection_observer = Arc::clone(&connection_count);
    let request_observer = Arc::clone(&request_count);
    let (ready, address) = std::sync::mpsc::channel();
    let (stop, stopped) = tokio::sync::oneshot::channel();
    let host = std::thread::spawn(move || {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
            .block_on(async move {
                let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
                ready.send(listener.local_addr().unwrap()).unwrap();
                let task = spawn_fixture_server(
                    listener,
                    connection_observer,
                    request_observer,
                    authorization,
                );
                let _ = stopped.await;
                task.abort();
            });
    });
    let base = format!(
        "http://{}/v1",
        address.recv_timeout(Duration::from_secs(5))?
    );
    for expected in 1..=2 {
        // The global pool survives both independent client reactor lifetimes.
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()?;
        runtime.block_on(async {
            let provider = OpenAiCompatibleProvider::new(&base, Duration::from_secs(5))?;
            let (sink, mut receiver) = mpsc::channel(64);
            let (_cancel, cancelled) = watch::channel(false);
            provider.generate(request(), None, sink, cancelled).await?;
            assert!(receiver.try_recv().is_ok());
            Ok::<_, lorepia_domain::CoreError>(())
        })?;
        drop(runtime);
        assert_eq!(request_count.load(Ordering::SeqCst), expected);
        assert_eq!(
            connection_count.load(Ordering::SeqCst),
            expected,
            "a new reactor must reconnect instead of retaining the retired socket"
        );
    }
    let _ = stop.send(());
    host.join().unwrap();
    Ok(())
}
