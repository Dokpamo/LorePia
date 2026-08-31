import type {
    CreatorControlValue,
    OrchestrationConditionExprDto,
    OrchestrationModuleScope,
    OrchestrationVariableMapDto,
    OrchestrationVariableRefDto,
    OrchestrationVariableScope,
    OrchestrationVariableValueDto,
    PromptBlockKind,
    PromptBlockOverflowPolicy,
    PromptBlockRoleHint,
    PromptTokenPolicyDto,
    RevisionedDto,
    SafePromptTemplateDto,
    SafeRegexDto,
} from './contracts/common';

import type {
    CreatorInteractionRuleSetDocumentDto,
    DeleteInteractionRuleSetInput,
    ExplainPromptPlanInput,
    GenerationTargetDto,
    GetInteractionRuleSetInput,
    InteractionProposalListItemDto,
    InteractionStateEntryDto,
    PromptBlockResolutionTraceDto,
    PromptCacheMode,
    PromptCacheRoleFilterDto,
    PromptCacheTtl,
    PromptOverflowTraceDto,
    PromptPlanPreviewDto,
    PromptPlanRequestInput,
    PromptRoleMappingTraceDto,
    PromptWarningCodeDto,
    UpsertInteractionRuleSetInput,
} from './contracts/generation';

import type {
    ContentModuleReviewDto,
    ContentModuleRevisionDiffDocumentDto,
    ContentModuleRevisionListResultDto,
    ContentRevisionDiffDto,
    ContentShareGateDto,
    DiffContentModuleRevisionsInput,
    EvaluateContentModuleShareInput,
    ListContentModuleBindingsInput,
    ListContentModuleRevisionsInput,
    ModuleBindingDocumentDto,
} from './contracts/import';

import type {
    CreatorKnowledgeBookDocumentDto,
    CreatorMemoryProfileDocumentDto,
    DeleteKnowledgeBookInput,
    DeleteMemoryProfileInput,
    DeleteMemoryRecordRequest,
    GetKnowledgeBookInput,
    GetMemoryProfileInput,
    GetMemoryRecordInput,
    InterruptedMemoryJobDto,
    KnowledgeActivationResultDto,
    KnowledgeSimulationDto,
    ListInterruptedMemoryJobsInput,
    ListMemoryRecordsInput,
    ListRetryableMemoryQueryEmbeddingsInput,
    MemoryJobRetryReceiptDto,
    MemoryQueryEmbeddingRetryCandidateDto,
    MemoryRecordDto,
    MemoryRecordListResultDto,
    PatchMemoryRecordRequest,
    PromptSelectionEvidenceDto,
    RetryInterruptedMemoryJobInput,
    RetryMemoryQueryEmbeddingInput,
    SetMemoryRecordExclusionRequest,
    SimulateKnowledgeActivationInput,
    UpsertKnowledgeBookInput,
    UpsertMemoryProfileInput,
} from './contracts/memory';

import type { CapabilityKeyInput, PromptPresetSummaryDto } from './contracts/provider';

export type {
    ImportDynamicContentReviewDto,
    ImportImagePreviewDto,
    ImportInspectionDto,
    ImportIssueDto,
    ImportRegexRuleReviewDto,
    ImportTicketDto,
} from './contracts/import';

export type {
    CharacterDisplayTransformDto,
    CharacterRenderAssetDto,
    CharacterRenderProfileDto,
    CharacterRuntimeKnowledgeDto,
    CharacterRuntimeScriptDto,
    PortableRuntimeCapabilityDto,
    GetPortableRuntimeStateDto,
    GetPortableRuntimeStateInput,
    PortableRuntimeStatePayloadDto,
    PortableRuntimeStatePayloadValueDto,
    PortableRuntimeStateRecordDto,
    PortableRuntimeStateScopeInput,
    PutPortableRuntimeStateInput,
    PutPortableRuntimeStateResultDto,
} from './contracts/portable-runtime';

export {
    SUPPORTED_SHELL_API_VERSION,
    SUPPORTED_CORE_API_VERSION,
    SUPPORTED_CHAT_EVENT_VERSION,
} from './contracts/common';

export type { PlatformKind } from './contracts/platform';

export type { LoadingPhase } from './contracts/common';

export type { ConversationMode, MessageRole, MessageStatus } from './contracts/conversation';

export type { CredentialStatus } from './contracts/provider';

export type { HealthDto, PlatformCapabilitiesDto, BootstrapDto } from './contracts/platform';

export type { MemorySupervisorStatusDto } from './contracts/memory';

export type { FieldErrorDto, ShellErrorDto } from './contracts/common';

export type {
    CharacterDto,
    CharacterGreetingCatalogDto,
    CharacterGreetingSelectionInput,
    AssetDeliverySelector,
    ResolveAssetDeliveryInput,
    AssetDeliveryDto,
} from './contracts/character';

export type {
    InteractionUiRegionDto,
    InteractionChoiceDto,
    InteractionEffectProjectionRejectionReasonDto,
    InteractionEffectDto,
    InteractionEffectEventDto,
    DecideInteractionProposalInput,
    InteractionProposalRecordDto,
    InteractionProposalDecisionReceiptDto,
    ListInteractionProposalsInput,
    InteractionProposalListItemDto,
    ExpireInteractionProposalsInput,
    InteractionProposalExpiryReceiptDto,
    GenerationAttemptProposalDto,
    ListGenerationAttemptProposalsInput,
    GenerationAttemptProposalListItemDto,
    DecideGenerationAttemptProposalInput,
    GenerationAttemptProposalDecisionReceiptDto,
    ExpireGenerationAttemptProposalsInput,
    GenerationAttemptProposalExpiryReceiptDto,
    RetryableGenerationAttemptStatusDto,
    ListRetryableGenerationAttemptsInput,
    RetryableGenerationAttemptDto,
    InteractionEffectHistoryCursorDto,
    ListInteractionEffectHistoryInput,
    InteractionChoiceStatusDto,
    InteractionEffectHistoryItemDto,
    InteractionEffectHistoryPageDto,
    ListReopenInteractionEffectsInput,
    InteractionReopenSnapshotDto,
    SubmitInteractionChoiceInput,
    InteractionChoiceSelectionReceiptDto,
    RoomInteractionClientApi,
    GenerationAttemptApprovalClientApi,
} from './contracts/generation';

