use std::{
    collections::BTreeSet,
    fmt::Write as _,
    fs,
    path::{Path, PathBuf},
};

use serde_json::Value;
use sha2::{Digest, Sha256};

const PROMPT_PLATFORM_GOLDEN: &str =
    include_str!("contract/prompt-orchestration-platform-golden.json");
const PROMPT_PLATFORM_GOLDEN_SHA256: &str =
    include_str!("contract/prompt-orchestration-platform-golden.sha256");
const RECOVERY_COMPATIBILITY_CONTRACT: &str =
    include_str!("contract/recovery-compatibility-v1.json");
const RESOLVED_PROMPT_PLAN_GOLDEN_SHA256: &str = include_str!(
    "../../../crates/orchestration/tests/fixtures/cross_platform_resolved_prompt_plan.sha256"
);
const PROMPT_PLATFORM_CONFIGS: [(&str, &str); 4] = [
    ("android", "tauri.android.conf.json"),
    ("ios", "tauri.ios.conf.json"),
    ("macos", "tauri.macos.conf.json"),
    ("windows", "tauri.windows.conf.json"),
];
const PROMPT_SHARED_CONFIG: &str = "tauri.conf.json";
const PROMPT_RELEASE_CONFIG: &str = "tauri.release.conf.json";
const PROMPT_PROJECTION_OWNER: &str = "lorepia-shell-api";
const WINDOWS_COMMON_CONTROLS_MANIFEST: &str = r#"<assembly xmlns="urn:schemas-microsoft-com:asm.v1" manifestVersion="1.0">
  <dependency>
    <dependentAssembly>
      <assemblyIdentity
        type="win32"
        name="Microsoft.Windows.Common-Controls"
        version="6.0.0.0"
        processorArchitecture="*"
        publicKeyToken="6595b64144ccf1df"
        language="*"
      />
    </dependentAssembly>
  </dependency>
</assembly>
"#;
const PROMPT_COMMAND_CONTRACTS: [(&str, &str, &str, &str, &str); 3] = [
    (
        "resolve_prompt_preview",
        "ResolvePromptPreviewInput",
        "ExpertPromptPreviewDto",
        "async",
        "AppHandle+State<AppState>",
    ),
    (
        "send_reviewed_prompt",
        "ReviewedPromptSendInput",
        "GenerationStartedDto",
        "async",
        "AppHandle+State<AppState>",
    ),
    (
        "explain_prompt_plan",
        "ExplainPromptPlanInput",
        "PromptResolutionTraceDto",
        "sync",
        "State<AppState>",
    ),
];

