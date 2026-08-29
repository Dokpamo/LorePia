//! End-to-end invariants for generation-attempt-owned derived interactions.

use std::{
    io::{Read, Write},
    net::{TcpListener, TcpStream},
    path::{Path, PathBuf},
    sync::mpsc,
    thread,
    time::{Duration, Instant},
};

use chrono::{Duration as ChronoDuration, Utc};
use lorepia_core::{
    ApiFamily, CanonicalOrigin, CapabilityKey, CapabilityObservation, CapabilityValue, Confidence,
    ConnectionBoundCredential, ConnectionConfigEntry, ConnectionConfigValue,
    ContentModuleActivationRequest, ContentModuleBindingDraft, ContentModuleDeactivationRequest,
    ContentModuleRuntimeTarget, Conversation, ConversationBranch,
    ConversationPersonaSelectionRequest, ConversationPersonaSelectionState, Core, CoreConfig,
    CoreErrorCode, EndpointPath, ExpertPromptPreview, GenerationOperationContext, GenerationPreset,
    GenerationPromptCacheSettings, GenerationReasoningSettings, GenerationTarget,
    InteractionProposalDecisionRequest, Message, MessageActionGeneration, MessageStatus,
    ModelAvailability, ModelMetadataSource, ModelRoute, ModelRouteConfig, ModelRouteId,
    ModuleActivationApproval, ModuleMergeResolutionSet, ObjectRevision, ObservationId,
    ObservationSource, PersonaCreateRequest, PersonaUpdateRequest, PromptPlanRequest,
    ProviderConnectionDraft, ProviderConnectionId, ProviderNetworkMode, Revisioned,
    RoomOrchestrationConfig, RoomOrchestrationConfigPatch, SupportStatus,
};
use lorepia_domain::{
    ActivationRule, AuxiliaryTaskKind, BlockSource, BuiltInTemplateValue, CharacterContentV1,
    CharacterField, ContentCapability, ContentModule, ContentModuleId, ControlId, ControlKind,
    ControlSpec, ConversationMode, GenerationId, GenerationReasoningEffort, HistorySelector,
    InstructionAuthority, InteractionAction, InteractionEffect, InteractionEvent,
    InteractionProposalDecision, InteractionProposalStatus, InteractionRule, InteractionRuleId,
    InteractionRuleSet, InteractionRuleSetId, KnowledgeBook, KnowledgeBookId, KnowledgeEntry,
    KnowledgeEntryId, KnowledgePlacement, LocalUserId, MemoryKind, MemoryProfile, MemoryProfileId,
    MemoryRecord, MemoryRecordId, MergePolicy, MessageId, ModuleBindingId,
    ModuleRevisionResolutionMode, ModuleScope, OverflowPolicy, PackageMetadata, Persona,
    PlacementZone, PresetMetadata, PromptBlock, PromptBlockId, PromptBlockKind, PromptPreset,
    PromptPresetId, PromptResolutionTrace, ProposalSpec, Provenance, RateLimit, RoleHint,
    SafeTemplate, SourceKind, SummarySchemaId, TaskProfile, TaskProfileId, TemplatePart,
    TemplateSlot, TokenBudget, TokenPolicy, ValueExpr, VariableId, VariableMap, VariableRef,
    VariableScope, VariableType, VariableValue, VersionedJson,
};
use lorepia_storage::{
    GenerationAttemptStatus, InteractionActionResultStatus, InteractionActionResultWrite,
    InteractionDerivedEventWrite, InteractionEvaluationSeal, InteractionEventCommit,
    InteractionPolicySnapshot, MemoryRecordUserPatch, PromptResponseLength,
    ProviderCredentialAccessAuthority, ProviderCredentialObservedStatus,
    ProviderCredentialOperationKind, Storage, StoredInteractionState, StoredRevision,
    generation_attempt_derived_closure_sha256, interaction_action_sha256,
    interaction_evaluation_seal_sha256, interaction_state_snapshot_sha256,
};
use rusqlite::{Connection, params};
use serde_json::json;
use tempfile::{NamedTempFile, TempDir, tempdir};

const CONNECTION_ID: &str = "synthetic-derived-closure-connection";
const KNOWLEDGE_TEXT: &str = "SYNTHETIC_DERIVED_KNOWLEDGE_7C31";
const SEALED_USER: &str = "SYNTHETIC_SEALED_USER_4D20";
const DRIFTED_USER: &str = "SYNTHETIC_DRIFTED_USER_839A";
const SEALED_SLOT: &str = "SYNTHETIC_SEALED_SLOT_2B18";
const DRIFTED_SLOT: &str = "SYNTHETIC_DRIFTED_SLOT_66E1";
const SEALED_PERSONA_DESCRIPTION: &str = "SYNTHETIC_SEALED_PERSONA_DESCRIPTION_9D11";
const DRIFTED_PERSONA_DESCRIPTION: &str = "SYNTHETIC_DRIFTED_PERSONA_DESCRIPTION_A822";
const SEALED_CHARACTER_DESCRIPTION: &str = "SYNTHETIC_SEALED_CHARACTER_DESCRIPTION_18C2";
const DRIFTED_CHARACTER_DESCRIPTION: &str = "SYNTHETIC_DRIFTED_CHARACTER_DESCRIPTION_3A07";
const SEALED_CHARACTER_PERSONALITY: &str = "SYNTHETIC_SEALED_CHARACTER_PERSONALITY_1B4D";
const DRIFTED_CHARACTER_PERSONALITY: &str = "SYNTHETIC_DRIFTED_CHARACTER_PERSONALITY_6E90";
const CHARACTER_KNOWLEDGE_BOOK_ID: &str = "synthetic.derived-closure.character-knowledge";
const SEALED_CHARACTER_KNOWLEDGE: &str = "SYNTHETIC_SEALED_CHARACTER_KNOWLEDGE_4C52";
const DRIFTED_CHARACTER_KNOWLEDGE: &str = "SYNTHETIC_DRIFTED_CHARACTER_KNOWLEDGE_71AF";
const SELECTED_MEMORY_ID: &str = "synthetic.derived-closure.selected-memory";
const SEALED_SELECTED_MEMORY: &str = "SYNTHETIC_SEALED_SELECTED_MEMORY_247B";
const DRIFTED_SELECTED_MEMORY: &str = "SYNTHETIC_DRIFTED_SELECTED_MEMORY_8A63";
const SUMMARY_MEMORY_ID: &str = "synthetic.derived-closure.conversation-summary";
const SEALED_CONVERSATION_SUMMARY: &str = "SYNTHETIC_SEALED_CONVERSATION_SUMMARY_2DD1";
const DRIFTED_CONVERSATION_SUMMARY: &str = "SYNTHETIC_DRIFTED_CONVERSATION_SUMMARY_7F24";
const NO_MODULE_GATE_BINDING_ID: &str = "synthetic.no-module-review.gate.binding";
const NO_MODULE_GATE_PROPOSAL_ID: &str = "synthetic-no-module-review-gate";

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

fn open_core_after_drop(data_root: &std::path::Path) -> Core {
    let deadline = Instant::now() + Duration::from_secs(2);
    loop {
        match Core::open(CoreConfig::new(data_root)) {
            Ok(core) => return core,
            Err(error)
                if error.code == CoreErrorCode::StorageUnavailable
                    && error.message == "data root is already owned by another LorePia process"
                    && Instant::now() < deadline =>
            {
                thread::sleep(Duration::from_millis(10));
            }
            Err(error) => panic!("open Core after prior owner drop: {error:?}"),
        }
    }
}

fn provenance(source_id: &str) -> Provenance {
    Provenance {
        source_kind: SourceKind::UserCreated,
        source_id: Some(source_id.to_owned()),
        source_hash: Some("ab".repeat(32)),
        author: Some("Synthetic derived-closure test".to_owned()),
        license: Some("LicenseRef-Synthetic-Test".to_owned()),
        imported_at: None,
    }
}

fn variable(module_id: &ContentModuleId, id: &str) -> VariableRef {
    VariableRef {
        scope: VariableScope::Module,
        namespace: Some(module_id.clone()),
        id: VariableId::from(id),
    }
}

fn text_value(value: &str) -> ValueExpr {
    ValueExpr::Literal {
        value: VariableValue::Text(value.to_owned()),
    }
}

