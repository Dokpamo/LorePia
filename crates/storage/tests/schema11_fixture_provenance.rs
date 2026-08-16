use std::{collections::BTreeMap, fs, path::Path};

use rusqlite::{Connection, OpenFlags};
use serde_json::Value;
use sha2::{Digest, Sha256};
use tempfile::tempdir;

const SCHEMA_SQL: &[u8] =
    include_bytes!("../../../testdata/tauri-upgrade/native-schema-11/schema-11.sql");
const SEMANTIC_MANIFEST: &[u8] =
    include_bytes!("../../../testdata/tauri-upgrade/native-schema-11/semantic-manifest.json");
const SCHEMA_INVENTORY: &[u8] =
    include_bytes!("../../../testdata/tauri-upgrade/native-schema-11/schema-inventory.json");
const FROZEN_CORE_RUN: &[u8] =
    include_bytes!("../../../testdata/tauri-upgrade/native-schema-11/frozen-core-run.json");
const PROVENANCE: &[u8] =
    include_bytes!("../../../testdata/tauri-upgrade/native-schema-11/provenance.json");
const HARNESS_MANIFEST: &[u8] =
    include_bytes!("../../../testdata/tauri-upgrade/native-schema-11/harness/Cargo.toml.in");
const HARNESS_LOCKFILE: &[u8] =
    include_bytes!("../../../testdata/tauri-upgrade/native-schema-11/harness/Cargo.lock");
const HARNESS_SOURCE: &[u8] =
    include_bytes!("../../../testdata/tauri-upgrade/native-schema-11/harness/src/main.rs");
const COMPATIBLE_HARNESS_MANIFEST: &[u8] = include_bytes!(
    "../../../testdata/tauri-upgrade/native-schema-11/compatible-harness/Cargo.toml.in"
);
const COMPATIBLE_HARNESS_LOCKFILE: &[u8] = include_bytes!(
    "../../../testdata/tauri-upgrade/native-schema-11/compatible-harness/Cargo.lock"
);
const COMPATIBLE_HARNESS_SOURCE: &[u8] = include_bytes!(
    "../../../testdata/tauri-upgrade/native-schema-11/compatible-harness/src/main.rs"
);
const RUNTIME_SCRIPT: &[u8] = include_bytes!("../../../scripts/test-native-schema11-runtime.sh");
const SOURCE_PACKAGE: &[u8] = include_bytes!("../../../testdata/packages/with-avatar.charx");
const AVATAR_ASSET: &[u8] =
    include_bytes!("../../../testdata/tauri-upgrade/native-schema-11/assets/avatar.png");