export type {
    ConversationDto,
    ConversationStateDto,
    ConversationBranchDto,
    MessageDto,
    MessageTransformStage,
    MessageTransformDisposition,
    MessageTransformDiagnosticDto,
    MessageDisplayProjectionDto,
} from './contracts/conversation';

export type {
    GenerationTargetDto,
    GenerationUsageDto,
    ChatEventKindDto,
    ChatEventDto,
    ChatStreamItemDto,
    GenerationSelectionInput,
    RuntimePromptRoleInput,
    RuntimePromptMessageInput,
    GenerateRuntimeTextInput,
    RuntimeTextGenerationDto,
    SendMessageInput,
    GenerationStartedDto,
    MessageActionGenerationDto,
    EditUserMessageInput,
    RegenerateAssistantMessageInput,
    RemoveMessageInput,
} from './contracts/generation';

export type {
    ConnectionFieldSpecDto,
    ProviderTemplateDto,
    AuthBindingDto,
    ConnectionConfigValueDto,
    ProviderConfigEntryDto,
    CredentialScopeDto,
    ProviderConnectionDto,
    ModelRouteDto,
} from './contracts/provider';

export { CAPABILITY_KEYS } from './contracts/provider';

export type {
    CapabilityKeyInput,
    CapabilityValueDto,
    CapabilityOverrideValueInput,
    CapabilityOverrideStatusInput,
    UpsertCapabilityOverrideInput,
    CapabilityObservationDto,
    EffectiveCapabilityDto,
    ParameterLiteralDto,
    ParameterChoiceDto,
    ParameterConditionDto,
    ParameterConflictDto,
    ProviderParameterMappingDto,
    ParameterSpecDto,
    ParameterValueStateDto,
    GenerationParameterDto,
    GenerationPresetDto,
} from './contracts/provider';

export type {
    PromptBlockRoleHint,
    PromptBlockOverflowPolicy,
    PromptBlockKind,
} from './contracts/common';

export type {
    PromptPresetSummaryDto,
    ListPromptPresetRevisionsInput,
    PromptPresetRevisionSummaryDto,
    PromptPresetRevisionListDto,
    DiffPromptPresetRevisionsInput,
    PromptPresetRevisionDiffDto,
    ReviewPromptPresetRollbackInput,
    PromptPresetRollbackReviewDto,
    ApplyPromptPresetRollbackInput,
    PromptPresetRollbackReceiptDto,
    PromptPresetHistoryClientApi,
} from './contracts/provider';

export type CreatorPromptBlockAuthority = 'creator' | 'user' | 'conversation' | 'imported_content';

export type CreatorPromptBlockPlacementZone =
    | 'preset_instruction'
    | 'character_context'
    | 'retrieved_context'
    | 'older_history'
    | 'recent_enhancement'
    | 'recent_history'
    | 'post_history'
    | 'latest_user'
    | 'assistant_prefill';

export interface CreatorOrchestrationProvenanceDto {
    source_kind: 'user_created' | 'imported_standard' | 'imported_package' | 'generated';
    source_id: string | null;
    source_hash: string | null;
    author: string | null;
    license: string | null;
    imported_at: string | null;
}

export type {
    OrchestrationConditionExprDto,
    SafePromptTemplateDto,
    SafePromptTemplatePartDto,
} from './contracts/common';

export type PromptBlockSourceDto =
    | { kind: 'template' }
    | {
          kind: 'character_field';
          field:
              | 'name'
              | 'description'
              | 'personality'
              | 'scenario'
              | 'first_message'
              | 'dialogue_examples'
              | 'system_instruction'
              | 'post_history_instruction';
      }
    | { kind: 'history' }
    | { kind: 'latest_user' }
    | { kind: 'selected_knowledge' }
    | { kind: 'selected_memory' }
    | { kind: 'conversation_summary' }
    | { kind: 'author_note' }
    | { kind: 'user_persona' }
    | { kind: 'group_context' };

export type PromptHistorySelectorDto =
    | { kind: 'all' }
    | { kind: 'before_recent_turns'; recent_turns: number }
    | { kind: 'recent_turns'; count: number }
    | { kind: 'excluding_latest_user'; count: number }
    | { kind: 'message_range'; start: string; end: string }
    | { kind: 'since_summary'; summary_id: string };

export type { PromptTokenPolicyDto } from './contracts/common';

export type CreatorControlKind =
    | 'toggle'
    | 'select'
    | 'multi_select'
    | 'text'
    | 'number'
    | 'slider'
    | 'section'
    | 'caption'
    | 'divider';

export type OrchestrationVariableType =
    'bool' | 'integer' | 'decimal' | 'text' | 'enum' | 'string_list';