fn proposal_template(marker: &str, variable: &VariableRef) -> SafeTemplate {
    SafeTemplate {
        parts: vec![
            TemplatePart::Text {
                value: format!("{marker};VALUE="),
            },
            TemplatePart::Variable {
                variable: variable.clone(),
            },
            TemplatePart::Text {
                value: ";CHAR=".to_owned(),
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
        max_output_chars: 512,
    }
}

fn text_control(variable: &VariableRef, id: &str) -> ControlSpec {
    ControlSpec {
        id: ControlId::from(id),
        label: format!("Synthetic {id}"),
        description: "Synthetic generation-attempt variable".to_owned(),
        kind: ControlKind::Text,
        value_type: Some(VariableType::Text),
        variable: Some(variable.clone()),
        default_value: Some(VariableValue::Text("initial".to_owned())),
        options: Vec::new(),
        minimum: None,
        maximum: None,
        step: None,
        visible_when: None,
        scope: VariableScope::Module,
        sensitive: false,
        requires_regeneration: true,
    }
}

#[derive(Clone)]
struct ClosureVariables {
    root: VariableRef,
    child: VariableRef,
    knowledge_child: VariableRef,
    approval_child: VariableRef,
    capability_child: VariableRef,
    final_child: VariableRef,
}

struct ClosureFixture {
    module: ContentModule,
    rules: InteractionRuleSet,
    variables: ClosureVariables,
    knowledge_entry_id: KnowledgeEntryId,
}

fn closure_variables(module_id: &ContentModuleId) -> ClosureVariables {
    ClosureVariables {
        root: variable(module_id, "synthetic.derived-closure.module.root"),
        child: variable(module_id, "synthetic.derived-closure.module.child"),
        knowledge_child: variable(
            module_id,
            "synthetic.derived-closure.module.knowledge-child",
        ),
        approval_child: variable(module_id, "synthetic.derived-closure.module.approval-child"),
        capability_child: variable(
            module_id,
            "synthetic.derived-closure.module.capability-child",
        ),
        final_child: variable(module_id, "synthetic.derived-closure.module.final-child"),
    }
}

fn closure_before_rule(
    variables: &ClosureVariables,
    knowledge_entry_id: &KnowledgeEntryId,
) -> InteractionRule {
    InteractionRule {
        id: InteractionRuleId::from("synthetic.derived-closure.before"),
        name: "Seed derived generation state".to_owned(),
        enabled: true,
        imported_author_enabled: false,
        event: InteractionEvent::BeforeGeneration,
        condition: None,
        actions: vec![
            InteractionAction::SetVariable {
                target: variables.root.clone(),
                value: text_value("root-applied"),
            },
            InteractionAction::ActivateKnowledge {
                entry_id: knowledge_entry_id.clone(),
            },
        ],
        priority: 0,
        stop_after_match: false,
        provenance: provenance("synthetic.derived-closure.before"),
    }
}

fn closure_root_child_rule(variables: &ClosureVariables) -> InteractionRule {
    InteractionRule {
        id: InteractionRuleId::from("synthetic.derived-closure.root-child"),
        name: "Handle the root VariableChanged child".to_owned(),
        enabled: true,
        imported_author_enabled: false,
        event: InteractionEvent::VariableChanged {
            variable: variables.root.clone(),
        },
        condition: None,
        actions: vec![
            InteractionAction::SetVariable {
                target: variables.child.clone(),
                value: text_value("root-child-visible"),
            },
            InteractionAction::RequestUserApproval {
                proposal: ProposalSpec {
                    id: "approve-first-child".to_owned(),
                    title: "Approve first derived child".to_owned(),
                    body: proposal_template("SYNTHETIC_FIRST_CHILD_PROPOSAL", &variables.child),
                    expires_after_seconds: None,
                },
            },
        ],
        priority: 0,
        stop_after_match: false,
        provenance: provenance("synthetic.derived-closure.root-child"),
    }
}

fn closure_knowledge_child_rule(
    variables: &ClosureVariables,
    knowledge_entry_id: &KnowledgeEntryId,
) -> InteractionRule {
    InteractionRule {
        id: InteractionRuleId::from("synthetic.derived-closure.knowledge-child"),
        name: "Handle the KnowledgeActivated child".to_owned(),
        enabled: true,
        imported_author_enabled: false,
        event: InteractionEvent::KnowledgeActivated {
            entry_id: knowledge_entry_id.clone(),
        },
        condition: None,
        actions: vec![InteractionAction::SetVariable {
            target: variables.knowledge_child.clone(),
            value: text_value("knowledge-child-visible"),
        }],
        priority: 1,
        stop_after_match: false,
        provenance: provenance("synthetic.derived-closure.knowledge-child"),
    }
}

fn closure_first_approval_rule(variables: &ClosureVariables) -> InteractionRule {
    InteractionRule {
        id: InteractionRuleId::from("synthetic.derived-closure.approve-first"),
        name: "Approve the first child".to_owned(),
        enabled: true,
        imported_author_enabled: false,
        event: InteractionEvent::UserAction {
            action_id: "approve-first-child".to_owned(),
        },
        condition: None,
        actions: vec![InteractionAction::SetVariable {
            target: variables.approval_child.clone(),
            value: text_value("approval-user-action-applied"),
        }],
        priority: 0,
        stop_after_match: false,
        provenance: provenance("synthetic.derived-closure.approve-first"),
    }
}

fn closure_approval_child_rule(variables: &ClosureVariables) -> InteractionRule {
    InteractionRule {
        id: InteractionRuleId::from("synthetic.derived-closure.approval-child"),
        name: "Handle the approved UserAction derived child".to_owned(),
        enabled: true,
        imported_author_enabled: false,
        event: InteractionEvent::VariableChanged {
            variable: variables.approval_child.clone(),
        },
        condition: Some(lorepia_domain::ConditionExpr::ModelSupports {
            capability: CapabilityKey::JsonMode,
        }),
        actions: vec![
            InteractionAction::SetVariable {
                target: variables.capability_child.clone(),
                value: text_value("sealed-capability-visible"),
            },
            InteractionAction::RequestUserApproval {
                proposal: ProposalSpec {
                    id: "approve-second-child".to_owned(),
                    title: "Approve second derived child".to_owned(),
                    body: proposal_template(
                        "SYNTHETIC_SECOND_CHILD_PROPOSAL",
                        &variables.capability_child,
                    ),
                    expires_after_seconds: None,
                },
            },
        ],
        priority: 0,
        stop_after_match: false,
        provenance: provenance("synthetic.derived-closure.approval-child"),
    }
}

fn closure_second_approval_rule(variables: &ClosureVariables) -> InteractionRule {
    InteractionRule {
        id: InteractionRuleId::from("synthetic.derived-closure.approve-second"),
        name: "Finish the second child approval".to_owned(),
        enabled: true,
        imported_author_enabled: false,
        event: InteractionEvent::UserAction {
            action_id: "approve-second-child".to_owned(),
        },
        condition: None,
        actions: vec![InteractionAction::SetVariable {
            target: variables.final_child.clone(),
            value: text_value("second-approval-complete"),
        }],
        priority: 0,
        stop_after_match: false,
        provenance: provenance("synthetic.derived-closure.approve-second"),
    }
}

fn closure_rule_set(
    variables: &ClosureVariables,
    knowledge_entry_id: &KnowledgeEntryId,
) -> InteractionRuleSet {
    InteractionRuleSet {
        id: InteractionRuleSetId::from("synthetic.derived-closure.rules"),
        name: "Synthetic derived-closure rules".to_owned(),
        schema_version: 1,
        rules: vec![
            closure_before_rule(variables, knowledge_entry_id),
            closure_root_child_rule(variables),
            closure_knowledge_child_rule(variables, knowledge_entry_id),
            closure_first_approval_rule(variables),
            closure_approval_child_rule(variables),
            closure_second_approval_rule(variables),
        ],
        max_actions_per_event: 16,
        provenance: provenance("synthetic.derived-closure.rules"),
    }
}

fn closure_prompt_template(variables: &ClosureVariables) -> SafeTemplate {
    SafeTemplate {
        parts: vec![
            TemplatePart::Text {
                value: "SYNTHETIC_DERIVED_CLOSURE;ROOT=".to_owned(),
            },
            TemplatePart::Variable {
                variable: variables.root.clone(),
            },
            TemplatePart::Text {
                value: ";CHILD=".to_owned(),
            },
            TemplatePart::Variable {
                variable: variables.child.clone(),
            },
            TemplatePart::Text {
                value: ";KNOWLEDGE_CHILD=".to_owned(),
            },
            TemplatePart::Variable {
                variable: variables.knowledge_child.clone(),
            },
            TemplatePart::Text {
                value: ";APPROVAL_CHILD=".to_owned(),
            },
            TemplatePart::Variable {
                variable: variables.approval_child.clone(),
            },
            TemplatePart::Text {
                value: ";CAPABILITY_CHILD=".to_owned(),
            },
            TemplatePart::Variable {
                variable: variables.capability_child.clone(),
            },
            TemplatePart::Text {
                value: ";FINAL_CHILD=".to_owned(),
            },
            TemplatePart::Variable {
                variable: variables.final_child.clone(),
            },
            TemplatePart::Text {
                value: ";CHARACTER=".to_owned(),
            },
            TemplatePart::BuiltIn {
                value: BuiltInTemplateValue::CharacterName,
            },
            TemplatePart::Text {
                value: ";USER=".to_owned(),
            },
            TemplatePart::BuiltIn {
                value: BuiltInTemplateValue::UserName,
            },
            TemplatePart::Text {
                value: ";PERSONA_NAME=".to_owned(),
            },
            TemplatePart::BuiltIn {
                value: BuiltInTemplateValue::PersonaName,
            },
            TemplatePart::Text {
                value: ";PERSONA_DESCRIPTION=".to_owned(),
            },
            TemplatePart::BuiltIn {
                value: BuiltInTemplateValue::PersonaDescription,
            },
            TemplatePart::Text {
                value: ";SLOT=".to_owned(),
            },
            TemplatePart::Slot {
                name: "closure_context".to_owned(),
            },
            TemplatePart::Text {
                value: ";DATE=".to_owned(),
            },
            TemplatePart::BuiltIn {
                value: BuiltInTemplateValue::CurrentDate,
            },
            TemplatePart::Text {
                value: ";TIME=".to_owned(),
            },
            TemplatePart::BuiltIn {
                value: BuiltInTemplateValue::CurrentTime,
            },
        ],
        max_output_chars: 2_048,
    }
}

fn closure_controls(variables: &ClosureVariables) -> Vec<ControlSpec> {
    [
        (&variables.root, "synthetic.derived-closure.root-control"),
        (&variables.child, "synthetic.derived-closure.child-control"),
        (
            &variables.knowledge_child,
            "synthetic.derived-closure.knowledge-child-control",
        ),
        (
            &variables.approval_child,
            "synthetic.derived-closure.approval-child-control",
        ),
        (
            &variables.capability_child,
            "synthetic.derived-closure.capability-child-control",
        ),
        (
            &variables.final_child,
            "synthetic.derived-closure.final-child-control",
        ),
    ]
    .map(|(variable, id)| text_control(variable, id))
    .to_vec()
}

fn closure_prompt_blocks(prompt_template: SafeTemplate) -> Vec<PromptBlock> {
    vec![
        PromptBlock {
            id: PromptBlockId::from("synthetic.derived-closure.prompt"),
            name: "Derived closure state".to_owned(),
            kind: PromptBlockKind::StaticInstruction,
            enabled: true,
            role_hint: RoleHint::System,
            authority: InstructionAuthority::Creator,
            template: Some(prompt_template),
            condition: None,
            source: BlockSource::Template,
            placement_zone: PlacementZone::AssistantPrefill,
            history_selector: None,
            token_policy: TokenPolicy {
                priority: 1_000,
                min_tokens: None,
                max_tokens: Some(512),
                reserve_tokens: None,
            },
            overflow_policy: OverflowPolicy::Reject,
            merge_policy: MergePolicy::SeparateMessage,
            provenance: provenance("synthetic.derived-closure.prompt"),
        },
        PromptBlock {
            id: PromptBlockId::from("synthetic.derived-closure.selected-knowledge"),
            name: "Derived closure selected knowledge".to_owned(),
            kind: PromptBlockKind::WorldKnowledge,
            enabled: true,
            role_hint: RoleHint::System,
            authority: InstructionAuthority::Creator,
            template: None,
            condition: None,
            source: BlockSource::SelectedKnowledge,
            placement_zone: PlacementZone::RetrievedContext,
            history_selector: None,
            token_policy: TokenPolicy {
                priority: 1_000,
                min_tokens: None,
                max_tokens: Some(128),
                reserve_tokens: None,
            },
            overflow_policy: OverflowPolicy::Reject,
            merge_policy: MergePolicy::SeparateMessage,
            provenance: provenance("synthetic.derived-closure.selected-knowledge"),
        },
    ]
}

fn closure_module(
    module_id: ContentModuleId,
    knowledge_book_id: KnowledgeBookId,
    rule_set_id: InteractionRuleSetId,
    variables: &ClosureVariables,
) -> ContentModule {
    ContentModule {
        id: module_id,
        name: "Synthetic derived closure module".to_owned(),
        version: "1.0.0".to_owned(),
        schema_version: 1,
        prompt_fragments: closure_prompt_blocks(closure_prompt_template(variables)),
        knowledge_book_ids: vec![knowledge_book_id],
        control_specs: closure_controls(variables),
        transform_set_ids: Vec::new(),
        interaction_rule_set_ids: vec![rule_set_id],
        asset_ids: Vec::new(),
        imported_components_enabled: true,
        required_capabilities: vec![
            ContentCapability::PromptFragments,
            ContentCapability::Knowledge,
            ContentCapability::Variables,
            ContentCapability::DeclarativeInteractions,
        ],
        metadata: PackageMetadata {
            author: Some("Synthetic derived-closure test".to_owned()),
            license: "LicenseRef-Synthetic-Test".to_owned(),
            redistribution_allowed: false,
            homepage: None,
            description: "Synthetic attempt-owned closure acceptance fixture".to_owned(),
            tags: vec!["synthetic".to_owned()],
            provenance: provenance("synthetic.derived-closure.module"),
        },
    }
}

fn closure_fixture() -> ClosureFixture {
    let module_id = ContentModuleId::from("synthetic.derived-closure.module");
    let variables = closure_variables(&module_id);
    let knowledge_book_id = KnowledgeBookId::from("synthetic.derived-closure.knowledge");
    let knowledge_entry_id = KnowledgeEntryId::from("synthetic.derived-closure.knowledge.entry");
    let rules = closure_rule_set(&variables, &knowledge_entry_id);
    let module = closure_module(module_id, knowledge_book_id, rules.id.clone(), &variables);
    ClosureFixture {
        module,
        rules,
        variables,
        knowledge_entry_id,
    }
}

fn install_closure_fixture(
    core: &Core,
    runtime_target: ContentModuleRuntimeTarget,
) -> ClosureFixture {
    let fixture = closure_fixture();
    let book_id = fixture.module.knowledge_book_ids[0].clone();
    core.upsert_knowledge_book(
        &KnowledgeBook {
            id: book_id.clone(),
            name: "Synthetic derived closure knowledge".to_owned(),
            schema_version: 1,
            entries: vec![KnowledgeEntry {
                id: fixture.knowledge_entry_id.clone(),
                book_id,
                name: "Synthetic derived knowledge".to_owned(),
                content: KNOWLEDGE_TEXT.to_owned(),
                enabled: true,
                activation: ActivationRule::Manual,
                priority: 100,
                importance: 100,
                placement: KnowledgePlacement::RetrievedContext,
                token_policy: TokenPolicy {
                    priority: 100,
                    min_tokens: None,
                    max_tokens: Some(64),
                    reserve_tokens: None,
                },
                parent_id: None,
                activation_probability_basis_points: 10_000,
                provenance: provenance("synthetic.derived-closure.knowledge.entry"),
            }],
            scan_depth: 8,
            token_budget: TokenBudget { max_tokens: 128 },
            recursive: false,
            max_recursion_depth: 0,
            provenance: provenance("synthetic.derived-closure.knowledge"),
        },
        None,
    )
    .expect("save derived-closure knowledge");
    core.upsert_interaction_rule_set(&fixture.rules, None)
        .expect("save derived-closure rules");
    core.upsert_content_module(&fixture.module, None)
        .expect("save derived-closure module");

    let mut initial_variables = VariableMap::default();
    for variable in [
        &fixture.variables.root,
        &fixture.variables.child,
        &fixture.variables.knowledge_child,
        &fixture.variables.approval_child,
        &fixture.variables.capability_child,
        &fixture.variables.final_child,
    ] {
        initial_variables.insert(variable.clone(), VariableValue::Text("initial".to_owned()));
    }
    let request = ContentModuleActivationRequest {
        runtime_target,
        expected_binding_revision: None,
        binding: ContentModuleBindingDraft {
            id: ModuleBindingId::from("synthetic.derived-closure.binding"),
            module_id: fixture.module.id.clone(),
            scope: ModuleScope::App,
            target_id: None,
            conversation_id: None,
            priority: 0,
            resolution_mode: ModuleRevisionResolutionMode::Active,
            pinned_revision_id: None,
            package_import_approval_id: None,
            variable_overrides: initial_variables,
        },
    };
    let review = core
        .review_content_module_activation(&request)
        .expect("review derived-closure activation");
    let resolutions = ModuleMergeResolutionSet {
        expected_review_sha256: review.review_sha256.clone(),
        resolutions: Vec::new(),
    };
    let plan = core
        .resolve_content_module_activation(&request, &resolutions)
        .expect("resolve derived-closure activation");
    core.activate_content_module(
        &request,
        &resolutions,
        &ModuleActivationApproval {
            approval_id: "synthetic-derived-closure-activation".to_owned(),
            expected_review_sha256: review.review_sha256,
            expected_plan_sha256: plan.plan_sha256,
        },
    )
    .expect("activate derived-closure module")
    .verify()
    .expect("verify derived-closure activation receipt");
    fixture
}

fn no_module_review_gate_rule_set() -> InteractionRuleSet {
    InteractionRuleSet {
        id: InteractionRuleSetId::from("synthetic.no-module-review.gate.rules"),
        name: "Synthetic no-module review gate".to_owned(),
        schema_version: 1,
        rules: vec![InteractionRule {
            id: InteractionRuleId::from("synthetic.no-module-review.gate.opened"),
            name: "Pause generation behind an ordinary room approval".to_owned(),
            enabled: true,
            imported_author_enabled: false,
            event: InteractionEvent::ConversationOpened,
            condition: None,
            actions: vec![InteractionAction::RequestUserApproval {
                proposal: ProposalSpec {
                    id: NO_MODULE_GATE_PROPOSAL_ID.to_owned(),
                    title: "Synthetic no-module review gate".to_owned(),
                    body: SafeTemplate {
                        parts: vec![TemplatePart::Text {
                            value: "SYNTHETIC_NO_MODULE_REVIEW_GATE_8B31".to_owned(),
                        }],
                        max_output_chars: 128,
                    },
                    expires_after_seconds: None,
                },
            }],
            priority: 0,
            stop_after_match: false,
            provenance: provenance("synthetic.no-module-review.gate.opened"),
        }],
        max_actions_per_event: 4,
        provenance: provenance("synthetic.no-module-review.gate.rules"),
    }
}

fn no_module_review_gate_module(rule_set_id: &InteractionRuleSetId) -> ContentModule {
    ContentModule {
        id: ContentModuleId::from("synthetic.no-module-review.gate.module"),
        name: "Synthetic no-module review gate module".to_owned(),
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
            author: Some("Synthetic derived-closure test".to_owned()),
            license: "LicenseRef-Synthetic-Test".to_owned(),
            redistribution_allowed: false,
            homepage: None,
            description: "Temporary ordinary approval gate for a no-module attempt".to_owned(),
            tags: vec!["synthetic".to_owned()],
            provenance: provenance("synthetic.no-module-review.gate.module"),
        },
    }
}

fn activate_no_module_review_gate(
    core: &Core,
    runtime_target: ContentModuleRuntimeTarget,
) -> ModuleBindingId {
    let rules = no_module_review_gate_rule_set();
    core.upsert_interaction_rule_set(&rules, None)
        .expect("save no-module gate rules");
    let module = no_module_review_gate_module(&rules.id);
    core.upsert_content_module(&module, None)
        .expect("save no-module gate module");
    let binding_id = ModuleBindingId::from(NO_MODULE_GATE_BINDING_ID);
    let request = ContentModuleActivationRequest {
        runtime_target,
        expected_binding_revision: None,
        binding: ContentModuleBindingDraft {
            id: binding_id.clone(),
            module_id: module.id,
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
        .expect("review no-module gate activation");
    let resolutions = ModuleMergeResolutionSet {
        expected_review_sha256: review.review_sha256.clone(),
        resolutions: Vec::new(),
    };
    let plan = core
        .resolve_content_module_activation(&request, &resolutions)
        .expect("resolve no-module gate activation");
    core.activate_content_module(
        &request,
        &resolutions,
        &ModuleActivationApproval {
            approval_id: "synthetic-no-module-gate-activation".to_owned(),
            expected_review_sha256: review.review_sha256,
            expected_plan_sha256: plan.plan_sha256,
        },
    )
    .expect("activate no-module gate")
    .verify()
    .expect("verify no-module gate activation");
    binding_id
}

fn deactivate_no_module_review_gate(
    core: &Core,
    runtime_target: ContentModuleRuntimeTarget,
    binding_id: ModuleBindingId,
) {
    let request = ContentModuleDeactivationRequest {
        runtime_target,
        binding_id,
    };
    let review = core
        .review_content_module_deactivation(&request)
        .expect("review no-module gate deactivation");
    core.deactivate_content_module(&request, &review.review_sha256)
        .expect("deactivate no-module gate")
        .verify()
        .expect("verify no-module gate deactivation");
}

fn read_branch_interaction_policy(
    connection: &Connection,
    conversation_id: &lorepia_core::ConversationId,
    branch_id: &lorepia_core::ConversationBranchId,
) -> InteractionPolicySnapshot {
    let policy_json = connection
        .query_row(
            "SELECT snapshot.policy_json
             FROM generation_attempt_before_event_snapshots AS snapshot
             JOIN generation_attempt_intents AS attempt
               ON attempt.generation_id = snapshot.generation_id
             WHERE attempt.conversation_id = ?1
               AND attempt.source_branch_id = ?2
             ORDER BY snapshot.created_at DESC
             LIMIT 1",
            params![conversation_id.0.as_str(), branch_id.0.as_str()],
            |row| row.get::<_, String>(0),
        )
        .expect("read synthetic predecessor policy");
    serde_json::from_str(&policy_json).expect("decode synthetic predecessor policy")
}

fn read_branch_evaluation_seal(
    connection: &Connection,
    conversation_id: &lorepia_core::ConversationId,
    branch_id: &lorepia_core::ConversationBranchId,
) -> InteractionEvaluationSeal {
    let seal_json = connection
        .query_row(
            "SELECT snapshot.evaluation_seal_json
             FROM generation_attempt_before_event_snapshots AS snapshot
             JOIN generation_attempt_intents AS attempt
               ON attempt.generation_id = snapshot.generation_id
             WHERE attempt.conversation_id = ?1
               AND attempt.source_branch_id = ?2
             ORDER BY snapshot.created_at DESC
             LIMIT 1",
            params![conversation_id.0.as_str(), branch_id.0.as_str()],
            |row| row.get::<_, String>(0),
        )
        .expect("read synthetic predecessor evaluation seal");
    serde_json::from_str(&seal_json).expect("decode synthetic predecessor evaluation seal")
}

struct DeferredPredecessor {
    occurrence_id: String,
    parent_resulting_state_revision: u64,
    delivery_attempts: u64,
    available_at: chrono::DateTime<Utc>,
}

struct PredecessorCommitInput<'a> {
    snapshot: StoredInteractionState,
    policy: InteractionPolicySnapshot,
    revision_id: String,
    rule: &'a InteractionRule,
    action: &'a InteractionAction,
    fixture: &'a ClosureFixture,
    evaluation_seal: InteractionEvaluationSeal,
}

fn predecessor_event_commit(input: PredecessorCommitInput<'_>) -> InteractionEventCommit {
    let mut next_state = input.snapshot.state.clone();
    let value = VariableValue::Text("synthetic-pending-predecessor".to_owned());
    let previous = next_state
        .variables
        .get(&input.fixture.variables.approval_child)
        .cloned();
    next_state.variables.insert(
        input.fixture.variables.approval_child.clone(),
        value.clone(),
    );
    next_state.revision = next_state
        .revision
        .checked_add(1)
        .expect("predecessor state revision");
    let created_at = Utc::now();
    let mut evaluation_seal = input.evaluation_seal;
    evaluation_seal.event_epoch_seconds = created_at.timestamp();
    evaluation_seal.template_values.current_date = Some(created_at.format("%Y-%m-%d").to_string());
    evaluation_seal.template_values.current_time =
        Some(created_at.format("%H:%M:%S%:z").to_string());
    let event_id = "synthetic-generation-predecessor-root";
    let deterministic_seed = 0xA11C_E55E_0000_0001_u64;
    InteractionEventCommit {
        event_id: event_id.to_owned(),
        idempotency_key: format!("{event_id}-idempotency"),
        key: input.snapshot.key,
        expected_state_revision: input.snapshot.state.revision,
        event: input.rule.event.clone(),
        generation_attempt_id: None,
        owner_message_id: None,
        policy: input.policy,
        evaluation_seal: Some(evaluation_seal),
        deterministic_seed: Some(deterministic_seed),
        next_state,
        knowledge: input.snapshot.knowledge,
        action_results: vec![InteractionActionResultWrite {
            set_revision_id: input.revision_id.clone(),
            rule_id: input.rule.id.clone(),
            action_ordinal: 0,
            status: InteractionActionResultStatus::Applied,
            result: VersionedJson {
                schema_version: 1,
                value: json!({
                    "rule_status": "applied",
                    "state_changed": true,
                    "effect_count": 1,
                }),
            },
        }],
        effects: vec![InteractionEffect::VariableSet {
            target: input.fixture.variables.approval_child.clone(),
            previous,
            value,
        }],
        derived_events: vec![InteractionDerivedEventWrite {
            event: InteractionEvent::VariableChanged {
                variable: input.fixture.variables.approval_child.clone(),
            },
            source_set_revision_id: input.revision_id,
            source_rule_id: input.rule.id.clone(),
            source_action_ordinal: 0,
            source_effect_ordinal: 0,
            source_action_sha256: interaction_action_sha256(input.action)
                .expect("hash predecessor source action"),
            deterministic_seed: deterministic_seed.wrapping_add(1),
        }],
        proposals: Vec::new(),
        created_at,
    }
}

fn defer_predecessor(storage: &Storage) -> DeferredPredecessor {
    let claimed_at = Utc::now();
    let occurrence = storage
        .claim_interaction_derived_events(claimed_at, claimed_at + ChronoDuration::seconds(30), 1)
        .expect("claim synthetic predecessor")
        .pop()
        .expect("synthetic predecessor occurrence");
    let available_at = Utc::now() + ChronoDuration::minutes(5);
    storage
        .retry_interaction_derived_event_after(
            &occurrence.occurrence_id,
            occurrence.delivery_attempts,
            available_at,
        )
        .expect("defer synthetic predecessor beyond the E2E window");
    DeferredPredecessor {
        occurrence_id: occurrence.occurrence_id,
        parent_resulting_state_revision: occurrence.parent_resulting_state_revision,
        delivery_attempts: occurrence.delivery_attempts,
        available_at,
    }
}

fn seed_deferred_same_branch_predecessor(
    root: &std::path::Path,
    conversation_id: &lorepia_core::ConversationId,
    branch_id: &lorepia_core::ConversationBranchId,
    fixture: &ClosureFixture,
) -> DeferredPredecessor {
    let storage = Storage::open(root).expect("open predecessor fixture storage");
    let connection = Connection::open(active_database_path(root))
        .expect("open predecessor fixture evidence connection");
    let policy = read_branch_interaction_policy(&connection, conversation_id, branch_id);
    let revision_id = policy
        .rule_sets
        .iter()
        .find(|revision| revision.rule_set_id == fixture.rules.id)
        .map(|revision| revision.revision_id.clone())
        .expect("active derived-closure rule revision");
    let rule = fixture
        .rules
        .rules
        .iter()
        .find(|rule| {
            matches!(
                &rule.event,
                InteractionEvent::UserAction { action_id }
                    if action_id == "approve-first-child"
            )
        })
        .expect("synthetic predecessor source rule");
    let action = rule.actions.first().expect("synthetic predecessor action");
    let snapshot = storage
        .get_interaction_state_snapshot(conversation_id, branch_id)
        .expect("load predecessor source state");
    let evaluation_seal = read_branch_evaluation_seal(&connection, conversation_id, branch_id);
    storage
        .commit_interaction_event(&predecessor_event_commit(PredecessorCommitInput {
            snapshot,
            policy,
            revision_id,
            rule,
            action,
            fixture,
            evaluation_seal,
        }))
        .expect("commit synthetic pending predecessor root");
    defer_predecessor(&storage)
}

struct HistoricalLineageIds {
    first_user: MessageId,
    checkpoint_assistant: MessageId,
    second_user: MessageId,
    source_head: MessageId,
}

fn historical_lineage_ids() -> HistoricalLineageIds {
    HistoricalLineageIds {
        first_user: MessageId("synthetic-historical-barrier-user-one".to_owned()),
        checkpoint_assistant: MessageId(
            "synthetic-historical-barrier-assistant-checkpoint".to_owned(),
        ),
        second_user: MessageId("synthetic-historical-barrier-user-two".to_owned()),
        source_head: MessageId("synthetic-historical-barrier-assistant-head".to_owned()),
    }
}

fn insert_synthetic_historical_messages(
    transaction: &rusqlite::Transaction<'_>,
    conversation_id: &lorepia_core::ConversationId,
    ids: &HistoricalLineageIds,
    now: chrono::DateTime<Utc>,
) {
    for (id, parent_id, role, content, generation_id, offset) in [
        (
            &ids.first_user,
            None,
            "user",
            "SYNTHETIC_HISTORICAL_BARRIER_USER_ONE",
            None,
            0_i64,
        ),
        (
            &ids.checkpoint_assistant,
            Some(&ids.first_user),
            "assistant",
            "SYNTHETIC_HISTORICAL_BARRIER_ASSISTANT_ONE",
            Some("synthetic-historical-barrier-generation-one"),
            1,
        ),
        (
            &ids.second_user,
            Some(&ids.checkpoint_assistant),
            "user",
            "SYNTHETIC_HISTORICAL_BARRIER_USER_TWO",
            None,
            2,
        ),
        (
            &ids.source_head,
            Some(&ids.second_user),
            "assistant",
            "SYNTHETIC_HISTORICAL_BARRIER_ASSISTANT_TWO",
            Some("synthetic-historical-barrier-generation-two"),
            3,
        ),
    ] {
        transaction
            .execute(
                "INSERT INTO messages
                 (id, conversation_id, parent_id, role, content, status,
                  generation_id, created_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, 'complete', ?6, ?7)",
                params![
                    id.0.as_str(),
                    conversation_id.0.as_str(),
                    parent_id.map(|id| id.0.as_str()),
                    role,
                    content,
                    generation_id,
                    (now + ChronoDuration::seconds(offset)).to_rfc3339(),
                ],
            )
            .expect("insert synthetic historical message");
    }
}

fn insert_synthetic_historical_checkpoint(
    transaction: &rusqlite::Transaction<'_>,
    conversation_id: &lorepia_core::ConversationId,
    branch_id: &lorepia_core::ConversationBranchId,
    checkpoint: &StoredInteractionState,
    checkpoint_assistant: &MessageId,
    now: chrono::DateTime<Utc>,
) {
    let mut knowledge = checkpoint.knowledge.clone();
    knowledge.sort();
    let state_json =
        serde_json::to_string(&checkpoint.state).expect("encode synthetic checkpoint state");
    let knowledge_json =
        serde_json::to_string(&knowledge).expect("encode synthetic checkpoint knowledge");
    let checkpoint_sha256 = interaction_state_snapshot_sha256(&checkpoint.state, &knowledge)
        .expect("hash synthetic historical checkpoint");
    transaction
        .execute(
            "INSERT INTO interaction_state_checkpoints
             (conversation_id, branch_id, message_id,
              source_interaction_state_id, state_revision,
              state_document_json, knowledge_bindings_json,
              checkpoint_sha256, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            params![
                conversation_id.0.as_str(),
                branch_id.0.as_str(),
                checkpoint_assistant.0.as_str(),
                checkpoint.key.state_id,
                i64::try_from(checkpoint.state.revision)
                    .expect("synthetic checkpoint revision fits i64"),
                state_json,
                knowledge_json,
                checkpoint_sha256,
                (now + ChronoDuration::seconds(1)).to_rfc3339(),
            ],
        )
        .expect("insert synthetic historical interaction checkpoint");
}

fn install_synthetic_historical_lineage(
    root: &std::path::Path,
    conversation_id: &lorepia_core::ConversationId,
    branch_id: &lorepia_core::ConversationBranchId,
    checkpoint: &StoredInteractionState,
) -> (MessageId, MessageId) {
    let ids = historical_lineage_ids();
    let now = Utc::now();
    let mut connection = Connection::open(active_database_path(root))
        .expect("open synthetic historical lineage connection");
    let transaction = connection
        .transaction()
        .expect("begin synthetic historical lineage transaction");
    insert_synthetic_historical_messages(&transaction, conversation_id, &ids, now);
    transaction
        .execute(
            "UPDATE conversation_branches
             SET head_message_id = ?3, updated_at = ?4
             WHERE conversation_id = ?1 AND id = ?2",
            params![
                conversation_id.0.as_str(),
                branch_id.0.as_str(),
                ids.source_head.0.as_str(),
                (now + ChronoDuration::seconds(3)).to_rfc3339(),
            ],
        )
        .expect("advance synthetic historical source head");
    insert_synthetic_historical_checkpoint(
        &transaction,
        conversation_id,
        branch_id,
        checkpoint,
        &ids.checkpoint_assistant,
        now,
    );
    transaction
        .commit()
        .expect("commit synthetic historical lineage");
    (ids.source_head.clone(), ids.source_head)
}

struct HistoricalPredecessorFixture {
    root: TempDir,
    conversation_id: lorepia_core::ConversationId,
    branch_id: lorepia_core::ConversationBranchId,
    source_head: MessageId,
    target_assistant: MessageId,
    target: GenerationTarget,
    predecessor: DeferredPredecessor,
    provider_requests: mpsc::Receiver<Vec<u8>>,
    provider: thread::JoinHandle<()>,
}

fn prepare_historical_predecessor_fixture(
    checkpoint_includes_predecessor: bool,
) -> HistoricalPredecessorFixture {
    let root = tempdir().expect("create historical predecessor Core root");
    let (origin, requests, provider) = spawn_provider(0);
    let core = Core::open(CoreConfig::new(root.path())).expect("open historical predecessor Core");
    let character_id = import_character(&core);
    let target = provider_fixture(&core, &origin);
    let core = reopen_with_provider_credential_authority(core, root.path());
    core.select_generation_target(Some(target.clone()))
        .expect("select historical predecessor target");
    set_json_mode_capability(&core, &target.model_route_id, true);
    let conversation = core
        .open_conversation(&character_id)
        .expect("open historical predecessor conversation");
    let branch = core
        .list_conversation_branches(&conversation.id)
        .expect("list historical predecessor branches")
        .into_iter()
        .next()
        .expect("historical predecessor root branch");
    assert!(
        core.drain_core_lifecycle_occurrences(64)
            .expect("initialize historical predecessor state")
            .queue_idle
    );
    let fixture = install_closure_fixture(
        &core,
        ContentModuleRuntimeTarget {
            conversation_id: conversation.id.clone(),
            branch_id: branch.id.clone(),
        },
    );
    let room_config = core
        .get_room_orchestration_config(&conversation.id, &branch.id)
        .expect("load historical predecessor room");
    save_room_context(&core, &room_config, &target, SEALED_USER, SEALED_SLOT);
    let bootstrap = core
        .resolve_prompt_preview(
            &PromptPlanRequest {
                conversation_id: conversation.id.clone(),
                branch_id: branch.id.clone(),
                expected_head: None,
                user_text: "Synthetic historical predecessor authority bootstrap".to_owned(),
                generation_target: target.clone(),
                prompt_preset_id: None,
                variable_overrides: VariableMap::default(),
                expected_plan_hash: None,
            },
            GenerationOperationContext::New {
                operation_nonce: "historical-predecessor-bootstrap-v1",
            },
        )
        .expect_err("historical bootstrap must stop at approval");
    assert_eq!(
        bootstrap.code,
        CoreErrorCode::PermissionDenied,
        "unexpected historical bootstrap failure: {bootstrap:?}"
    );
    drop(core);

    let storage = Storage::open(root.path()).expect("open pre-predecessor boundary storage");
    let boundary_before = storage
        .get_interaction_state_snapshot(&conversation.id, &branch.id)
        .expect("capture pre-predecessor boundary");
    drop(storage);
    let predecessor =
        seed_deferred_same_branch_predecessor(root.path(), &conversation.id, &branch.id, &fixture);
    let storage = Storage::open(root.path()).expect("open post-predecessor boundary storage");
    let boundary_after = storage
        .get_interaction_state_snapshot(&conversation.id, &branch.id)
        .expect("capture post-predecessor boundary");
    drop(storage);
    assert_eq!(
        boundary_after.state.revision,
        predecessor.parent_resulting_state_revision
    );
    assert!(boundary_before.state.revision < boundary_after.state.revision);
    let selected_checkpoint = if checkpoint_includes_predecessor {
        &boundary_after
    } else {
        &boundary_before
    };
    let (source_head, target_assistant) = install_synthetic_historical_lineage(
        root.path(),
        &conversation.id,
        &branch.id,
        selected_checkpoint,
    );
    HistoricalPredecessorFixture {
        root,
        conversation_id: conversation.id,
        branch_id: branch.id,
        source_head,
        target_assistant,
        target,
        predecessor,
        provider_requests: requests,
        provider,
    }
}

fn read_http_request(stream: &mut TcpStream) -> Vec<u8> {
    stream
        .set_read_timeout(Some(Duration::from_secs(5)))
        .expect("set provider request timeout");
    let mut request = Vec::new();
    let mut buffer = [0_u8; 4_096];
    loop {
        let read = stream.read(&mut buffer).expect("read provider request");
        if read == 0 {
            break;
        }
        request.extend_from_slice(&buffer[..read]);
        let Some(header_end) = request.windows(4).position(|bytes| bytes == b"\r\n\r\n") else {
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
            return request;
        }
    }
    request
}

fn spawn_provider(
    request_count: usize,
) -> (
    CanonicalOrigin,
    mpsc::Receiver<Vec<u8>>,
    thread::JoinHandle<()>,
) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind synthetic provider");
    let address = listener
        .local_addr()
        .expect("read synthetic provider address");
    let (sender, receiver) = mpsc::channel();
    let handle = thread::spawn(move || {
        for _ in 0..request_count {
            let (mut stream, _) = listener
                .accept()
                .expect("accept synthetic provider request");
            sender
                .send(read_http_request(&mut stream))
                .expect("capture synthetic provider request");
            let body = concat!(
                "data: {\"choices\":[{\"index\":0,\"delta\":{\"content\":\"Synthetic closure reply\"}}]}\n\n",
                "data: {\"choices\":[{\"index\":0,\"delta\":{},\"finish_reason\":\"stop\"}]}\n\n",
                "data: [DONE]\n\n"
            );
            write!(
                stream,
                "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                body.len(),
                body
            )
            .expect("write synthetic provider response");
        }
    });
    (
        CanonicalOrigin::parse(&format!("http://{address}"))
            .expect("parse synthetic provider origin"),
        receiver,
        handle,
    )
}

fn provider_fixture(core: &Core, origin: &CanonicalOrigin) -> GenerationTarget {
    let template = core
        .list_provider_templates()
        .expect("list provider templates")
        .into_iter()
        .find(|template| template.id.as_str() == "openai-chat-compatible-v1")
        .expect("OpenAI-compatible provider template");
    assert_eq!(template.api_family, ApiFamily::OpenAiChatCompletions);
    let connection = core
        .create_provider_connection(ProviderConnectionDraft {
            id: ProviderConnectionId::from(CONNECTION_ID),
            template_id: template.id,
            template_version: template.manifest_version,
            display_name: "Synthetic derived closure provider".to_owned(),
            api_origin: origin.clone(),
            api_base_path: Some(EndpointPath::parse("/v1").expect("parse provider API path")),
            network_mode: ProviderNetworkMode::LocalLoopback,
            local_network_approval: None,
            values: vec![ConnectionConfigEntry {
                key: "api_base_url".to_owned(),
                value: ConnectionConfigValue::Text(format!("{}/v1", origin.as_str())),
            }],
            approved_credential_origin: Some(origin.clone()),
            timeout_seconds: 5,
        })
        .expect("create synthetic provider connection");
    let now = Utc::now();
    let route = core
        .upsert_model_route(ModelRoute {
            id: ModelRouteId::from("synthetic-derived-closure-route"),
            connection_id: connection.id,
            api_family: ApiFamily::OpenAiChatCompletions,
            model_id: "synthetic-derived-closure-model".to_owned(),
            display_name: Some("Synthetic derived closure model".to_owned()),
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
            id: "synthetic-derived-closure-preset".into(),
            model_route_id: route.id.clone(),
            display_name: "Synthetic derived closure preset".to_owned(),
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

fn install_provider_credential_authority(root: &Path, connection_id: &ProviderConnectionId) {
    let storage = Storage::open(root).expect("open credential-authority fixture storage");
    let authority = storage
        .propose_provider_credential_install_authority(connection_id)
        .expect("propose synthetic credential install authority");
    let install = storage
        .prepare_provider_credential_operation_with_install_authority(
            connection_id,
            ProviderCredentialOperationKind::Install,
            ProviderCredentialObservedStatus::Missing,
            Some(&authority),
        )
        .expect("prepare synthetic credential install");
    storage
        .start_provider_credential_operation(&install.plan.operation_id, &install.plan_sha256)
        .expect("start synthetic credential install");
    storage
        .finish_provider_credential_operation(
            &install.plan.operation_id,
            &install.plan_sha256,
            ProviderCredentialObservedStatus::Available,
        )
        .expect("finish synthetic credential install");
    let authority = storage
        .ensure_provider_credential_access_settled(connection_id)
        .expect("read synthetic credential access authority");
    assert_eq!(authority.authority_id, install.plan.operation_id);
}

fn reopen_with_provider_credential_authority(core: Core, root: &Path) -> Core {
    drop(core);
    install_provider_credential_authority(root, &ProviderConnectionId::from(CONNECTION_ID));
    open_core_after_drop(root)
}

fn set_json_mode_capability(core: &Core, route_id: &ModelRouteId, supported: bool) {
    core.upsert_user_capability_override(CapabilityObservation {
        id: ObservationId::from("synthetic-derived-closure-json-mode"),
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
    .expect("save synthetic JsonMode capability override");
}

fn import_character(core: &Core) -> String {
    let mut source = NamedTempFile::new().expect("create synthetic character source");
    write!(
        source,
        r#"{{"spec":"chara_card_v3","data":{{"name":"Closure Ari","description":"Entirely synthetic derived-closure character."}}}}"#
    )
    .expect("write synthetic character source");
    let review = core
        .inspect_import(source.path())
        .expect("inspect character");
    core.commit_import(&review.id)
        .expect("commit synthetic character")
        .id
}

fn import_authority_character(core: &Core) -> String {
    let mut source = NamedTempFile::new().expect("create synthetic authority character source");
    write!(
        source,
        r#"{{"spec":"chara_card_v3","data":{{"name":"Closure Ari","description":"{SEALED_CHARACTER_DESCRIPTION}","personality":"{SEALED_CHARACTER_PERSONALITY}","character_book":{{"id":"{CHARACTER_KNOWLEDGE_BOOK_ID}","name":"Synthetic sealed character knowledge","entries":[]}}}}}}"#
    )
    .expect("write synthetic authority character source");
    let review = core
        .inspect_import(source.path())
        .expect("inspect authority character");
    core.commit_import(&review.id)
        .expect("commit synthetic authority character")
        .id
}

fn authority_character_knowledge() -> KnowledgeBook {
    let book_id = KnowledgeBookId::from(CHARACTER_KNOWLEDGE_BOOK_ID);
    KnowledgeBook {
        id: book_id.clone(),
        name: "Synthetic sealed character knowledge".to_owned(),
        schema_version: 1,
        entries: vec![KnowledgeEntry {
            id: KnowledgeEntryId::from("synthetic.derived-closure.character-knowledge.entry"),
            book_id,
            name: "Synthetic sealed character knowledge entry".to_owned(),
            content: SEALED_CHARACTER_KNOWLEDGE.to_owned(),
            enabled: true,
            activation: ActivationRule::Always,
            priority: 100,
            importance: 100,
            placement: KnowledgePlacement::RetrievedContext,
            token_policy: TokenPolicy {
                priority: 100,
                min_tokens: None,
                max_tokens: Some(64),
                reserve_tokens: None,
            },
            parent_id: None,
            activation_probability_basis_points: 10_000,
            provenance: provenance("synthetic.derived-closure.character-knowledge.entry"),
        }],
        scan_depth: 8,
        token_budget: TokenBudget { max_tokens: 128 },
        recursive: false,
        max_recursion_depth: 0,
        provenance: provenance("synthetic.derived-closure.character-knowledge"),
    }
}

fn install_synthetic_authority_turn(
    root: &std::path::Path,
    conversation_id: &lorepia_core::ConversationId,
    branch_id: &lorepia_core::ConversationBranchId,
) -> (MessageId, MessageId) {
    let user_id = MessageId("synthetic-authority-baseline-user".to_owned());
    let assistant_id = MessageId("synthetic-authority-baseline-assistant".to_owned());
    let now = Utc::now();
    let mut connection = Connection::open(active_database_path(root))
        .expect("open synthetic authority lineage connection");
    let transaction = connection
        .transaction()
        .expect("begin synthetic authority lineage transaction");
    transaction
        .execute(
            "INSERT INTO messages
             (id, conversation_id, parent_id, role, content, status,
              generation_id, created_at)
             VALUES (?1, ?2, NULL, 'user', ?3, 'complete', NULL, ?4)",
            params![
                user_id.0.as_str(),
                conversation_id.0.as_str(),
                "SYNTHETIC_AUTHORITY_BASELINE_USER",
                now.to_rfc3339(),
            ],
        )
        .expect("insert synthetic authority user message");
    transaction
        .execute(
            "INSERT INTO messages
             (id, conversation_id, parent_id, role, content, status,
              generation_id, created_at)
             VALUES (?1, ?2, ?3, 'assistant', ?4, 'complete', ?5, ?6)",
            params![
                assistant_id.0.as_str(),
                conversation_id.0.as_str(),
                user_id.0.as_str(),
                "SYNTHETIC_AUTHORITY_BASELINE_ASSISTANT",
                "synthetic-authority-baseline-generation",
                (now + ChronoDuration::seconds(1)).to_rfc3339(),
            ],
        )
        .expect("insert synthetic authority assistant message");
    assert_eq!(
        transaction
            .execute(
                "UPDATE conversation_branches
                 SET head_message_id = ?3, updated_at = ?4
                 WHERE conversation_id = ?1 AND id = ?2",
                params![
                    conversation_id.0.as_str(),
                    branch_id.0.as_str(),
                    assistant_id.0.as_str(),
                    (now + ChronoDuration::seconds(1)).to_rfc3339(),
                ],
            )
            .expect("advance synthetic authority branch head"),
        1
    );
    transaction
        .commit()
        .expect("commit synthetic authority lineage");
    (user_id, assistant_id)
}

struct AuthorityMemorySource<'a> {
    conversation: &'a lorepia_core::ConversationId,
    branch: &'a lorepia_core::ConversationBranchId,
    start_message: &'a MessageId,
    end_message: &'a MessageId,
}

fn synthetic_authority_memory(
    id: &str,
    kind: MemoryKind,
    summary: &str,
    pinned: bool,
    source: &AuthorityMemorySource<'_>,
) -> MemoryRecord {
    let now = Utc::now();
    MemoryRecord {
        id: MemoryRecordId::from(id),
        conversation_id: source.conversation.clone(),
        branch_id: source.branch.clone(),
        source_start_message_id: source.start_message.clone(),
        source_end_message_id: source.end_message.clone(),
        kind,
        title: format!("Synthetic authority memory {id}"),
        summary: summary.to_owned(),
        structured_data: VersionedJson {
            schema_version: 1,
            value: json!({"fixture": id}),
        },
        importance: 100,
        keywords: vec!["synthetic-authority".to_owned()],
        embedding_ref: None,
        pinned,
        excluded_from_conversation: false,
        excluded_from_character: false,
        created_at: now,
        updated_at: now,
        invalidated_at: None,
        provenance: provenance(id),
    }
}

struct AuthorityPromptBlockSpec<'a> {
    id: &'a str,
    name: &'a str,
    kind: PromptBlockKind,
    source: BlockSource,
    placement_zone: PlacementZone,
    role_hint: RoleHint,
    history_selector: Option<HistorySelector>,
    overflow_policy: OverflowPolicy,
}

fn authority_prompt_block(spec: AuthorityPromptBlockSpec<'_>) -> PromptBlock {
    PromptBlock {
        id: PromptBlockId::from(spec.id),
        name: spec.name.to_owned(),
        kind: spec.kind,
        enabled: true,
        role_hint: spec.role_hint,
        authority: InstructionAuthority::Creator,
        template: None,
        condition: None,
        source: spec.source,
        placement_zone: spec.placement_zone,
        history_selector: spec.history_selector,
        token_policy: TokenPolicy {
            priority: 1_000,
            min_tokens: None,
            max_tokens: Some(1_024),
            reserve_tokens: None,
        },
        overflow_policy: spec.overflow_policy,
        merge_policy: MergePolicy::SeparateMessage,
        provenance: provenance(spec.id),
    }
}

fn install_authority_memory_profile(core: &Core, target: &GenerationTarget) -> MemoryProfileId {
    let summary_task_id = TaskProfileId::from("synthetic.derived-closure.summary-task");
    core.upsert_task_profile(
        &TaskProfile {
            id: summary_task_id.clone(),
            kind: AuxiliaryTaskKind::MemorySummary,
            route_id: target.model_route_id.clone(),
            generation_preset_id: target.generation_preset_id.clone(),
            fallback_route_ids: Vec::new(),
            embedding_dimensions: None,
            timeout_ms: 5_000,
            rate_limit: RateLimit {
                requests: 100,
                per_seconds: 60,
            },
            concurrency_limit: 1,
        },
        None,
    )
    .expect("save synthetic authority memory task");
    let memory_profile_id =
        MemoryProfileId::from("synthetic.derived-closure.authority-memory-profile");
    core.upsert_memory_profile(
        &MemoryProfile {
            id: memory_profile_id.clone(),
            name: "Synthetic authority memory profile".to_owned(),
            schema_version: 1,
            summary_task: summary_task_id,
            embedding_task: None,
            turns_per_summary: 100,
            recent_raw_budget: TokenBudget { max_tokens: 1_024 },
            episodic_budget: TokenBudget { max_tokens: 1_024 },
            semantic_budget: TokenBudget { max_tokens: 1_024 },
            retrieval_count: 16,
            recency_weight: 1.0,
            similarity_weight: 1.0,
            importance_weight: 1.0,
            preserve_invalidated_records: true,
            summary_schema: SummarySchemaId::from(
                "synthetic.derived-closure.authority-summary-schema",
            ),
            provenance: provenance("synthetic.derived-closure.authority-memory-profile"),
        },
        None,
    )
    .expect("save synthetic authority memory profile");
    memory_profile_id
}

fn authority_prompt_blocks() -> Vec<PromptBlock> {
    let mut latest_user = authority_prompt_block(AuthorityPromptBlockSpec {
        id: "synthetic.derived-closure.authority-preset.latest-user",
        name: "Synthetic latest user",
        kind: PromptBlockKind::LatestUserTurn,
        source: BlockSource::LatestUser,
        placement_zone: PlacementZone::LatestUser,
        role_hint: RoleHint::User,
        history_selector: None,
        overflow_policy: OverflowPolicy::Reject,
    });
    latest_user.token_policy.min_tokens = Some(1);
    latest_user.token_policy.max_tokens = None;
    vec![
        authority_prompt_block(AuthorityPromptBlockSpec {
            id: "synthetic.derived-closure.authority-preset.character-description",
            name: "Synthetic character description",
            kind: PromptBlockKind::CharacterDescription,
            source: BlockSource::CharacterField {
                field: CharacterField::Description,
            },
            placement_zone: PlacementZone::CharacterContext,
            role_hint: RoleHint::User,
            history_selector: None,
            overflow_policy: OverflowPolicy::DropBlock,
        }),
        authority_prompt_block(AuthorityPromptBlockSpec {
            id: "synthetic.derived-closure.authority-preset.character-personality",
            name: "Synthetic character personality",
            kind: PromptBlockKind::CharacterPersonality,
            source: BlockSource::CharacterField {
                field: CharacterField::Personality,
            },
            placement_zone: PlacementZone::CharacterContext,
            role_hint: RoleHint::User,
            history_selector: None,
            overflow_policy: OverflowPolicy::DropBlock,
        }),
        authority_prompt_block(AuthorityPromptBlockSpec {
            id: "synthetic.derived-closure.authority-preset.persona",
            name: "Synthetic user persona",
            kind: PromptBlockKind::UserPersona,
            source: BlockSource::UserPersona,
            placement_zone: PlacementZone::CharacterContext,
            role_hint: RoleHint::User,
            history_selector: None,
            overflow_policy: OverflowPolicy::DropBlock,
        }),
        authority_prompt_block(AuthorityPromptBlockSpec {
            id: "synthetic.derived-closure.authority-preset.selected-memory",
            name: "Synthetic selected memory",
            kind: PromptBlockKind::RetrievedMemory,
            source: BlockSource::SelectedMemory,
            placement_zone: PlacementZone::RetrievedContext,
            role_hint: RoleHint::System,
            history_selector: None,
            overflow_policy: OverflowPolicy::TrimTail,
        }),
        authority_prompt_block(AuthorityPromptBlockSpec {
            id: "synthetic.derived-closure.authority-preset.conversation-summary",
            name: "Synthetic conversation summary",
            kind: PromptBlockKind::ConversationSummary,
            source: BlockSource::ConversationSummary,
            placement_zone: PlacementZone::OlderHistory,
            role_hint: RoleHint::System,
            history_selector: None,
            overflow_policy: OverflowPolicy::TrimTail,
        }),
        authority_prompt_block(AuthorityPromptBlockSpec {
            id: "synthetic.derived-closure.authority-preset.history",
            name: "Synthetic conversation history",
            kind: PromptBlockKind::HistorySlice,
            source: BlockSource::History,
            placement_zone: PlacementZone::RecentHistory,
            role_hint: RoleHint::ProviderDefault,
            history_selector: Some(HistorySelector::All),
            overflow_policy: OverflowPolicy::KeepLatestItems,
        }),
        latest_user,
    ]
}

fn install_authority_prompt_preset(
    core: &Core,
    target: &GenerationTarget,
) -> Revisioned<PromptPreset> {
    let memory_profile_id = install_authority_memory_profile(core, target);
    let now = Utc::now();
    core.upsert_prompt_preset(
        &PromptPreset {
            id: PromptPresetId::from("synthetic.derived-closure.authority-preset"),
            name: "Synthetic sealed authority preset".to_owned(),
            schema_version: 1,
            blocks: authority_prompt_blocks(),
            controls: Vec::new(),
            default_values: VariableMap::default(),
            default_generation_preset_id: Some(target.generation_preset_id.clone()),
            memory_profile_id: Some(memory_profile_id),
            knowledge_book_ids: Vec::new(),
            transform_set_ids: Vec::new(),
            module_ids: Vec::new(),
            cache_boundaries: Vec::new(),
            metadata: PresetMetadata {
                description: "Synthetic approval-pause prompt authority fixture".to_owned(),
                tags: vec!["synthetic".to_owned()],
                provenance: provenance("synthetic.derived-closure.authority-preset"),
                created_at: now,
                updated_at: now,
                local_override_of: None,
            },
        },
        None,
    )
    .expect("save synthetic authority prompt preset")
}

fn save_room_context(
    core: &Core,
    current: &RoomOrchestrationConfig,
    target: &GenerationTarget,
    user_name: &str,
    slot: &str,
) -> RoomOrchestrationConfig {
    core.save_room_orchestration_config(
        &current.conversation_id,
        &current.branch_id,
        current.binding_revision,
        &RoomOrchestrationConfigPatch {
            prompt_preset_id: Some(current.prompt_preset_id.clone()),
            generation_preset_id: Some(target.generation_preset_id.clone()),
            creator_values: current.creator_values.clone(),
            response_length: current.response_length,
            creativity: current.creativity,
            reasoning_effort: current.reasoning_effort,
            memory_enabled: current.memory_enabled,
            knowledge_enabled: current.knowledge_enabled,
            user_name_override: Some(user_name.to_owned()),
            author_note: current.author_note.clone(),
            group_context: current.group_context.clone(),
            template_slots: vec![TemplateSlot {
                name: "closure_context".to_owned(),
                value: slot.to_owned(),
            }],
        },
    )
    .expect("save synthetic room context")
}

struct AuthorityRoomSettings<'a> {
    prompt_preset_id: &'a PromptPresetId,
    response_length: PromptResponseLength,
    creativity: u8,
    reasoning_effort: Option<GenerationReasoningEffort>,
    memory_enabled: bool,
    knowledge_enabled: bool,
    user_name: &'a str,
    slot: &'a str,
}

fn save_authority_room_context(
    core: &Core,
    current: &RoomOrchestrationConfig,
    target: &GenerationTarget,
    settings: &AuthorityRoomSettings<'_>,
) -> RoomOrchestrationConfig {
    core.save_room_orchestration_config(
        &current.conversation_id,
        &current.branch_id,
        current.binding_revision,
        &RoomOrchestrationConfigPatch {
            prompt_preset_id: Some(settings.prompt_preset_id.clone()),
            generation_preset_id: Some(target.generation_preset_id.clone()),
            creator_values: current.creator_values.clone(),
            response_length: settings.response_length,
            creativity: settings.creativity,
            reasoning_effort: settings.reasoning_effort,
            memory_enabled: settings.memory_enabled,
            knowledge_enabled: settings.knowledge_enabled,
            user_name_override: Some(settings.user_name.to_owned()),
            author_note: current.author_note.clone(),
            group_context: current.group_context.clone(),
            template_slots: vec![TemplateSlot {
                name: "closure_context".to_owned(),
                value: settings.slot.to_owned(),
            }],
        },
    )
    .expect("save synthetic authority room context")
}

fn mutate_live_local_user(root: &std::path::Path) -> LocalUserId {
    let drifted_local_user_id = LocalUserId::new();
    let connection = Connection::open(active_database_path(root))
        .expect("open synthetic local-user drift connection");
    let settings_json = connection
        .query_row(
            "SELECT value_json FROM app_settings WHERE key = 'application'",
            [],
            |row| row.get::<_, String>(0),
        )
        .expect("read synthetic application settings");
    let mut settings: serde_json::Value =
        serde_json::from_str(&settings_json).expect("decode synthetic application settings");
    settings
        .as_object_mut()
        .expect("application settings object")
        .insert(
            "local_user_id".to_owned(),
            serde_json::Value::String(drifted_local_user_id.as_str().to_owned()),
        );
    assert_eq!(
        connection
            .execute(
                "UPDATE app_settings SET value_json = ?1 WHERE key = 'application'",
                [serde_json::to_string(&settings).expect("encode drifted application settings")],
            )
            .expect("drift synthetic local user identity"),
        1
    );
    drifted_local_user_id
}

fn mutate_live_local_user_and_character(root: &std::path::Path, character_id: &str) -> LocalUserId {
    let drifted_local_user_id = mutate_live_local_user(root);
    let connection = Connection::open(active_database_path(root))
        .expect("open synthetic live-character drift connection");
    assert_eq!(
        connection
            .execute(
                "UPDATE characters
                 SET name = 'Drifted Closure Ari', description = ?2
                 WHERE id = ?1",
                params![character_id, DRIFTED_CHARACTER_DESCRIPTION],
            )
            .expect("drift synthetic live character head"),
        1
    );
    drifted_local_user_id
}

fn credential(root: &Path) -> ConnectionBoundCredential {
    let connection_id = ProviderConnectionId::from(CONNECTION_ID);
    let authority = Connection::open(active_database_path(root))
        .expect("open synthetic credential ownership projection")
        .query_row(
            "SELECT authority_id, connection_binding_sha256
             FROM provider_credential_ownership
             WHERE connection_id = ?1
               AND credential_ref = ?1
               AND ownership_state = 'ordinary_owned'",
            [connection_id.as_str()],
            |row| {
                Ok(ProviderCredentialAccessAuthority {
                    authority_id: row.get(0)?,
                    connection_binding_sha256: row.get(1)?,
                })
            },
        )
        .expect("load current synthetic credential access authority");
    ConnectionBoundCredential::new_with_access_authority(
        connection_id,
        Some("synthetic-derived-closure-credential".to_owned()),
        authority,
    )
}

fn preview_text(preview: &lorepia_core::ExpertPromptPreview) -> String {
    preview
        .effective_messages
        .iter()
        .map(|message| message.content.as_str())
        .collect::<Vec<_>>()
        .join("\n")
}

fn wait_for_generation(
    core: &Core,
    branch_id: &lorepia_core::ConversationBranchId,
    id: &GenerationId,
) {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let messages = core
            .list_branch_messages(branch_id)
            .expect("read messages while waiting for generation");
        if let Some(assistant) = messages
            .iter()
            .find(|message| message.generation_id.as_ref() == Some(id))
            && assistant.status != MessageStatus::Pending
        {
            assert_eq!(assistant.status, MessageStatus::Complete);
            return;
        }
        assert!(
            Instant::now() < deadline,
            "synthetic derived-closure generation did not finish"
        );
        thread::sleep(Duration::from_millis(10));
    }
}

fn request_body(request: &[u8]) -> serde_json::Value {
    let header_end = request
        .windows(4)
        .position(|bytes| bytes == b"\r\n\r\n")
        .expect("provider request header terminator");
    serde_json::from_slice(&request[header_end + 4..]).expect("decode provider request JSON")
}

fn assert_deferred_predecessor_unchanged(
    root: &std::path::Path,
    predecessor: &DeferredPredecessor,
) {
    let connection = Connection::open(active_database_path(root))
        .expect("open deferred predecessor evidence connection");
    let stored_occurrence = connection
        .query_row(
            "SELECT status, delivery_attempts, available_at,
                    parent_resulting_state_revision
             FROM interaction_derived_event_outbox
             WHERE occurrence_id = ?1",
            [predecessor.occurrence_id.as_str()],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, u64>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, u64>(3)?,
                ))
            },
        )
        .expect("read unchanged deferred predecessor occurrence");
    assert_eq!(stored_occurrence.0, "pending");
    assert_eq!(stored_occurrence.1, predecessor.delivery_attempts);
    assert_eq!(
        chrono::DateTime::parse_from_rfc3339(&stored_occurrence.2)
            .expect("parse deferred predecessor availability")
            .with_timezone(&Utc),
        predecessor.available_at
    );
    assert_eq!(
        stored_occurrence.3,
        predecessor.parent_resulting_state_revision
    );
}