#[test]
fn frozen_schema_eleven_artifacts_match_provenance_inventory_semantics_and_cas() {
    let provenance = parse_json(PROVENANCE, "provenance");
    assert_eq!(
        provenance["source_repository"].as_str(),
        Some("https://github.com/Dokpamo/lorepia-native-reference")
    );
    assert_eq!(
        provenance["annotated_tag"].as_str(),
        Some("native-baseline-before-tauri-2026-08-02")
    );
    assert_eq!(
        provenance["annotated_tag_object"].as_str(),
        Some("9a4a3d5ee08c3457fed9842ccf4184272805e0d0")
    );
    assert_eq!(
        provenance["peeled_commit"].as_str(),
        Some("66e398fa6256f17b04c82569e6764a9e5332265c")
    );
    assert_eq!(
        provenance["checkout"]["location"].as_str(),
        Some("ephemeral_external_checkout_outside_workspace")
    );
    assert_eq!(
        provenance["generator"]["command"].as_str(),
        Some("./scripts/test-native-schema11-runtime.sh")
    );

    for (relative_path, bytes) in [
        ("schema-11.sql", SCHEMA_SQL),
        ("semantic-manifest.json", SEMANTIC_MANIFEST),
        ("schema-inventory.json", SCHEMA_INVENTORY),
        ("frozen-core-run.json", FROZEN_CORE_RUN),
        ("assets/avatar.png", AVATAR_ASSET),
        ("harness/Cargo.toml.in", HARNESS_MANIFEST),
        ("harness/Cargo.lock", HARNESS_LOCKFILE),
        ("harness/src/main.rs", HARNESS_SOURCE),
        (
            "compatible-harness/Cargo.toml.in",
            COMPATIBLE_HARNESS_MANIFEST,
        ),
        ("compatible-harness/Cargo.lock", COMPATIBLE_HARNESS_LOCKFILE),
        ("compatible-harness/src/main.rs", COMPATIBLE_HARNESS_SOURCE),
    ] {
        assert_eq!(
            provenance["artifacts"][relative_path].as_str(),
            Some(sha256(bytes).as_str()),
            "artifact bytes drifted from provenance: {relative_path}"
        );
    }
    assert_eq!(
        provenance["seed"]["sha256"].as_str(),
        Some(sha256(SOURCE_PACKAGE).as_str()),
        "the project-owned synthetic source package drifted from provenance"
    );
    assert_eq!(
        provenance["runtime_verification"]["script_sha256"].as_str(),
        Some(sha256(RUNTIME_SCRIPT).as_str()),
        "the exact frozen-runtime verification script drifted from provenance"
    );
    validate_runtime_script_token_lifetime();
    validate_runtime_script_result_labels();
    validate_runtime_credential_contract();
    validate_source_compatible_rollback_contract();

    let root = tempdir().expect("temporary frozen fixture root");
    let database_path = restore_fixture(root.path());
    let connection = Connection::open_with_flags(&database_path, OpenFlags::SQLITE_OPEN_READ_ONLY)
        .expect("open restored frozen fixture read-only");
    validate_schema(&connection, &provenance);

    let semantic_manifest = parse_json(SEMANTIC_MANIFEST, "semantic manifest");
    validate_semantics(&connection, &semantic_manifest);
    validate_cas(root.path(), &connection, &semantic_manifest);
}

fn validate_runtime_script_result_labels() {
    let script = std::str::from_utf8(RUNTIME_SCRIPT).expect("runtime script is UTF-8");
    for exact_result in [
        "exact frozen runtime preserved-canonical schema11 readback: PASS",
        "Tauri AppState shell-api active-generation write A: PASS",
        "source-compatible shell-api recovery client active-generation A/B round trip: PASS",
        "active-generation A/B visible to exact frozen runtime: EXPECTED_FALSE",
        "signed/platform rollback-client drill: NOT_RUN",
    ] {
        assert!(
            script.contains(exact_result),
            "runtime script is missing its exact result label: {exact_result}"
        );
    }
    assert!(
        !script.contains("frozen-runtime rollback snapshot: PASS"),
        "preserved canonical readback must not be labeled as rollback proof"
    );
    assert!(
        !script.contains("signed or compatible rollback-client drill: NOT_RUN"),
        "a source-compatible Shell API round trip and a signed platform drill must be reported separately"
    );
    assert!(
        !script.contains("post-Tauri"),
        "a command-equivalent Shell API fixture must not be mislabeled as a packaged Tauri write"
    );
}

fn validate_runtime_script_token_lifetime() {
    let script = std::str::from_utf8(RUNTIME_SCRIPT).expect("runtime script is UTF-8");
    let private_fetch = script
        .find("-C \"$native_checkout\" fetch")
        .expect("runtime script performs the private reference fetch");
    let unset_token = script
        .find("unset LOREPIA_NATIVE_REFERENCE_TOKEN")
        .expect("runtime script removes the private reference token");
    let first_cargo = script
        .find("cargo fetch")
        .expect("runtime script resolves frozen harness dependencies");
    assert!(
        private_fetch < unset_token && unset_token < first_cargo,
        "the private reference token must be removed after Git fetch and before any Cargo build"
    );
}

