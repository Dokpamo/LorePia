import type { AssetDeliveryDto } from './character';

import type {
    CreatorControlValue,
    OrchestrationConditionExprDto,
    OrchestrationVariableMapDto,
    OrchestrationVariableRefDto,
    OrchestrationVariableValueDto,
    PromptBlockKind,
    PromptBlockOverflowPolicy,
    PromptBlockRoleHint,
    SafePromptTemplateDto,
} from './common';

import type { ConversationBranchDto, ConversationMode, MessageStatus } from './conversation';

import type { MemorySelectionLane, MemorySelectionReasonDto } from './memory';

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

export type RuntimePromptRoleInput = 'system' | 'user' | 'assistant';

export interface RuntimePromptMessageInput {
    role: RuntimePromptRoleInput;
    content: string;
}

export interface GenerateRuntimeTextInput {
    request_id: string;
    audit: {
        character_id: string;
        character_content_revision_id: string | null;
        capability: 'model:primary' | 'model:auxiliary';
        grant_sha256: string;
    };
    selection: GenerationSelectionInput;
    messages: RuntimePromptMessageInput[];
}

export interface RuntimeTextGenerationDto {
    request_id: string;
    result: string;
    usage: GenerationUsageDto;
}

export interface SendMessageInput {
    conversation_id: string;
    branch_id: string;
    expected_head: string | null;
    mode: ConversationMode;
    text: string;
    selection: GenerationSelectionInput;
    /** Per-generation character/runtime values merged after stored prompt state. */
    variable_overrides?: OrchestrationVariableMapDto;
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

export interface InteractionStateEntryDto {
    id: string;
    label: string;
    value: CreatorControlValue;
    scope: string;
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
