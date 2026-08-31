import { t } from '../../../lib/i18n';
import { normalizeClientError } from '../../../lib/ipc/errors';
import type {
    CreatorContentModuleDocumentDto,
    CreatorInteractionRuleSetDocumentDto,
    CreatorKnowledgeBookDocumentDto,
    CreatorMemoryProfileDocumentDto,
    CreatorPromptBlockDocumentDto,
    CreatorPromptPresetDocumentDto,
    CreatorTransformSetDocumentDto,
    InteractionProposalListItemDto,
    KnowledgeSimulationDto,
    LorepiaClient,
    OrchestrationClientApi,
    OrchestrationDocumentClientApi,
    OrchestrationWorkspaceDto,
    PromptBlockDto,
    PromptPlanRequestInput,
    RoomOrchestrationConfigDto,
    RevisionedDto,
    RoomInteractionClientApi,
    TaskProfileDocumentDto,
    TransformPreviewDto,
} from '../../../lib/ipc/contracts';

export const MAX_VISIBLE_PROMPT_BLOCKS = 200;
export const MAX_VISIBLE_MEMORY_RECORDS = 250;
export const MAX_VISIBLE_SELECTION_EVIDENCE = 300;
export const MAX_VISIBLE_CONTENT_MODULES = 100;
export const MAX_VISIBLE_PLAN_OPERATION_NONCE_CHARS = 64;
export const MAX_ROOM_PROMPT_NAME_CHARS = 128;
export const MAX_ROOM_PROMPT_TEXT_CHARS = 32_768;
export const MAX_ROOM_PROMPT_TEMPLATE_SLOTS = 128;

export const MEMORY_RECORD_RESPONSE_AUTHORITY_ERROR = t('orchestration.error.memory_scope');

export type OrchestrationPhase = 'idle' | 'loading' | 'ready' | 'error' | 'unavailable';

export interface EditableTaskProfileState {
    value: TaskProfileDocumentDto;
    expected_revision: number | null;
    dirty: boolean;
}

export interface EditableCreatorDocumentState<Value> {
    value: Value;
    expected_revision: number | null;
    dirty: boolean;
}

export interface OrchestrationState {
    phase: OrchestrationPhase;
    saving: boolean;
    busy_interaction_proposal_id: string | null;
    error: string | null;
    announcement: string;
    context_key: string;
    dirty_room_config: boolean;
    workspace: OrchestrationWorkspaceDto;
    plan_operation_nonce: string | null;
    plan_generation_attempt_id: string | null;
    plan_preview_request: PromptPlanRequestInput | null;
    knowledge_simulation: KnowledgeSimulationDto | null;
    transform_preview: TransformPreviewDto | null;
    editable_prompt_preset: RevisionedDto<CreatorPromptPresetDocumentDto> | null;
    editable_prompt_preset_dirty: boolean;
    editable_prompt_preset_loading: boolean;
    editable_prompt_preset_error: string | null;
    editable_task_profiles: EditableTaskProfileState[];
    editable_task_profiles_loading: boolean;
    editable_task_profiles_error: string | null;
    editable_memory_profiles: EditableCreatorDocumentState<CreatorMemoryProfileDocumentDto>[];
    editable_knowledge_books: EditableCreatorDocumentState<CreatorKnowledgeBookDocumentDto>[];
    editable_transform_sets: EditableCreatorDocumentState<CreatorTransformSetDocumentDto>[];
    editable_interaction_rule_sets: EditableCreatorDocumentState<CreatorInteractionRuleSetDocumentDto>[];
    editable_content_modules: EditableCreatorDocumentState<CreatorContentModuleDocumentDto>[];
    editable_creator_documents_loading: boolean;
    editable_creator_documents_error: string | null;
    list_truncation: {
        prompt_blocks: boolean;
        memory_records: boolean;
        selection_evidence: boolean;
        content_modules: boolean;
    };
}