fn generation_attempt_evidence_counts(
    root: &std::path::Path,
    conversation_id: &lorepia_core::ConversationId,
) -> (u64, u64) {
    let connection = Connection::open(active_database_path(root))
        .expect("open generation-attempt evidence connection");
    let before_snapshot_count = connection
        .query_row(
            "SELECT COUNT(*)
             FROM generation_attempt_before_event_snapshots AS snapshot
             JOIN generation_attempt_intents AS attempt
               ON attempt.generation_id = snapshot.generation_id
             WHERE attempt.conversation_id = ?1",
            [conversation_id.0.as_str()],
            |row| row.get::<_, u64>(0),
        )
        .expect("count generation-attempt BeforeGeneration snapshots");
    let dispatched_count = connection
        .query_row(
            "SELECT COUNT(*)
             FROM generation_attempt_intents
             WHERE conversation_id = ?1
               AND status IN ('running', 'completed')",
            [conversation_id.0.as_str()],
            |row| row.get::<_, u64>(0),
        )
        .expect("count dispatched generation attempts");
    (before_snapshot_count, dispatched_count)
}

fn assert_generation_prompt_authority_json_canonical(
    root: &std::path::Path,
    generation_id: &GenerationId,
) {
    let connection = Connection::open(active_database_path(root))
        .expect("open prompt-authority canonical evidence connection");
    let stored_json = connection
        .query_row(
            "SELECT prompt_selection_authority_json
             FROM generation_attempt_intents
             WHERE generation_id = ?1",
            [generation_id.0.as_str()],
            |row| row.get::<_, String>(0),
        )
        .expect("read stored prompt-selection authority JSON");
    let decoded: lorepia_storage::GenerationPromptSelectionAuthority =
        serde_json::from_str(&stored_json).expect("decode prompt-selection authority JSON");
    let canonical =
        serde_json::to_string(&decoded).expect("canonicalize prompt-selection authority JSON");
    assert_eq!(
        stored_json, canonical,
        "generation prompt-selection authority must remain canonical"
    );
}

