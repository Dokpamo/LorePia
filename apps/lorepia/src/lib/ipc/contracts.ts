export const SUPPORTED_SHELL_API_VERSION = 2;
export const SUPPORTED_CORE_API_VERSION = 9;
export const SUPPORTED_CHAT_EVENT_VERSION = 4;

export type PlatformKind = 'android' | 'ios' | 'macos' | 'windows';
export type LoadingPhase = 'idle' | 'loading' | 'ready' | 'error';
export type ConversationMode = 'chat' | 'story';
export type MessageRole = 'system' | 'user' | 'assistant';
export type MessageStatus = 'pending' | 'complete' | 'cancelled' | 'failed';
export type CredentialStatus = 'missing' | 'available' | 'unreadable';

export interface HealthDto {
    core_version: string;
    database_open: boolean;
    schema_version: number;
    data_root_writable: boolean;
    staging_writable: boolean;
    recovery_pending: boolean;
    active_jobs: number;
}

export interface PlatformCapabilitiesDto {
    file_picker: boolean;
    credential_store: boolean;
    native_menu: boolean;
    notifications: boolean;
    creator_runtime: boolean;
}

export interface BootstrapDto {
    app_version?: string;
    shell_api_version: number;
    core_version?: string;
    core_api_version: number;
    chat_event_version: number;
    creator_schema_version?: number;
    platform?: PlatformKind;
    health: HealthDto;
    capabilities?: PlatformCapabilitiesDto;
}

export interface MemorySupervisorStatusDto {
    sequence: number;
    phase: 'not_started' | 'recovered' | 'running' | 'failed';
    recovered_interrupted_jobs: number;
    completed_jobs: number;
}

export interface FieldErrorDto {
    field: string;
    message_key: string;
}

export interface ShellErrorDto {
    code: string;
    message_key: string;
    recoverable: boolean;
    operation_id: string | null;
    field_errors: FieldErrorDto[];
}

export interface CharacterDto {
    id: string;
    name: string;
    description: string;
    source_hash: string;
    avatar_asset_id: string | null;
    created_at: string;
}

export interface CharacterGreetingCatalogDto {
    character_id: string;
    character_content_revision_id: string | null;
    greetings: {
        id: string;
        kind: 'default' | 'alternate';
        enabled: boolean;
    }[];
}

export interface CharacterGreetingSelectionInput {
    character_content_revision_id: string | null;
    greeting_id: string | null;
}

export type AssetDeliverySelector =
    { kind: 'asset_id'; asset_id: string } | { kind: 'sha256'; sha256: string };

export interface ResolveAssetDeliveryInput {
    selector: AssetDeliverySelector;
}

export interface AssetDeliveryDto {
    asset_id: string;
    sha256: string;
    media_type: string;
    kind: 'image' | 'audio' | 'video';
    size_bytes: number;
    width: number | null;
    height: number | null;
    duration_ms: number | null;
    url: string;
}

export type InteractionUiRegionDto =
    'message' | 'background' | 'character_portrait' | 'status_panel' | 'audio';

export interface InteractionChoiceDto {
    id: string;
    label: string;
}

export type InteractionEffectProjectionRejectionReasonDto =
    'unsafe_native_text' | 'invalid_stored_effect' | 'asset_unavailable';

export type InteractionEffectDto =
    | { kind: 'state_changed' }
    | { kind: 'knowledge_activated'; entry_id: string }
    | { kind: 'show_asset'; asset: AssetDeliveryDto; region: InteractionUiRegionDto }
    | { kind: 'play_audio'; asset: AssetDeliveryDto }
    | { kind: 'present_choices'; choices: InteractionChoiceDto[] }
    | { kind: 'visible_system_event'; text: string }
    | {
          kind: 'dice_rolled';
          count: number;
          sides: number;
          modifier: number;
          rolls: number[];
          total: number;
      }
    | {
          kind: 'approval_pending';
          title: string;
          body: string;
          expires_after_seconds: number | null;
      }
    | {
          kind: 'projection_rejected';
          reason: InteractionEffectProjectionRejectionReasonDto;
      };

export interface InteractionEffectEventDto {
    delivery_id: string;
    effect_id: string;
    conversation_id: string;
    branch_id: string;
    resulting_state_revision: number;
    event_created_at: string;
    effect: InteractionEffectDto;
}

export interface DecideInteractionProposalInput {
    conversation_id: string;
    branch_id: string;
    proposal_record_id: string;
    expected_state_revision: number;
    expected_proposal_revision: number;
    decision: 'approve' | 'reject';
}

export interface InteractionProposalRecordDto {
    id: string;
    title: string;
    body: string;
    projection_rejection_reason?: 'unsafe_native_text';
    status: 'pending' | 'approved' | 'rejected' | 'expired';
    source_interaction_state_revision: number;
    requested_at_epoch_seconds: number;
    expires_at_epoch_seconds: number | null;
    decided_at_epoch_seconds: number | null;
}

export interface InteractionProposalDecisionReceiptDto {
    proposal: InteractionProposalRecordDto;
    state_revision: number;
    effects: InteractionEffectDto[];
}

export interface ListInteractionProposalsInput {
    conversation_id: string;
    branch_id: string;
    status: 'pending' | 'approved' | 'rejected' | 'expired';
    limit: number;
}

export interface InteractionProposalListItemDto {
    conversation_id: string;
    branch_id: string;
    state_revision: number;
    proposal_revision: number;
    proposal: InteractionProposalRecordDto;
}

export interface ExpireInteractionProposalsInput {
    conversation_id: string;
    branch_id: string;
    limit: number;
}

export interface InteractionProposalExpiryReceiptDto {
    conversation_id: string;
    branch_id: string;
    current_state_revision: number;
    expired_proposals: InteractionProposalListItemDto[];
    has_more_expired: boolean;
}

export interface GenerationAttemptProposalDto {
    id: string;
    title: string;
    body: string;
    projection_rejection_reason?: 'unsafe_native_text';
    status: 'pending' | 'approved' | 'rejected' | 'expired';
    /** Canonical u64 decimal (including zero); never coerce to a JavaScript number. */
    source_interaction_state_revision: string;
    requested_at_epoch_seconds: number;
    expires_at_epoch_seconds: number | null;
    decided_at_epoch_seconds: number | null;
}

export interface ListGenerationAttemptProposalsInput {
    conversation_id: string;
    source_branch_id: string;
    status: 'pending' | 'approved' | 'rejected' | 'expired';
    limit: number;
}

export interface GenerationAttemptProposalListItemDto {
    conversation_id: string;
    source_branch_id: string;
    proposed_branch_id: string;
    generation_id: string;
    /** Canonical positive u64 decimal; echo unchanged for decision CAS. */
    aggregate_revision: string;
    /** Canonical positive u64 decimal; presentation-only. */
    interaction_state_revision: string;
    pending_proposal_count: number;
    /** Canonical positive u64 decimal; echo unchanged for decision CAS. */
    proposal_revision: string;
    proposal: GenerationAttemptProposalDto;
}

export interface DecideGenerationAttemptProposalInput {
    conversation_id: string;
    source_branch_id: string;
    generation_id: string;
    proposal_record_id: string;
    expected_aggregate_revision: string;
    expected_proposal_revision: string;
    decision: 'approve' | 'reject';
}

export interface GenerationAttemptProposalDecisionReceiptDto {
    conversation_id: string;
    source_branch_id: string;
    proposed_branch_id: string;
    generation_id: string;
    aggregate_revision: string;
    interaction_state_revision: string;
    pending_proposal_count: number;
    proposal_revision: string;
    proposal: GenerationAttemptProposalDto;
    approval_evidence_sha256: string | null;
    exact_replay: boolean;
}

export interface ExpireGenerationAttemptProposalsInput {
    conversation_id: string;
    source_branch_id: string;
    limit: number;
}

export interface GenerationAttemptProposalExpiryReceiptDto {
    conversation_id: string;
    source_branch_id: string;
    decisions: GenerationAttemptProposalDecisionReceiptDto[];
    has_more_due: boolean;
}

export type RetryableGenerationAttemptStatusDto = 'before_generation_applied' | 'dispatch_ready';

export interface ListRetryableGenerationAttemptsInput {
    conversation_id: string;
    source_branch_id: string;
    limit: number;
}

/** Safe restart projection; operation, prompt, provider, and credential state stays in Rust. */
export interface RetryableGenerationAttemptDto {
    generation_id: string;
    status: RetryableGenerationAttemptStatusDto;
    created_at: string;
    updated_at: string;
}

export interface InteractionEffectHistoryCursorDto {
    resulting_state_revision: number;
    sequence: number;
}

export interface ListInteractionEffectHistoryInput {
    conversation_id: string;
    branch_id: string;
    after: InteractionEffectHistoryCursorDto | null;
    limit: number;
}

export type InteractionChoiceStatusDto = 'pending' | 'consumed' | 'expired';

export interface InteractionEffectHistoryItemDto {
    effect_id: string;
    conversation_id: string;
    branch_id: string;
    resulting_state_revision: number;
    sequence: number;
    event_created_at: string;
    replay_on_reopen: boolean;
    choice_status: InteractionChoiceStatusDto | null;
    selected_choice_id: string | null;
    choice_decided_at_epoch_seconds: number | null;
    effect: InteractionEffectDto;
}

export interface InteractionEffectHistoryPageDto {
    current_state_revision: number;
    items: InteractionEffectHistoryItemDto[];
    next_cursor: InteractionEffectHistoryCursorDto | null;
}

export interface ListReopenInteractionEffectsInput {
    conversation_id: string;
    branch_id: string;
    limit: number;
}

export interface InteractionReopenSnapshotDto {
    current_state_revision: number;
    items: InteractionEffectHistoryItemDto[];
    older_cursor: InteractionEffectHistoryCursorDto | null;
}

export interface SubmitInteractionChoiceInput {
    conversation_id: string;
    branch_id: string;
    effect_id: string;
    choice_id: string;
    expected_state_revision: number;
}

export interface InteractionChoiceSelectionReceiptDto {
    choice_effect: InteractionEffectHistoryItemDto;
    resulting_state_revision: number;
}

export interface RoomInteractionClientApi {
    expireInteractionProposals(
        input: ExpireInteractionProposalsInput,
    ): Promise<InteractionProposalExpiryReceiptDto>;
    listInteractionProposals(
        input: ListInteractionProposalsInput,
    ): Promise<InteractionProposalListItemDto[]>;
    listInteractionEffectHistory(
        input: ListInteractionEffectHistoryInput,
    ): Promise<InteractionEffectHistoryPageDto>;
    listReopenInteractionEffects(
        input: ListReopenInteractionEffectsInput,
    ): Promise<InteractionReopenSnapshotDto>;
    submitInteractionChoice(
        input: SubmitInteractionChoiceInput,
    ): Promise<InteractionChoiceSelectionReceiptDto>;
}

export interface GenerationAttemptApprovalClientApi {
    expireGenerationAttemptProposals(
        input: ExpireGenerationAttemptProposalsInput,
    ): Promise<GenerationAttemptProposalExpiryReceiptDto>;
    listGenerationAttemptProposals(
        input: ListGenerationAttemptProposalsInput,
    ): Promise<GenerationAttemptProposalListItemDto[]>;
    listRetryableGenerationAttempts(
        input: ListRetryableGenerationAttemptsInput,
    ): Promise<RetryableGenerationAttemptDto[]>;
    decideGenerationAttemptProposal(
        input: DecideGenerationAttemptProposalInput,
    ): Promise<GenerationAttemptProposalDecisionReceiptDto>;
}

export interface ImportTicketDto {
    ticket_id: string;
    display_name: string;
    size_bytes: number;
}

export interface ImportIssueDto {
    code: string;
    message: string;
}

export interface ImportImagePreviewDto {
    logical_asset_id: string;
    media_type: string;
    size_bytes: number;
}

export interface ImportInspectionDto {
    inspection_id: string;
    kind: 'character_card_v3' | 'charx_package';
    display_name: string;
    description: string;
    source_sha256: string;
    source_size: number;
    estimated_stored_size: number;
    asset_count: number;
    representative_image: ImportImagePreviewDto | null;
    warnings: ImportIssueDto[];
    blocked_reasons: string[];
    unsupported_optional_fields: string[];
    allowed: boolean;
}

