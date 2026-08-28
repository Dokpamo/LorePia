mod support;

use std::{
    fs,
    io::{Read, Write},
    net::{TcpListener, TcpStream},
    path::{Path, PathBuf},
    sync::mpsc,
    thread,
    time::{Duration, Instant},
};

use lorepia_core::{
    AppSettings, ChatEventKind, Core, CoreConfig, CoreErrorCode, GenerationOperationContext,
    GenerationStatus, MessageStatus, ProviderProfile,
};
use lorepia_storage::Storage;
use support::is_live_owner_lock_sharing_violation;
use tempfile::{NamedTempFile, tempdir};

const REFLECTED_CREDENTIAL: &str = "sk-core-reflection-canary-7a91";

fn active_database_path(root: &Path) -> PathBuf {
    let cutover = root.join("db/schema-cutover");
    let (_, relative) = fs::read_dir(cutover)
        .expect("read committed database generations")
        .filter_map(Result::ok)
        .filter(|entry| entry.path().join("generation-committed.json").is_file())
        .map(|entry| {
            let manifest = serde_json::from_slice::<serde_json::Value>(
                &fs::read(entry.path().join("generation-manifest.json"))
                    .expect("read generation manifest"),
            )
            .expect("parse generation manifest");
            let sequence = manifest["activation_sequence"]
                .as_u64()
                .expect("generation activation sequence");
            let relative = manifest["active_database_relative_path"]
                .as_str()
                .expect("active database relative path")
                .to_owned();
            (sequence, relative)
        })
        .max_by_key(|(sequence, _)| *sequence)
        .expect("at least one committed database generation");
    root.join(relative)
}

fn imported_core() -> (tempfile::TempDir, Core, String) {
    let root = tempdir().expect("temporary data root");
    let core = Core::open(CoreConfig::new(root.path())).expect("open core");
    let mut source = NamedTempFile::new().expect("temporary source");
    write!(
        source,
        r#"{{"spec":"chara_card_v3","data":{{"name":"세구","description":"테스트"}}}}"#
    )
    .expect("write source");
    let review = core.inspect_import(source.path()).expect("inspect");
    let character = core.commit_import(&review.id).expect("commit");
    (root, core, character.id)
}

fn open_storage_after_core_drop(root: &Path) -> Storage {
    let deadline = Instant::now() + Duration::from_secs(2);
    loop {
        match Storage::open(root) {
            Ok(storage) => return storage,
            Err(error)
                if error.code == CoreErrorCode::StorageUnavailable && Instant::now() < deadline =>
            {
                thread::sleep(Duration::from_millis(10));
            }
            Err(error) => panic!("open storage after Core drop: {error:?}"),
        }
    }
}

fn assert_reflected_credential_reached_provider(captured_request: &mpsc::Receiver<Vec<u8>>) {
    let request = captured_request
        .recv_timeout(Duration::from_secs(2))
        .expect("captured reflecting request");
    assert!(
        request
            .windows(REFLECTED_CREDENTIAL.len())
            .any(|window| window == REFLECTED_CREDENTIAL.as_bytes()),
        "the fixture must prove the credential reached only the approved provider request"
    );
}

fn read_request(stream: &mut TcpStream) -> Vec<u8> {
    stream
        .set_read_timeout(Some(Duration::from_secs(2)))
        .expect("request timeout");
    let mut request = Vec::new();
    let mut buffer = [0_u8; 4096];
    loop {
        let read = stream.read(&mut buffer).expect("read request");
        if read == 0 {
            break;
        }
        request.extend_from_slice(&buffer[..read]);
        let Some(header_end) = request.windows(4).position(|value| value == b"\r\n\r\n") else {
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
            .unwrap_or(0);
        if request.len() >= header_end + 4 + content_length {
            break;
        }
    }
    request
}

fn spawn_completed_provider() -> (String, mpsc::Receiver<Vec<u8>>) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind provider");
    let address = listener.local_addr().expect("provider address");
    let (request_sender, request_receiver) = mpsc::channel();
    thread::spawn(move || {
        let (mut stream, _) = listener.accept().expect("accept provider request");
        let request = read_request(&mut stream);
        request_sender.send(request).expect("capture request");
        let body = concat!(
            "data: {\"choices\":[{\"index\":0,\"delta\":{\"content\":\"안녕\"}}]}\n\n",
            "data: {\"choices\":[{\"index\":0,\"delta\":{},\"finish_reason\":\"stop\"}]}\n\n",
            "data: {\"choices\":[],\"usage\":{\"prompt_tokens\":7,\"completion_tokens\":2}}\n\n",
            "data: [DONE]\n\n"
        );
        write!(
            stream,
            "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
            body.len(),
            body
        )
        .expect("write provider response");
    });
    (format!("http://{address}/v1"), request_receiver)
}

