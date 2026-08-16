use std::{
    env,
    fmt::Write as _,
    fs,
    io::{Read, Write},
    net::{TcpListener, TcpStream},
    path::{Component, Path, PathBuf},
    sync::mpsc,
    thread,
    time::{Duration, Instant},
};

use chrono::{DateTime, Utc};
use lorepia_core::{
    CanonicalOrigin, ConnectionConfigEntry, ConnectionConfigValue, ConversationMode, Core,
    CoreConfig, EndpointPath, GenerationPreset, GenerationPresetId, GenerationPromptCacheSettings,
    GenerationReasoningSettings, GenerationTarget, MessageStatus, ModelAvailability,
    ModelMetadataSource, ModelRoute, ModelRouteConfig, ModelRouteId, ProviderConnectionDraft,
    ProviderConnectionId, ProviderNetworkMode,
};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use zeroize::{Zeroize, Zeroizing};

const CONNECTION_ID: &str = "fixture-openai-loopback";
const MODEL_ROUTE_ID: &str = "fixture-openai-route";
const GENERATION_PRESET_ID: &str = "fixture-openai-preset";
const ASSISTANT_MESSAGE: &str = "Synthetic assistant reply.";

fn main() {
    let mut args = env::args_os().skip(1);
    let command = args.next().expect(
        "usage: lorepia-schema11-runtime-harness \
         <seed ROOT PACKAGE MANIFEST|inspect-legacy ROOT MANIFEST STATE>",
    );
    match command.to_str() {
        Some("seed") => {
            let root = PathBuf::from(args.next().expect("missing Core root"));
            let package = PathBuf::from(args.next().expect("missing fixture package"));
            let manifest_path = PathBuf::from(args.next().expect("missing runtime manifest"));
            assert!(args.next().is_none(), "unexpected extra argument");
            seed(&root, &package, &manifest_path);
        }
        Some("inspect-legacy") => {
            let root = PathBuf::from(args.next().expect("missing Core root"));
            let manifest_path = PathBuf::from(args.next().expect("missing runtime manifest"));
            let state_path = PathBuf::from(args.next().expect("missing candidate state"));
            assert!(args.next().is_none(), "unexpected extra argument");
            inspect_legacy(&root, &manifest_path, &state_path);
        }
        _ => panic!("unsupported harness command"),
    }
}