export interface CreatorControlSpecDocumentDto {
    id: string;
    label: string;
    description: string;
    kind: CreatorControlKind;
    value_type: OrchestrationVariableType | null;
    variable: OrchestrationVariableRefDto | null;
    default_value: OrchestrationVariableValueDto | null;
    options: { value: OrchestrationVariableValueDto; label: string }[];
    minimum: number | null;
    maximum: number | null;
    step: number | null;
    visible_when: OrchestrationConditionExprDto | null;
    scope: OrchestrationVariableScope;
    sensitive: boolean;
    requires_regeneration: boolean;
}

export interface PromptCacheBoundaryDocumentDto {
    id: string;
    after_block_id: string;
    role_filter: PromptCacheRoleFilterDto;
    ttl: PromptCacheTtl;
    mode: PromptCacheMode;
}

/**
 * The editable portion of the Rust PromptPreset document. Application-owned
 * authority, ApplicationPolicy placement, and application-built-in
 * provenance are intentionally unrepresentable here and are re-injected by
 * Core after validation.
 */
export interface CreatorPromptBlockDocumentDto {
    id: string;
    name: string;
    kind: PromptBlockKind;
    enabled: boolean;
    role_hint: PromptBlockRoleHint;
    authority: CreatorPromptBlockAuthority;
    template: SafePromptTemplateDto | null;
    condition: OrchestrationConditionExprDto | null;
    source: PromptBlockSourceDto;
    placement_zone: CreatorPromptBlockPlacementZone;
    history_selector: PromptHistorySelectorDto | null;
    token_policy: PromptTokenPolicyDto;
    overflow_policy: PromptBlockOverflowPolicy;
    merge_policy: 'separate_message' | 'merge_with_previous_same_role';
    provenance: CreatorOrchestrationProvenanceDto;
}

export interface CreatorPromptPresetDocumentDto {
    id: string;
    name: string;
    schema_version: number;
    blocks: CreatorPromptBlockDocumentDto[];
    controls: CreatorControlSpecDocumentDto[];
    default_values: OrchestrationVariableMapDto;
    default_generation_preset_id: string | null;
    memory_profile_id: string | null;
    knowledge_book_ids: string[];
    transform_set_ids: string[];
    module_ids: string[];
    cache_boundaries: PromptCacheBoundaryDocumentDto[];
    metadata: {
        description: string;
        tags: string[];
        provenance: CreatorOrchestrationProvenanceDto;
        created_at: string;
        updated_at: string;
        local_override_of: string | null;
    };
}

export interface UpsertPromptPresetInput {
    value: CreatorPromptPresetDocumentDto;
    expected_revision: number | null;
}

export interface DeletePromptPresetInput {
    prompt_preset_id: string;
    expected_revision: number;
}

export interface GetPromptPresetInput {
    prompt_preset_id: string;
}

export interface PromptBlockDto {
    id: string;
    name: string;
    kind: PromptBlockKind;
    enabled: boolean;
    /**
     * Core-owned editability projection. ApplicationPolicy blocks are always
     * false and are never accepted as editable documents from the webview.
     */
    order_editable: boolean;
    role_hint: PromptBlockRoleHint;
    placement_zone: string;
    template_preview: string | null;
    condition_summary: string | null;
    source_label: string;
    provenance_label: string;
    priority: number;
    minimum_tokens: number | null;
    maximum_tokens: number | null;
    overflow_policy: PromptBlockOverflowPolicy;
    cache_boundary_after: boolean;
}

export type { CreatorControlValue } from './contracts/common';

export interface CreatorControlDto {
    id: string;
    label: string;
    description: string | null;
    kind: 'toggle' | 'select' | 'multi_select' | 'text' | 'number' | 'slider';
    value: CreatorControlValue;
    choices: string[];
    minimum: number | null;
    maximum: number | null;
    step: number | null;
}

export interface RoomPromptTemplateSlotDto {
    name: string;
    value: string;
}

export interface RoomOrchestrationConfigDto {
    conversation_id: string;
    branch_id: string;
    prompt_preset_id: string | null;
    generation_preset_id: string | null;
    response_length: 'short' | 'balanced' | 'long';
    creativity: number;
    reasoning_effort:
        'provider_default' | 'minimal' | 'low' | 'medium' | 'high' | 'extra_high' | 'maximum';
    memory_enabled: boolean;
    knowledge_enabled: boolean;
    creator_values: Record<string, CreatorControlValue>;
    variable_overrides: OrchestrationVariableMapDto;
    user_name_override: string | null;
    author_note: string | null;
    group_context: string | null;
    template_slots: RoomPromptTemplateSlotDto[];
    supported_fields: {
        prompt_preset_id: boolean;
        generation_preset_id: boolean;
        creator_values: boolean;
        variable_overrides: boolean;
        response_length: boolean;
        creativity: boolean;
        reasoning_effort: boolean;
        memory_enabled: boolean;
        knowledge_enabled: boolean;
        user_name_override: boolean;
        author_note: boolean;
        group_context: boolean;
        template_slots: boolean;
    };
}

export interface TaskProfileDto {
    id: string;
    name: string;
    task_kind: string;
    model_route_id: string;
    generation_preset_id: string;
    fallback_route_ids: string[];
    embedding_dimensions: number | null;
    timeout_seconds: number;
    concurrency_limit: number;
}

export type AuxiliaryTaskKind =
    | 'memory_summary'
    | 'memory_embedding'
    | 'translation'
    | 'emotion_classification'
    | 'state_extraction'
    | 'image_prompt'
    | 'title_generation';

export interface TaskProfileDocumentDto {
    id: string;
    kind: AuxiliaryTaskKind;
    route_id: string;
    generation_preset_id: string;
    fallback_route_ids: string[];
    embedding_dimensions: number | null;
    timeout_ms: number;
    rate_limit: {
        requests: number;
        per_seconds: number;
    };
    concurrency_limit: number;
}

