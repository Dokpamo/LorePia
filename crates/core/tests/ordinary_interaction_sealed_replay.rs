//! Ordinary derived interactions replay only their durable evaluation authority.

use std::{
    io::Write,
    path::{Path, PathBuf},
    thread,
    time::{Duration, Instant},
};

use chrono::{Duration as ChronoDuration, Utc};
use lorepia_core::{
    ApiFamily, CanonicalOrigin, CapabilityKey, CapabilityObservation, CapabilityValue, Confidence,
    ConnectionConfigEntry, ConnectionConfigValue, ContentModuleActivationRequest,
    ContentModuleBindingDraft, ContentModuleRuntimeTarget, Core, CoreConfig, EndpointPath,
    GenerationPreset, GenerationPromptCacheSettings, GenerationReasoningSettings, GenerationTarget,
    ModelAvailability, ModelMetadataSource, ModelRoute, ModelRouteConfig, ModelRouteId,
    ModuleActivationApproval, ModuleMergeResolutionSet, ObservationId, ObservationSource,
    ProviderConnectionDraft, ProviderConnectionId, ProviderNetworkMode, SupportStatus,
};
use lorepia_domain::{
    BuiltInTemplateValue, ConditionExpr, ContentCapability, ContentModule, ContentModuleId,
    ConversationMode, DiceExpression, InteractionAction, InteractionEffect, InteractionEvent,
    InteractionRule, InteractionRuleId, InteractionRuleSet, InteractionRuleSetId, ModuleBindingId,
    ModuleRevisionResolutionMode, ModuleScope, PackageMetadata, Provenance, SafeTemplate,
    Sha256Digest, SourceKind, TemplatePart, ValueExpr, VariableId, VariableMap, VariableRef,
    VariableScope, VariableValue, VersionedJson,
};
use lorepia_orchestration::{
    InteractionContext, InteractionEngine, InteractionLimits, InteractionRuleStatus,
    InteractionTemplateValues,
};
use lorepia_storage::{
    InteractionActionResultStatus, InteractionActionResultWrite, InteractionDerivedEventWrite,
    InteractionEvaluationSeal, InteractionEventCommit, InteractionPolicySnapshot, Storage,
    interaction_action_sha256, interaction_policy_sha256,
};
use rusqlite::{Connection, params};
use tempfile::{NamedTempFile, tempdir};

const CONNECTION_ID: &str = "synthetic-ordinary-sealed-replay-connection";
const ROOT_ACTION_ID: &str = "synthetic-ordinary-sealed-root";
const RULE_SET_ID: &str = "synthetic.core.ordinary-sealed.rules";
const ROOT_RULE_ID: &str = "synthetic.core.ordinary-sealed.root";
const CHILD_RULE_ID: &str = "synthetic.core.ordinary-sealed.child";
const SEALED_CHARACTER_NAME: &str = "Sealed Ari";
const DRIFTED_CHARACTER_NAME: &str = "Drifted Ari";