#[allow(
    clippy::too_many_lines,
    reason = "the frozen seed is one auditable, linear compatibility scenario"
)]
fn seed(root: &Path, package: &Path, manifest_path: &Path) {
    assert!(!root.exists(), "seed Core root must not pre-exist");
    let credential = runtime_credential();
    let credential_fingerprint = sha256_bytes(credential.as_bytes());
    let (authenticated_request, api_origin) = spawn_provider(credential_fingerprint);
    let core = Core::open(CoreConfig::new(root)).expect("open exact frozen Core");
    assert_eq!(
        core.health_check()
            .expect("frozen Core health")
            .schema_version,
        11
    );

    let inspection = core
        .inspect_import(package)
        .expect("inspect project-owned fixture package");
    let source_size = inspection.source_size;
    let character = core
        .commit_import(&inspection.id)
        .expect("commit project-owned fixture package");
    let avatar_sha256 = character
        .avatar_asset_hash
        .as_deref()
        .expect("fixture character must have an avatar")
        .to_owned();

    let template = core
        .list_provider_templates()
        .expect("list frozen provider templates")
        .into_iter()
        .find(|candidate| candidate.id.as_str() == "openai-chat-compatible-v1")
        .expect("frozen OpenAI-compatible template");
    let origin = CanonicalOrigin::parse(&api_origin).expect("canonical fixture origin");
    let connection = core
        .create_provider_connection(ProviderConnectionDraft {
            id: ProviderConnectionId::from(CONNECTION_ID),
            template_id: template.id.clone(),
            template_version: template.manifest_version,
            display_name: "Synthetic loopback fixture".to_owned(),
            api_origin: origin.clone(),
            api_base_path: Some(EndpointPath::parse("/v1").expect("fixed API base path")),
            network_mode: ProviderNetworkMode::LocalLoopback,
            local_network_approval: None,
            values: vec![ConnectionConfigEntry {
                key: "api_base_url".to_owned(),
                value: ConnectionConfigValue::Text(format!("{api_origin}/v1")),
            }],
            approved_credential_origin: Some(origin),
            timeout_seconds: 5,
        })
        .expect("create deterministic provider connection");

    let timestamp = fixed_time();
    let route = core
        .upsert_model_route(ModelRoute {
            id: ModelRouteId::from(MODEL_ROUTE_ID),
            connection_id: connection.id.clone(),
            api_family: template.api_family,
            model_id: "fixture-model".to_owned(),
            display_name: Some("Synthetic fixture model".to_owned()),
            route_config: ModelRouteConfig::default(),
            status: ModelAvailability::Available,
            miss_count: 0,
            raw_metadata: None,
            metadata_source: ModelMetadataSource::UserOverride,
            metadata_observed_at: None,
            last_reconciled_sync_job_id: None,
            metadata_sync_job_id: None,
            first_seen_at: timestamp,
            last_seen_at: Some(timestamp),
        })
        .expect("create deterministic model route");
    let preset = core
        .upsert_generation_preset(GenerationPreset {
            id: GenerationPresetId::from(GENERATION_PRESET_ID),
            model_route_id: route.id.clone(),
            display_name: "Synthetic fixture preset".to_owned(),
            values: Vec::new(),
            reasoning: GenerationReasoningSettings::default(),
            prompt_cache: GenerationPromptCacheSettings::default(),
            created_at: timestamp,
            updated_at: timestamp,
        })
        .expect("create deterministic generation preset");
    let target = GenerationTarget {
        model_route_id: route.id.clone(),
        generation_preset_id: preset.id.clone(),
    };
    let settings = core
        .select_generation_target(Some(target.clone()))
        .expect("select deterministic generation target");

    let conversation = core
        .create_conversation(
            &character.id,
            "Synthetic continuity conversation",
            ConversationMode::Chat,
        )
        .expect("create synthetic conversation");
    let generation_id = core
        .send_message_with_target(
            &conversation.id,
            "Synthetic user message.",
            &target,
            Some(credential.as_str().to_owned()),
        )
        .expect("start frozen loopback generation");
    let messages = wait_for_messages(&core, &conversation.id);

    let authenticated_match = authenticated_request
        .recv_timeout(Duration::from_secs(5))
        .expect("receive redacted frozen loopback authentication result");
    assert!(
        authenticated_match,
        "the frozen Core did not bind the runtime fixture credential"
    );
    assert_eq!(messages[1].content, ASSISTANT_MESSAGE);

    let branches = core
        .list_conversation_branches(&conversation.id)
        .expect("list fixture branches");
    let conversation = core
        .get_conversation(&conversation.id)
        .expect("reload persisted fixture conversation");
    let stats = core.database_stats().expect("read fixture database stats");
    drop(core);
    assert_tree_excludes(root, credential.as_bytes());

    let source_relative_path = cas_relative_path("sources", &character.source_hash);
    let avatar_relative_path = cas_relative_path("assets", &avatar_sha256);
    let avatar_size = fs::metadata(root.join(&avatar_relative_path))
        .expect("read fixture avatar metadata")
        .len();
    let manifest = json!({
        "format_version": 1,
        "application_schema_version": 11,
        "database_stats": {
            "characters": stats.characters,
            "conversations": stats.conversations,
            "messages": stats.messages,
            "pending_imports": stats.pending_imports,
        },
        "content_sources": [{
            "sha256": character.source_hash,
            "relative_path": source_relative_path,
            "size_bytes": source_size,
        }],
        "assets": [{
            "sha256": avatar_sha256,
            "relative_path": avatar_relative_path,
            "size_bytes": avatar_size,
        }],
        "characters": [character],
        "provider_connections": [connection],
        "model_routes": [route],
        "generation_presets": [preset],
        "app_settings": [{"key": "application", "value": settings}],
        "conversations": [conversation],
        "conversation_branches": branches,
        "messages": messages,
        "generation_id": generation_id,
        "credential_evidence": {
            "loopback_authenticated_match": true,
            "raw_credential_persisted": false,
            "platform_vault_seeded": false,
        },
    });
    let manifest_bytes =
        Zeroizing::new(serde_json::to_vec_pretty(&manifest).expect("encode runtime manifest"));
    assert!(
        !contains_bytes(&manifest_bytes, credential.as_bytes()),
        "runtime manifest retained the disposable credential"
    );
    fs::write(manifest_path, &*manifest_bytes).expect("write runtime manifest");
    drop(credential);
    inspect_snapshot(root, &manifest);
}