type EditableDocumentClientApi = Pick<
    OrchestrationDocumentClientApi,
    | 'getEditablePromptPreset'
    | 'upsertPromptPreset'
    | 'listTaskProfiles'
    | 'upsertTaskProfile'
    | 'deleteTaskProfile'
    | 'listMemoryProfiles'
    | 'upsertMemoryProfile'
    | 'deleteMemoryProfile'
    | 'listKnowledgeBooks'
    | 'upsertKnowledgeBook'
    | 'deleteKnowledgeBook'
    | 'listTransformSets'
    | 'upsertTransformSet'
    | 'deleteTransformSet'
    | 'listInteractionRuleSets'
    | 'upsertInteractionRuleSet'
    | 'deleteInteractionRuleSet'
    | 'listContentModules'
    | 'upsertContentModule'
    | 'deleteContentModule'
>;

export type OrchestrationCapableClient = LorepiaClient &
    Partial<OrchestrationClientApi & EditableDocumentClientApi & RoomInteractionClientApi>;

export type EditablePromptBlockPatch = Partial<
    Pick<
        CreatorPromptBlockDocumentDto,
        | 'name'
        | 'enabled'
        | 'role_hint'
        | 'template'
        | 'condition'
        | 'placement_zone'
        | 'history_selector'
        | 'token_policy'
        | 'overflow_policy'
        | 'merge_policy'
    >
>;

export type RoomOrchestrationConfigPatch = Partial<
    Pick<
        RoomOrchestrationConfigDto,
        | 'prompt_preset_id'
        | 'generation_preset_id'
        | 'creator_values'
        | 'variable_overrides'
        | 'response_length'
        | 'creativity'
        | 'reasoning_effort'
        | 'memory_enabled'
        | 'knowledge_enabled'
        | 'user_name_override'
        | 'author_note'
        | 'group_context'
        | 'template_slots'
    >
>;

type RoomPromptSourceConfig = Pick<
    RoomOrchestrationConfigDto,
    'user_name_override' | 'author_note' | 'group_context' | 'template_slots'
>;

function characterCount(value: string): number {
    return Array.from(value).length;
}

function invalidOptionalRoomText(
    value: string | null,
    maximumChars: number,
    requireTrimmed: boolean,
): boolean {
    return (
        value !== null &&
        (value.trim() === '' ||
            characterCount(value) > maximumChars ||
            value.includes('\0') ||
            (requireTrimmed && value.trim() !== value))
    );
}

export function roomPromptSourceValidationError(config: RoomPromptSourceConfig): string | null {
    if (invalidOptionalRoomText(config.user_name_override, MAX_ROOM_PROMPT_NAME_CHARS, true)) {
        return t('orchestration.error.name_length', { max: MAX_ROOM_PROMPT_NAME_CHARS });
    }
    if (invalidOptionalRoomText(config.author_note, MAX_ROOM_PROMPT_TEXT_CHARS, false)) {
        return t('orchestration.error.author_note_length', {
            max: MAX_ROOM_PROMPT_TEXT_CHARS.toLocaleString(),
        });
    }
    if (invalidOptionalRoomText(config.group_context, MAX_ROOM_PROMPT_TEXT_CHARS, false)) {
        return t('orchestration.error.group_context_length', {
            max: MAX_ROOM_PROMPT_TEXT_CHARS.toLocaleString(),
        });
    }
    if (config.template_slots.length > MAX_ROOM_PROMPT_TEMPLATE_SLOTS) {
        return t('orchestration.error.slot_count', { max: MAX_ROOM_PROMPT_TEMPLATE_SLOTS });
    }

    const names = new Set<string>();
    for (const [index, slot] of config.template_slots.entries()) {
        const displayIndex = index + 1;
        if (
            slot.name.trim() === '' ||
            slot.name.trim() !== slot.name ||
            characterCount(slot.name) > MAX_ROOM_PROMPT_NAME_CHARS ||
            Array.from(slot.name).some((character) => /\p{Cc}/u.test(character))
        ) {
            return t('orchestration.error.slot_name', {
                index: displayIndex,
                max: MAX_ROOM_PROMPT_NAME_CHARS,
            });
        }
        if (slot.name === 'block_content') {
            return t('orchestration.error.reserved_slot');
        }
        if (names.has(slot.name)) {
            return t('orchestration.error.slot_duplicate', { name: slot.name });
        }
        if (characterCount(slot.value) > MAX_ROOM_PROMPT_TEXT_CHARS || slot.value.includes('\0')) {
            return t('orchestration.error.slot_value_length', {
                index: displayIndex,
                max: MAX_ROOM_PROMPT_TEXT_CHARS.toLocaleString(),
            });
        }
        names.add(slot.name);
    }
    return null;
}