fn spawn_stalling_provider() -> (String, mpsc::Receiver<()>, mpsc::Sender<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind provider");
    let address = listener.local_addr().expect("provider address");
    let (ready_sender, ready_receiver) = mpsc::channel();
    let (stop_sender, stop_receiver) = mpsc::channel();
    thread::spawn(move || {
        let (mut stream, _) = listener.accept().expect("accept provider request");
        let _request = read_request(&mut stream);
        let event = "data: {\"choices\":[{\"index\":0,\"delta\":{\"content\":\"부분\"}}]}\n\n";
        write!(
            stream,
            "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n{:X}\r\n{}\r\n",
            event.len(),
            event
        )
        .expect("write provider chunk");
        stream.flush().expect("flush provider chunk");
        ready_sender.send(()).expect("provider ready");
        let _ = stop_receiver.recv_timeout(Duration::from_secs(3));
        let _ = stream.write_all(b"0\r\n\r\n");
    });
    (format!("http://{address}/v1"), ready_receiver, stop_sender)
}

fn spawn_reflecting_provider() -> (String, mpsc::Receiver<Vec<u8>>) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind reflecting provider");
    let address = listener.local_addr().expect("reflecting provider address");
    let (request_sender, request_receiver) = mpsc::channel();
    thread::spawn(move || {
        let (mut stream, _) = listener
            .accept()
            .expect("accept reflecting provider request");
        let request = read_request(&mut stream);
        request_sender
            .send(request)
            .expect("capture reflecting request");
        let body = concat!(
            "data: {\"choices\":[{\"index\":0,\"delta\":{\"content\":\"safe prefix \"}}]}\n\n",
            "data: {\"choices\":[{\"index\":0,\"delta\":{\"content\":\"sk-core-reflection-\"}}]}\n\n",
            "data: {\"choices\":[{\"index\":0,\"delta\":{\"content\":\"canary-7a91\"}}]}\n\n",
            "data: {\"choices\":[{\"index\":0,\"delta\":{},\"finish_reason\":\"stop\"}]}\n\n",
            "data: [DONE]\n\n"
        );
        write!(
            stream,
            "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
            body.len(),
            body
        )
        .expect("write reflecting provider response");
    });
    (format!("http://{address}/v1"), request_receiver)
}

fn assert_tree_does_not_contain(root: &Path, needle: &[u8]) {
    let mut pending = vec![root.to_path_buf()];
    while let Some(path) = pending.pop() {
        let metadata = fs::symlink_metadata(&path).expect("inspect data-root entry");
        if metadata.is_dir() {
            pending.extend(
                fs::read_dir(&path)
                    .expect("read data-root directory")
                    .map(|entry| entry.expect("read data-root entry").path()),
            );
            continue;
        }
        if metadata.is_file() {
            let bytes = match fs::read(&path) {
                Ok(bytes) => bytes,
                Err(error) if is_live_owner_lock_sharing_violation(&path, &error) => continue,
                Err(error) => panic!("read data-root file {}: {error}", path.display()),
            };
            assert!(
                !bytes.windows(needle.len()).any(|window| window == needle),
                "protected credential was persisted in {}",
                path.display()
            );
        }
    }
}

fn wait_for_terminal_message(core: &Core, conversation_id: &lorepia_core::ConversationId) {
    let deadline = Instant::now() + Duration::from_secs(3);
    loop {
        let messages = core.list_messages(conversation_id).expect("messages");
        if messages.len() == 2 && messages[1].status != MessageStatus::Pending {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "generation did not finish; stored messages: {messages:?}"
        );
        thread::sleep(Duration::from_millis(10));
    }
}

