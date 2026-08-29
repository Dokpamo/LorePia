//! Typed, bounded IPC adapter for prompt orchestration and creator content.
//!
//! These domain documents are intentionally safe to expose to the local
//! frontend: they contain declarative configuration and content, but no raw
//! credentials, host paths, generic filesystem operations, or executable
//! script payloads. Every mutation retains Core's optimistic-concurrency
//! revision instead of inventing a shell-owned write model.

use std::{collections::BTreeMap, future::Future, pin::Pin};

use chrono::{DateTime, Utc};
use lorepia_core::{
    ActivationRule, ApiFamily, AssetId, BlockResolutionStatus, BlockSource, CacheBoundary,
    CacheDirectiveStatus, CacheMode, CacheRoleFilter, CacheTtl, CapabilityKey, ClaimedMemoryJob,
    ConditionExpr, ConnectionBoundCredential, ContentCapability, ContentModule, ContentModuleId,
    ContentShareGate, ControlKind, ControlSpec, ConversationBranchId, ConversationId, CoreError,
    CoreErrorCode, CoreResult, CreatorControlValue as CoreCreatorControlValue, ExpertPromptPreview,
    GenerationPresetId, GenerationReasoningEffort, HistorySelector, InstructionAuthority,
    InteractionAction, InteractionEvent, InteractionRule, InteractionRuleId, InteractionRuleSet,
    InteractionRuleSetId, InterruptedMemoryJob, KnowledgeActivationReason, KnowledgeBook,
    KnowledgeBookId, KnowledgeEntry, KnowledgeEntryId, KnowledgePlacement, KnowledgeSelection,
    KnowledgeSimulationRequest, KnowledgeTokenEstimate, MAX_BLOCK_TEXT_CHARS, MAX_NAME_CHARS,
    MemoryJobId, MemoryJobKind, MemoryJobStatus, MemoryKind, MemoryProfile, MemoryProfileId,
    MemoryQueryEmbeddingRetryCandidate, MemoryQueryEmbeddingStatus, MemoryRecord,
    MemoryRecordExclusionScope, MemoryRecordId, MemoryRecordUserPatch, MergePolicy, MessageId,
    ModuleBinding, ModuleScope, ObjectRevision, OverflowPolicy, PackageMetadata, PlacementZone,
    PresetMetadata, PromptBlock, PromptBlockId, PromptBlockKind, PromptMemorySelectionLane,
    PromptMemorySelectionReason, PromptPlanMessagePreview, PromptPlanPreview, PromptPlanRequest,
    PromptPreset, PromptPresetBinding, PromptPresetId,
    PromptPresetRevisionDiff as CorePromptPresetRevisionDiff,
    PromptPresetRollbackApplyRequest as CorePromptPresetRollbackApplyRequest,
    PromptPresetRollbackReview as CorePromptPresetRollbackReview, PromptProviderMessagePreview,
    PromptResolutionTrace, PromptResponseLength, Provenance, ProviderCacheBoundaryCompilation,
    ProviderConnectionId, ProviderCredentialAccessAuthority, ProviderMessageRole,
    ProviderPromptPlacement, ProviderWireRole, Revisioned, RoleHint, RoomOrchestrationConfig,
    SafeRegex, SafeTemplate, SelectedKnowledgeEntry, SemanticKnowledgeScore, SourceKind,
    SummarySchemaId, TaskCredentialBroker, TaskProfile, TaskProfileId, TemplateSlot, TokenBudget,
    TokenPolicy, TransformDiff, TransformFailure, TransformPhase, TransformPreviewRequest,
    TransformRule, TransformRuleId, TransformRuleReport, TransformSet, TransformSetId, VariableMap,
    VariableValue,
};
use serde::{Deserialize, Serialize};

use crate::{
    GenerationCredential, GenerationTargetDto, ShellApi, ShellError, ShellResult,
    StartedGeneration, TaskCredentialRead, TaskCredentialReader,
    api::{validate_generation_operation_context, validate_identifier},
    sensitive::GenerationCredentialKind,
};

const MAX_DOCUMENT_BYTES: usize = 2 * 1024 * 1024;
const MAX_DOCUMENT_STRING_CHARS: usize = 1_000_000;
const MAX_DOCUMENT_DEPTH: usize = 32;
const MAX_DOCUMENT_NODES: usize = 100_000;
const MAX_COLLECTION_ITEMS: usize = 4_096;
const MAX_PREVIEW_TEXT_BYTES: usize = 16 * 1024;
const MAX_MEMORY_LIST_ITEMS: usize = 250;
const MAX_CREATOR_DOCUMENTS: usize = 100;
const MAX_SELECTION_ITEMS: usize = 300;
const MAX_MODULE_REVISIONS: usize = 64;
const MAX_PROMPT_PRESET_REVISIONS: usize = 100;
const MAX_DIFF_PATHS: usize = 1_024;
const MAX_PROMPT_PREVIEW_ITEMS: usize = 512;
const MAX_PROMPT_WARNINGS: usize = 128;
const MAX_PROMPT_BLOCK_PREVIEW_BYTES: usize = 4 * 1024;
const MAX_ROOM_TEMPLATE_SLOTS: usize = 128;

pub type PromptPresetDto = PromptPreset;
pub type TaskProfileDto = TaskProfile;
pub type MemoryProfileDto = CreatorMemoryProfileDocumentDto;
pub type MemoryRecordDto = MemoryRecord;
pub type KnowledgeBookDto = CreatorKnowledgeBookDocumentDto;
pub type TransformSetDto = CreatorTransformSetDocumentDto;
pub type InteractionRuleSetDto = CreatorInteractionRuleSetDocumentDto;
pub type ContentModuleDto = CreatorContentModuleDocumentDto;
pub type PromptPresetBindingDto = PromptPresetBinding;
pub type ModuleBindingDto = ModuleBinding;
pub type SelectedKnowledgeEntryDto = SelectedKnowledgeEntry;
pub type KnowledgeSelectionEvidenceDto = lorepia_core::KnowledgeSelectionEvidence;
pub type ContentShareGateDto = ContentShareGate;

/// Creator-editable instruction authorities. Application authority is
/// intentionally not representable at the webview boundary.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CreatorPromptBlockAuthority {
    Creator,
    User,
    Conversation,
    ImportedContent,
}

impl From<CreatorPromptBlockAuthority> for InstructionAuthority {
    fn from(value: CreatorPromptBlockAuthority) -> Self {
        match value {
            CreatorPromptBlockAuthority::Creator => Self::Creator,
            CreatorPromptBlockAuthority::User => Self::User,
            CreatorPromptBlockAuthority::Conversation => Self::Conversation,
            CreatorPromptBlockAuthority::ImportedContent => Self::ImportedContent,
        }
    }
}

impl TryFrom<InstructionAuthority> for CreatorPromptBlockAuthority {
    type Error = ShellError;

    fn try_from(value: InstructionAuthority) -> Result<Self, Self::Error> {
        match value {
            InstructionAuthority::Creator => Ok(Self::Creator),
            InstructionAuthority::User => Ok(Self::User),
            InstructionAuthority::Conversation => Ok(Self::Conversation),
            InstructionAuthority::ImportedContent => Ok(Self::ImportedContent),
            InstructionAuthority::Application => Err(shell_invalid(
                "application authority cannot cross the editable prompt boundary",
            )),
        }
    }
}

/// Creator-editable placement zones. The canonical application-policy zone is
/// Core-owned and intentionally absent.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CreatorPromptBlockPlacementZone {
    PresetInstruction,
    CharacterContext,
    RetrievedContext,
    OlderHistory,
    RecentEnhancement,
    RecentHistory,
    PostHistory,
    LatestUser,
    AssistantPrefill,
}

impl From<CreatorPromptBlockPlacementZone> for PlacementZone {
    fn from(value: CreatorPromptBlockPlacementZone) -> Self {
        match value {
            CreatorPromptBlockPlacementZone::PresetInstruction => Self::PresetInstruction,
            CreatorPromptBlockPlacementZone::CharacterContext => Self::CharacterContext,
            CreatorPromptBlockPlacementZone::RetrievedContext => Self::RetrievedContext,
            CreatorPromptBlockPlacementZone::OlderHistory => Self::OlderHistory,
            CreatorPromptBlockPlacementZone::RecentEnhancement => Self::RecentEnhancement,
            CreatorPromptBlockPlacementZone::RecentHistory => Self::RecentHistory,
            CreatorPromptBlockPlacementZone::PostHistory => Self::PostHistory,
            CreatorPromptBlockPlacementZone::LatestUser => Self::LatestUser,
            CreatorPromptBlockPlacementZone::AssistantPrefill => Self::AssistantPrefill,
        }
    }
}

impl TryFrom<PlacementZone> for CreatorPromptBlockPlacementZone {
    type Error = ShellError;

    fn try_from(value: PlacementZone) -> Result<Self, Self::Error> {
        match value {
            PlacementZone::PresetInstruction => Ok(Self::PresetInstruction),
            PlacementZone::CharacterContext => Ok(Self::CharacterContext),
            PlacementZone::RetrievedContext => Ok(Self::RetrievedContext),
            PlacementZone::OlderHistory => Ok(Self::OlderHistory),
            PlacementZone::RecentEnhancement => Ok(Self::RecentEnhancement),
            PlacementZone::RecentHistory => Ok(Self::RecentHistory),
            PlacementZone::PostHistory => Ok(Self::PostHistory),
            PlacementZone::LatestUser => Ok(Self::LatestUser),
            PlacementZone::AssistantPrefill => Ok(Self::AssistantPrefill),
            PlacementZone::ApplicationPolicy => Err(shell_invalid(
                "application policy cannot cross the editable prompt boundary",
            )),
        }
    }
}

/// Creator-editable provenance kinds. Application-built-in provenance is
/// intentionally not representable.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CreatorOrchestrationSourceKind {
    UserCreated,
    ImportedStandard,
    ImportedPackage,
    Generated,
}

impl From<CreatorOrchestrationSourceKind> for SourceKind {
    fn from(value: CreatorOrchestrationSourceKind) -> Self {
        match value {
            CreatorOrchestrationSourceKind::UserCreated => Self::UserCreated,
            CreatorOrchestrationSourceKind::ImportedStandard => Self::ImportedStandard,
            CreatorOrchestrationSourceKind::ImportedPackage => Self::ImportedPackage,
            CreatorOrchestrationSourceKind::Generated => Self::Generated,
        }
    }
}

impl TryFrom<SourceKind> for CreatorOrchestrationSourceKind {
    type Error = ShellError;

