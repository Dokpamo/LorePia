use std::{
    io::{Read, Write},
    net::{TcpListener, TcpStream},
    sync::mpsc,
    thread,
    time::{Duration, Instant},
};

use lorepia_core::{
    Core, CoreConfig, GenerationOperationContext, GenerationStatus, GenerationTarget,
    MessageStatus, ModelSyncState, ProviderConnectionId, ProviderProfile,
};
use lorepia_storage::Storage;
use tempfile::NamedTempFile;
use tempfile::tempdir;

fn read_request(stream: &mut TcpStream) -> Vec<u8> {
    stream
        .set_read_timeout(Some(Duration::from_secs(2)))
        .expect("set request timeout");
    let mut request = Vec::new();
    let mut buffer = [0_u8; 4_096];
    loop {
        let read = stream.read(&mut buffer).expect("read request");
        if read == 0 {
            break;
        }
        request.extend_from_slice(&buffer[..read]);
        let Some(header_end) = request.windows(4).position(|window| window == b"\r\n\r\n") else {
            continue;
        };
        let headers = String::from_utf8_lossy(&request[..header_end]);
        let content_length = headers
            .lines()
            .find_map(|line| {
                let (name, value) = line.split_once(':')?;
                name.eq_ignore_ascii_case("content-length")
                    .then(|| value.trim().parse::<usize>().ok())
                    .flatten()
            })
            .unwrap_or_default();
        if request.len() >= header_end + 4 + content_length {
            break;
        }
    }
    request
}

fn write_chunk(stream: &mut TcpStream, payload: &str) {
    write!(stream, "{:X}\r\n{payload}\r\n", payload.len()).expect("write response chunk");
    stream.flush().expect("flush response chunk");
}

fn spawn_concurrent_provider() -> (
    String,
    mpsc::Receiver<()>,
    mpsc::Sender<()>,
    thread::JoinHandle<()>,
) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind concurrent provider");
    let address = listener.local_addr().expect("concurrent provider address");
    let (generation_started_sender, generation_started_receiver) = mpsc::channel();
    let (release_generation_sender, release_generation_receiver) = mpsc::channel();
    let worker = thread::spawn(move || {
        let (mut generation_stream, _) = listener.accept().expect("accept generation request");
        let generation_request = read_request(&mut generation_stream);
        let generation_request =
            String::from_utf8(generation_request).expect("generation request is UTF-8");
        assert!(
            generation_request.starts_with("POST /v1/chat/completions HTTP/1.1\r\n"),
            "unexpected first request: {generation_request}"
        );
        write!(
            generation_stream,
            "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n"
        )
        .expect("write generation response headers");
        write_chunk(
            &mut generation_stream,
            "data: {\"choices\":[{\"index\":0,\"delta\":{\"content\":\"before refresh\"}}]}\n\n",
        );
        generation_started_sender
            .send(())
            .expect("signal generation start");

        let generation_worker = thread::spawn(move || {
            release_generation_receiver
                .recv_timeout(Duration::from_secs(5))
                .expect("release in-flight generation");
            write_chunk(
                &mut generation_stream,
                "data: {\"choices\":[{\"index\":0,\"delta\":{\"content\":\" after refresh\"}}]}\n\n",
            );
            write_chunk(
                &mut generation_stream,
                "data: {\"choices\":[{\"index\":0,\"delta\":{},\"finish_reason\":\"stop\"}]}\n\n",
            );
            write_chunk(&mut generation_stream, "data: [DONE]\n\n");
            generation_stream
                .write_all(b"0\r\n\r\n")
                .expect("finish chunked generation response");
        });

        let (mut models_stream, _) = listener.accept().expect("accept model-list request");
        let models_request = read_request(&mut models_stream);
        let models_request = String::from_utf8(models_request).expect("models request is UTF-8");
        assert!(
            models_request.starts_with("GET /v1/models HTTP/1.1\r\n"),
            "unexpected second request: {models_request}"
        );
        let body = r#"{"data":[{"id":"stable-model"},{"id":"new-model"}]}"#;
        write!(
            models_stream,
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        )
        .expect("write model-list response");
        generation_worker.join().expect("join generation worker");
    });
    (
        format!("http://{address}/v1"),
        generation_started_receiver,
        release_generation_sender,
        worker,
    )
}

fn wait_for_model_review(
    core: &Core,
    job_id: &lorepia_core::ModelSyncJobId,
) -> lorepia_core::ModelSyncReview {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let job = core.get_provider_model_sync(job_id).expect("model sync");
        match job.state {
            ModelSyncState::DiffReadyAwaitingReview => {
                return job.review.expect("review-ready job has a review");
            }
            ModelSyncState::Failed | ModelSyncState::Cancelled | ModelSyncState::Interrupted => {
                panic!("model sync became terminal before review: {job:?}");
            }
            ModelSyncState::Created
            | ModelSyncState::Fetching
            | ModelSyncState::Committing
            | ModelSyncState::Completed => {}
        }
        assert!(
            Instant::now() < deadline,
            "model sync did not become review-ready: {job:?}"
        );
        thread::sleep(Duration::from_millis(10));
    }
}