fn collect_generation_events_until_terminal(
    event_receiver: &mut tokio::sync::broadcast::Receiver<lorepia_core::ChatEvent>,
    generation_id: &lorepia_core::GenerationId,
) -> Vec<lorepia_core::ChatEvent> {
    let deadline = Instant::now() + Duration::from_secs(3);
    let mut received = Vec::new();
    loop {
        let event = match event_receiver.try_recv() {
            Ok(event) => event,
            Err(tokio::sync::broadcast::error::TryRecvError::Empty) => {
                assert!(
                    Instant::now() < deadline,
                    "terminal chat event did not arrive"
                );
                thread::sleep(Duration::from_millis(5));
                continue;
            }
            Err(error) => panic!("chat event stream failed before terminal event: {error}"),
        };
        if event.generation_id != *generation_id {
            continue;
        }
        let terminal = matches!(
            &event.kind,
            ChatEventKind::GenerationCancelled
                | ChatEventKind::GenerationFailed { .. }
                | ChatEventKind::GenerationFinished
        );
        received.push(event);
        if terminal {
            return received;
        }
    }
}

#[test]
fn streams_commits_and_restores_messages_without_persisting_the_credential() {
    let (root, core, character_id) = imported_core();
    let (base_url, captured_request) = spawn_completed_provider();
    let profile = core
        .upsert_provider_profile(ProviderProfile {
            id: "local-test".to_owned(),
            display_name: "Local test".to_owned(),
            base_url,
            model: "fixture".to_owned(),
            timeout_seconds: 5,
        })
        .expect("save provider profile");
    core.update_settings(&AppSettings {
        preserve_partial_generations: true,
        selected_provider_profile_id: Some(profile.id.clone()),
        selected_model_route_id: None,
        selected_generation_preset_id: None,
        ..AppSettings::default()
    })
    .expect("save settings");

    let conversation = core
        .open_conversation(&character_id)
        .expect("open conversation");
    let mut events = core.subscribe_events();
    let generation = core
        .send_message(
            &conversation.id,
            "반가워",
            GenerationOperationContext::New {
                operation_nonce: "chat-vertical-stream-commit-v1",
            },
            &profile.id,
            Some("short-lived-secret".to_owned()),
        )
        .expect("send message");
    let received = collect_generation_events_until_terminal(&mut events, &generation);
    assert!(
        matches!(
            received.last().map(|event| &event.kind),
            Some(ChatEventKind::GenerationFinished)
        ),
        "expected successful generation events, got {received:?}"
    );
    wait_for_terminal_message(&core, &conversation.id);

    let request = String::from_utf8_lossy(
        &captured_request
            .recv_timeout(Duration::from_secs(2))
            .expect("captured request"),
    )
    .into_owned();
    assert!(
        request
            .to_ascii_lowercase()
            .contains("authorization: bearer short-lived-secret")
    );
    assert!(
        request.contains("\"max_tokens\":4096"),
        "core must apply its finite provider output-token limit"
    );
    assert!(
        received
            .windows(2)
            .all(|pair| pair[0].sequence < pair[1].sequence)
    );

    let messages = core.list_messages(&conversation.id).expect("messages");
    assert_eq!(messages[1].content, "안녕");
    assert_eq!(messages[1].status, MessageStatus::Complete);
    drop(core);

    let reopened = Core::open(CoreConfig::new(root.path())).expect("reopen core");
    let restored = reopened
        .list_messages(&conversation.id)
        .expect("restored messages");
    assert_eq!(restored, messages);
    let database = std::fs::read(active_database_path(root.path())).expect("database bytes");
    assert!(
        !database
            .windows(b"short-lived-secret".len())
            .any(|window| window == b"short-lived-secret"),
        "credentials must never be persisted in the Rust data root"
    );
}

#[test]
fn cancellation_preserves_partial_text_and_emits_a_terminal_event() {
    let (_root, core, character_id) = imported_core();
    let (base_url, provider_ready, provider_stop) = spawn_stalling_provider();
    let profile = core
        .upsert_provider_profile(ProviderProfile {
            id: "cancellation-test".to_owned(),
            display_name: "Cancellation test".to_owned(),
            base_url,
            model: "fixture".to_owned(),
            timeout_seconds: 5,
        })
        .expect("save provider profile");
    let conversation = core
        .open_conversation(&character_id)
        .expect("open conversation");
    let mut events = core.subscribe_events();
    let generation = core
        .send_message(
            &conversation.id,
            "중지해",
            GenerationOperationContext::New {
                operation_nonce: "chat-vertical-cancel-v1",
            },
            &profile.id,
            None,
        )
        .expect("send message");
    provider_ready
        .recv_timeout(Duration::from_secs(2))
        .expect("provider started streaming");

    let deadline = Instant::now() + Duration::from_secs(2);
    loop {
        if let Ok(event) = events.try_recv()
            && event.generation_id == generation
            && matches!(event.kind, ChatEventKind::TextDelta(_))
        {
            break;
        }
        assert!(Instant::now() < deadline, "text delta did not arrive");
        thread::sleep(Duration::from_millis(5));
    }
    core.cancel_generation(&generation)
        .expect("cancel generation");
    let received = collect_generation_events_until_terminal(&mut events, &generation);
    assert!(
        matches!(
            received.last().map(|event| &event.kind),
            Some(ChatEventKind::GenerationCancelled)
        ),
        "expected cancelled generation events, got {received:?}"
    );
    wait_for_terminal_message(&core, &conversation.id);
    let _ = provider_stop.send(());

    let messages = core.list_messages(&conversation.id).expect("messages");
    assert_eq!(messages[1].status, MessageStatus::Cancelled);
    assert_eq!(messages[1].content, "부분");
}