export interface ConversationDto {
    id: string;
    character_id: string;
    title: string;
    created_at: string;
    updated_at: string;
}

export interface ConversationStateDto {
    conversation_id: string;
    active_branch_id: string;
    selected_mode: ConversationMode;
    updated_at: string;
}

export interface ConversationBranchDto {
    id: string;
    conversation_id: string;
    title: string | null;
    fork_message_id: string | null;
    head_message_id: string | null;
    created_at: string;
    updated_at: string;
}

export interface MessageDto {
    id: string;
    conversation_id: string;
    parent_id: string | null;
    role: MessageRole;
    content: string;
    status: MessageStatus;
    generation_id: string | null;
    created_at: string;
    /** Present only after Rust verified the immutable DisplayOnly sidecar. */
    display_projection?: MessageDisplayProjectionDto;
}

export type MessageTransformStage = 'provider_output_canonical' | 'display_only';

export type MessageTransformDisposition =
    | 'applied'
    | 'no_match'
    | 'disabled'
    | 'pending_import_approval'
    | 'resolved_prompt_disabled'
    | 'condition_false'
    | 'failed'
    | 'limit_rejected'
    | 'pipeline_rejected';

/** Content-free, generation-linked transform evidence safe for expert UI. */
export interface MessageTransformDiagnosticDto {
    set_revision_id: string | null;
    rule_id: string | null;
    stage: MessageTransformStage;
    disposition: MessageTransformDisposition;
    code: string | null;
    before_sha256: string;
    after_sha256: string | null;
    recorded_at: string;
}

export interface MessageDisplayProjectionDto {
    canonical_content_sha256: string;
    display_content_sha256: string;
    diagnostics_sha256: string;
    diagnostics: MessageTransformDiagnosticDto[];
}

export interface GenerationTargetDto {
    model_route_id: string;
    generation_preset_id: string;
}

export interface GenerationUsageDto {
    input_tokens: number | null;
    cached_read_tokens: number | null;
    cached_write_tokens: number | null;
    output_tokens: number | null;
    reasoning_tokens: number | null;
    tool_tokens: number | null;
}

export type ChatEventKindDto =
    | { type: 'generation_started' }
    | { type: 'reasoning_delta'; payload: string }
    | { type: 'text_delta'; payload: string }
    | { type: 'tool_call_started'; payload: { id: string; name: string } }
    | { type: 'tool_call_arguments_delta'; payload: { id: string; delta: string } }
    | { type: 'tool_call_completed'; payload: { id: string } }
    | { type: 'usage_updated'; payload: GenerationUsageDto }
    | { type: 'message_committed'; payload: { message_id: string; status: MessageStatus } }
    | { type: 'generation_cancelled' }
    | { type: 'generation_failed'; payload: { code: string; message: string } }
    | { type: 'generation_finished' };

export interface ChatEventDto {
    event_version: number;
    generation_id: string;
    conversation_id: string;
    branch_id: string | null;
    assistant_message_id: string | null;
    sequence: number;
    emitted_at: string;
    kind: ChatEventKindDto;
}

export type ChatStreamItemDto =
    | { type: 'event'; payload: ChatEventDto }
    | {
          type: 'reconciliation_required';
          payload: {
              reason:
                  | 'broadcast_lagged'
                  | 'unsupported_event_version'
                  | 'route_mismatch'
                  | 'duplicate_or_decreasing_sequence'
                  | 'sequence_gap'
                  | 'live_snapshot'
                  | 'event_after_terminal';
              generation_id: string;
              conversation_id: string;
              branch_id: string;
              last_sequence: number | null;
              observed_sequence: number | null;
              dropped_events: number | null;
              supported_event_version: number;
              display_prefix: string | null;
              reasoning_prefix: string | null;
          };
      }
    | { type: 'closed' };

export type GenerationSelectionInput =
    | { kind: 'legacy_profile'; provider_profile_id: string }
    | { kind: 'target'; target: GenerationTargetDto };

export interface SendMessageInput {
    conversation_id: string;
    branch_id: string;
    expected_head: string | null;
    mode: ConversationMode;
    text: string;
    selection: GenerationSelectionInput;
    /** New and resume identities are mutually exclusive; supply exactly one. */
    operation_nonce?: string | null;
    generation_attempt_id?: string | null;
}

export interface GenerationStartedDto {
    generation_id: string;
}

export interface MessageActionGenerationDto {
    branch: ConversationBranchDto;
    generation_id: string;
}

export interface EditUserMessageInput {
    conversation_id: string;
    branch_id: string;
    expected_head: string | null;
    message_id: string;
    replacement_text: string;
    selection: GenerationSelectionInput;
    /** New and resume identities are mutually exclusive; supply exactly one. */
    operation_nonce?: string | null;
    generation_attempt_id?: string | null;
}

export interface RegenerateAssistantMessageInput {
    conversation_id: string;
    branch_id: string;
    expected_head: string | null;
    message_id: string;
    selection: GenerationSelectionInput;
    /** New and resume identities are mutually exclusive; supply exactly one. */
    operation_nonce?: string | null;
    generation_attempt_id?: string | null;
}

export interface RemoveMessageInput {
    conversation_id: string;
    branch_id: string;
    expected_head: string | null;
    message_id: string;
}

export interface ConnectionFieldSpecDto {
    key: string;
    label_key: string;
    description_key: string | null;
    value_type: string;
    required: boolean;
}

export interface ProviderTemplateDto {
    id: string;
    display_name: string;
    manifest_version: number;
    source: string;
    api_family: string;
    connection_fields: ConnectionFieldSpecDto[];
    default_network_mode: string;
    default_api_origin: string | null;
    credential_required: boolean;
    supports_model_listing: boolean;
    auth_binding: AuthBindingDto;
    parameters: ParameterSpecDto[];
}

export type AuthBindingDto =
    { kind: 'none' } | { kind: 'bearer_header' } | { kind: 'header_api_key'; header_name: string };

export type ConnectionConfigValueDto =
    | { type: 'text'; value: string }
    | { type: 'integer'; value: number }
    | { type: 'boolean'; value: boolean };

export interface ProviderConfigEntryDto {
    key: string;
    value: ConnectionConfigValueDto;
}

export interface CredentialScopeDto {
    allowed_origins: string[];
    auth_binding: AuthBindingDto;
    redirect_policy: string;
}

export interface ProviderConnectionDto {
    id: string;
    template_id: string;
    template_version: number;
    display_name: string;
    api_origin: string;
    api_base_path: string | null;
    network_mode: string;
    local_network_approval: ProviderLocalNetworkApprovalInput | null;
    config_values: ProviderConfigEntryDto[];
    credential_binding_required: boolean;
    credential_status?: CredentialStatus;
    credential_scope: CredentialScopeDto | null;
    approved_credential_origins: string[];
    timeout_seconds: number;
    status: string;
    created_at: string;
    updated_at: string;
}

export interface ModelRouteDto {
    id: string;
    connection_id: string;
    api_family: string;
    model_id: string;
    display_name: string | null;
    route_config: {
        deployment_id: string | null;
        region: string | null;
        endpoint_path: string | null;
        values: ProviderConfigEntryDto[];
    };
    status: string;
    miss_count: number;
    metadata_source: string;
    metadata_observed_at: string | null;
    first_seen_at: string;
    last_seen_at: string | null;
}

export const CAPABILITY_KEYS = [
    'streaming',
    'reasoning',
    'prompt_caching',
    'tool_calling',
    'parallel_tool_calling',
    'structured_output',
    'json_mode',
    'image_input',
    'audio_input',
    'audio_output',
    'logprobs',
    'seed',
    'batch',
    'background',
    'context_window',
    'max_output_tokens',
] as const;

export type CapabilityKeyInput = (typeof CAPABILITY_KEYS)[number];

export type CapabilityValueDto =
    | { type: 'boolean'; value: boolean }
    | { type: 'integer'; value: number }
    | { type: 'enum_values'; value: string[] }
    | { type: 'structured'; value: JsonValue };

export type CapabilityOverrideValueInput =
    | { type: 'boolean'; value: boolean }
    | { type: 'integer'; value: number }
    | { type: 'enum_values'; value: string[] };

export type CapabilityOverrideStatusInput = 'verified' | 'unsupported' | 'unknown' | 'conditional';

export interface UpsertCapabilityOverrideInput {
    id: string;
    model_route_id: string;
    key: CapabilityKeyInput;
    value: CapabilityOverrideValueInput;
    status: CapabilityOverrideStatusInput;
    expires_at: string | null;
}

export interface CapabilityObservationDto {
    id: string;
    model_route_id: string;
    key: string;
    value: CapabilityValueDto;
    status: string;
    source: string;
    confidence: string;
    observed_at: string;
    expires_at: string | null;
    evidence_ref: string | null;
}

export interface EffectiveCapabilityDto {
    selected: CapabilityObservationDto;
    alternatives: CapabilityObservationDto[];
    evaluated_at: string;
    selected_is_stale: boolean;
    has_conflict: boolean;
}

export type ParameterLiteralDto =
    | { type: 'boolean'; value: boolean }
    | { type: 'integer'; value: number }
    | { type: 'number'; value: number }
    | { type: 'string'; value: string }
    | { type: 'enum'; value: string }
    | { type: 'string_list'; value: string[] }
    | { type: 'json_schema'; value: string }
    | { type: 'stop_sequence_list'; value: string[] }
    | { type: 'tool_policy'; value: string };

export interface ParameterChoiceDto {
    value: ParameterLiteralDto;
    label_key: string;
}

export interface ParameterConditionDto {
    parameter_id: string;
    operator: string;
    value: ParameterLiteralDto;
}

export interface ParameterConflictDto {
    parameter_id: string;
    kind: string;
    message_key: string;
}

export interface ProviderParameterMappingDto {
    target: string;
    field_name: string;
}

export interface ParameterSpecDto {
    id: string;
    label_key: string;
    description_key: string | null;
    value_type: string;
    allowed_values: ParameterChoiceDto[];
    minimum: number | null;
    maximum: number | null;
    step: number | null;
    default_mode: string;
    visibility: ParameterConditionDto | null;
    conflicts: ParameterConflictDto[];
    provider_mapping: ProviderParameterMappingDto;
    level: string;
}

export type ParameterValueStateDto =
    { state: 'inherit_provider_default' } | { state: 'explicit'; value: ParameterLiteralDto };

export interface GenerationParameterDto {
    parameter_id: string;
    state: ParameterValueStateDto;
}

export interface GenerationPresetDto {
    id: string;
    model_route_id: string;
    display_name: string;
    values: GenerationParameterDto[];
    reasoning: {
        mode: string;
        effort: string | null;
        budget_tokens: number | null;
        summary: string;
        preserve_opaque_state: boolean;
    };
    prompt_cache: {
        mode: string;
        ttl_kind: string;
        ttl_seconds: number | null;
        context_reference: string | null;
    };
    created_at: string;
    updated_at: string;
}

export type PromptBlockRoleHint =
    'system' | 'developer' | 'user' | 'assistant' | 'provider_default';

export type PromptBlockOverflowPolicy =
    | 'reject'
    | 'drop_block'
    | 'trim_head'
    | 'trim_tail'
    | 'keep_latest_items'
    | 'summarize'
    | 'reduce_knowledge_entries';

export type PromptBlockKind =
    | 'static_instruction'
    | 'character_identity'
    | 'character_description'
    | 'character_personality'
    | 'scenario'
    | 'user_persona'
    | 'dialogue_examples'
    | 'world_knowledge'
    | 'retrieved_memory'
    | 'conversation_summary'
    | 'history_slice'
    | 'latest_user_turn'
    | 'author_note'
    | 'post_history_instruction'
    | 'assistant_prefill'
    | 'group_context';

export interface PromptPresetSummaryDto {
    id: string;
    name: string;
    schema_version: number;
    block_count: number;
    default_generation_preset_id: string | null;
}

export interface ListPromptPresetRevisionsInput {
    prompt_preset_id: string;
    limit: number;
}

export interface PromptPresetRevisionSummaryDto {
    revision_id: string;
    revision: number;
    sha256: string;
    name: string;
    created_at: string;
    rollback_allowed: boolean;
}

