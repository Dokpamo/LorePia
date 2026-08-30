use std::sync::{
    Mutex,
    atomic::{AtomicBool, Ordering},
};
use std::{
    io::Read,
    net::{IpAddr, TcpListener, TcpStream},
    sync::mpsc as std_mpsc,
    thread,
};

use lorepia_domain::{
    EndpointPath, GenerationUsage, ModelAvailability, ModelMetadataSource, ModelRouteConfig,
    ProviderCapabilities, ProviderConnectionDraft, ProviderProfile,
};
use lorepia_providers::setup_assistant::{
    AssistantManifestDraft, AssistantTurn, ConfidenceLevel, FieldConfidence, FieldEvidenceMapping,
};
use lorepia_storage::{ProviderCredentialObservedStatus, ProviderCredentialOperationKind};
use serde_json::json;
use tempfile::tempdir;

use super::*;

#[path = "../schema_fixture.rs"]
mod schema_fixture;

fn credential_commit_confirmation(
    context: &ProviderDiscoveryCredentialInstallContext,
) -> ProviderDiscoveryCredentialCommitConfirmation {
    ProviderDiscoveryCredentialCommitConfirmation::try_from(context)
        .expect("started credential install context has a physical execution authority")
}

fn reserve_credential_install(
    core: &crate::Core,
    prepared: &ProviderDiscoveryCredentialInstallContext,
) -> ProviderDiscoveryCredentialInstallContext {
    let reserved = core
        .reserve_provider_discovery_credential_install(
            &prepared.session_id,
            prepared.session_revision,
            &prepared.operation_id,
            &prepared.commit_attempt_id,
            &prepared.commit_plan_sha256,
        )
        .expect("reserve exact physical credential execution");
    assert_eq!(
        reserved.operation_status,
        DiscoveryOperationStatus::Prepared
    );
    assert!(reserved.native_execution_reservation_id.is_some());
    assert_eq!(reserved.native_execution_id, None);
    assert!(
        ProviderDiscoveryCredentialCommitConfirmation::try_from(&reserved).is_err(),
        "a reservation is not native store or commit authority"
    );
    reserved
}

fn start_reserved_credential_install(
    core: &crate::Core,
    reserved: &ProviderDiscoveryCredentialInstallContext,
) -> ProviderDiscoveryCredentialInstallContext {
    let reservation_id = reserved
        .native_execution_reservation_id
        .as_deref()
        .expect("reserved physical credential execution");
    let started = core
        .start_provider_discovery_credential_install(
            &reserved.session_id,
            reserved.session_revision,
            &reserved.operation_id,
            &reserved.commit_attempt_id,
            &reserved.commit_plan_sha256,
            reservation_id,
        )
        .expect("start exact reserved physical credential execution");
    assert_eq!(started.operation_status, DiscoveryOperationStatus::Started);
    assert_eq!(
        started.native_execution_reservation_id.as_deref(),
        started.native_execution_id.as_deref()
    );
    assert!(started.native_execution_id.is_some());
    started
}

fn reserve_and_start_credential_install(
    core: &crate::Core,
    prepared: &ProviderDiscoveryCredentialInstallContext,
) -> ProviderDiscoveryCredentialInstallContext {
    let reserved = reserve_credential_install(core, prepared);
    start_reserved_credential_install(core, &reserved)
}

fn native_execution_id(context: &ProviderDiscoveryCredentialInstallContext) -> &str {
    context
        .native_execution_id
        .as_deref()
        .expect("started credential install has native execution authority")
}