#[test]
fn historical_checkpoint_predecessor_blocks_fork_before_review_and_dispatch() {
    let fixture = prepare_historical_predecessor_fixture(true);
    let core = Core::open(CoreConfig::new(fixture.root.path()))
        .expect("reopen historical predecessor Core");
    let blocked = core
        .regenerate_assistant_message_with_connection_credential(
            &fixture.conversation_id,
            &fixture.branch_id,
            Some(&fixture.source_head),
            &fixture.target_assistant,
            GenerationOperationContext::New {
                operation_nonce: "historical-checkpoint-blocked-regenerate-v1",
            },
            &fixture.target,
            credential(fixture.root.path()),
        )
        .expect_err("checkpoint predecessor at or before R must block the historical fork");
    assert_eq!(blocked.code, CoreErrorCode::InvalidInput);
    assert!(blocked.recoverable);
    assert_eq!(
        blocked.message,
        "a predecessor derived interaction occurrence must be drained first"
    );
    drop(core);
    fixture
        .provider
        .join()
        .expect("join blocked historical zero-call provider probe");
    assert!(
        fixture.provider_requests.try_recv().is_err(),
        "the blocked historical attempt must not call the provider"
    );
    assert_deferred_predecessor_unchanged(fixture.root.path(), &fixture.predecessor);
    let (before_snapshot_count, dispatched_count) =
        generation_attempt_evidence_counts(fixture.root.path(), &fixture.conversation_id);
    assert_eq!(
        before_snapshot_count, 1,
        "the blocked fork must not persist a second BeforeGeneration snapshot"
    );
    assert_eq!(dispatched_count, 0);
}

#[test]
fn future_source_occurrence_does_not_block_older_historical_checkpoint() {
    let fixture = prepare_historical_predecessor_fixture(false);
    assert!(
        fixture.predecessor.parent_resulting_state_revision > 0,
        "the future occurrence must have a concrete parent revision"
    );
    let core = Core::open(CoreConfig::new(fixture.root.path()))
        .expect("reopen future-occurrence historical Core");
    let approval = core
        .regenerate_assistant_message_with_connection_credential(
            &fixture.conversation_id,
            &fixture.branch_id,
            Some(&fixture.source_head),
            &fixture.target_assistant,
            GenerationOperationContext::New {
                operation_nonce: "historical-future-source-regenerate-v1",
            },
            &fixture.target,
            credential(fixture.root.path()),
        )
        .expect_err("the older checkpoint must advance to its derived approval gate");
    assert_eq!(
        approval.code,
        CoreErrorCode::PermissionDenied,
        "a pending occurrence after checkpoint R must not trigger the predecessor barrier: {approval:?}"
    );
    assert!(approval.recoverable);
    let pending = core
        .list_generation_attempt_proposals_for_source_room(
            &fixture.conversation_id,
            &fixture.branch_id,
            InteractionProposalStatus::Pending,
            10,
        )
        .expect("list historical attempt approval evidence");
    assert_eq!(
        pending.len(),
        2,
        "the bootstrap and admitted historical attempt must each retain one proposal"
    );
    drop(core);
    fixture
        .provider
        .join()
        .expect("join admitted historical zero-call provider probe");
    assert!(
        fixture.provider_requests.try_recv().is_err(),
        "the historical approval gate must precede provider dispatch"
    );
    assert_deferred_predecessor_unchanged(fixture.root.path(), &fixture.predecessor);
    let (before_snapshot_count, dispatched_count) =
        generation_attempt_evidence_counts(fixture.root.path(), &fixture.conversation_id);
    assert_eq!(
        before_snapshot_count, 2,
        "the admitted historical fork must persist its own BeforeGeneration snapshot"
    );
    assert_eq!(dispatched_count, 0);
}

struct PendingPredecessorFixture {
    root: TempDir,
    requests: mpsc::Receiver<Vec<u8>>,
    provider: thread::JoinHandle<()>,
    conversation_id: lorepia_core::ConversationId,
    branch_id: lorepia_core::ConversationBranchId,
    target: GenerationTarget,
    predecessor: DeferredPredecessor,
}

fn prepare_pending_predecessor_fixture() -> PendingPredecessorFixture {
    let root = tempdir().expect("create predecessor Core root");
    let (origin, requests, provider) = spawn_provider(0);
    let core = Core::open(CoreConfig::new(root.path())).expect("open predecessor Core");
    let character_id = import_character(&core);
    let target = provider_fixture(&core, &origin);
    core.select_generation_target(Some(target.clone()))
        .expect("select predecessor generation target");
    set_json_mode_capability(&core, &target.model_route_id, true);
    let conversation = core
        .open_conversation(&character_id)
        .expect("open predecessor conversation");
    let branch = core
        .list_conversation_branches(&conversation.id)
        .expect("list predecessor branches")
        .into_iter()
        .next()
        .expect("predecessor root branch");
    let initial_lifecycle = core
        .drain_core_lifecycle_occurrences(64)
        .expect("initialize predecessor interaction state");
    assert!(initial_lifecycle.queue_idle);
    let fixture = install_closure_fixture(
        &core,
        ContentModuleRuntimeTarget {
            conversation_id: conversation.id.clone(),
            branch_id: branch.id.clone(),
        },
    );
    let room_config = core
        .get_room_orchestration_config(&conversation.id, &branch.id)
        .expect("load predecessor room context");
    save_room_context(&core, &room_config, &target, SEALED_USER, SEALED_SLOT);
    let bootstrap = core
        .resolve_prompt_preview(
            &PromptPlanRequest {
                conversation_id: conversation.id.clone(),
                branch_id: branch.id.clone(),
                expected_head: None,
                user_text: "Synthetic predecessor authority bootstrap".to_owned(),
                generation_target: target.clone(),
                prompt_preset_id: None,
                variable_overrides: VariableMap::default(),
                expected_plan_hash: None,
            },
            GenerationOperationContext::New {
                operation_nonce: "pending-predecessor-bootstrap-v1",
            },
        )
        .expect_err("bootstrap attempt must stop at its synthetic approval");
    assert_eq!(bootstrap.code, CoreErrorCode::PermissionDenied);
    assert_eq!(
        core.list_generation_attempt_proposals_for_source_room(
            &conversation.id,
            &branch.id,
            InteractionProposalStatus::Pending,
            10,
        )
        .expect("load bootstrap proposal authority")
        .len(),
        1
    );
    drop(core);
    let predecessor =
        seed_deferred_same_branch_predecessor(root.path(), &conversation.id, &branch.id, &fixture);
    PendingPredecessorFixture {
        root,
        requests,
        provider,
        conversation_id: conversation.id,
        branch_id: branch.id,
        target,
        predecessor,
    }
}

fn assert_pending_predecessor_gate(fixture: &PendingPredecessorFixture) {
    let core = Core::open(CoreConfig::new(fixture.root.path())).expect("reopen predecessor Core");
    let blocked = core
        .resolve_prompt_preview(
            &PromptPlanRequest {
                conversation_id: fixture.conversation_id.clone(),
                branch_id: fixture.branch_id.clone(),
                expected_head: None,
                user_text: "Synthetic request behind a pending predecessor".to_owned(),
                generation_target: fixture.target.clone(),
                prompt_preset_id: None,
                variable_overrides: VariableMap::default(),
                expected_plan_hash: None,
            },
            GenerationOperationContext::New {
                operation_nonce: "pending-predecessor-blocked-request-v1",
            },
        )
        .expect_err("pending same-branch predecessor must stop BeforeGeneration staging");
    assert_eq!(blocked.code, CoreErrorCode::InvalidInput);
    assert!(blocked.recoverable);
    assert_eq!(
        blocked.message,
        "a predecessor derived interaction occurrence must be drained first"
    );
    drop(core);
}