fn inspect_legacy(root: &Path, manifest_path: &Path, state_path: &Path) {
    let manifest = read_json(manifest_path, "runtime manifest");
    let state = read_json(state_path, "current candidate state");
    inspect_snapshot(root, &manifest);

    assert_eq!(state["format_version"].as_u64(), Some(1));
    assert_eq!(
        PathBuf::from(json_string(&state, "root")),
        root,
        "current continuity test reported a different Core root"
    );
    assert_eq!(
        state["post_cutover_conversation_visible_in_canonical"].as_bool(),
        Some(false)
    );
    assert_eq!(
        state["post_cutover_conversation_visible_in_active"].as_bool(),
        Some(true)
    );
    assert_eq!(
        state["compatible_rollback_conversation_visible_in_canonical"].as_bool(),
        Some(false)
    );
    assert_eq!(
        state["compatible_rollback_conversation_visible_in_active"].as_bool(),
        Some(true)
    );
    assert_eq!(
        sha256_file(&root.join("db/lorepia.sqlite3")),
        json_string(&state, "canonical_database_sha256"),
        "current cutover did not preserve the exact frozen canonical database"
    );

    let active_relative = PathBuf::from(json_string(&state, "active_database_relative_path"));
    assert_owned_relative_path(&active_relative);
    assert!(
        root.join(active_relative).is_file(),
        "current continuity test selected a missing active candidate"
    );

    let core = Core::open(CoreConfig::new(root)).expect("frozen Core must reopen canonical root");
    let canonical_conversations = core
        .list_conversations()
        .expect("list canonical conversations");
    for field in [
        "post_cutover_conversation_id",
        "compatible_rollback_conversation_id",
    ] {
        let candidate_conversation_id = json_string(&state, field);
        assert!(
            canonical_conversations
                .iter()
                .all(|conversation| conversation.id.0 != candidate_conversation_id),
            "frozen snapshot must not be misrepresented as containing active-generation write {field}"
        );
    }
}

fn inspect_snapshot(root: &Path, manifest: &Value) {
    let core = Core::open(CoreConfig::new(root)).expect("frozen Core must reopen canonical root");
    assert_eq!(
        core.health_check()
            .expect("frozen Core health")
            .schema_version,
        11,
        "the exact frozen Core must select the preserved schema-eleven canonical snapshot"
    );
    validate_original_semantics(&core, manifest);
    validate_cas(root, manifest);
}

fn validate_original_semantics(core: &Core, manifest: &Value) {
    let expected_character = &manifest["characters"][0];
    let character = core
        .list_characters()
        .expect("list fixture characters")
        .into_iter()
        .find(|candidate| candidate.id == json_string(expected_character, "id"))
        .expect("find fixture character");
    let actual_character = serde_json::to_value(character).expect("serialize fixture character");
    for field in [
        "id",
        "name",
        "description",
        "source_hash",
        "avatar_asset_hash",
        "created_at",
    ] {
        assert_eq!(
            actual_character[field], expected_character[field],
            "fixture character field changed: {field}"
        );
    }

    let expected_conversation = &manifest["conversations"][0];
    let conversation = core
        .list_conversations()
        .expect("list fixture conversations")
        .into_iter()
        .find(|candidate| candidate.id.0 == json_string(expected_conversation, "id"))
        .expect("find original fixture conversation");
    assert_eq!(
        serde_json::to_value(&conversation).expect("serialize fixture conversation"),
        *expected_conversation
    );

    let actual_messages = core
        .list_messages(&conversation.id)
        .expect("list original fixture messages");
    assert_eq!(
        serde_json::to_value(&actual_messages).expect("serialize fixture messages"),
        manifest["messages"]
    );
    let actual_branches = core
        .list_conversation_branches(&conversation.id)
        .expect("list original fixture branches");
    assert_eq!(
        serde_json::to_value(actual_branches).expect("serialize fixture branches"),
        manifest["conversation_branches"]
    );

    let settings = serde_json::to_value(core.get_settings().expect("read fixture settings"))
        .expect("serialize fixture settings");
    let expected_settings = &manifest["app_settings"][0]["value"];
    for field in [
        "preserve_partial_generations",
        "selected_provider_profile_id",
        "selected_model_route_id",
        "selected_generation_preset_id",
    ] {
        assert_eq!(
            settings[field], expected_settings[field],
            "fixture setting changed: {field}"
        );
    }

    let connections = core
        .list_provider_connections()
        .expect("list fixture provider connections");
    assert_eq!(connections.len(), 1);
    let connection = connections
        .into_iter()
        .find(|candidate| candidate.id.as_str() == CONNECTION_ID)
        .expect("find fixture provider connection");
    let routes = core
        .list_model_routes(&connection.id)
        .expect("list fixture model routes");
    assert_eq!(routes.len(), 1);
    assert_eq!(routes[0].id.as_str(), MODEL_ROUTE_ID);
    let presets = core
        .list_generation_presets(&routes[0].id)
        .expect("list fixture generation presets");
    assert_eq!(presets.len(), 1);
    assert_eq!(presets[0].id.as_str(), GENERATION_PRESET_ID);

    let stats = core.database_stats().expect("read fixture database stats");
    assert_eq!(
        stats.characters,
        manifest["database_stats"]["characters"]
            .as_u64()
            .expect("character count")
    );
    assert_eq!(
        stats.conversations,
        manifest["database_stats"]["conversations"]
            .as_u64()
            .expect("conversation count")
    );
    assert_eq!(
        stats.messages,
        manifest["database_stats"]["messages"]
            .as_u64()
            .expect("message count")
    );
}