fn active_test_database_path(root: &std::path::Path) -> std::path::PathBuf {
    let cutover = root.join("db/schema-cutover");
    let (_, relative) = std::fs::read_dir(cutover)
        .expect("read committed database generations")
        .filter_map(Result::ok)
        .filter(|entry| entry.path().join("generation-committed.json").is_file())
        .map(|entry| {
            let manifest = serde_json::from_slice::<serde_json::Value>(
                &std::fs::read(entry.path().join("generation-manifest.json"))
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

fn open_core_after_drop(
    data_root: &std::path::Path,
    recovery_owner: crate::DiscoveryRecoveryOwner,
) -> crate::Core {
    let deadline = std::time::Instant::now() + Duration::from_secs(2);
    loop {
        match crate::Core::open_with_discovery_recovery_owner(
            crate::CoreConfig::new(data_root),
            recovery_owner,
        ) {
            Ok(core) => return core,
            Err(error)
                if error.code == CoreErrorCode::StorageUnavailable
                    && error.message == "data root is already owned by another LorePia process"
                    && std::time::Instant::now() < deadline =>
            {
                thread::sleep(Duration::from_millis(10));
            }
            Err(error) => panic!("open Core after prior owner drop: {error:?}"),
        }
    }
}

fn checkpoint_test_database(database: &std::path::Path) {
    let connection = rusqlite::Connection::open(database).expect("open test database");
    let _: (i64, i64, i64) = connection
        .query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |row| {
            Ok((row.get(0)?, row.get(1)?, row.get(2)?))
        })
        .expect("checkpoint test database");
}

fn restore_test_database(database: &std::path::Path, backup: &std::path::Path) {
    for sidecar in [
        database.with_extension("sqlite3-wal"),
        database.with_extension("sqlite3-shm"),
    ] {
        if sidecar.exists() {
            std::fs::remove_file(sidecar).expect("remove rolled-forward SQLite sidecar");
        }
    }
    std::fs::copy(backup, database).expect("restore prepared test database");
}

fn exact_openrouter_listed_model() -> lorepia_providers::ListedModel {
    lorepia_providers::ListedModel {
        model_id: "openai/exact-persisted-model".to_owned(),
        display_name: Some("Exact persisted model".to_owned()),
        max_input_tokens: Some(128_000),
        max_output_tokens: Some(16_384),
        supported_generation_methods: Vec::new(),
        capabilities: lorepia_providers::ListedModelCapabilities {
            supported: vec![
                lorepia_providers::ListedModelCapability::Reasoning,
                lorepia_providers::ListedModelCapability::ToolCalling,
                lorepia_providers::ListedModelCapability::ParallelToolCalling,
                lorepia_providers::ListedModelCapability::StructuredOutput,
                lorepia_providers::ListedModelCapability::JsonMode,
                lorepia_providers::ListedModelCapability::Logprobs,
                lorepia_providers::ListedModelCapability::Seed,
            ],
            parameters: lorepia_providers::OpenRouterSupportedParameterSupport::Exact(vec![
                lorepia_providers::OpenRouterSupportedParameter::Logprobs,
                lorepia_providers::OpenRouterSupportedParameter::MaxCompletionTokens,
                lorepia_providers::OpenRouterSupportedParameter::MaxTokens,
                lorepia_providers::OpenRouterSupportedParameter::ParallelToolCalls,
                lorepia_providers::OpenRouterSupportedParameter::Reasoning,
                lorepia_providers::OpenRouterSupportedParameter::ResponseFormat,
                lorepia_providers::OpenRouterSupportedParameter::Seed,
                lorepia_providers::OpenRouterSupportedParameter::StructuredOutputs,
                lorepia_providers::OpenRouterSupportedParameter::Temperature,
                lorepia_providers::OpenRouterSupportedParameter::Tools,
                lorepia_providers::OpenRouterSupportedParameter::TopP,
            ]),
            reasoning: Some(lorepia_providers::ListedModelReasoningCapability {
                supported_efforts: lorepia_providers::OpenRouterReasoningEffortSupport::Exact(
                    vec![
                        lorepia_providers::OpenRouterReasoningEffort::High,
                        lorepia_providers::OpenRouterReasoningEffort::Low,
                    ],
                ),
                default_effort: Some(lorepia_providers::OpenRouterReasoningEffort::High),
                default_enabled: Some(true),
                supports_max_tokens: Some(true),
                mandatory: Some(false),
            }),
        },
        source: lorepia_providers::ModelRecordSource::ProviderApi,
        availability: ModelAvailability::Available,
    }
}

fn approve_credential_and_seed_model_listing(
    core: &crate::Core,
    snapshot: &DiscoverySessionSnapshot,
    approval_id: DiscoveryApprovalId,
    listed_models: &[lorepia_providers::ListedModel],
) -> DiscoverySessionSnapshot {
    let orchestrator = core.provider_discovery();
    let envelope = provider_discovery_action_envelope(
        DiscoveryActionId::new(),
        snapshot.session.revision,
        ProviderDiscoveryAction::ApproveCredentialOrigin { approval_id },
    )
    .expect("approve-credential action");
    let mut draft = hydrate_working_draft(snapshot).expect("hydrate credential draft");
    let occurred_at = Utc::now();
    let (approval, review, prepared_commit) = orchestrator
        .prepare_user_action(snapshot, &envelope, &mut draft, occurred_at)
        .expect("prepare credential approval");
    let transition = snapshot
        .session
        .apply(&envelope)
        .expect("apply credential approval");
    let new_operation_id =
        operation_for_effect(&transition.effect).map(|_| DiscoveryOperationId::new());
    orchestrator
        .storage
        .persist_discovery_transition(&DiscoveryTransitionWrite {
            transition,
            draft: DiscoveryJsonUpdate::Replace(
                working_draft_value(&draft).expect("serialize credential-approved draft"),
            ),
            review,
            new_evidence: Vec::new(),
            new_candidates: Vec::new(),
            approval,
            new_operation_id,
            completed_operation: None,
            prepared_commit,
            provider_graph: None,
            occurred_at,
        })
        .expect("persist credential approval without running network");

    let listing = orchestrator
        .get(&snapshot.session.id)
        .expect("load model-list operation");
    assert_eq!(listing.session.state, DiscoveryState::ListingModels);
    let operation = orchestrator
        .storage
        .get_current_discovery_operation(&snapshot.session.id)
        .expect("load current model-list operation")
        .expect("model-list operation");
    assert_eq!(operation.kind, DiscoveryOperationKind::ListModels);
    assert!(
        orchestrator
            .storage
            .mark_discovery_operation_started(&operation.id, Utc::now())
            .expect("start model-list operation"),
        "prepared model-list operation must start exactly once"
    );
    let mut draft = hydrate_working_draft(&listing).expect("hydrate model-list draft");
    apply_listed_models_to_draft(&mut draft, listed_models, Utc::now())
        .expect("apply canonical normalized OpenRouter listing");
    draft.probe_route_ids = draft.routes.iter().map(|route| route.id.clone()).collect();
    let model_count = u32::try_from(draft.routes.len()).expect("bounded model count");
    let candidates = model_candidates(&listing, &draft).expect("build model candidates");
    orchestrator
        .persist_operation_completion(
            &listing,
            &operation.id,
            &mut draft,
            ProviderDiscoveryAction::ModelsListed {
                model_count,
                probe_candidate_count: model_count,
            },
            DurableOperationOutcome::Succeeded,
            Vec::new(),
            candidates,
            DiscoveryJsonUpdate::Preserve,
        )
        .expect("persist normalized model-list completion");
    orchestrator
        .get(&snapshot.session.id)
        .expect("load seeded model-list result")
}

fn prepare_no_network_credential_commit(
    core: &crate::Core,
    connection_id: &str,
) -> DiscoverySessionSnapshot {
    let template = AdapterRegistry::built_in_template(BuiltInTemplateId::OpenRouter)
        .expect("OpenRouter template");
    let connection_id = ProviderConnectionId::from(connection_id);
    let selecting = core
        .begin_provider_discovery_known(
            SanitizedDiscoveryInput {
                connection_id: connection_id.clone(),
                display_name: "No-network recovery provider".to_owned(),
                site_url: HttpUrl::parse("https://openrouter.ai/").expect("OpenRouter site URL"),
                docs_url: None,
                credential_ref: Some(CredentialRef(connection_id.as_str().to_owned())),
                preferred_assistant: None,
                connection_options: ProviderDiscoveryConnectionOptions::default(),
                supplied_evidence_ids: Vec::new(),
            },
            template.id.clone(),
        )
        .expect("begin no-network provider discovery");
    finish_no_network_credential_commit(core, &template, &selecting)
}

include!("credential_dispatch.rs");
include!("credential_commit.rs");
include!("credential_recovery.rs");
include!("assistant_restart.rs");
include!("network_integrity.rs");