fn assert_pending_predecessor_evidence(fixture: &PendingPredecessorFixture) {
    let connection = Connection::open(active_database_path(fixture.root.path()))
        .expect("inspect blocked predecessor evidence");
    let stored_occurrence = connection
        .query_row(
            "SELECT status, delivery_attempts, available_at,
                    parent_resulting_state_revision
             FROM interaction_derived_event_outbox
             WHERE occurrence_id = ?1",
            [fixture.predecessor.occurrence_id.as_str()],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, u64>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, u64>(3)?,
                ))
            },
        )
        .expect("read unchanged predecessor occurrence");
    assert_eq!(stored_occurrence.0, "pending");
    assert_eq!(stored_occurrence.1, fixture.predecessor.delivery_attempts);
    assert_eq!(
        chrono::DateTime::parse_from_rfc3339(&stored_occurrence.2)
            .expect("parse predecessor availability")
            .with_timezone(&Utc),
        fixture.predecessor.available_at
    );
    assert_eq!(
        stored_occurrence.3,
        fixture.predecessor.parent_resulting_state_revision
    );
    let before_snapshot_count = connection
        .query_row(
            "SELECT COUNT(*)
             FROM generation_attempt_before_event_snapshots AS snapshot
             JOIN generation_attempt_intents AS attempt
               ON attempt.generation_id = snapshot.generation_id
             WHERE attempt.conversation_id = ?1",
            [fixture.conversation_id.0.as_str()],
            |row| row.get::<_, u64>(0),
        )
        .expect("count blocked BeforeGeneration snapshots");
    assert_eq!(
        before_snapshot_count, 1,
        "only the explicit bootstrap attempt may own a BeforeGeneration snapshot"
    );
    let dispatched_count = connection
        .query_row(
            "SELECT COUNT(*)
             FROM generation_attempt_intents
             WHERE conversation_id = ?1
               AND status IN ('running', 'completed')",
            [fixture.conversation_id.0.as_str()],
            |row| row.get::<_, u64>(0),
        )
        .expect("count dispatched predecessor attempts");
    assert_eq!(dispatched_count, 0);
}

#[test]
fn pending_same_branch_predecessor_blocks_before_review_and_provider_dispatch() {
    let fixture = prepare_pending_predecessor_fixture();
    assert_pending_predecessor_gate(&fixture);
    assert_pending_predecessor_evidence(&fixture);
    fixture
        .provider
        .join()
        .expect("join zero-call provider probe");
    assert!(
        fixture.requests.try_recv().is_err(),
        "a blocked predecessor must not reach the provider"
    );
}

struct NoModuleReviewFixture {
    root: TempDir,
    requests: mpsc::Receiver<Vec<u8>>,
    provider: thread::JoinHandle<()>,
    conversation_id: lorepia_core::ConversationId,
    branch_id: lorepia_core::ConversationBranchId,
    request: PromptPlanRequest,
    generation_id: GenerationId,
    ordinary_proposal: lorepia_core::InteractionProposalView,
    sealed_local_user_id_sha256: String,
    drifted_local_user_id: LocalUserId,
}

struct NoModuleGateRoom {
    conversation_id: lorepia_core::ConversationId,
    branch_id: lorepia_core::ConversationBranchId,
    ordinary_proposal: lorepia_core::InteractionProposalView,
}

fn create_no_module_gate_room(core: &Core, character_id: &str) -> NoModuleGateRoom {
    let bootstrap = core
        .open_conversation(character_id)
        .expect("open no-module gate bootstrap conversation");
    let bootstrap_branch = core
        .list_conversation_branches(&bootstrap.id)
        .expect("list no-module gate bootstrap branches")
        .into_iter()
        .next()
        .expect("no-module gate bootstrap branch");
    assert!(
        core.drain_core_lifecycle_occurrences(64)
            .expect("initialize no-module gate bootstrap lifecycle")
            .queue_idle
    );
    let gate_binding_id = activate_no_module_review_gate(
        core,
        ContentModuleRuntimeTarget {
            conversation_id: bootstrap.id,
            branch_id: bootstrap_branch.id,
        },
    );
    let conversation = core
        .open_conversation(character_id)
        .expect("open no-module conversation");
    let branch = core
        .list_conversation_branches(&conversation.id)
        .expect("list no-module branches")
        .into_iter()
        .next()
        .expect("no-module root branch");
    assert!(
        core.drain_core_lifecycle_occurrences(64)
            .expect("materialize no-module ordinary approval gate")
            .queue_idle
    );
    let pending = core
        .list_interaction_proposals(
            &conversation.id,
            &branch.id,
            InteractionProposalStatus::Pending,
            10,
        )
        .expect("list no-module ordinary gate proposal");
    let [ordinary_proposal] = pending.as_slice() else {
        panic!("expected one no-module gate proposal, got {pending:?}");
    };
    assert_eq!(
        ordinary_proposal.record.proposal_id,
        NO_MODULE_GATE_PROPOSAL_ID
    );
    deactivate_no_module_review_gate(
        core,
        ContentModuleRuntimeTarget {
            conversation_id: conversation.id.clone(),
            branch_id: branch.id.clone(),
        },
        gate_binding_id,
    );
    NoModuleGateRoom {
        conversation_id: conversation.id,
        branch_id: branch.id,
        ordinary_proposal: ordinary_proposal.clone(),
    }
}

fn prepare_no_module_review_fixture() -> NoModuleReviewFixture {
    let root = tempdir().expect("create temporary no-module Core root");
    let (origin, requests, provider) = spawn_provider(1);
    let core = Core::open(CoreConfig::new(root.path())).expect("open no-module Core");
    let character_id = import_character(&core);
    let target = provider_fixture(&core, &origin);
    let core = reopen_with_provider_credential_authority(core, root.path());
    core.select_generation_target(Some(target.clone()))
        .expect("select no-module generation target");
    let gate_room = create_no_module_gate_room(&core, &character_id);

    let sealed_local_user_id = core
        .get_settings()
        .expect("load no-module sealed local user")
        .local_user_id;
    let sealed_local_user_id_sha256 =
        lorepia_domain::prompt_local_user_id_sha256(&sealed_local_user_id);
    let request = PromptPlanRequest {
        conversation_id: gate_room.conversation_id.clone(),
        branch_id: gate_room.branch_id.clone(),
        expected_head: None,
        user_text: "SYNTHETIC_NO_MODULE_SEALED_LOCAL_USER_7C61".to_owned(),
        generation_target: target,
        prompt_preset_id: None,
        variable_overrides: VariableMap::default(),
        expected_plan_hash: None,
    };
    let blocked = core
        .resolve_prompt_preview(
            &request,
            GenerationOperationContext::New {
                operation_nonce: "no-module-review-v1",
            },
        )
        .expect_err("ordinary approval must pause the no-module prepared attempt");
    assert_eq!(
        blocked.code,
        CoreErrorCode::PermissionDenied,
        "unexpected no-module approval gate failure: {blocked:?}"
    );
    assert!(blocked.recoverable);
    assert_eq!(
        blocked.message,
        "generation is blocked by an existing interaction approval"
    );
    assert!(
        core.list_generation_attempt_proposals_for_source_room(
            &gate_room.conversation_id,
            &gate_room.branch_id,
            InteractionProposalStatus::Pending,
            10,
        )
        .expect("list isolated no-module attempt proposals")
        .is_empty()
    );
    let generation_id =
        read_prepared_no_module_generation_id(root.path(), &gate_room.conversation_id);
    assert_generation_prompt_authority_json_canonical(root.path(), &generation_id);
    drop(core);

    let drifted_local_user_id = verify_no_module_review_and_drift_local_user(
        root.path(),
        &generation_id,
        &sealed_local_user_id,
        &sealed_local_user_id_sha256,
    );
    NoModuleReviewFixture {
        root,
        requests,
        provider,
        conversation_id: gate_room.conversation_id,
        branch_id: gate_room.branch_id,
        request,
        generation_id,
        ordinary_proposal: gate_room.ordinary_proposal,
        sealed_local_user_id_sha256,
        drifted_local_user_id,
    }
}

fn read_prepared_no_module_generation_id(
    root: &std::path::Path,
    conversation_id: &lorepia_core::ConversationId,
) -> GenerationId {
    let connection = Connection::open(active_database_path(root))
        .expect("open prepared no-module attempt evidence connection");
    let (generation_id, status, attempt_count) = connection
        .query_row(
            "SELECT generation_id, status,
                    (SELECT COUNT(*) FROM generation_attempt_intents
                     WHERE conversation_id = ?1)
             FROM generation_attempt_intents
             WHERE conversation_id = ?1
             ORDER BY created_at DESC
             LIMIT 1",
            [conversation_id.0.as_str()],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, u64>(2)?,
                ))
            },
        )
        .expect("read prepared no-module attempt identity");
    assert_eq!(status, "prepared");
    assert_eq!(attempt_count, 1);
    GenerationId(generation_id)
}

fn verify_no_module_review_and_drift_local_user(
    root: &std::path::Path,
    generation_id: &GenerationId,
    sealed_local_user_id: &LocalUserId,
    sealed_local_user_id_sha256: &str,
) -> LocalUserId {
    let storage = Storage::open(root).expect("open no-module attempt evidence storage");
    let attempt = storage
        .get_generation_attempt(generation_id)
        .expect("load prepared no-module attempt");
    assert_eq!(attempt.status, GenerationAttemptStatus::Prepared);
    assert_eq!(
        attempt.input.module_plan_sha256,
        lorepia_orchestration::no_applied_module_runtime_plan_sha256()
    );
    assert_eq!(
        attempt
            .input
            .prompt_selection_authority
            .as_ref()
            .expect("prepared no-module prompt authority")
            .local_user_id_sha256,
        sealed_local_user_id_sha256
    );
    assert!(
        storage
            .get_generation_attempt_before_review(generation_id)
            .expect("check absent blocked no-module BeforeGeneration review")
            .is_none()
    );
    drop(storage);

    let drifted_local_user_id = mutate_live_local_user(root);
    assert_ne!(&drifted_local_user_id, sealed_local_user_id);
    drifted_local_user_id
}

fn verify_no_module_before_review(
    root: &std::path::Path,
    generation_id: &GenerationId,
    sealed_local_user_id_sha256: &str,
) {
    let storage = Storage::open(root).expect("open resumed no-module attempt evidence storage");
    let before = storage
        .get_generation_attempt_before_review(generation_id)
        .expect("load resumed no-module BeforeGeneration review")
        .expect("resumed no-module BeforeGeneration review exists");
    assert!(before.applied_runtime_plan.is_none());
    assert!(before.derived_closure.transitions.iter().all(|transition| {
        transition.policy.rule_sets.is_empty() && transition.policy.module_plan_sha256.is_none()
    }));
    assert_eq!(
        before.prompt_selection_authority.local_user_id_sha256,
        sealed_local_user_id_sha256
    );
    drop(storage);

    let connection = Connection::open(active_database_path(root))
        .expect("open no-module review evidence connection");
    let review_json = connection
        .query_row(
            "SELECT module_runtime_review_json
             FROM generation_attempt_before_event_snapshots
             WHERE generation_id = ?1",
            [generation_id.0.as_str()],
            |row| row.get::<_, String>(0),
        )
        .expect("read no-module runtime review JSON");
    let review: lorepia_orchestration::ModuleMergeReview =
        serde_json::from_str(&review_json).expect("decode no-module runtime review");
    review.verify().expect("verify no-module runtime review");
    assert!(review.activation_binding_ids.is_empty());
    assert!(review.ordered_bindings.is_empty());
    assert_eq!(
        lorepia_domain::prompt_local_user_id_sha256(&review.context.local_user_id),
        sealed_local_user_id_sha256
    );
}

fn dispatch_no_module_review(
    core: Core,
    fixture: NoModuleReviewFixture,
    reopened_preview: lorepia_core::ExpertPromptPreview,
) {
    let mut reviewed = fixture.request.clone();
    reviewed.expected_plan_hash = Some(reopened_preview.plan.plan_hash.clone());
    let dispatched = core
        .send_message_with_prompt_plan(
            &reviewed,
            &reopened_preview.generation_attempt_id,
            credential(fixture.root.path()),
        )
        .expect("dispatch no-module attempt from sealed local-user authority");
    assert_eq!(dispatched, fixture.generation_id);
    wait_for_generation(&core, &fixture.branch_id, &dispatched);
    let wire_request = fixture
        .requests
        .recv_timeout(Duration::from_secs(5))
        .expect("receive no-module provider request");
    let wire_body = request_body(&wire_request);
    assert_eq!(wire_body, reopened_preview.provider_request);
    assert!(
        wire_body
            .to_string()
            .contains("SYNTHETIC_NO_MODULE_SEALED_LOCAL_USER_7C61")
    );
    let stored_plan = core
        .get_generation_prompt_plan(&fixture.generation_id)
        .expect("load no-module stored prompt plan");
    let stored_resolved: lorepia_domain::ResolvedPromptPlan =
        serde_json::from_value(stored_plan.plan.value)
            .expect("decode no-module stored sealed prompt plan");
    assert_eq!(
        stored_resolved
            .trace
            .context_snapshot
            .as_ref()
            .expect("stored no-module context snapshot")
            .local_user_id_sha256,
        fixture.sealed_local_user_id_sha256
    );
    assert_ne!(
        fixture.sealed_local_user_id_sha256,
        lorepia_domain::prompt_local_user_id_sha256(&fixture.drifted_local_user_id)
    );
    drop(core);
    fixture.provider.join().expect("join no-module provider");
}

fn resume_no_module_review(fixture: NoModuleReviewFixture) {
    let core = Core::open(CoreConfig::new(fixture.root.path()))
        .expect("reopen no-module attempt after local-user drift");
    assert_eq!(
        core.get_settings()
            .expect("load no-module drifted local user")
            .local_user_id,
        fixture.drifted_local_user_id
    );
    let rejection = core
        .decide_interaction_proposal(&InteractionProposalDecisionRequest {
            conversation_id: fixture.conversation_id.clone(),
            branch_id: fixture.branch_id.clone(),
            proposal_record_id: fixture.ordinary_proposal.record.id.clone(),
            expected_state_revision: fixture.ordinary_proposal.state_revision,
            expected_proposal_revision: fixture.ordinary_proposal.proposal_revision,
            decision: InteractionProposalDecision::Reject,
        })
        .expect("reject ordinary no-module gate proposal");
    assert_eq!(
        rejection.proposal.status,
        InteractionProposalStatus::Rejected
    );
    drop(core);

    let core = open_core_after_drop(fixture.root.path());
    let preview = core
        .resolve_prompt_preview(
            &fixture.request,
            GenerationOperationContext::Resume {
                generation_attempt_id: &fixture.generation_id,
            },
        )
        .expect("resume no-module attempt from sealed local-user authority");
    assert_eq!(preview.generation_attempt_id, fixture.generation_id);
    let sealed_trace = core
        .explain_prompt_plan(
            &fixture.request,
            GenerationOperationContext::Resume {
                generation_attempt_id: &fixture.generation_id,
            },
            &preview.plan.plan_hash,
        )
        .expect("explain resumed no-module sealed prompt");
    assert_eq!(
        sealed_trace
            .context_snapshot
            .as_ref()
            .expect("resumed no-module context snapshot")
            .local_user_id_sha256,
        fixture.sealed_local_user_id_sha256
    );
    let preview_before_reopen = preview.clone();
    let trace_before_reopen = sealed_trace.clone();
    drop(core);
    verify_no_module_before_review(
        fixture.root.path(),
        &fixture.generation_id,
        &fixture.sealed_local_user_id_sha256,
    );

    let core = Core::open(CoreConfig::new(fixture.root.path()))
        .expect("reopen reviewed no-module attempt before dispatch");
    let reopened_preview = core
        .resolve_prompt_preview(
            &fixture.request,
            GenerationOperationContext::Resume {
                generation_attempt_id: &fixture.generation_id,
            },
        )
        .expect("reopen exact no-module sealed preview");
    assert_eq!(reopened_preview, preview_before_reopen);
    assert_eq!(
        core.explain_prompt_plan(
            &fixture.request,
            GenerationOperationContext::Resume {
                generation_attempt_id: &fixture.generation_id,
            },
            &reopened_preview.plan.plan_hash,
        )
        .expect("explain reopened no-module sealed prompt"),
        trace_before_reopen
    );
    dispatch_no_module_review(core, fixture, reopened_preview);
}

#[test]
fn no_module_review_reuses_sealed_local_user_after_restart_and_dispatch() {
    resume_no_module_review(prepare_no_module_review_fixture());
}

struct AuthorityBase {
    root: TempDir,
    requests: mpsc::Receiver<Vec<u8>>,
    provider: thread::JoinHandle<()>,
    core: Core,
    character_id: String,
    target: GenerationTarget,
    conversation: Conversation,
    branch: ConversationBranch,
    sealed_local_user_id: LocalUserId,
    sealed_local_user_id_sha256: String,
    sealed_character_content: Revisioned<CharacterContentV1>,
    sealed_character_knowledge: Revisioned<KnowledgeBook>,
    authority_preset_id: PromptPresetId,
    authority_source_start: MessageId,
    authority_head: MessageId,
    sealed_selected_memory: StoredRevision<MemoryRecord>,
    sealed_summary_memory: StoredRevision<MemoryRecord>,
}

struct AuthorityScenario {
    root: TempDir,
    requests: mpsc::Receiver<Vec<u8>>,
    provider: Option<thread::JoinHandle<()>>,
    character_id: String,
    target: GenerationTarget,
    conversation: Conversation,
    branch: ConversationBranch,
    sealed_local_user_id: LocalUserId,
    sealed_local_user_id_sha256: String,
    sealed_character_content: Revisioned<CharacterContentV1>,
    sealed_character_knowledge: Revisioned<KnowledgeBook>,
    authority_source_start: MessageId,
    authority_head: MessageId,
    sealed_selected_memory: StoredRevision<MemoryRecord>,
    sealed_summary_memory: StoredRevision<MemoryRecord>,
    sealed_persona: Revisioned<Persona>,
    sealed_persona_revision: ObjectRevision<Persona>,
    drifted_persona: Revisioned<Persona>,
    sealed_persona_selection: ConversationPersonaSelectionState,
    fixture: ClosureFixture,
    sealed_room: RoomOrchestrationConfig,
    live_revision_before_attempt: u64,
    request: PromptPlanRequest,
}

struct AuthorityDrift {
    drifted_room: RoomOrchestrationConfig,
    drifted_local_user_id: LocalUserId,
}

fn save_authority_memory_revisions(
    root: &std::path::Path,
    conversation: &Conversation,
    branch: &ConversationBranch,
    source_start: &MessageId,
    source_end: &MessageId,
) -> (StoredRevision<MemoryRecord>, StoredRevision<MemoryRecord>) {
    let storage = Storage::open(root).expect("open authority memory fixture storage");
    let source = AuthorityMemorySource {
        conversation: &conversation.id,
        branch: &branch.id,
        start_message: source_start,
        end_message: source_end,
    };
    let selected = storage
        .save_memory_record(
            &synthetic_authority_memory(
                SELECTED_MEMORY_ID,
                MemoryKind::CreatorPinned,
                SEALED_SELECTED_MEMORY,
                true,
                &source,
            ),
            None,
        )
        .expect("save sealed selected-memory revision");
    let summary = storage
        .save_memory_record(
            &synthetic_authority_memory(
                SUMMARY_MEMORY_ID,
                MemoryKind::ConversationSummary,
                SEALED_CONVERSATION_SUMMARY,
                false,
                &source,
            ),
            None,
        )
        .expect("save sealed conversation-summary revision");
    (selected, summary)
}