fn validate_cas(root: &Path, manifest: &Value) {
    for key in ["content_sources", "assets"] {
        for entry in manifest[key].as_array().expect("CAS manifest entries") {
            let relative_path = json_string(entry, "relative_path");
            let relative = Path::new(&relative_path);
            assert_owned_relative_path(relative);
            let bytes = fs::read(root.join(relative)).expect("read fixture CAS object");
            assert_eq!(
                u64::try_from(bytes.len()).expect("CAS size fits u64"),
                entry["size_bytes"].as_u64().expect("CAS byte size")
            );
            assert_eq!(
                format!("{:x}", Sha256::digest(&bytes)),
                json_string(entry, "sha256"),
                "fixture CAS digest changed"
            );
        }
    }
}

fn assert_owned_relative_path(path: &Path) {
    assert!(
        !path.is_absolute()
            && path
                .components()
                .all(|component| matches!(component, Component::Normal(_))),
        "fixture path must remain an owned relative path"
    );
}

fn sha256_file(path: &Path) -> String {
    format!(
        "{:x}",
        Sha256::digest(fs::read(path).expect("read file for SHA-256"))
    )
}

fn wait_for_messages(
    core: &Core,
    conversation_id: &lorepia_core::ConversationId,
) -> Vec<lorepia_core::Message> {
    let deadline = Instant::now() + Duration::from_secs(15);
    loop {
        let messages = core
            .list_messages(conversation_id)
            .expect("list frozen fixture messages");
        if messages.len() == 2
            && messages
                .iter()
                .all(|message| message.status == MessageStatus::Complete)
        {
            return messages;
        }
        assert!(
            Instant::now() < deadline,
            "frozen fixture generation did not finish: {messages:?}"
        );
        thread::sleep(Duration::from_millis(20));
    }
}

fn spawn_provider(expected_credential_fingerprint: [u8; 32]) -> (mpsc::Receiver<bool>, String) {
    let listener =
        TcpListener::bind("127.0.0.1:0").expect("bind isolated frozen-runtime loopback fixture");
    let api_origin = format!(
        "http://{}",
        listener
            .local_addr()
            .expect("read frozen-runtime loopback address")
    );
    let (sender, receiver) = mpsc::channel();
    thread::spawn(move || {
        let (mut stream, _) = listener.accept().expect("accept one fixture request");
        let mut request = read_request(&mut stream);
        let authenticated_match =
            request_bears_credential_fingerprint(&request, &expected_credential_fingerprint);
        request.zeroize();
        sender
            .send(authenticated_match)
            .expect("report redacted fixture authentication result");
        let body = concat!(
            "data: {\"choices\":[{\"index\":0,\"delta\":{\"content\":\"Synthetic assistant reply.\"}}]}\n\n",
            "data: {\"choices\":[{\"index\":0,\"delta\":{},\"finish_reason\":\"stop\"}]}\n\n",
            "data: {\"choices\":[],\"usage\":{\"prompt_tokens\":7,\"completion_tokens\":3}}\n\n",
            "data: [DONE]\n\n"
        );
        write!(
            stream,
            "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
            body.len(),
            body
        )
        .expect("write deterministic fixture response");
    });
    (receiver, api_origin)
}