export interface UpsertTaskProfileInput {
    value: TaskProfileDocumentDto;
    expected_revision: number | null;
}

export interface DeleteTaskProfileInput {
    task_profile_id: string;
    expected_revision: number;
}

export type { CreatorMemoryProfileDocumentDto } from './contracts/memory';

export type { SafeRegexDto } from './contracts/common';

export type {
    KnowledgePlacementDto,
    CreatorKnowledgeActivationRuleDto,
    CreatorKnowledgeEntryDocumentDto,
    CreatorKnowledgeBookDocumentDto,
} from './contracts/memory';

export interface CreatorTransformRuleDocumentDto {
    id: string;
    name: string;
    enabled: boolean;
    phase: TransformPhaseDto;
    order: number;
    pattern: SafeRegexDto;
    replacement: string;
    condition: OrchestrationConditionExprDto | null;
    max_replacements: number;
    input_limit: number;
    output_limit: number;
}

export interface CreatorTransformSetDocumentDto {
    id: string;
    name: string;
    enabled: boolean;
    rules: CreatorTransformRuleDocumentDto[];
    max_rules_per_phase: number;
    max_output_chars: number;
}

export type {
    CreatorValueExprDto,
    CreatorInteractionEventDto,
    CreatorInteractionChoiceDto,
    CreatorInteractionActionDto,
    CreatorInteractionRuleDocumentDto,
    CreatorInteractionRuleSetDocumentDto,
} from './contracts/generation';

export type CreatorModulePromptFragmentDocumentDto = Omit<
    CreatorPromptBlockDocumentDto,
    'provenance'
>;

export type CreatorContentModuleCapabilityDto =
    | 'prompt_fragments'
    | 'knowledge'
    | 'variables'
    | 'transforms'
    | 'declarative_interactions'
    | 'image_assets'
    | 'audio_assets'
    | 'video_assets'
    | 'attachment_assets'
    | 'high_risk_assets';

export interface CreatorContentModuleMetadataDto {
    author: string | null;
    license: string;
    redistribution_allowed: boolean;
    homepage: string | null;
    description: string;
    tags: string[];
}

export interface CreatorContentModuleDocumentDto {
    id: string;
    name: string;
    version: string;
    prompt_fragments: CreatorModulePromptFragmentDocumentDto[];
    knowledge_book_ids: string[];
    control_specs: CreatorControlSpecDocumentDto[];
    transform_set_ids: string[];
    interaction_rule_set_ids: string[];
    asset_ids: string[];
    required_capabilities: CreatorContentModuleCapabilityDto[];
    metadata: CreatorContentModuleMetadataDto;
}

export type {
    UpsertMemoryProfileInput,
    GetMemoryProfileInput,
    DeleteMemoryProfileInput,
    UpsertKnowledgeBookInput,
    GetKnowledgeBookInput,
    DeleteKnowledgeBookInput,
} from './contracts/memory';

export interface UpsertTransformSetInput {
    value: CreatorTransformSetDocumentDto;
    expected_revision: number | null;
}

export interface GetTransformSetInput {
    transform_set_id: string;
}

export interface DeleteTransformSetInput extends GetTransformSetInput {
    expected_revision: number;
}

export type {
    UpsertInteractionRuleSetInput,
    GetInteractionRuleSetInput,
    DeleteInteractionRuleSetInput,
} from './contracts/generation';

export interface UpsertContentModuleInput {
    value: CreatorContentModuleDocumentDto;
    expected_revision: number | null;
}

export interface GetContentModuleInput {
    content_module_id: string;
}

export interface DeleteContentModuleInput extends GetContentModuleInput {
    expected_revision: number;
}

export type {
    PromptSelectionEvidenceDto,
    MemoryRecordDto,
    MemoryRecordSourceNavigationDto,
    MemoryRecordPatchInput,
    KnowledgeSimulationDto,
} from './contracts/memory';

export interface TransformPreviewDto {
    transform_set_id: string;
    rule_id: string;
    phase: TransformPhaseDto;
    input: string;
    output: string;
    changed: boolean;
    rendering: 'native_plain_text';
    used_original: boolean;
    diagnostics: string[];
    reports: TransformRuleReportDto[];
    diff: TransformDiffDto | null;
    error: TransformFailureDto | null;
    truncated: boolean;
}

export type { InteractionStateEntryDto } from './contracts/generation';

export type {
    ContentModuleComponentDto,
    ContentModuleReviewDto,
    ContentRevisionDiffDto,
} from './contracts/import';

export type {
    PromptPlanMessagePreviewDto,
    PromptKnowledgeSelectionReasonDto,
    PromptEvidenceExclusionCodeDto,
    PromptKnowledgeSelectionEvidenceDto,
    PromptMemorySelectionEvidenceDto,
    PromptBlockSourceTraceDto,
    PromptBlockResolutionTraceDto,
    PromptRoleMappingTraceDto,
    PromptOverflowTraceDto,
    PromptCacheDirectivePreviewDto,
    PromptProviderFamily,
    PromptCacheRoleFilterDto,
    PromptCacheTtl,
    PromptCacheMode,
    PromptProviderMessagePreviewDto,
    PromptProviderCacheBoundaryWarning,
    PromptProviderCacheBoundaryDispositionDto,
    PromptProviderCacheBoundaryDto,
    PromptAppliedParameterPreviewDto,
    PromptDiffEntryDto,
    PromptWarningCodeDto,
    PromptPlanPreviewDto,
    ExplainPromptPlanInput,
} from './contracts/generation';

