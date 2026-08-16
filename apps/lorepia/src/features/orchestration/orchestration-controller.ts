import { get, writable, type Readable } from 'svelte/store';

import { t } from '../../lib/i18n';
import { normalizeClientError } from '../../lib/ipc/errors';
import type {
    CreatorContentModuleDocumentDto,
    CreatorInteractionRuleSetDocumentDto,
    CreatorKnowledgeBookDocumentDto,
    CreatorMemoryProfileDocumentDto,
    CreatorPromptBlockDocumentDto,
    CreatorPromptPresetDocumentDto,
    CreatorTransformSetDocumentDto,
    CreatorControlValue,
    InteractionProposalListItemDto,
    KnowledgeSimulationDto,
    LorepiaClient,
    MemoryRecordExclusionScope,
    MemoryRecordPatchInput,
    OrchestrationClientApi,
    OrchestrationDocumentClientApi,
    OrchestrationWorkspaceDto,
    PromptBlockDto,
    PromptPlanPreviewDto,
    PromptPlanRequestInput,
    ReviewedPromptSendInput,
    RoomOrchestrationConfigDto,
    RevisionedDto,
    RoomInteractionClientApi,
    TaskProfileDocumentDto,
    TransformPreviewDto,
} from '../../lib/ipc/contracts';

export const MAX_VISIBLE_PROMPT_BLOCKS = 200;
export const MAX_VISIBLE_MEMORY_RECORDS = 250;
export const MAX_VISIBLE_SELECTION_EVIDENCE = 300;
export const MAX_VISIBLE_CONTENT_MODULES = 100;
export const MAX_VISIBLE_PLAN_OPERATION_NONCE_CHARS = 64;
export const MAX_ROOM_PROMPT_NAME_CHARS = 128;
export const MAX_ROOM_PROMPT_TEXT_CHARS = 32_768;
export const MAX_ROOM_PROMPT_TEMPLATE_SLOTS = 128;

const MEMORY_RECORD_RESPONSE_AUTHORITY_ERROR = t('orchestration.error.memory_scope');

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

function errorLabel(error: unknown): string {
    const normalized = normalizeClientError(error);
    return normalized.messageKey === 'error.unexpected'
        ? t('orchestration.error.generic')
        : normalized.messageKey;
}