fn validate_runtime_credential_contract() {
    let source = std::str::from_utf8(HARNESS_SOURCE).expect("harness source is UTF-8");
    let manifest = std::str::from_utf8(HARNESS_MANIFEST).expect("harness manifest is UTF-8");

    assert!(
        !source.contains("const CREDENTIAL"),
        "the frozen runtime credential must never be committed as a source literal"
    );
    for required_source_contract in [
        "getrandom::fill(&mut *entropy)",
        "Zeroizing<String>",
        "sha256_bytes(credential.as_bytes())",
        "mpsc::Receiver<bool>",
        "request.zeroize()",
        "assert_tree_excludes(root, credential.as_bytes())",
        "!contains_bytes(&manifest_bytes, credential.as_bytes())",
    ] {
        assert!(
            source.contains(required_source_contract),
            "the frozen runtime credential contract is missing: {required_source_contract}"
        );
    }
    assert!(
        !source.contains("mpsc::Receiver<Vec<u8>>"),
        "the complete authenticated request must not cross the fixture-server thread boundary"
    );
    for required_dependency in ["getrandom = \"0.4.3\"", "zeroize = \"1.9.0\""] {
        assert!(
            manifest.lines().any(|line| line == required_dependency),
            "the frozen runtime credential dependency is missing: {required_dependency}"
        );
    }
}

fn validate_source_compatible_rollback_contract() {
    let script = std::str::from_utf8(RUNTIME_SCRIPT).expect("runtime script is UTF-8");
    let source =
        std::str::from_utf8(COMPATIBLE_HARNESS_SOURCE).expect("compatible harness source is UTF-8");
    let manifest = std::str::from_utf8(COMPATIBLE_HARNESS_MANIFEST)
        .expect("compatible harness manifest is UTF-8");

    for required_source_contract in [
        "ShellApi::open_data_root(root)",
        ".bootstrap()",
        "current_exe()",
        "compatible_rollback_artifact_sha256",
        "get_conversation(&a_id)",
        "create_conversation(CreateConversationInput {",
        "mode: ConversationModeDto::Chat",
        "drop(shell)",
    ] {
        assert!(
            source.contains(required_source_contract),
            "the source-compatible rollback contract is missing: {required_source_contract}"
        );
    }
    assert!(
        manifest.contains("lorepia-shell-api = { path = \"@LOREPIA_SHELL_API_PATH@\" }"),
        "the compatible client must link the production Shell API boundary through the prepared path"
    );
    assert!(
        !manifest.contains("lorepia-core")
            && !manifest.contains("lorepia-storage")
            && !manifest.contains("tauri ="),
        "the compatible client must not bypass Shell API through Core, Storage, or Tauri dependencies"
    );

    let build = script
        .find("cargo build \\")
        .expect("runtime script prebuilds the compatible client");
    let write_a = script
        .find("tauri_app_state_shell_api_writes_active_generation_for_compatible_recovery")
        .expect("runtime script writes A through Tauri AppState and Shell API");
    let compatible_process = script
        .find("\"$compatible_binary\" round-trip")
        .expect("runtime script runs the prebuilt compatible client directly");
    let current_reopen = script
        .find("\n    state::tests::tauri_app_state_shell_api_reopens_compatible_recovery_writes \\")
        .expect("runtime script reopens A and B through Tauri AppState and Shell API");
    let frozen_reopen = script
        .find("inspect-legacy")
        .expect("runtime script finally reopens the canonical snapshot through frozen Core");
    assert!(
        build < write_a
            && write_a < compatible_process
            && compatible_process < current_reopen
            && current_reopen < frozen_reopen,
        "compatible rollback execution order must be prebuild, AppState/Shell write A, separate Shell A-read/B-write, AppState/Shell A/B read, frozen canonical read"
    );
    assert!(
        script.contains("compatible_artifact_sha256=\"$(sha256_file \"$compatible_binary\")\"")
            && script.contains("Source-compatible rollback client artifact changed after write A."),
        "the runtime proof must bind and recheck the exact prebuilt compatible artifact"
    );
    let expected_reopen_test_output = concat!(
        "test state::tests::",
        "tauri_app_state_shell_api_reopens_compatible_recovery_writes ... ok"
    );
    assert!(
        script.contains(expected_reopen_test_output)
            && script.contains("current_reopen_test_log=")
            && script.contains("2>&1 | tee \"$current_reopen_test_log\"")
            && script.contains("grep -Fqx \"$expected_reopen_test_output\""),
        "the current AppState reopen proof must require the exact ignored test success line"
    );
}