export interface PromptPresetRevisionListDto {
    revisions: PromptPresetRevisionSummaryDto[];
    truncated: boolean;
}

export interface DiffPromptPresetRevisionsInput {
    prompt_preset_id: string;
    from_revision: number;
    to_revision: number;
}

export interface PromptPresetRevisionDiffDto {
    preset_id: string;
    from_revision_id: string;
    from_revision: number;
    from_sha256: string;
    to_revision_id: string;
    to_revision: number;
    to_sha256: string;
    changed_paths: string[];
    truncated: boolean;
    diff_sha256: string;
}

export interface ReviewPromptPresetRollbackInput {
    prompt_preset_id: string;
    expected_current_revision: number;
    target_revision: number;
}

export interface PromptPresetRollbackReviewDto {
    review_sha256: string;
    preset_id: string;
    expected_current_state_revision: number;
    expected_current_revision_id: string;
    expected_current_sha256: string;
    target_revision_id: string;
    target_revision: number;
    target_sha256: string;
    target_document_sha256: string;
    target_dependency_sha256: string;
    binding_snapshot_sha256: string;
    diff: PromptPresetRevisionDiffDto;
    reviewed_at: string;
}

export interface ApplyPromptPresetRollbackInput {
    prompt_preset_id: string;
    expected_current_revision: number;
    target_revision: number;
    approval_id: string;
    expected_review_sha256: string;
}

export interface PromptPresetRollbackReceiptDto {
    preset_id: string;
    target_revision: number;
    applied_revision_id: string;
    applied_revision: number;
    applied_sha256: string;
    review_sha256: string;
    approval_id: string;
    approval_sha256: string;
    approved_at: string;
}

export interface PromptPresetHistoryClientApi {
    listPromptPresetRevisions(
        input: ListPromptPresetRevisionsInput,
    ): Promise<PromptPresetRevisionListDto>;
    diffPromptPresetRevisions(
        input: DiffPromptPresetRevisionsInput,
    ): Promise<PromptPresetRevisionDiffDto>;
    reviewPromptPresetRollback(
        input: ReviewPromptPresetRollbackInput,
    ): Promise<PromptPresetRollbackReviewDto>;
    applyPromptPresetRollback(
        input: ApplyPromptPresetRollbackInput,
    ): Promise<PromptPresetRollbackReceiptDto>;
}

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

export type OrchestrationConditionExprDto =
    | { op: 'true' }
    | { op: 'false' }
    | {
          op: 'equals' | 'not_equals';
          variable: OrchestrationVariableRefDto;
          value: OrchestrationVariableValueDto;
      }
    | { op: 'greater_than'; variable: OrchestrationVariableRefDto; value: number }
    | { op: 'contains'; variable: OrchestrationVariableRefDto; value: string }
    | { op: 'exists'; variable: OrchestrationVariableRefDto }
    | { op: 'model_supports'; capability: string }
    | { op: 'all' | 'any'; expressions: OrchestrationConditionExprDto[] }
    | { op: 'not'; expression: OrchestrationConditionExprDto };

export interface SafePromptTemplateDto {
    parts: SafePromptTemplatePartDto[];
    max_output_chars: number;
}

export type SafePromptTemplatePartDto =
    | { kind: 'text'; value: string }
    | { kind: 'variable'; variable: OrchestrationVariableRefDto }
    | {
          kind: 'built_in';
          value:
              | 'character_name'
              | 'user_name'
              | 'persona_name'
              | 'persona_description'
              | 'current_date'
              | 'current_time';
      }
    | { kind: 'slot'; name: string }
    | { kind: 'join'; variable: OrchestrationVariableRefDto; separator: string }
    | {
          kind: 'conditional';
          condition: OrchestrationConditionExprDto;
          then_template: SafePromptTemplateDto;
          else_template: SafePromptTemplateDto | null;
      };

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

export interface PromptTokenPolicyDto {
    priority: number;
    min_tokens: number | null;
    max_tokens: number | null;
    reserve_tokens: number | null;
}

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

export type CreatorControlValue = boolean | number | string | string[];

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

export interface CreatorMemoryProfileDocumentDto {
    id: string;
    name: string;
    summary_task: string;
    embedding_task: string | null;
    turns_per_summary: number;
    recent_raw_budget: { max_tokens: number };
    episodic_budget: { max_tokens: number };
    semantic_budget: { max_tokens: number };
    retrieval_count: number;
    recency_weight: number;
    similarity_weight: number;
    importance_weight: number;
    preserve_invalidated_records: boolean;
    summary_schema: string;
}

export interface SafeRegexDto {
    pattern: string;
    case_insensitive: boolean;
}

export type KnowledgePlacementDto =
    'retrieved_context' | 'before_older_history' | 'before_recent_history' | 'post_history';

export type CreatorKnowledgeActivationRuleDto =
    | { kind: 'always' }
    | { kind: 'manual' }
    | {
          kind: 'keyword';
          primary: string[];
          secondary: string[];
          selective: boolean;
          case_sensitive: boolean;
          whole_word: boolean;
      }
    | { kind: 'regex'; patterns: SafeRegexDto[] }
    | { kind: 'semantic'; threshold: number; top_k: number }
    | { kind: 'condition'; expression: OrchestrationConditionExprDto }
    | { kind: 'any' | 'all'; rules: CreatorKnowledgeActivationRuleDto[] };

export interface CreatorKnowledgeEntryDocumentDto {
    id: string;
    name: string;
    content: string;
    enabled: boolean;
    activation: CreatorKnowledgeActivationRuleDto;
    priority: number;
    importance: number;
    placement: KnowledgePlacementDto;
    token_policy: PromptTokenPolicyDto;
    parent_id: string | null;
    activation_probability_basis_points: number;
}

export interface CreatorKnowledgeBookDocumentDto {
    id: string;
    name: string;
    entries: CreatorKnowledgeEntryDocumentDto[];
    scan_depth: number;
    token_budget: { max_tokens: number };
    recursive: boolean;
    max_recursion_depth: number;
}

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

export type CreatorValueExprDto =
    | { kind: 'literal'; value: OrchestrationVariableValueDto }
    | { kind: 'variable'; variable: OrchestrationVariableRefDto };

export type CreatorInteractionEventDto =
    | { kind: 'conversation_opened' }
    | { kind: 'conversation_started' }
    | { kind: 'before_generation' }
    | { kind: 'after_generation' }
    | { kind: 'message_committed' }
    | { kind: 'user_action'; action_id: string }
    | { kind: 'variable_changed'; variable: OrchestrationVariableRefDto }
    | { kind: 'knowledge_activated'; entry_id: string };

export interface CreatorInteractionChoiceDto {
    id: string;
    label: string;
    value: OrchestrationVariableValueDto;
    enabled_when: OrchestrationConditionExprDto | null;
}

export type CreatorInteractionActionDto =
    | {
          kind: 'set_variable';
          target: OrchestrationVariableRefDto;
          value: CreatorValueExprDto;
      }
    | { kind: 'increment_variable'; target: OrchestrationVariableRefDto; amount: number }
    | { kind: 'activate_knowledge'; entry_id: string }
    | {
          kind: 'show_asset';
          asset_id: string;
          region: 'message' | 'background' | 'character_portrait' | 'status_panel' | 'audio';
      }
    | { kind: 'play_audio'; asset_id: string }
    | { kind: 'present_choices'; choices: CreatorInteractionChoiceDto[] }
    | { kind: 'append_visible_system_event'; text: SafePromptTemplateDto }
    | {
          kind: 'roll_dice';
          expression: { count: number; sides: number; modifier: number };
          target: OrchestrationVariableRefDto | null;
      }
    | {
          kind: 'request_user_approval';
          proposal: {
              id: string;
              title: string;
              body: SafePromptTemplateDto;
              expires_after_seconds: number | null;
          };
      };

export interface CreatorInteractionRuleDocumentDto {
    id: string;
    name: string;
    enabled: boolean;
    event: CreatorInteractionEventDto;
    condition: OrchestrationConditionExprDto | null;
    actions: CreatorInteractionActionDto[];
    priority: number;
    stop_after_match: boolean;
}

export interface CreatorInteractionRuleSetDocumentDto {
    id: string;
    name: string;
    rules: CreatorInteractionRuleDocumentDto[];
    max_actions_per_event: number;
}

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

export interface UpsertMemoryProfileInput {
    value: CreatorMemoryProfileDocumentDto;
    expected_revision: number | null;
}

export interface GetMemoryProfileInput {
    memory_profile_id: string;
}

export interface DeleteMemoryProfileInput extends GetMemoryProfileInput {
    expected_revision: number;
}

export interface UpsertKnowledgeBookInput {
    value: CreatorKnowledgeBookDocumentDto;
    expected_revision: number | null;
}

export interface GetKnowledgeBookInput {
    knowledge_book_id: string;
}

export interface DeleteKnowledgeBookInput extends GetKnowledgeBookInput {
    expected_revision: number;
}

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

export interface UpsertInteractionRuleSetInput {
    value: CreatorInteractionRuleSetDocumentDto;
    expected_revision: number | null;
}

export interface GetInteractionRuleSetInput {
    interaction_rule_set_id: string;
}

export interface DeleteInteractionRuleSetInput extends GetInteractionRuleSetInput {
    expected_revision: number;
}

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

export interface PromptSelectionEvidenceDto {
    id: string;
    source_kind: 'memory' | 'knowledge';
    title: string;
    selected: boolean;
    reason: string;
    score: number | null;
    estimated_tokens: number;
    placement: string | null;
}

export interface MemoryRecordDto {
    id: string;
    conversation_id: string;
    branch_id: string;
    kind: MemoryRecordKind;
    title: string;
    summary: string;
    importance: number;
    keywords: string[];
    pinned: boolean;
    excluded_from_conversation: boolean;
    excluded_from_character: boolean;
    source_navigation: MemoryRecordSourceNavigationDto;
    invalidated_at: string | null;
    updated_at: string;
    revision: number;
}

export interface MemoryRecordSourceNavigationDto {
    conversation_id: string;
    branch_id: string;
    start_message_id: string;
    end_message_id: string;
}

export interface MemoryRecordPatchInput {
    title?: string;
    summary?: string;
    importance?: number;
    keywords?: string[];
    pinned?: boolean;
}

export interface KnowledgeSimulationDto {
    sample_text: string;
    entries: PromptSelectionEvidenceDto[];
    total_estimated_tokens: number;
    truncated: boolean;
}

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

export interface InteractionStateEntryDto {
    id: string;
    label: string;
    value: CreatorControlValue;
    scope: string;
}

export interface ContentModuleComponentDto {
    id: string;
    kind: string;
    name: string;
    selected: boolean;
    enabled: boolean;
}

export interface ContentModuleReviewDto {
    id: string;
    name: string;
    version: string;
    source_label: string;
    license_label: string;
    redistribution_status: 'allowed' | 'blocked' | 'unknown';
    conflicts: string[];
    required_capabilities: string[];
    components: ContentModuleComponentDto[];
    active_revision: number | null;
    available_revision: number;
    revision: number;
    state_revision: number;
    merge_review_sha256: string | null;
    merge_plan_sha256: string | null;
}

export interface ContentRevisionDiffDto {
    module_id: string;
    from_revision: number;
    to_revision: number;
    summary: string[];
    expected_state_revision: number;
    rollback_review_sha256: string | null;
    rollback_plan_sha256: string | null;
}

export interface PromptPlanMessagePreviewDto {
    sequence: number;
    block_id: string;
    block_kind: PromptBlockKind;
    requested_role: PromptBlockRoleHint;
    effective_role: 'system' | 'developer' | 'user' | 'assistant';
    estimated_tokens: number;
    source_message_ids: string[];
    truncated: boolean;
}

export type PromptKnowledgeSelectionReasonDto =
    | { kind: 'always' }
    | { kind: 'manual' }
    | { kind: 'keyword' }
    | { kind: 'regex' }
    | { kind: 'semantic'; score_millionths: number }
    | { kind: 'condition' }
    | { kind: 'recursive'; parent_id: string };