export interface PromptResolutionTraceDto {
    estimator_id: string;
    session_seed: number | null;
    max_context_tokens: number;
    reserved_output_tokens: number;
    available_input_tokens: number;
    estimated_input_tokens: number;
    blocks: PromptBlockResolutionTraceDto[];
    role_mappings: PromptRoleMappingTraceDto[];
    overflow: PromptOverflowTraceDto[];
    warnings: PromptWarningCodeDto[];
    truncated: boolean;
}

export interface OrchestrationWorkspaceDto {
    expected_head: string | null;
    room_config_revision: number | null;
    prompt_preset_revision: number | null;
    interaction_state_revision: number | null;
    /** Exact credential-free target resolved by Core for this room. */
    generation_target: GenerationTargetDto | null;
    prompt_presets: PromptPresetSummaryDto[];
    room_config: RoomOrchestrationConfigDto;
    prompt_blocks: PromptBlockDto[];
    creator_controls: CreatorControlDto[];
    knowledge_book_ids: string[];
    task_profiles: TaskProfileDto[];
    memory_records: MemoryRecordDto[];
    selection_evidence: PromptSelectionEvidenceDto[];
    interaction_state: InteractionStateEntryDto[];
    interaction_proposals: InteractionProposalListItemDto[];
    content_modules: ContentModuleReviewDto[];
    module_diff: ContentRevisionDiffDto | null;
    plan_preview: PromptPlanPreviewDto | null;
}

/**
 * Bounded live snapshot returned by Core. Review/result collections owned by
 * separate commands are deliberately absent and are composed by the
 * controller only after those commands succeed.
 */
export type OrchestrationWorkspaceSnapshotDto = Pick<
    OrchestrationWorkspaceDto,
    | 'expected_head'
    | 'room_config_revision'
    | 'prompt_preset_revision'
    | 'interaction_state_revision'
    | 'generation_target'
    | 'prompt_presets'
    | 'room_config'
    | 'prompt_blocks'
    | 'creator_controls'
    | 'knowledge_book_ids'
    | 'memory_records'
>;

export interface SaveRoomOrchestrationConfigInput {
    conversation_id: string;
    branch_id: string;
    prompt_preset_id: string | null;
    generation_preset_id: string | null;
    response_length: RoomOrchestrationConfigDto['response_length'];
    creativity: number;
    reasoning_effort: RoomOrchestrationConfigDto['reasoning_effort'];
    memory_enabled: boolean;
    knowledge_enabled: boolean;
    creator_values: Record<string, CreatorControlValue>;
    variable_overrides: OrchestrationVariableMapDto;
    user_name_override: string | null;
    author_note: string | null;
    group_context: string | null;
    template_slots: RoomPromptTemplateSlotDto[];
    expected_revision: number | null;
}

export interface SaveRoomOrchestrationConfigResult {
    room_config: RoomOrchestrationConfigDto;
    revision: number;
    generation_target: GenerationTargetDto | null;
}

export interface ReorderPromptBlocksInput {
    prompt_preset_id: string;
    ordered_block_ids: string[];
    expected_revision: number;
}

export interface ReorderPromptBlocksResult {
    blocks: PromptBlockDto[];
    revision: number;
}

export type {
    PatchMemoryRecordRequest,
    DeleteMemoryRecordRequest,
    MemoryRecordExclusionScope,
    SetMemoryRecordExclusionRequest,
} from './contracts/memory';

export interface SimulateKnowledgeRequest {
    knowledge_book_id: string;
    sample_text: string;
    variables: OrchestrationVariableMapDto;
}

export interface PreviewTransformRequest {
    transform_set_id: string;
    rule_id: string;
    sample_text: string;
    variables: OrchestrationVariableMapDto;
}

export type { PromptPlanRequestInput, ReviewedPromptSendInput } from './contracts/generation';

/**
 * High-level, UI-safe orchestration boundary.
 *
 * Implementations must keep raw credentials, host paths, SQLite access, and
 * unrestricted package contents in Rust. The separate interface lets the
 * frontend land before the Tauri commands without pretending unavailable
 * operations succeeded.
 */
export interface OrchestrationClientApi {
    getOrchestrationWorkspace(
        conversationId: string,
        branchId: string,
    ): Promise<OrchestrationWorkspaceSnapshotDto>;
    saveRoomOrchestrationConfig(
        input: SaveRoomOrchestrationConfigInput,
    ): Promise<SaveRoomOrchestrationConfigResult>;
    reorderPromptBlocks(input: ReorderPromptBlocksInput): Promise<ReorderPromptBlocksResult>;
    patchMemoryRecord(input: PatchMemoryRecordRequest): Promise<MemoryRecordDto>;
    deleteMemoryRecord(input: DeleteMemoryRecordRequest): Promise<void>;
    setMemoryRecordExclusion(input: SetMemoryRecordExclusionRequest): Promise<MemoryRecordDto>;
    simulateKnowledge(input: SimulateKnowledgeRequest): Promise<KnowledgeSimulationDto>;
    previewTransform(input: PreviewTransformRequest): Promise<TransformPreviewDto>;
    resolvePromptPreview(input: PromptPlanRequestInput): Promise<PromptPlanPreviewDto>;
}