fn prepare_authority_base() -> AuthorityBase {
    let root = tempdir().expect("create temporary Core root");
    let (origin, requests, provider) = spawn_provider(1);
    let core = Core::open(CoreConfig::new(root.path())).expect("open Core");
    let character_id = import_authority_character(&core);
    let target = provider_fixture(&core, &origin);
    let core = reopen_with_provider_credential_authority(core, root.path());
    core.select_generation_target(Some(target.clone()))
        .expect("select synthetic generation target");
    set_json_mode_capability(&core, &target.model_route_id, true);
    let conversation = core
        .open_conversation(&character_id)
        .expect("open synthetic conversation through the lifecycle boundary");
    let branch = core
        .list_conversation_branches(&conversation.id)
        .expect("list synthetic branches")
        .into_iter()
        .next()
        .expect("root synthetic branch");
    let lifecycle = core
        .drain_core_lifecycle_occurrences(64)
        .expect("initialize the synthetic room interaction boundary");
    assert!(lifecycle.queue_idle);
    assert!(!lifecycle.deliveries.is_empty());
    let sealed_local_user_id = core
        .get_settings()
        .expect("load sealed local-user authority")
        .local_user_id;
    let sealed_local_user_id_sha256 =
        lorepia_domain::prompt_local_user_id_sha256(&sealed_local_user_id);
    let sealed_character_content = core
        .get_character_content(&character_id)
        .expect("load sealed character content authority");
    assert_eq!(
        sealed_character_content.value.personality,
        SEALED_CHARACTER_PERSONALITY
    );
    let sealed_character_knowledge = core
        .upsert_knowledge_book(&authority_character_knowledge(), None)
        .expect("save sealed linked character knowledge");
    let authority_preset_id = install_authority_prompt_preset(&core, &target).value.id;
    let (authority_source_start, authority_head) =
        install_synthetic_authority_turn(root.path(), &conversation.id, &branch.id);
    drop(core);
    let (sealed_selected_memory, sealed_summary_memory) = save_authority_memory_revisions(
        root.path(),
        &conversation,
        &branch,
        &authority_source_start,
        &authority_head,
    );
    let core = Core::open(CoreConfig::new(root.path()))
        .expect("reopen Core after synthetic memory fixture setup");
    AuthorityBase {
        root,
        requests,
        provider,
        core,
        character_id,
        target,
        conversation,
        branch,
        sealed_local_user_id,
        sealed_local_user_id_sha256,
        sealed_character_content,
        sealed_character_knowledge,
        authority_preset_id,
        authority_source_start,
        authority_head,
        sealed_selected_memory,
        sealed_summary_memory,
    }
}

fn prepare_authority_scenario() -> AuthorityScenario {
    let base = prepare_authority_base();
    let sealed_persona = base
        .core
        .create_persona(&PersonaCreateRequest {
            name: SEALED_USER.to_owned(),
            description: SEALED_PERSONA_DESCRIPTION.to_owned(),
        })
        .expect("create sealed prompt persona");
    let sealed_persona_revision = base
        .core
        .get_persona_revision(
            &sealed_persona.value.id,
            sealed_persona
                .revision_id
                .as_deref()
                .expect("sealed persona immutable revision"),
        )
        .expect("load sealed persona revision evidence");
    let drifted_persona = base
        .core
        .create_persona(&PersonaCreateRequest {
            name: DRIFTED_USER.to_owned(),
            description: DRIFTED_PERSONA_DESCRIPTION.to_owned(),
        })
        .expect("create drift target persona");
    let sealed_persona_selection = base
        .core
        .select_conversation_persona(&ConversationPersonaSelectionRequest {
            conversation_id: base.conversation.id.clone(),
            persona_id: sealed_persona.value.id.clone(),
            expected_revision: None,
        })
        .expect("select sealed prompt persona");
    let fixture = install_closure_fixture(
        &base.core,
        ContentModuleRuntimeTarget {
            conversation_id: base.conversation.id.clone(),
            branch_id: base.branch.id.clone(),
        },
    );
    let initial_room = base
        .core
        .get_room_orchestration_config(&base.conversation.id, &base.branch.id)
        .expect("load initial room config");
    let sealed_room = save_authority_room_context(
        &base.core,
        &initial_room,
        &base.target,
        &AuthorityRoomSettings {
            prompt_preset_id: &base.authority_preset_id,
            response_length: PromptResponseLength::Short,
            creativity: 20,
            reasoning_effort: Some(GenerationReasoningEffort::High),
            memory_enabled: true,
            knowledge_enabled: true,
            user_name: SEALED_USER,
            slot: SEALED_SLOT,
        },
    );
    let live_revision_before_attempt = base
        .core
        .get_interaction_state_revision(&base.conversation.id, &base.branch.id)
        .expect("read pre-attempt interaction revision");
    let request = PromptPlanRequest {
        conversation_id: base.conversation.id.clone(),
        branch_id: base.branch.id.clone(),
        expected_head: Some(base.authority_head.clone()),
        user_text: "Synthetic derived closure request".to_owned(),
        generation_target: base.target.clone(),
        prompt_preset_id: None,
        variable_overrides: VariableMap::default(),
        expected_plan_hash: None,
    };
    drop(base.core);
    AuthorityScenario {
        root: base.root,
        requests: base.requests,
        provider: Some(base.provider),
        character_id: base.character_id,
        target: base.target,
        conversation: base.conversation,
        branch: base.branch,
        sealed_local_user_id: base.sealed_local_user_id,
        sealed_local_user_id_sha256: base.sealed_local_user_id_sha256,
        sealed_character_content: base.sealed_character_content,
        sealed_character_knowledge: base.sealed_character_knowledge,
        authority_source_start: base.authority_source_start,
        authority_head: base.authority_head,
        sealed_selected_memory: base.sealed_selected_memory,
        sealed_summary_memory: base.sealed_summary_memory,
        sealed_persona,
        sealed_persona_revision,
        drifted_persona,
        sealed_persona_selection,
        fixture,
        sealed_room,
        live_revision_before_attempt,
        request,
    }
}

fn begin_authority_review(
    scenario: &AuthorityScenario,
) -> (
    GenerationId,
    lorepia_core::GenerationAttemptProposalDecisionRequest,
) {
    let core = open_core_after_drop(scenario.root.path());
    let blocked = core
        .resolve_prompt_preview(
            &scenario.request,
            GenerationOperationContext::New {
                operation_nonce: "derived-closure-authority-review-v1",
            },
        )
        .expect_err("a derived child proposal must block dispatch");
    assert_eq!(
        blocked.code,
        CoreErrorCode::PermissionDenied,
        "unexpected initial closure failure: {blocked:?}"
    );
    assert_eq!(
        core.get_interaction_state_revision(&scenario.conversation.id, &scenario.branch.id)
            .expect("read isolated live revision"),
        scenario.live_revision_before_attempt,
        "attempt-owned root and derived children must remain isolated"
    );
    assert!(
        core.list_interaction_effect_history(
            &scenario.conversation.id,
            &scenario.branch.id,
            None,
            100,
        )
        .expect("read live effects before append")
        .is_empty(),
        "attempt-owned effects must not leak before append"
    );
    let pending = core
        .list_generation_attempt_proposals_for_source_room(
            &scenario.conversation.id,
            &scenario.branch.id,
            InteractionProposalStatus::Pending,
            10,
        )
        .expect("list first derived proposal");
    let [first] = pending.as_slice() else {
        panic!("expected one first derived proposal, got {pending:?}");
    };
    assert_eq!(first.proposal.record.proposal_id, "approve-first-child");
    assert!(
        first
            .proposal
            .record
            .body
            .contains("SYNTHETIC_FIRST_CHILD_PROPOSAL;VALUE=root-child-visible")
    );
    let generation_id = first.proposal.generation_id.clone();
    assert_generation_prompt_authority_json_canonical(scenario.root.path(), &generation_id);
    let request = lorepia_core::GenerationAttemptProposalDecisionRequest {
        conversation_id: scenario.conversation.id.clone(),
        source_branch_id: scenario.branch.id.clone(),
        generation_id: generation_id.clone(),
        proposal_record_id: first.proposal.record.id.clone(),
        expected_aggregate_revision: first.aggregate_revision,
        expected_proposal_revision: first.proposal.proposal_revision,
        decision: InteractionProposalDecision::Approve,
    };
    (generation_id, request)
}

fn drift_authority_runtime_sources(
    core: &Core,
    scenario: &AuthorityScenario,
) -> RoomOrchestrationConfig {
    set_json_mode_capability(core, &scenario.target.model_route_id, false);
    let current_room = core
        .get_room_orchestration_config(&scenario.conversation.id, &scenario.branch.id)
        .expect("reload sealed room config");
    assert_eq!(
        current_room.binding_revision,
        scenario.sealed_room.binding_revision
    );
    let drifted_room = save_authority_room_context(
        core,
        &current_room,
        &scenario.target,
        &AuthorityRoomSettings {
            prompt_preset_id: &current_room.prompt_preset_id,
            response_length: PromptResponseLength::Long,
            creativity: 90,
            reasoning_effort: Some(GenerationReasoningEffort::Low),
            memory_enabled: false,
            knowledge_enabled: false,
            user_name: DRIFTED_USER,
            slot: DRIFTED_SLOT,
        },
    );
    assert_eq!(
        drifted_room.user_name_override.as_deref(),
        Some(DRIFTED_USER)
    );
    assert_eq!(
        core.set_conversation_mode(&scenario.conversation.id, ConversationMode::Story)
            .expect("drift live conversation mode after attempt capture")
            .selected_mode,
        ConversationMode::Story
    );
    core.update_persona(&PersonaUpdateRequest {
        persona_id: scenario.sealed_persona.value.id.clone(),
        expected_revision: scenario.sealed_persona.revision,
        name: "Updated sealed persona head".to_owned(),
        description: "SYNTHETIC_UPDATED_SEALED_PERSONA_HEAD_019B".to_owned(),
    })
    .expect("drift sealed persona head after attempt creation");
    let drifted_selection = core
        .select_conversation_persona(&ConversationPersonaSelectionRequest {
            conversation_id: scenario.conversation.id.clone(),
            persona_id: scenario.drifted_persona.value.id.clone(),
            expected_revision: scenario.sealed_persona_selection.revision,
        })
        .expect("drift live persona selection after attempt creation");
    assert_ne!(
        drifted_selection.selected_persona_revision_id,
        scenario
            .sealed_persona_selection
            .selected_persona_revision_id
    );
    let mut knowledge = authority_character_knowledge();
    "Drifted character knowledge".clone_into(&mut knowledge.name);
    DRIFTED_CHARACTER_KNOWLEDGE.clone_into(&mut knowledge.entries[0].content);
    core.upsert_knowledge_book(
        &knowledge,
        Some(scenario.sealed_character_knowledge.revision),
    )
    .expect("drift linked character knowledge head");
    drifted_room
}

fn drift_authority_memories(core: &Core, scenario: &AuthorityScenario) {
    let edited_selected = core
        .patch_memory_record_user_fields(
            &scenario.sealed_selected_memory.value.conversation_id,
            &scenario.sealed_selected_memory.value.branch_id,
            &scenario.sealed_selected_memory.value.id,
            scenario.sealed_selected_memory.revision,
            &MemoryRecordUserPatch {
                summary: Some(DRIFTED_SELECTED_MEMORY.to_owned()),
                ..MemoryRecordUserPatch::default()
            },
        )
        .expect("edit selected-memory head during approval");
    let edited_summary = core
        .patch_memory_record_user_fields(
            &scenario.sealed_summary_memory.value.conversation_id,
            &scenario.sealed_summary_memory.value.branch_id,
            &scenario.sealed_summary_memory.value.id,
            scenario.sealed_summary_memory.revision,
            &MemoryRecordUserPatch {
                summary: Some(DRIFTED_CONVERSATION_SUMMARY.to_owned()),
                ..MemoryRecordUserPatch::default()
            },
        )
        .expect("edit conversation-summary head during approval");
    assert_ne!(
        edited_selected.revision_id,
        scenario.sealed_selected_memory.revision_id
    );
    assert_ne!(
        edited_summary.revision_id,
        scenario.sealed_summary_memory.revision_id
    );
    let invalidated = core
        .invalidate_memory_range(
            &scenario.conversation.id,
            &scenario.branch.id,
            &scenario.authority_source_start,
            &scenario.authority_head,
            Utc::now(),
        )
        .expect("invalidate drifted live memory range");
    assert_eq!(invalidated.invalidated_records, 2);
}

fn apply_authority_drift(scenario: &AuthorityScenario) -> AuthorityDrift {
    let core = open_core_after_drop(scenario.root.path());
    let drifted_room = drift_authority_runtime_sources(&core, scenario);
    drift_authority_memories(&core, scenario);
    drop(core);
    let storage =
        Storage::open(scenario.root.path()).expect("open character-content drift storage");
    let mut content = scenario.sealed_character_content.value.clone();
    DRIFTED_CHARACTER_PERSONALITY.clone_into(&mut content.personality);
    storage
        .save_character_content(
            &scenario.character_id,
            &content,
            Some(scenario.sealed_character_content.revision),
        )
        .expect("drift live character-content head");
    drop(storage);
    let drifted_local_user_id =
        mutate_live_local_user_and_character(scenario.root.path(), &scenario.character_id);
    assert_ne!(drifted_local_user_id, scenario.sealed_local_user_id);
    AuthorityDrift {
        drifted_room,
        drifted_local_user_id,
    }
}

fn approve_first_authority_child(
    core: &Core,
    scenario: &AuthorityScenario,
    generation_id: &GenerationId,
    request: &lorepia_core::GenerationAttemptProposalDecisionRequest,
) -> lorepia_core::GenerationAttemptProposalDecisionRequest {
    let receipt = core
        .decide_generation_attempt_proposal(request)
        .expect("approve first derived proposal from sealed authority");
    assert_eq!(receipt.pending_proposal_count, 1);
    assert_generation_prompt_authority_json_canonical(scenario.root.path(), generation_id);
    let pending = core
        .list_generation_attempt_proposals_for_source_room(
            &scenario.conversation.id,
            &scenario.branch.id,
            InteractionProposalStatus::Pending,
            10,
        )
        .expect("list second derived proposal");
    let [second] = pending.as_slice() else {
        panic!("expected one second derived proposal, got {pending:?}");
    };
    assert_eq!(second.proposal.generation_id, *generation_id);
    assert_eq!(second.proposal.record.proposal_id, "approve-second-child");
    assert!(
        second
            .proposal
            .record
            .body
            .contains("SYNTHETIC_SECOND_CHILD_PROPOSAL;VALUE=sealed-capability-visible"),
        "approved UserAction child must use the attempt-sealed capability set: {}",
        second.proposal.record.body
    );
    let blocked = core
        .resolve_prompt_preview(
            &scenario.request,
            GenerationOperationContext::Resume {
                generation_attempt_id: generation_id,
            },
        )
        .expect_err("second child approval must still block dispatch");
    assert_eq!(
        blocked.code,
        CoreErrorCode::PermissionDenied,
        "unexpected second-approval resume failure: {blocked:?}"
    );
    lorepia_core::GenerationAttemptProposalDecisionRequest {
        conversation_id: scenario.conversation.id.clone(),
        source_branch_id: scenario.branch.id.clone(),
        generation_id: generation_id.clone(),
        proposal_record_id: second.proposal.record.id.clone(),
        expected_aggregate_revision: second.aggregate_revision,
        expected_proposal_revision: second.proposal.proposal_revision,
        decision: InteractionProposalDecision::Approve,
    }
}

fn approve_nested_authority_children(
    scenario: &AuthorityScenario,
    generation_id: &GenerationId,
    first_request: &lorepia_core::GenerationAttemptProposalDecisionRequest,
    drift: &AuthorityDrift,
) -> Core {
    let core = Core::open(CoreConfig::new(scenario.root.path()))
        .expect("reopen after all live prompt-authority drift");
    assert_eq!(
        core.get_settings()
            .expect("load drifted local-user authority")
            .local_user_id,
        drift.drifted_local_user_id
    );
    let second_request =
        approve_first_authority_child(&core, scenario, generation_id, first_request);
    let second_body = core
        .list_generation_attempt_proposals_for_source_room(
            &scenario.conversation.id,
            &scenario.branch.id,
            InteractionProposalStatus::Pending,
            10,
        )
        .expect("reload second derived proposal")
        .into_iter()
        .next()
        .expect("second derived proposal")
        .proposal
        .record
        .body;
    drop(core);
    let core = open_core_after_drop(scenario.root.path());
    let reopened = core
        .list_generation_attempt_proposals_for_source_room(
            &scenario.conversation.id,
            &scenario.branch.id,
            InteractionProposalStatus::Pending,
            10,
        )
        .expect("rediscover second proposal after restart");
    assert_eq!(reopened.len(), 1);
    assert_eq!(reopened[0].proposal.record.body, second_body);
    let receipt = core
        .decide_generation_attempt_proposal(&second_request)
        .expect("approve second derived proposal from sealed authority");
    assert_eq!(receipt.pending_proposal_count, 0);
    core
}

fn assert_authority_preview_text(preview: &ExpertPromptPreview) {
    let text = preview_text(preview);
    for expected in [
        "ROOT=root-applied",
        "CHILD=root-child-visible",
        "KNOWLEDGE_CHILD=knowledge-child-visible",
        "APPROVAL_CHILD=approval-user-action-applied",
        "CAPABILITY_CHILD=sealed-capability-visible",
        "FINAL_CHILD=second-approval-complete",
        KNOWLEDGE_TEXT,
        SEALED_CHARACTER_DESCRIPTION,
        SEALED_CHARACTER_PERSONALITY,
        SEALED_CHARACTER_KNOWLEDGE,
        SEALED_PERSONA_DESCRIPTION,
        SEALED_SELECTED_MEMORY,
        SEALED_CONVERSATION_SUMMARY,
        SEALED_USER,
        SEALED_SLOT,
    ] {
        assert!(
            text.contains(expected),
            "preview omitted {expected}:\n{text}"
        );
    }
    assert!(
        !text.contains(DRIFTED_USER),
        "preview used drifted user context"
    );
    assert!(
        !text.contains(DRIFTED_SLOT),
        "preview used drifted slot context"
    );
    for drifted in [
        DRIFTED_CHARACTER_DESCRIPTION,
        DRIFTED_CHARACTER_PERSONALITY,
        DRIFTED_CHARACTER_KNOWLEDGE,
        DRIFTED_PERSONA_DESCRIPTION,
        DRIFTED_SELECTED_MEMORY,
        DRIFTED_CONVERSATION_SUMMARY,
    ] {
        assert!(
            !text.contains(drifted),
            "preview used drifted prompt authority {drifted}:\n{text}"
        );
    }
    assert_eq!(
        preview
            .provider_request
            .get("temperature")
            .and_then(serde_json::Value::as_f64),
        Some(0.3)
    );
    assert_eq!(
        preview
            .provider_request
            .get("max_tokens")
            .and_then(serde_json::Value::as_u64),
        Some(512)
    );
}

fn assert_authority_preview_context(
    trace: &PromptResolutionTrace,
    scenario: &AuthorityScenario,
    drift: &AuthorityDrift,
) {
    let context = trace
        .context_snapshot
        .as_ref()
        .expect("sealed prompt context snapshot");
    assert_eq!(
        context.local_user_id_sha256,
        scenario.sealed_local_user_id_sha256
    );
    assert_ne!(
        context.local_user_id_sha256,
        lorepia_domain::prompt_local_user_id_sha256(&drift.drifted_local_user_id)
    );
    let binding = context.binding.as_ref().expect("sealed binding evidence");
    assert_eq!(
        Some(binding.binding_revision),
        scenario.sealed_room.binding_revision
    );
    assert_ne!(
        Some(binding.binding_revision),
        drift.drifted_room.binding_revision
    );
    let persona = context.persona.as_ref().expect("sealed persona evidence");
    assert_eq!(
        Some(persona.selection_revision),
        scenario.sealed_persona_selection.revision
    );
    assert_eq!(persona.persona_id, scenario.sealed_persona.value.id);
    assert_eq!(
        persona.persona_revision_id,
        scenario.sealed_persona_revision.revision_id
    );
    assert_eq!(
        persona.persona_sha256,
        scenario.sealed_persona_revision.sha256
    );
    assert_eq!(
        context.conversation_summary_id.as_ref(),
        Some(&scenario.sealed_summary_memory.value.id)
    );
    let [summary] = context.summaries.as_slice() else {
        panic!(
            "expected one sealed conversation-summary evidence row, got {:?}",
            context.summaries
        );
    };
    assert_eq!(
        summary.active_revision_id,
        scenario
            .sealed_summary_memory
            .revision_id
            .as_deref()
            .expect("sealed summary immutable revision")
    );
}