export type PromptEvidenceExclusionCodeDto =
    | 'entry_disabled'
    | 'activation_probability_gate'
    | 'activation_rule_did_not_match'
    | 'per_entry_token_limit'
    | 'knowledge_token_budget_overflow'
    | 'knowledge_remaining_token_budget'
    | 'prompt_token_budget'
    | 'memory_retrieval_count_limit'
    | 'memory_remaining_token_budget'
    | 'other_conversation'
    | 'memory_invalidated'
    | 'excluded_from_conversation'
    | 'excluded_from_character'
    | 'not_on_active_branch_lineage'
    | 'reversed_source_range'
    | 'other';

export interface PromptKnowledgeSelectionEvidenceDto {
    entry_id: string;
    selected: boolean;
    reasons: PromptKnowledgeSelectionReasonDto[];
    estimated_tokens: number;
    exclusion_code: PromptEvidenceExclusionCodeDto | null;
}

export interface PromptMemorySelectionEvidenceDto {
    record_id: string;
    selected: boolean;
    lane: MemorySelectionLane | null;
    rank_millionths: number | null;
    estimated_tokens: number;
    reasons: MemorySelectionReasonDto[];
    exclusion_code: PromptEvidenceExclusionCodeDto | null;
}

export interface PromptBlockSourceTraceDto {
    authority: 'application' | 'creator' | 'user' | 'conversation' | 'imported_content';
    source_kind:
        | 'application_built_in'
        | 'user_created'
        | 'imported_standard'
        | 'imported_package'
        | 'generated';
    source_id: string | null;
    source_revision: string | null;
    source_hash: string | null;
}

export interface PromptBlockResolutionTraceDto {
    block_id: string;
    block_kind: PromptBlockKind;
    source: PromptBlockSourceTraceDto;
    status:
        | 'included'
        | 'condition_false'
        | 'disabled'
        | 'empty'
        | 'dropped_for_budget'
        | 'trimmed_head'
        | 'trimmed_tail'
        | 'reduced_items'
        | 'summarized';
    original_estimated_tokens: number;
    final_estimated_tokens: number;
    produced_message_count: number;
    knowledge_evidence: PromptKnowledgeSelectionEvidenceDto[];
    memory_record_ids: string[];
    memory_evidence: PromptMemorySelectionEvidenceDto[];
    truncated: boolean;
}

export interface PromptRoleMappingTraceDto {
    block_id: string;
    requested_role: PromptBlockRoleHint;
    effective_role: 'system' | 'developer' | 'user' | 'assistant';
}

export interface PromptOverflowTraceDto {
    block_id: string;
    policy: PromptBlockOverflowPolicy;
    tokens_before: number;
    tokens_after: number;
}

export interface PromptCacheDirectivePreviewDto {
    boundary_id: string;
    after_block_id: string;
    after_message_sequence: number | null;
    role_filter: PromptCacheRoleFilterDto;
    ttl: PromptCacheTtl;
    mode: PromptCacheMode;
    status: 'applied' | 'ignored_unsupported' | 'ignored_limit' | 'removed_with_block';
}

export type PromptProviderFamily =
    | 'openai_responses'
    | 'openai_chat_completions'
    | 'anthropic_messages'
    | 'gemini_generate_content'
    | 'ollama_native';

export type PromptCacheRoleFilterDto =
    { kind: 'all' } | { kind: 'system_like' } | { kind: 'exact_role'; role: PromptBlockRoleHint };

export type PromptCacheTtl = 'provider_default' | 'short' | 'long';
export type PromptCacheMode = 'automatic' | 'explicit' | 'disabled';

export interface PromptProviderMessagePreviewDto {
    sequence: number;
    block_id: string;
    effective_role: 'system' | 'developer' | 'user' | 'assistant';
    wire_role: 'system' | 'developer' | 'user' | 'assistant' | 'model';
    placement: 'message' | 'system_instruction';
    estimated_tokens: number;
}

export type PromptProviderCacheBoundaryWarning =
    | 'prompt_caching_unavailable'
    | 'provider_managed_caching_has_no_explicit_boundary'
    | 'explicit_cached_context_must_be_created_separately'
    | 'requested_cache_mode_unavailable'
    | 'cache_disable_unavailable'
    | 'cache_boundary_limit_exceeded'
    | 'cache_boundary_target_was_removed'
    | 'cache_role_filter_unavailable'
    | 'requested_cache_ttl_unavailable';

export type PromptProviderCacheBoundaryDispositionDto =
    | { disposition: 'no_directive' }
    | {
          disposition: 'mapped';
          strategy:
              'provider_managed_automatic' | 'anthropic_inline_breakpoint' | 'caching_disabled';
      }
    | {
          disposition: 'ignored';
          warning: PromptProviderCacheBoundaryWarning;
      };

export interface PromptProviderCacheBoundaryDto {
    boundary_id: string;
    after_block_id: string;
    after_message_sequence: number | null;
    role_filter: PromptCacheRoleFilterDto;
    ttl: PromptCacheTtl;
    mode: PromptCacheMode;
    disposition: PromptProviderCacheBoundaryDispositionDto;
}

export interface PromptAppliedParameterPreviewDto {
    field: string;
    value_kind: 'null' | 'boolean' | 'number' | 'string' | 'array' | 'object';
    item_count: number | null;
}

export interface PromptDiffEntryDto {
    sequence: number;
    block_id: string;
    requested_role: PromptBlockRoleHint;
    effective_role: 'system' | 'developer' | 'user' | 'assistant';
    wire_role: 'system' | 'developer' | 'user' | 'assistant' | 'model';
    placement: 'message' | 'system_instruction';
}

export type PromptWarningCodeDto =
    | 'cache_boundary_ignored_unsupported'
    | 'cache_boundary_ignored_limit'
    | 'reasoning_effort_omitted'
    | 'creativity_ignored'
    | 'knowledge_disabled'
    | 'memory_disabled'
    | 'cache_reuse_suboptimal'
    | 'missing_module_dependencies'
    | 'resolved_prompt_transform_failed'
    | 'resolved_prompt_transform_ignored'
    | 'provider_cache_boundary_ignored'
    | 'other';

export interface PromptPlanPreviewDto {
    generation_attempt_id: string;
    plan_id: string;
    plan_hash: string;
    prompt_preset_id: string;
    prompt_preset_revision: number;
    generation_target: GenerationTargetDto;
    estimated_input_tokens: number;
    available_input_tokens: number;
    token_estimator_id: string;
    token_estimate_exact: boolean;
    messages: PromptPlanMessagePreviewDto[];
    provider_family: PromptProviderFamily;
    provider_messages: PromptProviderMessagePreviewDto[];
    provider_cache_boundaries: PromptProviderCacheBoundaryDto[];
    cache_directives: PromptCacheDirectivePreviewDto[];
    blocks: PromptBlockResolutionTraceDto[];
    role_mappings: PromptRoleMappingTraceDto[];
    overflow: PromptOverflowTraceDto[];
    warnings: PromptWarningCodeDto[];
    truncated: boolean;
    applied_parameters: PromptAppliedParameterPreviewDto[];
    prompt_diff: PromptDiffEntryDto[];
}

export interface ExplainPromptPlanInput extends Omit<PromptPlanRequestInput, 'expected_plan_hash'> {
    plan_hash: string;
}

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

export interface PatchMemoryRecordRequest {
    memory_record_id: string;
    patch: MemoryRecordPatchInput;
    expected_revision: number;
}

export interface DeleteMemoryRecordRequest {
    memory_record_id: string;
    expected_revision: number;
}

export type MemoryRecordExclusionScope = 'conversation' | 'character';

export interface SetMemoryRecordExclusionRequest {
    memory_record_id: string;
    scope: MemoryRecordExclusionScope;
    excluded: boolean;
    expected_revision: number;
}

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

export interface PromptPlanRequestInput {
    conversation_id: string;
    branch_id: string;
    expected_head: string | null;
    user_text: string;
    generation_target: GenerationTargetDto;
    prompt_preset_id: string | null;
    variable_overrides: OrchestrationVariableMapDto;
    expected_plan_hash: string | null;
    /** New and resume identities are mutually exclusive; supply exactly one. */
    operation_nonce?: string | null;
    generation_attempt_id?: string | null;
}

/** Reviewed dispatch resumes only the exact preview attempt; it never accepts a nonce. */
export interface ReviewedPromptSendInput extends Omit<
    PromptPlanRequestInput,
    'expected_plan_hash' | 'operation_nonce' | 'generation_attempt_id'
> {
    expected_plan_hash: string;
    generation_attempt_id: string;
}

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

/**
 * Opaque, platform-owned hand-off for a staged LorePia package. No host path
 * or archive bytes cross the webview boundary.
 */
export type ContentPackageImportStatusDto =
    | 'inspected'
    | 'awaiting_review'
    | 'approved'
    | 'committing'
    | 'completed'
    | 'failed'
    | 'discarded'
    | 'rolled_back';

export type ContentPackageComponentKindDto =
    | 'prompt_preset'
    | 'memory_profile'
    | 'knowledge_book'
    | 'transform_set'
    | 'interaction_rule_set'
    | 'content_module'
    | 'asset_index'
    | 'raw_extension';

export type ContentPackageComponentDispositionDto = 'importable' | 'unsupported' | 'quarantined';

export type ContentPackageIssueSeverityDto = 'warning' | 'blocker';

export type ContentPackageRedistributionStatusDto =
    | 'allowed'
    | 'denied_by_manifest'
    | 'license_unclear'
    | 'provenance_incomplete'
    | 'validation_blocked';

export type ContentPackageCapabilityDto =
    | 'prompt_fragments'
    | 'knowledge'
    | 'variables'
    | 'transforms'
    | 'declarative_interactions'
    | 'image_assets'
    | 'audio_assets'
    | 'video_assets'
    | 'attachment_assets'
    | 'high_risk_assets'
    | 'external_urls'
    | 'html'
    | 'script'
    | 'native_code'
    | 'network'
    | 'filesystem'
    | 'shell'
    | 'credentials';

export type ApprovableContentPackageCapabilityDto = 'transforms' | 'declarative_interactions';

export type ContentPackageCapabilitySupportDto = 'supported' | 'unsupported' | 'approval_required';

export interface ContentPackageManifestReviewDto {
    package_id: string;
    name: string;
    version: string;
    author: string | null;
    license: string;
    redistribution_allowed: boolean;
    required_app_version: string | null;
    required_capabilities: ContentPackageCapabilityDto[];
}

export interface ContentPackageComponentReviewDto {
    id: string;
    kind: ContentPackageComponentKindDto;
    disposition: ContentPackageComponentDispositionDto;
    dependency_ids: string[];
    conflict_ids: string[];
    required_capabilities: ContentPackageCapabilityDto[];
    asset_count: number;
}

export interface ContentPackageIssueDto {
    severity: ContentPackageIssueSeverityDto;
    code: string;
    message: string;
}

export interface ContentPackageCapabilityDecisionDto {
    capability: ContentPackageCapabilityDto;
    support: ContentPackageCapabilitySupportDto;
    approved: boolean;
    reason: string;
}

export interface ContentPackageInspectionReviewDto {
    import_id: string;
    revision: number;
    manifest: ContentPackageManifestReviewDto;
    source_size_bytes: number;
    total_uncompressed_size_bytes: number;
    components: ContentPackageComponentReviewDto[];
    asset_count: number;
    issues: ContentPackageIssueDto[];
    local_import_allowed: boolean;
    redistribution_status: ContentPackageRedistributionStatusDto;
    package_plan_hash: string;
    review_sha256: string;
    capability_review_sha256: string;
    capability_decisions: ContentPackageCapabilityDecisionDto[];
}

export interface PackageNormalizationEvidenceDto {
    component_id: string;
    object_id: string;
    field: string;
    before: boolean;
    after: boolean;
    reason: string;
}

export type ContentPackageTargetDispositionDto = 'create' | 'update';

export type ContentPackageTargetDocumentKindDto =
    | 'prompt_preset'
    | 'knowledge_book'
    | 'memory_profile'
    | 'transform_set'
    | 'interaction_rule_set'
    | 'content_module'
    | 'character_content';