    fn try_from(value: SourceKind) -> Result<Self, Self::Error> {
        match value {
            SourceKind::UserCreated => Ok(Self::UserCreated),
            SourceKind::ImportedStandard => Ok(Self::ImportedStandard),
            SourceKind::ImportedPackage => Ok(Self::ImportedPackage),
            SourceKind::Generated => Ok(Self::Generated),
            SourceKind::ApplicationBuiltIn => Err(shell_invalid(
                "application-built-in provenance cannot cross the editable prompt boundary",
            )),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CreatorOrchestrationProvenanceDto {
    pub source_kind: CreatorOrchestrationSourceKind,
    pub source_id: Option<String>,
    pub source_hash: Option<String>,
    pub author: Option<String>,
    pub license: Option<String>,
    pub imported_at: Option<DateTime<Utc>>,
}

impl From<CreatorOrchestrationProvenanceDto> for Provenance {
    fn from(value: CreatorOrchestrationProvenanceDto) -> Self {
        Self {
            source_kind: value.source_kind.into(),
            source_id: value.source_id,
            source_hash: value.source_hash,
            author: value.author,
            license: value.license,
            imported_at: value.imported_at,
        }
    }
}

impl TryFrom<Provenance> for CreatorOrchestrationProvenanceDto {
    type Error = ShellError;

    fn try_from(value: Provenance) -> Result<Self, Self::Error> {
        Ok(Self {
            source_kind: value.source_kind.try_into()?,
            source_id: value.source_id,
            source_hash: value.source_hash,
            author: value.author,
            license: value.license,
            imported_at: value.imported_at,
        })
    }
}

/// Exact editable block document. Every nested template, condition, history,
/// token, merge, overflow, and source field remains typed, while application
/// authority and placement are unrepresentable.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CreatorPromptBlockDocumentDto {
    pub id: String,
    pub name: String,
    pub kind: PromptBlockKind,
    pub enabled: bool,
    pub role_hint: RoleHint,
    pub authority: CreatorPromptBlockAuthority,
    pub template: Option<SafeTemplate>,
    pub condition: Option<ConditionExpr>,
    pub source: BlockSource,
    pub placement_zone: CreatorPromptBlockPlacementZone,
    pub history_selector: Option<HistorySelector>,
    pub token_policy: TokenPolicy,
    pub overflow_policy: OverflowPolicy,
    pub merge_policy: MergePolicy,
    pub provenance: CreatorOrchestrationProvenanceDto,
}

impl From<CreatorPromptBlockDocumentDto> for PromptBlock {
    fn from(value: CreatorPromptBlockDocumentDto) -> Self {
        Self {
            id: PromptBlockId::from(value.id),
            name: value.name,
            kind: value.kind,
            enabled: value.enabled,
            role_hint: value.role_hint,
            authority: value.authority.into(),
            template: value.template,
            condition: value.condition,
            source: value.source,
            placement_zone: value.placement_zone.into(),
            history_selector: value.history_selector,
            token_policy: value.token_policy,
            overflow_policy: value.overflow_policy,
            merge_policy: value.merge_policy,
            provenance: value.provenance.into(),
        }
    }
}

impl TryFrom<PromptBlock> for CreatorPromptBlockDocumentDto {
    type Error = ShellError;

    fn try_from(value: PromptBlock) -> Result<Self, Self::Error> {
        Ok(Self {
            id: value.id.0,
            name: value.name,
            kind: value.kind,
            enabled: value.enabled,
            role_hint: value.role_hint,
            authority: value.authority.try_into()?,
            template: value.template,
            condition: value.condition,
            source: value.source,
            placement_zone: value.placement_zone.try_into()?,
            history_selector: value.history_selector,
            token_policy: value.token_policy,
            overflow_policy: value.overflow_policy,
            merge_policy: value.merge_policy,
            provenance: value.provenance.try_into()?,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CreatorPromptPresetMetadataDto {
    pub description: String,
    pub tags: Vec<String>,
    pub provenance: CreatorOrchestrationProvenanceDto,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub local_override_of: Option<String>,
}

impl From<CreatorPromptPresetMetadataDto> for PresetMetadata {
    fn from(value: CreatorPromptPresetMetadataDto) -> Self {
        Self {
            description: value.description,
            tags: value.tags,
            provenance: value.provenance.into(),
            created_at: value.created_at,
            updated_at: value.updated_at,
            local_override_of: value.local_override_of.map(PromptPresetId::from),
        }
    }
}

impl TryFrom<PresetMetadata> for CreatorPromptPresetMetadataDto {
    type Error = ShellError;

    fn try_from(value: PresetMetadata) -> Result<Self, Self::Error> {
        Ok(Self {
            description: value.description,
            tags: value.tags,
            provenance: value.provenance.try_into()?,
            created_at: value.created_at,
            updated_at: value.updated_at,
            local_override_of: value.local_override_of.map(|id| id.0),
        })
    }
}

/// Complete creator-owned `PromptPreset` document. Core injects its canonical
/// `ApplicationPolicy` block after validating this document on every upsert.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CreatorPromptPresetDocumentDto {
    pub id: String,
    pub name: String,
    pub schema_version: u32,
    pub blocks: Vec<CreatorPromptBlockDocumentDto>,
    pub controls: Vec<ControlSpec>,
    pub default_values: VariableMap,
    pub default_generation_preset_id: Option<String>,
    pub memory_profile_id: Option<String>,
    pub knowledge_book_ids: Vec<String>,
    pub transform_set_ids: Vec<String>,
    pub module_ids: Vec<String>,
    pub cache_boundaries: Vec<CacheBoundary>,
    pub metadata: CreatorPromptPresetMetadataDto,
}

impl From<CreatorPromptPresetDocumentDto> for PromptPreset {
    fn from(value: CreatorPromptPresetDocumentDto) -> Self {
        Self {
            id: PromptPresetId::from(value.id),
            name: value.name,
            schema_version: value.schema_version,
            blocks: value.blocks.into_iter().map(Into::into).collect(),
            controls: value.controls,
            default_values: value.default_values,
            default_generation_preset_id: value
                .default_generation_preset_id
                .map(GenerationPresetId::from),
            memory_profile_id: value.memory_profile_id.map(MemoryProfileId::from),
            knowledge_book_ids: value
                .knowledge_book_ids
                .into_iter()
                .map(KnowledgeBookId::from)
                .collect(),
            transform_set_ids: value
                .transform_set_ids
                .into_iter()
                .map(TransformSetId::from)
                .collect(),
            module_ids: value
                .module_ids
                .into_iter()
                .map(ContentModuleId::from)
                .collect(),
            cache_boundaries: value.cache_boundaries,
            metadata: value.metadata.into(),
        }
    }
}

/// Creator-owned memory policy. Schema and provenance are injected by Rust.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CreatorMemoryProfileDocumentDto {
    pub id: String,
    pub name: String,
    pub summary_task: String,
    pub embedding_task: Option<String>,
    pub turns_per_summary: u32,
    pub recent_raw_budget: TokenBudget,
    pub episodic_budget: TokenBudget,
    pub semantic_budget: TokenBudget,
    pub retrieval_count: u32,
    pub recency_weight: f32,
    pub similarity_weight: f32,
    pub importance_weight: f32,
    pub preserve_invalidated_records: bool,
    pub summary_schema: String,
}

impl From<CreatorMemoryProfileDocumentDto> for MemoryProfile {
    fn from(value: CreatorMemoryProfileDocumentDto) -> Self {
        Self {
            id: MemoryProfileId::from(value.id),
            name: value.name,
            schema_version: 1,
            summary_task: TaskProfileId::from(value.summary_task),
            embedding_task: value.embedding_task.map(TaskProfileId::from),
            turns_per_summary: value.turns_per_summary,
            recent_raw_budget: value.recent_raw_budget,
            episodic_budget: value.episodic_budget,
            semantic_budget: value.semantic_budget,
            retrieval_count: value.retrieval_count,
            recency_weight: value.recency_weight,
            similarity_weight: value.similarity_weight,
            importance_weight: value.importance_weight,
            preserve_invalidated_records: value.preserve_invalidated_records,
            summary_schema: SummarySchemaId::from(value.summary_schema),
            provenance: user_created_provenance(),
        }
    }
}

impl TryFrom<MemoryProfile> for CreatorMemoryProfileDocumentDto {
    type Error = ShellError;

    fn try_from(value: MemoryProfile) -> Result<Self, Self::Error> {
        require_user_created_provenance(&value.provenance, "memory profile")?;
        Ok(Self {
            id: value.id.0,
            name: value.name,
            summary_task: value.summary_task.0,
            embedding_task: value.embedding_task.map(|id| id.0),
            turns_per_summary: value.turns_per_summary,
            recent_raw_budget: value.recent_raw_budget,
            episodic_budget: value.episodic_budget,
            semantic_budget: value.semantic_budget,
            retrieval_count: value.retrieval_count,
            recency_weight: value.recency_weight,
            similarity_weight: value.similarity_weight,
            importance_weight: value.importance_weight,
            preserve_invalidated_records: value.preserve_invalidated_records,
            summary_schema: value.summary_schema.0,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CreatorKnowledgeEntryDocumentDto {
    pub id: String,
    pub name: String,
    pub content: String,
    pub enabled: bool,
    pub activation: ActivationRule,
    pub priority: i32,
    pub importance: u8,
    pub placement: KnowledgePlacement,
    pub token_policy: TokenPolicy,
    pub parent_id: Option<String>,
    pub activation_probability_basis_points: u16,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CreatorKnowledgeBookDocumentDto {
    pub id: String,
    pub name: String,
    pub entries: Vec<CreatorKnowledgeEntryDocumentDto>,
    pub scan_depth: u32,
    pub token_budget: TokenBudget,
    pub recursive: bool,
    pub max_recursion_depth: u32,
}

impl From<CreatorKnowledgeBookDocumentDto> for KnowledgeBook {
    fn from(value: CreatorKnowledgeBookDocumentDto) -> Self {
        let book_id = KnowledgeBookId::from(value.id);
        Self {
            id: book_id.clone(),
            name: value.name,
            schema_version: 1,
            entries: value
                .entries
                .into_iter()
                .map(|entry| KnowledgeEntry {
                    id: KnowledgeEntryId::from(entry.id),
                    book_id: book_id.clone(),
                    name: entry.name,
                    content: entry.content,
                    enabled: entry.enabled,
                    activation: entry.activation,
                    priority: entry.priority,
                    importance: entry.importance,
                    placement: entry.placement,
                    token_policy: entry.token_policy,
                    parent_id: entry.parent_id.map(KnowledgeEntryId::from),
                    activation_probability_basis_points: entry.activation_probability_basis_points,
                    provenance: user_created_provenance(),
                })
                .collect(),
            scan_depth: value.scan_depth,
            token_budget: value.token_budget,
            recursive: value.recursive,
            max_recursion_depth: value.max_recursion_depth,
            provenance: user_created_provenance(),
        }
    }
}

impl TryFrom<KnowledgeBook> for CreatorKnowledgeBookDocumentDto {
    type Error = ShellError;

    fn try_from(value: KnowledgeBook) -> Result<Self, Self::Error> {
        require_user_created_provenance(&value.provenance, "knowledge book")?;
        let entries = value
            .entries
            .into_iter()
            .map(|entry| {
                require_user_created_provenance(&entry.provenance, "knowledge entry")?;
                Ok(CreatorKnowledgeEntryDocumentDto {
                    id: entry.id.0,
                    name: entry.name,
                    content: entry.content,
                    enabled: entry.enabled,
                    activation: entry.activation,
                    priority: entry.priority,
                    importance: entry.importance,
                    placement: entry.placement,
                    token_policy: entry.token_policy,
                    parent_id: entry.parent_id.map(|id| id.0),
                    activation_probability_basis_points: entry.activation_probability_basis_points,
                })
            })
            .collect::<ShellResult<Vec<_>>>()?;
        Ok(Self {
            id: value.id.0,
            name: value.name,
            entries,
            scan_depth: value.scan_depth,
            token_budget: value.token_budget,
            recursive: value.recursive,
            max_recursion_depth: value.max_recursion_depth,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CreatorTransformRuleDocumentDto {
    pub id: String,
    pub name: String,
    pub enabled: bool,
    pub phase: TransformPhase,
    pub order: i32,
    pub pattern: SafeRegex,
    pub replacement: String,
    pub condition: Option<ConditionExpr>,
    pub max_replacements: u32,
    pub input_limit: u32,
    pub output_limit: u32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CreatorTransformSetDocumentDto {
    pub id: String,
    pub name: String,
    pub enabled: bool,
    pub rules: Vec<CreatorTransformRuleDocumentDto>,
    pub max_rules_per_phase: u32,
    pub max_output_chars: u32,
}

impl From<CreatorTransformSetDocumentDto> for TransformSet {
    fn from(value: CreatorTransformSetDocumentDto) -> Self {
        Self {
            id: TransformSetId::from(value.id),
            name: value.name,
            schema_version: 1,
            enabled: value.enabled,
            imported_author_enabled: false,
            rules: value
                .rules
                .into_iter()
                .map(|rule| TransformRule {
                    id: TransformRuleId::from(rule.id),
                    name: rule.name,
                    enabled: rule.enabled,
                    imported_enabled: false,
                    imported_author_enabled: false,
                    phase: rule.phase,
                    order: rule.order,
                    pattern: rule.pattern,
                    replacement: rule.replacement,
                    condition: rule.condition,
                    max_replacements: rule.max_replacements,
                    input_limit: rule.input_limit,
                    output_limit: rule.output_limit,
                    provenance: user_created_provenance(),
                })
                .collect(),
            max_rules_per_phase: value.max_rules_per_phase,
            max_output_chars: value.max_output_chars,
            provenance: user_created_provenance(),
        }
    }
}

impl TryFrom<TransformSet> for CreatorTransformSetDocumentDto {
    type Error = ShellError;

    fn try_from(value: TransformSet) -> Result<Self, Self::Error> {
        require_user_created_provenance(&value.provenance, "transform set")?;
        let rules = value
            .rules
            .into_iter()
            .map(|rule| {
                require_user_created_provenance(&rule.provenance, "transform rule")?;
                Ok(CreatorTransformRuleDocumentDto {
                    id: rule.id.0,
                    name: rule.name,
                    enabled: rule.enabled,
                    phase: rule.phase,
                    order: rule.order,
                    pattern: rule.pattern,
                    replacement: rule.replacement,
                    condition: rule.condition,
                    max_replacements: rule.max_replacements,
                    input_limit: rule.input_limit,
                    output_limit: rule.output_limit,
                })
            })
            .collect::<ShellResult<Vec<_>>>()?;
        Ok(Self {
            id: value.id.0,
            name: value.name,
            enabled: value.enabled,
            rules,
            max_rules_per_phase: value.max_rules_per_phase,
            max_output_chars: value.max_output_chars,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CreatorInteractionRuleDocumentDto {
    pub id: String,
    pub name: String,
    pub enabled: bool,
    pub event: InteractionEvent,
    pub condition: Option<ConditionExpr>,
    pub actions: Vec<InteractionAction>,
    pub priority: i32,
    pub stop_after_match: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CreatorInteractionRuleSetDocumentDto {
    pub id: String,
    pub name: String,
    pub rules: Vec<CreatorInteractionRuleDocumentDto>,
    pub max_actions_per_event: u32,
}

impl From<CreatorInteractionRuleSetDocumentDto> for InteractionRuleSet {
    fn from(value: CreatorInteractionRuleSetDocumentDto) -> Self {
        Self {
            id: InteractionRuleSetId::from(value.id),
            name: value.name,
            schema_version: 1,
            rules: value
                .rules
                .into_iter()
                .map(|rule| InteractionRule {
                    id: InteractionRuleId::from(rule.id),
                    name: rule.name,
                    enabled: rule.enabled,
                    imported_author_enabled: false,
                    event: rule.event,
                    condition: rule.condition,
                    actions: rule.actions,
                    priority: rule.priority,
                    stop_after_match: rule.stop_after_match,
                    provenance: user_created_provenance(),
                })
                .collect(),
            max_actions_per_event: value.max_actions_per_event,
            provenance: user_created_provenance(),
        }
    }
}

impl TryFrom<InteractionRuleSet> for CreatorInteractionRuleSetDocumentDto {
    type Error = ShellError;

    fn try_from(value: InteractionRuleSet) -> Result<Self, Self::Error> {
        require_user_created_provenance(&value.provenance, "interaction rule set")?;
        let rules = value
            .rules
            .into_iter()
            .map(|rule| {
                require_user_created_provenance(&rule.provenance, "interaction rule")?;
                Ok(CreatorInteractionRuleDocumentDto {
                    id: rule.id.0,
                    name: rule.name,
                    enabled: rule.enabled,
                    event: rule.event,
                    condition: rule.condition,
                    actions: rule.actions,
                    priority: rule.priority,
                    stop_after_match: rule.stop_after_match,
                })
            })
            .collect::<ShellResult<Vec<_>>>()?;
        Ok(Self {
            id: value.id.0,
            name: value.name,
            rules,
            max_actions_per_event: value.max_actions_per_event,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CreatorModulePromptFragmentDocumentDto {
    pub id: String,
    pub name: String,
    pub kind: PromptBlockKind,
    pub enabled: bool,
    pub role_hint: RoleHint,
    pub authority: CreatorPromptBlockAuthority,
    pub template: Option<SafeTemplate>,
    pub condition: Option<ConditionExpr>,
    pub source: BlockSource,
    pub placement_zone: CreatorPromptBlockPlacementZone,
    pub history_selector: Option<HistorySelector>,
    pub token_policy: TokenPolicy,
    pub overflow_policy: OverflowPolicy,
    pub merge_policy: MergePolicy,
}

impl From<CreatorModulePromptFragmentDocumentDto> for PromptBlock {
    fn from(value: CreatorModulePromptFragmentDocumentDto) -> Self {
        Self {
            id: PromptBlockId::from(value.id),
            name: value.name,
            kind: value.kind,
            enabled: value.enabled,
            role_hint: value.role_hint,
            authority: value.authority.into(),
            template: value.template,
            condition: value.condition,
            source: value.source,
            placement_zone: value.placement_zone.into(),
            history_selector: value.history_selector,
            token_policy: value.token_policy,
            overflow_policy: value.overflow_policy,
            merge_policy: value.merge_policy,
            provenance: user_created_provenance(),
        }
    }
}

impl TryFrom<PromptBlock> for CreatorModulePromptFragmentDocumentDto {
    type Error = ShellError;

    fn try_from(value: PromptBlock) -> Result<Self, Self::Error> {
        require_user_created_provenance(&value.provenance, "module prompt fragment")?;
        Ok(Self {
            id: value.id.0,
            name: value.name,
            kind: value.kind,
            enabled: value.enabled,
            role_hint: value.role_hint,
            authority: value.authority.try_into()?,
            template: value.template,
            condition: value.condition,
            source: value.source,
            placement_zone: value.placement_zone.try_into()?,
            history_selector: value.history_selector,
            token_policy: value.token_policy,
            overflow_policy: value.overflow_policy,
            merge_policy: value.merge_policy,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CreatorContentModuleMetadataDto {
    pub author: Option<String>,
    pub license: String,
    pub redistribution_allowed: bool,
    pub homepage: Option<String>,
    pub description: String,
    pub tags: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CreatorContentModuleDocumentDto {
    pub id: String,
    pub name: String,
    pub version: String,
    pub prompt_fragments: Vec<CreatorModulePromptFragmentDocumentDto>,
    pub knowledge_book_ids: Vec<String>,
    pub control_specs: Vec<ControlSpec>,
    pub transform_set_ids: Vec<String>,
    pub interaction_rule_set_ids: Vec<String>,
    pub asset_ids: Vec<String>,
    pub required_capabilities: Vec<ContentCapability>,
    pub metadata: CreatorContentModuleMetadataDto,
}

impl From<CreatorContentModuleDocumentDto> for ContentModule {
    fn from(value: CreatorContentModuleDocumentDto) -> Self {
        Self {
            id: ContentModuleId::from(value.id),
            name: value.name,
            version: value.version,
            schema_version: 1,
            prompt_fragments: value.prompt_fragments.into_iter().map(Into::into).collect(),
            knowledge_book_ids: value
                .knowledge_book_ids
                .into_iter()
                .map(KnowledgeBookId::from)
                .collect(),
            control_specs: value.control_specs,
            transform_set_ids: value
                .transform_set_ids
                .into_iter()
                .map(TransformSetId::from)
                .collect(),
            interaction_rule_set_ids: value
                .interaction_rule_set_ids
                .into_iter()
                .map(InteractionRuleSetId::from)
                .collect(),
            asset_ids: value.asset_ids.into_iter().map(AssetId::from).collect(),
            imported_components_enabled: false,
            required_capabilities: value.required_capabilities,
            metadata: PackageMetadata {
                author: value.metadata.author,
                license: value.metadata.license,
                redistribution_allowed: value.metadata.redistribution_allowed,
                homepage: value.metadata.homepage,
                description: value.metadata.description,
                tags: value.metadata.tags,
                provenance: user_created_provenance(),
            },
        }
    }
}

impl TryFrom<ContentModule> for CreatorContentModuleDocumentDto {
    type Error = ShellError;

    fn try_from(value: ContentModule) -> Result<Self, Self::Error> {
        require_user_created_provenance(&value.metadata.provenance, "content module")?;
        Ok(Self {
            id: value.id.0,
            name: value.name,
            version: value.version,
            prompt_fragments: value
                .prompt_fragments
                .into_iter()
                .map(TryInto::try_into)
                .collect::<ShellResult<Vec<_>>>()?,
            knowledge_book_ids: value
                .knowledge_book_ids
                .into_iter()
                .map(|id| id.0)
                .collect(),
            control_specs: value.control_specs,
            transform_set_ids: value.transform_set_ids.into_iter().map(|id| id.0).collect(),
            interaction_rule_set_ids: value
                .interaction_rule_set_ids
                .into_iter()
                .map(|id| id.0)
                .collect(),
            asset_ids: value.asset_ids.into_iter().map(|id| id.0).collect(),
            required_capabilities: value.required_capabilities,
            metadata: CreatorContentModuleMetadataDto {
                author: value.metadata.author,
                license: value.metadata.license,
                redistribution_allowed: value.metadata.redistribution_allowed,
                homepage: value.metadata.homepage,
                description: value.metadata.description,
                tags: value.metadata.tags,
            },
        })
    }
}

/// Exact inputs needed to resolve the same prompt plan used for generation.
///
/// There is deliberately no implicit generation target, branch head, user
/// text, or preset fallback in the shell. The frontend must send the state it
/// reviewed, and Core validates it against current durable state.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ResolvePromptPreviewInput {
    pub conversation_id: String,
    pub branch_id: String,
    pub expected_head: Option<String>,
    pub user_text: String,
    pub generation_target: GenerationTargetDto,
    pub prompt_preset_id: Option<String>,
    pub variable_overrides: VariableMap,
    pub expected_plan_hash: Option<String>,
    /// Caller-owned identity for a new preview operation.
    #[serde(default)]
    pub operation_nonce: Option<String>,
    /// Exact durable preview attempt to resume; mutually exclusive with nonce.
    #[serde(default)]
    pub generation_attempt_id: Option<String>,
}

/// Exact retained expert review required to dispatch a previewed prompt.
/// Both identities are mandatory and Core revalidates them before append.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReviewedPromptSendInput {
    pub conversation_id: String,
    pub branch_id: String,
    pub expected_head: Option<String>,
    pub user_text: String,
    pub generation_target: GenerationTargetDto,
    pub prompt_preset_id: Option<String>,
    pub variable_overrides: VariableMap,
    pub expected_plan_hash: String,
    pub generation_attempt_id: String,
}

/// Re-resolves an immutable plan identity and returns its redacted trace.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExplainPromptPlanInput {
    pub conversation_id: String,
    pub branch_id: String,
    pub expected_head: Option<String>,
    pub user_text: String,
    pub generation_target: GenerationTargetDto,
    pub prompt_preset_id: Option<String>,
    pub variable_overrides: VariableMap,
    pub plan_hash: String,
    /// Caller-owned identity for a new explain operation.
    #[serde(default)]
    pub operation_nonce: Option<String>,
    /// Exact durable preview attempt to explain; mutually exclusive with nonce.
    #[serde(default)]
    pub generation_attempt_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PromptPlanMessagePreviewDto {
    pub sequence: u32,
    pub block_id: String,
    pub block_kind: PromptBlockKind,
    pub requested_role: RoleHint,
    pub effective_role: ProviderMessageRole,
    pub estimated_tokens: u32,
    pub source_message_ids: Vec<String>,
    pub truncated: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PromptProviderMessagePreviewDto {
    pub sequence: u32,
    pub block_id: String,
    pub effective_role: ProviderMessageRole,
    pub wire_role: ProviderWireRole,
    pub placement: ProviderPromptPlacement,
    pub estimated_tokens: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PromptCacheDirectivePreviewDto {
    pub boundary_id: String,
    pub after_block_id: String,
    pub after_message_sequence: Option<u32>,
    pub role_filter: CacheRoleFilter,
    pub ttl: CacheTtl,
    pub mode: CacheMode,
    pub status: CacheDirectiveStatus,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PromptBlockSourceTraceDto {
    pub authority: InstructionAuthority,
    pub source_kind: SourceKind,
    pub source_id: Option<String>,
    pub source_revision: Option<String>,
    pub source_hash: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum PromptKnowledgeSelectionReasonDto {
    Always,
    Manual,
    /// A keyword matched, but the matched prompt text is Rust-internal.
    Keyword,
    /// A regular expression matched, but its pattern is not an IPC field.
    Regex,
    Semantic {
        score_millionths: u32,
    },
    Condition,
    Recursive {
        parent_id: String,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PromptEvidenceExclusionCodeDto {
    EntryDisabled,
    ActivationProbabilityGate,
    ActivationRuleDidNotMatch,
    PerEntryTokenLimit,
    KnowledgeTokenBudgetOverflow,
    KnowledgeRemainingTokenBudget,
    PromptTokenBudget,
    MemoryRetrievalCountLimit,
    MemoryRemainingTokenBudget,
    OtherConversation,
    MemoryInvalidated,
    ExcludedFromConversation,
    ExcludedFromCharacter,
    NotOnActiveBranchLineage,
    ReversedSourceRange,
    Other,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PromptKnowledgeSelectionEvidenceDto {
    pub entry_id: String,
    pub selected: bool,
    pub reasons: Vec<PromptKnowledgeSelectionReasonDto>,
    pub estimated_tokens: u32,
    pub exclusion_code: Option<PromptEvidenceExclusionCodeDto>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum PromptMemorySelectionReasonDto {
    Pinned,
    CurrentBranch,
    SharedAncestor { source_branch_id: String },
    Recency { score_millionths: u32 },
    Similarity { score_millionths: u32 },
    Importance { score_millionths: u32 },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PromptMemorySelectionEvidenceDto {
    pub record_id: String,
    pub selected: bool,
    pub lane: Option<PromptMemorySelectionLane>,
    pub rank_millionths: Option<u64>,
    pub estimated_tokens: u32,
    pub reasons: Vec<PromptMemorySelectionReasonDto>,
    pub exclusion_code: Option<PromptEvidenceExclusionCodeDto>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PromptBlockResolutionTraceDto {
    pub block_id: String,
    pub block_kind: PromptBlockKind,
    pub source: PromptBlockSourceTraceDto,
    pub status: BlockResolutionStatus,
    pub original_estimated_tokens: u32,
    pub final_estimated_tokens: u32,
    pub produced_message_count: u32,
    pub knowledge_evidence: Vec<PromptKnowledgeSelectionEvidenceDto>,
    pub memory_record_ids: Vec<String>,
    pub memory_evidence: Vec<PromptMemorySelectionEvidenceDto>,
    pub truncated: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PromptRoleMappingTraceDto {
    pub block_id: String,
    pub requested_role: RoleHint,
    pub effective_role: ProviderMessageRole,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PromptOverflowTraceDto {
    pub block_id: String,
    pub policy: OverflowPolicy,
    pub tokens_before: u32,
    pub tokens_after: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PromptWarningCodeDto {
    CacheBoundaryIgnoredUnsupported,
    CacheBoundaryIgnoredLimit,
    ReasoningEffortOmitted,
    CreativityIgnored,
    KnowledgeDisabled,
    MemoryDisabled,
    CacheReuseSuboptimal,
    MissingModuleDependencies,
    ResolvedPromptTransformFailed,
    ResolvedPromptTransformIgnored,
    ProviderCacheBoundaryIgnored,
    Other,
}

/// Redacted, bounded preview of the exact Core-owned generation plan.
///
/// Prompt bodies, resolved variables, memory contents, credentials, request
/// headers, and provider payloads are intentionally not represented.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PromptPlanPreviewDto {
    pub plan_id: String,
    pub plan_hash: String,
    pub prompt_preset_id: String,
    pub prompt_preset_revision: u64,
    pub generation_target: GenerationTargetDto,
    pub estimated_input_tokens: u32,
    pub available_input_tokens: u32,
    pub token_estimator_id: String,
    pub token_estimate_exact: bool,
    pub messages: Vec<PromptPlanMessagePreviewDto>,
    pub provider_family: ApiFamily,
    pub provider_messages: Vec<PromptProviderMessagePreviewDto>,
    pub provider_cache_boundaries: Vec<ProviderCacheBoundaryCompilation>,
    pub cache_directives: Vec<PromptCacheDirectivePreviewDto>,
    pub blocks: Vec<PromptBlockResolutionTraceDto>,
    pub role_mappings: Vec<PromptRoleMappingTraceDto>,
    pub overflow: Vec<PromptOverflowTraceDto>,
    pub warnings: Vec<PromptWarningCodeDto>,
    pub truncated: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PromptAppliedParameterPreviewDto {
    pub field: String,
    pub value_kind: PromptAppliedParameterValueKindDto,
    /// Number of members for arrays and objects; scalar values remain opaque.
    pub item_count: Option<u32>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PromptAppliedParameterValueKindDto {
    Null,
    Boolean,
    Number,
    String,
    Array,
    Object,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PromptDiffEntryDto {
    pub sequence: u32,
    pub block_id: String,
    pub requested_role: RoleHint,
    pub effective_role: ProviderMessageRole,
    pub wire_role: ProviderWireRole,
    pub placement: ProviderPromptPlacement,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExpertPromptPreviewDto {
    pub generation_attempt_id: String,
    #[serde(flatten)]
    pub plan: PromptPlanPreviewDto,
    pub applied_parameters: Vec<PromptAppliedParameterPreviewDto>,
    pub prompt_diff: Vec<PromptDiffEntryDto>,
}

/// Bounded explanation for a previously reviewed immutable plan hash.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PromptResolutionTraceDto {
    pub estimator_id: String,
    pub session_seed: Option<u64>,
    pub max_context_tokens: u32,
    pub reserved_output_tokens: u32,
    pub available_input_tokens: u32,
    pub estimated_input_tokens: u32,
    pub blocks: Vec<PromptBlockResolutionTraceDto>,
    pub role_mappings: Vec<PromptRoleMappingTraceDto>,
    pub overflow: Vec<PromptOverflowTraceDto>,
    pub warnings: Vec<PromptWarningCodeDto>,
    pub truncated: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PromptBlockProjectionDto {
    pub id: String,
    pub name: String,
    pub kind: PromptBlockKind,
    pub enabled: bool,
    pub order_editable: bool,
    pub role_hint: RoleHint,
    pub placement_zone: PlacementZone,
    pub template_preview: Option<String>,
    pub condition_summary: Option<String>,
    pub source_label: String,
    pub provenance_label: String,
    pub priority: u16,
    pub minimum_tokens: Option<u32>,
    pub maximum_tokens: Option<u32>,
    pub overflow_policy: OverflowPolicy,
    pub cache_boundary_after: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PromptPresetSummaryDto {
    pub id: String,
    pub name: String,
    pub schema_version: u32,
    pub block_count: u32,
    pub default_generation_preset_id: Option<String>,
}

/// Credential-free reasoning effort presented by room quick settings.
/// `provider_default` maps to Core's absence of an explicit effort.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RoomReasoningEffortDto {
    ProviderDefault,
    Minimal,
    Low,
    Medium,
    High,
    ExtraHigh,
    Maximum,
}

impl From<Option<GenerationReasoningEffort>> for RoomReasoningEffortDto {
    fn from(value: Option<GenerationReasoningEffort>) -> Self {
        match value {
            None => Self::ProviderDefault,
            Some(GenerationReasoningEffort::Minimal) => Self::Minimal,
            Some(GenerationReasoningEffort::Low) => Self::Low,
            Some(GenerationReasoningEffort::Medium) => Self::Medium,
            Some(GenerationReasoningEffort::High) => Self::High,
            Some(GenerationReasoningEffort::ExtraHigh) => Self::ExtraHigh,
            Some(GenerationReasoningEffort::Maximum) => Self::Maximum,
        }
    }
}

impl From<RoomReasoningEffortDto> for Option<GenerationReasoningEffort> {
    fn from(value: RoomReasoningEffortDto) -> Self {
        match value {
            RoomReasoningEffortDto::ProviderDefault => None,
            RoomReasoningEffortDto::Minimal => Some(GenerationReasoningEffort::Minimal),
            RoomReasoningEffortDto::Low => Some(GenerationReasoningEffort::Low),
            RoomReasoningEffortDto::Medium => Some(GenerationReasoningEffort::Medium),
            RoomReasoningEffortDto::High => Some(GenerationReasoningEffort::High),
            RoomReasoningEffortDto::ExtraHigh => Some(GenerationReasoningEffort::ExtraHigh),
            RoomReasoningEffortDto::Maximum => Some(GenerationReasoningEffort::Maximum),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct RoomOrchestrationFieldSupportDto(pub bool);

impl RoomOrchestrationFieldSupportDto {
    pub const SUPPORTED: Self = Self(true);
    pub const UNSUPPORTED: Self = Self(false);

    pub const fn is_supported(self) -> bool {
        self.0
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RoomOrchestrationSupportedFieldsDto {
    pub prompt_preset_id: RoomOrchestrationFieldSupportDto,
    pub generation_preset_id: RoomOrchestrationFieldSupportDto,
    pub creator_values: RoomOrchestrationFieldSupportDto,
    pub variable_overrides: RoomOrchestrationFieldSupportDto,
    pub response_length: RoomOrchestrationFieldSupportDto,
    pub creativity: RoomOrchestrationFieldSupportDto,
    pub reasoning_effort: RoomOrchestrationFieldSupportDto,
    pub memory_enabled: RoomOrchestrationFieldSupportDto,
    pub knowledge_enabled: RoomOrchestrationFieldSupportDto,
    pub user_name_override: RoomOrchestrationFieldSupportDto,
    pub author_note: RoomOrchestrationFieldSupportDto,
    pub group_context: RoomOrchestrationFieldSupportDto,
    pub template_slots: RoomOrchestrationFieldSupportDto,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RoomPromptTemplateSlotDto {
    pub name: String,
    pub value: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RoomOrchestrationConfigDto {
    pub conversation_id: String,
    pub branch_id: String,
    pub prompt_preset_id: Option<String>,
    pub generation_preset_id: Option<String>,
    pub response_length: PromptResponseLength,
    pub creativity: u8,
    pub reasoning_effort: RoomReasoningEffortDto,
    pub memory_enabled: bool,
    pub knowledge_enabled: bool,
    pub creator_values: BTreeMap<String, CoreCreatorControlValue>,
    pub variable_overrides: VariableMap,
    pub user_name_override: Option<String>,
    pub author_note: Option<String>,
    pub group_context: Option<String>,
    pub template_slots: Vec<RoomPromptTemplateSlotDto>,
    pub supported_fields: RoomOrchestrationSupportedFieldsDto,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CreatorControlProjectionDto {
    pub id: String,
    pub label: String,
    pub description: Option<String>,
    pub kind: ControlKind,
    pub value: CoreCreatorControlValue,
    pub choices: Vec<String>,
    pub minimum: Option<f64>,
    pub maximum: Option<f64>,
    pub step: Option<f64>,
}

/// Exact, bounded room snapshot consumed by the production frontend adapter.
///
/// Only sections with a current Core/Shell read contract are represented.
/// Preview evidence, interaction state values, and module review state remain
/// owned by their separate explicit commands and are never synthesized here.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OrchestrationWorkspaceSnapshotDto {
    pub expected_head: Option<String>,
    pub room_config_revision: Option<u64>,
    pub prompt_preset_revision: u64,
    pub interaction_state_revision: u64,
    pub generation_target: Option<GenerationTargetDto>,
    pub prompt_presets: Vec<PromptPresetSummaryDto>,
    pub room_config: RoomOrchestrationConfigDto,
    pub prompt_blocks: Vec<PromptBlockProjectionDto>,
    pub creator_controls: Vec<CreatorControlProjectionDto>,
    pub knowledge_book_ids: Vec<String>,
    pub memory_records: Vec<MemoryRecordProjectionDto>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GetOrchestrationWorkspaceInput {
    pub conversation_id: String,
    pub branch_id: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SaveRoomOrchestrationConfigInput {
    pub conversation_id: String,
    pub branch_id: String,
    pub prompt_preset_id: Option<String>,
    pub generation_preset_id: Option<String>,
    pub response_length: PromptResponseLength,
    pub creativity: u8,
    pub reasoning_effort: RoomReasoningEffortDto,
    pub memory_enabled: bool,
    pub knowledge_enabled: bool,
    pub creator_values: BTreeMap<String, CoreCreatorControlValue>,
    #[serde(default)]
    pub user_name_override: Option<String>,
    #[serde(default)]
    pub author_note: Option<String>,
    #[serde(default)]
    pub group_context: Option<String>,
    #[serde(default)]
    pub template_slots: Vec<RoomPromptTemplateSlotDto>,
    /// Read-back review token only. Core derives the next variable map from
    /// declared creator controls and rejects arbitrary renderer-authored refs.
    pub variable_overrides: VariableMap,
    pub expected_revision: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SaveRoomOrchestrationConfigResultDto {
    pub room_config: RoomOrchestrationConfigDto,
    pub revision: u64,
    pub generation_target: Option<GenerationTargetDto>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReorderPromptBlocksInput {
    pub prompt_preset_id: String,
    pub ordered_block_ids: Vec<String>,
    pub expected_revision: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReorderPromptBlocksResultDto {
    pub blocks: Vec<PromptBlockProjectionDto>,
    pub revision: u64,
}

/// A UI document together with the exact compare-and-swap revision that
/// produced it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RevisionedDto<T> {
    pub value: T,
    pub revision: u64,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub deleted_at: Option<DateTime<Utc>>,
}

impl<T> From<Revisioned<T>> for RevisionedDto<T> {
    fn from(value: Revisioned<T>) -> Self {
        Self {
            value: value.value,
            revision: value.revision,
            created_at: value.created_at,
            updated_at: value.updated_at,
            deleted_at: value.deleted_at,
        }
    }
}

impl<T> RevisionedDto<T> {
    fn project<U>(self, project: impl FnOnce(T) -> U) -> RevisionedDto<U> {
        RevisionedDto {
            value: project(self.value),
            revision: self.revision,
            created_at: self.created_at,
            updated_at: self.updated_at,
            deleted_at: self.deleted_at,
        }
    }

    fn try_project<U>(
        self,
        project: impl FnOnce(T) -> ShellResult<U>,
    ) -> ShellResult<RevisionedDto<U>> {
        Ok(RevisionedDto {
            value: project(self.value)?,
            revision: self.revision,
            created_at: self.created_at,
            updated_at: self.updated_at,
            deleted_at: self.deleted_at,
        })
    }
}

impl From<PromptPreset> for PromptPresetSummaryDto {
    fn from(value: PromptPreset) -> Self {
        Self {
            id: value.id.0,
            name: value.name,
            schema_version: value.schema_version,
            block_count: u32::try_from(value.blocks.len()).unwrap_or(u32::MAX),
            default_generation_preset_id: value.default_generation_preset_id.map(|id| id.0),
        }
    }
}

impl TryFrom<PromptPreset> for CreatorPromptPresetDocumentDto {
    type Error = ShellError;

    fn try_from(value: PromptPreset) -> Result<Self, Self::Error> {
        if value.metadata.provenance.source_kind == SourceKind::ApplicationBuiltIn {
            return Err(shell_invalid(
                "application-built-in prompt presets are read-only",
            ));
        }

        let mut blocks = Vec::with_capacity(value.blocks.len());
        for block in value.blocks {
            let application_authority = block.authority == InstructionAuthority::Application;
            let application_zone = block.placement_zone == PlacementZone::ApplicationPolicy;
            if application_authority != application_zone {
                return Err(shell_invalid(
                    "prompt preset contains an inconsistent application-policy block",
                ));
            }
            if !application_authority {
                blocks.push(block.try_into()?);
            }
        }
        let cache_boundaries = value
            .cache_boundaries
            .into_iter()
            .filter(|boundary| {
                blocks.iter().any(|block: &CreatorPromptBlockDocumentDto| {
                    block.id == boundary.after_block_id.as_str()
                })
            })
            .collect();

        Ok(Self {
            id: value.id.0,
            name: value.name,
            schema_version: value.schema_version,
            blocks,
            controls: value.controls,
            default_values: value.default_values,
            default_generation_preset_id: value.default_generation_preset_id.map(|id| id.0),
            memory_profile_id: value.memory_profile_id.map(|id| id.0),
            knowledge_book_ids: value
                .knowledge_book_ids
                .into_iter()
                .map(|id| id.0)
                .collect(),
            transform_set_ids: value.transform_set_ids.into_iter().map(|id| id.0).collect(),
            module_ids: value.module_ids.into_iter().map(|id| id.0).collect(),
            cache_boundaries,
            metadata: value.metadata.try_into()?,
        })
    }
}

impl From<RevisionedDto<MemoryRecord>> for MemoryRecordProjectionDto {
    fn from(value: RevisionedDto<MemoryRecord>) -> Self {
        let record = value.value;
        let source_navigation = MemoryRecordSourceNavigationDto {
            conversation_id: record.conversation_id.0.clone(),
            branch_id: record.branch_id.0.clone(),
            start_message_id: record.source_start_message_id.0.clone(),
            end_message_id: record.source_end_message_id.0.clone(),
        };
        Self {
            id: record.id.0,
            conversation_id: record.conversation_id.0,
            branch_id: record.branch_id.0,
            kind: record.kind,
            title: record.title,
            summary: record.summary,
            importance: record.importance,
            keywords: record.keywords,
            pinned: record.pinned,
            excluded_from_conversation: record.excluded_from_conversation,
            excluded_from_character: record.excluded_from_character,
            source_navigation,
            invalidated_at: record.invalidated_at,
            updated_at: value.updated_at,
            revision: value.revision,
        }
    }
}

impl From<KnowledgeSelection> for KnowledgeSimulationDto {
    fn from(value: KnowledgeSelection) -> Self {
        let truncated = value.selected.len() > MAX_SELECTION_ITEMS
            || value.evidence.len() > MAX_SELECTION_ITEMS;
        Self {
            selected: value
                .selected
                .into_iter()
                .take(MAX_SELECTION_ITEMS)
                .collect(),
            evidence: value
                .evidence
                .into_iter()
                .take(MAX_SELECTION_ITEMS)
                .collect(),
            used_tokens: value.used_tokens,
            token_budget: value.token_budget,
            truncated,
        }
    }
}

impl From<ObjectRevision<ContentModule>> for ContentModuleRevisionSummaryDto {
    fn from(value: ObjectRevision<ContentModule>) -> Self {
        Self {
            revision_id: value.revision_id,
            revision: value.revision,
            sha256: value.sha256,
            created_at: value.created_at,
        }
    }
}

impl From<ObjectRevision<PromptPreset>> for PromptPresetRevisionSummaryDto {
    fn from(value: ObjectRevision<PromptPreset>) -> Self {
        Self {
            revision_id: value.revision_id,
            revision: value.revision,
            sha256: value.sha256,
            name: value.value.name,
            created_at: value.created_at,
            rollback_allowed: value.value.metadata.provenance.source_kind
                != SourceKind::ApplicationBuiltIn,
        }
    }
}

impl From<CorePromptPresetRevisionDiff> for PromptPresetRevisionDiffDto {
    fn from(value: CorePromptPresetRevisionDiff) -> Self {
        let truncated = value.changed_paths.len() > MAX_DIFF_PATHS;
        Self {
            preset_id: value.preset_id.0,
            from_revision_id: value.from_revision_id,
            from_revision: value.from_revision,
            from_sha256: value.from_sha256,
            to_revision_id: value.to_revision_id,
            to_revision: value.to_revision,
            to_sha256: value.to_sha256,
            changed_paths: value
                .changed_paths
                .into_iter()
                .take(MAX_DIFF_PATHS)
                .collect(),
            truncated,
            diff_sha256: value.diff_sha256,
        }
    }
}

impl From<CorePromptPresetRollbackReview> for PromptPresetRollbackReviewDto {
    fn from(value: CorePromptPresetRollbackReview) -> Self {
        Self {
            review_sha256: value.review_sha256,
            preset_id: value.preset_id.0,
            expected_current_state_revision: value.expected_current_state_revision,
            expected_current_revision_id: value.expected_current_revision_id,
            expected_current_sha256: value.expected_current_sha256,
            target_revision_id: value.target_revision_id,
            target_revision: value.target_revision,
            target_sha256: value.target_sha256,
            target_document_sha256: value.target_document_sha256,
            target_dependency_sha256: value.target_dependency_sha256,
            binding_snapshot_sha256: value.binding_snapshot_sha256,
            diff: value.diff.into(),
            reviewed_at: value.reviewed_at,
        }
    }
}

impl From<PromptPlanMessagePreview> for PromptPlanMessagePreviewDto {
    fn from(value: PromptPlanMessagePreview) -> Self {
        let truncated = value.source_message_ids.len() > MAX_PROMPT_PREVIEW_ITEMS;
        Self {
            sequence: value.sequence,
            block_id: value.block_id.0,
            block_kind: value.block_kind,
            requested_role: value.requested_role,
            effective_role: value.effective_role,
            estimated_tokens: value.estimated_tokens,
            source_message_ids: value
                .source_message_ids
                .into_iter()
                .take(MAX_PROMPT_PREVIEW_ITEMS)
                .map(|id| id.0)
                .collect(),
            truncated,
        }
    }
}

impl From<PromptProviderMessagePreview> for PromptProviderMessagePreviewDto {
    fn from(value: PromptProviderMessagePreview) -> Self {
        Self {
            sequence: value.sequence,
            block_id: value.block_id.0,
            effective_role: value.effective_role,
            wire_role: value.wire_role,
            placement: value.placement,
            estimated_tokens: value.estimated_tokens,
        }
    }
}

impl From<lorepia_core::ResolvedCacheDirective> for PromptCacheDirectivePreviewDto {
    fn from(value: lorepia_core::ResolvedCacheDirective) -> Self {
        Self {
            boundary_id: value.boundary_id.0,
            after_block_id: value.after_block_id.0,
            after_message_sequence: value.after_message_sequence,
            role_filter: value.role_filter,
            ttl: value.ttl,
            mode: value.mode,
            status: value.status,
        }
    }
}

impl From<KnowledgeActivationReason> for PromptKnowledgeSelectionReasonDto {
    fn from(value: KnowledgeActivationReason) -> Self {
        match value {
            KnowledgeActivationReason::Always => Self::Always,
            KnowledgeActivationReason::Manual => Self::Manual,
            KnowledgeActivationReason::Keyword { .. } => Self::Keyword,
            KnowledgeActivationReason::Regex { .. } => Self::Regex,
            KnowledgeActivationReason::Semantic { score_millionths } => {
                Self::Semantic { score_millionths }
            }
            KnowledgeActivationReason::Condition => Self::Condition,
            KnowledgeActivationReason::Recursive { parent_id } => Self::Recursive {
                parent_id: parent_id.0,
            },
        }
    }
}

impl From<PromptMemorySelectionReason> for PromptMemorySelectionReasonDto {
    fn from(value: PromptMemorySelectionReason) -> Self {
        match value {
            PromptMemorySelectionReason::Pinned => Self::Pinned,
            PromptMemorySelectionReason::CurrentBranch => Self::CurrentBranch,
            PromptMemorySelectionReason::SharedAncestor { source_branch_id } => {
                Self::SharedAncestor {
                    source_branch_id: source_branch_id.0,
                }
            }
            PromptMemorySelectionReason::Recency { score_millionths } => {
                Self::Recency { score_millionths }
            }
            PromptMemorySelectionReason::Similarity { score_millionths } => {
                Self::Similarity { score_millionths }
            }
            PromptMemorySelectionReason::Importance { score_millionths } => {
                Self::Importance { score_millionths }
            }
        }
    }
}

impl From<lorepia_core::BlockResolutionTrace> for PromptBlockResolutionTraceDto {
    fn from(value: lorepia_core::BlockResolutionTrace) -> Self {
        let truncated = value.knowledge_evidence.len() > MAX_PROMPT_PREVIEW_ITEMS
            || value.memory_record_ids.len() > MAX_PROMPT_PREVIEW_ITEMS
            || value.memory_evidence.len() > MAX_PROMPT_PREVIEW_ITEMS
            || value
                .knowledge_evidence
                .iter()
                .any(|evidence| evidence.reasons.len() > MAX_PROMPT_PREVIEW_ITEMS)
            || value
                .memory_evidence
                .iter()
                .any(|evidence| evidence.reasons.len() > MAX_PROMPT_PREVIEW_ITEMS);
        Self {
            block_id: value.block_id.0,
            block_kind: value.block_kind,
            source: PromptBlockSourceTraceDto {
                authority: value.source.authority,
                source_kind: value.source.source_kind,
                source_id: value.source.source_id,
                source_revision: value.source.source_revision,
                source_hash: value.source.source_hash,
            },
            status: value.status,
            original_estimated_tokens: value.original_estimated_tokens,
            final_estimated_tokens: value.final_estimated_tokens,
            produced_message_count: value.produced_message_count,
            knowledge_evidence: value
                .knowledge_evidence
                .into_iter()
                .take(MAX_PROMPT_PREVIEW_ITEMS)
                .map(|evidence| PromptKnowledgeSelectionEvidenceDto {
                    entry_id: evidence.entry_id.0,
                    selected: evidence.selected,
                    reasons: evidence
                        .reasons
                        .into_iter()
                        .take(MAX_PROMPT_PREVIEW_ITEMS)
                        .map(Into::into)
                        .collect(),
                    estimated_tokens: evidence.estimated_tokens,
                    exclusion_code: prompt_evidence_exclusion_code(
                        evidence.exclusion_reason.as_deref(),
                    ),
                })
                .collect(),
            memory_record_ids: value
                .memory_record_ids
                .into_iter()
                .take(MAX_PROMPT_PREVIEW_ITEMS)
                .map(|id| id.0)
                .collect(),
            memory_evidence: value
                .memory_evidence
                .into_iter()
                .take(MAX_PROMPT_PREVIEW_ITEMS)
                .map(|evidence| PromptMemorySelectionEvidenceDto {
                    record_id: evidence.record_id.0,
                    selected: evidence.selected,
                    lane: evidence.lane,
                    rank_millionths: evidence.rank_millionths,
                    estimated_tokens: evidence.estimated_tokens,
                    reasons: evidence
                        .reasons
                        .into_iter()
                        .take(MAX_PROMPT_PREVIEW_ITEMS)
                        .map(Into::into)
                        .collect(),
                    exclusion_code: prompt_evidence_exclusion_code(
                        evidence.exclusion_reason.as_deref(),
                    ),
                })
                .collect(),
            truncated,
        }
    }
}

impl From<lorepia_core::RoleMappingTrace> for PromptRoleMappingTraceDto {
    fn from(value: lorepia_core::RoleMappingTrace) -> Self {
        Self {
            block_id: value.block_id.0,
            requested_role: value.requested_role,
            effective_role: value.effective_role,
        }
    }
}

impl From<lorepia_core::OverflowTrace> for PromptOverflowTraceDto {
    fn from(value: lorepia_core::OverflowTrace) -> Self {
        Self {
            block_id: value.block_id.0,
            policy: value.policy,
            tokens_before: value.tokens_before,
            tokens_after: value.tokens_after,
        }
    }
}

impl PromptWarningCodeDto {
    fn from_message(value: &str) -> Self {
        if value.starts_with("cache boundary `")
            && value.ends_with("ignored because the provider lacks explicit caching")
        {
            Self::CacheBoundaryIgnoredUnsupported
        } else if value.starts_with("cache boundary `")
            && value.ends_with("ignored because the provider limit was reached")
        {
            Self::CacheBoundaryIgnoredLimit
        } else if value
            == "reasoning effort quick setting was omitted because the selected route does not expose that exact effort"
        {
            Self::ReasoningEffortOmitted
        } else if value
            == "creativity quick setting was ignored because the selected route does not expose temperature"
        {
            Self::CreativityIgnored
        } else if value == "knowledge retrieval was disabled by quick settings" {
            Self::KnowledgeDisabled
        } else if value == "memory retrieval was disabled by quick settings" {
            Self::MemoryDisabled
        } else if value.starts_with("cache boundary has volatile prompt content") {
            Self::CacheReuseSuboptimal
        } else if value.contains("local preset module dependencies were omitted") {
            Self::MissingModuleDependencies
        } else if value.starts_with("resolved-prompt transform failed for block ") {
            Self::ResolvedPromptTransformFailed
        } else if value.starts_with(
            "resolved-prompt transform exceeded the reviewed plan boundary and was ignored",
        ) {
            Self::ResolvedPromptTransformIgnored
        } else if value.starts_with("provider ignored cache boundary ") {
            Self::ProviderCacheBoundaryIgnored
        } else {
            Self::Other
        }
    }
}

fn prompt_evidence_exclusion_code(value: Option<&str>) -> Option<PromptEvidenceExclusionCodeDto> {
    value.map(|value| match value {
        "entry is disabled" => PromptEvidenceExclusionCodeDto::EntryDisabled,
        "deterministic activation probability gate" => {
            PromptEvidenceExclusionCodeDto::ActivationProbabilityGate
        }
        "activation rule did not match" => {
            PromptEvidenceExclusionCodeDto::ActivationRuleDidNotMatch
        }
        "entry exceeds its per-entry token limit" => {
            PromptEvidenceExclusionCodeDto::PerEntryTokenLimit
        }
        "knowledge token budget overflow" => {
            PromptEvidenceExclusionCodeDto::KnowledgeTokenBudgetOverflow
        }
        "entry does not fit the remaining knowledge token budget" => {
            PromptEvidenceExclusionCodeDto::KnowledgeRemainingTokenBudget
        }
        "removed by the prompt token budget" => PromptEvidenceExclusionCodeDto::PromptTokenBudget,
        "memory retrieval count limit reached" => {
            PromptEvidenceExclusionCodeDto::MemoryRetrievalCountLimit
        }
        "memory record does not fit the remaining token budgets" => {
            PromptEvidenceExclusionCodeDto::MemoryRemainingTokenBudget
        }
        "memory belongs to another conversation" => {
            PromptEvidenceExclusionCodeDto::OtherConversation
        }
        "memory has been invalidated" => PromptEvidenceExclusionCodeDto::MemoryInvalidated,
        "memory is excluded from this conversation" => {
            PromptEvidenceExclusionCodeDto::ExcludedFromConversation
        }
        "memory is excluded from this character" => {
            PromptEvidenceExclusionCodeDto::ExcludedFromCharacter
        }
        "memory source range is not on the active branch lineage" => {
            PromptEvidenceExclusionCodeDto::NotOnActiveBranchLineage
        }
        "memory source range is reversed" => PromptEvidenceExclusionCodeDto::ReversedSourceRange,
        _ => PromptEvidenceExclusionCodeDto::Other,
    })
}

impl From<PromptResolutionTrace> for PromptResolutionTraceDto {
    fn from(value: PromptResolutionTrace) -> Self {
        let truncated = value.blocks.len() > MAX_PROMPT_PREVIEW_ITEMS
            || value.role_mappings.len() > MAX_PROMPT_PREVIEW_ITEMS
            || value.overflow.len() > MAX_PROMPT_PREVIEW_ITEMS
            || value.warnings.len() > MAX_PROMPT_WARNINGS;
        Self {
            estimator_id: value.estimator_id,
            session_seed: value.session_seed,
            max_context_tokens: value.max_context_tokens,
            reserved_output_tokens: value.reserved_output_tokens,
            available_input_tokens: value.available_input_tokens,
            estimated_input_tokens: value.estimated_input_tokens,
            blocks: value
                .blocks
                .into_iter()
                .take(MAX_PROMPT_PREVIEW_ITEMS)
                .map(Into::into)
                .collect(),
            role_mappings: value
                .role_mappings
                .into_iter()
                .take(MAX_PROMPT_PREVIEW_ITEMS)
                .map(Into::into)
                .collect(),
            overflow: value
                .overflow
                .into_iter()
                .take(MAX_PROMPT_PREVIEW_ITEMS)
                .map(Into::into)
                .collect(),
            warnings: value
                .warnings
                .into_iter()
                .take(MAX_PROMPT_WARNINGS)
                .map(|warning| PromptWarningCodeDto::from_message(&warning))
                .collect(),
            truncated,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UpsertPromptPresetInput {
    pub value: CreatorPromptPresetDocumentDto,
    pub expected_revision: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DeletePromptPresetInput {
    pub prompt_preset_id: String,
    pub expected_revision: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GetPromptPresetInput {
    pub prompt_preset_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ListPromptPresetRevisionsInput {
    pub prompt_preset_id: String,
    pub limit: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DiffPromptPresetRevisionsInput {
    pub prompt_preset_id: String,
    pub from_revision: u64,
    pub to_revision: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReviewPromptPresetRollbackInput {
    pub prompt_preset_id: String,
    pub expected_current_revision: u64,
    pub target_revision: u64,
}

/// Minimal rollback confirmation. The frontend cannot submit a historical
/// document, dependency snapshot, binding snapshot, or provenance.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ApplyPromptPresetRollbackInput {
    pub prompt_preset_id: String,
    pub expected_current_revision: u64,
    pub target_revision: u64,
    pub approval_id: String,
    pub expected_review_sha256: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PromptPresetRevisionSummaryDto {
    pub revision_id: String,
    pub revision: u64,
    pub sha256: String,
    pub name: String,
    pub created_at: DateTime<Utc>,
    pub rollback_allowed: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PromptPresetRevisionListDto {
    pub revisions: Vec<PromptPresetRevisionSummaryDto>,
    pub truncated: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PromptPresetRevisionDiffDto {
    pub preset_id: String,
    pub from_revision_id: String,
    pub from_revision: u64,
    pub from_sha256: String,
    pub to_revision_id: String,
    pub to_revision: u64,
    pub to_sha256: String,
    pub changed_paths: Vec<String>,
    pub truncated: bool,
    pub diff_sha256: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PromptPresetRollbackReviewDto {
    pub review_sha256: String,
    pub preset_id: String,
    pub expected_current_state_revision: u64,
    pub expected_current_revision_id: String,
    pub expected_current_sha256: String,
    pub target_revision_id: String,
    pub target_revision: u64,
    pub target_sha256: String,
    pub target_document_sha256: String,
    pub target_dependency_sha256: String,
    pub binding_snapshot_sha256: String,
    pub diff: PromptPresetRevisionDiffDto,
    pub reviewed_at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PromptPresetRollbackReceiptDto {
    pub preset_id: String,
    pub target_revision: u64,
    pub applied_revision_id: String,
    pub applied_revision: u64,
    pub applied_sha256: String,
    pub review_sha256: String,
    pub approval_id: String,
    pub approval_sha256: String,
    pub approved_at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UpsertTaskProfileInput {
    pub value: TaskProfileDto,
    pub expected_revision: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GetTaskProfileInput {
    pub task_profile_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DeleteTaskProfileInput {
    pub task_profile_id: String,
    pub expected_revision: u64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UpsertMemoryProfileInput {
    pub value: MemoryProfileDto,
    pub expected_revision: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GetMemoryProfileInput {
    pub memory_profile_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DeleteMemoryProfileInput {
    pub memory_profile_id: String,
    pub expected_revision: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GetMemoryRecordInput {
    pub conversation_id: String,
    pub branch_id: String,
    pub memory_record_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DeleteMemoryRecordInput {
    pub conversation_id: String,
    pub branch_id: String,
    pub memory_record_id: String,
    pub expected_revision: u64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UpsertKnowledgeBookInput {
    pub value: KnowledgeBookDto,
    pub expected_revision: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GetKnowledgeBookInput {
    pub knowledge_book_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DeleteKnowledgeBookInput {
    pub knowledge_book_id: String,
    pub expected_revision: u64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UpsertTransformSetInput {
    pub value: TransformSetDto,
    pub expected_revision: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GetTransformSetInput {
    pub transform_set_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DeleteTransformSetInput {
    pub transform_set_id: String,
    pub expected_revision: u64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UpsertInteractionRuleSetInput {
    pub value: InteractionRuleSetDto,
    pub expected_revision: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GetInteractionRuleSetInput {
    pub interaction_rule_set_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DeleteInteractionRuleSetInput {
    pub interaction_rule_set_id: String,
    pub expected_revision: u64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UpsertContentModuleInput {
    pub value: ContentModuleDto,
    pub expected_revision: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GetContentModuleInput {
    pub content_module_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DeleteContentModuleInput {
    pub content_module_id: String,
    pub expected_revision: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ListPromptPresetBindingsInput {
    pub scope: ModuleScope,
    pub target_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ListMemoryRecordsInput {
    pub conversation_id: String,
    pub branch_id: String,
    pub include_invalidated: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ListRetryableMemoryQueryEmbeddingsInput {
    pub conversation_id: String,
    pub branch_id: String,
    pub limit: u32,
}

/// Explicit authorization to requeue one provider memory job whose outcome is
/// unknown after interruption. The expected revision keeps the retry a single
/// compare-and-swap operation instead of a blind replay.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RetryInterruptedMemoryJobInput {
    pub conversation_id: String,
    pub branch_id: String,
    pub memory_job_id: String,
    pub expected_revision: u64,
    pub acknowledge_unknown_outcome: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MemoryJobRetryKindDto {
    Summary,
    Embedding,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MemoryJobRetryStatusDto {
    Queued,
}

/// Bounded confirmation of an explicit retry. Queue payloads, idempotency
/// keys, raw provider errors, and credentials remain inside Core.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MemoryJobRetryReceiptDto {
    pub memory_job_id: String,
    pub kind: MemoryJobRetryKindDto,
    pub status: MemoryJobRetryStatusDto,
    pub revision: u64,
    pub conversation_id: String,
    pub branch_id: String,
    pub source_start_message_id: String,
    pub source_end_message_id: String,
    pub attempt: u32,
}

impl TryFrom<ClaimedMemoryJob> for MemoryJobRetryReceiptDto {
    type Error = ShellError;

    fn try_from(value: ClaimedMemoryJob) -> Result<Self, Self::Error> {
        let kind = match value.job.value.kind {
            MemoryJobKind::Summary => MemoryJobRetryKindDto::Summary,
            MemoryJobKind::Embedding => MemoryJobRetryKindDto::Embedding,
            MemoryJobKind::InvalidateRange => {
                return Err(ShellError::from(CoreError::internal(
                    "Core returned a non-provider memory job from explicit retry",
                )));
            }
        };
        if value.job.value.status != MemoryJobStatus::Queued {
            return Err(ShellError::from(CoreError::internal(
                "Core returned a memory job that was not requeued",
            )));
        }
        Ok(Self {
            memory_job_id: value.job.value.id.0,
            kind,
            status: MemoryJobRetryStatusDto::Queued,
            revision: value.job.revision,
            conversation_id: value.job.value.conversation_id.0,
            branch_id: value.job.value.branch_id.0,
            source_start_message_id: value.job.value.source_start_message_id.0,
            source_end_message_id: value.job.value.source_end_message_id.0,
            attempt: value.job.value.attempt,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RetryMemoryQueryEmbeddingInput {
    pub conversation_id: String,
    pub branch_id: String,
    pub id: String,
    pub expected_revision: u64,
    pub acknowledge_unknown_outcome: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MemoryQueryEmbeddingRetryStatusDto {
    Interrupted,
    Failed,
    Cancelled,
    Queued,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MemoryQueryEmbeddingRetryCandidateDto {
    pub id: String,
    pub status: MemoryQueryEmbeddingRetryStatusDto,
    pub revision: u64,
    pub conversation_id: String,
    pub branch_id: String,
    pub error_code: Option<String>,
    pub requires_unknown_outcome_acknowledgement: bool,
}

impl TryFrom<MemoryQueryEmbeddingRetryCandidate> for MemoryQueryEmbeddingRetryCandidateDto {
    type Error = ShellError;

    fn try_from(value: MemoryQueryEmbeddingRetryCandidate) -> Result<Self, Self::Error> {
        let status = match value.status {
            MemoryQueryEmbeddingStatus::Interrupted => {
                MemoryQueryEmbeddingRetryStatusDto::Interrupted
            }
            MemoryQueryEmbeddingStatus::Failed => MemoryQueryEmbeddingRetryStatusDto::Failed,
            MemoryQueryEmbeddingStatus::Cancelled => MemoryQueryEmbeddingRetryStatusDto::Cancelled,
            MemoryQueryEmbeddingStatus::Queued => MemoryQueryEmbeddingRetryStatusDto::Queued,
            MemoryQueryEmbeddingStatus::Running | MemoryQueryEmbeddingStatus::Succeeded => {
                return Err(ShellError::from(CoreError::internal(
                    "Core returned a non-retryable memory query embedding",
                )));
            }
        };
        Ok(Self {
            id: value.id,
            status,
            revision: value.revision,
            conversation_id: value.conversation_id.0,
            branch_id: value.branch_id.0,
            error_code: value.error_code,
            requires_unknown_outcome_acknowledgement: value
                .requires_unknown_outcome_acknowledgement,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ListInterruptedMemoryJobsInput {
    pub conversation_id: String,
    pub branch_id: String,
    pub limit: u32,
}

/// One interrupted memory job offered for an explicit user retry decision.
///
/// Interrupted jobs are never requeued automatically because the provider may
/// already have applied a side effect. The bounded interruption audit lets the
/// user see why the job stopped before acknowledging that unknown outcome.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InterruptedMemoryJobDto {
    pub memory_job_id: String,
    pub kind: MemoryJobRetryKindDto,
    pub revision: u64,
    pub conversation_id: String,
    pub branch_id: String,
    pub source_start_message_id: String,
    pub source_end_message_id: String,
    pub attempt: u32,
    pub interruption_count: u32,
    pub last_interrupted_at: Option<DateTime<Utc>>,
    pub last_error_code: Option<String>,
}

impl TryFrom<InterruptedMemoryJob> for InterruptedMemoryJobDto {
    type Error = ShellError;

    fn try_from(value: InterruptedMemoryJob) -> Result<Self, Self::Error> {
        let kind = match value.job.value.kind {
            MemoryJobKind::Summary => MemoryJobRetryKindDto::Summary,
            MemoryJobKind::Embedding => MemoryJobRetryKindDto::Embedding,
            MemoryJobKind::InvalidateRange => {
                return Err(ShellError::from(CoreError::internal(
                    "Core listed a non-provider memory job as interrupted",
                )));
            }
        };
        if value.job.value.status != MemoryJobStatus::Interrupted {
            return Err(ShellError::from(CoreError::internal(
                "Core listed a memory job that is not interrupted",
            )));
        }
        let interruption_count = u32::try_from(value.interruptions.len()).map_err(|_| {
            ShellError::from(CoreError::internal(
                "memory job interruption history exceeds the shell counter",
            ))
        })?;
        let last = value.interruptions.last();
        Ok(Self {
            memory_job_id: value.job.value.id.as_str().to_owned(),
            kind,
            revision: value.job.revision,
            conversation_id: value.job.value.conversation_id.0,
            branch_id: value.job.value.branch_id.0,
            source_start_message_id: value.job.value.source_start_message_id.0,
            source_end_message_id: value.job.value.source_end_message_id.0,
            attempt: value.job.value.attempt,
            interruption_count,
            last_interrupted_at: last.map(|entry| entry.interrupted_at),
            last_error_code: last.and_then(|entry| entry.error_code.clone()),
        })
    }
}

/// Closed source range that the UI may use to focus already-loaded messages.
///
/// It carries stable content identifiers only; no database row, package path,
/// host path, or raw memory provenance crosses the webview boundary.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MemoryRecordSourceNavigationDto {
    pub conversation_id: String,
    pub branch_id: String,
    pub start_message_id: String,
    pub end_message_id: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MemoryRecordProjectionDto {
    pub id: String,
    pub conversation_id: String,
    pub branch_id: String,
    pub kind: MemoryKind,
    pub title: String,
    pub summary: String,
    pub importance: u8,
    pub keywords: Vec<String>,
    pub pinned: bool,
    pub excluded_from_conversation: bool,
    pub excluded_from_character: bool,
    pub source_navigation: MemoryRecordSourceNavigationDto,
    pub invalidated_at: Option<DateTime<Utc>>,
    pub updated_at: DateTime<Utc>,
    pub revision: u64,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MemoryRecordPatchDto {
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub summary: Option<String>,
    #[serde(default)]
    pub importance: Option<u8>,
    #[serde(default)]
    pub keywords: Option<Vec<String>>,
    #[serde(default)]
    pub pinned: Option<bool>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PatchMemoryRecordInput {
    pub conversation_id: String,
    pub branch_id: String,
    pub memory_record_id: String,
    pub patch: MemoryRecordPatchDto,
    pub expected_revision: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MemoryRecordExclusionScopeDto {
    Conversation,
    Character,
}

impl From<MemoryRecordExclusionScopeDto> for MemoryRecordExclusionScope {
    fn from(value: MemoryRecordExclusionScopeDto) -> Self {
        match value {
            MemoryRecordExclusionScopeDto::Conversation => Self::Conversation,
            MemoryRecordExclusionScopeDto::Character => Self::Character,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SetMemoryRecordExclusionInput {
    pub conversation_id: String,
    pub branch_id: String,
    pub memory_record_id: String,
    pub scope: MemoryRecordExclusionScopeDto,
    pub excluded: bool,
    pub expected_revision: u64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MemoryRecordListDto {
    pub records: Vec<MemoryRecordProjectionDto>,
    pub truncated: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SimulateKnowledgeInput {
    pub knowledge_book_id: String,
    pub sample_texts: Vec<String>,
    pub manual_entry_ids: Vec<String>,
    pub semantic_scores: Vec<SemanticKnowledgeScore>,
    pub variables: VariableMap,
    pub supported_capabilities: Vec<CapabilityKey>,
    pub token_estimates: Vec<KnowledgeTokenEstimateInput>,
    pub activation_seed: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct KnowledgeTokenEstimateInput {
    pub knowledge_entry_id: String,
    pub tokens: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct KnowledgeSimulationDto {
    pub selected: Vec<SelectedKnowledgeEntryDto>,
    pub evidence: Vec<KnowledgeSelectionEvidenceDto>,
    pub used_tokens: u32,
    pub token_budget: u32,
    pub truncated: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PreviewTransformRuleInput {
    pub transform_set_id: String,
    pub transform_rule_id: String,
    pub sample_text: String,
    pub variables: VariableMap,
    pub supported_capabilities: Vec<CapabilityKey>,
    pub approved_import_source_ids: Vec<String>,
    pub allow_resolved_prompt: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TransformRulePreviewDto {
    pub phase: TransformPhase,
    pub original: String,
    pub output: String,
    pub changed: bool,
    pub rendering: String,
    pub reports: Vec<TransformRuleReport>,
    pub diff: Option<TransformDiff>,
    pub error: Option<TransformFailure>,
    pub truncated: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ListContentModuleBindingsInput {
    pub content_module_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ListContentModuleRevisionsInput {
    pub content_module_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DiffContentModuleRevisionsInput {
    pub content_module_id: String,
    pub from_revision: u64,
    pub to_revision: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EvaluateContentModuleShareInput {
    pub content_module_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ContentModuleRevisionSummaryDto {
    pub revision_id: String,
    pub revision: u64,
    pub sha256: String,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ContentModuleRevisionListDto {
    pub revisions: Vec<ContentModuleRevisionSummaryDto>,
    pub truncated: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ContentModuleRevisionDiffDto {
    pub content_module_id: String,
    pub from_revision: u64,
    pub to_revision: u64,
    pub from_sha256: String,
    pub to_sha256: String,
    pub changed_paths: Vec<String>,
    pub truncated: bool,
}

pub(crate) struct ShellTaskCredentialBroker<'a> {
    pub(crate) credential_reader: &'a dyn TaskCredentialReader,
}

impl TaskCredentialBroker for ShellTaskCredentialBroker<'_> {
    fn credential_for<'a>(
        &'a self,
        connection_id: &'a ProviderConnectionId,
    ) -> Pin<Box<dyn Future<Output = CoreResult<ConnectionBoundCredential>> + Send + 'a>> {
        Box::pin(async move {
            match self
                .credential_reader
                .credential_for(connection_id.as_str())
                .await
            {
                TaskCredentialRead::Available(credential) => Ok(ConnectionBoundCredential::new(
                    connection_id.clone(),
                    Some(credential.into_core_value()),
                )),
                TaskCredentialRead::AvailableWithLease { credential, lease } => {
                    Ok(ConnectionBoundCredential::new_with_dispatch_lease(
                        connection_id.clone(),
                        Some(credential.into_core_value()),
                        lease.into_inner(),
                    ))
                }
                TaskCredentialRead::Missing => {
                    Ok(ConnectionBoundCredential::new(connection_id.clone(), None))
                }
                TaskCredentialRead::MissingWithLease(dispatch_lease) => {
                    Ok(ConnectionBoundCredential::new_with_dispatch_lease(
                        connection_id.clone(),
                        None,
                        dispatch_lease.into_inner(),
                    ))
                }
                TaskCredentialRead::Unreadable => Err(CoreError::new(
                    CoreErrorCode::ProviderAuthFailed,
                    "native task credential is unavailable",
                    true,
                )),
            }
        })
    }
}

impl ShellApi {
    /// Marks jobs left running by the prior process as interrupted.
    ///
    /// Unknown provider side effects are never replayed automatically.
    pub fn recover_running_memory_jobs(&self) -> ShellResult<usize> {
        let provider_jobs = self
            .core
            .recover_running_memory_jobs()
            .map(|jobs| jobs.len())
            .map_err(ShellError::from)?;
        let query_jobs = self
            .core
            .recover_running_memory_query_embeddings()
            .map_err(ShellError::from)?;
        provider_jobs.checked_add(query_jobs).ok_or_else(|| {
            ShellError::from(CoreError::internal("memory recovery count overflowed"))
        })
    }

    /// Executes at most one queued memory job through a native credential
    /// reader. This Rust-only method is intentionally not a Tauri command.
    pub async fn execute_next_memory_job(
        &self,
        credential_reader: &dyn TaskCredentialReader,
        cancelled: tokio::sync::watch::Receiver<bool>,
    ) -> ShellResult<bool> {
        self.core
            .execute_next_memory_job(&ShellTaskCredentialBroker { credential_reader }, cancelled)
            .await
            .map(|result| result.is_some())
            .map_err(ShellError::from)
    }

    fn validate_prompt_preset(&self, value: &PromptPresetDto) -> ShellResult<()> {
        validate_creator_prompt_preset_input(value)?;
        validate_identifier("prompt_preset_id", value.id.as_str())?;
        validate_document(value)?;
        self.core
            .validate_prompt_preset(value)
            .map_err(ShellError::from)
    }

    pub fn validate_editable_prompt_preset(
        &self,
        value: CreatorPromptPresetDocumentDto,
    ) -> ShellResult<()> {
        let value = PromptPreset::from(value);
        self.validate_prompt_preset(&value)
    }

    pub fn resolve_prompt_preview(
        &self,
        input: ResolvePromptPreviewInput,
    ) -> ShellResult<ExpertPromptPreviewDto> {
        let operation_context = validate_generation_operation_context(
            input.operation_nonce.as_deref(),
            input.generation_attempt_id.as_deref(),
        )?;
        let request = prompt_plan_request(PromptPlanRequestParts {
            conversation_id: input.conversation_id,
            branch_id: input.branch_id,
            expected_head: input.expected_head,
            user_text: input.user_text,
            generation_target: input.generation_target,
            prompt_preset_id: input.prompt_preset_id,
            variable_overrides: input.variable_overrides,
            expected_plan_hash: input.expected_plan_hash,
        })?;
        let preview = self
            .core
            .resolve_prompt_preview(&request, operation_context.as_core())
            .map_err(ShellError::from)?;
        let preview = bounded_expert_prompt_preview(preview)?;
        validate_document(&preview)?;
        Ok(preview)
    }

    /// Native async preview path with exact auxiliary-task credential
    /// resolution. The synchronous compatibility method above remains valid
    /// only for prompt presets whose exact memory profile has no embedding
    /// task; Core rejects any attempted synchronous embedding fallback.
    pub async fn resolve_prompt_preview_async(
        &self,
        input: ResolvePromptPreviewInput,
        credential_reader: &dyn TaskCredentialReader,
        cancelled: tokio::sync::watch::Receiver<bool>,
    ) -> ShellResult<ExpertPromptPreviewDto> {
        let operation_context = validate_generation_operation_context(
            input.operation_nonce.as_deref(),
            input.generation_attempt_id.as_deref(),
        )?;
        let request = prompt_plan_request(PromptPlanRequestParts {
            conversation_id: input.conversation_id,
            branch_id: input.branch_id,
            expected_head: input.expected_head,
            user_text: input.user_text,
            generation_target: input.generation_target,
            prompt_preset_id: input.prompt_preset_id,
            variable_overrides: input.variable_overrides,
            expected_plan_hash: input.expected_plan_hash,
        })?;
        let preview = self
            .core
            .resolve_prompt_preview_async(
                &request,
                operation_context.as_core(),
                &ShellTaskCredentialBroker { credential_reader },
                cancelled,
            )
            .await
            .map_err(ShellError::from)?;
        let preview = bounded_expert_prompt_preview(preview)?;
        validate_document(&preview)?;
        Ok(preview)
    }

    /// Dispatches only the exact attempt and execution hash returned by an
    /// expert preview. Ordinary message send remains a separate unreviewed
    /// product action.
    pub async fn send_reviewed_prompt_async(
        &self,
        input: ReviewedPromptSendInput,
        credential: GenerationCredential,
        credential_reader: &dyn TaskCredentialReader,
        cancelled: tokio::sync::watch::Receiver<bool>,
    ) -> ShellResult<StartedGeneration> {
        validate_identifier("generation_attempt_id", &input.generation_attempt_id)?;
        validate_sha256("expected_plan_hash", &input.expected_plan_hash)?;
        let conversation_id = input.conversation_id.clone();
        let branch_id = input.branch_id.clone();
        let request = prompt_plan_request(PromptPlanRequestParts {
            conversation_id: input.conversation_id,
            branch_id: input.branch_id,
            expected_head: input.expected_head,
            user_text: input.user_text,
            generation_target: input.generation_target.clone(),
            prompt_preset_id: input.prompt_preset_id,
            variable_overrides: input.variable_overrides,
            expected_plan_hash: Some(input.expected_plan_hash),
        })?;
        let (connection_id, credential, access_authority, dispatch_lease) =
            match credential.into_kind() {
                GenerationCredentialKind::Connection {
                    connection_id,
                    credential,
                    access_authority,
                    dispatch_lease,
                } => (connection_id, credential, access_authority, dispatch_lease),
                GenerationCredentialKind::Legacy { .. } => {
                    return Err(ShellError::from(CoreError::invalid(
                        "reviewed prompt send requires a generation-target credential",
                    )));
                }
            };
        validate_identifier("connection_id", &connection_id)?;
        if !self
            .list_model_routes(&connection_id)?
            .iter()
            .any(|route| route.id == input.generation_target.model_route_id)
        {
            return Err(ShellError::from(CoreError::invalid(
                "credential does not belong to the reviewed generation target",
            )));
        }
        let receiver = self.core.subscribe_events();
        let credential = match access_authority {
            Some(authority) => ConnectionBoundCredential::new_with_access_authority(
                ProviderConnectionId::from(connection_id),
                credential.map(crate::SecretCredential::into_core_value),
                ProviderCredentialAccessAuthority {
                    authority_id: authority.authority_id,
                    connection_binding_sha256: authority.connection_binding_sha256,
                },
            ),
            None => ConnectionBoundCredential::new(
                ProviderConnectionId::from(connection_id),
                credential.map(crate::SecretCredential::into_core_value),
            ),
        };
        let credential = match dispatch_lease {
            Some(lease) => credential.with_dispatch_lease(lease.into_inner()),
            None => credential,
        };
        let generation_id = self
            .core
            .send_message_with_prompt_plan_async(
                &request,
                &lorepia_core::GenerationId(input.generation_attempt_id),
                credential,
                &ShellTaskCredentialBroker { credential_reader },
                cancelled,
            )
            .await
            .map_err(ShellError::from)?;
        Ok(StartedGeneration::new(
            generation_id,
            receiver,
            conversation_id,
            branch_id,
        ))
    }

    pub fn explain_prompt_plan(
        &self,
        input: ExplainPromptPlanInput,
    ) -> ShellResult<PromptResolutionTraceDto> {
        validate_sha256("plan_hash", &input.plan_hash)?;
        let operation_context = validate_generation_operation_context(
            input.operation_nonce.as_deref(),
            input.generation_attempt_id.as_deref(),
        )?;
        let request = prompt_plan_request(PromptPlanRequestParts {
            conversation_id: input.conversation_id,
            branch_id: input.branch_id,
            expected_head: input.expected_head,
            user_text: input.user_text,
            generation_target: input.generation_target,
            prompt_preset_id: input.prompt_preset_id,
            variable_overrides: input.variable_overrides,
            expected_plan_hash: Some(input.plan_hash.clone()),
        })?;
        let trace = self
            .core
            .explain_prompt_plan(&request, operation_context.as_core(), &input.plan_hash)
            .map(PromptResolutionTraceDto::from)
            .map_err(ShellError::from)?;
        validate_document(&trace)?;
        Ok(trace)
    }

    fn upsert_prompt_preset(
        &self,
        input: UpsertPromptPresetInput,
    ) -> ShellResult<RevisionedDto<PromptPresetDto>> {
        let value = PromptPreset::from(input.value);
        self.validate_prompt_preset(&value)?;
        self.core
            .upsert_prompt_preset(&value, input.expected_revision)
            .map(Into::into)
            .map_err(ShellError::from)
            .and_then(validated_output)
    }

    pub fn upsert_prompt_preset_summary(
        &self,
        input: UpsertPromptPresetInput,
    ) -> ShellResult<RevisionedDto<PromptPresetSummaryDto>> {
        self.upsert_prompt_preset(input)
            .map(|value| value.project(Into::into))
    }

    fn get_prompt_preset(
        &self,
        input: GetPromptPresetInput,
    ) -> ShellResult<RevisionedDto<PromptPresetDto>> {
        validate_identifier("prompt_preset_id", &input.prompt_preset_id)?;
        self.core
            .get_prompt_preset(&PromptPresetId::from(input.prompt_preset_id))
            .map(Into::into)
            .map_err(ShellError::from)
            .and_then(validated_output)
    }

    pub fn get_prompt_preset_summary(
        &self,
        input: GetPromptPresetInput,
    ) -> ShellResult<RevisionedDto<PromptPresetSummaryDto>> {
        self.get_prompt_preset(input)
            .map(|value| value.project(Into::into))
    }

    pub fn get_editable_prompt_preset(
        &self,
        input: GetPromptPresetInput,
    ) -> ShellResult<RevisionedDto<CreatorPromptPresetDocumentDto>> {
        validate_identifier("prompt_preset_id", &input.prompt_preset_id)?;
        let stored: RevisionedDto<PromptPresetDto> = self
            .core
            .get_editable_prompt_preset(&PromptPresetId::from(input.prompt_preset_id))
            .map(Into::into)
            .map_err(ShellError::from)?;
        let result = RevisionedDto {
            value: stored.value.try_into()?,
            revision: stored.revision,
            created_at: stored.created_at,
            updated_at: stored.updated_at,
            deleted_at: stored.deleted_at,
        };
        validated_output(result)
    }

    fn list_prompt_presets(&self) -> ShellResult<Vec<RevisionedDto<PromptPresetDto>>> {
        self.core
            .list_prompt_presets()
            .and_then(bound_core_collection)
            .map(|values| values.into_iter().map(Into::into).collect())
            .map_err(ShellError::from)
            .and_then(validated_output)
    }

    pub fn list_prompt_preset_summaries(
        &self,
    ) -> ShellResult<Vec<RevisionedDto<PromptPresetSummaryDto>>> {
        self.list_prompt_presets().map(|values| {
            values
                .into_iter()
                .map(|value| value.project(Into::into))
                .collect()
        })
    }

    /// Loads the bounded, credential-free room snapshot required by the live
    /// orchestration UI. Every returned selection is resolved by Core; the
    /// renderer does not reproduce prompt or generation target precedence.
    pub fn get_orchestration_workspace(
        &self,
        input: GetOrchestrationWorkspaceInput,
    ) -> ShellResult<OrchestrationWorkspaceSnapshotDto> {
        validate_identifier("conversation_id", &input.conversation_id)?;
        validate_identifier("branch_id", &input.branch_id)?;
        let conversation_id = ConversationId(input.conversation_id);
        let branch_id = ConversationBranchId(input.branch_id);
        let room = self
            .core
            .get_room_orchestration_config(&conversation_id, &branch_id)
            .map_err(ShellError::from)?;
        let selected_preset = self
            .core
            .get_prompt_preset(&room.prompt_preset_id)
            .map_err(ShellError::from)?;
        let prompt_blocks = project_prompt_blocks(&selected_preset.value)?;
        let creator_controls =
            project_creator_controls(&selected_preset.value.controls, &room.creator_values)?;
        let knowledge_book_ids = selected_preset
            .value
            .knowledge_book_ids
            .iter()
            .map(|id| id.0.clone())
            .collect();
        let prompt_presets = self
            .core
            .list_prompt_presets()
            .and_then(bound_core_collection)
            .map_err(ShellError::from)?
            .into_iter()
            .map(|stored| PromptPresetSummaryDto::from(stored.value))
            .collect();
        let expected_head = self
            .core
            .list_branch_messages(&branch_id)
            .map_err(ShellError::from)?
            .last()
            .map(|message| message.id.0.clone());
        let memory_records = self
            .core
            .list_memory_records(&conversation_id, &branch_id, false)
            .map_err(ShellError::from)?
            .into_iter()
            .take(MAX_MEMORY_LIST_ITEMS.saturating_add(1))
            .map(RevisionedDto::from)
            .map(Into::into)
            .collect();
        let interaction_state_revision = self
            .core
            .get_interaction_state_revision(&conversation_id, &branch_id)
            .map_err(ShellError::from)?;
        let generation_target = room_generation_target_dto(&room);
        let result = OrchestrationWorkspaceSnapshotDto {
            expected_head,
            room_config_revision: room.binding_revision,
            prompt_preset_revision: selected_preset.revision,
            interaction_state_revision,
            generation_target,
            prompt_presets,
            room_config: project_room_orchestration_config(room),
            prompt_blocks,
            creator_controls,
            knowledge_book_ids,
            memory_records,
        };
        validate_document(&result)?;
        Ok(result)
    }

    /// Persists every Core-supported quick setting with exact binding CAS.
    /// The submitted variable map is a read-back token only; new values are
    /// derived from the selected preset's declared creator controls in Core.
    pub fn save_room_orchestration_config(
        &self,
        input: SaveRoomOrchestrationConfigInput,
    ) -> ShellResult<SaveRoomOrchestrationConfigResultDto> {
        validate_identifier("conversation_id", &input.conversation_id)?;
        validate_identifier("branch_id", &input.branch_id)?;
        validate_optional_identifier("prompt_preset_id", input.prompt_preset_id.as_deref())?;
        validate_optional_identifier(
            "generation_preset_id",
            input.generation_preset_id.as_deref(),
        )?;
        validate_room_prompt_source_input(&input)?;
        let conversation_id = ConversationId(input.conversation_id);
        let branch_id = ConversationBranchId(input.branch_id);
        let current = self
            .core
            .get_room_orchestration_config(&conversation_id, &branch_id)
            .map_err(ShellError::from)?;
        if input.variable_overrides != current.variable_overrides {
            return Err(shell_invalid(
                "room variable overrides changed before save or were not derived by Core",
            ));
        }
        let patch = lorepia_core::RoomOrchestrationConfigPatch {
            prompt_preset_id: input.prompt_preset_id.map(PromptPresetId::from),
            generation_preset_id: input.generation_preset_id.map(GenerationPresetId::from),
            creator_values: input.creator_values,
            response_length: input.response_length,
            creativity: input.creativity,
            reasoning_effort: input.reasoning_effort.into(),
            memory_enabled: input.memory_enabled,
            knowledge_enabled: input.knowledge_enabled,
            user_name_override: input.user_name_override,
            author_note: input.author_note,
            group_context: input.group_context,
            template_slots: input
                .template_slots
                .into_iter()
                .map(|slot| TemplateSlot {
                    name: slot.name,
                    value: slot.value,
                })
                .collect(),
        };
        let saved = self
            .core
            .save_room_orchestration_config(
                &conversation_id,
                &branch_id,
                input.expected_revision,
                &patch,
            )
            .map_err(ShellError::from)?;
        let revision = saved.binding_revision.ok_or_else(|| {
            ShellError::from(CoreError::internal(
                "saved room orchestration binding has no revision",
            ))
        })?;
        let generation_target = room_generation_target_dto(&saved);
        let result = SaveRoomOrchestrationConfigResultDto {
            room_config: project_room_orchestration_config(saved),
            revision,
            generation_target,
        };
        validate_document(&result)?;
        Ok(result)
    }

    pub fn list_prompt_preset_revisions(
        &self,
        input: ListPromptPresetRevisionsInput,
    ) -> ShellResult<PromptPresetRevisionListDto> {
        validate_identifier("prompt_preset_id", &input.prompt_preset_id)?;
        if !(1..=MAX_PROMPT_PRESET_REVISIONS).contains(&input.limit) {
            return Err(shell_invalid(format!(
                "limit must be between 1 and {MAX_PROMPT_PRESET_REVISIONS}"
            )));
        }
        let values = self
            .core
            .list_prompt_preset_revisions(&PromptPresetId::from(input.prompt_preset_id))
            .map_err(ShellError::from)?;
        let truncated = values.len() > input.limit;
        let skip = values.len().saturating_sub(input.limit);
        let result = PromptPresetRevisionListDto {
            revisions: values.into_iter().skip(skip).map(Into::into).collect(),
            truncated,
        };
        validate_document(&result)?;
        Ok(result)
    }

    pub fn diff_prompt_preset_revisions(
        &self,
        input: DiffPromptPresetRevisionsInput,
    ) -> ShellResult<PromptPresetRevisionDiffDto> {
        validate_identifier("prompt_preset_id", &input.prompt_preset_id)?;
        validate_positive_revision("from_revision", input.from_revision)?;
        validate_positive_revision("to_revision", input.to_revision)?;
        let result = self
            .core
            .diff_prompt_preset_revisions(
                &PromptPresetId::from(input.prompt_preset_id),
                input.from_revision,
                input.to_revision,
            )
            .map(PromptPresetRevisionDiffDto::from)
            .map_err(ShellError::from)?;
        validate_document(&result)?;
        Ok(result)
    }

    pub fn review_prompt_preset_rollback(
        &self,
        input: ReviewPromptPresetRollbackInput,
    ) -> ShellResult<PromptPresetRollbackReviewDto> {
        validate_identifier("prompt_preset_id", &input.prompt_preset_id)?;
        validate_positive_revision("expected_current_revision", input.expected_current_revision)?;
        validate_positive_revision("target_revision", input.target_revision)?;
        let result = self
            .core
            .review_prompt_preset_rollback(
                &PromptPresetId::from(input.prompt_preset_id),
                input.expected_current_revision,
                input.target_revision,
            )
            .map(PromptPresetRollbackReviewDto::from)
            .map_err(ShellError::from)?;
        validate_document(&result)?;
        Ok(result)
    }

    pub fn apply_prompt_preset_rollback(
        &self,
        input: ApplyPromptPresetRollbackInput,
    ) -> ShellResult<PromptPresetRollbackReceiptDto> {
        validate_identifier("prompt_preset_id", &input.prompt_preset_id)?;
        validate_identifier("approval_id", &input.approval_id)?;
        validate_positive_revision("expected_current_revision", input.expected_current_revision)?;
        validate_positive_revision("target_revision", input.target_revision)?;
        validate_sha256("expected_review_sha256", &input.expected_review_sha256)?;

        let preset_id = PromptPresetId::from(input.prompt_preset_id);
        let review = self
            .core
            .review_prompt_preset_rollback(
                &preset_id,
                input.expected_current_revision,
                input.target_revision,
            )
            .map_err(ShellError::from)?;
        if review.review_sha256 != input.expected_review_sha256 {
            return Err(shell_invalid(
                "prompt preset rollback review changed before approval",
            ));
        }
        let review_sha256 = review.review_sha256.clone();
        let target_revision = review.target_revision;
        let receipt = self
            .core
            .apply_prompt_preset_rollback(&CorePromptPresetRollbackApplyRequest {
                review,
                approval_id: input.approval_id,
                expected_review_sha256: input.expected_review_sha256,
            })
            .map_err(ShellError::from)?;
        let applied_revision = receipt.preset.revision;
        let applied = self
            .core
            .list_prompt_preset_revisions(&preset_id)
            .map_err(ShellError::from)?
            .into_iter()
            .find(|revision| revision.revision == applied_revision)
            .ok_or_else(|| {
                shell_invalid("applied prompt preset revision is missing from immutable history")
            })?;
        if applied.object_id != preset_id.0 || applied.value.id != preset_id {
            return Err(shell_invalid(
                "applied prompt preset revision does not match the requested preset",
            ));
        }
        let result = PromptPresetRollbackReceiptDto {
            preset_id: preset_id.0,
            target_revision,
            applied_revision_id: applied.revision_id,
            applied_revision,
            applied_sha256: applied.sha256,
            review_sha256,
            approval_id: receipt.approval.approval_id,
            approval_sha256: receipt.approval.approval_sha256,
            approved_at: receipt.approval.approved_at,
        };
        validate_document(&result)?;
        Ok(result)
    }

    pub fn reorder_prompt_blocks(
        &self,
        input: ReorderPromptBlocksInput,
    ) -> ShellResult<ReorderPromptBlocksResultDto> {
        validate_identifier("prompt_preset_id", &input.prompt_preset_id)?;
        validate_identifier_list("ordered_block_ids", &input.ordered_block_ids)?;
        let stored = self
            .core
            .reorder_prompt_blocks(
                &PromptPresetId::from(input.prompt_preset_id),
                &input
                    .ordered_block_ids
                    .into_iter()
                    .map(PromptBlockId::from)
                    .collect::<Vec<_>>(),
                input.expected_revision,
            )
            .map_err(ShellError::from)?;
        let result = ReorderPromptBlocksResultDto {
            blocks: project_prompt_blocks(&stored.value)?,
            revision: stored.revision,
        };
        validate_document(&result)?;
        Ok(result)
    }

    fn delete_prompt_preset(
        &self,
        input: DeletePromptPresetInput,
    ) -> ShellResult<RevisionedDto<PromptPresetDto>> {
        validate_identifier("prompt_preset_id", &input.prompt_preset_id)?;
        self.core
            .delete_prompt_preset(
                &PromptPresetId::from(input.prompt_preset_id),
                input.expected_revision,
            )
            .map(Into::into)
            .map_err(ShellError::from)
            .and_then(validated_output)
    }

    pub fn delete_prompt_preset_summary(
        &self,
        input: DeletePromptPresetInput,
    ) -> ShellResult<RevisionedDto<PromptPresetSummaryDto>> {
        self.delete_prompt_preset(input)
            .map(|value| value.project(Into::into))
    }

    pub fn upsert_task_profile(
        &self,
        input: UpsertTaskProfileInput,
    ) -> ShellResult<RevisionedDto<TaskProfileDto>> {
        validate_identifier("task_profile_id", input.value.id.as_str())?;
        validate_document(&input.value)?;
        self.core
            .upsert_task_profile(&input.value, input.expected_revision)
            .map(Into::into)
            .map_err(ShellError::from)
            .and_then(validated_output)
    }

    pub fn get_task_profile(
        &self,
        input: GetTaskProfileInput,
    ) -> ShellResult<RevisionedDto<TaskProfileDto>> {
        validate_identifier("task_profile_id", &input.task_profile_id)?;
        self.core
            .get_task_profile(&TaskProfileId::from(input.task_profile_id))
            .map(Into::into)
            .map_err(ShellError::from)
            .and_then(validated_output)
    }

    pub fn list_task_profiles(&self) -> ShellResult<Vec<RevisionedDto<TaskProfileDto>>> {
        self.core
            .list_task_profiles()
            .and_then(bound_core_collection)
            .map(|values| values.into_iter().map(Into::into).collect())
            .map_err(ShellError::from)
            .and_then(validated_output)
    }

    pub fn delete_task_profile(
        &self,
        input: DeleteTaskProfileInput,
    ) -> ShellResult<RevisionedDto<TaskProfileDto>> {
        validate_identifier("task_profile_id", &input.task_profile_id)?;
        self.core
            .delete_task_profile(
                &TaskProfileId::from(input.task_profile_id),
                input.expected_revision,
            )
            .map(Into::into)
            .map_err(ShellError::from)
            .and_then(validated_output)
    }

    pub fn upsert_memory_profile(
        &self,
        input: UpsertMemoryProfileInput,
    ) -> ShellResult<RevisionedDto<MemoryProfileDto>> {
        validate_identifier("memory_profile_id", input.value.id.as_str())?;
        validate_document(&input.value)?;
        if let Some(expected_revision) = input.expected_revision {
            let current = self
                .core
                .get_memory_profile(&MemoryProfileId::from(input.value.id.clone()))
                .map_err(ShellError::from)?;
            require_creator_revision(
                current.revision,
                expected_revision,
                &current.value.provenance,
                "memory profile",
            )?;
        }
        let value = MemoryProfile::from(input.value);
        self.core
            .upsert_memory_profile(&value, input.expected_revision)
            .map(RevisionedDto::from)
            .map_err(ShellError::from)
            .and_then(|value| value.try_project(TryInto::try_into))
            .and_then(validated_output)
    }

    pub fn get_memory_profile(
        &self,
        input: GetMemoryProfileInput,
    ) -> ShellResult<RevisionedDto<MemoryProfileDto>> {
        validate_identifier("memory_profile_id", &input.memory_profile_id)?;
        self.core
            .get_memory_profile(&MemoryProfileId::from(input.memory_profile_id))
            .map(RevisionedDto::from)
            .map_err(ShellError::from)
            .and_then(|value| value.try_project(TryInto::try_into))
            .and_then(validated_output)
    }

    pub fn list_memory_profiles(&self) -> ShellResult<Vec<RevisionedDto<MemoryProfileDto>>> {
        let values = self
            .core
            .list_memory_profiles()
            .and_then(bound_core_collection)
            .map_err(ShellError::from)?;
        project_creator_revisions(
            values,
            |value| &value.provenance,
            CreatorMemoryProfileDocumentDto::try_from,
        )
    }

    pub fn delete_memory_profile(
        &self,
        input: DeleteMemoryProfileInput,
    ) -> ShellResult<RevisionedDto<MemoryProfileDto>> {
        validate_identifier("memory_profile_id", &input.memory_profile_id)?;
        let current = self
            .core
            .get_memory_profile(&MemoryProfileId::from(input.memory_profile_id.clone()))
            .map_err(ShellError::from)?;
        require_creator_revision(
            current.revision,
            input.expected_revision,
            &current.value.provenance,
            "memory profile",
        )?;
        self.core
            .delete_memory_profile(
                &MemoryProfileId::from(input.memory_profile_id),
                input.expected_revision,
            )
            .map(RevisionedDto::from)
            .map_err(ShellError::from)
            .and_then(|value| value.try_project(TryInto::try_into))
            .and_then(validated_output)
    }

    fn get_memory_record(
        &self,
        input: GetMemoryRecordInput,
    ) -> ShellResult<RevisionedDto<MemoryRecordDto>> {
        validate_identifier("conversation_id", &input.conversation_id)?;
        validate_identifier("branch_id", &input.branch_id)?;
        validate_identifier("memory_record_id", &input.memory_record_id)?;
        let conversation_id = lorepia_core::ConversationId(input.conversation_id);
        let branch_id = lorepia_core::ConversationBranchId(input.branch_id);
        self.core
            .get_memory_record(
                &conversation_id,
                &branch_id,
                &MemoryRecordId::from(input.memory_record_id),
            )
            .map(Into::into)
            .map_err(ShellError::from)
            .and_then(validated_output)
    }

    pub fn get_memory_record_projection(
        &self,
        input: GetMemoryRecordInput,
    ) -> ShellResult<MemoryRecordProjectionDto> {
        self.get_memory_record(input).map(Into::into)
    }

    pub fn patch_memory_record(
        &self,
        input: PatchMemoryRecordInput,
    ) -> ShellResult<MemoryRecordProjectionDto> {
        validate_identifier("conversation_id", &input.conversation_id)?;
        validate_identifier("branch_id", &input.branch_id)?;
        validate_identifier("memory_record_id", &input.memory_record_id)?;
        let conversation_id = lorepia_core::ConversationId(input.conversation_id);
        let branch_id = lorepia_core::ConversationBranchId(input.branch_id);
        let patch = MemoryRecordUserPatch {
            title: input.patch.title,
            summary: input.patch.summary,
            importance: input.patch.importance,
            keywords: input.patch.keywords,
            pinned: input.patch.pinned,
            excluded_from_conversation: None,
            excluded_from_character: None,
        };
        self.core
            .patch_memory_record_user_fields(
                &conversation_id,
                &branch_id,
                &MemoryRecordId::from(input.memory_record_id),
                input.expected_revision,
                &patch,
            )
            .map(RevisionedDto::from)
            .map(Into::into)
            .map_err(ShellError::from)
            .and_then(validated_output)
    }

    pub fn set_memory_record_exclusion(
        &self,
        input: SetMemoryRecordExclusionInput,
    ) -> ShellResult<MemoryRecordProjectionDto> {
        validate_identifier("conversation_id", &input.conversation_id)?;
        validate_identifier("branch_id", &input.branch_id)?;
        validate_identifier("memory_record_id", &input.memory_record_id)?;
        let conversation_id = lorepia_core::ConversationId(input.conversation_id);
        let branch_id = lorepia_core::ConversationBranchId(input.branch_id);
        self.core
            .set_memory_record_exclusion(
                &conversation_id,
                &branch_id,
                &MemoryRecordId::from(input.memory_record_id),
                input.expected_revision,
                input.scope.into(),
                input.excluded,
            )
            .map(RevisionedDto::from)
            .map(Into::into)
            .map_err(ShellError::from)
            .and_then(validated_output)
    }

    pub fn delete_memory_record(&self, input: DeleteMemoryRecordInput) -> ShellResult<()> {
        validate_identifier("conversation_id", &input.conversation_id)?;
        validate_identifier("branch_id", &input.branch_id)?;
        validate_identifier("memory_record_id", &input.memory_record_id)?;
        let conversation_id = lorepia_core::ConversationId(input.conversation_id);
        let branch_id = lorepia_core::ConversationBranchId(input.branch_id);
        self.core
            .delete_memory_record(
                &conversation_id,
                &branch_id,
                &MemoryRecordId::from(input.memory_record_id),
                input.expected_revision,
            )
            .map(|_| ())
            .map_err(ShellError::from)
    }

    /// Requeues exactly one interrupted summary or embedding job after the
    /// caller explicitly acknowledges that the prior provider outcome may be
    /// unknown. The expected revision prevents duplicate or stale retries.
    pub fn retry_interrupted_memory_job(
        &self,
        input: RetryInterruptedMemoryJobInput,
    ) -> ShellResult<MemoryJobRetryReceiptDto> {
        validate_identifier("conversation_id", &input.conversation_id)?;
        validate_identifier("branch_id", &input.branch_id)?;
        validate_identifier("memory_job_id", &input.memory_job_id)?;
        if !input.acknowledge_unknown_outcome {
            return Err(ShellError::from(CoreError::new(
                CoreErrorCode::PermissionDenied,
                "unknown provider outcome must be acknowledged before retrying a memory job",
                true,
            )));
        }
        self.core
            .retry_interrupted_memory_job(
                &lorepia_core::ConversationId(input.conversation_id),
                &lorepia_core::ConversationBranchId(input.branch_id),
                &MemoryJobId::from(input.memory_job_id),
                input.expected_revision,
            )
            .map_err(ShellError::from)
            .and_then(TryInto::try_into)
            .and_then(validated_output)
    }

    pub fn list_interrupted_memory_jobs(
        &self,
        input: ListInterruptedMemoryJobsInput,
    ) -> ShellResult<Vec<InterruptedMemoryJobDto>> {
        validate_identifier("conversation_id", &input.conversation_id)?;
        validate_identifier("branch_id", &input.branch_id)?;
        self.core
            .list_interrupted_memory_jobs(
                &lorepia_core::ConversationId(input.conversation_id),
                &lorepia_core::ConversationBranchId(input.branch_id),
                input.limit,
            )
            .map_err(ShellError::from)?
            .into_iter()
            .map(TryInto::try_into)
            .collect::<ShellResult<Vec<_>>>()
            .and_then(validated_output)
    }

    pub fn list_retryable_memory_query_embeddings(
        &self,
        input: ListRetryableMemoryQueryEmbeddingsInput,
    ) -> ShellResult<Vec<MemoryQueryEmbeddingRetryCandidateDto>> {
        validate_identifier("conversation_id", &input.conversation_id)?;
        validate_identifier("branch_id", &input.branch_id)?;
        self.core
            .list_retryable_memory_query_embeddings(
                &lorepia_core::ConversationId(input.conversation_id),
                &lorepia_core::ConversationBranchId(input.branch_id),
                input.limit,
            )
            .map_err(ShellError::from)?
            .into_iter()
            .map(TryInto::try_into)
            .collect::<ShellResult<Vec<_>>>()
            .and_then(validated_output)
    }

    pub fn retry_memory_query_embedding(
        &self,
        input: RetryMemoryQueryEmbeddingInput,
    ) -> ShellResult<MemoryQueryEmbeddingRetryCandidateDto> {
        validate_identifier("conversation_id", &input.conversation_id)?;
        validate_identifier("branch_id", &input.branch_id)?;
        validate_identifier("memory_query_embedding_id", &input.id)?;
        self.core
            .retry_memory_query_embedding(
                &lorepia_core::ConversationId(input.conversation_id),
                &lorepia_core::ConversationBranchId(input.branch_id),
                &input.id,
                input.expected_revision,
                input.acknowledge_unknown_outcome,
            )
            .map_err(ShellError::from)
            .and_then(TryInto::try_into)
            .and_then(validated_output)
    }

    pub fn upsert_knowledge_book(
        &self,
        input: UpsertKnowledgeBookInput,
    ) -> ShellResult<RevisionedDto<KnowledgeBookDto>> {
        validate_identifier("knowledge_book_id", input.value.id.as_str())?;
        validate_document(&input.value)?;
        if let Some(expected_revision) = input.expected_revision {
            let current = self
                .core
                .get_knowledge_book(&KnowledgeBookId::from(input.value.id.clone()))
                .map_err(ShellError::from)?;
            require_creator_revision(
                current.revision,
                expected_revision,
                &current.value.provenance,
                "knowledge book",
            )?;
        }
        let value = KnowledgeBook::from(input.value);
        self.core
            .upsert_knowledge_book(&value, input.expected_revision)
            .map(RevisionedDto::from)
            .map_err(ShellError::from)
            .and_then(|value| value.try_project(TryInto::try_into))
            .and_then(validated_output)
    }

    pub fn get_knowledge_book(
        &self,
        input: GetKnowledgeBookInput,
    ) -> ShellResult<RevisionedDto<KnowledgeBookDto>> {
        validate_identifier("knowledge_book_id", &input.knowledge_book_id)?;
        self.core
            .get_knowledge_book(&KnowledgeBookId::from(input.knowledge_book_id))
            .map(RevisionedDto::from)
            .map_err(ShellError::from)
            .and_then(|value| value.try_project(TryInto::try_into))
            .and_then(validated_output)
    }

    pub fn list_knowledge_books(&self) -> ShellResult<Vec<RevisionedDto<KnowledgeBookDto>>> {
        let values = self
            .core
            .list_knowledge_books()
            .and_then(bound_core_collection)
            .map_err(ShellError::from)?;
        project_creator_revisions(
            values,
            |value| &value.provenance,
            CreatorKnowledgeBookDocumentDto::try_from,
        )
    }

    pub fn delete_knowledge_book(
        &self,
        input: DeleteKnowledgeBookInput,
    ) -> ShellResult<RevisionedDto<KnowledgeBookDto>> {
        validate_identifier("knowledge_book_id", &input.knowledge_book_id)?;
        let current = self
            .core
            .get_knowledge_book(&KnowledgeBookId::from(input.knowledge_book_id.clone()))
            .map_err(ShellError::from)?;
        require_creator_revision(
            current.revision,
            input.expected_revision,
            &current.value.provenance,
            "knowledge book",
        )?;
        self.core
            .delete_knowledge_book(
                &KnowledgeBookId::from(input.knowledge_book_id),
                input.expected_revision,
            )
            .map(RevisionedDto::from)
            .map_err(ShellError::from)
            .and_then(|value| value.try_project(TryInto::try_into))
            .and_then(validated_output)
    }

    pub fn upsert_transform_set(
        &self,
        input: UpsertTransformSetInput,
    ) -> ShellResult<RevisionedDto<TransformSetDto>> {
        validate_identifier("transform_set_id", input.value.id.as_str())?;
        validate_document(&input.value)?;
        if let Some(expected_revision) = input.expected_revision {
            let current = self
                .core
                .get_transform_set(&TransformSetId::from(input.value.id.clone()))
                .map_err(ShellError::from)?;
            require_creator_revision(
                current.revision,
                expected_revision,
                &current.value.provenance,
                "transform set",
            )?;
        }
        let value = TransformSet::from(input.value);
        self.core
            .upsert_transform_set(&value, input.expected_revision)
            .map(RevisionedDto::from)
            .map_err(ShellError::from)
            .and_then(|value| value.try_project(TryInto::try_into))
            .and_then(validated_output)
    }

    pub fn get_transform_set(
        &self,
        input: GetTransformSetInput,
    ) -> ShellResult<RevisionedDto<TransformSetDto>> {
        validate_identifier("transform_set_id", &input.transform_set_id)?;
        self.core
            .get_transform_set(&TransformSetId::from(input.transform_set_id))
            .map(RevisionedDto::from)
            .map_err(ShellError::from)
            .and_then(|value| value.try_project(TryInto::try_into))
            .and_then(validated_output)
    }

    pub fn list_transform_sets(&self) -> ShellResult<Vec<RevisionedDto<TransformSetDto>>> {
        let values = self
            .core
            .list_transform_sets()
            .and_then(bound_core_collection)
            .map_err(ShellError::from)?;
        project_creator_revisions(
            values,
            |value| &value.provenance,
            CreatorTransformSetDocumentDto::try_from,
        )
    }

    pub fn delete_transform_set(
        &self,
        input: DeleteTransformSetInput,
    ) -> ShellResult<RevisionedDto<TransformSetDto>> {
        validate_identifier("transform_set_id", &input.transform_set_id)?;
        let current = self
            .core
            .get_transform_set(&TransformSetId::from(input.transform_set_id.clone()))
            .map_err(ShellError::from)?;
        require_creator_revision(
            current.revision,
            input.expected_revision,
            &current.value.provenance,
            "transform set",
        )?;
        self.core
            .delete_transform_set(
                &TransformSetId::from(input.transform_set_id),
                input.expected_revision,
            )
            .map(RevisionedDto::from)
            .map_err(ShellError::from)
            .and_then(|value| value.try_project(TryInto::try_into))
            .and_then(validated_output)
    }

    pub fn upsert_interaction_rule_set(
        &self,
        input: UpsertInteractionRuleSetInput,
    ) -> ShellResult<RevisionedDto<InteractionRuleSetDto>> {
        validate_identifier("interaction_rule_set_id", input.value.id.as_str())?;
        validate_document(&input.value)?;
        if let Some(expected_revision) = input.expected_revision {
            let current = self
                .core
                .get_interaction_rule_set(&InteractionRuleSetId::from(input.value.id.clone()))
                .map_err(ShellError::from)?;
            require_creator_revision(
                current.revision,
                expected_revision,
                &current.value.provenance,
                "interaction rule set",
            )?;
        }
        let value = InteractionRuleSet::from(input.value);
        self.core
            .upsert_interaction_rule_set(&value, input.expected_revision)
            .map(RevisionedDto::from)
            .map_err(ShellError::from)
            .and_then(|value| value.try_project(TryInto::try_into))
            .and_then(validated_output)
    }

    pub fn get_interaction_rule_set(
        &self,
        input: GetInteractionRuleSetInput,
    ) -> ShellResult<RevisionedDto<InteractionRuleSetDto>> {
        validate_identifier("interaction_rule_set_id", &input.interaction_rule_set_id)?;
        self.core
            .get_interaction_rule_set(&InteractionRuleSetId::from(input.interaction_rule_set_id))
            .map(RevisionedDto::from)
            .map_err(ShellError::from)
            .and_then(|value| value.try_project(TryInto::try_into))
            .and_then(validated_output)
    }

    pub fn list_interaction_rule_sets(
        &self,
    ) -> ShellResult<Vec<RevisionedDto<InteractionRuleSetDto>>> {
        let values = self
            .core
            .list_interaction_rule_sets()
            .and_then(bound_core_collection)
            .map_err(ShellError::from)?;
        project_creator_revisions(
            values,
            |value| &value.provenance,
            CreatorInteractionRuleSetDocumentDto::try_from,
        )
    }

    pub fn delete_interaction_rule_set(
        &self,
        input: DeleteInteractionRuleSetInput,
    ) -> ShellResult<RevisionedDto<InteractionRuleSetDto>> {
        validate_identifier("interaction_rule_set_id", &input.interaction_rule_set_id)?;
        let current = self
            .core
            .get_interaction_rule_set(&InteractionRuleSetId::from(
                input.interaction_rule_set_id.clone(),
            ))
            .map_err(ShellError::from)?;
        require_creator_revision(
            current.revision,
            input.expected_revision,
            &current.value.provenance,
            "interaction rule set",
        )?;
        self.core
            .delete_interaction_rule_set(
                &InteractionRuleSetId::from(input.interaction_rule_set_id),
                input.expected_revision,
            )
            .map(RevisionedDto::from)
            .map_err(ShellError::from)
            .and_then(|value| value.try_project(TryInto::try_into))
            .and_then(validated_output)
    }

    pub fn upsert_content_module(
        &self,
        input: UpsertContentModuleInput,
    ) -> ShellResult<RevisionedDto<ContentModuleDto>> {
        validate_identifier("content_module_id", input.value.id.as_str())?;
        validate_document(&input.value)?;
        validate_creator_content_module_input(&input.value)?;
        if let Some(expected_revision) = input.expected_revision {
            let current = self
                .core
                .get_content_module(&ContentModuleId::from(input.value.id.clone()))
                .map_err(ShellError::from)?;
            require_creator_revision(
                current.revision,
                expected_revision,
                &current.value.metadata.provenance,
                "content module",
            )?;
        }
        let value = ContentModule::from(input.value);
        self.core
            .upsert_content_module(&value, input.expected_revision)
            .map(RevisionedDto::from)
            .map_err(ShellError::from)
            .and_then(|value| value.try_project(TryInto::try_into))
            .and_then(validated_output)
    }

    pub fn get_content_module(
        &self,
        input: GetContentModuleInput,
    ) -> ShellResult<RevisionedDto<ContentModuleDto>> {
        validate_identifier("content_module_id", &input.content_module_id)?;
        self.core
            .get_content_module(&ContentModuleId::from(input.content_module_id))
            .map(RevisionedDto::from)
            .map_err(ShellError::from)
            .and_then(|value| value.try_project(TryInto::try_into))
            .and_then(validated_output)
    }

    pub fn list_content_modules(&self) -> ShellResult<Vec<RevisionedDto<ContentModuleDto>>> {
        let values = self
            .core
            .list_content_modules()
            .and_then(bound_core_collection)
            .map_err(ShellError::from)?;
        project_creator_revisions(
            values,
            |value| &value.metadata.provenance,
            CreatorContentModuleDocumentDto::try_from,
        )
    }

    pub fn delete_content_module(
        &self,
        input: DeleteContentModuleInput,
    ) -> ShellResult<RevisionedDto<ContentModuleDto>> {
        validate_identifier("content_module_id", &input.content_module_id)?;
        let current = self
            .core
            .get_content_module(&ContentModuleId::from(input.content_module_id.clone()))
            .map_err(ShellError::from)?;
        require_creator_revision(
            current.revision,
            input.expected_revision,
            &current.value.metadata.provenance,
            "content module",
        )?;
        self.core
            .delete_content_module(
                &ContentModuleId::from(input.content_module_id),
                input.expected_revision,
            )
            .map(RevisionedDto::from)
            .map_err(ShellError::from)
            .and_then(|value| value.try_project(TryInto::try_into))
            .and_then(validated_output)
    }

    pub fn list_prompt_preset_bindings(
        &self,
        input: ListPromptPresetBindingsInput,
    ) -> ShellResult<Vec<RevisionedDto<PromptPresetBindingDto>>> {
        validate_optional_identifier("target_id", input.target_id.as_deref())?;
        self.core
            .list_prompt_preset_bindings(input.scope, input.target_id.as_deref())
            .and_then(bound_core_collection)
            .map(|values| values.into_iter().map(Into::into).collect())
            .map_err(ShellError::from)
            .and_then(validated_output)
    }

    pub fn list_memory_records(
        &self,
        input: ListMemoryRecordsInput,
    ) -> ShellResult<MemoryRecordListDto> {
        validate_identifier("conversation_id", &input.conversation_id)?;
        validate_identifier("branch_id", &input.branch_id)?;
        let values = self
            .core
            .list_memory_records(
                &lorepia_core::ConversationId(input.conversation_id),
                &lorepia_core::ConversationBranchId(input.branch_id),
                input.include_invalidated,
            )
            .map_err(ShellError::from)?;
        let truncated = values.len() > MAX_MEMORY_LIST_ITEMS;
        let result = MemoryRecordListDto {
            records: values
                .into_iter()
                .take(MAX_MEMORY_LIST_ITEMS)
                .map(RevisionedDto::from)
                .map(Into::into)
                .collect(),
            truncated,
        };
        validate_document(&result)?;
        Ok(result)
    }

    pub fn simulate_knowledge(
        &self,
        input: SimulateKnowledgeInput,
    ) -> ShellResult<KnowledgeSimulationDto> {
        validate_identifier("knowledge_book_id", &input.knowledge_book_id)?;
        validate_preview_texts(&input.sample_texts)?;
        validate_identifier_list("manual_entry_ids", &input.manual_entry_ids)?;
        if input.semantic_scores.len() > MAX_COLLECTION_ITEMS
            || input.supported_capabilities.len() > MAX_COLLECTION_ITEMS
            || input.token_estimates.len() > MAX_COLLECTION_ITEMS
        {
            return Err(shell_invalid(
                "knowledge simulation inputs exceed the item limit",
            ));
        }
        let token_estimates = input
            .token_estimates
            .into_iter()
            .map(|estimate| {
                validate_identifier("knowledge_entry_id", &estimate.knowledge_entry_id)?;
                Ok(KnowledgeTokenEstimate {
                    entry_id: KnowledgeEntryId::from(estimate.knowledge_entry_id),
                    tokens: estimate.tokens,
                })
            })
            .collect::<ShellResult<Vec<_>>>()?;
        let request = KnowledgeSimulationRequest {
            book_id: KnowledgeBookId::from(input.knowledge_book_id),
            sample_texts: input.sample_texts,
            manual_entry_ids: input
                .manual_entry_ids
                .into_iter()
                .map(KnowledgeEntryId::from)
                .collect(),
            semantic_scores: input.semantic_scores,
            variables: input.variables,
            supported_capabilities: input.supported_capabilities,
            token_estimates,
            activation_seed: input.activation_seed,
        };
        validate_document(&request)?;
        let result = self
            .core
            .simulate_knowledge_activation(&request)
            .map(Into::into)
            .map_err(ShellError::from)?;
        validate_document(&result)?;
        Ok(result)
    }

    pub fn preview_transform_rule(
        &self,
        input: PreviewTransformRuleInput,
    ) -> ShellResult<TransformRulePreviewDto> {
        validate_identifier("transform_set_id", &input.transform_set_id)?;
        validate_identifier("transform_rule_id", &input.transform_rule_id)?;
        validate_preview_texts(std::slice::from_ref(&input.sample_text))?;
        if input.supported_capabilities.len() > MAX_COLLECTION_ITEMS
            || input.approved_import_source_ids.len() > MAX_COLLECTION_ITEMS
        {
            return Err(shell_invalid(
                "transform preview authority inputs exceed the item limit",
            ));
        }
        validate_identifier_list(
            "approved_import_source_ids",
            &input.approved_import_source_ids,
        )?;
        let result = self
            .core
            .preview_transform(&TransformPreviewRequest {
                transform_set_id: TransformSetId::from(input.transform_set_id),
                rule_id: TransformRuleId::from(input.transform_rule_id),
                input: input.sample_text,
                variables: input.variables,
                supported_capabilities: input.supported_capabilities,
                approved_import_source_ids: input.approved_import_source_ids,
                allow_resolved_prompt: input.allow_resolved_prompt,
            })
            .map_err(ShellError::from)?;
        let (original, original_truncated) = bounded_utf8(result.original, MAX_PREVIEW_TEXT_BYTES);
        let (output, output_truncated) = bounded_utf8(result.output, MAX_PREVIEW_TEXT_BYTES);
        let reports_truncated = result.reports.len() > MAX_SELECTION_ITEMS;
        let projected = TransformRulePreviewDto {
            phase: result.phase,
            original,
            output,
            changed: result.changed,
            rendering: "native_plain_text".to_owned(),
            reports: result
                .reports
                .into_iter()
                .take(MAX_SELECTION_ITEMS)
                .collect(),
            diff: result.diff,
            error: result.error,
            truncated: original_truncated || output_truncated || reports_truncated,
        };
        validate_document(&projected)?;
        Ok(projected)
    }

    pub fn list_content_module_bindings(
        &self,
        input: ListContentModuleBindingsInput,
    ) -> ShellResult<Vec<RevisionedDto<ModuleBindingDto>>> {
        validate_identifier("content_module_id", &input.content_module_id)?;
        self.core
            .list_content_module_bindings(&ContentModuleId::from(input.content_module_id))
            .and_then(bound_core_collection)
            .map(|values| values.into_iter().map(Into::into).collect())
            .map_err(ShellError::from)
            .and_then(validated_output)
    }

    pub fn list_content_module_revisions(
        &self,
        input: ListContentModuleRevisionsInput,
    ) -> ShellResult<ContentModuleRevisionListDto> {
        validate_identifier("content_module_id", &input.content_module_id)?;
        let result = self
            .core
            .list_content_module_revisions(&ContentModuleId::from(input.content_module_id))
            .map(|values| {
                let truncated = values.len() > MAX_MODULE_REVISIONS;
                ContentModuleRevisionListDto {
                    revisions: values
                        .into_iter()
                        .take(MAX_MODULE_REVISIONS)
                        .map(Into::into)
                        .collect(),
                    truncated,
                }
            })
            .map_err(ShellError::from)?;
        validate_document(&result)?;
        Ok(result)
    }

    pub fn diff_content_module_revisions(
        &self,
        input: DiffContentModuleRevisionsInput,
    ) -> ShellResult<ContentModuleRevisionDiffDto> {
        validate_identifier("content_module_id", &input.content_module_id)?;
        let result = self
            .core
            .diff_content_module_revisions(
                &ContentModuleId::from(input.content_module_id),
                input.from_revision,
                input.to_revision,
            )
            .map(|value| {
                let truncated = value.changed_paths.len() > MAX_DIFF_PATHS;
                ContentModuleRevisionDiffDto {
                    content_module_id: value.module_id.0,
                    from_revision: value.from_revision,
                    to_revision: value.to_revision,
                    from_sha256: value.from_sha256,
                    to_sha256: value.to_sha256,
                    changed_paths: value
                        .changed_paths
                        .into_iter()
                        .take(MAX_DIFF_PATHS)
                        .collect(),
                    truncated,
                }
            })
            .map_err(ShellError::from)?;
        validate_document(&result)?;
        Ok(result)
    }

    pub fn evaluate_content_module_share(
        &self,
        input: EvaluateContentModuleShareInput,
    ) -> ShellResult<ContentShareGateDto> {
        validate_identifier("content_module_id", &input.content_module_id)?;
        let result = self
            .core
            .evaluate_content_module_share_gate(&ContentModuleId::from(input.content_module_id))
            .map_err(ShellError::from)?;
        validate_document(&result)?;
        Ok(result)
    }
}

fn shell_invalid(message: impl Into<String>) -> ShellError {
    ShellError::from(CoreError::invalid(message))
}

fn user_created_provenance() -> Provenance {
    Provenance {
        source_kind: SourceKind::UserCreated,
        source_id: None,
        source_hash: None,
        author: None,
        license: None,
        imported_at: None,
    }
}

fn require_user_created_provenance(
    provenance: &Provenance,
    document_label: &str,
) -> ShellResult<()> {
    if provenance.source_kind == SourceKind::UserCreated {
        Ok(())
    } else {
        Err(shell_invalid(format!(
            "{document_label} is read-only at the creator boundary"
        )))
    }
}

fn require_creator_revision(
    actual_revision: u64,
    expected_revision: u64,
    provenance: &Provenance,
    document_label: &str,
) -> ShellResult<()> {
    require_user_created_provenance(provenance, document_label)?;
    if actual_revision == expected_revision {
        Ok(())
    } else {
        Err(shell_invalid(format!(
            "{document_label} changed before the requested mutation"
        )))
    }
}

fn project_creator_revisions<T, U>(
    values: Vec<Revisioned<T>>,
    provenance: impl Fn(&T) -> &Provenance,
    project: impl Fn(T) -> ShellResult<U>,
) -> ShellResult<Vec<RevisionedDto<U>>>
where
    U: Serialize,
{
    let mut projected = Vec::new();
    for value in values {
        if provenance(&value.value).source_kind != SourceKind::UserCreated {
            continue;
        }
        if projected.len() == MAX_CREATOR_DOCUMENTS {
            break;
        }
        projected.push(RevisionedDto::from(value).try_project(&project)?);
    }
    validated_output(projected)
}

fn validate_creator_content_module_input(
    module: &CreatorContentModuleDocumentDto,
) -> ShellResult<()> {
    if !module.asset_ids.is_empty() {
        return Err(shell_invalid(
            "creator module assets require Rust-derived asset capabilities",
        ));
    }
    for (has_component, capability) in [
        (
            !module.prompt_fragments.is_empty(),
            ContentCapability::PromptFragments,
        ),
        (
            !module.knowledge_book_ids.is_empty(),
            ContentCapability::Knowledge,
        ),
        (
            !module.control_specs.is_empty(),
            ContentCapability::Variables,
        ),
        (
            !module.transform_set_ids.is_empty(),
            ContentCapability::Transforms,
        ),
        (
            !module.interaction_rule_set_ids.is_empty(),
            ContentCapability::DeclarativeInteractions,
        ),
    ] {
        if has_component && !module.required_capabilities.contains(&capability) {
            return Err(shell_invalid(format!(
                "creator module must declare the {capability:?} capability for its components"
            )));
        }
    }
    Ok(())
}

fn validate_creator_prompt_preset_input(preset: &PromptPreset) -> ShellResult<()> {
    if preset.blocks.iter().any(|block| {
        block.authority == InstructionAuthority::Application
            || block.placement_zone == PlacementZone::ApplicationPolicy
    }) {
        return Err(shell_invalid(
            "application policy blocks are Core-owned and cannot be submitted by the frontend",
        ));
    }
    if preset.metadata.provenance.source_kind == SourceKind::ApplicationBuiltIn {
        return Err(shell_invalid(
            "application-built-in provenance cannot be submitted by the frontend",
        ));
    }
    Ok(())
}

struct PromptPlanRequestParts {
    conversation_id: String,
    branch_id: String,
    expected_head: Option<String>,
    user_text: String,
    generation_target: GenerationTargetDto,
    prompt_preset_id: Option<String>,
    variable_overrides: VariableMap,
    expected_plan_hash: Option<String>,
}

fn prompt_plan_request(input: PromptPlanRequestParts) -> ShellResult<PromptPlanRequest> {
    let PromptPlanRequestParts {
        conversation_id,
        branch_id,
        expected_head,
        user_text,
        generation_target,
        prompt_preset_id,
        variable_overrides,
        expected_plan_hash,
    } = input;
    validate_identifier("conversation_id", &conversation_id)?;
    validate_identifier("branch_id", &branch_id)?;
    validate_optional_identifier("expected_head", expected_head.as_deref())?;
    validate_identifier("model_route_id", &generation_target.model_route_id)?;
    validate_identifier(
        "generation_preset_id",
        &generation_target.generation_preset_id,
    )?;
    validate_optional_identifier("prompt_preset_id", prompt_preset_id.as_deref())?;
    if let Some(plan_hash) = expected_plan_hash.as_deref() {
        validate_sha256("expected_plan_hash", plan_hash)?;
    }
    let request = PromptPlanRequest {
        conversation_id: lorepia_core::ConversationId(conversation_id),
        branch_id: lorepia_core::ConversationBranchId(branch_id),
        expected_head: expected_head.map(MessageId),
        user_text,
        generation_target: generation_target.into(),
        prompt_preset_id: prompt_preset_id.map(PromptPresetId::from),
        variable_overrides,
        expected_plan_hash,
    };
    validate_document(&request)?;
    Ok(request)
}

fn validate_sha256(field: &str, value: &str) -> ShellResult<()> {
    if value.len() != 64
        || value.bytes().any(|byte| byte.is_ascii_uppercase())
        || value.bytes().any(|byte| !byte.is_ascii_hexdigit())
    {
        return Err(shell_invalid(format!(
            "{field} must be a lowercase 64-character SHA-256 digest"
        )));
    }
    Ok(())
}

fn validate_positive_revision(field: &str, value: u64) -> ShellResult<()> {
    if value == 0 {
        return Err(shell_invalid(format!("{field} must be greater than zero")));
    }
    Ok(())
}

fn validate_optional_identifier(field: &str, value: Option<&str>) -> ShellResult<()> {
    value.map_or(Ok(()), |value| validate_identifier(field, value))
}

fn validate_identifier_list(field: &str, values: &[String]) -> ShellResult<()> {
    if values.len() > MAX_COLLECTION_ITEMS {
        return Err(shell_invalid(format!("{field} exceeds the item limit")));
    }
    for value in values {
        validate_identifier(field, value)?;
    }
    Ok(())
}

fn validate_preview_texts(values: &[String]) -> ShellResult<()> {
    if values.len() > MAX_SELECTION_ITEMS {
        return Err(shell_invalid(
            "knowledge sample texts exceed the item limit",
        ));
    }
    let total_bytes = values
        .iter()
        .try_fold(0_usize, |total, value| total.checked_add(value.len()))
        .ok_or_else(|| shell_invalid("knowledge sample text size overflow"))?;
    if total_bytes > MAX_PREVIEW_TEXT_BYTES {
        return Err(shell_invalid(
            "knowledge sample texts exceed the preview byte limit",
        ));
    }
    Ok(())
}

fn bounded_prompt_plan_preview(value: PromptPlanPreview) -> ShellResult<PromptPlanPreviewDto> {
    let generation_target = value.generation_target.ok_or_else(|| {
        ShellError::from(CoreError::internal(
            "resolved prompt preview omitted its required generation target",
        ))
    })?;
    let truncated = value.messages.len() > MAX_PROMPT_PREVIEW_ITEMS
        || value.provider_messages.len() > MAX_PROMPT_PREVIEW_ITEMS
        || value.provider_cache_boundaries.len() > MAX_PROMPT_PREVIEW_ITEMS
        || value.cache_directives.len() > MAX_PROMPT_PREVIEW_ITEMS
        || value.blocks.len() > MAX_PROMPT_PREVIEW_ITEMS
        || value.role_mappings.len() > MAX_PROMPT_PREVIEW_ITEMS
        || value.overflow.len() > MAX_PROMPT_PREVIEW_ITEMS
        || value.warnings.len() > MAX_PROMPT_WARNINGS
        || value
            .messages
            .iter()
            .any(|message| message.source_message_ids.len() > MAX_PROMPT_PREVIEW_ITEMS);
    Ok(PromptPlanPreviewDto {
        plan_id: value.plan_id,
        plan_hash: value.plan_hash,
        prompt_preset_id: value.prompt_preset_id.0,
        prompt_preset_revision: value.prompt_preset_revision,
        generation_target: GenerationTargetDto {
            model_route_id: generation_target.model_route_id.0,
            generation_preset_id: generation_target.generation_preset_id.0,
        },
        estimated_input_tokens: value.estimated_input_tokens,
        available_input_tokens: value.available_input_tokens,
        token_estimator_id: value.token_estimator_id,
        token_estimate_exact: value.token_estimate_exact,
        messages: value
            .messages
            .into_iter()
            .take(MAX_PROMPT_PREVIEW_ITEMS)
            .map(Into::into)
            .collect(),
        provider_family: value.provider_family,
        provider_messages: value
            .provider_messages
            .into_iter()
            .take(MAX_PROMPT_PREVIEW_ITEMS)
            .map(Into::into)
            .collect(),
        provider_cache_boundaries: value
            .provider_cache_boundaries
            .into_iter()
            .take(MAX_PROMPT_PREVIEW_ITEMS)
            .collect(),
        cache_directives: value
            .cache_directives
            .into_iter()
            .take(MAX_PROMPT_PREVIEW_ITEMS)
            .map(Into::into)
            .collect(),
        blocks: value
            .blocks
            .into_iter()
            .take(MAX_PROMPT_PREVIEW_ITEMS)
            .map(Into::into)
            .collect(),
        role_mappings: value
            .role_mappings
            .into_iter()
            .take(MAX_PROMPT_PREVIEW_ITEMS)
            .map(Into::into)
            .collect(),
        overflow: value
            .overflow
            .into_iter()
            .take(MAX_PROMPT_PREVIEW_ITEMS)
            .map(Into::into)
            .collect(),
        warnings: value
            .warnings
            .into_iter()
            .take(MAX_PROMPT_WARNINGS)
            .map(|warning| PromptWarningCodeDto::from_message(&warning))
            .collect(),
        truncated,
    })
}

fn bounded_expert_prompt_preview(
    value: ExpertPromptPreview,
) -> ShellResult<ExpertPromptPreviewDto> {
    if value.effective_messages.len() > MAX_PROMPT_PREVIEW_ITEMS
        || value.applied_parameters.len() > MAX_PROMPT_PREVIEW_ITEMS
        || value.prompt_diff.len() > MAX_PROMPT_PREVIEW_ITEMS
        || value
            .effective_messages
            .iter()
            .any(|message| message.source_message_ids.len() > MAX_PROMPT_PREVIEW_ITEMS)
    {
        return Err(shell_invalid(
            "expert prompt preview exceeds the collection limit",
        ));
    }
    let prompt_diff = project_prompt_diff(&value.plan, value.prompt_diff)?;
    let applied_parameters = value
        .applied_parameters
        .into_iter()
        .map(project_applied_parameter)
        .collect::<ShellResult<Vec<_>>>()?;
    Ok(ExpertPromptPreviewDto {
        generation_attempt_id: value.generation_attempt_id.0,
        plan: bounded_prompt_plan_preview(value.plan)?,
        applied_parameters,
        prompt_diff,
    })
}

fn project_applied_parameter(
    parameter: lorepia_core::PromptAppliedParameterPreview,
) -> ShellResult<PromptAppliedParameterPreviewDto> {
    if parameter.field.is_empty()
        || parameter.field.len() > 256
        || !parameter
            .field
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-' | b'.'))
    {
        return Err(ShellError::from(CoreError::internal(
            "Core produced a non-structural applied-parameter field",
        )));
    }
    let (value_kind, item_count) = match &parameter.value {
        serde_json::Value::Null => (PromptAppliedParameterValueKindDto::Null, None),
        serde_json::Value::Bool(_) => (PromptAppliedParameterValueKindDto::Boolean, None),
        serde_json::Value::Number(_) => (PromptAppliedParameterValueKindDto::Number, None),
        serde_json::Value::String(_) => (PromptAppliedParameterValueKindDto::String, None),
        serde_json::Value::Array(items) => (
            PromptAppliedParameterValueKindDto::Array,
            Some(u32::try_from(items.len()).map_err(|_| {
                ShellError::from(CoreError::internal(
                    "applied-parameter array count overflowed",
                ))
            })?),
        ),
        serde_json::Value::Object(fields) => (
            PromptAppliedParameterValueKindDto::Object,
            Some(u32::try_from(fields.len()).map_err(|_| {
                ShellError::from(CoreError::internal(
                    "applied-parameter object count overflowed",
                ))
            })?),
        ),
    };
    Ok(PromptAppliedParameterPreviewDto {
        field: parameter.field,
        value_kind,
        item_count,
    })
}

fn project_prompt_diff(
    plan: &PromptPlanPreview,
    entries: Vec<lorepia_core::PromptDiffEntry>,
) -> ShellResult<Vec<PromptDiffEntryDto>> {
    entries
        .into_iter()
        .map(|entry| {
            let message = plan
                .messages
                .iter()
                .find(|message| {
                    message.sequence == entry.sequence && message.block_id == entry.block_id
                })
                .ok_or_else(|| {
                    ShellError::from(CoreError::internal(
                        "prompt diff has no matching structural message",
                    ))
                })?;
            let provider_message = plan
                .provider_messages
                .iter()
                .find(|provider_message| {
                    provider_message.sequence == entry.sequence
                        && provider_message.block_id == entry.block_id
                })
                .ok_or_else(|| {
                    ShellError::from(CoreError::internal(
                        "prompt diff has no matching provider message",
                    ))
                })?;
            Ok(PromptDiffEntryDto {
                sequence: entry.sequence,
                block_id: entry.block_id.0,
                requested_role: message.requested_role,
                effective_role: message.effective_role,
                wire_role: provider_message.wire_role,
                placement: provider_message.placement,
            })
        })
        .collect()
}

fn room_generation_target_dto(room: &RoomOrchestrationConfig) -> Option<GenerationTargetDto> {
    room.generation_target
        .as_ref()
        .map(|target| GenerationTargetDto {
            model_route_id: target.model_route_id.0.clone(),
            generation_preset_id: target.generation_preset_id.0.clone(),
        })
}

fn validate_room_prompt_source_input(input: &SaveRoomOrchestrationConfigInput) -> ShellResult<()> {
    validate_room_prompt_optional_text(
        "user_name_override",
        input.user_name_override.as_deref(),
        MAX_NAME_CHARS,
        true,
    )?;
    validate_room_prompt_optional_text(
        "author_note",
        input.author_note.as_deref(),
        MAX_BLOCK_TEXT_CHARS,
        false,
    )?;
    validate_room_prompt_optional_text(
        "group_context",
        input.group_context.as_deref(),
        MAX_BLOCK_TEXT_CHARS,
        false,
    )?;
    if input.template_slots.len() > MAX_ROOM_TEMPLATE_SLOTS {
        return Err(shell_invalid(format!(
            "template_slots must contain at most {MAX_ROOM_TEMPLATE_SLOTS} values"
        )));
    }
    let mut names = Vec::with_capacity(input.template_slots.len());
    for slot in &input.template_slots {
        if slot.name.is_empty()
            || slot.name.chars().count() > MAX_NAME_CHARS
            || slot.name.trim() != slot.name
            || slot.name.chars().any(char::is_control)
            || slot.name == "block_content"
            || slot.value.chars().count() > MAX_BLOCK_TEXT_CHARS
            || slot.value.contains('\0')
        {
            return Err(shell_invalid(
                "template slot name or value violates the prompt-source bounds",
            ));
        }
        names.push(slot.name.as_str());
    }
    names.sort_unstable();
    if names.windows(2).any(|pair| pair[0] == pair[1]) {
        return Err(shell_invalid("template slot names must be unique"));
    }
    Ok(())
}

fn validate_room_prompt_optional_text(
    label: &str,
    value: Option<&str>,
    maximum_chars: usize,
    require_trimmed: bool,
) -> ShellResult<()> {
    let Some(value) = value else {
        return Ok(());
    };
    if value.is_empty()
        || value.chars().count() > maximum_chars
        || value.trim().is_empty()
        || value.contains('\0')
        || (require_trimmed && value.trim() != value)
    {
        return Err(shell_invalid(format!(
            "{label} is empty, oversized, invalidly padded, or contains NUL"
        )));
    }
    Ok(())
}

fn project_room_orchestration_config(room: RoomOrchestrationConfig) -> RoomOrchestrationConfigDto {
    RoomOrchestrationConfigDto {
        conversation_id: room.conversation_id.0,
        branch_id: room.branch_id.0,
        prompt_preset_id: Some(room.prompt_preset_id.0),
        generation_preset_id: room.generation_preset_id.map(|id| id.0),
        response_length: room.response_length,
        creativity: room.creativity,
        reasoning_effort: room.reasoning_effort.into(),
        memory_enabled: room.memory_enabled,
        knowledge_enabled: room.knowledge_enabled,
        creator_values: room.creator_values,
        variable_overrides: room.variable_overrides,
        user_name_override: room.user_name_override,
        author_note: room.author_note,
        group_context: room.group_context,
        template_slots: room
            .template_slots
            .into_iter()
            .map(|slot| RoomPromptTemplateSlotDto {
                name: slot.name,
                value: slot.value,
            })
            .collect(),
        supported_fields: RoomOrchestrationSupportedFieldsDto {
            prompt_preset_id: RoomOrchestrationFieldSupportDto::SUPPORTED,
            generation_preset_id: RoomOrchestrationFieldSupportDto::SUPPORTED,
            creator_values: RoomOrchestrationFieldSupportDto::SUPPORTED,
            variable_overrides: RoomOrchestrationFieldSupportDto::UNSUPPORTED,
            response_length: RoomOrchestrationFieldSupportDto::SUPPORTED,
            creativity: RoomOrchestrationFieldSupportDto::SUPPORTED,
            reasoning_effort: RoomOrchestrationFieldSupportDto::SUPPORTED,
            memory_enabled: RoomOrchestrationFieldSupportDto::SUPPORTED,
            knowledge_enabled: RoomOrchestrationFieldSupportDto::SUPPORTED,
            user_name_override: RoomOrchestrationFieldSupportDto::SUPPORTED,
            author_note: RoomOrchestrationFieldSupportDto::SUPPORTED,
            group_context: RoomOrchestrationFieldSupportDto::SUPPORTED,
            template_slots: RoomOrchestrationFieldSupportDto::SUPPORTED,
        },
    }
}

fn project_creator_controls(
    controls: &[ControlSpec],
    values: &BTreeMap<String, CoreCreatorControlValue>,
) -> ShellResult<Vec<CreatorControlProjectionDto>> {
    if controls.len() > MAX_SELECTION_ITEMS {
        return Err(shell_invalid(
            "prompt preset exceeds the creator control projection limit",
        ));
    }
    controls
        .iter()
        .filter(|control| {
            !control.sensitive
                && !matches!(
                    control.kind,
                    ControlKind::Section | ControlKind::Caption | ControlKind::Divider
                )
        })
        .map(|control| {
            let value = values
                .get(control.id.as_str())
                .cloned()
                .or_else(|| {
                    control
                        .default_value
                        .as_ref()
                        .and_then(variable_to_creator_control_value)
                })
                .ok_or_else(|| shell_invalid("interactive creator control has no safe value"))?;
            let choices = control
                .options
                .iter()
                .map(|option| match &option.value {
                    VariableValue::Text(value) | VariableValue::Enum(value) => Ok(value.clone()),
                    _ => Err(shell_invalid(
                        "select creator control has a non-text option value",
                    )),
                })
                .collect::<ShellResult<Vec<_>>>()?;
            Ok(CreatorControlProjectionDto {
                id: control.id.as_str().to_owned(),
                label: control.label.clone(),
                description: (!control.description.is_empty()).then(|| control.description.clone()),
                kind: control.kind,
                value,
                choices,
                minimum: control.minimum,
                maximum: control.maximum,
                step: control.step,
            })
        })
        .collect()
}

fn variable_to_creator_control_value(value: &VariableValue) -> Option<CoreCreatorControlValue> {
    match value {
        VariableValue::Bool(value) => Some(CoreCreatorControlValue::Bool(*value)),
        VariableValue::Integer(value) => Some(CoreCreatorControlValue::Integer(*value)),
        VariableValue::Decimal(value) if value.is_finite() => {
            Some(CoreCreatorControlValue::Decimal(*value))
        }
        VariableValue::Text(value) | VariableValue::Enum(value) => {
            Some(CoreCreatorControlValue::Text(value.clone()))
        }
        VariableValue::StringList(values) => {
            Some(CoreCreatorControlValue::StringList(values.clone()))
        }
        VariableValue::Decimal(_) => None,
    }
}

fn project_prompt_blocks(preset: &PromptPreset) -> ShellResult<Vec<PromptBlockProjectionDto>> {
    if preset.blocks.len() > MAX_PROMPT_PREVIEW_ITEMS {
        return Err(shell_invalid(
            "prompt preset exceeds the block projection limit",
        ));
    }
    preset
        .blocks
        .iter()
        .map(|block| project_prompt_block(preset, block))
        .collect()
}

fn project_prompt_block(
    preset: &PromptPreset,
    block: &PromptBlock,
) -> ShellResult<PromptBlockProjectionDto> {
    let application_policy = block.placement_zone == PlacementZone::ApplicationPolicy;
    let template_preview = if application_policy {
        None
    } else {
        block
            .template
            .as_ref()
            .map(bounded_serialized_preview)
            .transpose()?
    };
    let condition_summary = if application_policy {
        None
    } else {
        block
            .condition
            .as_ref()
            .map(bounded_serialized_preview)
            .transpose()?
    };
    Ok(PromptBlockProjectionDto {
        id: block.id.0.clone(),
        name: block.name.clone(),
        kind: block.kind,
        enabled: block.enabled,
        order_editable: !application_policy,
        role_hint: block.role_hint,
        placement_zone: block.placement_zone,
        template_preview,
        condition_summary,
        source_label: if application_policy {
            "application_policy".to_owned()
        } else {
            bounded_serialized_preview(&block.source)?
        },
        provenance_label: serialized_enum_label(&block.provenance.source_kind)?,
        priority: block.token_policy.priority,
        minimum_tokens: block.token_policy.min_tokens,
        maximum_tokens: block.token_policy.max_tokens,
        overflow_policy: block.overflow_policy,
        cache_boundary_after: preset
            .cache_boundaries
            .iter()
            .any(|boundary| boundary.after_block_id.as_str() == block.id.as_str()),
    })
}

fn bounded_serialized_preview<T: Serialize>(value: &T) -> ShellResult<String> {
    let encoded = serde_json::to_string(value)
        .map_err(|_| shell_invalid("prompt projection is not serializable"))?;
    Ok(bounded_utf8(encoded, MAX_PROMPT_BLOCK_PREVIEW_BYTES).0)
}

fn serialized_enum_label<T: Serialize>(value: &T) -> ShellResult<String> {
    let encoded = serde_json::to_value(value)
        .map_err(|_| shell_invalid("prompt projection label is not serializable"))?;
    encoded
        .as_str()
        .map(str::to_owned)
        .ok_or_else(|| shell_invalid("prompt projection label is not a string"))
}

fn bounded_utf8(value: String, max_bytes: usize) -> (String, bool) {
    if value.len() <= max_bytes {
        return (value, false);
    }
    let mut end = max_bytes;
    while !value.is_char_boundary(end) {
        end = end.saturating_sub(1);
    }
    (value[..end].to_owned(), true)
}

fn validate_document<T: Serialize>(value: &T) -> ShellResult<()> {
    validate_core_document(value).map_err(ShellError::from)
}

fn validated_output<T: Serialize>(value: T) -> ShellResult<T> {
    validate_document(&value)?;
    Ok(value)
}

fn validate_core_document<T: Serialize>(value: &T) -> lorepia_core::CoreResult<()> {
    let encoded = serde_json::to_vec(value)
        .map_err(|_| CoreError::invalid("document is not serializable"))?;
    if encoded.len() > MAX_DOCUMENT_BYTES {
        return Err(CoreError::invalid("document exceeds the IPC byte limit"));
    }
    let json = serde_json::to_value(value)
        .map_err(|_| CoreError::invalid("document is not serializable"))?;
    let mut budget = JsonBudget::default();
    inspect_json(&json, 0, &mut budget)?;
    Ok(())
}

#[derive(Default)]
struct JsonBudget {
    nodes: usize,
    string_chars: usize,
}

fn inspect_json(
    value: &serde_json::Value,
    depth: usize,
    budget: &mut JsonBudget,
) -> lorepia_core::CoreResult<()> {
    if depth > MAX_DOCUMENT_DEPTH {
        return Err(CoreError::invalid("document exceeds the nesting limit"));
    }
    budget.nodes = budget.nodes.saturating_add(1);
    if budget.nodes > MAX_DOCUMENT_NODES {
        return Err(CoreError::invalid("document exceeds the node limit"));
    }
    match value {
        serde_json::Value::String(value) => {
            budget.string_chars = budget.string_chars.saturating_add(value.chars().count());
            if budget.string_chars > MAX_DOCUMENT_STRING_CHARS {
                return Err(CoreError::invalid("document exceeds the text limit"));
            }
        }
        serde_json::Value::Array(values) => {
            if values.len() > MAX_COLLECTION_ITEMS {
                return Err(CoreError::invalid(
                    "document collection exceeds the item limit",
                ));
            }
            for value in values {
                inspect_json(value, depth.saturating_add(1), budget)?;
            }
        }
        serde_json::Value::Object(values) => {
            if values.len() > MAX_COLLECTION_ITEMS {
                return Err(CoreError::invalid(
                    "document object exceeds the field limit",
                ));
            }
            for (key, value) in values {
                budget.string_chars = budget.string_chars.saturating_add(key.chars().count());
                if budget.string_chars > MAX_DOCUMENT_STRING_CHARS {
                    return Err(CoreError::invalid("document exceeds the text limit"));
                }
                inspect_json(value, depth.saturating_add(1), budget)?;
            }
        }
        serde_json::Value::Null | serde_json::Value::Bool(_) | serde_json::Value::Number(_) => {}
    }
    Ok(())
}

fn bound_core_collection<T: Serialize>(values: Vec<T>) -> lorepia_core::CoreResult<Vec<T>> {
    if values.len() > MAX_COLLECTION_ITEMS {
        return Err(CoreError::invalid(
            "orchestration collection exceeds the IPC item limit",
        ));
    }
    validate_core_document(&values)?;
    Ok(values)
}

#[cfg(test)]
mod tests {
    use std::{
        future::Future,
        pin::Pin,
        sync::{
            Arc, Mutex,
            atomic::{AtomicUsize, Ordering},
        },
        thread,
        time::{Duration, Instant},
    };

    use lorepia_core::{
        ConversationBranchId, ConversationId, Core, CoreConfig, CoreErrorCode,
        InstructionAuthority, MemoryKind, MemoryRecord, MemoryRecordId, MessageId, PlacementZone,
        PromptPreset, PromptPresetId, Provenance, ProviderConnectionId, SourceKind,
        TaskCredentialBroker, VersionedJson,
    };
    use lorepia_storage::Storage;
    use tempfile::{NamedTempFile, tempdir};

    use super::{
        ApplyPromptPresetRollbackInput, CreatorContentModuleDocumentDto,
        CreatorInteractionRuleSetDocumentDto, CreatorKnowledgeBookDocumentDto,
        CreatorMemoryProfileDocumentDto, CreatorPromptPresetDocumentDto,
        CreatorTransformSetDocumentDto, DeleteMemoryRecordInput, DeletePromptPresetInput,
        ExplainPromptPlanInput, GetMemoryRecordInput, GetPromptPresetInput, ListMemoryRecordsInput,
        MAX_DOCUMENT_STRING_CHARS, MemoryRecordExclusionScopeDto, MemoryRecordPatchDto,
        PatchMemoryRecordInput, PromptBlockResolutionTraceDto, PromptPresetSummaryDto,
        ResolvePromptPreviewInput, ReviewedPromptSendInput, SetMemoryRecordExclusionInput,
        bound_core_collection, project_applied_parameter, project_prompt_blocks,
        validate_creator_content_module_input, validate_creator_prompt_preset_input,
        validate_document, validate_sha256,
    };
    use crate::{
        ConversationGreetingSelectionInput as ShellConversationGreetingSelectionInput,
        CreateConversationInput, SecretCredential, ShellApi, ShellErrorCode, StagedImportFile,
        TaskCredentialRead, TaskCredentialReader, dto::ConversationModeDto,
        orchestration::ShellTaskCredentialBroker,
    };

    struct CanaryTaskCredentialReader {
        canary: String,
        requested: Arc<Mutex<Vec<String>>>,
    }

    struct LeaseDropProbe(Arc<AtomicUsize>);

    impl Drop for LeaseDropProbe {
        fn drop(&mut self) {
            self.0.fetch_add(1, Ordering::SeqCst);
        }
    }

    impl TaskCredentialReader for CanaryTaskCredentialReader {
        fn credential_for<'a>(
            &'a self,
            connection_id: &'a str,
        ) -> Pin<Box<dyn Future<Output = TaskCredentialRead> + Send + 'a>> {
            Box::pin(async move {
                self.requested
                    .lock()
                    .expect("credential request log")
                    .push(connection_id.to_owned());
                TaskCredentialRead::Available(SecretCredential::new(self.canary.clone()))
            })
        }
    }

    #[tokio::test]
    async fn task_credential_broker_binds_exact_connection_and_redacts_canary() {
        let canary = "shell-task-credential-canary";
        let requested = Arc::new(Mutex::new(Vec::new()));
        let reader = CanaryTaskCredentialReader {
            canary: canary.to_owned(),
            requested: Arc::clone(&requested),
        };
        let broker = ShellTaskCredentialBroker {
            credential_reader: &reader,
        };
        let connection_id = ProviderConnectionId::from("embedding-connection");
        let credential = broker
            .credential_for(&connection_id)
            .await
            .expect("bound task credential");
        assert_eq!(
            requested.lock().expect("credential request log").as_slice(),
            [connection_id.as_str()]
        );
        let debug = format!("{credential:?}");
        assert!(debug.contains("[REDACTED]"));
        assert!(!debug.contains(canary));
        assert!(
            !serde_json::to_string(&debug)
                .expect("serialize redacted debug")
                .contains(canary)
        );
    }

    #[tokio::test]
    async fn task_credential_broker_transfers_dispatch_lease_to_core_carrier() {
        struct LeasedReader {
            dropped: Arc<AtomicUsize>,
            missing: bool,
        }

        impl TaskCredentialReader for LeasedReader {
            fn credential_for<'a>(
                &'a self,
                _connection_id: &'a str,
            ) -> Pin<Box<dyn Future<Output = TaskCredentialRead> + Send + 'a>> {
                Box::pin(async move {
                    let lease = LeaseDropProbe(Arc::clone(&self.dropped));
                    if self.missing {
                        TaskCredentialRead::MissingWithLease(crate::TaskCredentialLease::new(lease))
                    } else {
                        TaskCredentialRead::AvailableWithLease {
                            credential: SecretCredential::new("leased-secret"),
                            lease: crate::TaskCredentialLease::new(lease),
                        }
                    }
                })
            }
        }

        for missing in [false, true] {
            let dropped = Arc::new(AtomicUsize::new(0));
            let reader = LeasedReader {
                dropped: Arc::clone(&dropped),
                missing,
            };
            let broker = ShellTaskCredentialBroker {
                credential_reader: &reader,
            };
            let credential = broker
                .credential_for(&ProviderConnectionId::from("leased-connection"))
                .await
                .expect("leased credential carrier");
            assert_eq!(dropped.load(Ordering::SeqCst), 0);
            drop(credential);
            assert_eq!(dropped.load(Ordering::SeqCst), 1);
        }
    }

    #[test]
    fn mutation_inputs_reject_unknown_fields() {
        let error = serde_json::from_value::<DeletePromptPresetInput>(serde_json::json!({
            "prompt_preset_id": "preset",
            "expected_revision": 3,
            "generic_execute": "forbidden"
        }))
        .expect_err("unknown fields must be rejected");
        assert!(error.to_string().contains("unknown field"));
    }

    #[test]
    fn prompt_projection_schema_excludes_literal_and_free_form_carriers() {
        let parameter = project_applied_parameter(lorepia_core::PromptAppliedParameterPreview {
            field: "response_format.schema".to_owned(),
            value: serde_json::json!({"type": "object", "properties": {}}),
        })
        .expect("project structural parameter shape");
        assert_eq!(
            serde_json::to_value(parameter).expect("serialize parameter projection"),
            serde_json::json!({
                "field": "response_format.schema",
                "value_kind": "object",
                "item_count": 2
            })
        );

        let trace = PromptBlockResolutionTraceDto::from(lorepia_core::BlockResolutionTrace {
            block_id: lorepia_core::PromptBlockId::from("synthetic.knowledge-block"),
            block_kind: lorepia_core::PromptBlockKind::WorldKnowledge,
            source: lorepia_core::PromptBlockSourceTrace {
                authority: InstructionAuthority::Creator,
                source_kind: SourceKind::UserCreated,
                source_id: Some("synthetic.knowledge-source".to_owned()),
                source_revision: Some("1".to_owned()),
                source_hash: Some("a".repeat(64)),
            },
            status: lorepia_core::BlockResolutionStatus::ReducedItems,
            original_estimated_tokens: 20,
            final_estimated_tokens: 10,
            produced_message_count: 1,
            explanation: "lower-ranked knowledge entries removed".to_owned(),
            knowledge_evidence: vec![lorepia_core::KnowledgeSelectionEvidence {
                entry_id: lorepia_core::KnowledgeEntryId::from("synthetic.knowledge-entry"),
                selected: true,
                reasons: vec![
                    lorepia_core::KnowledgeActivationReason::Keyword {
                        matched: "moon".to_owned(),
                    },
                    lorepia_core::KnowledgeActivationReason::Regex {
                        pattern: "moon|night".to_owned(),
                    },
                ],
                estimated_tokens: 10,
                exclusion_reason: None,
            }],
            memory_record_ids: vec![lorepia_core::MemoryRecordId::from(
                "synthetic.memory-record",
            )],
            memory_evidence: vec![lorepia_core::PromptMemorySelectionEvidence {
                record_id: lorepia_core::MemoryRecordId::from("synthetic.memory-record"),
                selected: false,
                lane: None,
                rank_millionths: Some(500_000),
                estimated_tokens: 8,
                reasons: vec![lorepia_core::PromptMemorySelectionReason::Similarity {
                    score_millionths: 500_000,
                }],
                exclusion_reason: Some("removed by the prompt token budget".to_owned()),
            }],
        });
        let encoded = serde_json::to_value(trace).expect("serialize structural prompt trace");
        assert_eq!(
            encoded["knowledge_evidence"][0]["reasons"],
            serde_json::json!([{"kind": "keyword"}, {"kind": "regex"}])
        );
        assert_eq!(
            encoded["memory_evidence"][0]["exclusion_code"],
            "prompt_token_budget"
        );
        let encoded_text = serde_json::to_string(&encoded).expect("encode prompt trace JSON");
        for forbidden_key in [
            "matched",
            "pattern",
            "explanation",
            "exclusion_reason",
            "content",
            "provider_request",
        ] {
            assert!(!encoded_text.contains(&format!("\"{forbidden_key}\"")));
        }
    }

    #[test]
    fn prompt_preset_rollback_apply_rejects_submitted_review_documents() {
        let error = serde_json::from_value::<ApplyPromptPresetRollbackInput>(serde_json::json!({
            "prompt_preset_id": "preset",
            "expected_current_revision": 3,
            "target_revision": 1,
            "approval_id": "approval-1",
            "expected_review_sha256": "a".repeat(64),
            "review": {
                "target_document_sha256": "b".repeat(64)
            }
        }))
        .expect_err("historical review documents must be unrepresentable");
        assert!(error.to_string().contains("unknown field"));
    }

    #[test]
    fn orchestration_documents_are_bounded_before_core() {
        let oversized = "x".repeat(MAX_DOCUMENT_STRING_CHARS + 1);
        let error = validate_document(&oversized).expect_err("oversized text must fail");
        assert_eq!(error.code.as_str(), "invalid_input");

        let error =
            bound_core_collection(vec![oversized]).expect_err("oversized response must fail");
        assert_eq!(error.code.as_str(), "invalid_input");
    }

    #[test]
    fn creator_documents_reject_security_owned_fields() {
        let cases = [
            serde_json::from_value::<CreatorMemoryProfileDocumentDto>(serde_json::json!({
                "id": "memory-profile",
                "name": "Synthetic memory",
                "summary_task": "summary-task",
                "embedding_task": null,
                "turns_per_summary": 4,
                "recent_raw_budget": {"max_tokens": 100},
                "episodic_budget": {"max_tokens": 100},
                "semantic_budget": {"max_tokens": 100},
                "retrieval_count": 4,
                "recency_weight": 1.0,
                "similarity_weight": 1.0,
                "importance_weight": 1.0,
                "preserve_invalidated_records": false,
                "summary_schema": "summary-schema",
                "provenance": {"source_kind": "imported_package"}
            }))
            .expect_err("memory provenance must be unrepresentable"),
            serde_json::from_value::<CreatorKnowledgeBookDocumentDto>(serde_json::json!({
                "id": "knowledge-book",
                "name": "Synthetic knowledge",
                "entries": [],
                "scan_depth": 1,
                "token_budget": {"max_tokens": 100},
                "recursive": false,
                "max_recursion_depth": 1,
                "schema_version": 9
            }))
            .expect_err("knowledge schema must be Rust-owned"),
            serde_json::from_value::<CreatorTransformSetDocumentDto>(serde_json::json!({
                "id": "transform-set",
                "name": "Synthetic transforms",
                "enabled": true,
                "rules": [],
                "max_rules_per_phase": 10,
                "max_output_chars": 1024,
                "imported_author_enabled": true
            }))
            .expect_err("transform import gate must be unrepresentable"),
            serde_json::from_value::<CreatorInteractionRuleSetDocumentDto>(serde_json::json!({
                "id": "interaction-rules",
                "name": "Synthetic interactions",
                "rules": [],
                "max_actions_per_event": 10,
                "provenance": {"source_kind": "user_created"}
            }))
            .expect_err("interaction provenance must be Rust-owned"),
            serde_json::from_value::<CreatorContentModuleDocumentDto>(serde_json::json!({
                "id": "content-module",
                "name": "Synthetic module",
                "version": "1.0.0",
                "prompt_fragments": [],
                "knowledge_book_ids": [],
                "control_specs": [],
                "transform_set_ids": [],
                "interaction_rule_set_ids": [],
                "asset_ids": [],
                "required_capabilities": [],
                "metadata": {
                    "author": null,
                    "license": "LicenseRef-Private",
                    "redistribution_allowed": false,
                    "homepage": null,
                    "description": "",
                    "tags": []
                },
                "imported_components_enabled": true
            }))
            .expect_err("module import gate must be unrepresentable"),
        ];
        assert!(
            cases
                .iter()
                .all(|error| error.to_string().contains("unknown field"))
        );
    }

    #[test]
    fn creator_module_assets_fail_closed_without_derived_capabilities() {
        let module = serde_json::from_value::<CreatorContentModuleDocumentDto>(serde_json::json!({
            "id": "content-module",
            "name": "Synthetic module",
            "version": "1.0.0",
            "prompt_fragments": [],
            "knowledge_book_ids": [],
            "control_specs": [],
            "transform_set_ids": [],
            "interaction_rule_set_ids": [],
            "asset_ids": ["asset-1"],
            "required_capabilities": ["image_assets"],
            "metadata": {
                "author": null,
                "license": "LicenseRef-Private",
                "redistribution_allowed": false,
                "homepage": null,
                "description": "",
                "tags": []
            }
        }))
        .expect("safe creator module");
        let error = validate_creator_content_module_input(&module)
            .expect_err("asset capabilities require Rust derivation");
        assert_eq!(error.code.as_str(), "invalid_input");
    }

    #[test]
    fn prompt_resolution_inputs_reject_implicit_or_unknown_state() {
        let error = serde_json::from_value::<ResolvePromptPreviewInput>(serde_json::json!({
            "conversation_id": "conversation",
            "branch_id": "branch",
            "expected_head": null,
            "user_text": "hello",
            "generation_target": {
                "model_route_id": "route",
                "generation_preset_id": "generation-preset"
            },
            "prompt_preset_id": null,
            "variable_overrides": { "values": [] },
            "expected_plan_hash": null,
            "generic_execute": true
        }))
        .expect_err("unknown prompt resolution state must be rejected");
        assert!(error.to_string().contains("unknown field"));
    }

    #[test]
    fn generation_operation_identity_is_serde_compatible_at_the_prompt_boundary() {
        let preview = serde_json::from_value::<ResolvePromptPreviewInput>(serde_json::json!({
            "conversation_id": "conversation",
            "branch_id": "branch",
            "expected_head": null,
            "user_text": "hello",
            "generation_target": {
                "model_route_id": "route",
                "generation_preset_id": "generation-preset"
            },
            "prompt_preset_id": null,
            "variable_overrides": { "values": [] },
            "expected_plan_hash": null
        }))
        .expect("older preview payloads must still decode");
        assert_eq!(preview.operation_nonce, None);
        assert_eq!(preview.generation_attempt_id, None);

        let reviewed = ReviewedPromptSendInput {
            conversation_id: preview.conversation_id,
            branch_id: preview.branch_id,
            expected_head: preview.expected_head,
            user_text: preview.user_text,
            generation_target: preview.generation_target,
            prompt_preset_id: preview.prompt_preset_id,
            variable_overrides: preview.variable_overrides,
            expected_plan_hash: "a".repeat(64),
            generation_attempt_id: "generation-attempt-1".to_owned(),
        };
        let encoded = serde_json::to_value(reviewed).expect("reviewed input must serialize");
        assert_eq!(encoded["generation_attempt_id"], "generation-attempt-1");
        assert!(encoded.get("operation_nonce").is_none());
        let mut ambiguous_reviewed = encoded;
        ambiguous_reviewed["operation_nonce"] = serde_json::json!("must-be-rejected");
        assert!(
            serde_json::from_value::<ReviewedPromptSendInput>(ambiguous_reviewed).is_err(),
            "reviewed send must use only its required generation attempt identity"
        );

        let explain = serde_json::from_value::<ExplainPromptPlanInput>(serde_json::json!({
            "conversation_id": "conversation",
            "branch_id": "branch",
            "expected_head": null,
            "user_text": "hello",
            "generation_target": {
                "model_route_id": "route",
                "generation_preset_id": "generation-preset"
            },
            "prompt_preset_id": null,
            "variable_overrides": { "values": [] },
            "plan_hash": "a".repeat(64)
        }))
        .expect("older explain payloads must still decode");
        assert_eq!(explain.operation_nonce, None);
        assert_eq!(explain.generation_attempt_id, None);
    }

    #[test]
    fn plan_hashes_are_exact_lowercase_sha256_values() {
        validate_sha256("plan_hash", &"a".repeat(64)).expect("lowercase digest");
        assert!(validate_sha256("plan_hash", &"A".repeat(64)).is_err());
        assert!(validate_sha256("plan_hash", "short").is_err());
    }

    #[test]
    fn prompt_editor_projection_redacts_application_policy() {
        let root = tempdir().expect("temporary data root");
        let shell = ShellApi::open(CoreConfig::new(root.path())).expect("open shell");
        let preset = shell
            .list_prompt_presets()
            .expect("built-in presets")
            .into_iter()
            .next()
            .expect("built-in preset");
        let projected = project_prompt_blocks(&preset.value).expect("project blocks");
        let application_policy = projected
            .iter()
            .find(|block| block.placement_zone == PlacementZone::ApplicationPolicy)
            .expect("application policy projection");
        assert!(!application_policy.order_editable);
        assert!(application_policy.template_preview.is_none());
        assert!(application_policy.condition_summary.is_none());
        assert_eq!(application_policy.source_label, "application_policy");
        assert!(validate_creator_prompt_preset_input(&preset.value).is_err());
        assert!(
            shell
                .get_editable_prompt_preset(GetPromptPresetInput {
                    prompt_preset_id: preset.value.id.0.clone(),
                })
                .is_err()
        );

        let mut user_preset = preset.value.clone();
        user_preset.id = PromptPresetId::from("user-editable-preset");
        user_preset.metadata.provenance.source_kind = SourceKind::UserCreated;
        for block in &mut user_preset.blocks {
            if block.authority != InstructionAuthority::Application {
                block.provenance.source_kind = SourceKind::UserCreated;
            }
        }
        let editable =
            CreatorPromptPresetDocumentDto::try_from(user_preset).expect("creator projection");
        assert!(editable.blocks.iter().all(|block| {
            PlacementZone::from(block.placement_zone) != PlacementZone::ApplicationPolicy
        }));
        let mut crafted = serde_json::to_value(&editable).expect("serialize creator document");
        crafted["blocks"][0]["authority"] = serde_json::json!("application");
        assert!(
            serde_json::from_value::<CreatorPromptPresetDocumentDto>(crafted).is_err(),
            "application authority must be unrepresentable at deserialization"
        );
        let creator_round_trip = PromptPreset::from(editable);
        assert!(creator_round_trip.blocks.iter().all(|block| {
            block.authority != InstructionAuthority::Application
                && block.placement_zone != PlacementZone::ApplicationPolicy
        }));

        let summary = PromptPresetSummaryDto::from(preset.value);
        let encoded = serde_json::to_value(summary).expect("serialize summary");
        assert!(encoded.get("blocks").is_none());
    }

    struct MemoryCrudFixture {
        conversation: String,
        branch: String,
        record: String,
    }

    fn open_after_drop<T>(mut open: impl FnMut() -> lorepia_core::CoreResult<T>) -> T {
        let deadline = Instant::now() + Duration::from_secs(2);
        loop {
            match open() {
                Ok(value) => return value,
                Err(error)
                    if error.code == CoreErrorCode::StorageUnavailable
                        && error.message
                            == "data root is already owned by another LorePia process"
                        && Instant::now() < deadline =>
                {
                    thread::sleep(Duration::from_millis(10));
                }
                Err(error) => panic!("open after prior owner drop: {error:?}"),
            }
        }
    }

    fn open_shell_after_drop(data_root: &std::path::Path) -> ShellApi {
        open_after_drop(|| Core::open(CoreConfig::new(data_root)).map(ShellApi::from_core))
    }

    fn open_storage_after_drop(data_root: &std::path::Path) -> Storage {
        open_after_drop(|| Storage::open(data_root))
    }

    fn seed_memory_crud_fixture(data_root: &std::path::Path) -> MemoryCrudFixture {
        let shell = ShellApi::open(CoreConfig::new(data_root)).expect("open fixture Shell");
        let source = NamedTempFile::new().expect("create synthetic character source");
        std::fs::write(
            source.path(),
            br#"{"spec":"chara_card_v3","data":{"name":"Memory Shell","description":"Synthetic Shell memory CRUD fixture","first_mes":"Synthetic greeting source"}}"#,
        )
        .expect("write synthetic character source");
        let inspection = shell
            .inspect_import(&StagedImportFile::new(source.path()))
            .expect("inspect synthetic character");
        let character = shell
            .commit_import(&inspection.inspection_id)
            .expect("commit synthetic character");
        let catalog = shell
            .get_character_greeting_catalog(&character.id)
            .expect("load exact greeting catalog");
        let greeting_id = catalog
            .greetings
            .first()
            .expect("synthetic default greeting")
            .id
            .clone();
        let conversation = shell
            .create_conversation(CreateConversationInput {
                character_id: character.id,
                title: "Synthetic memory CRUD".to_owned(),
                mode: ConversationModeDto::Chat,
                greeting: Some(ShellConversationGreetingSelectionInput {
                    character_content_revision_id: catalog.character_content_revision_id,
                    greeting_id: Some(greeting_id),
                }),
            })
            .expect("create exact greeting conversation");
        let state = shell
            .get_conversation_state(&conversation.id)
            .expect("load conversation state");
        let messages = shell
            .list_branch_messages(&state.active_branch_id)
            .expect("load greeting lineage");
        let source_message = messages
            .first()
            .expect("committed greeting source message")
            .id
            .clone();
        drop(shell);

        let record_id = "synthetic.shell.memory-record".to_owned();
        let storage = open_storage_after_drop(data_root);
        let stored = storage
            .save_memory_record(
                &MemoryRecord {
                    id: MemoryRecordId::from(record_id.clone()),
                    conversation_id: ConversationId(conversation.id.clone()),
                    branch_id: ConversationBranchId(state.active_branch_id.clone()),
                    source_start_message_id: MessageId(source_message.clone()),
                    source_end_message_id: MessageId(source_message),
                    kind: MemoryKind::CreatorPinned,
                    title: "Synthetic initial memory".to_owned(),
                    summary: "Synthetic initial summary".to_owned(),
                    structured_data: VersionedJson {
                        schema_version: 1,
                        value: serde_json::json!({"fixture": "shell-memory-crud"}),
                    },
                    importance: 50,
                    keywords: vec!["initial".to_owned()],
                    embedding_ref: None,
                    pinned: false,
                    excluded_from_conversation: false,
                    excluded_from_character: false,
                    created_at: conversation.created_at,
                    updated_at: conversation.created_at,
                    invalidated_at: None,
                    provenance: Provenance {
                        source_kind: SourceKind::UserCreated,
                        source_id: Some("synthetic.shell.memory-record".to_owned()),
                        source_hash: None,
                        author: None,
                        license: None,
                        imported_at: None,
                    },
                },
                None,
            )
            .expect("seed actual memory record");
        assert_eq!(stored.revision, 1);
        MemoryCrudFixture {
            conversation: conversation.id,
            branch: state.active_branch_id,
            record: record_id,
        }
    }

    fn get_memory(
        shell: &ShellApi,
        fixture: &MemoryCrudFixture,
    ) -> super::MemoryRecordProjectionDto {
        shell
            .get_memory_record_projection(GetMemoryRecordInput {
                conversation_id: fixture.conversation.clone(),
                branch_id: fixture.branch.clone(),
                memory_record_id: fixture.record.clone(),
            })
            .expect("get memory record")
    }

    fn assert_shell_memory_owner_mismatch(
        shell: &ShellApi,
        fixture: &MemoryCrudFixture,
        conversation_id: &str,
        branch_id: &str,
        mismatch: &str,
    ) {
        let get = shell
            .get_memory_record_projection(GetMemoryRecordInput {
                conversation_id: conversation_id.to_owned(),
                branch_id: branch_id.to_owned(),
                memory_record_id: fixture.record.clone(),
            })
            .unwrap_err();
        assert_eq!(get.code, ShellErrorCode::NotFound, "{mismatch} get");

        let patch = shell
            .patch_memory_record(PatchMemoryRecordInput {
                conversation_id: conversation_id.to_owned(),
                branch_id: branch_id.to_owned(),
                memory_record_id: fixture.record.clone(),
                patch: MemoryRecordPatchDto {
                    title: Some("foreign Shell overwrite".to_owned()),
                    ..MemoryRecordPatchDto::default()
                },
                expected_revision: 1,
            })
            .unwrap_err();
        assert_eq!(patch.code, ShellErrorCode::NotFound, "{mismatch} patch");

        let exclusion = shell
            .set_memory_record_exclusion(SetMemoryRecordExclusionInput {
                conversation_id: conversation_id.to_owned(),
                branch_id: branch_id.to_owned(),
                memory_record_id: fixture.record.clone(),
                scope: MemoryRecordExclusionScopeDto::Conversation,
                excluded: true,
                expected_revision: 1,
            })
            .unwrap_err();
        assert_eq!(
            exclusion.code,
            ShellErrorCode::NotFound,
            "{mismatch} exclusion"
        );

        let delete = shell
            .delete_memory_record(DeleteMemoryRecordInput {
                conversation_id: conversation_id.to_owned(),
                branch_id: branch_id.to_owned(),
                memory_record_id: fixture.record.clone(),
                expected_revision: 1,
            })
            .unwrap_err();
        assert_eq!(delete.code, ShellErrorCode::NotFound, "{mismatch} delete");
    }

    fn mutate_memory(
        shell: &ShellApi,
        fixture: &MemoryCrudFixture,
        initial: &super::MemoryRecordProjectionDto,
    ) -> super::MemoryRecordProjectionDto {
        let edited = shell
            .patch_memory_record(PatchMemoryRecordInput {
                conversation_id: fixture.conversation.clone(),
                branch_id: fixture.branch.clone(),
                memory_record_id: fixture.record.clone(),
                patch: MemoryRecordPatchDto {
                    title: Some("User-edited memory".to_owned()),
                    summary: Some("User-edited exact summary".to_owned()),
                    importance: Some(88),
                    keywords: Some(vec!["edited".to_owned(), "exact-cas".to_owned()]),
                    pinned: None,
                },
                expected_revision: initial.revision,
            })
            .expect("edit at exact revision");
        assert_eq!(edited.revision, 2);
        assert_eq!(edited.title, "User-edited memory");
        assert_eq!(edited.summary, "User-edited exact summary");
        assert_eq!(edited.importance, 88);
        assert_eq!(edited.keywords, ["edited", "exact-cas"]);
        assert!(!edited.pinned);
        let pinned = shell
            .patch_memory_record(PatchMemoryRecordInput {
                conversation_id: fixture.conversation.clone(),
                branch_id: fixture.branch.clone(),
                memory_record_id: fixture.record.clone(),
                patch: MemoryRecordPatchDto {
                    pinned: Some(true),
                    ..MemoryRecordPatchDto::default()
                },
                expected_revision: edited.revision,
            })
            .expect("pin at exact revision");
        assert_eq!(pinned.revision, 3);
        assert!(pinned.pinned);
        let conversation_excluded = shell
            .set_memory_record_exclusion(SetMemoryRecordExclusionInput {
                conversation_id: fixture.conversation.clone(),
                branch_id: fixture.branch.clone(),
                memory_record_id: fixture.record.clone(),
                scope: MemoryRecordExclusionScopeDto::Conversation,
                excluded: true,
                expected_revision: pinned.revision,
            })
            .expect("exclude from conversation at exact revision");
        assert_eq!(conversation_excluded.revision, 4);
        let character_excluded = shell
            .set_memory_record_exclusion(SetMemoryRecordExclusionInput {
                conversation_id: fixture.conversation.clone(),
                branch_id: fixture.branch.clone(),
                memory_record_id: fixture.record.clone(),
                scope: MemoryRecordExclusionScopeDto::Character,
                excluded: true,
                expected_revision: conversation_excluded.revision,
            })
            .expect("exclude from character at exact revision");
        assert_eq!(character_excluded.revision, 5);
        assert!(character_excluded.excluded_from_conversation);
        assert!(character_excluded.excluded_from_character);

        let stale = shell
            .patch_memory_record(PatchMemoryRecordInput {
                conversation_id: fixture.conversation.clone(),
                branch_id: fixture.branch.clone(),
                memory_record_id: fixture.record.clone(),
                patch: MemoryRecordPatchDto {
                    title: Some("Stale overwrite".to_owned()),
                    ..MemoryRecordPatchDto::default()
                },
                expected_revision: initial.revision,
            })
            .expect_err("stale memory edit must fail");
        assert_eq!(stale.code, ShellErrorCode::InvalidInput);
        assert!(stale.recoverable);
        assert_eq!(get_memory(shell, fixture), character_excluded);
        character_excluded
    }

    #[test]
    fn live_memory_crud_uses_exact_cas_and_survives_reopen() {
        let root = tempdir().expect("temporary data root");
        let fixture = seed_memory_crud_fixture(root.path());
        let shell = ShellApi::open(CoreConfig::new(root.path())).expect("open Shell after seed");
        let initial = get_memory(&shell, &fixture);
        assert_eq!(initial.revision, 1);
        assert_eq!(initial.title, "Synthetic initial memory");
        let listed = shell
            .list_memory_records(ListMemoryRecordsInput {
                conversation_id: fixture.conversation.clone(),
                branch_id: fixture.branch.clone(),
                include_invalidated: false,
            })
            .expect("list initial memory");
        assert_eq!(listed.records.as_slice(), std::slice::from_ref(&initial));
        let character_excluded = mutate_memory(&shell, &fixture, &initial);
        drop(shell);

        let reopened = open_shell_after_drop(root.path());
        assert_eq!(get_memory(&reopened, &fixture), character_excluded);
        reopened
            .delete_memory_record(DeleteMemoryRecordInput {
                conversation_id: fixture.conversation.clone(),
                branch_id: fixture.branch.clone(),
                memory_record_id: fixture.record.clone(),
                expected_revision: character_excluded.revision,
            })
            .expect("delete memory at exact revision");
        drop(reopened);

        let deleted = open_shell_after_drop(root.path());
        let error = deleted
            .get_memory_record_projection(GetMemoryRecordInput {
                conversation_id: fixture.conversation.clone(),
                branch_id: fixture.branch.clone(),
                memory_record_id: fixture.record,
            })
            .expect_err("deleted memory must stay absent after reopen");
        assert_eq!(error.code, ShellErrorCode::NotFound);
        let listed = deleted
            .list_memory_records(ListMemoryRecordsInput {
                conversation_id: fixture.conversation,
                branch_id: fixture.branch,
                include_invalidated: true,
            })
            .expect("list after delete and reopen");
        assert!(listed.records.is_empty());
    }

    #[test]
    fn shell_memory_crud_rejects_each_partial_owner_mismatch() {
        let root = tempdir().expect("temporary data root");
        let fixture = seed_memory_crud_fixture(root.path());
        let shell = ShellApi::open(CoreConfig::new(root.path())).expect("open Shell after seed");
        let initial = get_memory(&shell, &fixture);

        for (conversation_id, branch_id, mismatch) in [
            (
                "synthetic.foreign.conversation",
                fixture.branch.as_str(),
                "conversation",
            ),
            (
                fixture.conversation.as_str(),
                "synthetic.foreign.branch",
                "branch",
            ),
        ] {
            assert_shell_memory_owner_mismatch(
                &shell,
                &fixture,
                conversation_id,
                branch_id,
                mismatch,
            );
        }

        assert_eq!(get_memory(&shell, &fixture), initial);
    }

    #[test]
    fn interrupted_memory_job_retry_requires_explicit_unknown_outcome_acknowledgement() {
        let root = tempdir().expect("temporary data root");
        let shell = ShellApi::open(CoreConfig::new(root.path())).expect("open Shell");

        let error = shell
            .retry_interrupted_memory_job(super::RetryInterruptedMemoryJobInput {
                conversation_id: "conversation:synthetic".to_owned(),
                branch_id: "branch:synthetic".to_owned(),
                memory_job_id: "synthetic.interrupted-memory-job".to_owned(),
                expected_revision: 2,
                acknowledge_unknown_outcome: false,
            })
            .expect_err("an interrupted provider outcome needs explicit acknowledgement");

        assert_eq!(error.code, ShellErrorCode::PermissionDenied);
        assert!(error.recoverable);
    }

    #[test]
    fn acknowledged_interrupted_memory_job_retry_routes_to_core() {
        let root = tempdir().expect("temporary data root");
        let shell = ShellApi::open(CoreConfig::new(root.path())).expect("open Shell");

        let error = shell
            .retry_interrupted_memory_job(super::RetryInterruptedMemoryJobInput {
                conversation_id: "conversation:synthetic".to_owned(),
                branch_id: "branch:synthetic".to_owned(),
                memory_job_id: "synthetic.missing-memory-job".to_owned(),
                expected_revision: 2,
                acknowledge_unknown_outcome: true,
            })
            .expect_err("acknowledged retry must reach Core's durable queue");

        assert_eq!(error.code, ShellErrorCode::NotFound);
        assert!(!error.recoverable);
    }

    #[test]
    fn interrupted_memory_job_retry_input_rejects_unknown_fields() {
        let error =
            serde_json::from_value::<super::RetryInterruptedMemoryJobInput>(serde_json::json!({
                "memory_job_id": "synthetic.interrupted-memory-job",
                "expected_revision": 2,
                "acknowledge_unknown_outcome": true,
                "generic_execute": "forbidden"
            }))
            .expect_err("unknown fields must be rejected");

        assert!(error.to_string().contains("unknown field"));
    }
}