const APP_COMMANDS: &[&str] = &[
    "bootstrap",
    "get_memory_supervisor_status",
    "list_characters",
    "get_character",
    "get_character_greeting_catalog",
    "resolve_asset_delivery",
    "pick_content_package_import",
    "list_completed_content_package_exports",
    "export_content_source",
    "reopen_content_package_import",
    "list_pending_content_package_imports",
    "select_content_package_import",
    "approve_content_package_import",
    "commit_content_package_import",
    "discard_content_package_import",
    "pick_import",
    "inspect_import",
    "commit_import",
    "discard_import",
    "create_conversation",
    "open_conversation",
    "open_existing_conversation",
    "list_conversations",
    "list_conversations_for_character",
    "get_conversation",
    "get_conversation_state",
    "list_branches",
    "create_branch",
    "select_branch",
    "set_conversation_mode",
    "create_persona",
    "update_persona",
    "get_persona",
    "list_personas",
    "list_persona_page",
    "delete_persona",
    "get_conversation_persona_selection",
    "select_conversation_persona",
    "clear_conversation_persona",
    "list_branch_messages",
    "list_messages",
    "send_message",
    "edit_user_message",
    "regenerate_assistant_message",
    "remove_message_from_branch",
    "cancel_generation",
    "subscribe_generation",
    "dispose_chat_stream",
    "get_provider_overview",
    "list_model_routes",
    "list_generation_presets",
    "preview_provider_request",
    "credential_status",
    "capture_credential",
    "delete_credential",
    "get_settings",
    "update_settings",
    "select_generation_target",
    "list_provider_templates",
    "list_provider_connections",
    "create_provider_connection",
    "upsert_provider_connection",
    "delete_provider_connection",
    "list_provider_profiles",
    "upsert_model_route",
    "delete_model_route",
    "list_capability_observations",
    "effective_capability",
    "effective_parameter_specs",
    "upsert_user_capability_override",
    "delete_user_capability_override",
    "upsert_generation_preset",
    "delete_generation_preset",
    "validate_generation_preset_candidate",
    "render_reasoning_control_for_preset",
    "render_prompt_cache_control_for_preset",
    "preview_provider_request_candidate",
    "start_provider_model_sync",
    "get_provider_model_sync",
    "list_provider_model_syncs",
    "approve_provider_model_sync",
    "cancel_provider_model_sync",
    "poll_provider_model_sync_events",
    "ack_provider_model_sync_event",
    "begin_provider_discovery",
    "begin_provider_discovery_curl",
    "list_provider_discoveries",
    "get_provider_discovery",
    "list_provider_discovery_candidates",
    "list_provider_discovery_evidence",
    "list_provider_discovery_approvals",
    "get_provider_discovery_review",
    "get_provider_discovery_approval_proposal",
    "get_provider_discovery_review_proposal",
    "get_provider_discovery_assistant_resume_boundary",
    "run_provider_discovery_assistant_turn",
    "resume_provider_discovery_assistant_core_host_action",
    "approve_provider_discovery_assistant_retry",
    "request_provider_discovery_assistant_revision",
    "accept_provider_discovery_assistant_draft",
    "record_provider_discovery_assistant_failure",
    "interrupt_provider_discovery_assistant",
    "restart_provider_discovery_assistant_after_interruption",
    "continue_provider_discovery",
    "supply_provider_discovery_document_evidence",
    "supply_provider_discovery_curl_evidence",
    "cancel_provider_discovery",
    "commit_provider_discovery",
    "poll_provider_discovery_events",
    "poll_provider_discovery_events_for_session",
    "ack_provider_discovery_event",
    "recover_provider_discovery",
    "list_provider_discovery_compensation_steps",
    "continue_provider_discovery_compensation",
    "resume_provider_discovery_compensation",
    "pick_provider_catalog_import",
    "activate_provider_catalog_import",
    "discard_provider_catalog_import",
    "provider_catalog_status",
    "provider_catalog_history",
    "diff_provider_catalog_revisions",
    "prepare_provider_catalog_rollback",
    "activate_provider_catalog_rollback",
    "validate_prompt_preset",
    "resolve_prompt_preview",
    "send_reviewed_prompt",
    "explain_prompt_plan",
    "get_orchestration_workspace",
    "save_room_orchestration_config",
    "upsert_prompt_preset",
    "get_prompt_preset",
    "get_editable_prompt_preset",
    "list_prompt_presets",
    "list_prompt_preset_revisions",
    "diff_prompt_preset_revisions",
    "review_prompt_preset_rollback",
    "apply_prompt_preset_rollback",
    "reorder_prompt_blocks",
    "delete_prompt_preset",
    "upsert_task_profile",
    "get_task_profile",
    "list_task_profiles",
    "delete_task_profile",
    "upsert_memory_profile",
    "get_memory_profile",
    "list_memory_profiles",
    "delete_memory_profile",
    "get_memory_record",
    "patch_memory_record",
    "set_memory_record_exclusion",
    "delete_memory_record",
    "upsert_knowledge_book",
    "get_knowledge_book",
    "list_knowledge_books",
    "delete_knowledge_book",
    "upsert_transform_set",
    "get_transform_set",
    "list_transform_sets",
    "delete_transform_set",
    "upsert_interaction_rule_set",
    "get_interaction_rule_set",
    "list_interaction_rule_sets",
    "delete_interaction_rule_set",
    "list_interaction_effects",
    "list_interaction_proposals",
    "list_generation_attempt_proposals",
    "list_retryable_generation_attempts",
    "expire_interaction_proposals",
    "expire_generation_attempt_proposals",
    "list_interaction_effect_history",
    "list_reopen_interaction_effects",
    "submit_interaction_choice",
    "acknowledge_interaction_effect",
    "retry_interaction_effect",
    "decide_interaction_proposal",
    "decide_generation_attempt_proposal",
    "upsert_content_module",
    "get_content_module",
    "list_content_modules",
    "delete_content_module",
    "list_prompt_preset_bindings",
    "list_memory_records",
    "retry_interrupted_memory_job",
    "list_retryable_memory_query_embeddings",
    "retry_memory_query_embedding",
    "simulate_knowledge_activation",
    "preview_transform_rule",
    "list_content_module_bindings",
    "list_content_module_revisions",
    "diff_content_module_revisions",
    "evaluate_content_module_share",
    "list_content_module_lifecycle_candidates",
    "list_content_module_lifecycle_bindings",
    "review_content_module_activation",
    "resolve_content_module_activation",
    "activate_content_module",
    "review_content_module_deactivation",
    "deactivate_content_module",
    "review_content_module_rollback",
    "resolve_content_module_rollback",
    "apply_content_module_rollback",
];