export interface ContentPackageTargetReviewDocumentDto {
    source_component_id: string;
    component_document_ordinal: number;
    document_index: number;
    document_kind: ContentPackageTargetDocumentKindDto;
    target_object_id: string;
    disposition: ContentPackageTargetDispositionDto;
    expected_target_revision_id: string | null;
    expected_target_state_revision: number | null;
    source_component_sha256: string;
    document_sha256: string;
}

export interface ContentPackageTargetReviewDto {
    target_review_sha256: string;
    documents: ContentPackageTargetReviewDocumentDto[];
}

export interface ConfirmedContentPackageUpdateTargetDto {
    source_component_id: string;
    component_document_ordinal: number;
    target_object_id: string;
    expected_target_revision_id: string;
    expected_target_state_revision: number;
}

export interface ContentPackageSelectionReviewDto {
    content_selection_plan_hash: string;
    import_plan_sha256: string;
    normalization_evidence_sha256: string;
    normalization_evidence: PackageNormalizationEvidenceDto[];
    target_review: ContentPackageTargetReviewDto;
}

export interface ContentPackageApprovalReviewDto {
    approval_sha256: string;
    approval_id: string;
    enabled_component_ids: string[];
    approved_capabilities: ApprovableContentPackageCapabilityDto[];
}

export interface ContentPackageImportReviewDto {
    import_id: string;
    package_id: string;
    status: ContentPackageImportStatusDto;
    revision: number;
    package_plan_hash: string;
    review_sha256: string;
    capability_review_sha256: string;
    selected_component_ids: string[];
    selection: ContentPackageSelectionReviewDto | null;
    approval: ContentPackageApprovalReviewDto | null;
}

export interface ContentPackageWorkspaceDto {
    inspection: ContentPackageInspectionReviewDto;
    lifecycle: ContentPackageImportReviewDto;
}

export interface ReopenContentPackageImportInput {
    import_id: string;
}

export interface ListPendingContentPackageImportsInput {
    limit: number;
}

export interface SelectContentPackageImportInput {
    import_id: string;
    expected_revision: number;
    expected_package_plan_hash: string;
    expected_review_sha256: string;
    expected_capability_review_sha256: string;
    selected_component_ids: string[];
}

export interface SelectContentPackageImportReceiptDto {
    import_id: string;
    status: ContentPackageImportStatusDto;
    revision: number;
    package_plan_hash: string;
    review_sha256: string;
    capability_review_sha256: string;
    selected_component_ids: string[];
    selection: ContentPackageSelectionReviewDto;
    required_capabilities: ContentPackageCapabilityDto[];
}

export interface ApproveContentPackageImportInput {
    import_id: string;
    expected_revision: number;
    expected_package_plan_hash: string;
    expected_content_selection_plan_hash: string;
    expected_review_sha256: string;
    expected_import_plan_sha256: string;
    expected_capability_review_sha256: string;
    expected_normalization_evidence_sha256: string;
    expected_target_review_sha256: string;
    approval_id: string;
    enable_component_ids: string[];
    approved_capabilities: ApprovableContentPackageCapabilityDto[];
    confirmed_update_targets: ConfirmedContentPackageUpdateTargetDto[];
}

export interface ApproveContentPackageImportReceiptDto {
    import_id: string;
    status: ContentPackageImportStatusDto;
    revision: number;
    package_plan_hash: string;
    content_selection_plan_hash: string;
    review_sha256: string;
    import_plan_sha256: string;
    capability_review_sha256: string;
    normalization_evidence_sha256: string;
    normalization_evidence: PackageNormalizationEvidenceDto[];
    target_review: ContentPackageTargetReviewDto;
    approval_sha256: string;
    approval_id: string;
    enabled_component_ids: string[];
    approved_capabilities: ApprovableContentPackageCapabilityDto[];
}

export interface CommitContentPackageImportInput {
    import_id: string;
    expected_revision: number;
    expected_package_plan_hash: string;
    expected_content_selection_plan_hash: string;
    expected_review_sha256: string;
    expected_import_plan_sha256: string;
    expected_approval_sha256: string;
    expected_capability_review_sha256: string;
    expected_normalization_evidence_sha256: string;
}

export interface CommitContentPackageImportReceiptDto {
    import_id: string;
    package_id: string;
    status: ContentPackageImportStatusDto;
    revision: number;
    committed_document_ids: string[];
    asset_ids: string[];
}

export interface DiscardContentPackageImportInput {
    import_id: string;
    expected_revision: number;
    expected_review_sha256: string;
    expected_import_plan_sha256: string | null;
    expected_capability_review_sha256: string;
}

export interface ContentPackageImportSummaryDto {
    import_id: string;
    package_id: string;
    status: ContentPackageImportStatusDto;
    revision: number;
    selected_component_ids: string[];
    failure_code: string | null;
    created_at: string;
    updated_at: string;
}

export type ContentSourceExportInput =
    | { kind: 'character_source'; character_id: string }
    | { kind: 'content_package'; import_id: string };

export type ContentSourceExportKindDto = 'character_card_v3' | 'charx_package' | 'lorepia_package';

export interface ContentSourceExportDescriptorDto {
    kind: ContentSourceExportKindDto;
    source_id: string;
    sha256: string;
    size_bytes: number;
    suggested_file_name: string;
}

export interface ContentSourceExportReceiptDto {
    kind: ContentSourceExportKindDto;
    source_id: string;
    sha256: string;
    size_bytes: number;
    file_name: string;
}

export interface ListCompletedContentPackageExportsInput {
    limit: number;
}

export interface ContentPackageClientApi {
    listPendingContentPackageImports(
        input: ListPendingContentPackageImportsInput,
    ): Promise<ContentPackageImportReviewDto[]>;
    pickContentPackageImport(): Promise<ContentPackageInspectionReviewDto | null>;
    reopenContentPackageImport(
        input: ReopenContentPackageImportInput,
    ): Promise<ContentPackageWorkspaceDto>;
    selectContentPackageImport(
        input: SelectContentPackageImportInput,
    ): Promise<SelectContentPackageImportReceiptDto>;
    approveContentPackageImport(
        input: ApproveContentPackageImportInput,
    ): Promise<ApproveContentPackageImportReceiptDto>;
    commitContentPackageImport(
        input: CommitContentPackageImportInput,
    ): Promise<CommitContentPackageImportReceiptDto>;
    discardContentPackageImport(
        input: DiscardContentPackageImportInput,
    ): Promise<ContentPackageImportSummaryDto>;
    listCompletedContentPackageExports(
        input: ListCompletedContentPackageExportsInput,
    ): Promise<ContentSourceExportDescriptorDto[]>;
    exportContentSource(
        input: ContentSourceExportInput,
    ): Promise<ContentSourceExportReceiptDto | null>;
}

export type OrchestrationModuleScope =
    'app' | 'user' | 'persona' | 'character' | 'conversation' | 'branch';

export type OrchestrationVariableScope =
    | 'app'
    | 'user'
    | 'persona'
    | 'character'
    | 'conversation'
    | 'branch'
    | 'session'
    | 'turn'
    | 'module';

export type OrchestrationVariableValueDto =
    | { type: 'bool'; value: boolean }
    | { type: 'integer'; value: number }
    | { type: 'decimal'; value: number }
    | { type: 'text'; value: string }
    | { type: 'enum'; value: string }
    | { type: 'string_list'; value: string[] };

export interface OrchestrationVariableRefDto {
    scope: OrchestrationVariableScope;
    namespace: string | null;
    id: string;
}

export interface OrchestrationVariableMapDto {
    values: {
        variable: OrchestrationVariableRefDto;
        value: OrchestrationVariableValueDto;
    }[];
}

export interface RevisionedDto<Value> {
    value: Value;
    revision: number;
    created_at: string;
    updated_at: string;
    deleted_at: string | null;
}

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

export type MemoryRecordKind =
    | 'episodic_event'
    | 'character_fact'
    | 'relationship_change'
    | 'user_preference'
    | 'world_state'
    | 'unresolved_thread'
    | 'conversation_summary'
    | 'creator_pinned';

export interface ListMemoryRecordsInput {
    conversation_id: string;
    branch_id: string;
    include_invalidated: boolean;
}

export interface GetMemoryRecordInput {
    memory_record_id: string;
}

export interface MemoryRecordListResultDto {
    records: MemoryRecordDto[];
    truncated: boolean;
}

export interface ListRetryableMemoryQueryEmbeddingsInput {
    conversation_id: string;
    branch_id: string;
    limit: number;
}

export interface RetryMemoryQueryEmbeddingInput {
    id: string;
    expected_revision: number;
    acknowledge_unknown_outcome: boolean;
}

export type MemoryQueryEmbeddingRetryStatus = 'interrupted' | 'failed' | 'cancelled' | 'queued';

export interface MemoryQueryEmbeddingRetryCandidateDto {
    id: string;
    status: MemoryQueryEmbeddingRetryStatus;
    revision: number;
    conversation_id: string;
    branch_id: string;
    error_code: string | null;
    requires_unknown_outcome_acknowledgement: boolean;
}

export interface RetrieveMemoryInput {
    conversation_id: string;
    branch_id: string;
    memory_profile_id: string;
    visible_message_ids: string[];
    query_texts: string[];
}

export type MemorySelectionReasonDto =
    | { kind: 'pinned' }
    | { kind: 'current_branch' }
    | { kind: 'shared_ancestor'; source_branch_id: string }
    | { kind: 'recency'; score_millionths: number }
    | { kind: 'similarity'; score_millionths: number }
    | { kind: 'importance'; score_millionths: number };

export type MemorySelectionLane = 'pinned' | 'semantic' | 'episodic';

export interface SelectedMemoryRecordDto {
    record_id: string;
    kind: MemoryRecordKind;
    title: string;
    summary: string;
    lane: MemorySelectionLane;
    rank_millionths: number;
    estimated_tokens: number;
    reasons: MemorySelectionReasonDto[];
}

export interface MemorySelectionEvidenceDto {
    record_id: string;
    selected: boolean;
    lane: MemorySelectionLane | null;
    rank_millionths: number | null;
    estimated_tokens: number;
    reasons: MemorySelectionReasonDto[];
    exclusion_reason: string | null;
}

export interface MemorySelectionResultDto {
    selected: SelectedMemoryRecordDto[];
    evidence: MemorySelectionEvidenceDto[];
    used_episodic_tokens: number;
    used_semantic_tokens: number;
    truncated: boolean;
}

export interface SemanticKnowledgeScoreDto {
    entry_id: string;
    score: number;
}

export interface KnowledgeTokenEstimateInput {
    knowledge_entry_id: string;
    tokens: number;
}

export interface SimulateKnowledgeActivationInput {
    knowledge_book_id: string;
    sample_texts: string[];
    manual_entry_ids: string[];
    semantic_scores: SemanticKnowledgeScoreDto[];
    variables: OrchestrationVariableMapDto;
    supported_capabilities: CapabilityKeyInput[];
    token_estimates: KnowledgeTokenEstimateInput[];
    activation_seed: number;
}

export type KnowledgeActivationReasonDto =
    | { kind: 'always' }
    | { kind: 'manual' }
    | { kind: 'keyword'; matched: string }
    | { kind: 'regex'; pattern: string }
    | { kind: 'semantic'; score_millionths: number }
    | { kind: 'condition' }
    | { kind: 'recursive'; parent_id: string };

export interface SelectedKnowledgeEntryDto {
    entry_id: string;
    content: string;
    placement:
        'retrieved_context' | 'before_older_history' | 'before_recent_history' | 'post_history';
    estimated_tokens: number;
    recursion_depth: number;
    reasons: KnowledgeActivationReasonDto[];
}

export interface KnowledgeSelectionEvidenceDocumentDto {
    entry_id: string;
    selected: boolean;
    reasons: KnowledgeActivationReasonDto[];
    estimated_tokens: number;
    exclusion_reason: string | null;
}

export interface KnowledgeActivationResultDto {
    selected: SelectedKnowledgeEntryDto[];
    evidence: KnowledgeSelectionEvidenceDocumentDto[];
    used_tokens: number;
    token_budget: number;
    truncated: boolean;
}

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

export interface ModuleBindingDocumentDto {
    id: string;
    module_id: string;
    scope: OrchestrationModuleScope;
    target_id: string | null;
    enabled: boolean;
    approved: boolean;
    revision_id: string;
    created_at: string;
}