function emptyRoomConfig(conversationId = '', branchId = ''): RoomOrchestrationConfigDto {
    return {
        conversation_id: conversationId,
        branch_id: branchId,
        prompt_preset_id: null,
        generation_preset_id: null,
        response_length: 'balanced',
        creativity: 50,
        reasoning_effort: 'provider_default',
        memory_enabled: true,
        knowledge_enabled: true,
        creator_values: {},
        variable_overrides: { values: [] },
        user_name_override: null,
        author_note: null,
        group_context: null,
        template_slots: [],
        supported_fields: {
            prompt_preset_id: true,
            generation_preset_id: true,
            creator_values: true,
            variable_overrides: false,
            response_length: true,
            creativity: true,
            reasoning_effort: true,
            memory_enabled: true,
            knowledge_enabled: true,
            user_name_override: true,
            author_note: true,
            group_context: true,
            template_slots: true,
        },
    };
}

export function emptyOrchestrationWorkspace(
    conversationId = '',
    branchId = '',
): OrchestrationWorkspaceDto {
    return {
        expected_head: null,
        room_config_revision: null,
        prompt_preset_revision: null,
        interaction_state_revision: null,
        generation_target: null,
        prompt_presets: [],
        room_config: emptyRoomConfig(conversationId, branchId),
        prompt_blocks: [],
        creator_controls: [],
        knowledge_book_ids: [],
        task_profiles: [],
        memory_records: [],
        selection_evidence: [],
        interaction_state: [],
        interaction_proposals: [],
        content_modules: [],
        module_diff: null,
        plan_preview: null,
    };
}

export const INITIAL_ORCHESTRATION_STATE: OrchestrationState = {
    phase: 'idle',
    saving: false,
    busy_interaction_proposal_id: null,
    error: null,
    announcement: '',
    context_key: '',
    dirty_room_config: false,
    workspace: emptyOrchestrationWorkspace(),
    plan_operation_nonce: null,
    plan_generation_attempt_id: null,
    plan_preview_request: null,
    knowledge_simulation: null,
    transform_preview: null,
    editable_prompt_preset: null,
    editable_prompt_preset_dirty: false,
    editable_prompt_preset_loading: false,
    editable_prompt_preset_error: null,
    editable_task_profiles: [],
    editable_task_profiles_loading: false,
    editable_task_profiles_error: null,
    editable_memory_profiles: [],
    editable_knowledge_books: [],
    editable_transform_sets: [],
    editable_interaction_rule_sets: [],
    editable_content_modules: [],
    editable_creator_documents_loading: false,
    editable_creator_documents_error: null,
    list_truncation: {
        prompt_blocks: false,
        memory_records: false,
        selection_evidence: false,
        content_modules: false,
    },
};

export function errorLabel(error: unknown): string {
    const normalized = normalizeClientError(error);
    return normalized.messageKey === 'error.unexpected'
        ? t('orchestration.error.generic')
        : normalized.messageKey;
}

export function isValidGenerationAttemptId(value: unknown): value is string {
    return (
        typeof value === 'string' &&
        value.length > 0 &&
        value.length <= 512 &&
        value.trim() === value &&
        !/\p{Cc}/u.test(value)
    );
}

export function taskProfileValidationError(profile: TaskProfileDocumentDto): string | null {
    if (profile.kind === 'memory_embedding') {
        if (
            !Number.isSafeInteger(profile.embedding_dimensions) ||
            profile.embedding_dimensions === null ||
            profile.embedding_dimensions < 1 ||
            profile.embedding_dimensions > 32_768
        ) {
            return t('orchestration.error.embedding_dimensions');
        }
        if (profile.fallback_route_ids.length > 0) {
            return t('orchestration.error.embedding_fallback');
        }
    } else if (profile.embedding_dimensions !== null) {
        return t('orchestration.error.embedding_not_applicable');
    }
    return null;
}