fn resolve_authority_preview(
    core: &Core,
    scenario: &AuthorityScenario,
    drift: &AuthorityDrift,
    generation_id: &GenerationId,
) -> (ExpertPromptPreview, PromptResolutionTrace) {
    let preview = core
        .resolve_prompt_preview(
            &scenario.request,
            GenerationOperationContext::Resume {
                generation_attempt_id: generation_id,
            },
        )
        .expect("resolve fully closed attempt preview");
    assert_eq!(preview.generation_attempt_id, *generation_id);
    assert_eq!(
        preview,
        core.resolve_prompt_preview(
            &scenario.request,
            GenerationOperationContext::Resume {
                generation_attempt_id: generation_id,
            },
        )
        .expect("repeat fully closed attempt preview"),
        "same-process re-preview must be exact"
    );
    assert_authority_preview_text(&preview);
    let trace = core
        .explain_prompt_plan(
            &scenario.request,
            GenerationOperationContext::Resume {
                generation_attempt_id: generation_id,
            },
            &preview.plan.plan_hash,
        )
        .expect("explain fully sealed prompt authority");
    assert_authority_preview_context(&trace, scenario, drift);
    (preview, trace)
}

fn assert_authority_wire_body(body: &serde_json::Value) {
    let text = body.to_string();
    for expected in [
        "root-child-visible",
        "knowledge-child-visible",
        "sealed-capability-visible",
        "second-approval-complete",
        KNOWLEDGE_TEXT,
        SEALED_CHARACTER_DESCRIPTION,
        SEALED_CHARACTER_PERSONALITY,
        SEALED_CHARACTER_KNOWLEDGE,
        SEALED_PERSONA_DESCRIPTION,
        SEALED_SELECTED_MEMORY,
        SEALED_CONVERSATION_SUMMARY,
        SEALED_USER,
        SEALED_SLOT,
    ] {
        assert!(
            text.contains(expected),
            "provider request omitted {expected}: {text}"
        );
    }
    assert!(!text.contains(DRIFTED_USER));
    assert!(!text.contains(DRIFTED_SLOT));
    for drifted in [
        DRIFTED_CHARACTER_DESCRIPTION,
        DRIFTED_CHARACTER_PERSONALITY,
        DRIFTED_CHARACTER_KNOWLEDGE,
        DRIFTED_PERSONA_DESCRIPTION,
        DRIFTED_SELECTED_MEMORY,
        DRIFTED_CONVERSATION_SUMMARY,
    ] {
        assert!(!text.contains(drifted));
    }
    assert_eq!(
        body.get("temperature").and_then(serde_json::Value::as_f64),
        Some(0.3)
    );
    assert_eq!(
        body.get("max_tokens").and_then(serde_json::Value::as_u64),
        Some(512)
    );
}

fn dispatch_authority_preview(
    core: Core,
    scenario: &mut AuthorityScenario,
    generation_id: &GenerationId,
    preview: &ExpertPromptPreview,
    trace: &PromptResolutionTrace,
) {
    drop(core);
    let core = open_core_after_drop(scenario.root.path());
    let reopened = core
        .resolve_prompt_preview(
            &scenario.request,
            GenerationOperationContext::Resume {
                generation_attempt_id: generation_id,
            },
        )
        .expect("resolve exact preview after restart");
    assert_eq!(reopened, *preview);
    assert_eq!(
        core.explain_prompt_plan(
            &scenario.request,
            GenerationOperationContext::Resume {
                generation_attempt_id: generation_id,
            },
            &reopened.plan.plan_hash,
        )
        .expect("explain exact sealed prompt after restart"),
        *trace
    );
    let mut reviewed = scenario.request.clone();
    reviewed.expected_plan_hash = Some(reopened.plan.plan_hash.clone());
    let dispatched = core
        .send_message_with_prompt_plan(
            &reviewed,
            &reopened.generation_attempt_id,
            credential(scenario.root.path()),
        )
        .expect("dispatch exact closed attempt");
    assert_eq!(dispatched, *generation_id);
    wait_for_generation(&core, &scenario.branch.id, &dispatched);
    let wire = scenario
        .requests
        .recv_timeout(Duration::from_secs(5))
        .expect("receive synthetic provider request");
    let wire_body = request_body(&wire);
    assert_authority_wire_body(&wire_body);
    let stored_plan = core
        .get_generation_prompt_plan(generation_id)
        .expect("load stored generation prompt plan");
    assert_eq!(stored_plan.id, reopened.plan.plan_id);
    assert_eq!(
        stored_plan.provider_request.request.value,
        reopened.provider_request
    );
    assert_eq!(stored_plan.provider_request.request.value, wire_body);
    let stored: lorepia_domain::ResolvedPromptPlan =
        serde_json::from_value(stored_plan.plan.value.clone())
            .expect("decode stored sealed prompt plan");
    assert_eq!(stored.trace.context_snapshot, trace.context_snapshot);
    assert!(
        core.get_interaction_state_revision(&scenario.conversation.id, &scenario.branch.id)
            .expect("load materialized interaction revision")
            > scenario.live_revision_before_attempt
    );
    drop(core);
    scenario
        .provider
        .take()
        .expect("synthetic provider handle")
        .join()
        .expect("join synthetic provider");
}

fn assert_stored_prompt_authority(
    before: &lorepia_storage::StoredGenerationAttemptBeforeReview,
    scenario: &AuthorityScenario,
) {
    assert_eq!(before.closure_authority_version, 1);
    let authority = &before.prompt_selection_authority;
    assert_eq!(authority.mode, ConversationMode::Chat);
    assert_eq!(
        authority.local_user_id_sha256,
        scenario.sealed_local_user_id_sha256
    );
    assert_eq!(
        authority.character.description,
        SEALED_CHARACTER_DESCRIPTION
    );
    let content = authority
        .character_content
        .as_ref()
        .expect("sealed character-content selection authority");
    assert_eq!(
        content.revision_id,
        scenario.sealed_character_content.revision_id
    );
    assert_eq!(content.value.personality, SEALED_CHARACTER_PERSONALITY);
    let book = authority
        .character_knowledge_book
        .as_ref()
        .expect("sealed linked character-knowledge authority");
    assert_eq!(
        book.revision_id,
        scenario.sealed_character_knowledge.revision_id
    );
    assert_eq!(book.value.entries[0].content, SEALED_CHARACTER_KNOWLEDGE);
    assert_eq!(
        authority.quick_settings.response_length,
        PromptResponseLength::Short
    );
    assert_eq!(authority.quick_settings.creativity, 20);
    assert_eq!(
        authority.quick_settings.reasoning_effort,
        Some(GenerationReasoningEffort::High)
    );
    assert!(authority.quick_settings.memory_enabled);
    assert!(authority.quick_settings.knowledge_enabled);
    assert!(authority.quick_settings.supports_temperature);
    assert_eq!(authority.quick_settings.resolved_temperature, Some(0.3));
    assert_eq!(
        authority.quick_settings.resolved_max_output_tokens,
        Some(512)
    );
    assert_eq!(
        authority
            .binding
            .as_ref()
            .expect("sealed prompt-binding authority")
            .revision,
        scenario
            .sealed_room
            .binding_revision
            .expect("sealed binding revision")
    );
    let persona = authority
        .persona_selection
        .as_ref()
        .expect("sealed persona-selection authority");
    assert_eq!(
        persona.revision,
        scenario
            .sealed_persona_selection
            .revision
            .expect("sealed persona selection revision")
    );
    assert_eq!(persona.value.persona_id, scenario.sealed_persona.value.id);
}

fn assert_stored_memory_and_live_authority(
    storage: &Storage,
    before: &lorepia_storage::StoredGenerationAttemptBeforeReview,
    scenario: &AuthorityScenario,
) {
    let selected = before
        .memory_head_snapshot
        .records
        .iter()
        .find(|record| record.record_id == scenario.sealed_selected_memory.value.id)
        .expect("sealed selected-memory evidence");
    assert_eq!(
        selected.active_revision_id,
        scenario
            .sealed_selected_memory
            .revision_id
            .as_deref()
            .expect("sealed selected-memory immutable revision")
    );
    let summary = before
        .memory_head_snapshot
        .records
        .iter()
        .find(|record| record.record_id == scenario.sealed_summary_memory.value.id)
        .expect("sealed summary-memory evidence");
    assert_eq!(
        summary.active_revision_id,
        scenario
            .sealed_summary_memory
            .revision_id
            .as_deref()
            .expect("sealed summary-memory immutable revision")
    );
    assert_eq!(
        before.memory_head_snapshot.context_head_message_id.as_ref(),
        Some(&scenario.authority_head)
    );
    assert_eq!(
        storage
            .get_character(&scenario.character_id)
            .expect("load drifted live character")
            .description,
        DRIFTED_CHARACTER_DESCRIPTION
    );
    assert_eq!(
        storage
            .get_character_content(&scenario.character_id)
            .expect("load drifted live character content")
            .value
            .personality,
        DRIFTED_CHARACTER_PERSONALITY
    );
    assert_eq!(
        storage
            .get_knowledge_book(&KnowledgeBookId::from(CHARACTER_KNOWLEDGE_BOOK_ID))
            .expect("load drifted live linked character knowledge")
            .value
            .entries[0]
            .content,
        DRIFTED_CHARACTER_KNOWLEDGE
    );
}

fn assert_stored_derived_closure(
    before: &lorepia_storage::StoredGenerationAttemptBeforeReview,
    scenario: &AuthorityScenario,
) {
    assert_eq!(
        before.derived_closure.event_count as usize,
        before.derived_closure.transitions.len()
    );
    assert_eq!(
        before.derived_closure.guard_count as usize,
        before.derived_closure.guard_audits.len()
    );
    assert_eq!(
        generation_attempt_derived_closure_sha256(&before.derived_closure)
            .expect("rehash immutable BeforeGeneration closure"),
        before.derived_closure_sha256
    );
    assert_eq!(
        interaction_evaluation_seal_sha256(&before.evaluation_seal)
            .expect("rehash immutable BeforeGeneration evaluation seal"),
        before.evaluation_seal_sha256
    );
    let last = before
        .derived_closure
        .transitions
        .last()
        .expect("BeforeGeneration closure contains its root transition");
    assert_eq!(before.derived_closure.final_state, last.next_state);
    assert_eq!(before.derived_closure.final_knowledge, last.knowledge);
    for transition in &before.derived_closure.transitions {
        assert_eq!(
            transition.evaluation_seal, before.evaluation_seal,
            "every transition in one closed evaluation must retain the same seal"
        );
    }
    assert!(
        before
            .evaluation_seal
            .supported_capabilities
            .contains(&CapabilityKey::JsonMode),
        "the immutable evaluation seal must retain the pre-drift capability"
    );
    assert!(before.derived_closure.transitions.iter().any(|transition| {
        matches!(transition.event, InteractionEvent::BeforeGeneration)
            && transition.parent_ordinal.is_none()
    }));
    assert!(before.derived_closure.transitions.iter().any(|transition| {
        matches!(
            &transition.event,
            InteractionEvent::VariableChanged { variable }
                if variable == &scenario.fixture.variables.root
        )
    }));
    assert!(before.derived_closure.transitions.iter().any(|transition| {
        matches!(
            &transition.event,
            InteractionEvent::KnowledgeActivated { entry_id }
                if entry_id == &scenario.fixture.knowledge_entry_id
        )
    }));
}

fn assert_stored_authority_aggregate(
    storage: &Storage,
    before: &lorepia_storage::StoredGenerationAttemptBeforeReview,
    scenario: &AuthorityScenario,
    generation_id: &GenerationId,
) {
    let aggregate = storage
        .get_generation_attempt_interaction_aggregate(generation_id)
        .expect("load final generation interaction aggregate");
    assert_eq!(aggregate.closure_authority_version, 1);
    assert_eq!(
        aggregate.evaluation_seal_sha256,
        before.evaluation_seal_sha256
    );
    assert!(
        aggregate.derived_event_count >= before.derived_closure.event_count,
        "approval closures must extend the aggregate event count"
    );
    assert!(
        aggregate.derived_guard_count >= before.derived_closure.guard_count,
        "approval closures must preserve the aggregate guard count"
    );
    assert_eq!(aggregate.pending_proposal_count, 0);
    for (variable, expected) in [
        (&scenario.fixture.variables.root, "root-applied"),
        (&scenario.fixture.variables.child, "root-child-visible"),
        (
            &scenario.fixture.variables.knowledge_child,
            "knowledge-child-visible",
        ),
        (
            &scenario.fixture.variables.capability_child,
            "sealed-capability-visible",
        ),
        (
            &scenario.fixture.variables.final_child,
            "second-approval-complete",
        ),
    ] {
        assert_eq!(
            aggregate.state.variables.get(variable),
            Some(&VariableValue::Text(expected.to_owned()))
        );
    }
    assert!(
        aggregate
            .state
            .manually_active_knowledge
            .contains(&scenario.fixture.knowledge_entry_id)
    );
    let live = storage
        .get_interaction_state_snapshot(&scenario.conversation.id, &scenario.branch.id)
        .expect("load live materialized interaction state");
    assert_eq!(live.state, aggregate.state);
    assert_eq!(live.knowledge, aggregate.knowledge);
    assert_eq!(
        storage
            .interaction_derived_event_supervisor_status()
            .expect("read derived supervisor status")
            .pending_count,
        0,
        "attempt append must leave no live derived outbox residue"
    );
}

fn assert_authority_sql_evidence(root: &std::path::Path, generation_id: &GenerationId) {
    let connection = Connection::open(active_database_path(root))
        .expect("open read-only closure evidence connection");
    let live_outbox_count = connection
        .query_row(
            "SELECT COUNT(*)
             FROM interaction_derived_event_outbox AS occurrence
             WHERE occurrence.status != 'acknowledged'
               AND NOT EXISTS (
                   SELECT 1 FROM interaction_derived_event_quarantines AS quarantine
                   WHERE quarantine.occurrence_id = occurrence.occurrence_id
               )",
            [],
            |row| row.get::<_, u64>(0),
        )
        .expect("count live derived outbox rows");
    assert_eq!(live_outbox_count, 0);
    let duplicate_events = connection
        .query_row(
            "SELECT COUNT(*) FROM (
                 SELECT idempotency_key
                 FROM interaction_events
                 WHERE generation_attempt_id = ?1
                 GROUP BY idempotency_key
                 HAVING COUNT(*) > 1
             )",
            [generation_id.0.as_str()],
            |row| row.get::<_, u64>(0),
        )
        .expect("count duplicate attempt event identities");
    assert_eq!(duplicate_events, 0);
}

fn assert_completed_authority_storage(scenario: &AuthorityScenario, generation_id: &GenerationId) {
    let storage =
        Storage::open(scenario.root.path()).expect("open storage after completed dispatch");
    let attempt = storage
        .get_generation_attempt(generation_id)
        .expect("load completed generation attempt");
    assert_eq!(attempt.status, GenerationAttemptStatus::Completed);
    let before = storage
        .get_generation_attempt_before_review(generation_id)
        .expect("load immutable BeforeGeneration closure")
        .expect("BeforeGeneration closure exists");
    assert_stored_prompt_authority(&before, scenario);
    assert_stored_memory_and_live_authority(&storage, &before, scenario);
    assert_stored_derived_closure(&before, scenario);
    assert_stored_authority_aggregate(&storage, &before, scenario, generation_id);
    drop(storage);
    assert_authority_sql_evidence(scenario.root.path(), generation_id);
}

#[test]
fn generation_attempt_closes_children_before_dispatch_and_reuses_the_sealed_context() {
    let mut scenario = prepare_authority_scenario();
    let (generation_id, first_request) = begin_authority_review(&scenario);
    let drift = apply_authority_drift(&scenario);
    let core = approve_nested_authority_children(&scenario, &generation_id, &first_request, &drift);
    let (preview, trace) = resolve_authority_preview(&core, &scenario, &drift, &generation_id);
    dispatch_authority_preview(core, &mut scenario, &generation_id, &preview, &trace);
    assert_completed_authority_storage(&scenario, &generation_id);
}

struct HistoricalForkBase {
    root: TempDir,
    requests: mpsc::Receiver<Vec<u8>>,
    provider: thread::JoinHandle<()>,
    core: Core,
    target: GenerationTarget,
    conversation: Conversation,
    source_branch: ConversationBranch,
    source_messages: Vec<Message>,
    source_user_id: MessageId,
    source_head_id: MessageId,
}

struct HistoricalForkScenario {
    root: TempDir,
    requests: mpsc::Receiver<Vec<u8>>,
    provider: Option<thread::JoinHandle<()>>,
    target: GenerationTarget,
    conversation: Conversation,
    source_branch: ConversationBranch,
    source_messages: Vec<Message>,
    source_user_id: MessageId,
    source_head_id: MessageId,
    fixture: ClosureFixture,
    sealed_room: RoomOrchestrationConfig,
    source_state_revision: u64,
}

fn prepare_historical_fork_base() -> HistoricalForkBase {
    let root = tempdir().expect("create temporary fork Core root");
    let (origin, requests, provider) = spawn_provider(2);
    let core = Core::open(CoreConfig::new(root.path())).expect("open fork Core");
    let character_id = import_character(&core);
    let target = provider_fixture(&core, &origin);
    let core = reopen_with_provider_credential_authority(core, root.path());
    core.select_generation_target(Some(target.clone()))
        .expect("select fork generation target");
    set_json_mode_capability(&core, &target.model_route_id, true);
    let conversation = core
        .open_conversation(&character_id)
        .expect("open fork conversation through lifecycle boundary");
    let source_branch = core
        .list_conversation_branches(&conversation.id)
        .expect("list fork source branches")
        .into_iter()
        .next()
        .expect("fork source branch");
    let lifecycle = core
        .drain_core_lifecycle_occurrences(64)
        .expect("initialize fork source interaction boundary");
    assert!(lifecycle.queue_idle);
    let generation_id = core
        .send_message_to_branch_with_connection_credential(
            &conversation.id,
            &source_branch.id,
            None,
            ConversationMode::Chat,
            "SYNTHETIC_FORK_BASELINE_USER_91C4",
            GenerationOperationContext::New {
                operation_nonce: "historical-fork-baseline-send-v1",
            },
            &target,
            credential(root.path()),
        )
        .expect("send baseline source turn");
    wait_for_generation(&core, &source_branch.id, &generation_id);
    let wire = requests
        .recv_timeout(Duration::from_secs(5))
        .expect("receive baseline provider request");
    assert!(
        request_body(&wire)
            .to_string()
            .contains("SYNTHETIC_FORK_BASELINE_USER_91C4")
    );
    let lifecycle = core
        .drain_core_lifecycle_occurrences(64)
        .expect("materialize baseline source checkpoint");
    assert!(lifecycle.queue_idle);
    let source_messages = core
        .list_branch_messages(&source_branch.id)
        .expect("load baseline source messages");
    assert_eq!(source_messages.len(), 2);
    let source_user_id = source_messages[0].id.clone();
    let source_head_id = source_messages[1].id.clone();
    HistoricalForkBase {
        root,
        requests,
        provider,
        core,
        target,
        conversation,
        source_branch,
        source_messages,
        source_user_id,
        source_head_id,
    }
}

fn prepare_historical_fork_scenario() -> HistoricalForkScenario {
    let base = prepare_historical_fork_base();
    let fixture = install_closure_fixture(
        &base.core,
        ContentModuleRuntimeTarget {
            conversation_id: base.conversation.id.clone(),
            branch_id: base.source_branch.id.clone(),
        },
    );
    let room = base
        .core
        .get_room_orchestration_config(&base.conversation.id, &base.source_branch.id)
        .expect("load source room context");
    let sealed_room = save_authority_room_context(
        &base.core,
        &room,
        &base.target,
        &AuthorityRoomSettings {
            prompt_preset_id: &room.prompt_preset_id,
            response_length: room.response_length,
            creativity: room.creativity,
            reasoning_effort: Some(GenerationReasoningEffort::High),
            memory_enabled: room.memory_enabled,
            knowledge_enabled: room.knowledge_enabled,
            user_name: SEALED_USER,
            slot: SEALED_SLOT,
        },
    );
    let source_state_revision = base
        .core
        .get_interaction_state_revision(&base.conversation.id, &base.source_branch.id)
        .expect("capture source checkpoint interaction revision");
    drop(base.core);
    HistoricalForkScenario {
        root: base.root,
        requests: base.requests,
        provider: Some(base.provider),
        target: base.target,
        conversation: base.conversation,
        source_branch: base.source_branch,
        source_messages: base.source_messages,
        source_user_id: base.source_user_id,
        source_head_id: base.source_head_id,
        fixture,
        sealed_room,
        source_state_revision,
    }
}