fn restore_fixture(root: &Path) -> std::path::PathBuf {
    let database_path = root.join("db/lorepia.sqlite3");
    fs::create_dir_all(database_path.parent().expect("database parent"))
        .expect("create database directory");
    let connection = Connection::open(&database_path).expect("create fixture database");
    connection
        .execute_batch(std::str::from_utf8(SCHEMA_SQL).expect("fixture SQL is UTF-8"))
        .expect("restore frozen fixture SQL");
    drop(connection);

    let semantic_manifest = parse_json(SEMANTIC_MANIFEST, "semantic manifest");
    write_cas_object(
        root,
        semantic_manifest["content_sources"][0]["relative_path"]
            .as_str()
            .expect("source CAS relative path"),
        SOURCE_PACKAGE,
    );
    write_cas_object(
        root,
        semantic_manifest["assets"][0]["relative_path"]
            .as_str()
            .expect("asset CAS relative path"),
        AVATAR_ASSET,
    );
    database_path
}

fn validate_schema(connection: &Connection, provenance: &Value) {
    let expected_versions = provenance["schema"]["application_registry_versions"]
        .as_array()
        .expect("provenance registry versions")
        .iter()
        .map(|version| version.as_u64().expect("numeric registry version"))
        .collect::<Vec<_>>();
    let mut statement = connection
        .prepare("SELECT version FROM schema_migrations ORDER BY version")
        .expect("prepare registry query");
    let actual_versions = statement
        .query_map([], |row| row.get::<_, u64>(0))
        .expect("query schema registry")
        .collect::<Result<Vec<_>, _>>()
        .expect("read schema registry");
    assert_eq!(actual_versions, expected_versions);

    let user_version = connection
        .query_row("PRAGMA user_version", [], |row| row.get::<_, u64>(0))
        .expect("read user_version");
    assert_eq!(
        user_version,
        provenance["schema"]["pragma_user_version"]
            .as_u64()
            .expect("provenance user_version")
    );
    let integrity = connection
        .query_row("PRAGMA integrity_check", [], |row| row.get::<_, String>(0))
        .expect("run integrity check");
    assert_eq!(
        Some(integrity.as_str()),
        provenance["schema"]["integrity_check"].as_str()
    );
    let mut statement = connection
        .prepare("PRAGMA foreign_key_check")
        .expect("prepare foreign-key check");
    assert_eq!(
        statement
            .query_map([], |_| Ok(()))
            .expect("run foreign-key check")
            .count(),
        0
    );

    let schema_inventory = parse_json(SCHEMA_INVENTORY, "schema inventory");
    let expected_objects = schema_inventory["objects"]
        .as_array()
        .expect("schema inventory objects")
        .iter()
        .map(|object| {
            (
                json_string(object, "type"),
                json_string(object, "name"),
                json_string(object, "table"),
            )
        })
        .collect::<Vec<_>>();
    let mut statement = connection
        .prepare(
            "SELECT type, name, tbl_name
             FROM sqlite_schema
             WHERE name NOT LIKE 'sqlite_%'
             ORDER BY type, name, tbl_name",
        )
        .expect("prepare schema inventory query");
    let actual_objects = statement
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
            ))
        })
        .expect("query schema inventory")
        .collect::<Result<Vec<_>, _>>()
        .expect("read schema inventory");
    assert_eq!(actual_objects, expected_objects);

    let expected_counts = schema_inventory["object_counts"]
        .as_object()
        .expect("schema object counts")
        .iter()
        .map(|(object_type, count)| {
            (
                object_type.clone(),
                count.as_u64().expect("numeric schema object count"),
            )
        })
        .collect::<BTreeMap<_, _>>();
    let mut actual_counts = expected_counts
        .keys()
        .map(|object_type| (object_type.clone(), 0_u64))
        .collect::<BTreeMap<_, _>>();
    for (object_type, _, _) in actual_objects {
        *actual_counts.entry(object_type).or_insert(0_u64) += 1;
    }
    assert_eq!(actual_counts, expected_counts);
}