fn request_bears_credential_fingerprint(
    request: &[u8],
    expected_credential_fingerprint: &[u8; 32],
) -> bool {
    let Some(header_end) = request.windows(4).position(|value| value == b"\r\n\r\n") else {
        return false;
    };
    let Ok(headers) = std::str::from_utf8(&request[..header_end]) else {
        return false;
    };
    let mut authorization_values = headers.lines().filter_map(|line| {
        let (name, value) = line.split_once(':')?;
        name.eq_ignore_ascii_case("authorization")
            .then_some(value.trim())
    });
    let Some(value) = authorization_values.next() else {
        return false;
    };
    if authorization_values.next().is_some() {
        return false;
    }
    let mut parts = value.split_ascii_whitespace();
    let (Some(scheme), Some(token)) = (parts.next(), parts.next()) else {
        return false;
    };
    parts.next().is_none()
        && scheme.eq_ignore_ascii_case("bearer")
        && sha256_bytes(token.as_bytes()) == *expected_credential_fingerprint
}

fn read_request(stream: &mut TcpStream) -> Vec<u8> {
    stream
        .set_read_timeout(Some(Duration::from_secs(10)))
        .expect("set fixture request timeout");
    let mut request = Vec::new();
    let mut buffer = Zeroizing::new([0_u8; 4096]);
    loop {
        let read = stream.read(&mut *buffer).expect("read fixture request");
        if read == 0 {
            break;
        }
        request.extend_from_slice(&buffer[..read]);
        let Some(header_end) = request.windows(4).position(|value| value == b"\r\n\r\n") else {
            continue;
        };
        let headers = std::str::from_utf8(&request[..header_end])
            .expect("fixture request headers must be UTF-8");
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

fn runtime_credential() -> Zeroizing<String> {
    let mut entropy = Zeroizing::new([0_u8; 32]);
    getrandom::fill(&mut *entropy).expect("generate disposable runtime credential");
    let mut credential = String::with_capacity(entropy.len() * 2);
    for byte in entropy.iter() {
        write!(&mut credential, "{byte:02x}").expect("encode disposable runtime credential");
    }
    Zeroizing::new(credential)
}

fn sha256_bytes(bytes: &[u8]) -> [u8; 32] {
    Sha256::digest(bytes).into()
}

fn assert_tree_excludes(root: &Path, needle: &[u8]) {
    let mut pending = vec![root.to_path_buf()];
    while let Some(path) = pending.pop() {
        let metadata = fs::symlink_metadata(&path).expect("inspect frozen fixture data root");
        if metadata.is_dir() {
            pending.extend(
                fs::read_dir(&path)
                    .expect("read frozen fixture data root")
                    .map(|entry| entry.expect("read frozen fixture data entry").path()),
            );
        } else if metadata.is_file() {
            let bytes = Zeroizing::new(fs::read(&path).expect("read frozen fixture data file"));
            assert!(
                !contains_bytes(&bytes, needle),
                "a frozen fixture data file retained the disposable credential: {}",
                path.display()
            );
        }
    }
}

fn contains_bytes(haystack: &[u8], needle: &[u8]) -> bool {
    !needle.is_empty()
        && haystack
            .windows(needle.len())
            .any(|window| window == needle)
}

fn cas_relative_path(namespace: &str, sha256: &str) -> String {
    assert_eq!(sha256.len(), 64, "fixture CAS digest length");
    format!("{namespace}/sha256/{}/{}", &sha256[..2], &sha256[2..])
}

fn fixed_time() -> DateTime<Utc> {
    DateTime::parse_from_rfc3339("2026-08-02T00:00:00Z")
        .expect("fixed RFC3339 timestamp")
        .with_timezone(&Utc)
}

fn read_json(path: &Path, label: &str) -> Value {
    serde_json::from_slice(
        &fs::read(path)
            .unwrap_or_else(|error| panic!("cannot read {label} at {}: {error}", path.display())),
    )
    .unwrap_or_else(|error| panic!("cannot parse {label}: {error}"))
}

fn json_string(value: &Value, key: &str) -> String {
    value[key]
        .as_str()
        .unwrap_or_else(|| panic!("missing string field {key}"))
        .to_owned()
}
