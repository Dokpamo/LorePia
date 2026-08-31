import type {
    OrchestrationConditionExprDto,
    OrchestrationVariableMapDto,
    PromptTokenPolicyDto,
    SafeRegexDto,
} from './common';

import type { CapabilityKeyInput } from './provider';

export interface MemorySupervisorStatusDto {
    sequence: number;
    phase: 'not_started' | 'recovered' | 'running' | 'failed';
    recovered_interrupted_jobs: number;
    completed_jobs: number;
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

export interface PatchMemoryRecordRequest {
    conversation_id: string;
    branch_id: string;
    memory_record_id: string;
    patch: MemoryRecordPatchInput;
    expected_revision: number;
}

export interface DeleteMemoryRecordRequest {
    conversation_id: string;
    branch_id: string;
    memory_record_id: string;
    expected_revision: number;
}

export type MemoryRecordExclusionScope = 'conversation' | 'character';

export interface SetMemoryRecordExclusionRequest {
    conversation_id: string;
    branch_id: string;
    memory_record_id: string;
    scope: MemoryRecordExclusionScope;
    excluded: boolean;
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
    conversation_id: string;
    branch_id: string;
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
    conversation_id: string;
    branch_id: string;
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

export interface ListInterruptedMemoryJobsInput {
    conversation_id: string;
    branch_id: string;
    limit: number;
}

export type MemoryJobRetryKind = 'summary' | 'embedding';

export interface InterruptedMemoryJobDto {
    memory_job_id: string;
    kind: MemoryJobRetryKind;
    revision: number;
    conversation_id: string;
    branch_id: string;
    source_start_message_id: string;
    source_end_message_id: string;
    attempt: number;
    interruption_count: number;
    last_interrupted_at: string | null;
    last_error_code: string | null;
}

export interface RetryInterruptedMemoryJobInput {
    conversation_id: string;
    branch_id: string;
    memory_job_id: string;
    expected_revision: number;
    acknowledge_unknown_outcome: boolean;
}

export type MemoryJobRetryStatus = 'queued';

export interface MemoryJobRetryReceiptDto {
    memory_job_id: string;
    kind: MemoryJobRetryKind;
    status: MemoryJobRetryStatus;
    revision: number;
    conversation_id: string;
    branch_id: string;
    source_start_message_id: string;
    source_end_message_id: string;
    attempt: number;
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