fn wait_for_generation(core: &Core, conversation_id: &lorepia_core::ConversationId) {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let messages = core.list_messages(conversation_id).expect("messages");
        if messages.len() == 2 && messages[1].status == MessageStatus::Complete {
            assert_eq!(messages[1].content, "before refresh after refresh");
            return;
        }
        assert!(
            Instant::now() < deadline,
            "generation did not complete after model refresh: {messages:?}"
        );
        thread::sleep(Duration::from_millis(10));
    }
}

#[test]
#[allow(
    clippy::too_many_lines,
    reason = "the concurrency invariant is clearest as one chronological integration scenario"
)]
fn generation_in_flight_keeps_its_route_and_preset_while_catalog_refreshes() {
    let root = tempdir().expect("temporary data root");
    let core = Core::open(CoreConfig::new(root.path())).expect("open core");
    let (base_url, generation_started, release_generation, provider_worker) =
        spawn_concurrent_provider();
    let profile = core
        .upsert_provider_profile(ProviderProfile {
            id: "concurrent-refresh".to_owned(),
            display_name: "Concurrent refresh".to_owned(),
            base_url,
            model: "stable-model".to_owned(),
            timeout_seconds: 5,
        })
        .expect("seed provider graph");
    let connection_id = ProviderConnectionId::from(profile.id);
    let stable_route = core
        .list_model_routes(&connection_id)
        .expect("seeded routes")
        .into_iter()
        .find(|route| route.model_id == "stable-model")
        .expect("stable route");
    let stable_preset = core
        .list_generation_presets(&stable_route.id)
        .expect("stable route presets")
        .into_iter()
        .next()
        .expect("stable route preset");
    let target = GenerationTarget {
        model_route_id: stable_route.id.clone(),
        generation_preset_id: stable_preset.id.clone(),
    };

    let mut character_source = NamedTempFile::new().expect("temporary character source");
    write!(
        character_source,
        r#"{{"spec":"chara_card_v3","data":{{"name":"Refresh","description":"Concurrency fixture"}}}}"#
    )
    .expect("write character source");
    let inspection = core
        .inspect_import(character_source.path())
        .expect("inspect character");
    let character = core
        .commit_import(&inspection.id)
        .expect("commit character");
    let conversation = core
        .open_conversation(&character.id)
        .expect("open conversation");
    let mut events = core.subscribe_events();
    let generation_id = core
        .send_message_with_target(
            &conversation.id,
            "start",
            GenerationOperationContext::New {
                operation_nonce: "model-refresh-concurrency-send-v1",
            },
            &target,
            Some("concurrency-test-credential".to_owned()),
        )
        .expect("start generation");
    if generation_started
        .recv_timeout(Duration::from_secs(2))
        .is_err()
    {
        let observed_events = std::iter::from_fn(|| events.try_recv().ok()).collect::<Vec<_>>();
        panic!(
            "generation did not reach provider; messages={:?}; events={observed_events:?}",
            core.list_messages(&conversation.id)
                .expect("diagnostic messages")
        );
    }

    let pending = core
        .list_messages(&conversation.id)
        .expect("pending messages");
    assert_eq!(pending.len(), 2);
    assert_eq!(pending[1].status, MessageStatus::Pending);

    let sync_id = core
        .start_provider_model_sync(
            &connection_id,
            Some("concurrency-test-credential".to_owned()),
        )
        .expect("start model sync");
    let review = wait_for_model_review(&core, &sync_id);
    core.approve_provider_model_sync(&sync_id, &review.sha256)
        .expect("approve model sync");

    let refreshed_routes = core
        .list_model_routes(&connection_id)
        .expect("refreshed routes");
    assert!(
        refreshed_routes
            .iter()
            .any(|route| { route.id == stable_route.id && route.model_id == "stable-model" })
    );
    assert!(
        refreshed_routes
            .iter()
            .any(|route| route.model_id == "new-model")
    );
    assert_eq!(
        core.list_generation_presets(&stable_route.id)
            .expect("preserved stable presets"),
        vec![stable_preset.clone()]
    );
    assert_eq!(
        core.list_messages(&conversation.id)
            .expect("still-pending messages")[1]
            .status,
        MessageStatus::Pending
    );

    release_generation
        .send(())
        .expect("release in-flight generation");
    wait_for_generation(&core, &conversation.id);
    provider_worker.join().expect("join concurrent provider");
    drop(core);

    let storage = Storage::open(root.path()).expect("reopen storage");
    let generation = storage
        .get_generation(&generation_id)
        .expect("persisted generation");
    assert_eq!(generation.status, GenerationStatus::Complete);
    assert_eq!(generation.model_route_id, Some(stable_route.id));
    assert_eq!(generation.generation_preset_id, Some(stable_preset.id));
}
