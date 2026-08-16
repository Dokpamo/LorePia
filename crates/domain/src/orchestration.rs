use std::collections::HashMap;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::{
    CapabilityKey, ConversationBranchId, ConversationId, GenerationPresetId, MessageId,
    ModelRouteId, content::Sha256Digest,
};
use uuid::Uuid;

macro_rules! string_id {
    ($($name:ident),+ $(,)?) => {$(
        #[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
        #[serde(transparent)]
        pub struct $name(pub String);

        impl $name {
            pub fn as_str(&self) -> &str { &self.0 }
        }

        impl From<&str> for $name {
            fn from(value: &str) -> Self { Self(value.to_owned()) }
        }

        impl From<String> for $name {
            fn from(value: String) -> Self { Self(value) }
        }
    )+};
}

string_id!(
    PromptPresetId,
    PromptBlockId,
    ControlId,
    VariableId,
    TaskProfileId,
    MemoryProfileId,
    MemoryRecordId,
    MemoryJobId,
    SummarySchemaId,
    EmbeddingRef,
    KnowledgeBookId,
    KnowledgeEntryId,
    TransformSetId,
    TransformRuleId,
    InteractionRuleSetId,
    InteractionRuleId,
    InteractionProposalRecordId,
    ContentModuleId,
    ModuleBindingId,
    ModuleRevisionId,
    CacheBoundaryId,
    AssetId,
    PackageId,
    LocalUserId,
    PersonaId,
);

impl LocalUserId {
    pub fn new() -> Self {
        Self(Uuid::new_v4().to_string())
    }
}