fn main() {
    validate_prompt_platform_contract();
    validate_recovery_compatibility_contract();
    let is_windows_msvc = is_windows_msvc_target();
    if is_windows_msvc {
        embed_common_controls_manifest_for_windows_targets();
    }
    let app_manifest = tauri_build::AppManifest::new().commands(APP_COMMANDS);
    let mut attributes = tauri_build::Attributes::new().app_manifest(app_manifest);
    if is_windows_msvc {
        attributes = attributes
            .windows_attributes(tauri_build::WindowsAttributes::new_without_app_manifest());
    }

    tauri_build::try_build(attributes).expect("failed to build the LorePia Tauri application");
    validate_registered_command_artifacts(Path::new(env!("CARGO_MANIFEST_DIR")));
}

fn validate_recovery_compatibility_contract() {
    let contract: Value = serde_json::from_str(RECOVERY_COMPATIBILITY_CONTRACT)
        .expect("recovery compatibility contract must be JSON");
    validate_recovery_contract_versions(&contract);
    println!("cargo:rerun-if-changed=contract/recovery-compatibility-v1.json");
}

fn validate_recovery_contract_versions(contract: &Value) {
    assert_eq!(contract["schema_version"].as_u64(), Some(1));
    assert_eq!(
        contract["artifact_kind"].as_str(),
        Some("tauri-compatible-recovery")
    );
    assert_eq!(contract["product_name"].as_str(), Some("LorePia"));
    assert_eq!(contract["build_number"]["minimum"].as_u64(), Some(3));
    assert_eq!(contract["build_number"]["maximum"].as_u64(), Some(65_535));
    assert_eq!(
        contract["compatibility"]["storage_schema_version"].as_u64(),
        Some(37)
    );
    assert_eq!(
        contract["compatibility"]["canonical_native_schema_version"].as_u64(),
        Some(11)
    );
    assert_eq!(
        contract["compatibility"]["cutover_manifest"]["read_versions"],
        serde_json::json!([2, 3])
    );
    assert_eq!(
        contract["compatibility"]["cutover_manifest"]["write_version"].as_u64(),
        Some(3)
    );
    assert_eq!(
        contract["compatibility"]["provider_credential_journal"]["schema_version"].as_u64(),
        Some(1)
    );
    assert_eq!(
        contract["compatibility"]["provider_credential_journal"]["redaction_version"].as_u64(),
        Some(1)
    );
    assert_eq!(
        contract["compatibility"]["bound_credential"]["physical_reference_prefix"].as_str(),
        Some("lpc2-")
    );
    assert_eq!(
        contract["compatibility"]["bound_credential"]["physical_reference_version"].as_u64(),
        Some(2)
    );
    assert_eq!(
        contract["compatibility"]["bound_credential"]["envelope_version"].as_u64(),
        Some(1)
    );
    assert_eq!(
        contract["toolchain"]["tauri_cli_version"].as_str(),
        Some("2.11.4")
    );
}

fn is_windows_msvc_target() -> bool {
    std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows")
        && std::env::var("CARGO_CFG_TARGET_ENV").as_deref() == Ok("msvc")
}