export type CreatorDocumentKind =
    | 'memory_profile'
    | 'knowledge_book'
    | 'transform_set'
    | 'interaction_rule_set'
    | 'content_module';

export type CreatorDocumentValue =
    | CreatorMemoryProfileDocumentDto
    | CreatorKnowledgeBookDocumentDto
    | CreatorTransformSetDocumentDto
    | CreatorInteractionRuleSetDocumentDto
    | CreatorContentModuleDocumentDto;

export function editableCreatorDocuments<Value>(
    documents: RevisionedDto<Value>[],
): EditableCreatorDocumentState<Value>[] {
    return documents.map(({ value, revision }) => ({
        value,
        expected_revision: revision,
        dirty: false,
    }));
}

export function stageEditableCreatorDocument<Value extends { id: string }>(
    documents: EditableCreatorDocumentState<Value>[],
    documentId: string,
    patch: Partial<Value>,
): EditableCreatorDocumentState<Value>[] {
    return documents.map((document) =>
        document.value.id === documentId
            ? {
                  ...document,
                  value: { ...document.value, ...patch, id: documentId },
                  dirty: true,
              }
            : document,
    );
}

export function replaceSavedCreatorDocument<Value extends { id: string }>(
    documents: EditableCreatorDocumentState<Value>[],
    saved: RevisionedDto<Value>,
    submitted: EditableCreatorDocumentState<Value>,
): EditableCreatorDocumentState<Value>[] {
    return documents.map((document) =>
        document.value.id === saved.value.id
            ? document !== submitted && document.dirty
                ? {
                      value: document.value,
                      expected_revision: saved.revision,
                      dirty: true,
                  }
                : {
                      value: saved.value,
                      expected_revision: saved.revision,
                      dirty: false,
                  }
            : document,
    );
}

export function replaceEditableCreatorDocument<Value extends { id: string }>(
    documents: EditableCreatorDocumentState<Value>[],
    documentId: string,
    value: Value,
): EditableCreatorDocumentState<Value>[] {
    return documents.map((document) =>
        document.value.id === documentId
            ? {
                  ...document,
                  value: { ...value, id: documentId },
                  dirty: true,
              }
            : document,
    );
}

export function validNewCreatorDocumentId(value: string): boolean {
    return value !== '' && value.length <= 256 && !value.includes('\0');
}

export function memoryProfileDraft(id: string): CreatorMemoryProfileDocumentDto {
    return {
        id,
        name: id,
        summary_task: '',
        embedding_task: null,
        turns_per_summary: 8,
        recent_raw_budget: { max_tokens: 2_048 },
        episodic_budget: { max_tokens: 1_024 },
        semantic_budget: { max_tokens: 1_024 },
        retrieval_count: 8,
        recency_weight: 1,
        similarity_weight: 1,
        importance_weight: 1,
        preserve_invalidated_records: false,
        summary_schema: '',
    };
}

export function knowledgeBookDraft(id: string): CreatorKnowledgeBookDocumentDto {
    return {
        id,
        name: id,
        entries: [],
        scan_depth: 8,
        token_budget: { max_tokens: 4_096 },
        recursive: false,
        max_recursion_depth: 0,
    };
}

export function transformSetDraft(id: string): CreatorTransformSetDocumentDto {
    return {
        id,
        name: id,
        enabled: false,
        rules: [],
        max_rules_per_phase: 64,
        max_output_chars: 65_536,
    };
}

export function interactionRuleSetDraft(id: string): CreatorInteractionRuleSetDocumentDto {
    return {
        id,
        name: id,
        rules: [],
        max_actions_per_event: 128,
    };
}