export interface ListContentModuleBindingsInput {
    content_module_id: string;
}

export interface ListContentModuleRevisionsInput {
    content_module_id: string;
}

export interface ContentModuleRevisionSummaryDocumentDto {
    revision_id: string;
    revision: number;
    sha256: string;
    created_at: string;
}

export interface ContentModuleRevisionListResultDto {
    revisions: ContentModuleRevisionSummaryDocumentDto[];
    truncated: boolean;
}

export interface DiffContentModuleRevisionsInput {
    content_module_id: string;
    from_revision: number;
    to_revision: number;
}

export interface ContentModuleRevisionDiffDocumentDto {
    content_module_id: string;
    from_revision: number;
    to_revision: number;
    from_sha256: string;
    to_sha256: string;
    changed_paths: string[];
    truncated: boolean;
}

export interface EvaluateContentModuleShareInput {
    content_module_id: string;
}

export interface ContentShareGateDto {
    module_id: string;
    local_use_allowed: boolean;
    sharing_allowed: boolean;
    reasons: string[];
}

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

export interface AppSettingsDto {
    preserve_partial_generations: boolean;
    selected_provider_profile_id: string | null;
    selected_model_route_id: string | null;
    selected_generation_preset_id: string | null;
}

export interface ProviderProfileDto {
    id: string;
    display_name: string;
    base_url: string;
    model: string;
    timeout_seconds: number;
}

export type ProviderNetworkModeInput = 'public' | 'local_loopback' | 'approved_local_network';

export interface ProviderLocalNetworkApprovalInput {
    origin: string;
    addresses: string[];
}

export interface CreateProviderConnectionInput {
    id: string;
    template_id: string;
    template_version: number;
    display_name: string;
    api_origin: string;
    api_base_path: string | null;
    network_mode: ProviderNetworkModeInput;
    local_network_approval: ProviderLocalNetworkApprovalInput | null;
    values: ProviderConfigEntryDto[];
    approved_credential_origin: string | null;
    timeout_seconds: number;
}

export interface UpdateProviderConnectionInput {
    id: string;
    display_name: string;
    timeout_seconds: number;
}

export type ApiFamilyInput =
    | 'open_ai_responses'
    | 'open_ai_chat_completions'
    | 'anthropic_messages'
    | 'gemini_generate_content'
    | 'ollama_native';

export type ModelAvailabilityInput =
    | 'available'
    | 'missing_temporarily'
    | 'documented_only'
    | 'access_denied'
    | 'deprecated'
    | 'retired'
    | 'unknown';

export type UpsertModelRouteInput =
    | {
          kind: 'create';
          id: string;
          connection_id: string;
          api_family: ApiFamilyInput;
          model_id: string;
          display_name: string | null;
          route_config: ModelRouteDto['route_config'];
          status: ModelAvailabilityInput;
      }
    | {
          kind: 'update';
          id: string;
          display_name: string | null;
          status: ModelAvailabilityInput;
      };

export interface GenerationPresetInput {
    id: string;
    model_route_id: string;
    display_name: string;
    values: GenerationParameterDto[];
    reasoning: GenerationPresetDto['reasoning'];
    prompt_cache: GenerationPresetDto['prompt_cache'];
}

export interface ParameterIssueDto {
    code: string;
    parameter_id: string | null;
    related_parameter_id: string | null;
}

export interface ReasoningControlDto {
    state: string;
    settings: GenerationPresetDto['reasoning'];
    allowed_modes: string[];
    allowed_efforts: string[];
    allowed_summaries: string[];
    budget_bounds: { minimum: number; maximum: number } | null;
    effort_field: string;
    budget_field: string;
    summary_field: string;
    issues: ParameterIssueDto[];
}

export type PromptCacheTtlDto =
    | { kind: 'provider_default' }
    | { kind: 'short' }
    | { kind: 'long' }
    | { kind: 'custom_seconds'; seconds: number };

export interface PromptCacheControlDto {
    state: string;
    settings: GenerationPresetDto['prompt_cache'];
    allowed_modes: string[];
    allowed_ttls: PromptCacheTtlDto[];
    supports_custom_ttl: boolean;
    custom_ttl_bounds: {
        minimum_seconds: number;
        maximum_seconds: number;
    } | null;
    ttl_field: string;
    context_reference_field: string;
    issues: ParameterIssueDto[];
}

export interface ProviderOverviewDto {
    settings: AppSettingsDto;
    templates: ProviderTemplateDto[];
    connections: ProviderConnectionDto[];
    legacy_profiles: ProviderProfileDto[];
}

export type CredentialTargetDto =
    | { kind: 'legacy_profile'; provider_profile_id: string }
    | { kind: 'connection'; connection_id: string }
    | {
          kind: 'discovery_session';
          session_id: string;
          expected_revision: number;
      };

export interface CredentialStatusDto {
    status: CredentialStatus;
}

export type ClipboardCleanupStatus = 'cleared' | 'already_replaced' | 'clear_failed';

export interface NativeCaptureStatusDto {
    clipboard_cleanup: ClipboardCleanupStatus;
}

export type RequestBodyShapeDto =
    | { kind: 'null' }
    | { kind: 'boolean' }
    | { kind: 'number' }
    | { kind: 'string' }
    | { kind: 'array'; items: RequestBodyShapeDto[]; truncated: boolean }
    | { kind: 'object'; fields: RequestBodyFieldDto[]; truncated: boolean }
    | { kind: 'redacted' }
    | { kind: 'truncated' };

export interface RequestBodyFieldDto {
    name: string;
    shape: RequestBodyShapeDto;
}

export interface RequestPreviewDto {
    method: string;
    origin: string;
    path: string;
    query_parameter_names: string[];
    header_names: string[];
    body: RequestBodyShapeDto | null;
    body_truncated: boolean;
}

export interface ModelSyncStartedDto {
    job_id: string;
}

export interface ModelSyncFailureDto {
    code: string;
    message_key: string;
    recoverable: boolean;
}

export interface ModelSyncSourceProvenanceDto {
    source: string;
    api_family: string;
    api_origin: string;
    endpoint_path: string;
    pages_fetched: number;
    response_bytes: number;
}

export interface ModelSyncDiffDto {
    connection_id: string;
    expected_connection: ProviderConnectionDto;
    expected_model_routes: ModelRouteDto[];
    observed_at: string;
    listed_routes: ModelRouteDto[];
    newly_seen_model_route_ids: string[];
    missing_model_route_ids: string[];
    initial_presets: GenerationPresetDto[];
    capability_observation_count: number;
    routes_requiring_preset_configuration: string[];
    provenance: ModelSyncSourceProvenanceDto;
}

export interface ModelSyncReviewDto {
    sha256: string;
    diff: ModelSyncDiffDto;
}

export interface ModelSyncJobDto {
    id: string;
    connection_id: string;
    state: string;
    revision: number;
    review: ModelSyncReviewDto | null;
    failure: ModelSyncFailureDto | null;
    created_at: string;
    updated_at: string;
}

export interface ModelSyncEventDto {
    version: number;
    job_id: string;
    sequence: number;
    job_revision: number;
    redaction_version: number;
    state: string;
    progress: {
        completed_steps: number;
        total_steps: number;
        message_key: string;
    };
    review_sha256: string | null;
    failure: ModelSyncFailureDto | null;
    emitted_at: string;
}

export interface ProviderDiscoveryConnectionOptionsInput {
    values: ProviderConfigEntryDto[];
    api_base_path: string | null;
    timeout_seconds: number;
    network_mode: ProviderNetworkModeInput;
    local_network_approval: ProviderLocalNetworkApprovalInput | null;
}

export interface ProviderDiscoveryConnectionOptionsDto {
    values: ProviderConfigEntryDto[];
    api_base_path: string | null;
    timeout_seconds: number;
    network_mode: string;
    local_network_approval: ProviderLocalNetworkApprovalInput | null;
}

export type BeginProviderDiscoverySourceInput =
    { kind: 'site' } | { kind: 'known_provider'; template_id: string };

export interface BeginProviderDiscoveryInput {
    connection_id: string;
    display_name: string;
    site_url: string;
    docs_url: string | null;
    credential_binding_requested: boolean;
    preferred_assistant: string | null;
    connection_options: ProviderDiscoveryConnectionOptionsInput;
    supplied_evidence_ids: string[];
    source: BeginProviderDiscoverySourceInput;
}

export interface BeginProviderDiscoveryCurlInput {
    connection_id: string;
    display_name: string;
    docs_url: string | null;
    credential_binding_requested: boolean;
    preferred_assistant: string | null;
    connection_options: ProviderDiscoveryConnectionOptionsInput;
    supplied_evidence_ids: string[];
}

export interface DiscoveryFailureDto {
    code: string;
    message_key: string;
    recoverable: boolean;
}

export interface DiscoveryReviewChangeDto {
    kind: string;
    target_kind: string;
    target_id: string;
    summary_key: string;
    evidence_ids: string[];
}

export interface DiscoveryReviewDto {
    sha256: string;
    graph_sha256: string;
    changes: DiscoveryReviewChangeDto[];
    unresolved_question_count: number;
    warning_count: number;
}

export type JsonValue =
    null | boolean | number | string | JsonValue[] | { [key: string]: JsonValue };

export interface ProviderDiscoveryApprovalProposalDto {
    id: string;
    grant: Record<string, JsonValue>;
    grant_sha256: string;
}

export interface ProviderDiscoveryReviewProposalDto {
    review: DiscoveryReviewDto;
    approval: ProviderDiscoveryApprovalProposalDto;
    commit_attempt_id: string;
    commit_plan_sha256: string;
    request_preview: RequestPreviewDto | null;
}

export type DiscoveryAssistantFailureKindInput =
    | 'transport'
    | 'timeout'
    | 'rate_limited'
    | 'invalid_structured_output'
    | 'draft_revision_required'
    | 'provider_rejected'
    | 'internal';

export type DiscoveryAssistantInterruptionOutcomeInput =
    'confirmed_no_external_effect' | 'external_outcome_unknown';

export type DiscoveryAssistantDraftFieldDto =
    | { kind: 'api_family' }
    | { kind: 'default_api_origin' }
    | { kind: 'auth' }
    | { kind: 'generate_endpoint' }
    | { kind: 'models_endpoint' }
    | { kind: 'response_decoder' }
    | { kind: 'streaming_decoder' }
    | { kind: 'parameter'; parameter_id: string };

export interface DiscoveryAssistantQuestionDto {
    id: string;
    field: DiscoveryAssistantDraftFieldDto | null;
    question: string;
    required_evidence: string;
}

export interface DiscoveryAssistantEvidenceMappingDto {
    field: DiscoveryAssistantDraftFieldDto;
    evidence_ids: string[];
    explanation: string;
}

export interface DiscoveryAssistantFieldConfidenceDto {
    field: DiscoveryAssistantDraftFieldDto;
    level: string;
    rationale: string;
}

export type DiscoveryAssistantConflictDispositionDto =
    | { status: 'unresolved' }
    | { status: 'resolved'; selected_evidence_id: string; rationale: string };

export interface DiscoveryAssistantEvidenceConflictDto {
    field: DiscoveryAssistantDraftFieldDto;
    evidence_ids: string[];
    disposition: DiscoveryAssistantConflictDispositionDto;
}

export interface DiscoveryAssistantManifestSourceDto {
    kind: string;
    url: string;
    content_sha256: string | null;
}

export interface DiscoveryAssistantEndpointDto {
    method: string;
    path: string;
}

export interface DiscoveryAssistantManifestDto {
    schema_version: number;
    api_family: string;
    sources: DiscoveryAssistantManifestSourceDto[];
    default_api_origin: string | null;
    auth: AuthBindingDto;
    models_endpoint: DiscoveryAssistantEndpointDto | null;
    generate_endpoint: DiscoveryAssistantEndpointDto;
    response_decoder: string;
    streaming_decoder: string | null;
    parameters: ParameterSpecDto[];
}

export interface DiscoveryAssistantManifestDraftDto {
    manifest: DiscoveryAssistantManifestDto;
    evidence_mappings: DiscoveryAssistantEvidenceMappingDto[];
    conflicts: DiscoveryAssistantEvidenceConflictDto[];
    unresolved_questions: DiscoveryAssistantQuestionDto[];
    confidence: DiscoveryAssistantFieldConfidenceDto[];
    summary: string;
}