export type {
    ContentPackageImportStatusDto,
    ContentPackageComponentKindDto,
    ContentPackageComponentDispositionDto,
    ContentPackageIssueSeverityDto,
    ContentPackageRedistributionStatusDto,
    ContentPackageCapabilityDto,
    ApprovableContentPackageCapabilityDto,
    ContentPackageCapabilitySupportDto,
    ContentPackageManifestReviewDto,
    ContentPackageComponentReviewDto,
    ContentPackageIssueDto,
    ContentPackageCapabilityDecisionDto,
    ContentPackageInspectionReviewDto,
    PackageNormalizationEvidenceDto,
    ContentPackageTargetDispositionDto,
    ContentPackageTargetDocumentKindDto,
    ContentPackageTargetReviewDocumentDto,
    ContentPackageTargetReviewDto,
    ConfirmedContentPackageUpdateTargetDto,
    ContentPackageSelectionReviewDto,
    ContentPackageApprovalReviewDto,
    ContentPackageImportReviewDto,
    ContentPackageWorkspaceDto,
    ReopenContentPackageImportInput,
    ListPendingContentPackageImportsInput,
    SelectContentPackageImportInput,
    SelectContentPackageImportReceiptDto,
    ApproveContentPackageImportInput,
    ApproveContentPackageImportReceiptDto,
    CommitContentPackageImportInput,
    CommitContentPackageImportReceiptDto,
    DiscardContentPackageImportInput,
    ContentPackageImportSummaryDto,
    ContentSourceExportInput,
    ContentSourceExportKindDto,
    ContentSourceExportDescriptorDto,
    ContentSourceExportReceiptDto,
    ListCompletedContentPackageExportsInput,
    ContentPackageClientApi,
} from './contracts/import';

export type {
    OrchestrationModuleScope,
    OrchestrationVariableScope,
    OrchestrationVariableValueDto,
    OrchestrationVariableRefDto,
    OrchestrationVariableMapDto,
    RevisionedDto,
} from './contracts/common';

export interface PromptPresetBindingDocumentDto {
    id: string;
    prompt_preset_id: string;
    scope: OrchestrationModuleScope;
    target_id: string | null;
    conversation_id: string | null;
    pinned_revision_id: string | null;
    priority: number;
    enabled: boolean;
    variable_overrides: OrchestrationVariableMapDto;
    generation_preset_override_id: string | null;
    created_at: string;
    updated_at: string;
}

export interface BindPromptPresetInput {
    value: PromptPresetBindingDocumentDto;
    expected_revision: number | null;
}

export interface ListPromptPresetBindingsInput {
    scope: OrchestrationModuleScope;
    target_id: string | null;
}

export interface UnbindPromptPresetInput {
    binding_id: string;
    expected_revision: number;
}

export type {
    MemoryRecordKind,
    ListMemoryRecordsInput,
    GetMemoryRecordInput,
    MemoryRecordListResultDto,
    ListRetryableMemoryQueryEmbeddingsInput,
    RetryMemoryQueryEmbeddingInput,
    MemoryQueryEmbeddingRetryStatus,
    MemoryQueryEmbeddingRetryCandidateDto,
    ListInterruptedMemoryJobsInput,
    MemoryJobRetryKind,
    InterruptedMemoryJobDto,
    RetryInterruptedMemoryJobInput,
    MemoryJobRetryStatus,
    MemoryJobRetryReceiptDto,
    RetrieveMemoryInput,
    MemorySelectionReasonDto,
    MemorySelectionLane,
    SelectedMemoryRecordDto,
    MemorySelectionEvidenceDto,
    MemorySelectionResultDto,
    SemanticKnowledgeScoreDto,
    KnowledgeTokenEstimateInput,
    SimulateKnowledgeActivationInput,
    KnowledgeActivationReasonDto,
    SelectedKnowledgeEntryDto,
    KnowledgeSelectionEvidenceDocumentDto,
    KnowledgeActivationResultDto,
} from './contracts/memory';

export type TransformPhaseDto =
    | 'user_input_for_request'
    | 'resolved_prompt'
    | 'provider_output_canonical'
    | 'display_only'
    | 'memory_input';

export interface PreviewTransformRuleInput {
    transform_set_id: string;
    transform_rule_id: string;
    sample_text: string;
    variables: OrchestrationVariableMapDto;
    supported_capabilities: CapabilityKeyInput[];
    approved_import_source_ids: string[];
    allow_resolved_prompt: boolean;
}

export interface TransformDiffDto {
    unchanged_prefix_chars: number;
    before_fragment: string;
    after_fragment: string;
    unchanged_suffix_chars: number;
    truncated: boolean;
}

export type TransformFailureCodeDto =
    | 'invalid_limits'
    | 'too_many_sets'
    | 'too_many_rules'
    | 'too_many_rules_for_phase'
    | 'duplicate_set_id'
    | 'duplicate_rule_id'
    | 'invalid_identifier'
    | 'invalid_rule_limit'
    | 'pattern_too_large'
    | 'replacement_too_large'
    | 'invalid_regex'
    | 'invalid_replacement'
    | 'input_limit_exceeded'
    | 'output_limit_exceeded'
    | 'condition_failed'
    | 'imported_rule_missing_source';

export interface TransformFailureDto {
    code: TransformFailureCodeDto;
    message: string;
}

export interface TransformRuleReportDto {
    trace: {
        rule_id: string;
        applied: boolean;
        replacements: number;
        input_chars: number;
        output_chars: number;
        error: string | null;
    };
    status:
        | 'applied'
        | 'no_match'
        | 'disabled'
        | 'pending_import_approval'
        | 'resolved_prompt_disabled'
        | 'condition_false'
        | 'failed';
    diff: TransformDiffDto | null;
}

export interface TransformRulePreviewDto {
    phase: TransformPhaseDto;
    original: string;
    output: string;
    changed: boolean;
    rendering: 'native_plain_text';
    reports: TransformRuleReportDto[];
    diff: TransformDiffDto | null;
    error: TransformFailureDto | null;
    truncated: boolean;
}