export function contentModuleDraft(id: string): CreatorContentModuleDocumentDto {
    return {
        id,
        name: id,
        version: '0.1.0',
        prompt_fragments: [],
        knowledge_book_ids: [],
        control_specs: [],
        transform_set_ids: [],
        interaction_rule_set_ids: [],
        asset_ids: [],
        required_capabilities: [],
        metadata: {
            author: null,
            license: '',
            redistribution_allowed: false,
            homepage: null,
            description: '',
            tags: [],
        },
    };
}

export function boundedWorkspace(workspace: OrchestrationWorkspaceDto): {
    workspace: OrchestrationWorkspaceDto;
    truncation: OrchestrationState['list_truncation'];
} {
    const truncation = {
        prompt_blocks: workspace.prompt_blocks.length > MAX_VISIBLE_PROMPT_BLOCKS,
        memory_records: workspace.memory_records.length > MAX_VISIBLE_MEMORY_RECORDS,
        selection_evidence: workspace.selection_evidence.length > MAX_VISIBLE_SELECTION_EVIDENCE,
        content_modules: workspace.content_modules.length > MAX_VISIBLE_CONTENT_MODULES,
    };
    return {
        workspace: {
            ...workspace,
            prompt_blocks: workspace.prompt_blocks.slice(0, MAX_VISIBLE_PROMPT_BLOCKS),
            memory_records: workspace.memory_records.slice(0, MAX_VISIBLE_MEMORY_RECORDS),
            selection_evidence: workspace.selection_evidence.slice(
                0,
                MAX_VISIBLE_SELECTION_EVIDENCE,
            ),
            content_modules: workspace.content_modules.slice(0, MAX_VISIBLE_CONTENT_MODULES),
        },
        truncation,
    };
}

export function validateInteractionProposalPage(
    items: InteractionProposalListItemDto[],
    conversationId: string,
    branchId: string,
    status: InteractionProposalListItemDto['proposal']['status'],
): void {
    if (items.length > 100) throw new Error('interaction proposal page exceeded its bound');
    const proposalIds = new Set<string>();
    for (const item of items) {
        const proposalId = item.proposal.id;
        const requestedAt = item.proposal.requested_at_epoch_seconds;
        const expiresAt = item.proposal.expires_at_epoch_seconds;
        const decidedAt = item.proposal.decided_at_epoch_seconds;
        if (
            item.conversation_id !== conversationId ||
            item.branch_id !== branchId ||
            item.proposal.status !== status ||
            proposalId.trim() === '' ||
            proposalId.length > 256 ||
            proposalIds.has(proposalId) ||
            !Number.isSafeInteger(item.state_revision) ||
            item.state_revision < 0 ||
            !Number.isSafeInteger(item.proposal_revision) ||
            item.proposal_revision < 1 ||
            !Number.isSafeInteger(item.proposal.source_interaction_state_revision) ||
            item.proposal.source_interaction_state_revision < 0 ||
            item.proposal.source_interaction_state_revision > item.state_revision ||
            !Number.isSafeInteger(requestedAt) ||
            requestedAt < 0 ||
            (expiresAt !== null && (!Number.isSafeInteger(expiresAt) || expiresAt < requestedAt)) ||
            (decidedAt !== null && (!Number.isSafeInteger(decidedAt) || decidedAt < requestedAt))
        ) {
            throw new Error('interaction proposal page authority validation failed');
        }
        proposalIds.add(proposalId);
    }
}
export function moveBlockByDrop(
    blocks: PromptBlockDto[],
    draggedId: string,
    targetId: string,
): PromptBlockDto[] {
    const from = blocks.findIndex((block) => block.id === draggedId);
    const to = blocks.findIndex((block) => block.id === targetId);
    if (from < 0 || to < 0 || from === to) return blocks;
    if (blocks[from]?.placement_zone !== blocks[to]?.placement_zone) return blocks;
    if (!blocks[from]?.order_editable || !blocks[to]?.order_editable) return blocks;
    const first = Math.min(from, to);
    const last = Math.max(from, to);
    if (blocks.slice(first, last + 1).some((block) => !block.order_editable)) return blocks;
    const reordered = [...blocks];
    const [moved] = reordered.splice(from, 1);
    if (moved === undefined) return blocks;
    reordered.splice(to, 0, moved);
    return reordered;
}