impl Default for LocalUserId {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SourceKind {
    ApplicationBuiltIn,
    UserCreated,
    ImportedStandard,
    ImportedPackage,
    Generated,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Provenance {
    pub source_kind: SourceKind,
    pub source_id: Option<String>,
    pub source_hash: Option<String>,
    pub author: Option<String>,
    pub license: Option<String>,
    pub imported_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(
    tag = "type",
    content = "value",
    rename_all = "snake_case",
    deny_unknown_fields
)]
pub enum VariableValue {
    Bool(bool),
    Integer(i64),
    Decimal(f64),
    Text(String),
    Enum(String),
    StringList(Vec<String>),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VariableType {
    Bool,
    Integer,
    Decimal,
    Text,
    Enum,
    StringList,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VariableScope {
    App,
    User,
    Persona,
    Character,
    Conversation,
    Branch,
    Session,
    Turn,
    Module,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct VariableRef {
    pub scope: VariableScope,
    pub namespace: Option<ContentModuleId>,
    pub id: VariableId,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct VariableMap {
    pub values: Vec<VariableBinding>,
}

impl VariableMap {
    pub fn get(&self, key: &VariableRef) -> Option<&VariableValue> {
        self.values
            .iter()
            .find(|binding| &binding.variable == key)
            .map(|binding| &binding.value)
    }

    pub fn insert(&mut self, variable: VariableRef, value: VariableValue) {
        if let Some(binding) = self
            .values
            .iter_mut()
            .find(|binding| binding.variable == variable)
        {
            binding.value = value;
        } else {
            self.values.push(VariableBinding { variable, value });
            self.values
                .sort_by(|left, right| left.variable.cmp(&right.variable));
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct VariableBinding {
    pub variable: VariableRef,
    pub value: VariableValue,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case", deny_unknown_fields)]
pub enum ConditionExpr {
    True,
    False,
    Equals {
        variable: VariableRef,
        value: VariableValue,
    },
    NotEquals {
        variable: VariableRef,
        value: VariableValue,
    },
    GreaterThan {
        variable: VariableRef,
        value: f64,
    },
    Contains {
        variable: VariableRef,
        value: String,
    },
    Exists {
        variable: VariableRef,
    },
    ModelSupports {
        capability: CapabilityKey,
    },
    All {
        expressions: Vec<ConditionExpr>,
    },
    Any {
        expressions: Vec<ConditionExpr>,
    },
    Not {
        expression: Box<ConditionExpr>,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum ValueExpr {
    Literal { value: VariableValue },
    Variable { variable: VariableRef },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SafeTemplate {
    pub parts: Vec<TemplatePart>,
    pub max_output_chars: u32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum TemplatePart {
    Text {
        value: String,
    },
    Variable {
        variable: VariableRef,
    },
    BuiltIn {
        value: BuiltInTemplateValue,
    },
    Slot {
        name: String,
    },
    Join {
        variable: VariableRef,
        separator: String,
    },
    Conditional {
        condition: ConditionExpr,
        then_template: Box<SafeTemplate>,
        else_template: Option<Box<SafeTemplate>>,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BuiltInTemplateValue {
    CharacterName,
    UserName,
    PersonaName,
    PersonaDescription,
    CurrentDate,
    CurrentTime,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SafeRegex {
    pub pattern: String,
    pub case_insensitive: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RoleHint {
    System,
    Developer,
    User,
    Assistant,
    ProviderDefault,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InstructionAuthority {
    Application,
    Creator,
    User,
    Conversation,
    ImportedContent,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PromptBlockKind {
    StaticInstruction,
    CharacterIdentity,
    CharacterDescription,
    CharacterPersonality,
    Scenario,
    UserPersona,
    DialogueExamples,
    WorldKnowledge,
    RetrievedMemory,
    ConversationSummary,
    HistorySlice,
    LatestUserTurn,
    AuthorNote,
    PostHistoryInstruction,
    AssistantPrefill,
    GroupContext,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PlacementZone {
    ApplicationPolicy,
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

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum HistorySelector {
    All,
    BeforeRecentTurns { recent_turns: u32 },
    RecentTurns { count: u32 },
    ExcludingLatestUser { count: u32 },
    MessageRange { start: MessageId, end: MessageId },
    SinceSummary { summary_id: MemoryRecordId },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TokenPolicy {
    pub priority: u16,
    pub min_tokens: Option<u32>,
    pub max_tokens: Option<u32>,
    pub reserve_tokens: Option<u32>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OverflowPolicy {
    Reject,
    DropBlock,
    TrimHead,
    TrimTail,
    KeepLatestItems,
    Summarize,
    ReduceKnowledgeEntries,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MergePolicy {
    SeparateMessage,
    MergeWithPreviousSameRole,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum BlockSource {
    Template,
    CharacterField { field: CharacterField },
    History,
    LatestUser,
    SelectedKnowledge,
    SelectedMemory,
    ConversationSummary,
    AuthorNote,
    UserPersona,
    GroupContext,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CharacterField {
    Name,
    Description,
    Personality,
    Scenario,
    FirstMessage,
    DialogueExamples,
    SystemInstruction,
    PostHistoryInstruction,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PromptBlock {
    pub id: PromptBlockId,
    pub name: String,
    pub kind: PromptBlockKind,
    pub enabled: bool,
    pub role_hint: RoleHint,
    pub authority: InstructionAuthority,
    pub template: Option<SafeTemplate>,
    pub condition: Option<ConditionExpr>,
    pub source: BlockSource,
    pub placement_zone: PlacementZone,
    pub history_selector: Option<HistorySelector>,
    pub token_policy: TokenPolicy,
    pub overflow_policy: OverflowPolicy,
    pub merge_policy: MergePolicy,
    pub provenance: Provenance,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PromptPreset {
    pub id: PromptPresetId,
    pub name: String,
    pub schema_version: u32,
    pub blocks: Vec<PromptBlock>,
    pub controls: Vec<ControlSpec>,
    pub default_values: VariableMap,
    pub default_generation_preset_id: Option<GenerationPresetId>,
    pub memory_profile_id: Option<MemoryProfileId>,
    pub knowledge_book_ids: Vec<KnowledgeBookId>,
    pub transform_set_ids: Vec<TransformSetId>,
    pub module_ids: Vec<ContentModuleId>,
    pub cache_boundaries: Vec<CacheBoundary>,
    pub metadata: PresetMetadata,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PresetMetadata {
    pub description: String,
    pub tags: Vec<String>,
    pub provenance: Provenance,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub local_override_of: Option<PromptPresetId>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ControlKind {
    Toggle,
    Select,
    MultiSelect,
    Text,
    Number,
    Slider,
    Section,
    Caption,
    Divider,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ControlOption {
    pub value: VariableValue,
    pub label: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ControlSpec {
    pub id: ControlId,
    pub label: String,
    pub description: String,
    pub kind: ControlKind,
    pub value_type: Option<VariableType>,
    pub variable: Option<VariableRef>,
    pub default_value: Option<VariableValue>,
    pub options: Vec<ControlOption>,
    pub minimum: Option<f64>,
    pub maximum: Option<f64>,
    pub step: Option<f64>,
    pub visible_when: Option<ConditionExpr>,
    pub scope: VariableScope,
    pub sensitive: bool,
    pub requires_regeneration: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum CacheRoleFilter {
    All,
    SystemLike,
    ExactRole { role: RoleHint },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CacheTtl {
    ProviderDefault,
    Short,
    Long,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CacheMode {
    Automatic,
    Explicit,
    Disabled,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CacheBoundary {
    pub id: CacheBoundaryId,
    pub after_block_id: PromptBlockId,
    pub role_filter: CacheRoleFilter,
    pub ttl: CacheTtl,
    pub mode: CacheMode,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AuxiliaryTaskKind {
    MemorySummary,
    MemoryEmbedding,
    Translation,
    EmotionClassification,
    StateExtraction,
    ImagePrompt,
    TitleGeneration,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RateLimit {
    pub requests: u32,
    pub per_seconds: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TaskProfile {
    pub id: TaskProfileId,
    pub kind: AuxiliaryTaskKind,
    pub route_id: ModelRouteId,
    pub generation_preset_id: GenerationPresetId,
    pub fallback_route_ids: Vec<ModelRouteId>,
    /// Exact output width for provider-native memory embeddings.
    ///
    /// This is required only for [`AuxiliaryTaskKind::MemoryEmbedding`].
    #[serde(default)]
    pub embedding_dimensions: Option<u32>,
    pub timeout_ms: u64,
    pub rate_limit: RateLimit,
    pub concurrency_limit: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TokenBudget {
    pub max_tokens: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MemoryKind {
    EpisodicEvent,
    CharacterFact,
    RelationshipChange,
    UserPreference,
    WorldState,
    UnresolvedThread,
    ConversationSummary,
    CreatorPinned,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct VersionedJson {
    pub schema_version: u32,
    pub value: serde_json::Value,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MemoryRecord {
    pub id: MemoryRecordId,
    pub conversation_id: ConversationId,
    pub branch_id: ConversationBranchId,
    pub source_start_message_id: MessageId,
    pub source_end_message_id: MessageId,
    pub kind: MemoryKind,
    pub title: String,
    pub summary: String,
    pub structured_data: VersionedJson,
    pub importance: u8,
    pub keywords: Vec<String>,
    pub embedding_ref: Option<EmbeddingRef>,
    pub pinned: bool,
    pub excluded_from_conversation: bool,
    pub excluded_from_character: bool,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub invalidated_at: Option<DateTime<Utc>>,
    pub provenance: Provenance,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MemoryProfile {
    pub id: MemoryProfileId,
    pub name: String,
    pub schema_version: u32,
    pub summary_task: TaskProfileId,
    pub embedding_task: Option<TaskProfileId>,
    pub turns_per_summary: u32,
    pub recent_raw_budget: TokenBudget,
    pub episodic_budget: TokenBudget,
    pub semantic_budget: TokenBudget,
    pub retrieval_count: u32,
    pub recency_weight: f32,
    pub similarity_weight: f32,
    pub importance_weight: f32,
    pub preserve_invalidated_records: bool,
    pub summary_schema: SummarySchemaId,
    pub provenance: Provenance,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MemoryJobKind {
    Summary,
    Embedding,
    InvalidateRange,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MemoryJobStatus {
    Queued,
    Running,
    Interrupted,
    Succeeded,
    Failed,
    Cancelled,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MemoryJob {
    pub id: MemoryJobId,
    pub idempotency_key: String,
    pub kind: MemoryJobKind,
    pub conversation_id: ConversationId,
    pub branch_id: ConversationBranchId,
    pub source_start_message_id: MessageId,
    pub source_end_message_id: MessageId,
    pub status: MemoryJobStatus,
    pub attempt: u32,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub error_code: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum KnowledgePlacement {
    RetrievedContext,
    BeforeOlderHistory,
    BeforeRecentHistory,
    PostHistory,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum ActivationRule {
    Always,
    Manual,
    Keyword {
        primary: Vec<String>,
        secondary: Vec<String>,
        selective: bool,
        case_sensitive: bool,
        whole_word: bool,
    },
    Regex {
        patterns: Vec<SafeRegex>,
    },
    Semantic {
        threshold: f32,
        top_k: u32,
    },
    Condition {
        expression: ConditionExpr,
    },
    Any {
        rules: Vec<ActivationRule>,
    },
    All {
        rules: Vec<ActivationRule>,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct KnowledgeBook {
    pub id: KnowledgeBookId,
    pub name: String,
    pub schema_version: u32,
    pub entries: Vec<KnowledgeEntry>,
    pub scan_depth: u32,
    pub token_budget: TokenBudget,
    pub recursive: bool,
    pub max_recursion_depth: u32,
    pub provenance: Provenance,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct KnowledgeEntry {
    pub id: KnowledgeEntryId,
    pub book_id: KnowledgeBookId,
    pub name: String,
    pub content: String,
    pub enabled: bool,
    pub activation: ActivationRule,
    pub priority: i32,
    pub importance: u8,
    pub placement: KnowledgePlacement,
    pub token_policy: TokenPolicy,
    pub parent_id: Option<KnowledgeEntryId>,
    pub activation_probability_basis_points: u16,
    pub provenance: Provenance,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SemanticKnowledgeScore {
    pub entry_id: KnowledgeEntryId,
    pub score: f32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum KnowledgeActivationReason {
    Always,
    Manual,
    Keyword { matched: String },
    Regex { pattern: String },
    Semantic { score_millionths: u32 },
    Condition,
    Recursive { parent_id: KnowledgeEntryId },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct KnowledgeSelectionEvidence {
    pub entry_id: KnowledgeEntryId,
    pub selected: bool,
    pub reasons: Vec<KnowledgeActivationReason>,
    pub estimated_tokens: u32,
    pub exclusion_reason: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TransformPhase {
    UserInputForRequest,
    ResolvedPrompt,
    ProviderOutputCanonical,
    DisplayOnly,
    MemoryInput,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TransformSet {
    pub id: TransformSetId,
    pub name: String,
    pub schema_version: u32,
    pub enabled: bool,
    /// Immutable package-author intent retained while `enabled` is forced
    /// false at import. Runtime still requires an exact approved module plan.
    #[serde(default)]
    pub imported_author_enabled: bool,
    pub rules: Vec<TransformRule>,
    pub max_rules_per_phase: u32,
    pub max_output_chars: u32,
    pub provenance: Provenance,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TransformRule {
    pub id: TransformRuleId,
    pub name: String,
    pub enabled: bool,
    pub imported_enabled: bool,
    /// Immutable package-author intent retained independently from the
    /// operational import-approval gate above.
    #[serde(default)]
    pub imported_author_enabled: bool,
    pub phase: TransformPhase,
    pub order: i32,
    pub pattern: SafeRegex,
    pub replacement: String,
    pub condition: Option<ConditionExpr>,
    pub max_replacements: u32,
    pub input_limit: u32,
    pub output_limit: u32,
    pub provenance: Provenance,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TransformTrace {
    pub rule_id: TransformRuleId,
    pub applied: bool,
    pub replacements: u32,
    pub input_chars: u32,
    pub output_chars: u32,
    pub error: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum InteractionEvent {
    ConversationOpened,
    ConversationStarted,
    BeforeGeneration,
    AfterGeneration,
    MessageCommitted,
    UserAction { action_id: String },
    VariableChanged { variable: VariableRef },
    KnowledgeActivated { entry_id: KnowledgeEntryId },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ChoiceSpec {
    pub id: String,
    pub label: String,
    pub value: VariableValue,
    pub enabled_when: Option<ConditionExpr>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProposalSpec {
    pub id: String,
    pub title: String,
    pub body: SafeTemplate,
    pub expires_after_seconds: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DiceExpression {
    pub count: u16,
    pub sides: u32,
    pub modifier: i64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UiRegion {
    Message,
    Background,
    CharacterPortrait,
    StatusPanel,
    Audio,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum InteractionAction {
    SetVariable {
        target: VariableRef,
        value: ValueExpr,
    },
    IncrementVariable {
        target: VariableRef,
        amount: i64,
    },
    ActivateKnowledge {
        entry_id: KnowledgeEntryId,
    },
    ShowAsset {
        asset_id: AssetId,
        region: UiRegion,
    },
    PlayAudio {
        asset_id: AssetId,
    },
    PresentChoices {
        choices: Vec<ChoiceSpec>,
    },
    AppendVisibleSystemEvent {
        text: SafeTemplate,
    },
    RollDice {
        expression: DiceExpression,
        target: Option<VariableRef>,
    },
    RequestUserApproval {
        proposal: ProposalSpec,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InteractionRule {
    pub id: InteractionRuleId,
    pub name: String,
    pub enabled: bool,
    /// Immutable package-author intent retained while `enabled` is forced
    /// false at import. Runtime still requires an exact approved module plan.
    #[serde(default)]
    pub imported_author_enabled: bool,
    pub event: InteractionEvent,
    pub condition: Option<ConditionExpr>,
    pub actions: Vec<InteractionAction>,
    pub priority: i32,
    pub stop_after_match: bool,
    pub provenance: Provenance,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InteractionRuleSet {
    pub id: InteractionRuleSetId,
    pub name: String,
    pub schema_version: u32,
    pub rules: Vec<InteractionRule>,
    pub max_actions_per_event: u32,
    pub provenance: Provenance,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InteractionState {
    pub variables: VariableMap,
    pub manually_active_knowledge: Vec<KnowledgeEntryId>,
    #[serde(default)]
    pub proposals: Vec<InteractionProposalRecord>,
    pub revision: u64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum InteractionEffect {
    VariableSet {
        target: VariableRef,
        previous: Option<VariableValue>,
        value: VariableValue,
    },
    KnowledgeActivated {
        entry_id: KnowledgeEntryId,
    },
    AssetShown {
        asset_id: AssetId,
        region: UiRegion,
    },
    AudioRequested {
        asset_id: AssetId,
    },
    ChoicesPresented {
        choices: Vec<ChoiceSpec>,
    },
    VisibleSystemEvent {
        text: String,
    },
    DiceRolled {
        expression: DiceExpression,
        rolls: Vec<u32>,
        total: i64,
        target: Option<VariableRef>,
    },
    ApprovalRequested {
        rule_set_id: InteractionRuleSetId,
        rule_id: InteractionRuleId,
        proposal_id: String,
        title: String,
        body: String,
        expires_after_seconds: Option<u32>,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InteractionProposalStatus {
    Pending,
    Approved,
    Rejected,
    Expired,
}

/// Durable, inert approval request created from an [`InteractionEffect`].
///
/// Approval never stores or accepts an arbitrary action. The interaction
/// engine derives `UserAction(proposal_id)` from this persisted record, while
/// Core atomically saves the returned state under its expected revision.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InteractionProposalRecord {
    pub id: InteractionProposalRecordId,
    pub rule_set_id: InteractionRuleSetId,
    pub rule_id: InteractionRuleId,
    pub proposal_id: String,
    pub title: String,
    pub body: String,
    pub status: InteractionProposalStatus,
    pub source_interaction_state_revision: u64,
    pub requested_at_epoch_seconds: i64,
    pub expires_at_epoch_seconds: Option<i64>,
    pub decided_at_epoch_seconds: Option<i64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InteractionProposalDecision {
    Approve,
    Reject,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ModuleScope {
    App,
    User,
    Persona,
    Character,
    Conversation,
    Branch,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ModuleRevisionResolutionMode {
    #[default]
    Active,
    Pinned,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ContentCapability {
    PromptFragments,
    Knowledge,
    Variables,
    Transforms,
    DeclarativeInteractions,
    ImageAssets,
    AudioAssets,
    VideoAssets,
    AttachmentAssets,
    HighRiskAssets,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PackageMetadata {
    pub author: Option<String>,
    pub license: String,
    pub redistribution_allowed: bool,
    pub homepage: Option<String>,
    pub description: String,
    pub tags: Vec<String>,
    pub provenance: Provenance,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ContentModule {
    pub id: ContentModuleId,
    pub name: String,
    pub version: String,
    pub schema_version: u32,
    pub prompt_fragments: Vec<PromptBlock>,
    pub knowledge_book_ids: Vec<KnowledgeBookId>,
    pub control_specs: Vec<ControlSpec>,
    pub transform_set_ids: Vec<TransformSetId>,
    pub interaction_rule_set_ids: Vec<InteractionRuleSetId>,
    pub asset_ids: Vec<AssetId>,
    /// Author intent for selected declarative transform/interaction
    /// components. This is inert until a full-context module plan explicitly
    /// approves the corresponding runtime overlay.
    #[serde(default)]
    pub imported_components_enabled: bool,
    pub required_capabilities: Vec<ContentCapability>,
    pub metadata: PackageMetadata,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ModuleBinding {
    pub id: ModuleBindingId,
    pub module_id: ContentModuleId,
    pub scope: ModuleScope,
    pub target_id: Option<String>,
    /// Owning conversation for a branch-scoped target.
    #[serde(default)]
    pub conversation_id: Option<ConversationId>,
    #[serde(default)]
    pub priority: i32,
    #[serde(default)]
    pub resolution_mode: ModuleRevisionResolutionMode,
    #[serde(default)]
    pub pinned_revision_id: Option<ModuleRevisionId>,
    pub enabled: bool,
    pub approved: bool,
    #[serde(default)]
    pub package_import_approval_id: Option<String>,
    #[serde(default)]
    pub activation_approval_id: Option<String>,
    #[serde(default)]
    pub activation_review_sha256: Option<Sha256Digest>,
    #[serde(default)]
    pub activation_plan_sha256: Option<Sha256Digest>,
    #[serde(default)]
    pub variable_overrides: VariableMap,
    /// Exact immutable revision resolved from `resolution_mode` by Core.
    pub revision_id: ModuleRevisionId,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum ModuleComponentRef {
    PromptBlock { id: PromptBlockId },
    Control { id: ControlId },
    KnowledgeBook { id: KnowledgeBookId },
    TransformSet { id: TransformSetId },
    InteractionRuleSet { id: InteractionRuleSetId },
    Asset { id: AssetId },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ModuleConflict {
    pub component: ModuleComponentRef,
    pub candidates: Vec<ModuleConflictCandidate>,
    pub reason: String,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ModuleConflictCandidate {
    pub module_id: ContentModuleId,
    pub revision_id: ModuleRevisionId,
    pub component_hash: Sha256Digest,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ModuleConflictResolution {
    pub component: ModuleComponentRef,
    /// Exact reviewed candidates. Apply fails if any revision or hash changed.
    pub expected_candidates: Vec<ModuleConflictCandidate>,
    /// `None` disables every candidate for this component.
    pub selected: Option<ModuleConflictCandidate>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ContentModuleRevision {
    pub id: ModuleRevisionId,
    pub module_id: ContentModuleId,
    pub version: String,
    pub source_hash: Sha256Digest,
    pub previous_revision_id: Option<ModuleRevisionId>,
    pub component_hashes: Vec<ComponentHash>,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ComponentHash {
    pub component: ModuleComponentRef,
    pub sha256: Sha256Digest,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PackageManifest {
    pub format: String,
    pub format_version: u32,
    pub package_id: PackageId,
    pub name: String,
    pub version: String,
    pub author: Option<String>,
    pub license: String,
    pub redistribution_allowed: bool,
    pub required_app_version: Option<String>,
    pub required_capabilities: Vec<ContentCapability>,
    pub content_hashes: Vec<PackageContentHash>,
    pub signature: Option<PackageSignature>,
    pub provenance: Provenance,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PackageContentHash {
    pub logical_path: String,
    pub sha256: Sha256Digest,
    pub size_bytes: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PackageSignature {
    pub algorithm: String,
    pub key_id: String,
    pub signature_base64: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PackageImportSelection {
    pub prompt_block_ids: Vec<PromptBlockId>,
    pub knowledge_book_ids: Vec<KnowledgeBookId>,
    pub transform_set_ids: Vec<TransformSetId>,
    pub interaction_rule_set_ids: Vec<InteractionRuleSetId>,
    pub asset_ids: Vec<AssetId>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PackageImportReview {
    pub package_id: PackageId,
    pub source_sha256: Sha256Digest,
    pub prompt_block_count: u32,
    pub estimated_prompt_tokens: u32,
    pub knowledge_entry_count: u32,
    pub transform_rule_count: u32,
    pub asset_count: u32,
    pub total_asset_bytes: u64,
    pub control_count: u32,
    pub action_kinds: Vec<String>,
    pub required_capabilities: Vec<ContentCapability>,
    pub conflicts: Vec<ModuleConflict>,
    pub unsupported_elements: Vec<String>,
    pub warnings: Vec<String>,
    pub imported_components_enabled: bool,
    pub redistribution_allowed: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CharacterPromptContent {
    pub character_id: String,
    pub name: String,
    pub aliases: Vec<String>,
    pub description: String,
    pub personality: String,
    pub scenario: String,
    pub first_message: String,
    pub dialogue_examples: Vec<String>,
    pub system_instruction: String,
    pub post_history_instruction: String,
    pub alternate_greetings: Vec<String>,
    pub knowledge_book_ids: Vec<KnowledgeBookId>,
    pub asset_ids: Vec<AssetId>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PersonaPromptContent {
    pub persona_id: PersonaId,
    pub name: String,
    pub description: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Persona {
    pub id: PersonaId,
    pub name: String,
    pub description: String,
    pub schema_version: u32,
    pub provenance: Provenance,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// Active local persona selected for one conversation.
///
/// Storage wraps this value in a CAS revision; absence means that the room
/// uses no persona. The selection is conversation-scoped so separate rooms
/// never share mutable persona state implicitly.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConversationPersonaSelection {
    pub conversation_id: ConversationId,
    pub persona_id: PersonaId,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PromptMessageRole {
    System,
    User,
    Assistant,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PromptConversationMessage {
    pub id: MessageId,
    pub branch_id: ConversationBranchId,
    pub role: PromptMessageRole,
    pub content: String,
    pub turn_index: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SelectedKnowledge {
    pub entry_id: KnowledgeEntryId,
    pub content: String,
    pub placement: KnowledgePlacement,
    pub priority: i32,
    pub evidence: Vec<KnowledgeActivationReason>,
    pub provenance: Provenance,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SelectedMemory {
    pub record_id: MemoryRecordId,
    pub branch_id: ConversationBranchId,
    pub content: String,
    pub score_millionths: u32,
    pub reason: String,
    pub provenance: Provenance,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PromptResolutionContext {
    pub conversation_id: ConversationId,
    pub branch_id: ConversationBranchId,
    pub character: CharacterPromptContent,
    pub persona: Option<PersonaPromptContent>,
    pub user_name: String,
    pub messages: Vec<PromptConversationMessage>,
    pub latest_user_message_id: MessageId,
    pub selected_knowledge: Vec<SelectedKnowledge>,
    pub selected_memory: Vec<SelectedMemory>,
    pub summary_boundaries: Vec<SummaryBoundary>,
    pub conversation_summary: Option<String>,
    pub author_note: Option<String>,
    pub group_context: Option<String>,
    pub variables: VariableMap,
    pub slots: Vec<TemplateSlot>,
    pub current_date: String,
    pub current_time: String,
    pub supported_capabilities: Vec<CapabilityKey>,
    pub session_seed: Option<u64>,
    /// Core-owned, content-free identity of every mutable source used to
    /// materialize this exact prompt. Pure resolver callers may omit it; Core
    /// generation plans always provide and seal it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context_snapshot: Option<PromptContextSnapshotV1>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SummaryBoundary {
    pub summary_id: MemoryRecordId,
    pub end_message_id: MessageId,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TemplateSlot {
    pub name: String,
    pub value: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PromptContextBindingEvidence {
    pub binding_id: String,
    pub binding_revision: u64,
    pub document_sha256: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PromptContextPersonaEvidence {
    pub selection_revision: u64,
    pub persona_id: PersonaId,
    pub persona_revision_id: String,
    pub persona_sha256: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PromptSummarySourceEvidence {
    pub summary_id: MemoryRecordId,
    pub record_branch_id: ConversationBranchId,
    pub source_start_message_id: MessageId,
    pub source_end_message_id: MessageId,
    pub state_revision: u64,
    pub active_revision_id: String,
    pub active_revision_sha256: String,
}

/// Content-free, hash-sealed prompt source identity captured at one exact
/// branch head. Values such as notes, summaries, persona text, and template
/// slots remain only in the resolved prompt; this snapshot carries identities
/// and hashes needed for append-time compare-and-swap verification.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PromptContextSnapshotV1 {
    pub schema_version: u32,
    pub conversation_id: ConversationId,
    pub source_branch_id: ConversationBranchId,
    pub context_head_message_id: Option<MessageId>,
    pub local_user_id_sha256: String,
    pub binding: Option<PromptContextBindingEvidence>,
    pub persona: Option<PromptContextPersonaEvidence>,
    pub conversation_summary_id: Option<MemoryRecordId>,
    pub summaries: Vec<PromptSummarySourceEvidence>,
    pub snapshot_sha256: String,
}

#[derive(Serialize)]
struct PromptContextSnapshotHashMaterial<'a> {
    schema_version: u32,
    conversation_id: &'a ConversationId,
    source_branch_id: &'a ConversationBranchId,
    context_head_message_id: Option<&'a MessageId>,
    local_user_id_sha256: &'a str,
    binding: Option<&'a PromptContextBindingEvidence>,
    persona: Option<&'a PromptContextPersonaEvidence>,
    conversation_summary_id: Option<&'a MemoryRecordId>,
    summaries: &'a [PromptSummarySourceEvidence],
}

/// Computes the canonical digest used by plan hashing and the storage append
/// recheck. The digest never contains prompt text.
pub fn prompt_context_snapshot_sha256(
    snapshot: &PromptContextSnapshotV1,
) -> Result<String, OrchestrationValidationError> {
    let encoded = serde_json::to_vec(&PromptContextSnapshotHashMaterial {
        schema_version: snapshot.schema_version,
        conversation_id: &snapshot.conversation_id,
        source_branch_id: &snapshot.source_branch_id,
        context_head_message_id: snapshot.context_head_message_id.as_ref(),
        local_user_id_sha256: &snapshot.local_user_id_sha256,
        binding: snapshot.binding.as_ref(),
        persona: snapshot.persona.as_ref(),
        conversation_summary_id: snapshot.conversation_summary_id.as_ref(),
        summaries: &snapshot.summaries,
    })
    .map_err(|error| {
        OrchestrationValidationError::new(
            "context.context_snapshot",
            format!("cannot encode prompt context snapshot: {error}"),
        )
    })?;
    Ok(hex::encode(Sha256::digest(encoded)))
}

/// Computes the content-free, domain-separated identity sealed into prompt
/// context snapshots. The raw repository-owned local user identifier must
/// never be copied into prompt text, provider metadata, or diagnostics.
#[must_use]
pub fn prompt_local_user_id_sha256(local_user_id: &LocalUserId) -> String {
    let mut hasher = Sha256::new();
    hasher.update(b"lorepia.prompt-context.local-user.v1\0");
    hasher.update(local_user_id.as_str().as_bytes());
    format!("{digest:x}", digest = hasher.finalize())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderMessageRole {
    System,
    Developer,
    User,
    Assistant,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UnsupportedRolePolicy {
    Reject,
    MapDeveloperToSystem,
    MapSystemToDeveloper,
    UseProviderDefault,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProviderPromptContract {
    pub supported_roles: Vec<ProviderMessageRole>,
    pub provider_default_role: ProviderMessageRole,
    pub unsupported_role_policy: UnsupportedRolePolicy,
    pub supports_explicit_cache: bool,
    pub max_cache_boundaries: u32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PromptResolveRequest {
    pub preset: PromptPreset,
    pub context: PromptResolutionContext,
    pub provider: ProviderPromptContract,
    pub generation_preset_id: Option<GenerationPresetId>,
    pub max_context_tokens: u32,
    pub reserved_output_tokens: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ResolvedPromptMessage {
    pub sequence: u32,
    pub block_id: PromptBlockId,
    pub block_kind: PromptBlockKind,
    pub requested_role: RoleHint,
    pub effective_role: ProviderMessageRole,
    pub authority: InstructionAuthority,
    pub content: String,
    pub estimated_tokens: u32,
    pub source_message_ids: Vec<MessageId>,
    pub provenance: Provenance,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CacheDirectiveStatus {
    Applied,
    IgnoredUnsupported,
    IgnoredLimit,
    RemovedWithBlock,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ResolvedCacheDirective {
    pub boundary_id: CacheBoundaryId,
    pub after_block_id: PromptBlockId,
    pub after_message_sequence: Option<u32>,
    pub role_filter: CacheRoleFilter,
    pub ttl: CacheTtl,
    pub mode: CacheMode,
    pub status: CacheDirectiveStatus,
    pub explanation: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BlockResolutionStatus {
    Included,
    ConditionFalse,
    Disabled,
    Empty,
    DroppedForBudget,
    TrimmedHead,
    TrimmedTail,
    ReducedItems,
    Summarized,
}

/// Safe, content-free identity for the source of one resolved prompt block.
///
/// `source_revision` is populated by Core when the block came from an
/// immutable stored preset/module revision. Built-in and transient blocks may
/// legitimately have no persisted revision.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PromptBlockSourceTrace {
    pub authority: InstructionAuthority,
    pub source_kind: SourceKind,
    pub source_id: Option<String>,
    pub source_revision: Option<String>,
    pub source_hash: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PromptMemorySelectionLane {
    Pinned,
    Semantic,
    Episodic,
}

/// Content-free reasons used to explain deterministic memory ranking.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum PromptMemorySelectionReason {
    Pinned,
    CurrentBranch,
    SharedAncestor {
        source_branch_id: ConversationBranchId,
    },
    Recency {
        score_millionths: u32,
    },
    Similarity {
        score_millionths: u32,
    },
    Importance {
        score_millionths: u32,
    },
}

/// Bounded, content-free selection evidence for a memory candidate considered
/// by the prompt planner.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PromptMemorySelectionEvidence {
    pub record_id: MemoryRecordId,
    pub selected: bool,
    pub lane: Option<PromptMemorySelectionLane>,
    pub rank_millionths: Option<u64>,
    pub estimated_tokens: u32,
    pub reasons: Vec<PromptMemorySelectionReason>,
    pub exclusion_reason: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BlockResolutionTrace {
    pub block_id: PromptBlockId,
    pub block_kind: PromptBlockKind,
    pub source: PromptBlockSourceTrace,
    pub status: BlockResolutionStatus,
    pub original_estimated_tokens: u32,
    pub final_estimated_tokens: u32,
    pub produced_message_count: u32,
    pub explanation: String,
    pub knowledge_evidence: Vec<KnowledgeSelectionEvidence>,
    pub memory_record_ids: Vec<MemoryRecordId>,
    pub memory_evidence: Vec<PromptMemorySelectionEvidence>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RoleMappingTrace {
    pub block_id: PromptBlockId,
    pub requested_role: RoleHint,
    pub effective_role: ProviderMessageRole,
    pub explanation: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OverflowTrace {
    pub block_id: PromptBlockId,
    pub policy: OverflowPolicy,
    pub tokens_before: u32,
    pub tokens_after: u32,
    pub explanation: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PromptResolutionTrace {
    pub estimator_id: String,
    pub session_seed: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context_snapshot: Option<PromptContextSnapshotV1>,
    pub max_context_tokens: u32,
    pub reserved_output_tokens: u32,
    pub available_input_tokens: u32,
    pub estimated_input_tokens: u32,
    pub blocks: Vec<BlockResolutionTrace>,
    pub role_mappings: Vec<RoleMappingTrace>,
    pub overflow: Vec<OverflowTrace>,
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PromptPreview {
    pub effective_messages: Vec<ResolvedPromptMessage>,
    pub cache_directives: Vec<ResolvedCacheDirective>,
    pub estimated_input_tokens: u32,
    pub available_input_tokens: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ResolvedPromptPlan {
    pub schema_version: u32,
    pub preset_id: PromptPresetId,
    pub generation_preset_id: Option<GenerationPresetId>,
    pub effective_messages: Vec<ResolvedPromptMessage>,
    pub cache_directives: Vec<ResolvedCacheDirective>,
    pub trace: PromptResolutionTrace,
    pub preview: PromptPreview,
    pub plan_hash: String,
}

pub const PROMPT_PLAN_SCHEMA_VERSION: u32 = 1;
pub const MAX_PROMPT_BLOCKS: usize = 256;
pub const MAX_PROMPT_CONTROLS: usize = 256;
pub const MAX_VARIABLES: usize = 512;
pub const MAX_TEMPLATE_NODES: usize = 1_024;
pub const MAX_TEMPLATE_DEPTH: usize = 16;
pub const MAX_CONDITION_NODES: usize = 512;
pub const MAX_CONDITION_DEPTH: usize = 16;
pub const MAX_TEMPLATE_OUTPUT_CHARS: u32 = 262_144;
pub const MAX_BLOCK_TEXT_CHARS: usize = 262_144;
pub const MAX_HISTORY_MESSAGES: usize = 16_384;
pub const MAX_KNOWLEDGE_ENTRIES: usize = 4_096;
pub const MAX_MEMORY_RECORDS: usize = 4_096;
pub const MAX_TRANSFORM_RULES: usize = 512;
pub const MAX_INTERACTION_RULES: usize = 512;
pub const MAX_INTERACTION_PROPOSALS: usize = 256;
/// Canonical Unicode-scalar bound for text delivered to a native client.
pub const MAX_INTERACTION_NATIVE_TEXT_CHARS: usize = 8 * 1_024;
/// Canonical UTF-8 byte bound for text delivered to a native client.
pub const MAX_INTERACTION_NATIVE_TEXT_BYTES: usize = 16 * 1_024;
pub const MAX_INTERACTION_PROPOSAL_TITLE_CHARS: usize = 1_024;
/// Legacy persisted proposal bound. New native delivery uses the stricter
/// scalar-and-byte contract enforced by `validate_interaction_native_text`.
pub const MAX_INTERACTION_PROPOSAL_BODY_CHARS: usize = 16 * 1_024;
pub const MAX_MODULE_COMPONENTS: usize = 8_192;
pub const MAX_PACKAGE_FILES: usize = 16_384;
pub const MAX_SAFE_REGEX_CHARS: usize = 4_096;
pub const MAX_IDENTIFIER_CHARS: usize = 256;
pub const MAX_NAME_CHARS: usize = 512;
pub const MAX_VARIABLE_TEXT_CHARS: usize = 16_384;
pub const MAX_VARIABLE_LIST_ITEMS: usize = 256;
const UNSUPPORTED_SAFE_REGEX_SYNTAX: [&str; 7] =
    ["(?=", "(?!", "(?<=", "(?<!", "\\1", "\\2", "\\k"];

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{path}: {reason}")]
pub struct OrchestrationValidationError {
    pub path: String,
    pub reason: String,
}

impl OrchestrationValidationError {
    fn new(path: impl Into<String>, reason: impl Into<String>) -> Self {
        Self {
            path: path.into(),
            reason: reason.into(),
        }
    }
}

pub trait ValidateOrchestration {
    /// Validates bounded persisted data before it reaches an engine or storage.
    ///
    /// # Errors
    ///
    /// Returns the first contract violation with a stable field path.
    fn validate(&self) -> Result<(), OrchestrationValidationError>;
}

fn validate_id(path: &str, value: &str) -> Result<(), OrchestrationValidationError> {
    validate_text(path, value, 1, MAX_IDENTIFIER_CHARS)
}

fn validate_id_list<'a>(
    path: &str,
    values: impl IntoIterator<Item = &'a str>,
    maximum: usize,
) -> Result<(), OrchestrationValidationError> {
    let mut values = values.into_iter().collect::<Vec<_>>();
    if values.len() > maximum {
        return Err(OrchestrationValidationError::new(
            path,
            format!("must contain at most {maximum} identifiers"),
        ));
    }
    for (index, value) in values.iter().enumerate() {
        validate_id(&format!("{path}[{index}]"), value)?;
    }
    values.sort_unstable();
    if values.windows(2).any(|pair| pair[0] == pair[1]) {
        return Err(OrchestrationValidationError::new(
            path,
            "identifiers must be unique",
        ));
    }
    Ok(())
}

fn validate_text(
    path: &str,
    value: &str,
    minimum_chars: usize,
    maximum_chars: usize,
) -> Result<(), OrchestrationValidationError> {
    let chars = value.chars().count();
    if chars < minimum_chars || chars > maximum_chars {
        return Err(OrchestrationValidationError::new(
            path,
            format!("must contain between {minimum_chars} and {maximum_chars} characters"),
        ));
    }
    if value.chars().any(|character| character == '\0') {
        return Err(OrchestrationValidationError::new(
            path,
            "must not contain NUL",
        ));
    }
    Ok(())
}

/// Validates the single canonical plain-text contract shared by interaction
/// evaluation, durable writes, and native projection.
///
/// This helper deliberately accepts empty text. Callers that require content
/// must enforce that independently so the byte/scalar authority cannot drift.
pub fn validate_interaction_native_text(
    path: &str,
    value: &str,
) -> Result<(), OrchestrationValidationError> {
    if value.len() > MAX_INTERACTION_NATIVE_TEXT_BYTES
        || value.chars().count() > MAX_INTERACTION_NATIVE_TEXT_CHARS
    {
        return Err(OrchestrationValidationError::new(
            path,
            format!(
                "must contain at most {MAX_INTERACTION_NATIVE_TEXT_CHARS} Unicode scalars and \
                 {MAX_INTERACTION_NATIVE_TEXT_BYTES} UTF-8 bytes"
            ),
        ));
    }
    if value
        .chars()
        .any(|character| character.is_control() && !matches!(character, '\n' | '\r' | '\t'))
    {
        return Err(OrchestrationValidationError::new(
            path,
            "contains disallowed control characters",
        ));
    }
    let normalized = value.to_ascii_lowercase();
    if (normalized.contains('<') && normalized.contains('>'))
        || normalized.contains("javascript:")
        || normalized.contains("data:text/html")
    {
        return Err(OrchestrationValidationError::new(
            path,
            "must remain plain native text",
        ));
    }
    Ok(())
}

/// Exhaustively validates every text field that can cross the native
/// interaction-effect boundary. Non-text effect evidence is intentionally
/// preserved and validated by its owning subsystem.
pub fn validate_interaction_effect_native_text(
    effect: &InteractionEffect,
) -> Result<(), OrchestrationValidationError> {
    match effect {
        InteractionEffect::ChoicesPresented { choices } => {
            for (index, choice) in choices.iter().enumerate() {
                validate_interaction_native_text(
                    &format!("interaction_effect.choices[{index}].label"),
                    &choice.label,
                )?;
            }
            Ok(())
        }
        InteractionEffect::VisibleSystemEvent { text } => {
            validate_interaction_native_text("interaction_effect.text", text)
        }
        InteractionEffect::ApprovalRequested { title, body, .. } => {
            validate_interaction_native_text("interaction_effect.title", title)?;
            validate_interaction_native_text("interaction_effect.body", body)
        }
        InteractionEffect::VariableSet { .. }
        | InteractionEffect::KnowledgeActivated { .. }
        | InteractionEffect::AssetShown { .. }
        | InteractionEffect::AudioRequested { .. }
        | InteractionEffect::DiceRolled { .. } => Ok(()),
    }
}

fn validate_provenance(
    provenance: &Provenance,
    path: &str,
) -> Result<(), OrchestrationValidationError> {
    if let Some(source_id) = &provenance.source_id {
        validate_text(&format!("{path}.source_id"), source_id, 1, 1_024)?;
    }
    if let Some(source_hash) = &provenance.source_hash {
        validate_sha256(&format!("{path}.source_hash"), source_hash)?;
    }
    if let Some(author) = &provenance.author {
        validate_text(&format!("{path}.author"), author, 1, MAX_NAME_CHARS)?;
    }
    if let Some(license) = &provenance.license {
        validate_text(&format!("{path}.license"), license, 1, MAX_NAME_CHARS)?;
    }
    Ok(())
}

fn validate_sha256(path: &str, value: &str) -> Result<(), OrchestrationValidationError> {
    if value.len() != 64
        || value.bytes().any(|byte| byte.is_ascii_uppercase())
        || hex::decode(value).is_err()
    {
        return Err(OrchestrationValidationError::new(
            path,
            "must be a lowercase 64-character SHA-256 digest",
        ));
    }
    Ok(())
}

fn validate_variable_ref(
    variable: &VariableRef,
    path: &str,
) -> Result<(), OrchestrationValidationError> {
    validate_id(&format!("{path}.id"), variable.id.as_str())?;
    match (variable.scope, variable.namespace.as_ref()) {
        (VariableScope::Module, Some(namespace)) => {
            validate_id(&format!("{path}.namespace"), namespace.as_str())
        }
        (VariableScope::Module, None) => Err(OrchestrationValidationError::new(
            format!("{path}.namespace"),
            "module variables require a module namespace",
        )),
        (_, Some(_)) => Err(OrchestrationValidationError::new(
            format!("{path}.namespace"),
            "only module variables may carry a module namespace",
        )),
        (_, None) => Ok(()),
    }
}

fn validate_variable_value(
    value: &VariableValue,
    path: &str,
) -> Result<(), OrchestrationValidationError> {
    match value {
        VariableValue::Decimal(value) if !value.is_finite() => Err(
            OrchestrationValidationError::new(path, "decimal must be finite"),
        ),
        VariableValue::Text(value) | VariableValue::Enum(value) => {
            validate_text(path, value, 0, MAX_VARIABLE_TEXT_CHARS)
        }
        VariableValue::StringList(values) => {
            if values.len() > MAX_VARIABLE_LIST_ITEMS {
                return Err(OrchestrationValidationError::new(
                    path,
                    format!("must contain at most {MAX_VARIABLE_LIST_ITEMS} values"),
                ));
            }
            for (index, value) in values.iter().enumerate() {
                validate_text(&format!("{path}[{index}]"), value, 0, MAX_BLOCK_TEXT_CHARS)?;
            }
            Ok(())
        }
        _ => Ok(()),
    }
}

const fn variable_value_type(value: &VariableValue) -> VariableType {
    match value {
        VariableValue::Bool(_) => VariableType::Bool,
        VariableValue::Integer(_) => VariableType::Integer,
        VariableValue::Decimal(_) => VariableType::Decimal,
        VariableValue::Text(_) => VariableType::Text,
        VariableValue::Enum(_) => VariableType::Enum,
        VariableValue::StringList(_) => VariableType::StringList,
    }
}

fn validate_variable_map(
    values: &VariableMap,
    path: &str,
) -> Result<(), OrchestrationValidationError> {
    if values.values.len() > MAX_VARIABLES {
        return Err(OrchestrationValidationError::new(
            path,
            format!("must contain at most {MAX_VARIABLES} variables"),
        ));
    }
    for (index, binding) in values.values.iter().enumerate() {
        validate_variable_ref(&binding.variable, &format!("{path}[{index}].variable"))?;
        validate_variable_value(&binding.value, &format!("{path}[{index}].value"))?;
    }
    let mut keys = values
        .values
        .iter()
        .map(|binding| &binding.variable)
        .collect::<Vec<_>>();
    keys.sort();
    if keys.windows(2).any(|pair| pair[0] == pair[1]) {
        return Err(OrchestrationValidationError::new(
            path,
            "variable references must be unique",
        ));
    }
    Ok(())
}

fn validate_condition(
    expression: &ConditionExpr,
    path: &str,
) -> Result<(), OrchestrationValidationError> {
    fn visit(
        expression: &ConditionExpr,
        path: &str,
        depth: usize,
        nodes: &mut usize,
    ) -> Result<(), OrchestrationValidationError> {
        if depth > MAX_CONDITION_DEPTH {
            return Err(OrchestrationValidationError::new(
                path,
                format!("condition nesting exceeds {MAX_CONDITION_DEPTH}"),
            ));
        }
        *nodes += 1;
        if *nodes > MAX_CONDITION_NODES {
            return Err(OrchestrationValidationError::new(
                path,
                format!("condition nodes exceed {MAX_CONDITION_NODES}"),
            ));
        }
        match expression {
            ConditionExpr::Equals { variable, value }
            | ConditionExpr::NotEquals { variable, value } => {
                validate_variable_ref(variable, &format!("{path}.variable"))?;
                validate_variable_value(value, &format!("{path}.value"))
            }
            ConditionExpr::GreaterThan { variable, value } => {
                validate_variable_ref(variable, &format!("{path}.variable"))?;
                if value.is_finite() {
                    Ok(())
                } else {
                    Err(OrchestrationValidationError::new(
                        format!("{path}.value"),
                        "comparison value must be finite",
                    ))
                }
            }
            ConditionExpr::Contains { variable, value } => {
                validate_variable_ref(variable, &format!("{path}.variable"))?;
                validate_text(&format!("{path}.value"), value, 0, MAX_BLOCK_TEXT_CHARS)
            }
            ConditionExpr::Exists { variable } => {
                validate_variable_ref(variable, &format!("{path}.variable"))
            }
            ConditionExpr::All { expressions } | ConditionExpr::Any { expressions } => {
                if expressions.is_empty() {
                    return Err(OrchestrationValidationError::new(
                        path,
                        "logical conditions must contain at least one expression",
                    ));
                }
                for (index, child) in expressions.iter().enumerate() {
                    visit(child, &format!("{path}[{index}]"), depth + 1, nodes)?;
                }
                Ok(())
            }
            ConditionExpr::Not { expression } => {
                visit(expression, &format!("{path}.not"), depth + 1, nodes)
            }
            ConditionExpr::True | ConditionExpr::False | ConditionExpr::ModelSupports { .. } => {
                Ok(())
            }
        }
    }

    visit(expression, path, 0, &mut 0)
}

fn validate_template(
    template: &SafeTemplate,
    path: &str,
) -> Result<(), OrchestrationValidationError> {
    fn visit(
        template: &SafeTemplate,
        path: &str,
        depth: usize,
        nodes: &mut usize,
    ) -> Result<(), OrchestrationValidationError> {
        if depth > MAX_TEMPLATE_DEPTH {
            return Err(OrchestrationValidationError::new(
                path,
                format!("template nesting exceeds {MAX_TEMPLATE_DEPTH}"),
            ));
        }
        if template.max_output_chars == 0 || template.max_output_chars > MAX_TEMPLATE_OUTPUT_CHARS {
            return Err(OrchestrationValidationError::new(
                format!("{path}.max_output_chars"),
                format!("must be between 1 and {MAX_TEMPLATE_OUTPUT_CHARS}"),
            ));
        }
        for (index, part) in template.parts.iter().enumerate() {
            *nodes += 1;
            if *nodes > MAX_TEMPLATE_NODES {
                return Err(OrchestrationValidationError::new(
                    path,
                    format!("template nodes exceed {MAX_TEMPLATE_NODES}"),
                ));
            }
            let part_path = format!("{path}.parts[{index}]");
            match part {
                TemplatePart::Text { value } => {
                    validate_text(&part_path, value, 0, MAX_BLOCK_TEXT_CHARS)?;
                }
                TemplatePart::Variable { variable } | TemplatePart::Join { variable, .. } => {
                    validate_variable_ref(variable, &format!("{part_path}.variable"))?;
                    if let TemplatePart::Join { separator, .. } = part {
                        validate_text(
                            &format!("{part_path}.separator"),
                            separator,
                            0,
                            MAX_NAME_CHARS,
                        )?;
                    }
                }
                TemplatePart::Slot { name } => {
                    validate_text(&format!("{part_path}.name"), name, 1, MAX_NAME_CHARS)?;
                }
                TemplatePart::Conditional {
                    condition,
                    then_template,
                    else_template,
                } => {
                    validate_condition(condition, &format!("{part_path}.condition"))?;
                    visit(
                        then_template,
                        &format!("{part_path}.then_template"),
                        depth + 1,
                        nodes,
                    )?;
                    if let Some(else_template) = else_template {
                        visit(
                            else_template,
                            &format!("{part_path}.else_template"),
                            depth + 1,
                            nodes,
                        )?;
                    }
                }
                TemplatePart::BuiltIn { .. } => {}
            }
        }
        Ok(())
    }

    visit(template, path, 0, &mut 0)
}

impl ValidateOrchestration for PromptPreset {
    fn validate(&self) -> Result<(), OrchestrationValidationError> {
        validate_id("id", self.id.as_str())?;
        validate_text("name", &self.name, 1, MAX_NAME_CHARS)?;
        if self.schema_version == 0 {
            return Err(OrchestrationValidationError::new(
                "schema_version",
                "must be positive",
            ));
        }
        if self.blocks.is_empty() || self.blocks.len() > MAX_PROMPT_BLOCKS {
            return Err(OrchestrationValidationError::new(
                "blocks",
                format!("must contain between 1 and {MAX_PROMPT_BLOCKS} blocks"),
            ));
        }
        if self.controls.len() > MAX_PROMPT_CONTROLS {
            return Err(OrchestrationValidationError::new(
                "controls",
                format!("must contain at most {MAX_PROMPT_CONTROLS} controls"),
            ));
        }
        validate_prompt_preset_references(self)?;
        validate_prompt_preset_blocks(self)?;
        validate_prompt_preset_controls(self)?;
        validate_prompt_preset_cache_boundaries(self)?;
        validate_provenance(&self.metadata.provenance, "metadata.provenance")
    }
}

fn validate_prompt_preset_references(
    preset: &PromptPreset,
) -> Result<(), OrchestrationValidationError> {
    validate_variable_map(&preset.default_values, "default_values")?;
    validate_id_list(
        "knowledge_book_ids",
        preset
            .knowledge_book_ids
            .iter()
            .map(KnowledgeBookId::as_str),
        128,
    )?;
    validate_id_list(
        "transform_set_ids",
        preset.transform_set_ids.iter().map(TransformSetId::as_str),
        128,
    )?;
    validate_id_list(
        "module_ids",
        preset.module_ids.iter().map(ContentModuleId::as_str),
        128,
    )?;
    if preset.metadata.tags.len() > 64 {
        return Err(OrchestrationValidationError::new(
            "metadata.tags",
            "must contain at most 64 tags",
        ));
    }
    for (index, tag) in preset.metadata.tags.iter().enumerate() {
        validate_text(&format!("metadata.tags[{index}]"), tag, 1, 128)?;
    }
    Ok(())
}

fn validate_prompt_preset_blocks(
    preset: &PromptPreset,
) -> Result<(), OrchestrationValidationError> {
    let mut block_ids = Vec::with_capacity(preset.blocks.len());
    let mut previous_zone = PlacementZone::ApplicationPolicy;
    let mut latest_user_count = 0_usize;
    let mut prefill_seen = false;
    for (index, block) in preset.blocks.iter().enumerate() {
        let path = format!("blocks[{index}]");
        validate_prompt_block(block, &path)?;
        if block.placement_zone < previous_zone {
            return Err(OrchestrationValidationError::new(
                format!("{path}.placement_zone"),
                "blocks must be ordered by placement zone",
            ));
        }
        previous_zone = block.placement_zone;
        if block.kind == PromptBlockKind::LatestUserTurn {
            latest_user_count += 1;
        }
        if prefill_seen {
            return Err(OrchestrationValidationError::new(
                path,
                "assistant prefill must be the final block",
            ));
        }
        prefill_seen = block.kind == PromptBlockKind::AssistantPrefill;
        block_ids.push(&block.id);
    }
    block_ids.sort();
    if block_ids.windows(2).any(|pair| pair[0] == pair[1]) {
        return Err(OrchestrationValidationError::new(
            "blocks",
            "block identifiers must be unique",
        ));
    }
    if latest_user_count != 1 {
        return Err(OrchestrationValidationError::new(
            "blocks",
            "exactly one latest-user block is required",
        ));
    }
    Ok(())
}

fn validate_prompt_preset_controls(
    preset: &PromptPreset,
) -> Result<(), OrchestrationValidationError> {
    let mut control_ids = Vec::with_capacity(preset.controls.len());
    for (index, control) in preset.controls.iter().enumerate() {
        validate_control(control, &format!("controls[{index}]"))?;
        control_ids.push(&control.id);
    }
    control_ids.sort();
    if control_ids.windows(2).any(|pair| pair[0] == pair[1]) {
        return Err(OrchestrationValidationError::new(
            "controls",
            "control identifiers must be unique",
        ));
    }
    Ok(())
}

fn validate_prompt_preset_cache_boundaries(
    preset: &PromptPreset,
) -> Result<(), OrchestrationValidationError> {
    if preset.cache_boundaries.len() > 32 {
        return Err(OrchestrationValidationError::new(
            "cache_boundaries",
            "must contain at most 32 boundaries",
        ));
    }
    for (index, boundary) in preset.cache_boundaries.iter().enumerate() {
        validate_id(
            &format!("cache_boundaries[{index}].id"),
            boundary.id.as_str(),
        )?;
        if !preset
            .blocks
            .iter()
            .any(|block| block.id == boundary.after_block_id)
        {
            return Err(OrchestrationValidationError::new(
                format!("cache_boundaries[{index}].after_block_id"),
                "must reference a block in the preset",
            ));
        }
    }
    Ok(())
}

fn validate_prompt_block(
    block: &PromptBlock,
    path: &str,
) -> Result<(), OrchestrationValidationError> {
    validate_id(&format!("{path}.id"), block.id.as_str())?;
    validate_text(&format!("{path}.name"), &block.name, 1, MAX_NAME_CHARS)?;
    if let Some(template) = &block.template {
        validate_template(template, &format!("{path}.template"))?;
    }
    if let Some(condition) = &block.condition {
        validate_condition(condition, &format!("{path}.condition"))?;
    }
    if block
        .token_policy
        .min_tokens
        .zip(block.token_policy.max_tokens)
        .is_some_and(|(minimum, maximum)| minimum > maximum)
    {
        return Err(OrchestrationValidationError::new(
            format!("{path}.token_policy"),
            "minimum tokens must not exceed maximum tokens",
        ));
    }
    match block.kind {
        PromptBlockKind::LatestUserTurn => {
            if block.placement_zone != PlacementZone::LatestUser
                || block.source != BlockSource::LatestUser
                || !block.enabled
                || block.template.is_some()
                || block.history_selector.is_some()
                || block.role_hint != RoleHint::User
                || block.overflow_policy != OverflowPolicy::Reject
            {
                return Err(OrchestrationValidationError::new(
                    path,
                    "latest-user block must be enabled, fixed to latest_user, and reject overflow",
                ));
            }
        }
        PromptBlockKind::AssistantPrefill
            if block.placement_zone != PlacementZone::AssistantPrefill
                || block.role_hint != RoleHint::Assistant =>
        {
            return Err(OrchestrationValidationError::new(
                format!("{path}.placement_zone"),
                "assistant prefill must use the assistant-prefill zone",
            ));
        }
        _ => {}
    }
    if matches!(block.source, BlockSource::History) != block.history_selector.is_some() {
        return Err(OrchestrationValidationError::new(
            format!("{path}.history_selector"),
            "history selectors are required only for history sources",
        ));
    }
    if !prompt_block_kind_matches_source(block.kind, &block.source) {
        return Err(OrchestrationValidationError::new(
            format!("{path}.source"),
            "prompt block kind and source are semantically inconsistent",
        ));
    }
    if (block.placement_zone == PlacementZone::ApplicationPolicy)
        != (block.authority == InstructionAuthority::Application)
    {
        return Err(OrchestrationValidationError::new(
            format!("{path}.authority"),
            "application authority is reserved for the fixed application-policy zone",
        ));
    }
    validate_provenance(&block.provenance, &format!("{path}.provenance"))
}

/// Rejects documents that label one dynamic source as a different semantic
/// block kind. Template-backed creator blocks remain intentionally flexible,
/// while materialized character and orchestration sources have one exact
/// meaning that Core can validate and seal.
fn prompt_block_kind_matches_source(kind: PromptBlockKind, source: &BlockSource) -> bool {
    match source {
        BlockSource::History => kind == PromptBlockKind::HistorySlice,
        BlockSource::LatestUser => kind == PromptBlockKind::LatestUserTurn,
        BlockSource::SelectedKnowledge => kind == PromptBlockKind::WorldKnowledge,
        BlockSource::SelectedMemory => kind == PromptBlockKind::RetrievedMemory,
        BlockSource::ConversationSummary => kind == PromptBlockKind::ConversationSummary,
        BlockSource::AuthorNote => kind == PromptBlockKind::AuthorNote,
        BlockSource::UserPersona => kind == PromptBlockKind::UserPersona,
        BlockSource::GroupContext => kind == PromptBlockKind::GroupContext,
        BlockSource::CharacterField { field } => match field {
            CharacterField::Name => kind == PromptBlockKind::CharacterIdentity,
            CharacterField::Description => kind == PromptBlockKind::CharacterDescription,
            CharacterField::Personality => kind == PromptBlockKind::CharacterPersonality,
            CharacterField::Scenario => kind == PromptBlockKind::Scenario,
            CharacterField::DialogueExamples => kind == PromptBlockKind::DialogueExamples,
            CharacterField::SystemInstruction => kind == PromptBlockKind::StaticInstruction,
            CharacterField::PostHistoryInstruction => {
                kind == PromptBlockKind::PostHistoryInstruction
            }
            // A greeting may be authored as a template-like instruction or
            // an assistant prefill depending on the selected product mode.
            CharacterField::FirstMessage => matches!(
                kind,
                PromptBlockKind::StaticInstruction | PromptBlockKind::AssistantPrefill
            ),
        },
        BlockSource::Template => !matches!(
            kind,
            PromptBlockKind::CharacterIdentity
                | PromptBlockKind::CharacterDescription
                | PromptBlockKind::CharacterPersonality
                | PromptBlockKind::Scenario
                | PromptBlockKind::UserPersona
                | PromptBlockKind::DialogueExamples
                | PromptBlockKind::WorldKnowledge
                | PromptBlockKind::RetrievedMemory
                | PromptBlockKind::ConversationSummary
                | PromptBlockKind::HistorySlice
                | PromptBlockKind::LatestUserTurn
                | PromptBlockKind::AuthorNote
                | PromptBlockKind::PostHistoryInstruction
                | PromptBlockKind::GroupContext
        ),
    }
}

fn validate_control(control: &ControlSpec, path: &str) -> Result<(), OrchestrationValidationError> {
    validate_id(&format!("{path}.id"), control.id.as_str())?;
    validate_text(&format!("{path}.label"), &control.label, 1, MAX_NAME_CHARS)?;
    validate_text(
        &format!("{path}.description"),
        &control.description,
        0,
        4_096,
    )?;
    validate_control_binding(control, path)?;
    validate_control_default(control, path)?;
    validate_control_options(control, path)?;
    validate_control_numeric_bounds(control, path)?;
    if let Some(condition) = &control.visible_when {
        validate_condition(condition, &format!("{path}.visible_when"))?;
    }
    Ok(())
}

fn validate_control_binding(
    control: &ControlSpec,
    path: &str,
) -> Result<(), OrchestrationValidationError> {
    let presentation_only = matches!(
        control.kind,
        ControlKind::Section | ControlKind::Caption | ControlKind::Divider
    );
    if presentation_only {
        if control.value_type.is_some()
            || control.variable.is_some()
            || control.default_value.is_some()
            || !control.options.is_empty()
            || control.minimum.is_some()
            || control.maximum.is_some()
            || control.step.is_some()
            || control.sensitive
        {
            return Err(OrchestrationValidationError::new(
                path,
                "presentation-only controls cannot bind, default, constrain, or expose a value",
            ));
        }
        return Ok(());
    }

    let value_type = control.value_type.ok_or_else(|| {
        OrchestrationValidationError::new(
            format!("{path}.value_type"),
            "interactive controls require a declared value type",
        )
    })?;
    let variable = control.variable.as_ref().ok_or_else(|| {
        OrchestrationValidationError::new(
            format!("{path}.variable"),
            "interactive controls require a variable binding",
        )
    })?;
    validate_variable_ref(variable, &format!("{path}.variable"))?;
    if variable.scope != control.scope {
        return Err(OrchestrationValidationError::new(
            format!("{path}.scope"),
            "control scope must match its variable binding",
        ));
    }
    let valid_kind_type = matches!(
        (control.kind, value_type),
        (ControlKind::Toggle, VariableType::Bool)
            | (ControlKind::Select, VariableType::Enum)
            | (ControlKind::MultiSelect, VariableType::StringList)
            | (ControlKind::Text, VariableType::Text)
            | (
                ControlKind::Number | ControlKind::Slider,
                VariableType::Integer | VariableType::Decimal
            )
    );
    if !valid_kind_type {
        return Err(OrchestrationValidationError::new(
            format!("{path}.value_type"),
            "value type is incompatible with the control kind",
        ));
    }
    Ok(())
}

fn validate_control_default(
    control: &ControlSpec,
    path: &str,
) -> Result<(), OrchestrationValidationError> {
    if let Some(value) = &control.default_value {
        validate_variable_value(value, &format!("{path}.default_value"))?;
        if control
            .value_type
            .is_some_and(|value_type| variable_value_type(value) != value_type)
        {
            return Err(OrchestrationValidationError::new(
                format!("{path}.default_value"),
                "default value does not match the declared value type",
            ));
        }
    }
    Ok(())
}

fn validate_control_options(
    control: &ControlSpec,
    path: &str,
) -> Result<(), OrchestrationValidationError> {
    if control.options.len() > MAX_VARIABLE_LIST_ITEMS {
        return Err(OrchestrationValidationError::new(
            format!("{path}.options"),
            format!("must contain at most {MAX_VARIABLE_LIST_ITEMS} options"),
        ));
    }
    let mut encoded_options = Vec::with_capacity(control.options.len());
    for (index, option) in control.options.iter().enumerate() {
        validate_variable_value(&option.value, &format!("{path}.options[{index}].value"))?;
        validate_text(
            &format!("{path}.options[{index}].label"),
            &option.label,
            1,
            MAX_NAME_CHARS,
        )?;
        let valid_option_type = match control.kind {
            ControlKind::Select => variable_value_type(&option.value) == VariableType::Enum,
            ControlKind::MultiSelect => matches!(
                variable_value_type(&option.value),
                VariableType::Text | VariableType::Enum
            ),
            _ => false,
        };
        if !valid_option_type {
            return Err(OrchestrationValidationError::new(
                format!("{path}.options[{index}].value"),
                "only select controls may declare compatible option values",
            ));
        }
        let encoded = serde_json::to_string(&option.value).map_err(|error| {
            OrchestrationValidationError::new(
                format!("{path}.options[{index}].value"),
                format!("cannot encode option value: {error}"),
            )
        })?;
        encoded_options.push(encoded);
    }
    encoded_options.sort_unstable();
    if encoded_options.windows(2).any(|pair| pair[0] == pair[1]) {
        return Err(OrchestrationValidationError::new(
            format!("{path}.options"),
            "option values must be unique",
        ));
    }
    if matches!(control.kind, ControlKind::Select | ControlKind::MultiSelect)
        && control.options.is_empty()
    {
        return Err(OrchestrationValidationError::new(
            format!("{path}.options"),
            "select controls require at least one option",
        ));
    }
    match (control.kind, control.default_value.as_ref()) {
        (ControlKind::Select, Some(default_value))
            if !control
                .options
                .iter()
                .any(|option| &option.value == default_value) =>
        {
            return Err(OrchestrationValidationError::new(
                format!("{path}.default_value"),
                "select default value must be one of the declared options",
            ));
        }
        (ControlKind::MultiSelect, Some(VariableValue::StringList(default_values))) => {
            for (index, default_value) in default_values.iter().enumerate() {
                let present = control.options.iter().any(|option| {
                    matches!(
                        &option.value,
                        VariableValue::Text(value) | VariableValue::Enum(value)
                            if value == default_value
                    )
                });
                if !present {
                    return Err(OrchestrationValidationError::new(
                        format!("{path}.default_value[{index}]"),
                        "multi-select defaults must be declared options",
                    ));
                }
            }
        }
        _ => {}
    }
    Ok(())
}

fn validate_control_numeric_bounds(
    control: &ControlSpec,
    path: &str,
) -> Result<(), OrchestrationValidationError> {
    if control.minimum.is_some_and(|value| !value.is_finite())
        || control.maximum.is_some_and(|value| !value.is_finite())
        || control
            .step
            .is_some_and(|value| !value.is_finite() || value <= 0.0)
    {
        return Err(OrchestrationValidationError::new(
            path,
            "numeric bounds must be finite and step must be positive",
        ));
    }
    if control
        .minimum
        .zip(control.maximum)
        .is_some_and(|(minimum, maximum)| minimum > maximum)
    {
        return Err(OrchestrationValidationError::new(
            path,
            "minimum must not exceed maximum",
        ));
    }
    if !matches!(control.kind, ControlKind::Number | ControlKind::Slider)
        && (control.minimum.is_some() || control.maximum.is_some() || control.step.is_some())
    {
        return Err(OrchestrationValidationError::new(
            path,
            "only number and slider controls may declare numeric bounds",
        ));
    }
    if control.kind == ControlKind::Slider
        && (control.minimum.is_none() || control.maximum.is_none())
    {
        return Err(OrchestrationValidationError::new(
            path,
            "slider controls require both minimum and maximum",
        ));
    }
    let numeric_default = control
        .default_value
        .as_ref()
        .and_then(|value| match value {
            VariableValue::Integer(value) => value.to_string().parse::<f64>().ok(),
            VariableValue::Decimal(value) => Some(*value),
            _ => None,
        });
    if numeric_default.is_some_and(|value| {
        control.minimum.is_some_and(|minimum| value < minimum)
            || control.maximum.is_some_and(|maximum| value > maximum)
    }) {
        return Err(OrchestrationValidationError::new(
            format!("{path}.default_value"),
            "numeric default value must fall within the declared bounds",
        ));
    }
    Ok(())
}

impl ValidateOrchestration for PromptResolveRequest {
    fn validate(&self) -> Result<(), OrchestrationValidationError> {
        self.preset.validate()?;
        validate_prompt_request_limits(self)?;
        validate_provider_prompt_contract(&self.provider)?;
        validate_prompt_context_messages(&self.context)?;
        validate_prompt_context_selection_ids(&self.context)?;
        validate_prompt_context_summaries_and_slots(&self.context)?;
        validate_prompt_context_text(&self.context)?;
        if let Some(snapshot) = &self.context.context_snapshot {
            validate_prompt_context_snapshot(snapshot, Some(&self.context))?;
        }
        validate_variable_map(&self.context.variables, "context.variables")
    }
}

fn validate_prompt_request_limits(
    request: &PromptResolveRequest,
) -> Result<(), OrchestrationValidationError> {
    if request.max_context_tokens == 0 {
        return Err(OrchestrationValidationError::new(
            "max_context_tokens",
            "must be positive",
        ));
    }
    if request.reserved_output_tokens >= request.max_context_tokens {
        return Err(OrchestrationValidationError::new(
            "reserved_output_tokens",
            "must be smaller than the context limit",
        ));
    }
    Ok(())
}

fn validate_provider_prompt_contract(
    provider: &ProviderPromptContract,
) -> Result<(), OrchestrationValidationError> {
    if provider.supported_roles.is_empty() {
        return Err(OrchestrationValidationError::new(
            "provider.supported_roles",
            "must contain at least one role",
        ));
    }
    let mut roles = provider.supported_roles.clone();
    roles.sort();
    if roles.windows(2).any(|pair| pair[0] == pair[1])
        || !roles.contains(&provider.provider_default_role)
    {
        return Err(OrchestrationValidationError::new(
            "provider.supported_roles",
            "roles must be unique and contain the provider default",
        ));
    }
    if provider.supports_explicit_cache != (provider.max_cache_boundaries > 0) {
        return Err(OrchestrationValidationError::new(
            "provider.max_cache_boundaries",
            "must be positive exactly when explicit caching is supported",
        ));
    }
    Ok(())
}

fn validate_prompt_context_messages(
    context: &PromptResolutionContext,
) -> Result<(), OrchestrationValidationError> {
    if context.messages.len() > MAX_HISTORY_MESSAGES {
        return Err(OrchestrationValidationError::new(
            "context.messages",
            format!("must contain at most {MAX_HISTORY_MESSAGES} messages"),
        ));
    }
    let mut message_ids = context
        .messages
        .iter()
        .map(|message| &message.id.0)
        .collect::<Vec<_>>();
    message_ids.sort();
    if message_ids.windows(2).any(|pair| pair[0] == pair[1]) {
        return Err(OrchestrationValidationError::new(
            "context.messages",
            "message identifiers must be unique",
        ));
    }
    let latest = context
        .messages
        .iter()
        .find(|message| message.id == context.latest_user_message_id);
    if !latest.is_some_and(|message| {
        message.role == PromptMessageRole::User
            && message.branch_id == context.branch_id
            && !message.content.is_empty()
    }) {
        return Err(OrchestrationValidationError::new(
            "context.latest_user_message_id",
            "must identify a non-empty user message on the active branch",
        ));
    }
    Ok(())
}

fn validate_prompt_context_selection_ids(
    context: &PromptResolutionContext,
) -> Result<(), OrchestrationValidationError> {
    if context.selected_knowledge.len() > MAX_KNOWLEDGE_ENTRIES {
        return Err(OrchestrationValidationError::new(
            "context.selected_knowledge",
            format!("must contain at most {MAX_KNOWLEDGE_ENTRIES} entries"),
        ));
    }
    let mut knowledge_ids = context
        .selected_knowledge
        .iter()
        .map(|entry| &entry.entry_id)
        .collect::<Vec<_>>();
    knowledge_ids.sort();
    if knowledge_ids.windows(2).any(|pair| pair[0] == pair[1]) {
        return Err(OrchestrationValidationError::new(
            "context.selected_knowledge",
            "entry identifiers must be unique",
        ));
    }
    if context.selected_memory.len() > MAX_MEMORY_RECORDS {
        return Err(OrchestrationValidationError::new(
            "context.selected_memory",
            format!("must contain at most {MAX_MEMORY_RECORDS} records"),
        ));
    }
    let mut memory_ids = context
        .selected_memory
        .iter()
        .map(|record| &record.record_id)
        .collect::<Vec<_>>();
    memory_ids.sort();
    if memory_ids.windows(2).any(|pair| pair[0] == pair[1]) {
        return Err(OrchestrationValidationError::new(
            "context.selected_memory",
            "record identifiers must be unique",
        ));
    }
    Ok(())
}

fn validate_prompt_context_summaries_and_slots(
    context: &PromptResolutionContext,
) -> Result<(), OrchestrationValidationError> {
    let mut summary_ids = context
        .summary_boundaries
        .iter()
        .map(|boundary| &boundary.summary_id)
        .collect::<Vec<_>>();
    summary_ids.sort();
    if summary_ids.windows(2).any(|pair| pair[0] == pair[1]) {
        return Err(OrchestrationValidationError::new(
            "context.summary_boundaries",
            "summary identifiers must be unique",
        ));
    }
    if context.summary_boundaries.iter().any(|boundary| {
        !context.messages.iter().any(|message| {
            message.id == boundary.end_message_id && message.branch_id == context.branch_id
        })
    }) {
        return Err(OrchestrationValidationError::new(
            "context.summary_boundaries",
            "every summary boundary must reference an active-branch message",
        ));
    }
    let mut slot_names = context
        .slots
        .iter()
        .map(|slot| slot.name.as_str())
        .collect::<Vec<_>>();
    slot_names.sort_unstable();
    if slot_names.windows(2).any(|pair| pair[0] == pair[1]) {
        return Err(OrchestrationValidationError::new(
            "context.slots",
            "template slot names must be unique",
        ));
    }
    if slot_names.contains(&"block_content") {
        return Err(OrchestrationValidationError::new(
            "context.slots",
            "`block_content` is reserved for block materialization",
        ));
    }
    Ok(())
}

fn validate_prompt_context_snapshot(
    snapshot: &PromptContextSnapshotV1,
    context: Option<&PromptResolutionContext>,
) -> Result<(), OrchestrationValidationError> {
    validate_prompt_context_snapshot_header(snapshot)?;
    validate_prompt_context_snapshot_binding(snapshot.binding.as_ref())?;
    validate_prompt_context_snapshot_persona(snapshot.persona.as_ref())?;
    let summary_ids = validate_prompt_context_snapshot_summaries(snapshot)?;
    if snapshot
        .conversation_summary_id
        .as_ref()
        .is_some_and(|id| !summary_ids.contains(id))
    {
        return Err(OrchestrationValidationError::new(
            "context.context_snapshot.conversation_summary_id",
            "selected conversation summary must have exact source evidence",
        ));
    }
    if context.is_some_and(|context| {
        prompt_context_snapshot_differs_from_materialized(snapshot, context, &summary_ids)
    }) {
        return Err(OrchestrationValidationError::new(
            "context.context_snapshot",
            "prompt context snapshot differs from the materialized context",
        ));
    }
    Ok(())
}

fn validate_prompt_context_snapshot_header(
    snapshot: &PromptContextSnapshotV1,
) -> Result<(), OrchestrationValidationError> {
    if snapshot.schema_version != 1 {
        return Err(OrchestrationValidationError::new(
            "context.context_snapshot.schema_version",
            "unsupported prompt context snapshot schema",
        ));
    }
    validate_id(
        "context.context_snapshot.conversation_id",
        &snapshot.conversation_id.0,
    )?;
    validate_id(
        "context.context_snapshot.source_branch_id",
        &snapshot.source_branch_id.0,
    )?;
    validate_sha256(
        "context.context_snapshot.local_user_id_sha256",
        &snapshot.local_user_id_sha256,
    )?;
    validate_sha256(
        "context.context_snapshot.snapshot_sha256",
        &snapshot.snapshot_sha256,
    )?;
    if prompt_context_snapshot_sha256(snapshot)? != snapshot.snapshot_sha256 {
        return Err(OrchestrationValidationError::new(
            "context.context_snapshot.snapshot_sha256",
            "prompt context snapshot fingerprint does not match its evidence",
        ));
    }
    Ok(())
}

fn validate_prompt_context_snapshot_binding(
    binding: Option<&PromptContextBindingEvidence>,
) -> Result<(), OrchestrationValidationError> {
    if let Some(binding) = binding {
        validate_id(
            "context.context_snapshot.binding.binding_id",
            &binding.binding_id,
        )?;
        if binding.binding_revision == 0 {
            return Err(OrchestrationValidationError::new(
                "context.context_snapshot.binding.binding_revision",
                "binding revision must be positive",
            ));
        }
        validate_sha256(
            "context.context_snapshot.binding.document_sha256",
            &binding.document_sha256,
        )?;
    }
    Ok(())
}

fn validate_prompt_context_snapshot_persona(
    persona: Option<&PromptContextPersonaEvidence>,
) -> Result<(), OrchestrationValidationError> {
    if let Some(persona) = persona {
        if persona.selection_revision == 0 {
            return Err(OrchestrationValidationError::new(
                "context.context_snapshot.persona.selection_revision",
                "persona selection revision must be positive",
            ));
        }
        validate_id(
            "context.context_snapshot.persona.persona_id",
            persona.persona_id.as_str(),
        )?;
        validate_id(
            "context.context_snapshot.persona.persona_revision_id",
            &persona.persona_revision_id,
        )?;
        validate_sha256(
            "context.context_snapshot.persona.persona_sha256",
            &persona.persona_sha256,
        )?;
    }
    Ok(())
}

fn validate_prompt_context_snapshot_summaries(
    snapshot: &PromptContextSnapshotV1,
) -> Result<Vec<MemoryRecordId>, OrchestrationValidationError> {
    if snapshot.summaries.len() > MAX_PROMPT_BLOCKS {
        return Err(OrchestrationValidationError::new(
            "context.context_snapshot.summaries",
            format!("must contain at most {MAX_PROMPT_BLOCKS} summary sources"),
        ));
    }
    let mut summary_ids = Vec::with_capacity(snapshot.summaries.len());
    for (index, summary) in snapshot.summaries.iter().enumerate() {
        validate_prompt_summary_source_evidence(index, summary)?;
        summary_ids.push(summary.summary_id.clone());
    }
    summary_ids.sort();
    if summary_ids.windows(2).any(|pair| pair[0] == pair[1]) {
        return Err(OrchestrationValidationError::new(
            "context.context_snapshot.summaries",
            "summary source identifiers must be unique",
        ));
    }
    Ok(summary_ids)
}

fn validate_prompt_summary_source_evidence(
    index: usize,
    summary: &PromptSummarySourceEvidence,
) -> Result<(), OrchestrationValidationError> {
    validate_id(
        &format!("context.context_snapshot.summaries[{index}].summary_id"),
        summary.summary_id.as_str(),
    )?;
    validate_id(
        &format!("context.context_snapshot.summaries[{index}].record_branch_id"),
        &summary.record_branch_id.0,
    )?;
    validate_id(
        &format!("context.context_snapshot.summaries[{index}].source_start_message_id"),
        &summary.source_start_message_id.0,
    )?;
    validate_id(
        &format!("context.context_snapshot.summaries[{index}].source_end_message_id"),
        &summary.source_end_message_id.0,
    )?;
    if summary.state_revision == 0 {
        return Err(OrchestrationValidationError::new(
            format!("context.context_snapshot.summaries[{index}].state_revision"),
            "memory state revision must be positive",
        ));
    }
    validate_id(
        &format!("context.context_snapshot.summaries[{index}].active_revision_id"),
        &summary.active_revision_id,
    )?;
    validate_sha256(
        &format!("context.context_snapshot.summaries[{index}].active_revision_sha256"),
        &summary.active_revision_sha256,
    )?;
    Ok(())
}

fn prompt_context_snapshot_differs_from_materialized(
    snapshot: &PromptContextSnapshotV1,
    context: &PromptResolutionContext,
    summary_ids: &[MemoryRecordId],
) -> bool {
    snapshot.conversation_id != context.conversation_id
        || snapshot
            .context_head_message_id
            .as_ref()
            .is_some_and(|head| !context.messages.iter().any(|message| &message.id == head))
        || context
            .summary_boundaries
            .iter()
            .any(|boundary| !summary_ids.contains(&boundary.summary_id))
        || snapshot.conversation_summary_id.is_some() != context.conversation_summary.is_some()
}

fn validate_prompt_context_text(
    context: &PromptResolutionContext,
) -> Result<(), OrchestrationValidationError> {
    validate_text("context.user_name", &context.user_name, 1, MAX_NAME_CHARS)?;
    validate_character_prompt_text(&context.character)?;
    validate_prompt_context_material_text(context)?;
    validate_prompt_context_optional_text(context)
}

fn validate_character_prompt_text(
    character: &CharacterPromptContent,
) -> Result<(), OrchestrationValidationError> {
    validate_text(
        "context.character.character_id",
        &character.character_id,
        1,
        MAX_IDENTIFIER_CHARS,
    )?;
    validate_text("context.character.name", &character.name, 1, MAX_NAME_CHARS)?;
    for (field, value) in [
        ("description", &character.description),
        ("personality", &character.personality),
        ("scenario", &character.scenario),
        ("first_message", &character.first_message),
        ("system_instruction", &character.system_instruction),
        (
            "post_history_instruction",
            &character.post_history_instruction,
        ),
    ] {
        validate_text(
            &format!("context.character.{field}"),
            value,
            0,
            MAX_BLOCK_TEXT_CHARS,
        )?;
    }
    if character.aliases.len() > 128
        || character.dialogue_examples.len() > 256
        || character.alternate_greetings.len() > 128
        || character.knowledge_book_ids.len() > 128
        || character.asset_ids.len() > MAX_MODULE_COMPONENTS
    {
        return Err(OrchestrationValidationError::new(
            "context.character",
            "character content collection exceeds its bound",
        ));
    }
    for (field, values, maximum_chars) in [
        ("aliases", &character.aliases, MAX_NAME_CHARS),
        (
            "dialogue_examples",
            &character.dialogue_examples,
            MAX_BLOCK_TEXT_CHARS,
        ),
        (
            "alternate_greetings",
            &character.alternate_greetings,
            MAX_BLOCK_TEXT_CHARS,
        ),
    ] {
        for (index, value) in values.iter().enumerate() {
            validate_text(
                &format!("context.character.{field}[{index}]"),
                value,
                1,
                maximum_chars,
            )?;
        }
    }
    Ok(())
}

fn validate_prompt_context_material_text(
    context: &PromptResolutionContext,
) -> Result<(), OrchestrationValidationError> {
    for (index, message) in context.messages.iter().enumerate() {
        validate_text(
            &format!("context.messages[{index}].content"),
            &message.content,
            1,
            MAX_BLOCK_TEXT_CHARS,
        )?;
    }
    for (index, entry) in context.selected_knowledge.iter().enumerate() {
        validate_text(
            &format!("context.selected_knowledge[{index}].content"),
            &entry.content,
            1,
            MAX_BLOCK_TEXT_CHARS,
        )?;
        if entry.evidence.is_empty() {
            return Err(OrchestrationValidationError::new(
                format!("context.selected_knowledge[{index}].evidence"),
                "selected knowledge requires activation evidence",
            ));
        }
    }
    for (index, record) in context.selected_memory.iter().enumerate() {
        validate_text(
            &format!("context.selected_memory[{index}].content"),
            &record.content,
            1,
            MAX_BLOCK_TEXT_CHARS,
        )?;
        validate_text(
            &format!("context.selected_memory[{index}].reason"),
            &record.reason,
            1,
            4_096,
        )?;
    }
    for (index, slot) in context.slots.iter().enumerate() {
        validate_text(
            &format!("context.slots[{index}].name"),
            &slot.name,
            1,
            MAX_NAME_CHARS,
        )?;
        validate_text(
            &format!("context.slots[{index}].value"),
            &slot.value,
            0,
            MAX_BLOCK_TEXT_CHARS,
        )?;
    }
    Ok(())
}

fn validate_prompt_context_optional_text(
    context: &PromptResolutionContext,
) -> Result<(), OrchestrationValidationError> {
    for (field, value) in [
        (
            "conversation_summary",
            context.conversation_summary.as_ref(),
        ),
        ("author_note", context.author_note.as_ref()),
        ("group_context", context.group_context.as_ref()),
    ] {
        if let Some(value) = value {
            validate_text(&format!("context.{field}"), value, 1, MAX_BLOCK_TEXT_CHARS)?;
        }
    }
    validate_text(
        "context.current_date",
        &context.current_date,
        1,
        MAX_NAME_CHARS,
    )?;
    validate_text(
        "context.current_time",
        &context.current_time,
        1,
        MAX_NAME_CHARS,
    )
}

impl SafeRegex {
    /// Checks the inert regex descriptor before a linear-time engine compiles it.
    ///
    /// # Errors
    ///
    /// Rejects empty, oversized, NUL-containing, or known unsupported
    /// backreference/look-around syntax.
    pub fn validate(&self) -> Result<(), OrchestrationValidationError> {
        validate_text("pattern", &self.pattern, 1, MAX_SAFE_REGEX_CHARS)?;
        if UNSUPPORTED_SAFE_REGEX_SYNTAX
            .iter()
            .any(|needle| self.pattern.contains(needle))
        {
            return Err(OrchestrationValidationError::new(
                "pattern",
                "look-around and backreferences are not supported",
            ));
        }
        Ok(())
    }
}

impl SafeTemplate {
    /// Validates template bounds and nested expressions.
    ///
    /// # Errors
    ///
    /// Returns a field-path error for unsafe or unbounded template data.
    pub fn validate(&self) -> Result<(), OrchestrationValidationError> {
        validate_template(self, "template")
    }
}

impl ConditionExpr {
    /// Validates expression depth, size, references, and numeric values.
    ///
    /// # Errors
    ///
    /// Returns a field-path error for malformed or unbounded expressions.
    pub fn validate(&self) -> Result<(), OrchestrationValidationError> {
        validate_condition(self, "condition")
    }
}

impl VariableMap {
    /// Validates unique, bounded variable bindings.
    ///
    /// # Errors
    ///
    /// Returns a field-path error for duplicate or invalid bindings.
    pub fn validate(&self) -> Result<(), OrchestrationValidationError> {
        validate_variable_map(self, "variables")
    }
}

impl ValidateOrchestration for TransformSet {
    fn validate(&self) -> Result<(), OrchestrationValidationError> {
        validate_id("id", self.id.as_str())?;
        validate_text("name", &self.name, 1, MAX_NAME_CHARS)?;
        if self.schema_version == 0 {
            return Err(OrchestrationValidationError::new(
                "schema_version",
                "must be positive",
            ));
        }
        let configured_limit = usize::try_from(self.max_rules_per_phase)
            .unwrap_or(usize::MAX)
            .min(MAX_TRANSFORM_RULES);
        for phase in [
            TransformPhase::UserInputForRequest,
            TransformPhase::ResolvedPrompt,
            TransformPhase::ProviderOutputCanonical,
            TransformPhase::DisplayOnly,
            TransformPhase::MemoryInput,
        ] {
            if self.rules.iter().filter(|rule| rule.phase == phase).count() > configured_limit {
                return Err(OrchestrationValidationError::new(
                    "rules",
                    "rules exceed the configured per-phase limit",
                ));
            }
        }
        if self.max_output_chars == 0 || self.max_output_chars > MAX_TEMPLATE_OUTPUT_CHARS {
            return Err(OrchestrationValidationError::new(
                "max_output_chars",
                format!("must be between 1 and {MAX_TEMPLATE_OUTPUT_CHARS}"),
            ));
        }
        for (index, rule) in self.rules.iter().enumerate() {
            validate_id(&format!("rules[{index}].id"), rule.id.as_str())?;
            rule.pattern.validate().map_err(|error| {
                OrchestrationValidationError::new(
                    format!("rules[{index}].{}", error.path),
                    error.reason,
                )
            })?;
            if rule.max_replacements == 0
                || rule.input_limit == 0
                || rule.output_limit == 0
                || rule.output_limit > self.max_output_chars
            {
                return Err(OrchestrationValidationError::new(
                    format!("rules[{index}]"),
                    "replacement and input/output limits must be positive and bounded by the set",
                ));
            }
        }
        Ok(())
    }
}

impl ValidateOrchestration for KnowledgeBook {
    fn validate(&self) -> Result<(), OrchestrationValidationError> {
        validate_id("id", self.id.as_str())?;
        validate_text("name", &self.name, 1, MAX_NAME_CHARS)?;
        if self.schema_version == 0 || self.entries.len() > MAX_KNOWLEDGE_ENTRIES {
            return Err(OrchestrationValidationError::new(
                "entries",
                format!("schema must be positive and entries at most {MAX_KNOWLEDGE_ENTRIES}"),
            ));
        }
        if self.scan_depth > 1_024
            || self.max_recursion_depth > 16
            || self.token_budget.max_tokens > 10_000_000
        {
            return Err(OrchestrationValidationError::new(
                "knowledge_book",
                "scan depth, recursion depth, or token budget is out of range",
            ));
        }
        if !self.recursive && self.max_recursion_depth != 0 {
            return Err(OrchestrationValidationError::new(
                "max_recursion_depth",
                "must be zero when recursive retrieval is disabled",
            ));
        }
        let mut entry_ids = Vec::with_capacity(self.entries.len());
        for (index, entry) in self.entries.iter().enumerate() {
            if entry.book_id != self.id {
                return Err(OrchestrationValidationError::new(
                    format!("entries[{index}].book_id"),
                    "must match the enclosing book",
                ));
            }
            validate_id(&format!("entries[{index}].id"), entry.id.as_str())?;
            validate_text(
                &format!("entries[{index}].content"),
                &entry.content,
                1,
                MAX_VARIABLE_TEXT_CHARS,
            )?;
            if entry.importance > 100 || entry.activation_probability_basis_points > 10_000 {
                return Err(OrchestrationValidationError::new(
                    format!("entries[{index}]"),
                    "importance or activation probability is out of range",
                ));
            }
            validate_activation_rule(
                &entry.activation,
                &format!("entries[{index}].activation"),
                0,
                &mut 0,
            )?;
            if entry
                .parent_id
                .as_ref()
                .is_some_and(|parent| parent == &entry.id)
            {
                return Err(OrchestrationValidationError::new(
                    format!("entries[{index}].parent_id"),
                    "an entry cannot be its own parent",
                ));
            }
            entry_ids.push(&entry.id);
        }
        entry_ids.sort();
        if entry_ids.windows(2).any(|pair| pair[0] == pair[1]) {
            return Err(OrchestrationValidationError::new(
                "entries",
                "entry identifiers must be unique",
            ));
        }
        validate_knowledge_parent_graph(&self.entries)?;
        Ok(())
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum KnowledgeParentVisit {
    Unseen,
    Active,
    Done,
}

fn validate_knowledge_parent_graph(
    entries: &[KnowledgeEntry],
) -> Result<(), OrchestrationValidationError> {
    let entries_by_id = entries
        .iter()
        .enumerate()
        .map(|(index, entry)| (entry.id.as_str(), index))
        .collect::<HashMap<_, _>>();
    let mut visits = vec![KnowledgeParentVisit::Unseen; entries.len()];
    let mut path = Vec::new();

    for root_index in 0..entries.len() {
        if visits[root_index] == KnowledgeParentVisit::Done {
            continue;
        }
        path.clear();
        let mut cursor = Some(root_index);
        while let Some(index) = cursor {
            match visits[index] {
                KnowledgeParentVisit::Done => break,
                KnowledgeParentVisit::Active => {
                    return Err(OrchestrationValidationError::new(
                        format!("entries[{root_index}].parent_id"),
                        "parent graph contains a cycle",
                    ));
                }
                KnowledgeParentVisit::Unseen => {
                    visits[index] = KnowledgeParentVisit::Active;
                    path.push(index);
                    cursor = entries[index]
                        .parent_id
                        .as_ref()
                        .map(|parent_id| {
                            record_knowledge_parent_edge_visit();
                            entries_by_id
                                .get(parent_id.as_str())
                                .copied()
                                .ok_or_else(|| {
                                    OrchestrationValidationError::new(
                                        format!("entries[{root_index}].parent_id"),
                                        "parent must reference an entry in the same book",
                                    )
                                })
                        })
                        .transpose()?;
                }
            }
        }
        for index in path.drain(..) {
            visits[index] = KnowledgeParentVisit::Done;
        }
    }
    Ok(())
}

#[cfg(test)]
std::thread_local! {
    static KNOWLEDGE_PARENT_EDGE_VISITS: std::cell::Cell<usize> = const {
        std::cell::Cell::new(0)
    };
}

#[inline]
fn record_knowledge_parent_edge_visit() {
    #[cfg(test)]
    KNOWLEDGE_PARENT_EDGE_VISITS.with(|visits| visits.set(visits.get().saturating_add(1)));
}

#[cfg(test)]
fn reset_knowledge_parent_edge_visits() {
    KNOWLEDGE_PARENT_EDGE_VISITS.with(|visits| visits.set(0));
}

#[cfg(test)]
fn knowledge_parent_edge_visits() -> usize {
    KNOWLEDGE_PARENT_EDGE_VISITS.with(std::cell::Cell::get)
}

fn validate_activation_rule(
    rule: &ActivationRule,
    path: &str,
    depth: usize,
    nodes: &mut usize,
) -> Result<(), OrchestrationValidationError> {
    if depth > 16 {
        return Err(OrchestrationValidationError::new(
            path,
            "activation nesting exceeds 16",
        ));
    }
    *nodes += 1;
    if *nodes > 512 {
        return Err(OrchestrationValidationError::new(
            path,
            "activation expression exceeds 512 nodes",
        ));
    }
    match rule {
        ActivationRule::Keyword {
            primary, secondary, ..
        } => {
            if primary.is_empty() || primary.len().saturating_add(secondary.len()) > 256 {
                return Err(OrchestrationValidationError::new(
                    path,
                    "keyword activation requires primary keys and at most 256 total keys",
                ));
            }
            for (index, keyword) in primary.iter().chain(secondary).enumerate() {
                validate_text(&format!("{path}.keywords[{index}]"), keyword, 1, 1_024)?;
            }
            Ok(())
        }
        ActivationRule::Regex { patterns } => {
            if patterns.is_empty() || patterns.len() > 64 {
                return Err(OrchestrationValidationError::new(
                    path,
                    "regex activation requires between 1 and 64 patterns",
                ));
            }
            for (index, pattern) in patterns.iter().enumerate() {
                pattern.validate().map_err(|error| {
                    OrchestrationValidationError::new(
                        format!("{path}.patterns[{index}].{}", error.path),
                        error.reason,
                    )
                })?;
            }
            Ok(())
        }
        ActivationRule::Semantic { threshold, top_k } => {
            if !threshold.is_finite() || !(0.0..=1.0).contains(threshold) || *top_k == 0 {
                return Err(OrchestrationValidationError::new(
                    path,
                    "semantic threshold must be within 0..=1 and top_k positive",
                ));
            }
            Ok(())
        }
        ActivationRule::Condition { expression } => validate_condition(expression, path),
        ActivationRule::Any { rules } | ActivationRule::All { rules } => {
            if rules.is_empty() {
                return Err(OrchestrationValidationError::new(
                    path,
                    "composed activation requires at least one rule",
                ));
            }
            for (index, child) in rules.iter().enumerate() {
                validate_activation_rule(child, &format!("{path}[{index}]"), depth + 1, nodes)?;
            }
            Ok(())
        }
        ActivationRule::Always | ActivationRule::Manual => Ok(()),
    }
}

impl ValidateOrchestration for MemoryProfile {
    fn validate(&self) -> Result<(), OrchestrationValidationError> {
        validate_id("id", self.id.as_str())?;
        validate_text("name", &self.name, 1, MAX_NAME_CHARS)?;
        validate_id("summary_task", self.summary_task.as_str())?;
        validate_id("summary_schema", self.summary_schema.as_str())?;
        if !self
            .summary_schema
            .as_str()
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-' | b':'))
        {
            return Err(OrchestrationValidationError::new(
                "summary_schema",
                "must contain only canonical ASCII identifier characters",
            ));
        }
        if let Some(embedding_task) = &self.embedding_task {
            validate_id("embedding_task", embedding_task.as_str())?;
        }
        let retrieval_weights = [
            self.recency_weight,
            self.similarity_weight,
            self.importance_weight,
        ];
        if self.schema_version == 0
            || self.turns_per_summary == 0
            || self.turns_per_summary > 10_000
            || self.retrieval_count == 0
            || self.retrieval_count > 10_000
            || self.recent_raw_budget.max_tokens > 10_000_000
            || self.episodic_budget.max_tokens > 10_000_000
            || self.semantic_budget.max_tokens > 10_000_000
            || retrieval_weights
                .iter()
                .any(|weight| !weight.is_finite() || *weight < 0.0)
        {
            return Err(OrchestrationValidationError::new(
                "memory_profile",
                "schema, counts, and finite non-negative weights are required",
            ));
        }
        if !retrieval_weights.iter().any(|weight| *weight > 0.0) {
            return Err(OrchestrationValidationError::new(
                "retrieval_weights",
                "at least one retrieval weight must be positive",
            ));
        }
        validate_provenance(&self.provenance, "provenance")
    }
}

impl ValidateOrchestration for InteractionRuleSet {
    fn validate(&self) -> Result<(), OrchestrationValidationError> {
        validate_id("id", self.id.as_str())?;
        validate_text("name", &self.name, 1, MAX_NAME_CHARS)?;
        if self.schema_version == 0 || self.rules.len() > MAX_INTERACTION_RULES {
            return Err(OrchestrationValidationError::new(
                "rules",
                format!("schema must be positive and rules at most {MAX_INTERACTION_RULES}"),
            ));
        }
        if self.max_actions_per_event == 0 || self.max_actions_per_event > 1_024 {
            return Err(OrchestrationValidationError::new(
                "max_actions_per_event",
                "must be between 1 and 1024",
            ));
        }
        for (index, rule) in self.rules.iter().enumerate() {
            validate_id(&format!("rules[{index}].id"), rule.id.as_str())?;
            if rule.actions.is_empty()
                || rule.actions.len()
                    > usize::try_from(self.max_actions_per_event).unwrap_or(usize::MAX)
            {
                return Err(OrchestrationValidationError::new(
                    format!("rules[{index}].actions"),
                    "must be non-empty and within the event action limit",
                ));
            }
            if let Some(condition) = &rule.condition {
                validate_condition(condition, &format!("rules[{index}].condition"))?;
            }
        }
        Ok(())
    }
}

impl ValidateOrchestration for InteractionState {
    fn validate(&self) -> Result<(), OrchestrationValidationError> {
        validate_variable_map(&self.variables, "variables")?;
        if self.manually_active_knowledge.len() > MAX_KNOWLEDGE_ENTRIES {
            return Err(OrchestrationValidationError::new(
                "manually_active_knowledge",
                format!("must contain at most {MAX_KNOWLEDGE_ENTRIES} entries"),
            ));
        }
        let mut active_knowledge = self
            .manually_active_knowledge
            .iter()
            .map(KnowledgeEntryId::as_str)
            .collect::<Vec<_>>();
        active_knowledge.sort_unstable();
        if active_knowledge.windows(2).any(|pair| pair[0] == pair[1]) {
            return Err(OrchestrationValidationError::new(
                "manually_active_knowledge",
                "entry identifiers must be unique",
            ));
        }
        if self.proposals.len() > MAX_INTERACTION_PROPOSALS {
            return Err(OrchestrationValidationError::new(
                "proposals",
                format!("must contain at most {MAX_INTERACTION_PROPOSALS} records"),
            ));
        }
        let mut record_ids = Vec::with_capacity(self.proposals.len());
        let mut pending_ids = Vec::new();
        for (index, proposal) in self.proposals.iter().enumerate() {
            validate_interaction_proposal(proposal, &format!("proposals[{index}]"))?;
            if proposal.source_interaction_state_revision > self.revision {
                return Err(OrchestrationValidationError::new(
                    format!("proposals[{index}].source_interaction_state_revision"),
                    "must not exceed the current interaction state revision",
                ));
            }
            record_ids.push(&proposal.id);
            if proposal.status == InteractionProposalStatus::Pending {
                pending_ids.push(proposal.proposal_id.as_str());
            }
        }
        record_ids.sort();
        pending_ids.sort_unstable();
        if record_ids.windows(2).any(|pair| pair[0] == pair[1]) {
            return Err(OrchestrationValidationError::new(
                "proposals",
                "record identifiers must be unique",
            ));
        }
        if pending_ids.windows(2).any(|pair| pair[0] == pair[1]) {
            return Err(OrchestrationValidationError::new(
                "proposals",
                "only one pending record may exist for a proposal id",
            ));
        }
        Ok(())
    }
}

fn validate_interaction_proposal(
    proposal: &InteractionProposalRecord,
    path: &str,
) -> Result<(), OrchestrationValidationError> {
    validate_id(&format!("{path}.id"), proposal.id.as_str())?;
    validate_id(
        &format!("{path}.rule_set_id"),
        proposal.rule_set_id.as_str(),
    )?;
    validate_id(&format!("{path}.rule_id"), proposal.rule_id.as_str())?;
    validate_id(&format!("{path}.proposal_id"), &proposal.proposal_id)?;
    validate_text(
        &format!("{path}.title"),
        &proposal.title,
        1,
        MAX_INTERACTION_PROPOSAL_TITLE_CHARS,
    )?;
    validate_text(
        &format!("{path}.body"),
        &proposal.body,
        1,
        MAX_INTERACTION_PROPOSAL_BODY_CHARS,
    )?;
    if proposal.requested_at_epoch_seconds < 0 {
        return Err(OrchestrationValidationError::new(
            format!("{path}.requested_at_epoch_seconds"),
            "must be a non-negative Unix timestamp",
        ));
    }
    if proposal
        .expires_at_epoch_seconds
        .is_some_and(|expires| expires < proposal.requested_at_epoch_seconds)
    {
        return Err(OrchestrationValidationError::new(
            format!("{path}.expires_at_epoch_seconds"),
            "must not predate the request",
        ));
    }
    match (proposal.status, proposal.decided_at_epoch_seconds) {
        (InteractionProposalStatus::Pending, None) => Ok(()),
        (InteractionProposalStatus::Pending, Some(_)) => Err(OrchestrationValidationError::new(
            format!("{path}.decided_at_epoch_seconds"),
            "pending proposals must not have a terminal timestamp",
        )),
        (
            InteractionProposalStatus::Approved | InteractionProposalStatus::Rejected,
            Some(decided),
        ) if decided >= proposal.requested_at_epoch_seconds
            && proposal
                .expires_at_epoch_seconds
                .is_none_or(|expires| decided < expires) =>
        {
            Ok(())
        }
        (InteractionProposalStatus::Expired, Some(decided))
            if proposal
                .expires_at_epoch_seconds
                .is_some_and(|expires| decided >= expires) =>
        {
            Ok(())
        }
        (InteractionProposalStatus::Expired, _) => Err(OrchestrationValidationError::new(
            format!("{path}.decided_at_epoch_seconds"),
            "expired proposals require an expiration and a terminal timestamp at or after it",
        )),
        _ => Err(OrchestrationValidationError::new(
            format!("{path}.decided_at_epoch_seconds"),
            "approved and rejected proposals require a timestamp within their valid lifetime",
        )),
    }
}

impl ValidateOrchestration for ContentModule {
    fn validate(&self) -> Result<(), OrchestrationValidationError> {
        validate_id("id", self.id.as_str())?;
        validate_text("name", &self.name, 1, MAX_NAME_CHARS)?;
        validate_text("version", &self.version, 1, MAX_IDENTIFIER_CHARS)?;
        if self.schema_version == 0 {
            return Err(OrchestrationValidationError::new(
                "schema_version",
                "must be positive",
            ));
        }
        let component_count = self.prompt_fragments.len()
            + self.knowledge_book_ids.len()
            + self.control_specs.len()
            + self.transform_set_ids.len()
            + self.interaction_rule_set_ids.len()
            + self.asset_ids.len();
        if component_count > MAX_MODULE_COMPONENTS {
            return Err(OrchestrationValidationError::new(
                "components",
                format!("must contain at most {MAX_MODULE_COMPONENTS} components"),
            ));
        }
        validate_text(
            "metadata.license",
            &self.metadata.license,
            1,
            MAX_NAME_CHARS,
        )?;
        validate_text(
            "metadata.description",
            &self.metadata.description,
            0,
            16_384,
        )?;
        validate_provenance(&self.metadata.provenance, "metadata.provenance")?;
        let mut block_ids = Vec::with_capacity(self.prompt_fragments.len());
        for (index, block) in self.prompt_fragments.iter().enumerate() {
            validate_prompt_block(block, &format!("prompt_fragments[{index}]"))?;
            if block.kind == PromptBlockKind::LatestUserTurn
                || block.placement_zone == PlacementZone::ApplicationPolicy
            {
                return Err(OrchestrationValidationError::new(
                    format!("prompt_fragments[{index}]"),
                    "modules cannot replace fixed application or latest-user blocks",
                ));
            }
            block_ids.push(&block.id);
        }
        block_ids.sort();
        if block_ids.windows(2).any(|pair| pair[0] == pair[1]) {
            return Err(OrchestrationValidationError::new(
                "prompt_fragments",
                "block identifiers must be unique",
            ));
        }
        for (index, control) in self.control_specs.iter().enumerate() {
            validate_control(control, &format!("control_specs[{index}]"))?;
        }
        let mut capabilities = self.required_capabilities.clone();
        capabilities.sort();
        if capabilities.windows(2).any(|pair| pair[0] == pair[1]) {
            return Err(OrchestrationValidationError::new(
                "required_capabilities",
                "capabilities must be unique",
            ));
        }
        Ok(())
    }
}

impl ValidateOrchestration for Persona {
    fn validate(&self) -> Result<(), OrchestrationValidationError> {
        validate_id("id", self.id.as_str())?;
        validate_text("name", &self.name, 1, MAX_NAME_CHARS)?;
        validate_text("description", &self.description, 0, MAX_BLOCK_TEXT_CHARS)?;
        if self.schema_version == 0 {
            return Err(OrchestrationValidationError::new(
                "schema_version",
                "must be positive",
            ));
        }
        if self.updated_at < self.created_at {
            return Err(OrchestrationValidationError::new(
                "timestamps",
                "updated_at must not predate created_at",
            ));
        }
        validate_provenance(&self.provenance, "provenance")
    }
}

impl ValidateOrchestration for PackageManifest {
    fn validate(&self) -> Result<(), OrchestrationValidationError> {
        if self.format != "lorepia_content_package" {
            return Err(OrchestrationValidationError::new(
                "format",
                "unsupported package format",
            ));
        }
        if self.format_version == 0 || self.content_hashes.len() > MAX_PACKAGE_FILES {
            return Err(OrchestrationValidationError::new(
                "content_hashes",
                format!("format must be positive and files at most {MAX_PACKAGE_FILES}"),
            ));
        }
        validate_id("package_id", self.package_id.as_str())?;
        validate_text("name", &self.name, 1, MAX_NAME_CHARS)?;
        validate_text("version", &self.version, 1, MAX_IDENTIFIER_CHARS)?;
        validate_text("license", &self.license, 1, MAX_NAME_CHARS)?;
        validate_provenance(&self.provenance, "provenance")?;
        if let Some(author) = &self.author {
            validate_text("author", author, 1, MAX_NAME_CHARS)?;
        }
        if let Some(required_app_version) = &self.required_app_version {
            validate_text(
                "required_app_version",
                required_app_version,
                1,
                MAX_IDENTIFIER_CHARS,
            )?;
        }
        let mut capabilities = self.required_capabilities.clone();
        capabilities.sort();
        if capabilities.windows(2).any(|pair| pair[0] == pair[1]) {
            return Err(OrchestrationValidationError::new(
                "required_capabilities",
                "capabilities must be unique",
            ));
        }
        if let Some(signature) = &self.signature {
            validate_text("signature.algorithm", &signature.algorithm, 1, 128)?;
            validate_text("signature.key_id", &signature.key_id, 1, 512)?;
            validate_text(
                "signature.signature_base64",
                &signature.signature_base64,
                1,
                16_384,
            )?;
        }
        let mut paths = Vec::with_capacity(self.content_hashes.len());
        let mut total_size_bytes = 0_u64;
        for (index, content) in self.content_hashes.iter().enumerate() {
            validate_text(
                &format!("content_hashes[{index}].logical_path"),
                &content.logical_path,
                1,
                1_024,
            )?;
            if content.logical_path.starts_with('/')
                || content.logical_path.split('/').any(|segment| {
                    segment.is_empty()
                        || segment == "."
                        || segment == ".."
                        || segment.contains('\\')
                })
            {
                return Err(OrchestrationValidationError::new(
                    format!("content_hashes[{index}].logical_path"),
                    "must be a normalized relative package path",
                ));
            }
            total_size_bytes = total_size_bytes
                .checked_add(content.size_bytes)
                .ok_or_else(|| {
                    OrchestrationValidationError::new(
                        "content_hashes",
                        "content size total overflowed",
                    )
                })?;
            if total_size_bytes > 4 * 1_024 * 1_024 * 1_024 {
                return Err(OrchestrationValidationError::new(
                    "content_hashes",
                    "content size total exceeds 4 GiB",
                ));
            }
            paths.push(&content.logical_path);
        }
        paths.sort();
        if paths.windows(2).any(|pair| pair[0] == pair[1]) {
            return Err(OrchestrationValidationError::new(
                "content_hashes",
                "logical paths must be unique",
            ));
        }
        Ok(())
    }
}

impl ValidateOrchestration for TaskProfile {
    fn validate(&self) -> Result<(), OrchestrationValidationError> {
        validate_id("id", self.id.as_str())?;
        validate_id("route_id", self.route_id.as_str())?;
        validate_id("generation_preset_id", self.generation_preset_id.as_str())?;
        if self.fallback_route_ids.len() > 16 {
            return Err(OrchestrationValidationError::new(
                "fallback_route_ids",
                "must contain at most 16 routes",
            ));
        }
        let mut route_ids = self
            .fallback_route_ids
            .iter()
            .map(ModelRouteId::as_str)
            .collect::<Vec<_>>();
        route_ids.sort_unstable();
        if route_ids.windows(2).any(|pair| pair[0] == pair[1])
            || self
                .fallback_route_ids
                .iter()
                .any(|route| route == &self.route_id)
        {
            return Err(OrchestrationValidationError::new(
                "fallback_route_ids",
                "fallback routes must be unique and differ from the primary route",
            ));
        }
        if !(1..=600_000).contains(&self.timeout_ms)
            || self.rate_limit.requests == 0
            || self.rate_limit.requests > 1_000_000
            || self.rate_limit.per_seconds == 0
            || self.rate_limit.per_seconds > 86_400
            || !(1..=128).contains(&self.concurrency_limit)
        {
            return Err(OrchestrationValidationError::new(
                "task_profile",
                "timeout, rate limit, and concurrency must be positive and bounded",
            ));
        }
        match (self.kind, self.embedding_dimensions) {
            (AuxiliaryTaskKind::MemoryEmbedding, Some(dimensions))
                if (1..=32_768).contains(&dimensions) => {}
            (AuxiliaryTaskKind::MemoryEmbedding, _) => {
                return Err(OrchestrationValidationError::new(
                    "embedding_dimensions",
                    "memory embedding tasks require 1 through 32768 dimensions",
                ));
            }
            (_, None) => {}
            (_, Some(_)) => {
                return Err(OrchestrationValidationError::new(
                    "embedding_dimensions",
                    "only memory embedding tasks may declare embedding dimensions",
                ));
            }
        }
        if self.kind == AuxiliaryTaskKind::MemoryEmbedding && !self.fallback_route_ids.is_empty() {
            return Err(OrchestrationValidationError::new(
                "fallback_route_ids",
                "memory embedding tasks use one exact model route and cannot declare fallbacks",
            ));
        }
        Ok(())
    }
}

impl ValidateOrchestration for MemoryRecord {
    fn validate(&self) -> Result<(), OrchestrationValidationError> {
        validate_id("id", self.id.as_str())?;
        validate_text("title", &self.title, 0, 1_024)?;
        validate_text("summary", &self.summary, 1, MAX_BLOCK_TEXT_CHARS)?;
        if self.structured_data.schema_version == 0 {
            return Err(OrchestrationValidationError::new(
                "structured_data.schema_version",
                "must be positive",
            ));
        }
        let encoded = serde_json::to_vec(&self.structured_data.value).map_err(|error| {
            OrchestrationValidationError::new("structured_data.value", error.to_string())
        })?;
        if encoded.len() > 256 * 1_024 {
            return Err(OrchestrationValidationError::new(
                "structured_data.value",
                "serialized value exceeds 256 KiB",
            ));
        }
        validate_versioned_json_value(&self.structured_data.value)?;
        if self.importance > 100 || self.keywords.len() > 256 {
            return Err(OrchestrationValidationError::new(
                "memory_record",
                "importance must be at most 100 and keywords at most 256",
            ));
        }
        if self.updated_at < self.created_at
            || self
                .invalidated_at
                .is_some_and(|invalidated| invalidated < self.created_at)
        {
            return Err(OrchestrationValidationError::new(
                "timestamps",
                "updated and invalidated timestamps must not predate creation",
            ));
        }
        validate_provenance(&self.provenance, "provenance")
    }
}

fn validate_versioned_json_value(
    value: &serde_json::Value,
) -> Result<(), OrchestrationValidationError> {
    fn visit(
        value: &serde_json::Value,
        depth: usize,
        nodes: &mut usize,
    ) -> Result<(), OrchestrationValidationError> {
        if depth > 32 {
            return Err(OrchestrationValidationError::new(
                "structured_data.value",
                "JSON nesting exceeds 32",
            ));
        }
        *nodes += 1;
        if *nodes > 8_192 {
            return Err(OrchestrationValidationError::new(
                "structured_data.value",
                "JSON node count exceeds 8192",
            ));
        }
        match value {
            serde_json::Value::Array(values) => {
                for value in values {
                    visit(value, depth + 1, nodes)?;
                }
            }
            serde_json::Value::Object(values) => {
                for (key, value) in values {
                    validate_text("structured_data.key", key, 1, 512)?;
                    visit(value, depth + 1, nodes)?;
                }
            }
            serde_json::Value::String(value) => {
                validate_text("structured_data.string", value, 0, 16_384)?;
            }
            serde_json::Value::Null | serde_json::Value::Bool(_) | serde_json::Value::Number(_) => {
            }
        }
        Ok(())
    }

    visit(value, 0, &mut 0)
}

impl ValidateOrchestration for MemoryJob {
    fn validate(&self) -> Result<(), OrchestrationValidationError> {
        validate_id("id", self.id.as_str())?;
        validate_text("idempotency_key", &self.idempotency_key, 16, 256)?;
        if self.attempt > 32 || self.updated_at < self.created_at {
            return Err(OrchestrationValidationError::new(
                "memory_job",
                "attempt count or timestamps are invalid",
            ));
        }
        if let Some(error_code) = &self.error_code {
            validate_text("error_code", error_code, 1, MAX_IDENTIFIER_CHARS)?;
        }
        Ok(())
    }
}

impl ValidateOrchestration for ModuleBinding {
    fn validate(&self) -> Result<(), OrchestrationValidationError> {
        validate_id("id", self.id.as_str())?;
        validate_id("module_id", self.module_id.as_str())?;
        validate_id("revision_id", self.revision_id.as_str())?;
        match (
            self.scope,
            self.target_id.as_deref(),
            self.conversation_id.as_ref(),
        ) {
            (ModuleScope::App | ModuleScope::User, None, None) => {}
            (
                ModuleScope::Persona | ModuleScope::Character | ModuleScope::Conversation,
                Some(target),
                None,
            ) => validate_id("target_id", target)?,
            (ModuleScope::Branch, Some(target), Some(conversation_id)) => {
                validate_id("target_id", target)?;
                validate_id("conversation_id", &conversation_id.0)?;
            }
            _ => {
                return Err(OrchestrationValidationError::new(
                    "target_id",
                    "module binding scope, target, and owning conversation are inconsistent",
                ));
            }
        }
        match (&self.resolution_mode, self.pinned_revision_id.as_ref()) {
            (ModuleRevisionResolutionMode::Active, None) => {}
            (ModuleRevisionResolutionMode::Pinned, Some(pinned)) if pinned == &self.revision_id => {
            }
            _ => {
                return Err(OrchestrationValidationError::new(
                    "pinned_revision_id",
                    "pinned mode requires the exact resolved revision; active mode forbids a pin",
                ));
            }
        }
        if let Some(approval_id) = &self.package_import_approval_id {
            validate_id("package_import_approval_id", approval_id)?;
        }
        validate_module_binding_activation(self)?;
        validate_variable_map(&self.variable_overrides, "variable_overrides")
    }
}

fn validate_module_binding_activation(
    binding: &ModuleBinding,
) -> Result<(), OrchestrationValidationError> {
    let approval = (
        binding.activation_approval_id.as_ref(),
        binding.activation_review_sha256.as_ref(),
        binding.activation_plan_sha256.as_ref(),
    );
    match (binding.approved, approval) {
        (false, (None, None, None)) => Ok(()),
        (true, (Some(approval_id), Some(review_sha256), Some(plan_sha256))) => {
            validate_id("activation_approval_id", approval_id)?;
            validate_sha256("activation_review_sha256", review_sha256.as_str())?;
            validate_sha256("activation_plan_sha256", plan_sha256.as_str())
        }
        _ => Err(OrchestrationValidationError::new(
            "approved",
            "approval state requires one complete activation id, review hash, and plan hash tuple",
        )),
    }
}

impl ValidateOrchestration for ContentModuleRevision {
    fn validate(&self) -> Result<(), OrchestrationValidationError> {
        validate_id("id", self.id.as_str())?;
        validate_id("module_id", self.module_id.as_str())?;
        validate_text("version", &self.version, 1, MAX_IDENTIFIER_CHARS)?;
        if self
            .previous_revision_id
            .as_ref()
            .is_some_and(|previous| previous == &self.id)
        {
            return Err(OrchestrationValidationError::new(
                "previous_revision_id",
                "a revision cannot point to itself",
            ));
        }
        if self.component_hashes.len() > MAX_MODULE_COMPONENTS {
            return Err(OrchestrationValidationError::new(
                "component_hashes",
                format!("must contain at most {MAX_MODULE_COMPONENTS} entries"),
            ));
        }
        let mut components = self
            .component_hashes
            .iter()
            .map(|entry| &entry.component)
            .collect::<Vec<_>>();
        components.sort();
        if components.windows(2).any(|pair| pair[0] == pair[1]) {
            return Err(OrchestrationValidationError::new(
                "component_hashes",
                "component references must be unique",
            ));
        }
        Ok(())
    }
}

impl ValidateOrchestration for ResolvedPromptPlan {
    #[allow(clippy::too_many_lines)] // The sealed plan's cross-field invariants are reviewed together.
    fn validate(&self) -> Result<(), OrchestrationValidationError> {
        if self.schema_version != PROMPT_PLAN_SCHEMA_VERSION {
            return Err(OrchestrationValidationError::new(
                "schema_version",
                "unsupported resolved prompt plan schema",
            ));
        }
        validate_sha256("plan_hash", &self.plan_hash)?;
        if let Some(snapshot) = &self.trace.context_snapshot {
            validate_prompt_context_snapshot(snapshot, None)?;
        }
        if self.effective_messages != self.preview.effective_messages
            || self.cache_directives != self.preview.cache_directives
        {
            return Err(OrchestrationValidationError::new(
                "preview",
                "preview must exactly match the effective request materialization",
            ));
        }
        if self.trace.blocks.len() > MAX_PROMPT_BLOCKS {
            return Err(OrchestrationValidationError::new(
                "trace.blocks",
                format!("must contain at most {MAX_PROMPT_BLOCKS} block traces"),
            ));
        }
        let mut traced_block_ids = Vec::with_capacity(self.trace.blocks.len());
        for (index, block) in self.trace.blocks.iter().enumerate() {
            traced_block_ids.push(&block.block_id);
            if let Some(source_id) = &block.source.source_id {
                validate_text(
                    &format!("trace.blocks[{index}].source.source_id"),
                    source_id,
                    1,
                    1_024,
                )?;
            }
            if let Some(source_revision) = &block.source.source_revision {
                validate_text(
                    &format!("trace.blocks[{index}].source.source_revision"),
                    source_revision,
                    1,
                    1_024,
                )?;
            }
            if let Some(source_hash) = &block.source.source_hash {
                validate_sha256(
                    &format!("trace.blocks[{index}].source.source_hash"),
                    source_hash,
                )?;
            }
            let final_tokens = self
                .effective_messages
                .iter()
                .filter(|message| message.block_id == block.block_id)
                .map(|message| message.estimated_tokens)
                .fold(0_u32, u32::saturating_add);
            let produced_message_count = self
                .effective_messages
                .iter()
                .filter(|message| message.block_id == block.block_id)
                .count();
            if block.final_estimated_tokens != final_tokens
                || usize::try_from(block.produced_message_count).ok()
                    != Some(produced_message_count)
            {
                return Err(OrchestrationValidationError::new(
                    format!("trace.blocks[{index}]"),
                    "final token and produced-message counts must match effective messages",
                ));
            }
            if block.memory_evidence.len() > MAX_MEMORY_RECORDS {
                return Err(OrchestrationValidationError::new(
                    format!("trace.blocks[{index}].memory_evidence"),
                    format!("must contain at most {MAX_MEMORY_RECORDS} records"),
                ));
            }
            let mut evidence_ids = Vec::with_capacity(block.memory_evidence.len());
            let mut selected_evidence_ids = Vec::new();
            for (evidence_index, evidence) in block.memory_evidence.iter().enumerate() {
                evidence_ids.push(&evidence.record_id);
                if evidence.selected {
                    selected_evidence_ids.push(&evidence.record_id);
                    if evidence.lane.is_none() || evidence.rank_millionths.is_none() {
                        return Err(OrchestrationValidationError::new(
                            format!("trace.blocks[{index}].memory_evidence[{evidence_index}]"),
                            "selected memory evidence requires a lane and rank",
                        ));
                    }
                }
                if evidence.reasons.len() > 16 {
                    return Err(OrchestrationValidationError::new(
                        format!("trace.blocks[{index}].memory_evidence[{evidence_index}].reasons"),
                        "must contain at most 16 reasons",
                    ));
                }
                if let Some(reason) = &evidence.exclusion_reason {
                    validate_text(
                        &format!(
                            "trace.blocks[{index}].memory_evidence[{evidence_index}].exclusion_reason"
                        ),
                        reason,
                        1,
                        1_024,
                    )?;
                }
            }
            evidence_ids.sort();
            if evidence_ids.windows(2).any(|pair| pair[0] == pair[1]) {
                return Err(OrchestrationValidationError::new(
                    format!("trace.blocks[{index}].memory_evidence"),
                    "record identifiers must be unique",
                ));
            }
            selected_evidence_ids.sort();
            let mut memory_record_ids = block.memory_record_ids.iter().collect::<Vec<_>>();
            memory_record_ids.sort();
            if memory_record_ids != selected_evidence_ids {
                return Err(OrchestrationValidationError::new(
                    format!("trace.blocks[{index}].memory_record_ids"),
                    "must exactly match selected memory evidence",
                ));
            }
        }
        traced_block_ids.sort();
        if traced_block_ids.windows(2).any(|pair| pair[0] == pair[1]) {
            return Err(OrchestrationValidationError::new(
                "trace.blocks",
                "block identifiers must be unique",
            ));
        }
        for (index, message) in self.effective_messages.iter().enumerate() {
            if usize::try_from(message.sequence).ok() != Some(index)
                || message.content.is_empty()
                || message.estimated_tokens == 0
            {
                return Err(OrchestrationValidationError::new(
                    format!("effective_messages[{index}]"),
                    "message sequence, content, and token estimate are invalid",
                ));
            }
            validate_provenance(
                &message.provenance,
                &format!("effective_messages[{index}].provenance"),
            )?;
            if !self
                .trace
                .blocks
                .iter()
                .any(|trace| trace.block_id == message.block_id)
            {
                return Err(OrchestrationValidationError::new(
                    format!("effective_messages[{index}].block_id"),
                    "must reference a block resolution trace",
                ));
            }
        }
        let estimated = self
            .effective_messages
            .iter()
            .map(|message| message.estimated_tokens)
            .fold(0_u32, u32::saturating_add);
        if estimated != self.trace.estimated_input_tokens
            || estimated != self.preview.estimated_input_tokens
            || estimated > self.trace.available_input_tokens
            || self.trace.available_input_tokens != self.preview.available_input_tokens
        {
            return Err(OrchestrationValidationError::new(
                "estimated_input_tokens",
                "token totals must agree and fit the available input budget",
            ));
        }
        if self
            .effective_messages
            .iter()
            .filter(|message| {
                message.block_kind == PromptBlockKind::LatestUserTurn
                    && message.effective_role == ProviderMessageRole::User
            })
            .count()
            != 1
        {
            return Err(OrchestrationValidationError::new(
                "effective_messages",
                "exactly one effective latest-user message is required",
            ));
        }
        if self
            .trace
            .max_context_tokens
            .saturating_sub(self.trace.reserved_output_tokens)
            != self.trace.available_input_tokens
        {
            return Err(OrchestrationValidationError::new(
                "trace.available_input_tokens",
                "must equal context limit minus reserved output",
            ));
        }
        if self.cache_directives.iter().any(|directive| {
            directive.after_message_sequence.is_some_and(|sequence| {
                usize::try_from(sequence)
                    .ok()
                    .is_none_or(|index| index >= self.effective_messages.len())
            })
        }) {
            return Err(OrchestrationValidationError::new(
                "cache_directives",
                "cache message sequence is outside the effective request",
            ));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use chrono::{TimeZone, Utc};

    use super::{
        ActivationRule, AuxiliaryTaskKind, BlockSource, CharacterField, ConditionExpr,
        ContentModuleId, GenerationPresetId, InstructionAuthority, InteractionProposalRecord,
        InteractionProposalRecordId, InteractionProposalStatus, InteractionRuleId,
        InteractionRuleSetId, InteractionState, KnowledgeBook, KnowledgeBookId, KnowledgeEntry,
        KnowledgeEntryId, KnowledgePlacement, MemoryProfile, MemoryProfileId, MergePolicy,
        ModelRouteId, OverflowPolicy, Persona, PersonaId, PlacementZone, PromptBlock,
        PromptBlockId, PromptBlockKind, Provenance, RateLimit, RoleHint, SafeTemplate, SourceKind,
        SummarySchemaId, TaskProfile, TaskProfileId, TokenBudget, TokenPolicy,
        ValidateOrchestration, VariableBinding, VariableId, VariableMap, VariableRef,
        VariableScope, VariableValue, knowledge_parent_edge_visits,
        reset_knowledge_parent_edge_visits, validate_prompt_block,
    };

    fn knowledge_parent_chain(entry_count: usize) -> KnowledgeBook {
        let provenance = Provenance {
            source_kind: SourceKind::UserCreated,
            source_id: None,
            source_hash: None,
            author: None,
            license: None,
            imported_at: None,
        };
        let entries = (0..entry_count)
            .map(|index| KnowledgeEntry {
                id: KnowledgeEntryId::from(format!("entry-{index:04}")),
                book_id: KnowledgeBookId::from("linear-parent-book"),
                name: format!("Entry {index}"),
                content: "content".to_owned(),
                enabled: true,
                activation: ActivationRule::Always,
                priority: 0,
                importance: 0,
                placement: KnowledgePlacement::RetrievedContext,
                token_policy: TokenPolicy {
                    priority: 0,
                    min_tokens: None,
                    max_tokens: None,
                    reserve_tokens: None,
                },
                parent_id: index
                    .checked_sub(1)
                    .map(|parent| KnowledgeEntryId::from(format!("entry-{parent:04}"))),
                activation_probability_basis_points: 10_000,
                provenance: provenance.clone(),
            })
            .collect();
        KnowledgeBook {
            id: KnowledgeBookId::from("linear-parent-book"),
            name: "Linear parent book".to_owned(),
            schema_version: 1,
            entries,
            scan_depth: 8,
            token_budget: TokenBudget { max_tokens: 10_000 },
            recursive: false,
            max_recursion_depth: 0,
            provenance,
        }
    }

    fn valid_memory_profile() -> MemoryProfile {
        MemoryProfile {
            id: MemoryProfileId::from("canonical-memory-profile"),
            name: "Canonical memory profile".to_owned(),
            schema_version: 1,
            summary_task: TaskProfileId::from("memory-summary-task"),
            embedding_task: None,
            turns_per_summary: 8,
            recent_raw_budget: TokenBudget { max_tokens: 1_024 },
            episodic_budget: TokenBudget { max_tokens: 1_024 },
            semantic_budget: TokenBudget { max_tokens: 1_024 },
            retrieval_count: 8,
            recency_weight: 1.0,
            similarity_weight: 0.0,
            importance_weight: 0.0,
            preserve_invalidated_records: false,
            summary_schema: SummarySchemaId::from("memory-summary-schema"),
            provenance: Provenance {
                source_kind: SourceKind::UserCreated,
                source_id: None,
                source_hash: None,
                author: None,
                license: None,
                imported_at: None,
            },
        }
    }

    #[test]
    fn memory_profile_requires_at_least_one_positive_retrieval_weight() {
        let valid = valid_memory_profile();
        valid.validate().expect("one positive weight is canonical");

        let mut invalid = valid;
        invalid.recency_weight = 0.0;
        invalid.similarity_weight = 0.0;
        invalid.importance_weight = 0.0;

        let error = invalid
            .validate()
            .expect_err("an all-zero retrieval policy must be rejected");
        assert_eq!(error.path, "retrieval_weights");
    }

    #[test]
    fn knowledge_book_requires_canonical_recursion_policy() {
        let non_recursive = knowledge_parent_chain(0);
        non_recursive
            .validate()
            .expect("non-recursive with zero depth is canonical");

        let mut recursive = non_recursive.clone();
        recursive.recursive = true;
        recursive.max_recursion_depth = 1;
        recursive
            .validate()
            .expect("recursive with a bounded positive depth is canonical");

        recursive.max_recursion_depth = 0;
        recursive
            .validate()
            .expect("recursive with zero configured depth remains canonical");

        let mut invalid = non_recursive;
        invalid.max_recursion_depth = 1;
        let error = invalid
            .validate()
            .expect_err("non-recursive books cannot carry recursion depth");
        assert_eq!(error.path, "max_recursion_depth");
    }

    #[test]
    fn knowledge_parent_validation_visits_each_edge_once() {
        let entry_count = 256;
        let book = knowledge_parent_chain(entry_count);
        reset_knowledge_parent_edge_visits();

        book.validate().expect("valid parent chain");

        assert_eq!(knowledge_parent_edge_visits(), entry_count - 1);
    }

    #[test]
    fn knowledge_parent_validation_preserves_error_paths() {
        let mut missing = knowledge_parent_chain(3);
        missing.entries[0].parent_id = Some(KnowledgeEntryId::from("missing"));
        let error = missing.validate().expect_err("missing parent");
        assert_eq!(error.path, "entries[0].parent_id");
        assert_eq!(
            error.reason,
            "parent must reference an entry in the same book"
        );

        let mut cycle = knowledge_parent_chain(3);
        cycle.entries[0].parent_id = Some(KnowledgeEntryId::from("entry-0002"));
        let error = cycle.validate().expect_err("parent cycle");
        assert_eq!(error.path, "entries[0].parent_id");
        assert_eq!(error.reason, "parent graph contains a cycle");
    }

    fn source_validation_block(kind: PromptBlockKind, source: BlockSource) -> PromptBlock {
        PromptBlock {
            id: PromptBlockId::from("source-validation"),
            name: "Source validation".to_owned(),
            kind,
            enabled: true,
            role_hint: RoleHint::System,
            authority: InstructionAuthority::Creator,
            template: None,
            condition: None,
            history_selector: matches!(source, BlockSource::History)
                .then_some(super::HistorySelector::All),
            source,
            placement_zone: PlacementZone::RetrievedContext,
            token_policy: TokenPolicy {
                priority: 1,
                min_tokens: None,
                max_tokens: None,
                reserve_tokens: None,
            },
            overflow_policy: OverflowPolicy::DropBlock,
            merge_policy: MergePolicy::SeparateMessage,
            provenance: Provenance {
                source_kind: SourceKind::UserCreated,
                source_id: None,
                source_hash: None,
                author: None,
                license: None,
                imported_at: None,
            },
        }
    }

    #[test]
    fn persisted_template_rejects_unknown_fields() {
        let json = r#"{"parts":[],"max_output_chars":32,"execute":"code"}"#;
        assert!(serde_json::from_str::<SafeTemplate>(json).is_err());
    }

    #[test]
    fn dynamic_prompt_sources_require_their_exact_semantic_kind() {
        for (kind, source) in [
            (
                PromptBlockKind::ConversationSummary,
                BlockSource::ConversationSummary,
            ),
            (PromptBlockKind::AuthorNote, BlockSource::AuthorNote),
            (PromptBlockKind::GroupContext, BlockSource::GroupContext),
            (
                PromptBlockKind::WorldKnowledge,
                BlockSource::SelectedKnowledge,
            ),
            (
                PromptBlockKind::RetrievedMemory,
                BlockSource::SelectedMemory,
            ),
            (
                PromptBlockKind::CharacterPersonality,
                BlockSource::CharacterField {
                    field: CharacterField::Personality,
                },
            ),
        ] {
            validate_prompt_block(&source_validation_block(kind, source), "blocks[0]")
                .expect("coherent source");
        }

        for (kind, source) in [
            (PromptBlockKind::AuthorNote, BlockSource::GroupContext),
            (
                PromptBlockKind::ConversationSummary,
                BlockSource::SelectedMemory,
            ),
            (
                PromptBlockKind::CharacterPersonality,
                BlockSource::CharacterField {
                    field: CharacterField::Scenario,
                },
            ),
        ] {
            assert!(
                validate_prompt_block(&source_validation_block(kind, source), "blocks[0]").is_err()
            );
        }
    }

    #[test]
    fn module_variables_require_an_exact_namespace() {
        let values = VariableMap {
            values: vec![VariableBinding {
                variable: VariableRef {
                    scope: VariableScope::Module,
                    namespace: None,
                    id: VariableId::from("state"),
                },
                value: VariableValue::Bool(true),
            }],
        };
        assert!(values.validate().is_err());

        let mut namespaced = values;
        namespaced.values[0].variable.namespace = Some(ContentModuleId::from("module"));
        assert!(namespaced.validate().is_ok());
    }

    #[test]
    fn condition_depth_is_bounded() {
        let mut expression = ConditionExpr::True;
        for _ in 0..32 {
            expression = ConditionExpr::Not {
                expression: Box::new(expression),
            };
        }
        assert!(expression.validate().is_err());
    }

    #[test]
    fn proposal_audit_status_and_timestamps_are_consistent() {
        let proposal = InteractionProposalRecord {
            id: InteractionProposalRecordId::from("record"),
            rule_set_id: InteractionRuleSetId::from("rules"),
            rule_id: InteractionRuleId::from("rule"),
            proposal_id: "proposal".into(),
            title: "Confirm".into(),
            body: "Apply the declared change?".into(),
            status: InteractionProposalStatus::Pending,
            source_interaction_state_revision: 0,
            requested_at_epoch_seconds: 1_000,
            expires_at_epoch_seconds: Some(1_060),
            decided_at_epoch_seconds: None,
        };
        let mut state = InteractionState {
            variables: VariableMap::default(),
            manually_active_knowledge: Vec::new(),
            proposals: vec![proposal],
            revision: 0,
        };
        assert!(state.validate().is_ok());

        state.proposals[0].status = InteractionProposalStatus::Approved;
        assert!(state.validate().is_err());
        state.proposals[0].decided_at_epoch_seconds = Some(1_010);
        assert!(state.validate().is_ok());
        state.proposals[0].status = InteractionProposalStatus::Expired;
        assert!(state.validate().is_err());
        state.proposals[0].decided_at_epoch_seconds = Some(1_060);
        assert!(state.validate().is_ok());
    }

    #[test]
    fn persona_requires_stable_identity_name_schema_and_timestamp_order() {
        let created_at = Utc
            .with_ymd_and_hms(2026, 8, 3, 12, 0, 0)
            .single()
            .expect("timestamp");
        let mut persona = Persona {
            id: PersonaId::from("persona.local"),
            name: "Local persona".to_owned(),
            description: String::new(),
            schema_version: 1,
            provenance: Provenance {
                source_kind: SourceKind::UserCreated,
                source_id: None,
                source_hash: None,
                author: None,
                license: None,
                imported_at: None,
            },
            created_at,
            updated_at: created_at,
        };
        persona.validate().expect("valid persona");

        persona.updated_at = created_at - chrono::Duration::seconds(1);
        assert!(persona.validate().is_err());
    }

    #[test]
    fn embedding_task_requires_one_exact_route_and_dimension_space() {
        let mut profile = TaskProfile {
            id: TaskProfileId::from("embedding"),
            kind: AuxiliaryTaskKind::MemoryEmbedding,
            route_id: ModelRouteId::from("route"),
            generation_preset_id: GenerationPresetId::from("preset"),
            fallback_route_ids: Vec::new(),
            embedding_dimensions: Some(1536),
            timeout_ms: 30_000,
            rate_limit: RateLimit {
                requests: 10,
                per_seconds: 60,
            },
            concurrency_limit: 2,
        };
        profile.validate().expect("valid embedding task");

        profile.embedding_dimensions = None;
        assert!(profile.validate().is_err());
        profile.embedding_dimensions = Some(1536);
        profile.fallback_route_ids = vec![ModelRouteId::from("fallback")];
        assert!(profile.validate().is_err());

        profile.kind = AuxiliaryTaskKind::MemorySummary;
        profile.fallback_route_ids.clear();
        assert!(profile.validate().is_err());
        profile.embedding_dimensions = None;
        profile.validate().expect("valid non-embedding task");
    }
}