export type {
    ModuleBindingDocumentDto,
    ListContentModuleBindingsInput,
    ListContentModuleRevisionsInput,
    ContentModuleRevisionSummaryDocumentDto,
    ContentModuleRevisionListResultDto,
    DiffContentModuleRevisionsInput,
    ContentModuleRevisionDiffDocumentDto,
    EvaluateContentModuleShareInput,
    ContentShareGateDto,
} from './contracts/import';

/**
 * Exact low-level document/read-model commands exposed by the Rust shell.
 * These do not manufacture the high-level room workspace used by the product
 * UI; that remains a separate Core-owned DTO.
 */
export interface OrchestrationDocumentClientApi {
    resolvePromptPreview(input: PromptPlanRequestInput): Promise<PromptPlanPreviewDto>;
    explainPromptPlan(input: ExplainPromptPlanInput): Promise<PromptResolutionTraceDto>;
    upsertPromptPreset(
        input: UpsertPromptPresetInput,
    ): Promise<RevisionedDto<PromptPresetSummaryDto>>;
    getPromptPreset(input: GetPromptPresetInput): Promise<RevisionedDto<PromptPresetSummaryDto>>;
    getEditablePromptPreset(
        input: GetPromptPresetInput,
    ): Promise<RevisionedDto<CreatorPromptPresetDocumentDto>>;
    listPromptPresets(): Promise<RevisionedDto<PromptPresetSummaryDto>[]>;
    deletePromptPreset(
        input: DeletePromptPresetInput,
    ): Promise<RevisionedDto<PromptPresetSummaryDto>>;
    reorderPromptBlocks(input: ReorderPromptBlocksInput): Promise<ReorderPromptBlocksResult>;
    listTaskProfiles(): Promise<RevisionedDto<TaskProfileDocumentDto>[]>;
    upsertTaskProfile(
        input: UpsertTaskProfileInput,
    ): Promise<RevisionedDto<TaskProfileDocumentDto>>;
    deleteTaskProfile(
        input: DeleteTaskProfileInput,
    ): Promise<RevisionedDto<TaskProfileDocumentDto>>;
    listMemoryProfiles(): Promise<RevisionedDto<CreatorMemoryProfileDocumentDto>[]>;
    getMemoryProfile(
        input: GetMemoryProfileInput,
    ): Promise<RevisionedDto<CreatorMemoryProfileDocumentDto>>;
    upsertMemoryProfile(
        input: UpsertMemoryProfileInput,
    ): Promise<RevisionedDto<CreatorMemoryProfileDocumentDto>>;
    deleteMemoryProfile(
        input: DeleteMemoryProfileInput,
    ): Promise<RevisionedDto<CreatorMemoryProfileDocumentDto>>;
    listKnowledgeBooks(): Promise<RevisionedDto<CreatorKnowledgeBookDocumentDto>[]>;
    getKnowledgeBook(
        input: GetKnowledgeBookInput,
    ): Promise<RevisionedDto<CreatorKnowledgeBookDocumentDto>>;
    upsertKnowledgeBook(
        input: UpsertKnowledgeBookInput,
    ): Promise<RevisionedDto<CreatorKnowledgeBookDocumentDto>>;
    deleteKnowledgeBook(
        input: DeleteKnowledgeBookInput,
    ): Promise<RevisionedDto<CreatorKnowledgeBookDocumentDto>>;
    listTransformSets(): Promise<RevisionedDto<CreatorTransformSetDocumentDto>[]>;
    getTransformSet(
        input: GetTransformSetInput,
    ): Promise<RevisionedDto<CreatorTransformSetDocumentDto>>;
    upsertTransformSet(
        input: UpsertTransformSetInput,
    ): Promise<RevisionedDto<CreatorTransformSetDocumentDto>>;
    deleteTransformSet(
        input: DeleteTransformSetInput,
    ): Promise<RevisionedDto<CreatorTransformSetDocumentDto>>;
    listInteractionRuleSets(): Promise<RevisionedDto<CreatorInteractionRuleSetDocumentDto>[]>;
    getInteractionRuleSet(
        input: GetInteractionRuleSetInput,
    ): Promise<RevisionedDto<CreatorInteractionRuleSetDocumentDto>>;
    upsertInteractionRuleSet(
        input: UpsertInteractionRuleSetInput,
    ): Promise<RevisionedDto<CreatorInteractionRuleSetDocumentDto>>;
    deleteInteractionRuleSet(
        input: DeleteInteractionRuleSetInput,
    ): Promise<RevisionedDto<CreatorInteractionRuleSetDocumentDto>>;
    listContentModules(): Promise<RevisionedDto<CreatorContentModuleDocumentDto>[]>;
    getContentModule(
        input: GetContentModuleInput,
    ): Promise<RevisionedDto<CreatorContentModuleDocumentDto>>;
    upsertContentModule(
        input: UpsertContentModuleInput,
    ): Promise<RevisionedDto<CreatorContentModuleDocumentDto>>;
    deleteContentModule(
        input: DeleteContentModuleInput,
    ): Promise<RevisionedDto<CreatorContentModuleDocumentDto>>;
    getMemoryRecord(input: GetMemoryRecordInput): Promise<MemoryRecordDto>;
    patchMemoryRecord(input: PatchMemoryRecordRequest): Promise<MemoryRecordDto>;
    setMemoryRecordExclusion(input: SetMemoryRecordExclusionRequest): Promise<MemoryRecordDto>;
    deleteMemoryRecord(input: DeleteMemoryRecordRequest): Promise<void>;
    listPromptPresetBindings(
        input: ListPromptPresetBindingsInput,
    ): Promise<RevisionedDto<PromptPresetBindingDocumentDto>[]>;
    listMemoryRecords(input: ListMemoryRecordsInput): Promise<MemoryRecordListResultDto>;
    listInterruptedMemoryJobs(
        input: ListInterruptedMemoryJobsInput,
    ): Promise<InterruptedMemoryJobDto[]>;
    retryInterruptedMemoryJob(
        input: RetryInterruptedMemoryJobInput,
    ): Promise<MemoryJobRetryReceiptDto>;
    listRetryableMemoryQueryEmbeddings(
        input: ListRetryableMemoryQueryEmbeddingsInput,
    ): Promise<MemoryQueryEmbeddingRetryCandidateDto[]>;
    retryMemoryQueryEmbedding(
        input: RetryMemoryQueryEmbeddingInput,
    ): Promise<MemoryQueryEmbeddingRetryCandidateDto>;
    simulateKnowledgeActivation(
        input: SimulateKnowledgeActivationInput,
    ): Promise<KnowledgeActivationResultDto>;
    listContentModuleBindings(
        input: ListContentModuleBindingsInput,
    ): Promise<RevisionedDto<ModuleBindingDocumentDto>[]>;
    listContentModuleRevisions(
        input: ListContentModuleRevisionsInput,
    ): Promise<ContentModuleRevisionListResultDto>;
    diffContentModuleRevisionDocuments(
        input: DiffContentModuleRevisionsInput,
    ): Promise<ContentModuleRevisionDiffDocumentDto>;
    evaluateContentModuleShare(
        input: EvaluateContentModuleShareInput,
    ): Promise<ContentShareGateDto>;
}

