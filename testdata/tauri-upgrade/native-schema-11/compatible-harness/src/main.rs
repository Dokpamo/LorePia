use std::{env, fs, path::Path};

use lorepia_shell_api::{ConversationModeDto, CreateConversationInput, ShellApi};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

const A_TITLE: &str = "Tauri AppState Shell API continuity evidence";
const B_TITLE: &str = "Source-compatible rollback continuity evidence";

fn main() {
    let mut args = env::args_os().skip(1);
    let command = args
        .next()
        .expect("usage: lorepia-schema11-compatible-rollback-harness round-trip ROOT STATE");
    assert_eq!(
        command.to_str(),
        Some("round-trip"),
        "unsupported compatible rollback harness command"
    );
    let root = args.next().expect("missing Core root");
    let state_path = args.next().expect("missing runtime state path");
    assert!(args.next().is_none(), "unexpected extra argument");
    round_trip(Path::new(&root), Path::new(&state_path));
}

fn round_trip(root: &Path, state_path: &Path) {
    let state = read_json(state_path, "current candidate state");
    assert_eq!(state["format_version"].as_u64(), Some(1));
    assert_eq!(
        Path::new(json_string(&state, "root")),
        root,
        "compatible rollback client received a different Core root"
    );
    assert_eq!(
        state["post_cutover_conversation_visible_in_active"].as_bool(),
        Some(true)
    );
    assert_eq!(
        state["post_cutover_conversation_visible_in_canonical"].as_bool(),
        Some(false)
    );

    let artifact_sha256 =
        sha256_file(&env::current_exe().expect("resolve compatible rollback client executable"));
    let canonical = root.join("db/lorepia.sqlite3");
    let canonical_sha256 = sha256_file(&canonical);
    assert_eq!(
        canonical_sha256,
        json_string(&state, "canonical_database_sha256"),
        "canonical schema-eleven database changed before compatible rollback"
    );

    let active_relative = Path::new(json_string(&state, "active_database_relative_path"));
    assert!(
        !active_relative.is_absolute(),
        "active database path must remain relative to the owned root"
    );
    assert!(
        root.join(active_relative).is_file(),
        "active committed database generation is missing"
    );

    let a_id = json_string(&state, "post_cutover_conversation_id").to_owned();
    let shell = ShellApi::open_data_root(root).expect("open active generation through Shell API");
    assert!(
        shell
            .bootstrap()
            .expect("compatible recovery Shell API bootstrap")
            .health
            .schema_version
            > 11,
        "compatible recovery client must understand the current storage schema"
    );
    let a = shell
        .get_conversation(&a_id)
        .expect("compatible recovery client must read Shell API write A");
    assert_eq!(a.title, A_TITLE);
    let b = shell
        .create_conversation(CreateConversationInput {
            character_id: a.character_id,
            title: B_TITLE.to_owned(),
            mode: ConversationModeDto::Chat,
            greeting: None,
        })
        .expect("compatible recovery client must persist write B through Shell API");
    assert_eq!(
        shell
            .get_conversation(&a_id)
            .expect("write A remains readable after B")
            .title,
        A_TITLE
    );
    assert_eq!(
        shell
            .get_conversation(&b.id)
            .expect("write B is readable before compatible client exit")
            .title,
        B_TITLE
    );
    drop(shell);

    assert_eq!(
        sha256_file(&canonical),
        canonical_sha256,
        "compatible rollback client changed the canonical schema-eleven database"
    );
    publish_runtime_evidence(state, state_path, &artifact_sha256, &b.id);
}

fn publish_runtime_evidence(
    mut state: Value,
    state_path: &Path,
    artifact_sha256: &str,
    conversation_id: &str,
) {
    let object = state
        .as_object_mut()
        .expect("runtime state must be a JSON object");
    object.insert(
        "compatible_rollback_artifact_sha256".to_owned(),
        json!(artifact_sha256),
    );
    object.insert(
        "compatible_rollback_conversation_id".to_owned(),
        json!(conversation_id),
    );
    object.insert(
        "compatible_rollback_conversation_title".to_owned(),
        json!(B_TITLE),
    );
    object.insert(
        "compatible_rollback_conversation_visible_in_active".to_owned(),
        json!(true),
    );
    object.insert(
        "compatible_rollback_conversation_visible_in_canonical".to_owned(),
        json!(false),
    );
    fs::write(
        state_path,
        serde_json::to_vec_pretty(&state).expect("encode compatible rollback runtime evidence"),
    )
    .expect("write compatible rollback runtime evidence");
}

fn read_json(path: &Path, label: &str) -> Value {
    serde_json::from_slice(
        &fs::read(path)
            .unwrap_or_else(|error| panic!("cannot read {label} at {}: {error}", path.display())),
    )
    .unwrap_or_else(|error| panic!("cannot parse {label}: {error}"))
}

fn json_string<'a>(value: &'a Value, key: &str) -> &'a str {
    value[key]
        .as_str()
        .unwrap_or_else(|| panic!("missing string field {key}"))
}

fn sha256_file(path: &Path) -> String {
    format!(
        "{:x}",
        Sha256::digest(fs::read(path).expect("read file for SHA-256"))
    )
}