fn validate_semantics(connection: &Connection, manifest: &Value) {
    validate_semantic_counts(connection, manifest);
    validate_character(connection, &manifest["characters"][0]);
    validate_provider_route_and_preset(connection, manifest);
    validate_conversation_and_messages(connection, manifest);
    validate_generation_and_settings(connection, manifest);
}

fn validate_semantic_counts(connection: &Connection, manifest: &Value) {
    for (manifest_key, table_name) in [
        ("characters", "characters"),
        ("conversations", "conversations"),
        ("messages", "messages"),
        ("generations", "generations"),
        ("provider_connections", "provider_connections"),
        ("model_routes", "provider_models"),
        ("generation_presets", "generation_presets"),
    ] {
        let expected = manifest["database_stats"][manifest_key]
            .as_u64()
            .expect("semantic database count");
        let actual = connection
            .query_row(&format!("SELECT COUNT(*) FROM {table_name}"), [], |row| {
                row.get::<_, u64>(0)
            })
            .expect("count semantic rows");
        assert_eq!(actual, expected, "semantic row count: {manifest_key}");
    }
}

fn validate_character(connection: &Connection, character: &Value) {
    let actual_character = connection
        .query_row(
            "SELECT id, name, description, source_hash, avatar_asset_hash
             FROM characters",
            [],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, String>(4)?,
                ))
            },
        )
        .expect("read fixture character");
    assert_eq!(
        actual_character,
        (
            json_string(character, "id"),
            json_string(character, "name"),
            json_string(character, "description"),
            json_string(character, "source_hash"),
            json_string(character, "avatar_asset_hash"),
        )
    );
}

fn validate_provider_route_and_preset(connection: &Connection, manifest: &Value) {
    let connection_manifest = &manifest["provider_connections"][0];
    let actual_connection = connection
        .query_row(
            "SELECT id, template_id, api_origin, credential_ref
             FROM provider_connections",
            [],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                ))
            },
        )
        .expect("read fixture provider connection");
    assert_eq!(
        actual_connection,
        (
            json_string(connection_manifest, "id"),
            json_string(connection_manifest, "template_id"),
            json_string(connection_manifest, "api_origin"),
            json_string(connection_manifest, "credential_ref"),
        )
    );

    let route = &manifest["model_routes"][0];
    let actual_route = connection
        .query_row(
            "SELECT id, connection_id, api_family, model_id FROM provider_models",
            [],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                ))
            },
        )
        .expect("read fixture model route");
    assert_eq!(
        actual_route,
        (
            json_string(route, "id"),
            json_string(route, "connection_id"),
            json_string(route, "api_family"),
            json_string(route, "model_id"),
        )
    );

    let preset = &manifest["generation_presets"][0];
    let actual_preset = connection
        .query_row(
            "SELECT id, model_route_id, display_name FROM generation_presets",
            [],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                ))
            },
        )
        .expect("read fixture generation preset");
    assert_eq!(
        actual_preset,
        (
            json_string(preset, "id"),
            json_string(preset, "model_route_id"),
            json_string(preset, "display_name"),
        )
    );
}

fn validate_conversation_and_messages(connection: &Connection, manifest: &Value) {
    let conversation = &manifest["conversations"][0];
    let actual_conversation = connection
        .query_row(
            "SELECT id, character_id, title FROM conversations",
            [],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                ))
            },
        )
        .expect("read fixture conversation");
    assert_eq!(
        actual_conversation,
        (
            json_string(conversation, "id"),
            json_string(conversation, "character_id"),
            json_string(conversation, "title"),
        )
    );

    let expected_messages = manifest["messages"]
        .as_array()
        .expect("semantic messages")
        .iter()
        .map(|message| {
            (
                json_string(message, "id"),
                json_string(message, "role"),
                json_string(message, "content"),
            )
        })
        .collect::<Vec<_>>();
    let mut statement = connection
        .prepare("SELECT id, role, content FROM messages ORDER BY created_at")
        .expect("prepare fixture message query");
    let actual_messages = statement
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
            ))
        })
        .expect("query fixture messages")
        .collect::<Result<Vec<_>, _>>()
        .expect("read fixture messages");
    assert_eq!(actual_messages, expected_messages);
}