export type {
    AppSettingsDto,
    ProviderProfileDto,
    ProviderNetworkModeInput,
    ProviderLocalNetworkApprovalInput,
    CreateProviderConnectionInput,
    UpdateProviderConnectionInput,
    ApiFamilyInput,
    ModelAvailabilityInput,
    UpsertModelRouteInput,
    GenerationPresetInput,
    ParameterIssueDto,
    ReasoningControlDto,
    PromptCacheTtlDto,
    PromptCacheControlDto,
    ProviderOverviewDto,
    CredentialTargetDto,
    CredentialStatusDto,
    ClipboardCleanupStatus,
    NativeCaptureStatusDto,
    RequestBodyShapeDto,
    RequestBodyFieldDto,
    RequestPreviewDto,
    ModelSyncStartedDto,
    ModelSyncFailureDto,
    ModelSyncSourceProvenanceDto,
    ModelSyncDiffDto,
    ModelSyncReviewDto,
    ModelSyncJobDto,
    ModelSyncEventDto,
} from './contracts/provider';

export type {
    ProviderDiscoveryConnectionOptionsInput,
    ProviderDiscoveryConnectionOptionsDto,
    BeginProviderDiscoverySourceInput,
    BeginProviderDiscoveryInput,
    BeginProviderDiscoveryCurlInput,
    DiscoveryFailureDto,
    DiscoveryReviewChangeDto,
    DiscoveryReviewDto,
} from './contracts/discovery';

export type { JsonValue } from './contracts/common';

export type {
    ProviderDiscoveryApprovalProposalDto,
    ProviderDiscoveryReviewProposalDto,
    DiscoveryAssistantFailureKindInput,
    DiscoveryAssistantInterruptionOutcomeInput,
    DiscoveryAssistantDraftFieldDto,
    DiscoveryAssistantQuestionDto,
    DiscoveryAssistantEvidenceMappingDto,
    DiscoveryAssistantFieldConfidenceDto,
    DiscoveryAssistantConflictDispositionDto,
    DiscoveryAssistantEvidenceConflictDto,
    DiscoveryAssistantManifestSourceDto,
    DiscoveryAssistantEndpointDto,
    DiscoveryAssistantManifestDto,
    DiscoveryAssistantManifestDraftDto,
    DiscoveryAssistantDraftReviewDto,
    DiscoveryAssistantHostActionDto,
    DiscoveryAssistantResumeAction,
    DiscoveryAssistantResumeBoundaryDto,
    DiscoveryStepDto,
    ProviderDiscoverySessionDto,
    CapturedProviderDiscoveryDto,
    DiscoveryCandidateSummaryDto,
    DiscoveryCandidateDto,
    DiscoveryEvidenceDto,
    DiscoveryApprovalRecordDto,
    DiscoveryUnknownOutcomeResolutionInput,
    ContinueProviderDiscoveryActionInput,
    ContinueProviderDiscoveryInput,
    ProviderDiscoveryEventDto,
    DiscoveryOutboxEventDto,
    DiscoveryRecoveryResultDto,
    DiscoveryCompensationRecordDto,
    CatalogChangeKind,
    CatalogHttpMethodDto,
    CatalogEndpointDto,
    CatalogManifestEndpointsDto,
    CatalogDecoderIdDto,
    CatalogManifestDecodersDto,
    CatalogManifestParameterMappingDto,
    CatalogManifestSecuritySurfaceDto,
    CatalogManifestSecurityReviewDto,
    CatalogManifestDiffDto,
    CatalogModelMetadataDiffDto,
    ProviderCatalogDiffDto,
    ProviderCatalogStatusDto,
    ProviderCatalogRevisionSummaryDto,
    ProviderCatalogHistoryDto,
    ProviderCatalogImportPlanDto,
    ProviderCatalogImportTicketDto,
    ProviderCatalogImportResultDto,
    ProviderCatalogRollbackPlanDto,
    ProviderCatalogRollbackResultDto,
} from './contracts/discovery';

export type { ProviderWorkspaceDto, LorepiaClient } from './contracts/platform';