export interface DiscoveryAssistantDraftReviewDto {
    draft: DiscoveryAssistantManifestDraftDto;
    unresolved_conflicts: DiscoveryAssistantDraftFieldDto[];
    required_checks: string[];
    persistence: string;
}

export type DiscoveryAssistantHostActionDto =
    | {
          kind: 'request_more_evidence';
          session_id: string;
          questions: DiscoveryAssistantQuestionDto[];
      }
    | { kind: 'review_draft'; review: DiscoveryAssistantDraftReviewDto };

export type DiscoveryAssistantResumeAction =
    | 'approve_consent'
    | 'run_assistant'
    | 'wait_for_assistant_outcome'
    | 'resume_core_host_action'
    | 'supply_more_evidence'
    | 'approve_retry'
    | 'review_draft'
    | 'restart_interrupted'
    | 'resolve_unknown_outcome';

export interface DiscoveryAssistantResumeBoundaryDto {
    checkpoint: string | null;
    action: DiscoveryAssistantResumeAction;
    questions: DiscoveryAssistantQuestionDto[];
    draft_review: DiscoveryAssistantDraftReviewDto | null;
}

export interface DiscoveryStepDto {
    id: string;
    title_key: string;
    state: string;
}

export interface ProviderDiscoverySessionDto {
    snapshot_schema_version: number;
    id: string;
    connection_id: string;
    display_name: string;
    site_url: string;
    docs_url: string | null;
    credential_binding_requested: boolean;
    preferred_assistant: string | null;
    connection_options: ProviderDiscoveryConnectionOptionsDto;
    supplied_evidence_ids: string[];
    state: string;
    revision: number;
    next_event_sequence: number;
    steps: DiscoveryStepDto[];
    action_required: { kind: string; operation: string | null } | null;
    active_operation_id: string | null;
    recovery_operation: string | null;
    unknown_operation: string | null;
    manifest_sha256: string | null;
    commit_plan_sha256: string | null;
    commit_attempt_id: string | null;
    committed_connection_id: string | null;
    cancellation_pending: boolean;
    active_effect_approval: {
        approval_id: string;
        grant_sha256: string;
    } | null;
    failure: DiscoveryFailureDto | null;
    has_private_draft: boolean;
    review: DiscoveryReviewDto | null;
    assistant_resume_boundary: DiscoveryAssistantResumeBoundaryDto | null;
    created_at: string;
    updated_at: string;
}

export interface CapturedProviderDiscoveryDto {
    session: ProviderDiscoverySessionDto;
    capture: NativeCaptureStatusDto;
}

export type DiscoveryCandidateSummaryDto =
    | { kind: 'provider_template'; template_id: string; template_version: number }
    | { kind: 'api_origin'; origin: string }
    | { kind: 'official_document'; url: string; content_sha256: string }
    | { kind: 'model_route'; model_id: string }
    | { kind: 'manifest_draft'; schema_version: number; manifest_sha256: string };

export interface DiscoveryCandidateDto {
    id: string;
    session_id: string;
    summary: DiscoveryCandidateSummaryDto;
    evidence_ids: string[];
    created_at: string;
    proposed_revision: number;
}

export interface DiscoveryEvidenceDto {
    id: string;
    session_id: string;
    kind: string;
    source_url: string;
    content_sha256: string;
    fetched_at: string;
}

export interface DiscoveryApprovalRecordDto {
    id: string;
    session_id: string;
    session_revision: number;
    decision: string;
    grant: Record<string, JsonValue>;
    created_at: string;
}

export type DiscoveryUnknownOutcomeResolutionInput =
    | { resolution: 'confirmed_no_effect' }
    | { resolution: 'confirmed_commit_completed'; connection_id: string }
    | { resolution: 'confirmed_compensated' }
    | { resolution: 'manually_reconciled_as_failed' };

export type ContinueProviderDiscoveryActionInput =
    | { kind: 'select_template'; candidate_id: string }
    | { kind: 'continue_without_template' }
    | { kind: 'supply_more_evidence'; evidence_ids: string[] }
    | { kind: 'request_assistant' }
    | {
          kind: 'approve_assistant';
          approval_id: string;
          approval_grant_sha256: string;
      }
    | { kind: 'decline_assistant' }
    | { kind: 'approve_credential_origin'; approval_id: string }
    | {
          kind: 'approve_probes';
          approval_id: string;
          approval_grant_sha256: string;
      }
    | { kind: 'skip_probes' }
    | {
          kind: 'approve_review';
          approval_id: string;
          commit_attempt_id: string;
          commit_plan_sha256: string;
          graph_sha256: string;
      }
    | { kind: 'resume_compensation' }
    | { kind: 'restart_interrupted' }
    | {
          kind: 'resolve_unknown_outcome';
          approval_id: string;
          resolution: DiscoveryUnknownOutcomeResolutionInput;
      };

export interface ContinueProviderDiscoveryInput {
    session_id: string;
    action_id: string;
    expected_revision: number;
    action: ContinueProviderDiscoveryActionInput;
}

export interface ProviderDiscoveryEventDto {
    version: number;
    id: string;
    session_id: string;
    sequence: number;
    session_revision: number;
    state: string;
    progress: { phase: string; completed: number; total: number | null } | null;
    action_required: { kind: string; operation: string | null } | null;
    warning: string | null;
    action_id: string;
    failure: DiscoveryFailureDto | null;
}

export interface DiscoveryOutboxEventDto {
    event: ProviderDiscoveryEventDto;
    delivery_attempts: number;
    available_at: string;
    created_at: string;
}

export interface DiscoveryRecoveryResultDto {
    operation_id: string;
    session_id: string;
    state: string;
    event: ProviderDiscoveryEventDto;
}

export interface DiscoveryCompensationRecordDto {
    id: string;
    commit_attempt_id: string;
    ordinal: number;
    action_id: string;
    kind: string;
    status: string;
    attempt_count: number;
    last_failure: DiscoveryFailureDto | null;
    created_at: string;
    updated_at: string;
    completed_at: string | null;
}

export type CatalogChangeKind = 'added' | 'updated' | 'removed';

export interface CatalogManifestDiffDto {
    provider_template_id: string;
    change: CatalogChangeKind;
    previous_manifest_version: number | null;
    next_manifest_version: number | null;
    previous_sha256: string | null;
    next_sha256: string | null;
    changed_sections: string[];
}

export interface CatalogModelMetadataDiffDto {
    model_entry_id: string;
    provider_template_id: string;
    change: CatalogChangeKind;
    previous_metadata_version: number | null;
    next_metadata_version: number | null;
    previous_sha256: string | null;
    next_sha256: string | null;
    changed_sections: string[];
}

export interface ProviderCatalogDiffDto {
    diff_schema_version: number;
    from_revision: number;
    to_revision: number;
    manifest_changes: CatalogManifestDiffDto[];
    model_changes: CatalogModelMetadataDiffDto[];
}

export interface ProviderCatalogStatusDto {
    status_schema_version: number;
    state_version: number;
    active_revision: number;
    active_snapshot_sha256: string;
    bundled_baseline_sha256: string;
    snapshot_count: number;
    signed_update_count: number;
    highest_accepted_revision: number;
    latest_issued_at: string | null;
    active_signed_revisions: number[];
}

export interface ProviderCatalogRevisionSummaryDto {
    revision: number;
    captured_at: string;
    snapshot_sha256: string;
    signed_revisions: number[];
    active: boolean;
}

export interface ProviderCatalogHistoryDto {
    history_schema_version: number;
    active_revision: number;
    revisions: ProviderCatalogRevisionSummaryDto[];
    activations: {
        action_id: string;
        state_version: number;
        kind: string;
        from_revision: number | null;
        to_revision: number;
        activated_at: string;
        diff: ProviderCatalogDiffDto;
    }[];
    next_before_revision: number | null;
    next_before_state_version: number | null;
}

export interface ProviderCatalogImportPlanDto {
    review: {
        plan_schema_version: number;
        action_id: string;
        expected_state_version: number;
        expected_active_revision: number;
        expected_active_snapshot_sha256: string;
        expected_highest_accepted_revision: number;
        envelope_byte_count: number;
        envelope_sha256: string;
        signing_key_id: string;
        payload_sha256: string;
        signed_catalog_revision: number;
        candidate_revision: number;
        candidate_snapshot_sha256: string;
        prepared_at: string;
        expires_at: string;
        diff: ProviderCatalogDiffDto;
    };
    plan_sha256: string;
}

export interface ProviderCatalogImportTicketDto {
    ticket_id: string;
    plan: ProviderCatalogImportPlanDto;
}

export interface ProviderCatalogImportResultDto {
    signed_catalog_revision: number;
    activated_revision: number;
    diff: ProviderCatalogDiffDto;
    status: ProviderCatalogStatusDto;
}

export interface ProviderCatalogRollbackPlanDto {
    plan_schema_version: number;
    action_id: string;
    expected_state_version: number;
    plan_sha256: string;
    catalog_plan: {
        rollback_plan_version: number;
        from_revision: number;
        to_revision: number;
        expected_active_sha256: string;
        target_sha256: string;
        created_at: string;
        expires_at: string;
        diff: ProviderCatalogDiffDto;
    };
}

export interface ProviderCatalogRollbackResultDto {
    from_revision: number;
    activated_revision: number;
    status: ProviderCatalogStatusDto;
}

export interface ProviderWorkspaceDto {
    templates: ProviderTemplateDto[];
    connections: ProviderConnectionDto[];
    legacy_profiles: ProviderProfileDto[];
    routes: ModelRouteDto[];
    presets: GenerationPresetDto[];
    settings: AppSettingsDto;
    credential_statuses: Record<string, CredentialStatus>;
    request_preview: RequestPreviewDto | null;
    selected_capability_model_route_id: string | null;
    capability_observations: CapabilityObservationDto[];
    capability_parameter_specs: ParameterSpecDto[];
    effective_capability: EffectiveCapabilityDto | null;
    model_sync_jobs: ModelSyncJobDto[];
    selected_model_sync_job_id: string | null;
    model_sync_event: ModelSyncEventDto | null;
    discoveries: ProviderDiscoverySessionDto[];
    selected_discovery_id: string | null;
    discovery_candidates: DiscoveryCandidateDto[];
    discovery_evidence: DiscoveryEvidenceDto[];
    discovery_approvals: DiscoveryApprovalRecordDto[];
    discovery_review: DiscoveryReviewDto | null;
    discovery_approval_proposal: ProviderDiscoveryApprovalProposalDto | null;
    discovery_review_proposal: ProviderDiscoveryReviewProposalDto | null;
    discovery_assistant_resume_boundary: DiscoveryAssistantResumeBoundaryDto | null;
    discovery_assistant_host_action: DiscoveryAssistantHostActionDto | null;
    discovery_event: ProviderDiscoveryEventDto | null;
    discovery_compensation_steps: DiscoveryCompensationRecordDto[];
    discovery_recovery_results: DiscoveryRecoveryResultDto[];
    catalog_status: ProviderCatalogStatusDto | null;
    catalog_history: ProviderCatalogHistoryDto | null;
    pending_catalog_import: ProviderCatalogImportTicketDto | null;
    pending_catalog_rollback: ProviderCatalogRollbackPlanDto | null;
    catalog_diff: ProviderCatalogDiffDto | null;
}

export interface LorepiaClient {
    bootstrapSnapshot(): Promise<BootstrapDto>;
    getMemorySupervisorStatus(): Promise<MemorySupervisorStatusDto>;
    subscribeMemorySupervisorStatus(
        onStatus: (status: MemorySupervisorStatusDto) => void,
    ): Promise<() => void>;