/// Link the same v6 manifest into the lib test harness and the application binary.
/// Cargo's `rustc-link-arg-tests` directive covers integration tests, not lib unit tests.
fn embed_common_controls_manifest_for_windows_targets() {
    let out_dir = PathBuf::from(std::env::var_os("OUT_DIR").expect("Cargo must provide OUT_DIR"));
    let manifest = out_dir.join("windows-common-controls-v6.manifest");
    fs::write(&manifest, WINDOWS_COMMON_CONTROLS_MANIFEST)
        .expect("write the Windows Common Controls v6 manifest");
    println!("cargo:rustc-link-arg=/MANIFEST:EMBED");
    println!("cargo:rustc-link-arg=/MANIFESTINPUT:{}", manifest.display());
}

/// Keep the four command-authority surfaces identical.
///
/// `tauri_build` runs first so a newly registered command can materialize its
/// generated permission before this exact-set check. It does not remove stale
/// permissions, so the check also prevents a retired high-level command from
/// remaining grantable by an old generated file.
fn validate_registered_command_artifacts(manifest_dir: &Path) {
    let expected = APP_COMMANDS.iter().copied().collect::<BTreeSet<_>>();
    assert_eq!(
        expected.len(),
        APP_COMMANDS.len(),
        "the Tauri app command manifest cannot contain duplicate commands"
    );

    validate_invoke_handler(&expected);
    validate_command_capabilities(manifest_dir, &expected);
    validate_generated_permissions(manifest_dir, &expected);
}

fn validate_invoke_handler(expected: &BTreeSet<&str>) {
    let source = include_str!("src/lib.rs");
    let handler = source
        .split_once("tauri::generate_handler![")
        .and_then(|(_, suffix)| suffix.split_once("])"))
        .map(|(handler, _)| handler)
        .expect("src/lib.rs must contain one Tauri invoke handler");
    let commands = handler
        .lines()
        .filter_map(|line| line.trim().strip_suffix(','))
        .map(|entry| {
            entry
                .rsplit_once("::")
                .map(|(_, command)| command)
                .expect("every invoke handler entry must be module-qualified")
        })
        .collect::<Vec<_>>();
    let actual = commands.iter().copied().collect::<BTreeSet<_>>();
    assert_eq!(
        actual.len(),
        commands.len(),
        "the Tauri invoke handler cannot contain duplicate commands"
    );
    assert_eq!(
        &actual, expected,
        "the Tauri invoke handler and app command manifest must match exactly"
    );
}

fn validate_command_capabilities(manifest_dir: &Path, expected: &BTreeSet<&str>) {
    let expected = expected
        .iter()
        .map(|command| format!("allow-{}", command.replace('_', "-")))
        .collect::<BTreeSet<_>>();
    for capability in [
        "capabilities/main-development.json",
        "capabilities/main-release.json",
    ] {
        println!("cargo:rerun-if-changed={capability}");
        let value: Value = serde_json::from_str(
            &fs::read_to_string(manifest_dir.join(capability))
                .expect("shared Tauri capability must exist"),
        )
        .expect("shared Tauri capability must be valid JSON");
        let permissions = value["permissions"]
            .as_array()
            .expect("shared Tauri permissions must be an array");
        let allow_commands = permissions
            .iter()
            .filter_map(Value::as_str)
            .filter(|permission| permission.starts_with("allow-"))
            .collect::<Vec<_>>();
        let actual = allow_commands.iter().copied().collect::<BTreeSet<_>>();
        assert_eq!(
            actual.len(),
            allow_commands.len(),
            "{capability} cannot contain duplicate app-command grants"
        );
        assert_eq!(
            actual,
            expected.iter().map(String::as_str).collect(),
            "{capability} must grant exactly the registered app commands"
        );
    }
}