fn validate_generation_and_settings(connection: &Connection, manifest: &Value) {
    let generation = &manifest["generations"][0];
    let actual_generation = connection
        .query_row(
            "SELECT id, conversation_id, status, input_tokens, output_tokens,
                    model_route_id, generation_preset_id
             FROM generations",
            [],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, u64>(3)?,
                    row.get::<_, u64>(4)?,
                    row.get::<_, String>(5)?,
                    row.get::<_, String>(6)?,
                ))
            },
        )
        .expect("read fixture generation");
    assert_eq!(
        actual_generation,
        (
            json_string(generation, "id"),
            json_string(generation, "conversation_id"),
            json_string(generation, "status"),
            generation["input_tokens"].as_u64().expect("input tokens"),
            generation["output_tokens"].as_u64().expect("output tokens"),
            json_string(generation, "model_route_id"),
            json_string(generation, "generation_preset_id"),
        )
    );

    let application_settings = connection
        .query_row(
            "SELECT value_json FROM app_settings WHERE key = 'application'",
            [],
            |row| row.get::<_, String>(0),
        )
        .expect("read application settings");
    assert_eq!(
        parse_json(application_settings.as_bytes(), "application settings"),
        manifest["app_settings"][0]["value"]
    );
}

fn validate_cas(root: &Path, connection: &Connection, manifest: &Value) {
    let source = &manifest["content_sources"][0];
    let asset = &manifest["assets"][0];
    for (entry, bytes) in [(source, SOURCE_PACKAGE), (asset, AVATAR_ASSET)] {
        let relative_path = json_string(entry, "relative_path");
        let expected_sha256 = json_string(entry, "sha256");
        let actual_bytes = fs::read(root.join(&relative_path)).expect("read restored CAS object");
        assert_eq!(actual_bytes, bytes);
        assert_eq!(sha256(&actual_bytes), expected_sha256);
        assert_eq!(
            u64::try_from(actual_bytes.len()).expect("CAS object size fits u64"),
            entry["size_bytes"].as_u64().expect("CAS object size")
        );
    }

    let database_source = connection
        .query_row(
            "SELECT sha256, relative_path, size_bytes FROM content_sources",
            [],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, u64>(2)?,
                ))
            },
        )
        .expect("read content source CAS row");
    assert_eq!(
        database_source,
        (
            json_string(source, "sha256"),
            json_string(source, "relative_path"),
            source["size_bytes"].as_u64().expect("source size"),
        )
    );
    let database_asset = connection
        .query_row(
            "SELECT sha256, relative_path, media_type, size_bytes FROM assets",
            [],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, u64>(3)?,
                ))
            },
        )
        .expect("read asset CAS row");
    assert_eq!(
        database_asset,
        (
            json_string(asset, "sha256"),
            json_string(asset, "relative_path"),
            json_string(asset, "media_type"),
            asset["size_bytes"].as_u64().expect("asset size"),
        )
    );
}

fn write_cas_object(root: &Path, relative_path: &str, bytes: &[u8]) {
    let path = root.join(relative_path);
    fs::create_dir_all(path.parent().expect("CAS object parent"))
        .expect("create CAS object directory");
    fs::write(path, bytes).expect("write CAS object");
}

fn parse_json(bytes: &[u8], label: &str) -> Value {
    serde_json::from_slice(bytes).unwrap_or_else(|error| panic!("parse {label}: {error}"))
}

fn json_string(value: &Value, field: &str) -> String {
    value[field]
        .as_str()
        .unwrap_or_else(|| panic!("missing string field {field}"))
        .to_owned()
}

fn sha256(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}