fn start_historical_fork_review(
    scenario: &HistoricalForkScenario,
    operation_nonce: &str,
) -> (
    GenerationId,
    lorepia_core::GenerationAttemptProposalDecisionRequest,
) {
    let core = open_core_after_drop(scenario.root.path());
    let blocked = core
        .edit_user_message_with_connection_credential(
            &scenario.conversation.id,
            &scenario.source_branch.id,
            Some(&scenario.source_head_id),
            &scenario.source_user_id,
            "SYNTHETIC_FORK_EDITED_USER_4A77",
            GenerationOperationContext::New { operation_nonce },
            &scenario.target,
            credential(scenario.root.path()),
        )
        .expect_err("fork attempt must stop at its derived child proposal");
    assert_eq!(
        blocked.code,
        CoreErrorCode::PermissionDenied,
        "unexpected fork attempt failure: {blocked:?}"
    );
    assert_eq!(
        core.get_interaction_state_revision(&scenario.conversation.id, &scenario.source_branch.id,)
            .expect("read isolated source interaction revision"),
        scenario.source_state_revision,
        "an unappended fork attempt must not mutate the source checkpoint"
    );
    let pending = core
        .list_generation_attempt_proposals_for_source_room(
            &scenario.conversation.id,
            &scenario.source_branch.id,
            InteractionProposalStatus::Pending,
            10,
        )
        .expect("list fork first proposal");
    let [first] = pending.as_slice() else {
        panic!("expected one fork first proposal, got {pending:?}");
    };
    assert_eq!(first.proposal.record.proposal_id, "approve-first-child");
    let generation_id = first.proposal.generation_id.clone();
    let receipt = core
        .decide_generation_attempt_proposal(
            &lorepia_core::GenerationAttemptProposalDecisionRequest {
                conversation_id: scenario.conversation.id.clone(),
                source_branch_id: scenario.source_branch.id.clone(),
                generation_id: generation_id.clone(),
                proposal_record_id: first.proposal.record.id.clone(),
                expected_aggregate_revision: first.aggregate_revision,
                expected_proposal_revision: first.proposal.proposal_revision,
                decision: InteractionProposalDecision::Approve,
            },
        )
        .expect("approve fork first child");
    assert_eq!(receipt.pending_proposal_count, 1);
    let pending = core
        .list_generation_attempt_proposals_for_source_room(
            &scenario.conversation.id,
            &scenario.source_branch.id,
            InteractionProposalStatus::Pending,
            10,
        )
        .expect("list fork second proposal");
    let [second] = pending.as_slice() else {
        panic!("expected one fork second proposal, got {pending:?}");
    };
    assert_eq!(second.proposal.generation_id, generation_id);
    assert_eq!(second.proposal.record.proposal_id, "approve-second-child");
    let second_request = lorepia_core::GenerationAttemptProposalDecisionRequest {
        conversation_id: scenario.conversation.id.clone(),
        source_branch_id: scenario.source_branch.id.clone(),
        generation_id: generation_id.clone(),
        proposal_record_id: second.proposal.record.id.clone(),
        expected_aggregate_revision: second.aggregate_revision,
        expected_proposal_revision: second.proposal.proposal_revision,
        decision: InteractionProposalDecision::Approve,
    };
    (generation_id, second_request)
}

fn approve_historical_fork_second_after_restart(
    scenario: &HistoricalForkScenario,
    second_request: &lorepia_core::GenerationAttemptProposalDecisionRequest,
) -> Core {
    let core = open_core_after_drop(scenario.root.path());
    let pending = core
        .list_generation_attempt_proposals_for_source_room(
            &scenario.conversation.id,
            &scenario.source_branch.id,
            InteractionProposalStatus::Pending,
            10,
        )
        .expect("rediscover fork second proposal");
    assert_eq!(pending.len(), 1);
    assert_eq!(
        pending[0].proposal.record.id,
        second_request.proposal_record_id
    );
    let receipt = core
        .decide_generation_attempt_proposal(second_request)
        .expect("approve fork second child after restart");
    assert_eq!(receipt.pending_proposal_count, 0);
    core
}

fn drift_historical_fork_authority(scenario: &HistoricalForkScenario) {
    let core = open_core_after_drop(scenario.root.path());
    set_json_mode_capability(&core, &scenario.target.model_route_id, false);
    let current_room = core
        .get_room_orchestration_config(&scenario.conversation.id, &scenario.source_branch.id)
        .expect("reload sealed fork room context");
    assert_eq!(
        current_room.binding_revision,
        scenario.sealed_room.binding_revision
    );
    let drifted_room = save_authority_room_context(
        &core,
        &current_room,
        &scenario.target,
        &AuthorityRoomSettings {
            prompt_preset_id: &current_room.prompt_preset_id,
            response_length: current_room.response_length,
            creativity: current_room.creativity,
            reasoning_effort: Some(GenerationReasoningEffort::Low),
            memory_enabled: current_room.memory_enabled,
            knowledge_enabled: current_room.knowledge_enabled,
            user_name: DRIFTED_USER,
            slot: DRIFTED_SLOT,
        },
    );
    assert_ne!(
        drifted_room.binding_revision,
        scenario.sealed_room.binding_revision
    );
    assert_eq!(
        core.set_conversation_mode(&scenario.conversation.id, ConversationMode::Story)
            .expect("drift fork live conversation mode")
            .selected_mode,
        ConversationMode::Story
    );
}

fn drift_historical_fork_provider_timeout(scenario: &HistoricalForkScenario) {
    let core = Core::open(CoreConfig::new(scenario.root.path()))
        .expect("reopen fork Core for provider drift");
    let mut connection = core
        .list_provider_connections()
        .expect("list fork provider connections")
        .into_iter()
        .find(|connection| connection.id.as_str() == CONNECTION_ID)
        .expect("find fork provider connection");
    let sealed_timeout = connection.timeout_seconds;
    connection.timeout_seconds = sealed_timeout.saturating_add(1);
    let updated = core
        .upsert_provider_connection(connection)
        .expect("drift mutable fork provider timeout through the public boundary");
    assert_ne!(updated.timeout_seconds, sealed_timeout);
}

fn historical_fork_attempt_and_branch_counts(scenario: &HistoricalForkScenario) -> (u64, u64) {
    let connection = Connection::open(active_database_path(scenario.root.path()))
        .expect("open historical fork nonce evidence connection");
    let attempt_count = connection
        .query_row(
            "SELECT COUNT(*)
             FROM generation_attempt_intents
             WHERE conversation_id = ?1",
            [scenario.conversation.id.0.as_str()],
            |row| row.get::<_, u64>(0),
        )
        .expect("count historical fork generation attempts");
    let branch_count = connection
        .query_row(
            "SELECT COUNT(*)
             FROM conversation_branches
             WHERE conversation_id = ?1",
            [scenario.conversation.id.0.as_str()],
            |row| row.get::<_, u64>(0),
        )
        .expect("count historical fork branches");
    (attempt_count, branch_count)
}

fn assert_historical_resume_rejects_provider_drift(
    scenario: &HistoricalForkScenario,
    generation_id: &GenerationId,
) {
    assert_eq!(
        historical_fork_attempt_and_branch_counts(scenario),
        (2, 1),
        "only the completed baseline and reviewed edit attempt may exist before resume"
    );
    let core = open_core_after_drop(scenario.root.path());
    let error = core
        .edit_user_message_with_connection_credential(
            &scenario.conversation.id,
            &scenario.source_branch.id,
            Some(&scenario.source_head_id),
            &scenario.source_user_id,
            "SYNTHETIC_FORK_EDITED_USER_4A77",
            GenerationOperationContext::Resume {
                generation_attempt_id: generation_id,
            },
            &scenario.target,
            credential(scenario.root.path()),
        )
        .expect_err("provider drift must reject the exact reviewed fork attempt");
    assert_eq!(error.code, CoreErrorCode::InvalidInput);
    assert!(error.recoverable);
    assert!(
        error
            .message
            .contains("provider configuration changed after generation review; start a new generation operation"),
        "unexpected provider-drift error: {error:?}"
    );
    assert_eq!(
        core.list_conversation_branches(&scenario.conversation.id)
            .expect("list branches after rejected reviewed fork")
            .len(),
        1
    );
    assert_eq!(
        core.list_branch_messages(&scenario.source_branch.id)
            .expect("reload source messages after rejected reviewed fork"),
        scenario.source_messages
    );
    drop(core);
    assert_eq!(historical_fork_attempt_and_branch_counts(scenario), (2, 1));
    let storage =
        Storage::open(scenario.root.path()).expect("open reviewed provider-drift attempt evidence");
    let attempt = storage
        .get_generation_attempt(generation_id)
        .expect("load reviewed provider-drift attempt");
    assert_eq!(
        attempt.status,
        GenerationAttemptStatus::BeforeGenerationApplied
    );
    assert!(attempt.dispatch_seal.is_none());
    assert!(matches!(
        scenario.requests.try_recv(),
        Err(mpsc::TryRecvError::Empty)
    ));
}

fn assert_historical_resume_hijacks_rejected(
    scenario: &HistoricalForkScenario,
    generation_id: &GenerationId,
) {
    assert_eq!(historical_fork_attempt_and_branch_counts(scenario), (2, 1));
    let core = open_core_after_drop(scenario.root.path());
    let wrong_head = core
        .edit_user_message_with_connection_credential(
            &scenario.conversation.id,
            &scenario.source_branch.id,
            Some(&scenario.source_user_id),
            &scenario.source_user_id,
            "SYNTHETIC_FORK_EDITED_USER_4A77",
            GenerationOperationContext::Resume {
                generation_attempt_id: generation_id,
            },
            &scenario.target,
            credential(scenario.root.path()),
        )
        .expect_err("a reviewed edit attempt must reject a different expected source head");
    let wrong_action = core
        .regenerate_assistant_message_with_connection_credential(
            &scenario.conversation.id,
            &scenario.source_branch.id,
            Some(&scenario.source_head_id),
            &scenario.source_head_id,
            GenerationOperationContext::Resume {
                generation_attempt_id: generation_id,
            },
            &scenario.target,
            credential(scenario.root.path()),
        )
        .expect_err("a reviewed edit attempt must not resume through regenerate");
    for error in [wrong_head, wrong_action] {
        assert_eq!(error.code, CoreErrorCode::InvalidInput);
        assert!(error.recoverable);
        assert!(
            error.message.contains("start a new generation operation"),
            "unexpected resume-hijack error: {error:?}"
        );
    }
    assert_eq!(historical_fork_attempt_and_branch_counts(scenario), (2, 1));
    assert_eq!(
        core.list_branch_messages(&scenario.source_branch.id)
            .expect("reload source messages after resume-hijack checks"),
        scenario.source_messages
    );
    drop(core);
    let attempt = Storage::open(scenario.root.path())
        .expect("open resume-hijack attempt evidence")
        .get_generation_attempt(generation_id)
        .expect("load resume-hijack attempt evidence");
    assert_eq!(
        attempt.status,
        GenerationAttemptStatus::BeforeGenerationApplied
    );
    assert!(attempt.dispatch_seal.is_none());
    assert!(matches!(
        scenario.requests.try_recv(),
        Err(mpsc::TryRecvError::Empty)
    ));
}

fn assert_historical_fork_wire(body: &serde_json::Value) {
    let text = body.to_string();
    for expected in [
        "SYNTHETIC_FORK_EDITED_USER_4A77",
        "root-child-visible",
        "knowledge-child-visible",
        "sealed-capability-visible",
        "second-approval-complete",
        KNOWLEDGE_TEXT,
        SEALED_USER,
        SEALED_SLOT,
    ] {
        assert!(
            text.contains(expected),
            "fork provider request omitted {expected}: {text}"
        );
    }
    assert!(
        !text.contains("SYNTHETIC_FORK_BASELINE_USER_91C4"),
        "the replaced source user message must not leak into the fork prompt"
    );
    assert!(!text.contains(DRIFTED_USER));
    assert!(!text.contains(DRIFTED_SLOT));
}

fn assert_historical_fork_materialization(
    core: &Core,
    scenario: &HistoricalForkScenario,
    action: &MessageActionGeneration,
) {
    assert_eq!(
        core.get_interaction_state_revision(&scenario.conversation.id, &scenario.source_branch.id,)
            .expect("reload unchanged source interaction revision"),
        scenario.source_state_revision
    );
    assert_eq!(
        core.list_branch_messages(&scenario.source_branch.id)
            .expect("reload unchanged source messages"),
        scenario.source_messages
    );
    let fork_messages = core
        .list_branch_messages(&action.branch.id)
        .expect("load materialized fork messages");
    assert_eq!(fork_messages.len(), 2);
    assert_eq!(fork_messages[0].content, "SYNTHETIC_FORK_EDITED_USER_4A77");
}

fn resume_historical_fork(
    scenario: &mut HistoricalForkScenario,
    generation_id: &GenerationId,
    second_request: &lorepia_core::GenerationAttemptProposalDecisionRequest,
) -> MessageActionGeneration {
    let core = approve_historical_fork_second_after_restart(scenario, second_request);
    let action = core
        .edit_user_message_with_connection_credential(
            &scenario.conversation.id,
            &scenario.source_branch.id,
            Some(&scenario.source_head_id),
            &scenario.source_user_id,
            "SYNTHETIC_FORK_EDITED_USER_4A77",
            GenerationOperationContext::Resume {
                generation_attempt_id: generation_id,
            },
            &scenario.target,
            credential(scenario.root.path()),
        )
        .expect("resume the exact reviewed fork attempt");
    assert_eq!(action.generation_id, *generation_id);
    assert_ne!(action.branch.id, scenario.source_branch.id);
    wait_for_generation(&core, &action.branch.id, &action.generation_id);
    let wire = scenario
        .requests
        .recv_timeout(Duration::from_secs(5))
        .expect("receive fork provider request");
    assert_historical_fork_wire(&request_body(&wire));
    assert_historical_fork_materialization(&core, scenario, &action);
    drop(core);
    scenario
        .provider
        .take()
        .expect("fork provider handle")
        .join()
        .expect("join fork synthetic provider");
    action
}

fn assert_historical_fork_before_snapshot(
    before: &lorepia_storage::StoredGenerationAttemptBeforeReview,
    scenario: &HistoricalForkScenario,
) {
    assert_eq!(before.closure_authority_version, 1);
    assert_eq!(
        before.prompt_selection_authority.mode,
        ConversationMode::Chat
    );
    assert_eq!(
        before
            .prompt_selection_authority
            .quick_settings
            .reasoning_effort,
        Some(GenerationReasoningEffort::High)
    );
    assert_eq!(
        before
            .prompt_selection_authority
            .binding
            .as_ref()
            .expect("sealed fork prompt binding")
            .revision,
        scenario
            .sealed_room
            .binding_revision
            .expect("sealed fork binding revision")
    );
    assert_eq!(
        generation_attempt_derived_closure_sha256(&before.derived_closure)
            .expect("rehash immutable fork closure"),
        before.derived_closure_sha256
    );
    let last = before
        .derived_closure
        .transitions
        .last()
        .expect("fork closure contains its root transition");
    assert_eq!(before.derived_closure.final_state, last.next_state);
    assert_eq!(before.derived_closure.final_knowledge, last.knowledge);
    for transition in &before.derived_closure.transitions {
        assert_eq!(
            transition.evaluation_seal, before.evaluation_seal,
            "every fork transition must retain the same immutable seal"
        );
    }
}

fn assert_historical_fork_aggregate(
    storage: &Storage,
    before: &lorepia_storage::StoredGenerationAttemptBeforeReview,
    scenario: &HistoricalForkScenario,
    generation_id: &GenerationId,
    action: &MessageActionGeneration,
) {
    let aggregate = storage
        .get_generation_attempt_interaction_aggregate(generation_id)
        .expect("load final fork aggregate");
    assert_eq!(aggregate.closure_authority_version, 1);
    assert_eq!(
        aggregate.evaluation_seal_sha256,
        before.evaluation_seal_sha256
    );
    assert!(
        aggregate.derived_event_count >= before.derived_closure.event_count,
        "fork approval closures must extend the aggregate event count"
    );
    assert_eq!(aggregate.pending_proposal_count, 0);
    assert_eq!(
        aggregate
            .state
            .variables
            .get(&scenario.fixture.variables.final_child),
        Some(&VariableValue::Text("second-approval-complete".to_owned()))
    );
    let target_state = storage
        .get_interaction_state_snapshot(&scenario.conversation.id, &action.branch.id)
        .expect("load fork target interaction state");
    assert_eq!(target_state.state, aggregate.state);
    assert_eq!(target_state.knowledge, aggregate.knowledge);
    let source_state = storage
        .get_interaction_state_snapshot(&scenario.conversation.id, &scenario.source_branch.id)
        .expect("load preserved source interaction state");
    assert_eq!(source_state.state.revision, scenario.source_state_revision);
    assert_ne!(source_state.state, target_state.state);
    assert_eq!(
        storage
            .interaction_derived_event_supervisor_status()
            .expect("read fork derived supervisor status")
            .pending_count,
        0,
        "fork append must leave no live derived outbox residue"
    );
}

fn assert_historical_fork_event_branch(
    root: &std::path::Path,
    generation_id: &GenerationId,
    action: &MessageActionGeneration,
) {
    let connection =
        Connection::open(active_database_path(root)).expect("open fork event evidence connection");
    let branches = connection
        .prepare(
            "SELECT DISTINCT branch_id
             FROM interaction_events
             WHERE generation_attempt_id = ?1
             ORDER BY branch_id",
        )
        .expect("prepare fork event branch query")
        .query_map([generation_id.0.as_str()], |row| row.get::<_, String>(0))
        .expect("query fork event branches")
        .collect::<Result<Vec<_>, _>>()
        .expect("collect fork event branches");
    assert_eq!(branches, vec![action.branch.id.0.clone()]);
}

fn assert_completed_historical_fork(
    scenario: &HistoricalForkScenario,
    generation_id: &GenerationId,
    action: &MessageActionGeneration,
) {
    let storage = Storage::open(scenario.root.path()).expect("open fork storage after dispatch");
    let before = storage
        .get_generation_attempt_before_review(generation_id)
        .expect("load immutable fork BeforeGeneration closure")
        .expect("fork BeforeGeneration closure exists");
    assert_historical_fork_before_snapshot(&before, scenario);
    assert_historical_fork_aggregate(&storage, &before, scenario, generation_id, action);
    drop(storage);
    assert_historical_fork_event_branch(scenario.root.path(), generation_id, action);
}

#[test]
fn historical_edit_materializes_the_closed_chain_only_on_its_fork() {
    let mut scenario = prepare_historical_fork_scenario();
    let (generation_id, second_request) =
        start_historical_fork_review(&scenario, "historical-fork-edit-review-v1");
    drift_historical_fork_authority(&scenario);
    let action = resume_historical_fork(&mut scenario, &generation_id, &second_request);
    assert_completed_historical_fork(&scenario, &generation_id, &action);
}

#[test]
fn historical_edit_provider_drift_requires_a_new_nonce_and_fresh_approvals() {
    let mut scenario = prepare_historical_fork_scenario();
    let (reviewed_generation_id, reviewed_second_request) =
        start_historical_fork_review(&scenario, "historical-fork-provider-drift-reviewed-v1");
    drop(approve_historical_fork_second_after_restart(
        &scenario,
        &reviewed_second_request,
    ));
    assert_historical_resume_hijacks_rejected(&scenario, &reviewed_generation_id);
    drift_historical_fork_provider_timeout(&scenario);
    assert_historical_resume_rejects_provider_drift(&scenario, &reviewed_generation_id);

    let (fresh_generation_id, fresh_second_request) =
        start_historical_fork_review(&scenario, "historical-fork-provider-drift-fresh-v2");
    assert_ne!(fresh_generation_id, reviewed_generation_id);
    assert_eq!(
        historical_fork_attempt_and_branch_counts(&scenario),
        (3, 1),
        "the explicit new nonce must create exactly one fresh paused attempt"
    );
    let action = resume_historical_fork(&mut scenario, &fresh_generation_id, &fresh_second_request);
    assert_eq!(action.generation_id, fresh_generation_id);
    assert_completed_historical_fork(&scenario, &fresh_generation_id, &action);
}