fn validate_generated_permissions(manifest_dir: &Path, expected: &BTreeSet<&str>) {
    let directory = manifest_dir.join("permissions/autogenerated");
    println!("cargo:rerun-if-changed={}", directory.display());
    let paths = fs::read_dir(&directory)
        .expect("autogenerated Tauri permission directory must exist")
        .map(|entry| {
            entry
                .expect("autogenerated permission entry must be readable")
                .path()
        })
        .filter(|path| {
            path.extension()
                .is_some_and(|extension| extension == "toml")
        })
        .collect::<Vec<PathBuf>>();
    let actual = paths
        .iter()
        .map(|path| {
            path.file_stem()
                .and_then(|stem| stem.to_str())
                .expect("autogenerated permission names must be UTF-8")
        })
        .collect::<BTreeSet<_>>();
    assert_eq!(
        actual.len(),
        paths.len(),
        "autogenerated Tauri permission names must be unique"
    );
    assert_eq!(
        &actual, expected,
        "autogenerated Tauri permissions must match the registered commands exactly"
    );

    for path in paths {
        let command = path
            .file_stem()
            .and_then(|stem| stem.to_str())
            .expect("autogenerated permission names must be UTF-8");
        let value =
            fs::read_to_string(&path).expect("autogenerated Tauri permission must be readable");
        assert_eq!(
            value,
            expected_generated_permission(command),
            "{} must keep the canonical single-command allow/deny structure",
            path.display(),
        );
    }
}

fn expected_generated_permission(command: &str) -> String {
    let identifier = command.replace('_', "-");
    format!(
        "# Automatically generated - DO NOT EDIT!\n\n\
[[permission]]\n\
identifier = \"allow-{identifier}\"\n\
description = \"Enables the {command} command without any pre-configured scope.\"\n\
commands.allow = [\"{command}\"]\n\n\
[[permission]]\n\
identifier = \"deny-{identifier}\"\n\
description = \"Denies the {command} command without any pre-configured scope.\"\n\
commands.deny = [\"{command}\"]\n"
    )
}

fn validate_prompt_platform_contract() {
    let actual_sha256 = sha256_hex(PROMPT_PLATFORM_GOLDEN.as_bytes());
    assert_eq!(
        actual_sha256,
        PROMPT_PLATFORM_GOLDEN_SHA256.trim(),
        "prompt orchestration platform golden changed without an explicit hash review"
    );
    let golden: Value =
        serde_json::from_str(PROMPT_PLATFORM_GOLDEN).expect("prompt platform golden must be JSON");
    assert_eq!(
        golden["projection_owner"], PROMPT_PROJECTION_OWNER,
        "all native targets must consume the one Shell projection"
    );
    assert_eq!(
        golden["resolved_prompt_plan_golden_sha256"],
        RESOLVED_PROMPT_PLAN_GOLDEN_SHA256.trim(),
        "Tauri and orchestration resolved-plan goldens must stay bound"
    );
    validate_prompt_command_contracts(&golden);

    let manifest_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    validate_prompt_platform_configs(&golden, manifest_dir);
    validate_prompt_capabilities(manifest_dir);
    write_prompt_platform_contract(&actual_sha256);
}

fn validate_prompt_command_contracts(golden: &Value) {
    let command_contracts = golden["commands"]
        .as_array()
        .expect("prompt platform commands must be an array")
        .iter()
        .map(|command| {
            (
                command["name"]
                    .as_str()
                    .expect("prompt command name must be a string"),
                command["request_type"]
                    .as_str()
                    .expect("prompt request type must be a string"),
                command["response_type"]
                    .as_str()
                    .expect("prompt response type must be a string"),
                command["execution"]
                    .as_str()
                    .expect("prompt command execution must be a string"),
                command["native_context"]
                    .as_str()
                    .expect("prompt command native context must be a string"),
            )
        })
        .collect::<Vec<_>>();
    assert_eq!(
        command_contracts.as_slice(),
        PROMPT_COMMAND_CONTRACTS.as_slice(),
        "prompt commands must keep the reviewed Shell request and response types"
    );
    for (command, _, _, _, _) in PROMPT_COMMAND_CONTRACTS {
        assert!(
            APP_COMMANDS.contains(&command),
            "{command} must be registered for every target"
        );
    }
}