function isValidGenerationAttemptId(value: unknown): value is string {
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

function editableCreatorDocuments<Value>(
    documents: RevisionedDto<Value>[],
): EditableCreatorDocumentState<Value>[] {
    return documents.map(({ value, revision }) => ({
        value,
        expected_revision: revision,
        dirty: false,
    }));
}

function stageEditableCreatorDocument<Value extends { id: string }>(
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

function replaceSavedCreatorDocument<Value extends { id: string }>(
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

function replaceEditableCreatorDocument<Value extends { id: string }>(
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

function validNewCreatorDocumentId(value: string): boolean {
    return value !== '' && value.length <= 256 && !value.includes('\0');
}

function memoryProfileDraft(id: string): CreatorMemoryProfileDocumentDto {
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

function knowledgeBookDraft(id: string): CreatorKnowledgeBookDocumentDto {
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

function transformSetDraft(id: string): CreatorTransformSetDocumentDto {
    return {
        id,
        name: id,
        enabled: false,
        rules: [],
        max_rules_per_phase: 64,
        max_output_chars: 65_536,
    };
}

function interactionRuleSetDraft(id: string): CreatorInteractionRuleSetDocumentDto {
    return {
        id,
        name: id,
        rules: [],
        max_actions_per_event: 128,
    };
}

function contentModuleDraft(id: string): CreatorContentModuleDocumentDto {
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

function boundedWorkspace(workspace: OrchestrationWorkspaceDto): {
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

function validateInteractionProposalPage(
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

export class OrchestrationController {
    private readonly mutable = writable<OrchestrationState>(
        structuredClone(INITIAL_ORCHESTRATION_STATE),
    );
    readonly state: Readable<OrchestrationState> = this.mutable;

    private contextEpoch = 0;
    private roomDraftEpoch = 0;
    private planPreviewEpoch = 0;

    constructor(private readonly client: OrchestrationCapableClient) {}

    private update(updater: (state: OrchestrationState) => OrchestrationState): void {
        this.mutable.update(updater);
    }

    private updateForContext(
        contextKey: string,
        updater: (state: OrchestrationState) => OrchestrationState,
    ): boolean {
        let applied = false;
        this.mutable.update((state) => {
            if (state.context_key !== contextKey) return state;
            applied = true;
            return updater(state);
        });
        return applied;
    }

    private isCurrentContext(contextKey: string): boolean {
        return get(this.mutable).context_key === contextKey;
    }

    private invalidatePlanPreviewForContext(contextKey: string): boolean {
        if (!this.isCurrentContext(contextKey)) return false;
        ++this.planPreviewEpoch;
        return true;
    }

    private async loadEditablePromptPresetForContext(
        contextKey: string,
        promptPresetId: string | null,
    ): Promise<void> {
        const loader = this.client.getEditablePromptPreset;
        if (promptPresetId === null || loader === undefined) {
            this.updateForContext(contextKey, (state) => ({
                ...state,
                editable_prompt_preset: null,
                editable_prompt_preset_dirty: false,
                editable_prompt_preset_loading: false,
                editable_prompt_preset_error:
                    promptPresetId !== null && loader === undefined
                        ? t('orchestration.error.unsupported_block_edit')
                        : null,
            }));
            return;
        }
        this.updateForContext(contextKey, (state) => ({
            ...state,
            editable_prompt_preset_loading: true,
            editable_prompt_preset_error: null,
        }));
        try {
            const document = await loader.call(this.client, {
                prompt_preset_id: promptPresetId,
            });
            this.updateForContext(contextKey, (state) => ({
                ...state,
                editable_prompt_preset: document,
                editable_prompt_preset_dirty: false,
                editable_prompt_preset_loading: false,
                editable_prompt_preset_error: null,
            }));
        } catch (error: unknown) {
            this.updateForContext(contextKey, (state) => ({
                ...state,
                editable_prompt_preset: null,
                editable_prompt_preset_dirty: false,
                editable_prompt_preset_loading: false,
                editable_prompt_preset_error: errorLabel(error),
            }));
        }
    }

    private async loadEditableTaskProfilesForContext(contextKey: string): Promise<void> {
        const loader = this.client.listTaskProfiles;
        if (loader === undefined) {
            this.updateForContext(contextKey, (state) => ({
                ...state,
                editable_task_profiles: [],
                editable_task_profiles_loading: false,
                editable_task_profiles_error: t('orchestration.error.unsupported_task_edit'),
            }));
            return;
        }
        this.updateForContext(contextKey, (state) => ({
            ...state,
            editable_task_profiles_loading: true,
            editable_task_profiles_error: null,
        }));
        try {
            const profiles = await loader.call(this.client);
            this.updateForContext(contextKey, (state) => ({
                ...state,
                editable_task_profiles: profiles.map(({ value, revision }) => ({
                    value,
                    expected_revision: revision,
                    dirty: false,
                })),
                editable_task_profiles_loading: false,
                editable_task_profiles_error: null,
            }));
        } catch (error: unknown) {
            this.updateForContext(contextKey, (state) => ({
                ...state,
                editable_task_profiles: [],
                editable_task_profiles_loading: false,
                editable_task_profiles_error: errorLabel(error),
            }));
        }
    }

    private async loadEditableCreatorDocumentsForContext(contextKey: string): Promise<void> {
        const listMemoryProfiles = this.client.listMemoryProfiles;
        const listKnowledgeBooks = this.client.listKnowledgeBooks;
        const listTransformSets = this.client.listTransformSets;
        const listInteractionRuleSets = this.client.listInteractionRuleSets;
        const listContentModules = this.client.listContentModules;
        if (
            listMemoryProfiles === undefined ||
            listKnowledgeBooks === undefined ||
            listTransformSets === undefined ||
            listInteractionRuleSets === undefined ||
            listContentModules === undefined
        ) {
            this.updateForContext(contextKey, (state) => ({
                ...state,
                editable_creator_documents_loading: false,
                editable_creator_documents_error: t('orchestration.error.unsupported_creator_edit'),
            }));
            return;
        }
        this.updateForContext(contextKey, (state) => ({
            ...state,
            editable_creator_documents_loading: true,
            editable_creator_documents_error: null,
        }));
        try {
            const [
                memoryProfiles,
                knowledgeBooks,
                transformSets,
                interactionRuleSets,
                contentModules,
            ] = await Promise.all([
                listMemoryProfiles.call(this.client),
                listKnowledgeBooks.call(this.client),
                listTransformSets.call(this.client),
                listInteractionRuleSets.call(this.client),
                listContentModules.call(this.client),
            ]);
            this.updateForContext(contextKey, (state) => ({
                ...state,
                editable_memory_profiles: editableCreatorDocuments(memoryProfiles),
                editable_knowledge_books: editableCreatorDocuments(knowledgeBooks),
                editable_transform_sets: editableCreatorDocuments(transformSets),
                editable_interaction_rule_sets: editableCreatorDocuments(interactionRuleSets),
                editable_content_modules: editableCreatorDocuments(contentModules),
                editable_creator_documents_loading: false,
                editable_creator_documents_error: null,
            }));
        } catch (error: unknown) {
            this.updateForContext(contextKey, (state) => ({
                ...state,
                editable_creator_documents_loading: false,
                editable_creator_documents_error: errorLabel(error),
            }));
        }
    }

    async loadContext(conversationId: string | null, branchId: string | null): Promise<void> {
        const epoch = ++this.contextEpoch;
        ++this.roomDraftEpoch;
        ++this.planPreviewEpoch;
        if (conversationId === null || branchId === null) {
            this.mutable.set(structuredClone(INITIAL_ORCHESTRATION_STATE));
            return;
        }

        const contextKey = `${conversationId}:${branchId}`;
        const loader = this.client.getOrchestrationWorkspace;
        if (loader === undefined) {
            this.mutable.set({
                ...structuredClone(INITIAL_ORCHESTRATION_STATE),
                phase: 'unavailable',
                context_key: contextKey,
                error: t('orchestration.error.unsupported'),
                workspace: emptyOrchestrationWorkspace(conversationId, branchId),
            });
            return;
        }

        this.update((state) => ({
            ...state,
            phase: 'loading',
            saving: false,
            busy_interaction_proposal_id: null,
            error: null,
            announcement: '',
            context_key: contextKey,
            dirty_room_config: false,
            workspace: emptyOrchestrationWorkspace(conversationId, branchId),
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
        }));
        try {
            const snapshot = await loader.call(this.client, conversationId, branchId);
            if (epoch !== this.contextEpoch) return;
            const response: OrchestrationWorkspaceDto = {
                ...emptyOrchestrationWorkspace(conversationId, branchId),
                ...snapshot,
            };
            const expireProposals = this.client.expireInteractionProposals;
            const listProposals = this.client.listInteractionProposals;
            if (expireProposals !== undefined && listProposals !== undefined) {
                const expiry = await expireProposals.call(this.client, {
                    conversation_id: conversationId,
                    branch_id: branchId,
                    limit: 100,
                });
                if (
                    expiry.conversation_id !== conversationId ||
                    expiry.branch_id !== branchId ||
                    !Number.isSafeInteger(expiry.current_state_revision) ||
                    expiry.current_state_revision < 0 ||
                    expiry.has_more_expired
                ) {
                    throw new Error('interaction proposal expiry authority validation failed');
                }
                validateInteractionProposalPage(
                    expiry.expired_proposals,
                    conversationId,
                    branchId,
                    'expired',
                );
                const pending = await listProposals.call(this.client, {
                    conversation_id: conversationId,
                    branch_id: branchId,
                    status: 'pending',
                    limit: 100,
                });
                validateInteractionProposalPage(pending, conversationId, branchId, 'pending');
                if (pending.some((item) => item.state_revision !== expiry.current_state_revision)) {
                    throw new Error('interaction proposal state revision changed during refresh');
                }
                response.interaction_state_revision = expiry.current_state_revision;
                response.interaction_proposals = pending;
            }
            if (epoch !== this.contextEpoch) return;
            const promptSourceError = roomPromptSourceValidationError(response.room_config);
            if (promptSourceError !== null) {
                throw new Error(
                    t('orchestration.error.prompt_source_limit', { detail: promptSourceError }),
                );
            }
            const bounded = boundedWorkspace(response);
            this.update((state) => ({
                ...state,
                phase: 'ready',
                error: null,
                workspace: bounded.workspace,
                list_truncation: bounded.truncation,
            }));
            await Promise.all([
                this.loadEditablePromptPresetForContext(
                    contextKey,
                    response.room_config.prompt_preset_id,
                ),
                this.loadEditableTaskProfilesForContext(contextKey),
                this.loadEditableCreatorDocumentsForContext(contextKey),
            ]);
        } catch (error: unknown) {
            if (epoch !== this.contextEpoch) return;
            this.update((state) => ({
                ...state,
                phase: 'error',
                error: errorLabel(error),
            }));
        }
    }

    stageRoomConfig(patch: RoomOrchestrationConfigPatch): void {
        const state = get(this.mutable);
        if (state.phase !== 'ready') return;
        const supported = state.workspace.room_config.supported_fields;
        const accepted: RoomOrchestrationConfigPatch = {};
        if (patch.prompt_preset_id !== undefined && supported.prompt_preset_id) {
            accepted.prompt_preset_id = patch.prompt_preset_id;
        }
        if (patch.generation_preset_id !== undefined && supported.generation_preset_id) {
            accepted.generation_preset_id = patch.generation_preset_id;
        }
        if (patch.creator_values !== undefined && supported.creator_values) {
            accepted.creator_values = patch.creator_values;
        }
        if (patch.variable_overrides !== undefined && supported.variable_overrides) {
            accepted.variable_overrides = patch.variable_overrides;
        }
        if (patch.response_length !== undefined && supported.response_length) {
            accepted.response_length = patch.response_length;
        }
        if (patch.creativity !== undefined && supported.creativity) {
            accepted.creativity = patch.creativity;
        }
        if (patch.reasoning_effort !== undefined && supported.reasoning_effort) {
            accepted.reasoning_effort = patch.reasoning_effort;
        }
        if (patch.memory_enabled !== undefined && supported.memory_enabled) {
            accepted.memory_enabled = patch.memory_enabled;
        }
        if (patch.knowledge_enabled !== undefined && supported.knowledge_enabled) {
            accepted.knowledge_enabled = patch.knowledge_enabled;
        }
        if (patch.user_name_override !== undefined && supported.user_name_override) {
            accepted.user_name_override = patch.user_name_override;
        }
        if (patch.author_note !== undefined && supported.author_note) {
            accepted.author_note = patch.author_note;
        }
        if (patch.group_context !== undefined && supported.group_context) {
            accepted.group_context = patch.group_context;
        }
        if (patch.template_slots !== undefined && supported.template_slots) {
            accepted.template_slots = structuredClone(patch.template_slots);
        }
        if (Object.keys(accepted).length === 0) return;
        ++this.roomDraftEpoch;
        this.invalidatePlanPreviewForContext(state.context_key);
        this.update((current) => ({
            ...current,
            dirty_room_config: true,
            workspace: {
                ...current.workspace,
                room_config: {
                    ...current.workspace.room_config,
                    ...accepted,
                },
                plan_preview: null,
            },
        }));
        if (accepted.prompt_preset_id !== undefined) {
            void this.loadEditablePromptPresetForContext(
                state.context_key,
                accepted.prompt_preset_id,
            );
        }
    }

    stageCreatorControl(controlId: string, value: CreatorControlValue): void {
        const state = get(this.mutable);
        this.stageRoomConfig({
            creator_values: {
                ...state.workspace.room_config.creator_values,
                [controlId]: value,
            },
        });
    }

    async saveRoomConfig(): Promise<boolean> {
        const saver = this.client.saveRoomOrchestrationConfig;
        if (saver === undefined) {
            this.update((state) => ({
                ...state,
                phase: 'unavailable',
                error: t('orchestration.error.unsupported_room_save'),
            }));
            return false;
        }
        const state = get(this.mutable);
        if (state.phase !== 'ready' || state.saving) return false;
        const contextKey = state.context_key;
        const contextEpoch = this.contextEpoch;
        const draftEpoch = this.roomDraftEpoch;
        const config = structuredClone(state.workspace.room_config);
        const promptSourceError = roomPromptSourceValidationError(config);
        if (promptSourceError !== null) {
            this.updateForContext(contextKey, (current) => ({
                ...current,
                error: promptSourceError,
            }));
            return false;
        }
        const input = {
            conversation_id: config.conversation_id,
            branch_id: config.branch_id,
            prompt_preset_id: config.prompt_preset_id,
            generation_preset_id: config.generation_preset_id,
            creator_values: structuredClone(config.creator_values),
            variable_overrides: structuredClone(config.variable_overrides),
            response_length: config.response_length,
            creativity: config.creativity,
            reasoning_effort: config.reasoning_effort,
            memory_enabled: config.memory_enabled,
            knowledge_enabled: config.knowledge_enabled,
            user_name_override: config.user_name_override,
            author_note: config.author_note,
            group_context: config.group_context,
            template_slots: structuredClone(config.template_slots),
            expected_revision: state.workspace.room_config_revision,
        };
        this.update((state) => ({ ...state, saving: true, error: null }));
        try {
            const saved = await saver.call(this.client, input);
            if (contextEpoch !== this.contextEpoch) return false;
            this.invalidatePlanPreviewForContext(contextKey);
            return this.updateForContext(contextKey, (current) => {
                if (draftEpoch !== this.roomDraftEpoch) {
                    return {
                        ...current,
                        saving: false,
                        dirty_room_config: true,
                        announcement: t('orchestration.notice.unsaved_changes'),
                        workspace: {
                            ...current.workspace,
                            room_config: {
                                ...current.workspace.room_config,
                                conversation_id: saved.room_config.conversation_id,
                                branch_id: saved.room_config.branch_id,
                                supported_fields: saved.room_config.supported_fields,
                            },
                            room_config_revision: saved.revision,
                            generation_target: saved.generation_target,
                            plan_preview: null,
                        },
                    };
                }
                return {
                    ...current,
                    saving: false,
                    dirty_room_config: false,
                    announcement: t('orchestration.notice.room_saved'),
                    workspace: {
                        ...current.workspace,
                        room_config: saved.room_config,
                        room_config_revision: saved.revision,
                        generation_target: saved.generation_target,
                        plan_preview: null,
                    },
                };
            });
        } catch (error: unknown) {
            if (contextEpoch !== this.contextEpoch) return false;
            this.updateForContext(contextKey, (current) => ({
                ...current,
                saving: false,
                error: errorLabel(error),
            }));
            return false;
        }
    }

    stageEditablePromptBlock(blockId: string, patch: EditablePromptBlockPatch): boolean {
        const state = get(this.mutable);
        const document = state.editable_prompt_preset;
        if (state.phase !== 'ready' || document === null) return false;
        const index = document.value.blocks.findIndex((block) => block.id === blockId);
        if (index < 0) return false;
        this.invalidatePlanPreviewForContext(state.context_key);
        this.updateForContext(state.context_key, (current) => ({
            ...current,
            editable_prompt_preset: {
                ...document,
                value: {
                    ...document.value,
                    blocks: document.value.blocks.map((block) =>
                        block.id === blockId ? { ...block, ...patch } : block,
                    ),
                },
            },
            editable_prompt_preset_dirty: true,
            editable_prompt_preset_error: null,
            workspace: {
                ...current.workspace,
                plan_preview: null,
            },
        }));
        return true;
    }

    setEditablePromptCacheBoundary(blockId: string, enabled: boolean): boolean {
        const state = get(this.mutable);
        const document = state.editable_prompt_preset;
        if (state.phase !== 'ready' || document === null) return false;
        if (!document.value.blocks.some((block) => block.id === blockId)) return false;
        const existing = document.value.cache_boundaries.filter(
            (boundary) => boundary.after_block_id === blockId,
        );
        if ((enabled && existing.length > 0) || (!enabled && existing.length === 0)) return true;
        this.invalidatePlanPreviewForContext(state.context_key);
        let cacheBoundaries = document.value.cache_boundaries.filter(
            (boundary) => boundary.after_block_id !== blockId,
        );
        if (enabled) {
            const usedIds = new Set(cacheBoundaries.map(({ id }) => id));
            const baseId = `cache-${blockId}`.slice(0, 240);
            let candidate = baseId;
            let suffix = 2;
            while (usedIds.has(candidate)) {
                candidate = `${baseId}-${String(suffix)}`;
                suffix += 1;
            }
            cacheBoundaries = [
                ...cacheBoundaries,
                {
                    id: candidate,
                    after_block_id: blockId,
                    role_filter: { kind: 'all' },
                    ttl: 'provider_default',
                    mode: 'automatic',
                },
            ];
        }
        this.updateForContext(state.context_key, (current) => ({
            ...current,
            editable_prompt_preset: {
                ...document,
                value: {
                    ...document.value,
                    cache_boundaries: cacheBoundaries,
                },
            },
            editable_prompt_preset_dirty: true,
            editable_prompt_preset_error: null,
            workspace: {
                ...current.workspace,
                plan_preview: null,
            },
        }));
        return true;
    }

    stageEditablePromptCacheBoundary(
        blockId: string,
        patch: Partial<
            Pick<
                CreatorPromptPresetDocumentDto['cache_boundaries'][number],
                'role_filter' | 'ttl' | 'mode'
            >
        >,
    ): boolean {
        const state = get(this.mutable);
        const document = state.editable_prompt_preset;
        if (document === null) return false;
        const boundary = document.value.cache_boundaries.find(
            (candidate) => candidate.after_block_id === blockId,
        );
        if (boundary === undefined) return false;
        this.invalidatePlanPreviewForContext(state.context_key);
        this.updateForContext(state.context_key, (current) => ({
            ...current,
            editable_prompt_preset: {
                ...document,
                value: {
                    ...document.value,
                    cache_boundaries: document.value.cache_boundaries.map((candidate) =>
                        candidate.id === boundary.id ? { ...candidate, ...patch } : candidate,
                    ),
                },
            },
            editable_prompt_preset_dirty: true,
            editable_prompt_preset_error: null,
            workspace: {
                ...current.workspace,
                plan_preview: null,
            },
        }));
        return true;
    }

    async reloadEditablePromptPreset(): Promise<void> {
        const state = get(this.mutable);
        await this.loadEditablePromptPresetForContext(
            state.context_key,
            state.workspace.room_config.prompt_preset_id,
        );
    }

    async saveEditablePromptPreset(): Promise<boolean> {
        const state = get(this.mutable);
        const document = state.editable_prompt_preset;
        const save = this.client.upsertPromptPreset;
        const reload = this.client.getEditablePromptPreset;
        if (document === null || !state.editable_prompt_preset_dirty) return false;
        if (save === undefined || reload === undefined) {
            this.updateForContext(state.context_key, (current) => ({
                ...current,
                editable_prompt_preset_error: t('orchestration.error.unsupported_block_save'),
            }));
            return false;
        }
        const contextKey = state.context_key;
        this.updateForContext(contextKey, (current) => ({
            ...current,
            editable_prompt_preset_loading: true,
            editable_prompt_preset_error: null,
        }));
        try {
            const summary = await save.call(this.client, {
                value: document.value,
                expected_revision: document.revision,
            });
            const refreshed = await reload.call(this.client, {
                prompt_preset_id: document.value.id,
            });
            if (!this.invalidatePlanPreviewForContext(contextKey)) return false;
            return this.updateForContext(contextKey, (current) => {
                const currentDocument = current.editable_prompt_preset;
                const hasNewerDraft =
                    current.editable_prompt_preset_dirty &&
                    currentDocument !== null &&
                    currentDocument !== document;
                return {
                    ...current,
                    editable_prompt_preset: hasNewerDraft
                        ? {
                              ...refreshed,
                              value: currentDocument.value,
                          }
                        : refreshed,
                    editable_prompt_preset_dirty: hasNewerDraft,
                    editable_prompt_preset_loading: false,
                    editable_prompt_preset_error: null,
                    announcement: hasNewerDraft
                        ? t('orchestration.notice.preset_saved_partial', {
                              name: summary.value.name,
                          })
                        : t('orchestration.notice.preset_saved', { name: summary.value.name }),
                    workspace: {
                        ...current.workspace,
                        prompt_preset_revision: refreshed.revision,
                        prompt_presets: current.workspace.prompt_presets.map((preset) =>
                            preset.id === summary.value.id ? summary.value : preset,
                        ),
                        plan_preview: null,
                    },
                };
            });
        } catch (error: unknown) {
            this.updateForContext(contextKey, (current) => ({
                ...current,
                editable_prompt_preset_loading: false,
                editable_prompt_preset_error: errorLabel(error),
            }));
            return false;
        }
    }

    addTaskProfileDraft(taskProfileId: string): boolean {
        const state = get(this.mutable);
        const id = taskProfileId.trim();
        if (
            state.phase !== 'ready' ||
            id === '' ||
            id.length > 256 ||
            state.editable_task_profiles.some((profile) => profile.value.id === id)
        ) {
            return false;
        }
        this.updateForContext(state.context_key, (current) => ({
            ...current,
            editable_task_profiles: [
                ...current.editable_task_profiles,
                {
                    value: {
                        id,
                        kind: 'memory_summary',
                        route_id: '',
                        generation_preset_id: '',
                        fallback_route_ids: [],
                        embedding_dimensions: null,
                        timeout_ms: 30_000,
                        rate_limit: { requests: 1, per_seconds: 60 },
                        concurrency_limit: 1,
                    },
                    expected_revision: null,
                    dirty: true,
                },
            ],
            editable_task_profiles_error: null,
        }));
        return true;
    }

    stageTaskProfile(taskProfileId: string, patch: Partial<TaskProfileDocumentDto>): boolean {
        const state = get(this.mutable);
        if (!state.editable_task_profiles.some((profile) => profile.value.id === taskProfileId)) {
            return false;
        }
        this.updateForContext(state.context_key, (current) => ({
            ...current,
            editable_task_profiles: current.editable_task_profiles.map((profile) =>
                profile.value.id === taskProfileId
                    ? (() => {
                          const nextValue = {
                              ...profile.value,
                              ...patch,
                              id: taskProfileId,
                          };
                          if (nextValue.kind === 'memory_embedding') {
                              nextValue.fallback_route_ids = [];
                          } else {
                              nextValue.embedding_dimensions = null;
                          }
                          return {
                              ...profile,
                              value: nextValue,
                              dirty: true,
                          };
                      })()
                    : profile,
            ),
            editable_task_profiles_error: null,
        }));
        return true;
    }

    async saveTaskProfile(taskProfileId: string): Promise<boolean> {
        const state = get(this.mutable);
        const profile = state.editable_task_profiles.find(
            (candidate) => candidate.value.id === taskProfileId,
        );
        const save = this.client.upsertTaskProfile;
        if (!profile?.dirty) return false;
        const validationError = taskProfileValidationError(profile.value);
        if (validationError !== null) {
            this.updateForContext(state.context_key, (current) => ({
                ...current,
                editable_task_profiles_error: validationError,
            }));
            return false;
        }
        if (save === undefined) {
            this.updateForContext(state.context_key, (current) => ({
                ...current,
                editable_task_profiles_error: t('orchestration.error.unsupported_task_save'),
            }));
            return false;
        }
        const contextKey = state.context_key;
        this.updateForContext(contextKey, (current) => ({
            ...current,
            editable_task_profiles_loading: true,
            editable_task_profiles_error: null,
        }));
        try {
            const saved = await save.call(this.client, {
                value: profile.value,
                expected_revision: profile.expected_revision,
            });
            return this.updateForContext(contextKey, (current) => {
                const currentProfile = current.editable_task_profiles.find(
                    (candidate) => candidate.value.id === taskProfileId,
                );
                const hasNewerDraft =
                    currentProfile !== undefined &&
                    currentProfile !== profile &&
                    currentProfile.dirty;
                return {
                    ...current,
                    editable_task_profiles: current.editable_task_profiles.map((candidate) =>
                        candidate.value.id === taskProfileId
                            ? hasNewerDraft
                                ? {
                                      value: candidate.value,
                                      expected_revision: saved.revision,
                                      dirty: true,
                                  }
                                : {
                                      value: saved.value,
                                      expected_revision: saved.revision,
                                      dirty: false,
                                  }
                            : candidate,
                    ),
                    editable_task_profiles_loading: false,
                    editable_task_profiles_error: null,
                    announcement: hasNewerDraft
                        ? t('orchestration.notice.task_saved_partial', { name: saved.value.id })
                        : t('orchestration.notice.task_saved', { name: saved.value.id }),
                };
            });
        } catch (error: unknown) {
            this.updateForContext(contextKey, (current) => ({
                ...current,
                editable_task_profiles_loading: false,
                editable_task_profiles_error: errorLabel(error),
            }));
            return false;
        }
    }

    async deleteTaskProfile(taskProfileId: string): Promise<boolean> {
        const state = get(this.mutable);
        const profile = state.editable_task_profiles.find(
            (candidate) => candidate.value.id === taskProfileId,
        );
        if (profile === undefined) return false;
        if (profile.expected_revision === null) {
            return this.updateForContext(state.context_key, (current) => ({
                ...current,
                editable_task_profiles: current.editable_task_profiles.filter(
                    (candidate) => candidate.value.id !== taskProfileId,
                ),
            }));
        }
        const remove = this.client.deleteTaskProfile;
        if (remove === undefined) {
            this.updateForContext(state.context_key, (current) => ({
                ...current,
                editable_task_profiles_error: t('orchestration.error.unsupported_task_delete'),
            }));
            return false;
        }
        const contextKey = state.context_key;
        this.updateForContext(contextKey, (current) => ({
            ...current,
            editable_task_profiles_loading: true,
            editable_task_profiles_error: null,
        }));
        try {
            await remove.call(this.client, {
                task_profile_id: taskProfileId,
                expected_revision: profile.expected_revision,
            });
            return this.updateForContext(contextKey, (current) => ({
                ...current,
                editable_task_profiles: current.editable_task_profiles.filter(
                    (candidate) => candidate.value.id !== taskProfileId,
                ),
                editable_task_profiles_loading: false,
                editable_task_profiles_error: null,
                announcement: t('orchestration.notice.task_deleted', { name: taskProfileId }),
            }));
        } catch (error: unknown) {
            this.updateForContext(contextKey, (current) => ({
                ...current,
                editable_task_profiles_loading: false,
                editable_task_profiles_error: errorLabel(error),
            }));
            return false;
        }
    }

    addCreatorDocumentDraft(kind: CreatorDocumentKind, requestedId: string): boolean {
        const state = get(this.mutable);
        const id = requestedId.trim();
        if (state.phase !== 'ready' || !validNewCreatorDocumentId(id)) return false;
        const duplicate =
            (kind === 'memory_profile' &&
                state.editable_memory_profiles.some((document) => document.value.id === id)) ||
            (kind === 'knowledge_book' &&
                state.editable_knowledge_books.some((document) => document.value.id === id)) ||
            (kind === 'transform_set' &&
                state.editable_transform_sets.some((document) => document.value.id === id)) ||
            (kind === 'interaction_rule_set' &&
                state.editable_interaction_rule_sets.some(
                    (document) => document.value.id === id,
                )) ||
            (kind === 'content_module' &&
                state.editable_content_modules.some((document) => document.value.id === id));
        if (duplicate) return false;
        return this.updateForContext(state.context_key, (current) => {
            const base = {
                ...current,
                editable_creator_documents_error: null,
            };
            if (kind === 'memory_profile') {
                return {
                    ...base,
                    editable_memory_profiles: [
                        ...current.editable_memory_profiles,
                        {
                            value: memoryProfileDraft(id),
                            expected_revision: null,
                            dirty: true,
                        },
                    ],
                };
            }
            if (kind === 'knowledge_book') {
                return {
                    ...base,
                    editable_knowledge_books: [
                        ...current.editable_knowledge_books,
                        {
                            value: knowledgeBookDraft(id),
                            expected_revision: null,
                            dirty: true,
                        },
                    ],
                };
            }
            if (kind === 'transform_set') {
                return {
                    ...base,
                    editable_transform_sets: [
                        ...current.editable_transform_sets,
                        {
                            value: transformSetDraft(id),
                            expected_revision: null,
                            dirty: true,
                        },
                    ],
                };
            }
            if (kind === 'interaction_rule_set') {
                return {
                    ...base,
                    editable_interaction_rule_sets: [
                        ...current.editable_interaction_rule_sets,
                        {
                            value: interactionRuleSetDraft(id),
                            expected_revision: null,
                            dirty: true,
                        },
                    ],
                };
            }
            return {
                ...base,
                editable_content_modules: [
                    ...current.editable_content_modules,
                    {
                        value: contentModuleDraft(id),
                        expected_revision: null,
                        dirty: true,
                    },
                ],
            };
        });
    }

    replaceCreatorDocument(
        kind: CreatorDocumentKind,
        documentId: string,
        value: CreatorDocumentValue,
    ): boolean {
        const state = get(this.mutable);
        if (value.id !== documentId) return false;
        if (kind === 'memory_profile') {
            if (
                !state.editable_memory_profiles.some((document) => document.value.id === documentId)
            ) {
                return false;
            }
            return this.updateForContext(state.context_key, (current) => ({
                ...current,
                editable_memory_profiles: replaceEditableCreatorDocument(
                    current.editable_memory_profiles,
                    documentId,
                    value as CreatorMemoryProfileDocumentDto,
                ),
                editable_creator_documents_error: null,
            }));
        }
        if (kind === 'knowledge_book') {
            if (
                !state.editable_knowledge_books.some((document) => document.value.id === documentId)
            ) {
                return false;
            }
            return this.updateForContext(state.context_key, (current) => ({
                ...current,
                editable_knowledge_books: replaceEditableCreatorDocument(
                    current.editable_knowledge_books,
                    documentId,
                    value as CreatorKnowledgeBookDocumentDto,
                ),
                editable_creator_documents_error: null,
            }));
        }
        if (kind === 'transform_set') {
            if (
                !state.editable_transform_sets.some((document) => document.value.id === documentId)
            ) {
                return false;
            }
            return this.updateForContext(state.context_key, (current) => ({
                ...current,
                editable_transform_sets: replaceEditableCreatorDocument(
                    current.editable_transform_sets,
                    documentId,
                    value as CreatorTransformSetDocumentDto,
                ),
                editable_creator_documents_error: null,
            }));
        }
        if (kind === 'interaction_rule_set') {
            if (
                !state.editable_interaction_rule_sets.some(
                    (document) => document.value.id === documentId,
                )
            ) {
                return false;
            }
            return this.updateForContext(state.context_key, (current) => ({
                ...current,
                editable_interaction_rule_sets: replaceEditableCreatorDocument(
                    current.editable_interaction_rule_sets,
                    documentId,
                    value as CreatorInteractionRuleSetDocumentDto,
                ),
                editable_creator_documents_error: null,
            }));
        }
        if (!state.editable_content_modules.some((document) => document.value.id === documentId)) {
            return false;
        }
        return this.updateForContext(state.context_key, (current) => ({
            ...current,
            editable_content_modules: replaceEditableCreatorDocument(
                current.editable_content_modules,
                documentId,
                value as CreatorContentModuleDocumentDto,
            ),
            editable_creator_documents_error: null,
        }));
    }

    stageMemoryProfile(
        documentId: string,
        patch: Partial<CreatorMemoryProfileDocumentDto>,
    ): boolean {
        const state = get(this.mutable);
        if (!state.editable_memory_profiles.some((document) => document.value.id === documentId)) {
            return false;
        }
        return this.updateForContext(state.context_key, (current) => ({
            ...current,
            editable_memory_profiles: stageEditableCreatorDocument(
                current.editable_memory_profiles,
                documentId,
                patch,
            ),
            editable_creator_documents_error: null,
        }));
    }

    stageKnowledgeBook(
        documentId: string,
        patch: Partial<CreatorKnowledgeBookDocumentDto>,
    ): boolean {
        const state = get(this.mutable);
        if (!state.editable_knowledge_books.some((document) => document.value.id === documentId)) {
            return false;
        }
        return this.updateForContext(state.context_key, (current) => ({
            ...current,
            editable_knowledge_books: stageEditableCreatorDocument(
                current.editable_knowledge_books,
                documentId,
                patch,
            ),
            editable_creator_documents_error: null,
        }));
    }

    stageTransformSet(documentId: string, patch: Partial<CreatorTransformSetDocumentDto>): boolean {
        const state = get(this.mutable);
        if (!state.editable_transform_sets.some((document) => document.value.id === documentId)) {
            return false;
        }
        return this.updateForContext(state.context_key, (current) => ({
            ...current,
            editable_transform_sets: stageEditableCreatorDocument(
                current.editable_transform_sets,
                documentId,
                patch,
            ),
            editable_creator_documents_error: null,
        }));
    }

    stageInteractionRuleSet(
        documentId: string,
        patch: Partial<CreatorInteractionRuleSetDocumentDto>,
    ): boolean {
        const state = get(this.mutable);
        if (
            !state.editable_interaction_rule_sets.some(
                (document) => document.value.id === documentId,
            )
        ) {
            return false;
        }
        return this.updateForContext(state.context_key, (current) => ({
            ...current,
            editable_interaction_rule_sets: stageEditableCreatorDocument(
                current.editable_interaction_rule_sets,
                documentId,
                patch,
            ),
            editable_creator_documents_error: null,
        }));
    }

    stageContentModule(
        documentId: string,
        patch: Partial<CreatorContentModuleDocumentDto>,
    ): boolean {
        const state = get(this.mutable);
        if (!state.editable_content_modules.some((document) => document.value.id === documentId)) {
            return false;
        }
        return this.updateForContext(state.context_key, (current) => ({
            ...current,
            editable_content_modules: stageEditableCreatorDocument(
                current.editable_content_modules,
                documentId,
                patch,
            ),
            editable_creator_documents_error: null,
        }));
    }

    private async saveCreatorDocumentValue<Value extends { id: string }>(
        document: EditableCreatorDocumentState<Value>,
        save:
            | ((input: {
                  value: Value;
                  expected_revision: number | null;
              }) => Promise<RevisionedDto<Value>>)
            | undefined,
        currentDocuments: (state: OrchestrationState) => EditableCreatorDocumentState<Value>[],
        applySaved: (state: OrchestrationState, saved: RevisionedDto<Value>) => OrchestrationState,
        label: string,
    ): Promise<boolean> {
        const state = get(this.mutable);
        if (!document.dirty) return false;
        if (save === undefined) {
            this.updateForContext(state.context_key, (current) => ({
                ...current,
                editable_creator_documents_error: t(
                    'orchestration.error.unsupported_document_save',
                    { label },
                ),
            }));
            return false;
        }
        const contextKey = state.context_key;
        this.updateForContext(contextKey, (current) => ({
            ...current,
            editable_creator_documents_loading: true,
            editable_creator_documents_error: null,
        }));
        try {
            const saved = await save.call(this.client, {
                value: document.value,
                expected_revision: document.expected_revision,
            });
            return this.updateForContext(contextKey, (current) => {
                const currentDocument = currentDocuments(current).find(
                    (candidate) => candidate.value.id === document.value.id,
                );
                const hasNewerDraft =
                    currentDocument !== undefined &&
                    currentDocument !== document &&
                    currentDocument.dirty;
                return {
                    ...applySaved(current, saved),
                    editable_creator_documents_loading: false,
                    editable_creator_documents_error: null,
                    announcement: hasNewerDraft
                        ? t('orchestration.notice.document_saved_partial', {
                              id: saved.value.id,
                              label,
                          })
                        : t('orchestration.notice.document_saved', { id: saved.value.id, label }),
                };
            });
        } catch (error: unknown) {
            this.updateForContext(contextKey, (current) => ({
                ...current,
                editable_creator_documents_loading: false,
                editable_creator_documents_error: errorLabel(error),
            }));
            return false;
        }
    }

    async saveCreatorDocument(kind: CreatorDocumentKind, documentId: string): Promise<boolean> {
        const state = get(this.mutable);
        if (kind === 'memory_profile') {
            const document = state.editable_memory_profiles.find(
                (candidate) => candidate.value.id === documentId,
            );
            if (document === undefined) return false;
            return this.saveCreatorDocumentValue(
                document,
                this.client.upsertMemoryProfile,
                (current) => current.editable_memory_profiles,
                (current, saved) => ({
                    ...current,
                    editable_memory_profiles: replaceSavedCreatorDocument(
                        current.editable_memory_profiles,
                        saved,
                        document,
                    ),
                }),
                t('orchestration.label.memory_profile'),
            );
        }
        if (kind === 'knowledge_book') {
            const document = state.editable_knowledge_books.find(
                (candidate) => candidate.value.id === documentId,
            );
            if (document === undefined) return false;
            return this.saveCreatorDocumentValue(
                document,
                this.client.upsertKnowledgeBook,
                (current) => current.editable_knowledge_books,
                (current, saved) => ({
                    ...current,
                    editable_knowledge_books: replaceSavedCreatorDocument(
                        current.editable_knowledge_books,
                        saved,
                        document,
                    ),
                }),
                t('orchestration.label.knowledge_book'),
            );
        }
        if (kind === 'transform_set') {
            const document = state.editable_transform_sets.find(
                (candidate) => candidate.value.id === documentId,
            );
            if (document === undefined) return false;
            return this.saveCreatorDocumentValue(
                document,
                this.client.upsertTransformSet,
                (current) => current.editable_transform_sets,
                (current, saved) => ({
                    ...current,
                    editable_transform_sets: replaceSavedCreatorDocument(
                        current.editable_transform_sets,
                        saved,
                        document,
                    ),
                }),
                t('orchestration.label.transform_set'),
            );
        }
        if (kind === 'interaction_rule_set') {
            const document = state.editable_interaction_rule_sets.find(
                (candidate) => candidate.value.id === documentId,
            );
            if (document === undefined) return false;
            return this.saveCreatorDocumentValue(
                document,
                this.client.upsertInteractionRuleSet,
                (current) => current.editable_interaction_rule_sets,
                (current, saved) => ({
                    ...current,
                    editable_interaction_rule_sets: replaceSavedCreatorDocument(
                        current.editable_interaction_rule_sets,
                        saved,
                        document,
                    ),
                }),
                t('orchestration.label.interaction_rule_set'),
            );
        }
        const document = state.editable_content_modules.find(
            (candidate) => candidate.value.id === documentId,
        );
        if (document === undefined) return false;
        return this.saveCreatorDocumentValue(
            document,
            this.client.upsertContentModule,
            (current) => current.editable_content_modules,
            (current, saved) => ({
                ...current,
                editable_content_modules: replaceSavedCreatorDocument(
                    current.editable_content_modules,
                    saved,
                    document,
                ),
            }),
            t('orchestration.label.content_module'),
        );
    }

    private async deleteCreatorDocumentValue<Value, Input>(
        remove: ((input: Input) => Promise<RevisionedDto<Value>>) | undefined,
        input: Input,
        applyDeleted: (state: OrchestrationState) => OrchestrationState,
        label: string,
        documentId: string,
    ): Promise<boolean> {
        const state = get(this.mutable);
        if (remove === undefined) {
            this.updateForContext(state.context_key, (current) => ({
                ...current,
                editable_creator_documents_error: t(
                    'orchestration.error.unsupported_document_delete',
                    { label },
                ),
            }));
            return false;
        }
        const contextKey = state.context_key;
        this.updateForContext(contextKey, (current) => ({
            ...current,
            editable_creator_documents_loading: true,
            editable_creator_documents_error: null,
        }));
        try {
            await remove.call(this.client, input);
            return this.updateForContext(contextKey, (current) => ({
                ...applyDeleted(current),
                editable_creator_documents_loading: false,
                editable_creator_documents_error: null,
                announcement: t('orchestration.notice.document_deleted', { id: documentId, label }),
            }));
        } catch (error: unknown) {
            this.updateForContext(contextKey, (current) => ({
                ...current,
                editable_creator_documents_loading: false,
                editable_creator_documents_error: errorLabel(error),
            }));
            return false;
        }
    }

    deleteCreatorDocument(kind: CreatorDocumentKind, documentId: string): Promise<boolean> {
        const state = get(this.mutable);
        if (kind === 'memory_profile') {
            const document = state.editable_memory_profiles.find(
                (candidate) => candidate.value.id === documentId,
            );
            if (document === undefined) return Promise.resolve(false);
            if (document.expected_revision === null) {
                return Promise.resolve(
                    this.updateForContext(state.context_key, (current) => ({
                        ...current,
                        editable_memory_profiles: current.editable_memory_profiles.filter(
                            (candidate) => candidate.value.id !== documentId,
                        ),
                        announcement: t('orchestration.notice.draft_discarded', {
                            id: documentId,
                            label: t('orchestration.label.memory_profile'),
                        }),
                    })),
                );
            }
            return this.deleteCreatorDocumentValue(
                this.client.deleteMemoryProfile,
                {
                    memory_profile_id: documentId,
                    expected_revision: document.expected_revision,
                },
                (current) => ({
                    ...current,
                    editable_memory_profiles: current.editable_memory_profiles.filter(
                        (candidate) => candidate.value.id !== documentId,
                    ),
                }),
                t('orchestration.label.memory_profile'),
                documentId,
            );
        }
        if (kind === 'knowledge_book') {
            const document = state.editable_knowledge_books.find(
                (candidate) => candidate.value.id === documentId,
            );
            if (document === undefined) return Promise.resolve(false);
            if (document.expected_revision === null) {
                return Promise.resolve(
                    this.updateForContext(state.context_key, (current) => ({
                        ...current,
                        editable_knowledge_books: current.editable_knowledge_books.filter(
                            (candidate) => candidate.value.id !== documentId,
                        ),
                        announcement: t('orchestration.notice.draft_discarded', {
                            id: documentId,
                            label: t('orchestration.label.knowledge_book'),
                        }),
                    })),
                );
            }
            return this.deleteCreatorDocumentValue(
                this.client.deleteKnowledgeBook,
                {
                    knowledge_book_id: documentId,
                    expected_revision: document.expected_revision,
                },
                (current) => ({
                    ...current,
                    editable_knowledge_books: current.editable_knowledge_books.filter(
                        (candidate) => candidate.value.id !== documentId,
                    ),
                }),
                t('orchestration.label.knowledge_book'),
                documentId,
            );
        }
        if (kind === 'transform_set') {
            const document = state.editable_transform_sets.find(
                (candidate) => candidate.value.id === documentId,
            );
            if (document === undefined) return Promise.resolve(false);
            if (document.expected_revision === null) {
                return Promise.resolve(
                    this.updateForContext(state.context_key, (current) => ({
                        ...current,
                        editable_transform_sets: current.editable_transform_sets.filter(
                            (candidate) => candidate.value.id !== documentId,
                        ),
                        announcement: t('orchestration.notice.draft_discarded', {
                            id: documentId,
                            label: t('orchestration.label.transform_set'),
                        }),
                    })),
                );
            }
            return this.deleteCreatorDocumentValue(
                this.client.deleteTransformSet,
                {
                    transform_set_id: documentId,
                    expected_revision: document.expected_revision,
                },
                (current) => ({
                    ...current,
                    editable_transform_sets: current.editable_transform_sets.filter(
                        (candidate) => candidate.value.id !== documentId,
                    ),
                }),
                t('orchestration.label.transform_set'),
                documentId,
            );
        }
        if (kind === 'interaction_rule_set') {
            const document = state.editable_interaction_rule_sets.find(
                (candidate) => candidate.value.id === documentId,
            );
            if (document === undefined) return Promise.resolve(false);
            if (document.expected_revision === null) {
                return Promise.resolve(
                    this.updateForContext(state.context_key, (current) => ({
                        ...current,
                        editable_interaction_rule_sets:
                            current.editable_interaction_rule_sets.filter(
                                (candidate) => candidate.value.id !== documentId,
                            ),
                        announcement: t('orchestration.notice.draft_discarded', {
                            id: documentId,
                            label: t('orchestration.label.interaction_rule_set'),
                        }),
                    })),
                );
            }
            return this.deleteCreatorDocumentValue(
                this.client.deleteInteractionRuleSet,
                {
                    interaction_rule_set_id: documentId,
                    expected_revision: document.expected_revision,
                },
                (current) => ({
                    ...current,
                    editable_interaction_rule_sets: current.editable_interaction_rule_sets.filter(
                        (candidate) => candidate.value.id !== documentId,
                    ),
                }),
                t('orchestration.label.interaction_rule_set'),
                documentId,
            );
        }
        const document = state.editable_content_modules.find(
            (candidate) => candidate.value.id === documentId,
        );
        if (document === undefined) return Promise.resolve(false);
        if (document.expected_revision === null) {
            return Promise.resolve(
                this.updateForContext(state.context_key, (current) => ({
                    ...current,
                    editable_content_modules: current.editable_content_modules.filter(
                        (candidate) => candidate.value.id !== documentId,
                    ),
                    announcement: t('orchestration.notice.draft_discarded', {
                        id: documentId,
                        label: t('orchestration.label.content_module'),
                    }),
                })),
            );
        }
        return this.deleteCreatorDocumentValue(
            this.client.deleteContentModule,
            {
                content_module_id: documentId,
                expected_revision: document.expected_revision,
            },
            (current) => ({
                ...current,
                editable_content_modules: current.editable_content_modules.filter(
                    (candidate) => candidate.value.id !== documentId,
                ),
            }),
            t('orchestration.label.content_module'),
            documentId,
        );
    }

    async movePromptBlock(blockId: string, direction: -1 | 1): Promise<boolean> {
        const state = get(this.mutable);
        const currentIndex = state.workspace.prompt_blocks.findIndex(
            (block) => block.id === blockId,
        );
        if (currentIndex < 0) return false;
        const current = state.workspace.prompt_blocks[currentIndex];
        if (!current?.order_editable) return false;
        const zoneIndexes = state.workspace.prompt_blocks.flatMap((block, index) =>
            block.placement_zone === current.placement_zone ? [index] : [],
        );
        const positionInZone = zoneIndexes.indexOf(currentIndex);
        const nextIndex = zoneIndexes[positionInZone + direction];
        if (nextIndex === undefined) return false;
        const reordered = [...state.workspace.prompt_blocks];
        const target = reordered[nextIndex];
        if (!target?.order_editable) return false;
        reordered[currentIndex] = target;
        reordered[nextIndex] = current;
        return this.persistPromptOrder(reordered, current.name);
    }

    async movePromptBlockTo(blockId: string, targetId: string): Promise<boolean> {
        const state = get(this.mutable);
        const reordered = moveBlockByDrop(state.workspace.prompt_blocks, blockId, targetId);
        if (reordered === state.workspace.prompt_blocks) return false;
        const moved = reordered.find((block) => block.id === blockId);
        return this.persistPromptOrder(reordered, moved?.name ?? t('orchestration.label.prompt'));
    }

    private async persistPromptOrder(
        reordered: PromptBlockDto[],
        movedName: string,
    ): Promise<boolean> {
        const state = get(this.mutable);
        const contextKey = state.context_key;
        const presetId = state.workspace.room_config.prompt_preset_id;
        const expectedRevision = state.workspace.prompt_preset_revision;
        const persist = this.client.reorderPromptBlocks;
        if (presetId === null || expectedRevision === null || persist === undefined) {
            this.updateForContext(contextKey, (current) => ({
                ...current,
                error: t('orchestration.error.unsupported_block_order'),
                announcement: t('orchestration.notice.order_failed'),
            }));
            return false;
        }
        try {
            const saved = await persist.call(this.client, {
                prompt_preset_id: presetId,
                ordered_block_ids: reordered.map((block) => block.id),
                expected_revision: expectedRevision,
            });
            if (!this.invalidatePlanPreviewForContext(contextKey)) return false;
            return this.updateForContext(contextKey, (current) => ({
                ...current,
                announcement: t('orchestration.notice.order_saved', { name: movedName }),
                error: null,
                workspace: {
                    ...current.workspace,
                    prompt_blocks: saved.blocks.slice(0, MAX_VISIBLE_PROMPT_BLOCKS),
                    prompt_preset_revision: saved.revision,
                    plan_preview: null,
                },
            }));
        } catch (error: unknown) {
            this.updateForContext(contextKey, (current) => ({
                ...current,
                error: errorLabel(error),
            }));
            await this.reloadContextIfCurrent(contextKey);
            return false;
        }
    }

    async simulateKnowledge(sampleText: string): Promise<boolean> {
        const state = get(this.mutable);
        const contextKey = state.context_key;
        const simulate = this.client.simulateKnowledge;
        const knowledgeBookId = state.workspace.knowledge_book_ids[0] ?? null;
        if (simulate === undefined || knowledgeBookId === null || sampleText.trim() === '') {
            if (simulate === undefined) {
                this.updateForContext(contextKey, (current) => ({
                    ...current,
                    error: t('orchestration.error.unsupported_knowledge_sim'),
                }));
            } else if (knowledgeBookId === null) {
                this.updateForContext(contextKey, (current) => ({
                    ...current,
                    error: t('orchestration.error.no_knowledge_book'),
                }));
            }
            return false;
        }
        try {
            const simulation = await simulate.call(this.client, {
                knowledge_book_id: knowledgeBookId,
                sample_text: sampleText,
                variables: structuredClone(state.workspace.room_config.variable_overrides),
            });
            return this.updateForContext(contextKey, (current) => ({
                ...current,
                knowledge_simulation: {
                    ...simulation,
                    entries: simulation.entries.slice(0, MAX_VISIBLE_SELECTION_EVIDENCE),
                    truncated:
                        simulation.truncated ||
                        simulation.entries.length > MAX_VISIBLE_SELECTION_EVIDENCE,
                },
                error: null,
            }));
        } catch (error: unknown) {
            this.updateForContext(contextKey, (current) => ({
                ...current,
                error: errorLabel(error),
            }));
            return false;
        }
    }

    async previewTransform(ruleId: string, sampleText: string): Promise<boolean> {
        const snapshot = get(this.mutable);
        const contextKey = snapshot.context_key;
        const preview = this.client.previewTransform;
        const matchingTransformSets = snapshot.editable_transform_sets.filter((document) =>
            document.value.rules.some((rule) => rule.id === ruleId),
        );
        if (preview === undefined || ruleId === '' || sampleText === '') {
            if (preview === undefined) {
                this.updateForContext(contextKey, (state) => ({
                    ...state,
                    error: t('orchestration.error.unsupported_transform_preview'),
                }));
            }
            return false;
        }
        if (matchingTransformSets.length !== 1) {
            this.updateForContext(contextKey, (state) => ({
                ...state,
                error:
                    matchingTransformSets.length === 0
                        ? t('orchestration.error.rule_not_found')
                        : t('orchestration.error.rule_ambiguous'),
            }));
            return false;
        }
        const transformSet = matchingTransformSets[0];
        if (transformSet === undefined) return false;
        try {
            const result = await preview.call(this.client, {
                transform_set_id: transformSet.value.id,
                rule_id: ruleId,
                sample_text: sampleText,
                variables: structuredClone(snapshot.workspace.room_config.variable_overrides),
            });
            if (
                result.transform_set_id !== transformSet.value.id ||
                result.rule_id !== ruleId ||
                result.reports.some((report) => report.trace.rule_id !== ruleId)
            ) {
                throw new Error(
                    'Core transform preview authority did not match the requested rule.',
                );
            }
            return this.updateForContext(contextKey, (state) => ({
                ...state,
                transform_preview: result,
                error: null,
            }));
        } catch (error: unknown) {
            this.updateForContext(contextKey, (state) => ({
                ...state,
                error: errorLabel(error),
            }));
            return false;
        }
    }

    async updateMemoryRecord(recordId: string, patch: MemoryRecordPatchInput): Promise<boolean> {
        const snapshot = get(this.mutable);
        const contextKey = snapshot.context_key;
        const record = snapshot.workspace.memory_records.find(
            (candidate) => candidate.id === recordId,
        );
        const update = this.client.patchMemoryRecord;
        if (record === undefined) return false;
        if (update === undefined) {
            this.updateForContext(contextKey, (state) => ({
                ...state,
                error: t('orchestration.error.unsupported_memory_edit'),
            }));
            return false;
        }
        try {
            const saved = await update.call(this.client, {
                conversation_id: record.conversation_id,
                branch_id: record.branch_id,
                memory_record_id: recordId,
                patch,
                expected_revision: record.revision,
            });
            return this.replaceMemoryRecord(
                contextKey,
                recordId,
                record.revision,
                saved,
                t('orchestration.notice.memory_updated'),
            );
        } catch (error: unknown) {
            this.updateForContext(contextKey, (state) => ({
                ...state,
                error: errorLabel(error),
                announcement: '',
            }));
            return false;
        }
    }

    async deleteMemoryRecord(recordId: string): Promise<boolean> {
        const snapshot = get(this.mutable);
        const contextKey = snapshot.context_key;
        const record = snapshot.workspace.memory_records.find(
            (candidate) => candidate.id === recordId,
        );
        const remove = this.client.deleteMemoryRecord;
        if (record === undefined) return false;
        if (remove === undefined) {
            this.updateForContext(contextKey, (state) => ({
                ...state,
                error: t('orchestration.error.unsupported_memory_delete'),
            }));
            return false;
        }
        try {
            await remove.call(this.client, {
                conversation_id: record.conversation_id,
                branch_id: record.branch_id,
                memory_record_id: recordId,
                expected_revision: record.revision,
            });
            if (!this.invalidatePlanPreviewForContext(contextKey)) return false;
            return this.updateForContext(contextKey, (state) => ({
                ...state,
                announcement: t('orchestration.notice.memory_deleted'),
                workspace: {
                    ...state.workspace,
                    memory_records: state.workspace.memory_records.filter(
                        (record) => record.id !== recordId,
                    ),
                    plan_preview: null,
                },
                error: null,
            }));
        } catch (error: unknown) {
            this.updateForContext(contextKey, (state) => ({
                ...state,
                error: errorLabel(error),
                announcement: '',
            }));
            return false;
        }
    }

    async setMemoryRecordPinned(recordId: string, pinned: boolean): Promise<boolean> {
        const snapshot = get(this.mutable);
        const contextKey = snapshot.context_key;
        const record = snapshot.workspace.memory_records.find(
            (candidate) => candidate.id === recordId,
        );
        const update = this.client.patchMemoryRecord;
        if (record === undefined) return false;
        if (update === undefined) {
            this.updateForContext(contextKey, (state) => ({
                ...state,
                error: t('orchestration.error.unsupported_memory_pin'),
            }));
            return false;
        }
        try {
            const saved = await update.call(this.client, {
                conversation_id: record.conversation_id,
                branch_id: record.branch_id,
                memory_record_id: recordId,
                patch: { pinned },
                expected_revision: record.revision,
            });
            return this.replaceMemoryRecord(
                contextKey,
                recordId,
                record.revision,
                saved,
                t('orchestration.notice.memory_pinned'),
            );
        } catch (error: unknown) {
            this.updateForContext(contextKey, (state) => ({
                ...state,
                error: errorLabel(error),
                announcement: '',
            }));
            return false;
        }
    }

    async setMemoryRecordExclusion(
        recordId: string,
        scope: MemoryRecordExclusionScope,
        excluded: boolean,
    ): Promise<boolean> {
        const snapshot = get(this.mutable);
        const contextKey = snapshot.context_key;
        const record = snapshot.workspace.memory_records.find(
            (candidate) => candidate.id === recordId,
        );
        const update = this.client.setMemoryRecordExclusion;
        if (record === undefined) return false;
        if (update === undefined) {
            this.updateForContext(contextKey, (state) => ({
                ...state,
                error: t('orchestration.error.unsupported_memory_exclusion'),
            }));
            return false;
        }
        try {
            const saved = await update.call(this.client, {
                conversation_id: record.conversation_id,
                branch_id: record.branch_id,
                memory_record_id: recordId,
                scope,
                excluded,
                expected_revision: record.revision,
            });
            const label =
                scope === 'conversation'
                    ? t('orchestration.label.conversation_scope')
                    : t('orchestration.label.character_scope');
            return this.replaceMemoryRecord(
                contextKey,
                recordId,
                record.revision,
                saved,
                t('orchestration.notice.exclusion_changed', { scope: label }),
            );
        } catch (error: unknown) {
            this.updateForContext(contextKey, (state) => ({
                ...state,
                error: errorLabel(error),
                announcement: '',
            }));
            return false;
        }
    }

    private replaceMemoryRecord(
        contextKey: string,
        requestedRecordId: string,
        expectedRevision: number,
        saved: OrchestrationWorkspaceDto['memory_records'][number],
        announcement: string,
    ): boolean {
        let accepted = false;
        if (!this.invalidatePlanPreviewForContext(contextKey)) return false;
        const contextApplied = this.updateForContext(contextKey, (state) => {
            const currentRecord = state.workspace.memory_records.find(
                (record) => record.id === requestedRecordId,
            );
            if (
                currentRecord === undefined ||
                saved.id !== requestedRecordId ||
                !Number.isSafeInteger(saved.revision) ||
                saved.revision <= expectedRevision ||
                saved.revision <= currentRecord.revision
            ) {
                return {
                    ...state,
                    error: MEMORY_RECORD_RESPONSE_AUTHORITY_ERROR,
                    announcement: '',
                };
            }
            accepted = true;
            return {
                ...state,
                announcement,
                workspace: {
                    ...state.workspace,
                    memory_records: state.workspace.memory_records.map((record) =>
                        record.id === requestedRecordId ? saved : record,
                    ),
                    plan_preview: null,
                },
                error: null,
            };
        });
        return contextApplied && accepted;
    }

    async resolvePlanPreview(userText: string): Promise<PromptPlanPreviewDto | null> {
        return this.resolvePlanPreviewOperation(userText, false, null);
    }

    async resolveNewPlanPreview(userText: string): Promise<PromptPlanPreviewDto | null> {
        return this.resolvePlanPreviewOperation(userText, true, null);
    }

    async resumePlanPreview(
        generationAttemptId: string,
        userText: string,
    ): Promise<PromptPlanPreviewDto | null> {
        if (!isValidGenerationAttemptId(generationAttemptId)) {
            const contextKey = get(this.mutable).context_key;
            this.updateForContext(contextKey, (state) => ({
                ...state,
                error: t('orchestration.error.invalid_attempt_id'),
            }));
            return null;
        }
        return this.resolvePlanPreviewOperation(userText, false, generationAttemptId);
    }

    private async resolvePlanPreviewOperation(
        userText: string,
        rotateOperationNonce: boolean,
        requestedResumeAttemptId: string | null,
    ): Promise<PromptPlanPreviewDto | null> {
        const state = get(this.mutable);
        const contextKey = state.context_key;
        const resolve = this.client.resolvePromptPreview;
        const generationTarget = state.workspace.generation_target;
        if (
            resolve === undefined ||
            state.workspace.room_config.conversation_id === '' ||
            userText.trim() === '' ||
            generationTarget === null
        ) {
            if (resolve === undefined) {
                this.updateForContext(contextKey, (current) => ({
                    ...current,
                    error: t('orchestration.error.unsupported_plan_preview'),
                }));
            }
            return null;
        }

        const resumeAttemptId = rotateOperationNonce
            ? null
            : (requestedResumeAttemptId ?? state.plan_generation_attempt_id);
        const operationNonce =
            resumeAttemptId === null
                ? rotateOperationNonce || state.plan_operation_nonce === null
                    ? crypto.randomUUID()
                    : state.plan_operation_nonce
                : state.plan_operation_nonce;
        const request: PromptPlanRequestInput = {
            conversation_id: state.workspace.room_config.conversation_id,
            branch_id: state.workspace.room_config.branch_id,
            expected_head: state.workspace.expected_head,
            user_text: userText,
            generation_target: generationTarget,
            prompt_preset_id: state.workspace.room_config.prompt_preset_id,
            variable_overrides: structuredClone(state.workspace.room_config.variable_overrides),
            expected_plan_hash: rotateOperationNonce
                ? null
                : (state.workspace.plan_preview?.plan_hash ?? null),
            ...(resumeAttemptId === null
                ? { operation_nonce: operationNonce }
                : { generation_attempt_id: resumeAttemptId }),
        };
        const previewEpoch = ++this.planPreviewEpoch;
        if (
            !this.updateForContext(contextKey, (current) => ({
                ...current,
                plan_operation_nonce: operationNonce,
                plan_generation_attempt_id: resumeAttemptId,
                plan_preview_request: request,
                workspace: { ...current.workspace, plan_preview: null },
                error: null,
            }))
        ) {
            return null;
        }

        try {
            const preview = await resolve.call(this.client, request);
            const contextApplied = this.updateForContext(contextKey, (current) => {
                if (
                    this.planPreviewEpoch !== previewEpoch ||
                    (resumeAttemptId === null
                        ? current.plan_operation_nonce !== operationNonce ||
                          current.plan_generation_attempt_id !== null
                        : current.plan_generation_attempt_id !== resumeAttemptId)
                ) {
                    return current;
                }
                if (
                    !isValidGenerationAttemptId(preview.generation_attempt_id) ||
                    (resumeAttemptId !== null && preview.generation_attempt_id !== resumeAttemptId)
                ) {
                    return {
                        ...current,
                        error: isValidGenerationAttemptId(preview.generation_attempt_id)
                            ? t('orchestration.error.plan_attempt_mismatch')
                            : t('orchestration.error.invalid_attempt_id'),
                    };
                }
                return {
                    ...current,
                    workspace: { ...current.workspace, plan_preview: preview },
                    plan_generation_attempt_id: preview.generation_attempt_id,
                    plan_preview_request: request,
                    error: null,
                };
            });
            const current = get(this.mutable);
            return contextApplied &&
                this.planPreviewEpoch === previewEpoch &&
                current.plan_generation_attempt_id === preview.generation_attempt_id &&
                current.workspace.plan_preview === preview
                ? preview
                : null;
        } catch (error: unknown) {
            this.updateForContext(contextKey, (current) =>
                this.planPreviewEpoch === previewEpoch &&
                (resumeAttemptId === null
                    ? current.plan_operation_nonce === operationNonce &&
                      current.plan_generation_attempt_id === null
                    : current.plan_generation_attempt_id === resumeAttemptId)
                    ? { ...current, error: errorLabel(error) }
                    : current,
            );
            return null;
        }
    }

    clearPlanPreview(): void {
        ++this.planPreviewEpoch;
        this.update((state) => ({
            ...state,
            plan_preview_request: null,
            workspace:
                state.workspace.plan_preview === null
                    ? state.workspace
                    : { ...state.workspace, plan_preview: null },
        }));
    }

    completePlanOperation(): void {
        ++this.planPreviewEpoch;
        this.update((state) => ({
            ...state,
            plan_operation_nonce: null,
            plan_generation_attempt_id: null,
            plan_preview_request: null,
            workspace:
                state.workspace.plan_preview === null
                    ? state.workspace
                    : { ...state.workspace, plan_preview: null },
        }));
    }

    reviewedPromptSendInput(): ReviewedPromptSendInput | null {
        const state = get(this.mutable);
        const preview = state.workspace.plan_preview;
        const request = state.plan_preview_request;
        if (
            preview === null ||
            request === null ||
            state.plan_generation_attempt_id === null ||
            preview.generation_attempt_id !== state.plan_generation_attempt_id
        ) {
            return null;
        }
        return {
            conversation_id: request.conversation_id,
            branch_id: request.branch_id,
            expected_head: request.expected_head,
            user_text: request.user_text,
            generation_target: structuredClone(request.generation_target),
            prompt_preset_id: request.prompt_preset_id,
            variable_overrides: structuredClone(request.variable_overrides),
            expected_plan_hash: preview.plan_hash,
            generation_attempt_id: preview.generation_attempt_id,
        };
    }

    async decideProposal(proposalId: string, approved: boolean): Promise<boolean> {
        const snapshot = get(this.mutable);
        const contextKey = snapshot.context_key;
        const target = snapshot.workspace.interaction_proposals.find(
            (candidate) => candidate.proposal.id === proposalId,
        );
        if (
            target === undefined ||
            snapshot.phase !== 'ready' ||
            snapshot.busy_interaction_proposal_id !== null ||
            target.conversation_id !== snapshot.workspace.room_config.conversation_id ||
            target.branch_id !== snapshot.workspace.room_config.branch_id ||
            target.proposal.status !== 'pending'
        ) {
            return false;
        }
        if (approved && target.proposal.projection_rejection_reason === 'unsafe_native_text') {
            this.updateForContext(contextKey, (state) => ({
                ...state,
                announcement: t('interaction.error.unreviewable'),
            }));
            return false;
        }
        this.updateForContext(contextKey, (state) => ({
            ...state,
            busy_interaction_proposal_id: proposalId,
            error: null,
            announcement: approved
                ? t('interaction.notice.approving')
                : t('interaction.notice.rejecting'),
        }));
        try {
            const receipt = await this.client.decideInteractionProposal({
                conversation_id: target.conversation_id,
                branch_id: target.branch_id,
                proposal_record_id: proposalId,
                expected_state_revision: target.state_revision,
                expected_proposal_revision: target.proposal_revision,
                decision: approved ? 'approve' : 'reject',
            });
            const decidedAt = receipt.proposal.decided_at_epoch_seconds;
            if (
                receipt.proposal.id !== proposalId ||
                receipt.proposal.status !== (approved ? 'approved' : 'rejected') ||
                receipt.proposal.title !== target.proposal.title ||
                receipt.proposal.body !== target.proposal.body ||
                receipt.proposal.projection_rejection_reason !==
                    target.proposal.projection_rejection_reason ||
                receipt.proposal.source_interaction_state_revision !==
                    target.proposal.source_interaction_state_revision ||
                receipt.proposal.requested_at_epoch_seconds !==
                    target.proposal.requested_at_epoch_seconds ||
                receipt.proposal.expires_at_epoch_seconds !==
                    target.proposal.expires_at_epoch_seconds ||
                decidedAt === null ||
                !Number.isSafeInteger(decidedAt) ||
                decidedAt < target.proposal.requested_at_epoch_seconds ||
                !Number.isSafeInteger(receipt.state_revision) ||
                receipt.state_revision < target.state_revision
            ) {
                throw new Error('Core interaction proposal receipt did not match the decision.');
            }
            return this.updateForContext(contextKey, (state) => ({
                ...state,
                busy_interaction_proposal_id: null,
                announcement: approved
                    ? t('interaction.notice.approved')
                    : t('interaction.notice.rejected'),
                workspace: {
                    ...state.workspace,
                    interaction_state_revision: receipt.state_revision,
                    interaction_proposals: state.workspace.interaction_proposals.filter(
                        (proposal) => proposal.proposal.id !== proposalId,
                    ),
                },
                error: null,
            }));
        } catch (error: unknown) {
            this.updateForContext(contextKey, (state) => ({
                ...state,
                busy_interaction_proposal_id: null,
                error: errorLabel(error),
                announcement: t('orchestration.notice.decision_failed'),
            }));
            return false;
        }
    }

    private async reloadContextIfCurrent(contextKey: string): Promise<void> {
        if (!this.isCurrentContext(contextKey)) return;
        const { conversation_id: conversationId, branch_id: branchId } = get(this.mutable).workspace
            .room_config;
        if (conversationId !== '' && branchId !== '') {
            await this.loadContext(conversationId, branchId);
        }
    }

    destroy(): void {
        ++this.contextEpoch;
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