fn active_database_path(root: &Path) -> PathBuf {
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

fn provenance(source_id: &str) -> Provenance {
    Provenance {
        source_kind: SourceKind::UserCreated,
        source_id: Some(source_id.to_owned()),
        source_hash: Some("ab".repeat(32)),
        author: Some("Synthetic ordinary sealed replay test".to_owned()),
        license: Some("LicenseRef-Synthetic-Test".to_owned()),
        imported_at: None,
    }
}

fn variable(id: &str) -> VariableRef {
    VariableRef {
        scope: VariableScope::Branch,
        namespace: None,
        id: VariableId::from(id),
    }
}

fn visible_template() -> SafeTemplate {
    SafeTemplate {
        parts: vec![
            TemplatePart::Text {
                value: "CHAR=".to_owned(),
            },
            TemplatePart::BuiltIn {
                value: BuiltInTemplateValue::CharacterName,
            },
            TemplatePart::Text {
                value: ";TIME=".to_owned(),
            },
            TemplatePart::BuiltIn {
                value: BuiltInTemplateValue::CurrentTime,
            },
        ],
        max_output_chars: 256,
    }
}

fn rule_set(trigger: &VariableRef, roll: &VariableRef) -> InteractionRuleSet {
    InteractionRuleSet {
        id: InteractionRuleSetId::from(RULE_SET_ID),
        name: "Synthetic ordinary sealed replay rules".to_owned(),
        schema_version: 1,
        rules: vec![
            InteractionRule {
                id: InteractionRuleId::from(ROOT_RULE_ID),
                name: "Enqueue one delayed variable event".to_owned(),
                enabled: true,
                imported_author_enabled: false,
                event: InteractionEvent::UserAction {
                    action_id: ROOT_ACTION_ID.to_owned(),
                },
                condition: None,
                actions: vec![InteractionAction::SetVariable {
                    target: trigger.clone(),
                    value: ValueExpr::Literal {
                        value: VariableValue::Integer(1),
                    },
                }],
                priority: 0,
                stop_after_match: false,
                provenance: provenance(ROOT_RULE_ID),
            },
            InteractionRule {
                id: InteractionRuleId::from(CHILD_RULE_ID),
                name: "Use sealed capability, template, and seed".to_owned(),
                enabled: true,
                imported_author_enabled: false,
                event: InteractionEvent::VariableChanged {
                    variable: trigger.clone(),
                },
                condition: Some(ConditionExpr::ModelSupports {
                    capability: CapabilityKey::JsonMode,
                }),
                actions: vec![
                    InteractionAction::AppendVisibleSystemEvent {
                        text: visible_template(),
                    },
                    InteractionAction::RollDice {
                        expression: DiceExpression {
                            count: 4,
                            sides: 20,
                            modifier: 3,
                        },
                        target: Some(roll.clone()),
                    },
                ],
                priority: 1,
                stop_after_match: false,
                provenance: provenance(CHILD_RULE_ID),
            },
        ],
        max_actions_per_event: 16,
        provenance: provenance(RULE_SET_ID),
    }
}

fn interaction_module(rule_set_id: &InteractionRuleSetId) -> ContentModule {
    ContentModule {
        id: ContentModuleId::from("synthetic.core.ordinary-sealed.module"),
        name: "Synthetic ordinary sealed replay module".to_owned(),
        version: "1.0.0".to_owned(),
        schema_version: 1,
        prompt_fragments: Vec::new(),
        knowledge_book_ids: Vec::new(),
        control_specs: Vec::new(),
        transform_set_ids: Vec::new(),
        interaction_rule_set_ids: vec![rule_set_id.clone()],
        asset_ids: Vec::new(),
        imported_components_enabled: true,
        required_capabilities: vec![ContentCapability::DeclarativeInteractions],
        metadata: PackageMetadata {
            author: Some("Synthetic ordinary sealed replay test".to_owned()),
            license: "LicenseRef-Unknown".to_owned(),
            redistribution_allowed: false,
            homepage: None,
            description: "Local-only sealed replay fixture".to_owned(),
            tags: vec!["synthetic".to_owned()],
            provenance: provenance("synthetic.core.ordinary-sealed.module"),
        },
    }
}

fn import_character(core: &Core) -> String {
    let mut source = NamedTempFile::new().expect("create synthetic character source");
    write!(
        source,
        r#"{{"spec":"chara_card_v3","data":{{"name":"{SEALED_CHARACTER_NAME}","description":"Entirely synthetic ordinary sealed replay character."}}}}"#
    )
    .expect("write synthetic character source");
    let review = core
        .inspect_import(source.path())
        .expect("inspect synthetic character");
    core.commit_import(&review.id)
        .expect("commit synthetic character")
        .id
}

fn provider_target(core: &Core) -> GenerationTarget {
    let origin =
        CanonicalOrigin::parse("http://127.0.0.1:9").expect("parse unused local provider origin");
    let template = core
        .list_provider_templates()
        .expect("list provider templates")
        .into_iter()
        .find(|template| template.id.as_str() == "openai-chat-compatible-v1")
        .expect("OpenAI-compatible provider template");
    let connection = core
        .create_provider_connection(ProviderConnectionDraft {
            id: ProviderConnectionId::from(CONNECTION_ID),
            template_id: template.id,
            template_version: template.manifest_version,
            display_name: "Synthetic ordinary sealed provider".to_owned(),
            api_origin: origin.clone(),
            api_base_path: Some(EndpointPath::parse("/v1").expect("parse provider API path")),
            network_mode: ProviderNetworkMode::LocalLoopback,
            local_network_approval: None,
            values: vec![ConnectionConfigEntry {
                key: "api_base_url".to_owned(),
                value: ConnectionConfigValue::Text(format!("{}/v1", origin.as_str())),
            }],
            approved_credential_origin: Some(origin),
            timeout_seconds: 5,
        })
        .expect("create synthetic provider connection");
    let now = Utc::now();
    let route = core
        .upsert_model_route(ModelRoute {
            id: ModelRouteId::from("synthetic-ordinary-sealed-route"),
            connection_id: connection.id,
            api_family: ApiFamily::OpenAiChatCompletions,
            model_id: "synthetic-ordinary-sealed-model".to_owned(),
            display_name: Some("Synthetic ordinary sealed model".to_owned()),
            route_config: ModelRouteConfig::default(),
            status: ModelAvailability::Available,
            miss_count: 0,
            raw_metadata: None,
            metadata_source: ModelMetadataSource::UserOverride,
            metadata_observed_at: None,
            last_reconciled_sync_job_id: None,
            metadata_sync_job_id: None,
            first_seen_at: now,
            last_seen_at: Some(now),
        })
        .expect("save synthetic model route");
    let preset = core
        .upsert_generation_preset(GenerationPreset {
            id: "synthetic-ordinary-sealed-preset".into(),
            model_route_id: route.id.clone(),
            display_name: "Synthetic ordinary sealed preset".to_owned(),
            values: Vec::new(),
            reasoning: GenerationReasoningSettings::default(),
            prompt_cache: GenerationPromptCacheSettings::default(),
            created_at: now,
            updated_at: now,
        })
        .expect("save synthetic generation preset");
    GenerationTarget {
        model_route_id: route.id,
        generation_preset_id: preset.id,
    }
}

fn set_json_mode(core: &Core, route_id: &ModelRouteId, supported: bool) {
    core.upsert_user_capability_override(CapabilityObservation {
        id: ObservationId::from("synthetic-ordinary-sealed-json-mode"),
        model_route_id: route_id.clone(),
        key: CapabilityKey::JsonMode,
        value: CapabilityValue::Boolean(supported),
        status: if supported {
            SupportStatus::Verified
        } else {
            SupportStatus::Unsupported
        },
        source: ObservationSource::UserOverride,
        confidence: Confidence::Low,
        observed_at: Utc::now(),
        expires_at: None,
        evidence_ref: None,
    })
    .expect("save synthetic JsonMode override");
}

fn activate_module(core: &Core, module: &ContentModule, target: ContentModuleRuntimeTarget) {
    core.upsert_content_module(module, None)
        .expect("save synthetic interaction module");
    let request = ContentModuleActivationRequest {
        runtime_target: target,
        expected_binding_revision: None,
        binding: ContentModuleBindingDraft {
            id: ModuleBindingId::from("synthetic.core.ordinary-sealed.binding"),
            module_id: module.id.clone(),
            scope: ModuleScope::App,
            target_id: None,
            conversation_id: None,
            priority: 0,
            resolution_mode: ModuleRevisionResolutionMode::Active,
            pinned_revision_id: None,
            package_import_approval_id: None,
            variable_overrides: VariableMap::default(),
        },
    };
    let review = core
        .review_content_module_activation(&request)
        .expect("review synthetic module activation");
    let resolutions = ModuleMergeResolutionSet {
        expected_review_sha256: review.review_sha256.clone(),
        resolutions: Vec::new(),
    };
    let plan = core
        .resolve_content_module_activation(&request, &resolutions)
        .expect("resolve synthetic module activation");
    core.activate_content_module(
        &request,
        &resolutions,
        &ModuleActivationApproval {
            approval_id: "synthetic-ordinary-sealed-activation".to_owned(),
            expected_review_sha256: review.review_sha256,
            expected_plan_sha256: plan.plan_sha256,
        },
    )
    .expect("activate synthetic interaction module")
    .verify()
    .expect("verify synthetic module activation receipt");
}

fn read_policy(
    connection: &Connection,
    conversation_id: &str,
    branch_id: &str,
) -> InteractionPolicySnapshot {
    let json = connection
        .query_row(
            "SELECT policy_json FROM interaction_events
             WHERE conversation_id = ?1 AND branch_id = ?2
             ORDER BY resulting_state_revision DESC LIMIT 1",
            params![conversation_id, branch_id],
            |row| row.get::<_, String>(0),
        )
        .expect("read latest interaction policy");
    serde_json::from_str(&json).expect("decode latest interaction policy")
}

fn read_seal(
    connection: &Connection,
    conversation_id: &str,
    branch_id: &str,
) -> InteractionEvaluationSeal {
    let json = connection
        .query_row(
            "SELECT evaluation_seal_json FROM interaction_events
             WHERE conversation_id = ?1 AND branch_id = ?2
               AND evaluation_seal_version = 1
             ORDER BY resulting_state_revision DESC LIMIT 1",
            params![conversation_id, branch_id],
            |row| row.get::<_, String>(0),
        )
        .expect("read latest interaction evaluation seal");
    serde_json::from_str(&json).expect("decode latest interaction evaluation seal")
}

fn ui_effects(effects: &[InteractionEffect]) -> Vec<InteractionEffect> {
    effects
        .iter()
        .filter(|effect| {
            matches!(
                effect,
                InteractionEffect::AssetShown { .. }
                    | InteractionEffect::AudioRequested { .. }
                    | InteractionEffect::ChoicesPresented { .. }
                    | InteractionEffect::VisibleSystemEvent { .. }
                    | InteractionEffect::DiceRolled { .. }
                    | InteractionEffect::ApprovalRequested { .. }
            )
        })
        .cloned()
        .collect()
}

#[test]
#[allow(
    clippy::too_many_lines,
    reason = "one restart scenario must preserve the same root state, sealed context, child seed, current-context counterfactual, durable effects, and terminal invalid-authority evidence"
)]
fn delayed_ordinary_occurrence_uses_stored_seal_and_seed_after_runtime_drift() {
    let root = tempdir().expect("create synthetic Core root");
    let trigger = variable("sealed_trigger");
    let roll = variable("sealed_roll");
    let rules = rule_set(&trigger, &roll);

    let core = Core::open(CoreConfig::new(root.path())).expect("open initial Core");
    let character_id = import_character(&core);
    let target = provider_target(&core);
    core.select_generation_target(Some(target.clone()))
        .expect("select synthetic generation target");
    set_json_mode(&core, &target.model_route_id, true);
    let conversation = core
        .create_conversation(
            &character_id,
            "Synthetic ordinary sealed replay room",
            ConversationMode::Chat,
        )
        .expect("create synthetic conversation");
    let branch = core
        .list_conversation_branches(&conversation.id)
        .expect("list synthetic branches")
        .into_iter()
        .next()
        .expect("synthetic root branch");
    let stored_rules = core
        .upsert_interaction_rule_set(&rules, None)
        .expect("save synthetic interaction rules");
    let revision_id = stored_rules
        .revision_id
        .expect("immutable interaction rule-set revision");
    activate_module(
        &core,
        &interaction_module(&rules.id),
        ContentModuleRuntimeTarget {
            conversation_id: conversation.id.clone(),
            branch_id: branch.id.clone(),
        },
    );
    let lifecycle = core
        .drain_core_lifecycle_occurrences(64)
        .expect("materialize lifecycle seal with active module");
    assert!(lifecycle.queue_idle);
    drop(core);

    let storage = Storage::open(root.path()).expect("open direct synthetic Storage");
    let connection = Connection::open(active_database_path(root.path()))
        .expect("open synthetic authority connection");
    let snapshot = storage
        .get_interaction_state_snapshot(&conversation.id, &branch.id)
        .expect("read root interaction state");
    let root_action = &rules.rules[0].actions[0];
    let mut root_state = snapshot.state.clone();
    root_state
        .variables
        .insert(trigger.clone(), VariableValue::Integer(1));
    root_state.revision = snapshot.state.revision + 1;
    let occurred_at = Utc::now();
    let mut seal = read_seal(&connection, &conversation.id.0, &branch.id.0);
    seal.event_epoch_seconds = occurred_at.timestamp();
    seal.template_values.current_date = Some(occurred_at.format("%Y-%m-%d").to_string());
    seal.template_values.current_time = Some(occurred_at.format("%H:%M:%S%:z").to_string());
    assert_eq!(
        seal.template_values.character_name.as_deref(),
        Some(SEALED_CHARACTER_NAME)
    );
    assert!(
        seal.supported_capabilities
            .contains(&CapabilityKey::JsonMode)
    );
    storage
        .commit_interaction_event(&InteractionEventCommit {
            event_id: "synthetic-ordinary-sealed-root-event".to_owned(),
            idempotency_key: "synthetic-ordinary-sealed-root-key".to_owned(),
            key: snapshot.key,
            expected_state_revision: snapshot.state.revision,
            event: InteractionEvent::UserAction {
                action_id: ROOT_ACTION_ID.to_owned(),
            },
            generation_attempt_id: None,
            owner_message_id: None,
            policy: read_policy(&connection, &conversation.id.0, &branch.id.0),
            evaluation_seal: Some(seal.clone()),
            deterministic_seed: Some(7),
            next_state: root_state.clone(),
            knowledge: snapshot.knowledge,
            action_results: vec![InteractionActionResultWrite {
                set_revision_id: revision_id.clone(),
                rule_id: rules.rules[0].id.clone(),
                action_ordinal: 0,
                status: InteractionActionResultStatus::Applied,
                result: VersionedJson {
                    schema_version: 1,
                    value: serde_json::json!({"status": "applied"}),
                },
            }],
            effects: vec![InteractionEffect::VariableSet {
                target: trigger.clone(),
                previous: None,
                value: VariableValue::Integer(1),
            }],
            derived_events: vec![InteractionDerivedEventWrite {
                event: InteractionEvent::VariableChanged {
                    variable: trigger.clone(),
                },
                deterministic_seed: u64::MAX,
                source_set_revision_id: revision_id.clone(),
                source_rule_id: rules.rules[0].id.clone(),
                source_action_ordinal: 0,
                source_effect_ordinal: 0,
                source_action_sha256: interaction_action_sha256(root_action)
                    .expect("hash synthetic root action"),
            }],
            proposals: Vec::new(),
            created_at: occurred_at,
        })
        .expect("commit sealed ordinary root event");
    let claimed = storage
        .claim_interaction_derived_events(occurred_at, occurred_at + ChronoDuration::seconds(30), 1)
        .expect("claim sealed child before restart")
        .pop()
        .expect("sealed child occurrence");
    assert_eq!(claimed.deterministic_seed, u64::MAX);
    assert_eq!(claimed.evaluation_seal, seal);
    let retry_at = Utc::now() + ChronoDuration::seconds(2);
    storage
        .retry_interaction_derived_event_after(
            &claimed.occurrence_id,
            claimed.delivery_attempts,
            retry_at,
        )
        .expect("defer sealed child across restart");
    drop(connection);
    drop(storage);

    let connection = Connection::open(active_database_path(root.path()))
        .expect("open synthetic character drift connection");
    connection
        .execute(
            "UPDATE characters SET name = ?1 WHERE id = ?2",
            params![DRIFTED_CHARACTER_NAME, character_id],
        )
        .expect("drift current character template value");
    drop(connection);

    let core = Core::open(CoreConfig::new(root.path())).expect("reopen Core with deferred child");
    set_json_mode(&core, &target.model_route_id, false);
    assert_eq!(
        core.get_character(&character_id)
            .expect("read drifted character")
            .name,
        DRIFTED_CHARACTER_NAME
    );
    let current_preview = core
        .preview_interaction_event(&lorepia_core::InteractionReviewRequest {
            conversation_id: conversation.id.clone(),
            branch_id: branch.id.clone(),
            expected_head: branch.head_message_id.clone(),
            event: claimed.event.clone(),
        })
        .expect("preview current mutable interaction context");
    assert!(
        !current_preview
            .supported_capabilities
            .contains(&CapabilityKey::JsonMode)
    );
    assert!(current_preview.outcome.effects.is_empty());
    assert!(current_preview.outcome.trace.iter().any(|trace| {
        trace.rule_id.as_str() == CHILD_RULE_ID
            && trace.status == InteractionRuleStatus::ConditionFalse
    }));

    let expected =
        InteractionEngine::compile(std::slice::from_ref(&rules), InteractionLimits::default())
            .expect("compile synthetic counterfactual engine")
            .handle_event(
                &root_state,
                &claimed.event,
                &InteractionContext {
                    deterministic_seed: claimed.deterministic_seed,
                    event_epoch_seconds: claimed.evaluation_seal.event_epoch_seconds,
                    model_capabilities: claimed.evaluation_seal.supported_capabilities.clone(),
                    template_values: InteractionTemplateValues {
                        character_name: claimed
                            .evaluation_seal
                            .template_values
                            .character_name
                            .clone(),
                        user_name: claimed.evaluation_seal.template_values.user_name.clone(),
                        persona_name: claimed.evaluation_seal.template_values.persona_name.clone(),
                        persona_description: claimed
                            .evaluation_seal
                            .template_values
                            .persona_description
                            .clone(),
                        current_date: claimed.evaluation_seal.template_values.current_date.clone(),
                        current_time: claimed.evaluation_seal.template_values.current_time.clone(),
                    },
                },
            )
            .expect("evaluate exact sealed child counterfactual");
    let expected_ui_effects = ui_effects(&expected.effects);
    assert_eq!(expected_ui_effects.len(), 2);

    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        core.drain_interaction_derived_events()
            .expect("drain available sealed derived occurrences");
        let connection = Connection::open(active_database_path(root.path()))
            .expect("inspect sealed child materialization");
        let count = connection
            .query_row(
                "SELECT COUNT(*) FROM interaction_events
                 WHERE event_kind = 'variable_changed'",
                [],
                |row| row.get::<_, u64>(0),
            )
            .expect("count sealed child events");
        let pending = connection
            .query_row(
                "SELECT COUNT(*) FROM interaction_derived_event_outbox
                 WHERE status != 'acknowledged'",
                [],
                |row| row.get::<_, u64>(0),
            )
            .expect("count pending sealed child occurrences");
        if count >= 1 && pending == 0 {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "sealed child chain did not become idle"
        );
        thread::sleep(Duration::from_millis(20));
    }

    let connection = Connection::open(active_database_path(root.path()))
        .expect("inspect final sealed materialization");
    let child_event_id = connection
        .query_row(
            "SELECT id FROM interaction_events
             WHERE event_kind = 'variable_changed'
             ORDER BY resulting_state_revision, id LIMIT 1",
            [],
            |row| row.get::<_, String>(0),
        )
        .expect("read sealed child event id");
    let mut statement = connection
        .prepare(
            "SELECT effect_json FROM interaction_effect_outbox
             WHERE event_id = ?1 ORDER BY sequence",
        )
        .expect("prepare sealed child effects query");
    let actual_ui_effects = statement
        .query_map([&child_event_id], |row| row.get::<_, String>(0))
        .expect("query sealed child effects")
        .map(|row| {
            serde_json::from_str::<InteractionEffect>(&row.expect("read sealed child effect"))
                .expect("decode sealed child effect")
        })
        .collect::<Vec<_>>();
    assert_eq!(actual_ui_effects, expected_ui_effects);
    let visible_text = actual_ui_effects
        .iter()
        .find_map(|effect| match effect {
            InteractionEffect::VisibleSystemEvent { text } => Some(text.as_str()),
            _ => None,
        })
        .expect("sealed child visible event");
    assert!(visible_text.contains(SEALED_CHARACTER_NAME));
    assert!(!visible_text.contains(DRIFTED_CHARACTER_NAME));
    assert!(
        visible_text.contains(
            claimed
                .evaluation_seal
                .template_values
                .current_time
                .as_deref()
                .expect("sealed current time")
        )
    );
    let payload: serde_json::Value = connection
        .query_row(
            "SELECT payload_json FROM interaction_events WHERE id = ?1",
            [&child_event_id],
            |row| row.get::<_, String>(0),
        )
        .map(|json| serde_json::from_str(&json).expect("decode sealed child payload"))
        .expect("read sealed child payload");
    assert_eq!(payload["deterministic_seed"].as_u64(), Some(u64::MAX));
    let pending = connection
        .query_row(
            "SELECT COUNT(*) FROM interaction_derived_event_outbox
             WHERE status != 'acknowledged'",
            [],
            |row| row.get::<_, u64>(0),
        )
        .expect("count remaining derived occurrences");
    assert_eq!(pending, 0);
    let event_count = connection
        .query_row("SELECT COUNT(*) FROM interaction_events", [], |row| {
            row.get::<_, u64>(0)
        })
        .expect("count final interaction events");
    drop(statement);
    drop(connection);
    drop(core);

    let reopened =
        Core::open(CoreConfig::new(root.path())).expect("reopen idle sealed replay Core");
    thread::sleep(Duration::from_millis(50));
    let connection =
        Connection::open(active_database_path(root.path())).expect("inspect idle reopen");
    assert_eq!(
        connection
            .query_row("SELECT COUNT(*) FROM interaction_events", [], |row| {
                row.get::<_, u64>(0)
            })
            .expect("count idle reopen events"),
        event_count
    );
    assert_eq!(
        connection
            .query_row(
                "SELECT COUNT(*) FROM interaction_derived_event_quarantines",
                [],
                |row| row.get::<_, u64>(0),
            )
            .expect("count idle reopen quarantines"),
        0
    );
    drop(connection);
    drop(reopened);

    // A well-formed occurrence may still name policy authority that cannot be
    // recovered after restart. That is terminal evidence, never an invitation
    // to reinterpret the event under the current module stack.
    let authority_core =
        Core::open(CoreConfig::new(root.path())).expect("open foreign-authority Core");
    let foreign_conversation = authority_core
        .create_conversation(
            &character_id,
            "Synthetic foreign authority room",
            ConversationMode::Chat,
        )
        .expect("create foreign-authority conversation");
    let foreign_branch = authority_core
        .list_conversation_branches(&foreign_conversation.id)
        .expect("list foreign-authority branches")
        .into_iter()
        .next()
        .expect("foreign-authority root branch");
    assert!(
        authority_core
            .drain_core_lifecycle_occurrences(64)
            .expect("materialize foreign branch module authority")
            .queue_idle
    );
    drop(authority_core);
    let connection = Connection::open(active_database_path(root.path()))
        .expect("inspect foreign module authority");
    let foreign_module_plan_sha256 = read_policy(
        &connection,
        &foreign_conversation.id.0,
        &foreign_branch.id.0,
    )
    .module_plan_sha256
    .expect("foreign branch applied module plan");
    drop(connection);

    let storage = Storage::open(root.path()).expect("open terminal-authority Storage");
    let connection = Connection::open(active_database_path(root.path()))
        .expect("inspect terminal-authority root");
    let snapshot = storage
        .get_interaction_state_snapshot(&conversation.id, &branch.id)
        .expect("read terminal-authority root state");
    let real_policy = read_policy(&connection, &conversation.id.0, &branch.id.0);
    let real_policy_sha256 = interaction_policy_sha256(&real_policy)
        .expect("hash current recoverable interaction policy");
    let mut unrecoverable_policy = real_policy.clone();
    assert_ne!(
        real_policy.module_plan_sha256.as_deref(),
        Some(foreign_module_plan_sha256.as_str())
    );
    unrecoverable_policy.module_plan_sha256 = Some(foreign_module_plan_sha256);
    let unrecoverable_policy_sha256 = interaction_policy_sha256(&unrecoverable_policy)
        .expect("hash synthetic unrecoverable policy");
    let terminal_at = Utc::now();
    let mut terminal_seal = read_seal(&connection, &conversation.id.0, &branch.id.0);
    terminal_seal.policy_sha256 = Sha256Digest::parse(unrecoverable_policy_sha256.clone())
        .expect("parse synthetic unrecoverable policy hash");
    terminal_seal.event_epoch_seconds = terminal_at.timestamp();
    terminal_seal.template_values.current_date = Some(terminal_at.format("%Y-%m-%d").to_string());
    terminal_seal.template_values.current_time =
        Some(terminal_at.format("%H:%M:%S%:z").to_string());
    let previous_trigger = snapshot.state.variables.get(&trigger).cloned();
    let mut terminal_state = snapshot.state.clone();
    terminal_state
        .variables
        .insert(trigger.clone(), VariableValue::Integer(2));
    terminal_state.revision = snapshot
        .state
        .revision
        .checked_add(1)
        .expect("terminal-authority state revision");
    storage
        .commit_interaction_event(&InteractionEventCommit {
            event_id: "synthetic-ordinary-unrecoverable-root-event".to_owned(),
            idempotency_key: "synthetic-ordinary-unrecoverable-root-key".to_owned(),
            key: snapshot.key,
            expected_state_revision: snapshot.state.revision,
            event: InteractionEvent::UserAction {
                action_id: ROOT_ACTION_ID.to_owned(),
            },
            generation_attempt_id: None,
            owner_message_id: None,
            policy: unrecoverable_policy,
            evaluation_seal: Some(terminal_seal),
            deterministic_seed: Some(8),
            next_state: terminal_state,
            knowledge: snapshot.knowledge,
            action_results: vec![InteractionActionResultWrite {
                set_revision_id: revision_id.clone(),
                rule_id: rules.rules[0].id.clone(),
                action_ordinal: 0,
                status: InteractionActionResultStatus::Applied,
                result: VersionedJson {
                    schema_version: 1,
                    value: serde_json::json!({"status": "applied"}),
                },
            }],
            effects: vec![InteractionEffect::VariableSet {
                target: trigger.clone(),
                previous: previous_trigger,
                value: VariableValue::Integer(2),
            }],
            derived_events: vec![InteractionDerivedEventWrite {
                event: InteractionEvent::VariableChanged {
                    variable: trigger.clone(),
                },
                deterministic_seed: u64::MAX - 1,
                source_set_revision_id: revision_id,
                source_rule_id: rules.rules[0].id.clone(),
                source_action_ordinal: 0,
                source_effect_ordinal: 0,
                source_action_sha256: interaction_action_sha256(root_action)
                    .expect("hash terminal-authority source action"),
            }],
            proposals: Vec::new(),
            created_at: terminal_at,
        })
        .expect("commit canonical occurrence with unavailable policy authority");
    let event_count_with_terminal_parent = connection
        .query_row(
            "SELECT COUNT(*) FROM interaction_events
             WHERE conversation_id = ?1 AND branch_id = ?2",
            params![conversation.id.0, branch.id.0],
            |row| row.get::<_, u64>(0),
        )
        .expect("count terminal-authority parent event");
    assert_eq!(event_count_with_terminal_parent, event_count + 1);
    drop(connection);
    drop(storage);

    let core = Core::open(CoreConfig::new(root.path()))
        .expect("reopen Core for terminal authority quarantine");
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        core.drain_interaction_derived_events()
            .expect("drain canonical but unrecoverable authority");
        let connection = Connection::open(active_database_path(root.path()))
            .expect("inspect terminal authority quarantine");
        let quarantines = connection
            .query_row(
                "SELECT COUNT(*) FROM interaction_derived_event_quarantines",
                [],
                |row| row.get::<_, u64>(0),
            )
            .expect("count terminal authority quarantines");
        if quarantines == 1 {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "unrecoverable authority was not terminally quarantined"
        );
        thread::sleep(Duration::from_millis(20));
    }
    let connection = Connection::open(active_database_path(root.path()))
        .expect("inspect terminal quarantine evidence");
    let (reason_kind, sealed_policy_sha256, active_policy_sha256, delivery_attempts) = connection
        .query_row(
            "SELECT reason_kind, sealed_policy_sha256, active_policy_sha256,
                    delivery_attempts
             FROM interaction_derived_event_quarantines",
            [],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, Option<String>>(2)?,
                    row.get::<_, u64>(3)?,
                ))
            },
        )
        .expect("read terminal quarantine evidence");
    assert_eq!(reason_kind, "sealed_policy_recovery_failed");
    assert_eq!(sealed_policy_sha256, unrecoverable_policy_sha256);
    assert_eq!(
        active_policy_sha256.as_deref(),
        Some(real_policy_sha256.as_str())
    );
    assert_eq!(delivery_attempts, 1);
    assert_eq!(
        connection
            .query_row(
                "SELECT COUNT(*) FROM interaction_events
                 WHERE conversation_id = ?1 AND branch_id = ?2",
                params![conversation.id.0, branch.id.0],
                |row| row.get::<_, u64>(0),
            )
            .expect("count events after terminal quarantine"),
        event_count_with_terminal_parent
    );
    assert_eq!(
        connection
            .query_row(
                "SELECT COUNT(*)
                 FROM interaction_derived_event_outbox AS occurrence
                 WHERE occurrence.status != 'acknowledged'
                   AND NOT EXISTS (
                       SELECT 1
                       FROM interaction_derived_event_quarantines AS quarantine
                       WHERE quarantine.occurrence_id = occurrence.occurrence_id
                   )",
                [],
                |row| row.get::<_, u64>(0),
            )
            .expect("count live occurrences after terminal quarantine"),
        0
    );
    drop(connection);
    drop(core);

    let core = Core::open(CoreConfig::new(root.path()))
        .expect("reopen Core after terminal authority quarantine");
    assert!(
        core.drain_interaction_derived_events()
            .expect("replay terminal quarantine")
            .is_empty()
    );
    let connection = Connection::open(active_database_path(root.path()))
        .expect("inspect terminal quarantine replay");
    assert_eq!(
        connection
            .query_row(
                "SELECT COUNT(*) FROM interaction_derived_event_quarantines",
                [],
                |row| row.get::<_, u64>(0),
            )
            .expect("count terminal quarantine replay"),
        1
    );
    assert_eq!(
        connection
            .query_row(
                "SELECT COUNT(*) FROM interaction_events
                 WHERE conversation_id = ?1 AND branch_id = ?2",
                params![conversation.id.0, branch.id.0],
                |row| row.get::<_, u64>(0),
            )
            .expect("count terminal quarantine replay events"),
        event_count_with_terminal_parent
    );
    drop(core);
}