fn validate_prompt_platform_configs(golden: &Value, manifest_dir: &Path) {
    validate_prompt_config(
        &golden["shared_config"],
        PROMPT_SHARED_CONFIG,
        manifest_dir,
        "shared",
    );
    validate_prompt_config(
        &golden["release_config"],
        PROMPT_RELEASE_CONFIG,
        manifest_dir,
        "release",
    );

    let platforms = golden["platforms"]
        .as_array()
        .expect("prompt platform list must be an array");
    assert_eq!(platforms.len(), PROMPT_PLATFORM_CONFIGS.len());
    for ((expected_platform, expected_config), platform) in
        PROMPT_PLATFORM_CONFIGS.iter().zip(platforms)
    {
        assert_eq!(platform["platform"], *expected_platform);
        validate_prompt_config(
            &platform["target_config"],
            expected_config,
            manifest_dir,
            "platform target",
        );
    }
}

fn validate_prompt_config(
    declared_config: &Value,
    expected_config: &str,
    manifest_dir: &Path,
    config_kind: &str,
) {
    let declared_config = declared_config
        .as_str()
        .unwrap_or_else(|| panic!("prompt {config_kind} config path must be a string"));
    assert_eq!(
        declared_config, expected_config,
        "prompt {config_kind} config path must stay exact"
    );
    println!("cargo:rerun-if-changed={expected_config}");
    let config = fs::read_to_string(manifest_dir.join(expected_config))
        .unwrap_or_else(|_| panic!("prompt {config_kind} config must exist"));
    let parsed: Value = serde_json::from_str(&config)
        .unwrap_or_else(|_| panic!("prompt {config_kind} config must be valid JSON"));
    assert!(
        parsed.is_object(),
        "prompt {config_kind} config must be an object"
    );
    assert!(
        !config.contains("prompt_preview")
            && !config.contains("prompt_plan")
            && !config.contains("prompt_dto"),
        "prompt {config_kind} config cannot substitute a native prompt DTO or command path"
    );
}

fn validate_prompt_capabilities(manifest_dir: &Path) {
    for capability in [
        "capabilities/main-development.json",
        "capabilities/main-release.json",
    ] {
        let value = fs::read_to_string(manifest_dir.join(capability))
            .expect("shared Tauri capability must exist");
        for (command, _, _, _, _) in PROMPT_COMMAND_CONTRACTS {
            let permission = format!("allow-{}", command.replace('_', "-"));
            assert!(
                value.contains(&format!("\"{permission}\"")),
                "{capability} must allow the shared {command} command"
            );
        }
    }
}

fn write_prompt_platform_contract(actual_sha256: &str) {
    let out_dir = std::env::var_os("OUT_DIR").expect("Cargo must provide OUT_DIR");
    let platform_targets = PROMPT_PLATFORM_CONFIGS.map(|(platform, _)| platform);
    let command_count = PROMPT_COMMAND_CONTRACTS.len();
    let platform_count = platform_targets.len();
    let resolved_plan_sha256 = RESOLVED_PROMPT_PLAN_GOLDEN_SHA256.trim();
    let generated = format!(
        "pub const PROMPT_PLATFORM_GOLDEN_SHA256: &str = \"{actual_sha256}\";\n\
         pub const PROMPT_PROJECTION_OWNER: &str = {PROMPT_PROJECTION_OWNER:?};\n\
         pub const PROMPT_RESOLVED_PLAN_GOLDEN_SHA256: &str = {resolved_plan_sha256:?};\n\
         pub const PROMPT_SHARED_CONFIG: &str = {PROMPT_SHARED_CONFIG:?};\n\
         pub const PROMPT_RELEASE_CONFIG: &str = {PROMPT_RELEASE_CONFIG:?};\n\
         pub const PROMPT_COMMAND_CONTRACTS: [(&str, &str, &str, &str, &str); {command_count}] = {PROMPT_COMMAND_CONTRACTS:?};\n\
         pub const PROMPT_PLATFORM_TARGETS: [&str; {platform_count}] = {platform_targets:?};\n"
    );
    fs::write(
        Path::new(&out_dir).join("prompt_orchestration_platform_contract.rs"),
        generated,
    )
    .expect("write prompt platform build contract");
}

fn sha256_hex(value: &[u8]) -> String {
    let digest = Sha256::digest(value);
    let mut encoded = String::with_capacity(digest.len() * 2);
    for byte in digest {
        write!(&mut encoded, "{byte:02x}").expect("writing to a String cannot fail");
    }
    encoded
}