#[test]
fn split_credential_reflection_never_reaches_events_partial_state_or_sqlite() {
    let (root, core, character_id) = imported_core();
    let (base_url, captured_request) = spawn_reflecting_provider();
    let profile = core
        .upsert_provider_profile(ProviderProfile {
            id: "credential-reflection-test".to_owned(),
            display_name: "Credential reflection test".to_owned(),
            base_url,
            model: "fixture".to_owned(),
            timeout_seconds: 5,
        })
        .expect("save reflecting provider profile");
    core.update_settings(&AppSettings {
        preserve_partial_generations: true,
        selected_provider_profile_id: Some(profile.id.clone()),
        selected_model_route_id: None,
        selected_generation_preset_id: None,
        ..AppSettings::default()
    })
    .expect("enable partial preservation");

    let conversation = core
        .open_conversation(&character_id)
        .expect("open reflection conversation");
    let mut events = core.subscribe_events();
    let generation = core
        .send_message(
            &conversation.id,
            "Do not reflect credentials",
            GenerationOperationContext::New {
                operation_nonce: "chat-vertical-reflection-v1",
            },
            &profile.id,
            Some(REFLECTED_CREDENTIAL.to_owned()),
        )
        .expect("start reflecting generation");
    wait_for_terminal_message(&core, &conversation.id);

    assert_reflected_credential_reached_provider(&captured_request);

    let received = collect_generation_events_until_terminal(&mut events, &generation);
    let encoded_events = serde_json::to_vec(&received).expect("serialize chat events");
    assert!(
        !encoded_events
            .windows(REFLECTED_CREDENTIAL.len())
            .any(|window| window == REFLECTED_CREDENTIAL.as_bytes()),
        "the reflected credential must not reach ChatEvent"
    );
    assert!(received.iter().any(|event| {
        matches!(
            &event.kind,
            ChatEventKind::GenerationFailed { code, message }
                if code == CoreErrorCode::ProviderUnavailable.as_str()
                    && !message.contains(REFLECTED_CREDENTIAL)
        )
    }));

    let messages = core
        .list_messages(&conversation.id)
        .expect("load failed reflection messages");
    assert_eq!(messages.len(), 2);
    assert_eq!(messages[1].status, MessageStatus::Failed);
    assert_eq!(messages[1].content, "safe prefix ");
    assert!(
        messages
            .iter()
            .all(|message| !message.content.contains(REFLECTED_CREDENTIAL))
    );
    drop(core);

    let reopened = Core::open(CoreConfig::new(root.path())).expect("reopen core after reflection");
    let restored = reopened
        .list_messages(&conversation.id)
        .expect("restore failed reflection messages");
    assert_eq!(restored, messages);
    assert!(
        restored
            .iter()
            .all(|message| !message.content.contains(REFLECTED_CREDENTIAL))
    );
    drop(reopened);

    let storage = open_storage_after_core_drop(root.path());
    let stored_generation = storage
        .get_generation(&generation)
        .expect("load reflected generation");
    assert_eq!(stored_generation.status, GenerationStatus::Failed);
    assert_eq!(
        stored_generation.error_code.as_deref(),
        Some(CoreErrorCode::ProviderUnavailable.as_str())
    );
    assert!(stored_generation.provider_raw_summary.is_none());
    assert!(stored_generation.opaque_reasoning_state.is_empty());
    assert!(
        storage
            .list_messages(&conversation.id)
            .expect("load stored reflection messages")
            .iter()
            .all(|message| !message.content.contains(REFLECTED_CREDENTIAL))
    );
    drop(storage);

    assert_tree_does_not_contain(root.path(), REFLECTED_CREDENTIAL.as_bytes());
}