    listCharacters(): Promise<CharacterDto[]>;
    getCharacter(characterId: string): Promise<CharacterDto>;
    getCharacterGreetingCatalog(characterId: string): Promise<CharacterGreetingCatalogDto>;
    resolveAssetDelivery(input: ResolveAssetDeliveryInput): Promise<AssetDeliveryDto>;
    listInteractionEffects(): Promise<InteractionEffectEventDto[]>;
    acknowledgeInteractionEffect(deliveryId: string): Promise<void>;
    retryInteractionEffect(deliveryId: string): Promise<void>;
    decideInteractionProposal(
        input: DecideInteractionProposalInput,
    ): Promise<InteractionProposalDecisionReceiptDto>;
    expireGenerationAttemptProposals(
        input: ExpireGenerationAttemptProposalsInput,
    ): Promise<GenerationAttemptProposalExpiryReceiptDto>;
    listGenerationAttemptProposals(
        input: ListGenerationAttemptProposalsInput,
    ): Promise<GenerationAttemptProposalListItemDto[]>;
    listRetryableGenerationAttempts(
        input: ListRetryableGenerationAttemptsInput,
    ): Promise<RetryableGenerationAttemptDto[]>;
    decideGenerationAttemptProposal(
        input: DecideGenerationAttemptProposalInput,
    ): Promise<GenerationAttemptProposalDecisionReceiptDto>;
    subscribeInteractionEffects(
        onEffect: (effect: InteractionEffectEventDto) => void,
    ): Promise<() => void>;
    selectImportSource(): Promise<ImportTicketDto | null>;
    inspectImport(ticketId: string): Promise<ImportInspectionDto>;
    commitImport(inspectionId: string): Promise<CharacterDto>;
    discardImport(inspectionId: string): Promise<void>;

    listConversations(characterId: string | null): Promise<ConversationDto[]>;
    createConversation(
        characterId: string,
        title: string,
        mode: ConversationMode,
        greeting?: CharacterGreetingSelectionInput,
    ): Promise<ConversationDto>;
    openConversation(characterId: string): Promise<ConversationDto>;
    openExistingConversation(conversationId: string): Promise<ConversationDto>;
    getConversation(conversationId: string): Promise<ConversationDto>;
    getConversationState(conversationId: string): Promise<ConversationStateDto>;
    listBranches(conversationId: string): Promise<ConversationBranchDto[]>;
    createBranch(
        conversationId: string,
        fromMessageId: string | null,
        title: string | null,
    ): Promise<ConversationBranchDto>;
    selectBranch(conversationId: string, branchId: string): Promise<ConversationStateDto>;
    setConversationMode(
        conversationId: string,
        mode: ConversationMode,
    ): Promise<ConversationStateDto>;
    listBranchMessages(branchId: string): Promise<MessageDto[]>;
    listMessages(conversationId: string): Promise<MessageDto[]>;
    listRetryableMemoryQueryEmbeddings(
        input: ListRetryableMemoryQueryEmbeddingsInput,
    ): Promise<MemoryQueryEmbeddingRetryCandidateDto[]>;
    retryMemoryQueryEmbedding(
        input: RetryMemoryQueryEmbeddingInput,
    ): Promise<MemoryQueryEmbeddingRetryCandidateDto>;

    sendMessage(
        input: SendMessageInput,
        streamId: string,
        onItem: (item: ChatStreamItemDto) => void,
    ): Promise<GenerationStartedDto>;
    sendReviewedPrompt(
        input: ReviewedPromptSendInput,
        streamId: string,
        onItem: (item: ChatStreamItemDto) => void,
    ): Promise<GenerationStartedDto>;
    editUserMessage(
        input: EditUserMessageInput,
        streamId: string,
        onItem: (item: ChatStreamItemDto) => void,
    ): Promise<MessageActionGenerationDto>;
    regenerateAssistantMessage(
        input: RegenerateAssistantMessageInput,
        streamId: string,
        onItem: (item: ChatStreamItemDto) => void,
    ): Promise<MessageActionGenerationDto>;
    removeMessageFromBranch(input: RemoveMessageInput): Promise<ConversationBranchDto>;
    cancelGeneration(generationId: string): Promise<void>;
    subscribeGeneration(
        generationId: string,
        conversationId: string,
        branchId: string,
        sequenceBaseline: number,
        streamId: string,
        onItem: (item: ChatStreamItemDto) => void,
    ): Promise<void>;
    disposeChatStream(streamId: string): Promise<boolean>;

    getProviderOverview(): Promise<ProviderOverviewDto>;
    getSettings(): Promise<AppSettingsDto>;
    updateSettings(settings: AppSettingsDto): Promise<AppSettingsDto>;
    selectGenerationTarget(target: GenerationTargetDto | null): Promise<AppSettingsDto>;
    listProviderTemplates(): Promise<ProviderTemplateDto[]>;
    listProviderConnections(): Promise<ProviderConnectionDto[]>;
    createProviderConnection(input: CreateProviderConnectionInput): Promise<ProviderConnectionDto>;
    upsertProviderConnection(input: UpdateProviderConnectionInput): Promise<ProviderConnectionDto>;
    deleteProviderConnection(connectionId: string): Promise<void>;
    listProviderProfiles(): Promise<ProviderProfileDto[]>;
    listModelRoutes(connectionId: string): Promise<ModelRouteDto[]>;
    upsertModelRoute(input: UpsertModelRouteInput): Promise<ModelRouteDto>;
    deleteModelRoute(routeId: string): Promise<void>;
    listCapabilityObservations(modelRouteId: string): Promise<CapabilityObservationDto[]>;
    effectiveCapability(
        modelRouteId: string,
        key: CapabilityKeyInput,
    ): Promise<EffectiveCapabilityDto | null>;
    effectiveParameterSpecs(modelRouteId: string): Promise<ParameterSpecDto[]>;
    upsertUserCapabilityOverride(
        input: UpsertCapabilityOverrideInput,
    ): Promise<CapabilityObservationDto>;
    deleteUserCapabilityOverride(modelRouteId: string, observationId: string): Promise<void>;
    listGenerationPresets(routeId: string): Promise<GenerationPresetDto[]>;
    upsertGenerationPreset(input: GenerationPresetInput): Promise<GenerationPresetDto>;
    deleteGenerationPreset(presetId: string): Promise<void>;
    validateGenerationPresetCandidate(input: GenerationPresetInput): Promise<void>;
    renderReasoningControlForPreset(input: GenerationPresetInput): Promise<ReasoningControlDto>;
    renderPromptCacheControlForPreset(input: GenerationPresetInput): Promise<PromptCacheControlDto>;
    previewProviderRequestCandidate(input: GenerationPresetInput): Promise<RequestPreviewDto>;
    credentialStatus(target: CredentialTargetDto): Promise<CredentialStatusDto>;
    captureCredential(target: CredentialTargetDto): Promise<NativeCaptureStatusDto>;
    deleteCredential(target: CredentialTargetDto): Promise<void>;
    previewProviderRequest(target: GenerationTargetDto): Promise<RequestPreviewDto>;

    startProviderModelSync(connectionId: string): Promise<ModelSyncStartedDto>;
    getProviderModelSync(jobId: string): Promise<ModelSyncJobDto>;
    listProviderModelSyncs(connectionId: string, limit: number): Promise<ModelSyncJobDto[]>;
    approveProviderModelSync(jobId: string, reviewSha256: string): Promise<ModelSyncJobDto>;
    cancelProviderModelSync(jobId: string): Promise<ModelSyncJobDto>;
    pollProviderModelSyncEvents(jobId: string, limit: number): Promise<ModelSyncEventDto[]>;
    ackProviderModelSyncEvent(jobId: string, sequence: number): Promise<boolean>;

    beginProviderDiscovery(
        input: BeginProviderDiscoveryInput,
    ): Promise<ProviderDiscoverySessionDto>;
    beginProviderDiscoveryCurl(
        input: BeginProviderDiscoveryCurlInput,
    ): Promise<CapturedProviderDiscoveryDto>;
    listProviderDiscoveries(limit: number): Promise<ProviderDiscoverySessionDto[]>;
    getProviderDiscovery(sessionId: string): Promise<ProviderDiscoverySessionDto>;
    listProviderDiscoveryCandidates(sessionId: string): Promise<DiscoveryCandidateDto[]>;
    listProviderDiscoveryEvidence(sessionId: string): Promise<DiscoveryEvidenceDto[]>;
    listProviderDiscoveryApprovals(sessionId: string): Promise<DiscoveryApprovalRecordDto[]>;
    getProviderDiscoveryReview(sessionId: string): Promise<DiscoveryReviewDto | null>;
    getProviderDiscoveryApprovalProposal(
        sessionId: string,
    ): Promise<ProviderDiscoveryApprovalProposalDto | null>;
    getProviderDiscoveryReviewProposal(
        sessionId: string,
    ): Promise<ProviderDiscoveryReviewProposalDto | null>;
    getProviderDiscoveryAssistantResumeBoundary(
        sessionId: string,
    ): Promise<DiscoveryAssistantResumeBoundaryDto | null>;
    runProviderDiscoveryAssistantTurn(sessionId: string): Promise<DiscoveryAssistantHostActionDto>;
    resumeProviderDiscoveryAssistantCoreHostAction(
        sessionId: string,
    ): Promise<ProviderDiscoverySessionDto>;
    approveProviderDiscoveryAssistantRetry(sessionId: string): Promise<ProviderDiscoverySessionDto>;
    requestProviderDiscoveryAssistantRevision(
        sessionId: string,
    ): Promise<ProviderDiscoverySessionDto>;
    acceptProviderDiscoveryAssistantDraft(sessionId: string): Promise<ProviderDiscoverySessionDto>;
    recordProviderDiscoveryAssistantFailure(
        sessionId: string,
        kind: DiscoveryAssistantFailureKindInput,
        retryable: boolean,
    ): Promise<ProviderDiscoverySessionDto>;
    interruptProviderDiscoveryAssistant(
        sessionId: string,
        outcome: DiscoveryAssistantInterruptionOutcomeInput,
    ): Promise<ProviderDiscoverySessionDto>;
    restartProviderDiscoveryAssistantAfterInterruption(
        sessionId: string,
    ): Promise<ProviderDiscoverySessionDto>;
    continueProviderDiscovery(
        input: ContinueProviderDiscoveryInput,
    ): Promise<ProviderDiscoverySessionDto>;
    supplyProviderDiscoveryDocumentEvidence(
        sessionId: string,
        expectedRevision: number,
        documentUrl: string,
    ): Promise<ProviderDiscoverySessionDto>;
    supplyProviderDiscoveryCurlEvidence(
        sessionId: string,
        expectedRevision: number,
    ): Promise<CapturedProviderDiscoveryDto>;
    cancelProviderDiscovery(
        sessionId: string,
        expectedRevision: number,
    ): Promise<ProviderDiscoverySessionDto>;
    commitProviderDiscovery(sessionId: string): Promise<ProviderConnectionDto>;
    pollProviderDiscoveryEvents(limit: number): Promise<DiscoveryOutboxEventDto[]>;
    pollProviderDiscoveryEventsForSession(
        sessionId: string,
        limit: number,
    ): Promise<DiscoveryOutboxEventDto[]>;
    ackProviderDiscoveryEvent(eventId: string): Promise<boolean>;
    recoverProviderDiscovery(): Promise<DiscoveryRecoveryResultDto[]>;
    listProviderDiscoveryCompensationSteps(
        commitAttemptId: string,
    ): Promise<DiscoveryCompensationRecordDto[]>;
    continueProviderDiscoveryCompensation(sessionId: string): Promise<ProviderDiscoverySessionDto>;
    resumeProviderDiscoveryCompensation(sessionId: string): Promise<ProviderDiscoverySessionDto>;

    pickProviderCatalogImport(): Promise<ProviderCatalogImportTicketDto | null>;
    activateProviderCatalogImport(ticketId: string): Promise<ProviderCatalogImportResultDto>;
    discardProviderCatalogImport(ticketId: string): Promise<void>;
    providerCatalogStatus(): Promise<ProviderCatalogStatusDto>;
    providerCatalogHistory(
        limit: number,
        beforeRevision: number | null,
        beforeStateVersion: number | null,
    ): Promise<ProviderCatalogHistoryDto>;
    diffProviderCatalogRevisions(
        fromRevision: number,
        toRevision: number,
    ): Promise<ProviderCatalogDiffDto>;
    prepareProviderCatalogRollback(targetRevision: number): Promise<ProviderCatalogRollbackPlanDto>;
    activateProviderCatalogRollback(
        plan: ProviderCatalogRollbackPlanDto,
    ): Promise<ProviderCatalogRollbackResultDto>;
}
