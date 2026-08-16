import { get } from 'svelte/store';
import { afterEach, describe, expect, it, vi } from 'vitest';

import type {
    CreatorPromptPresetDocumentDto,
    InteractionProposalListItemDto,
    LorepiaClient,
    OrchestrationWorkspaceDto,
    PromptBlockDto,
    ReorderPromptBlocksInput,
    SaveRoomOrchestrationConfigInput,
    SaveRoomOrchestrationConfigResult,
    TaskProfileDocumentDto,
    UpsertPromptPresetInput,
    UpsertTaskProfileInput,
} from '../../lib/ipc/contracts';
import { LiveLorepiaClient, type LorepiaTransport } from '../../lib/ipc/client';
import { LorepiaClientError } from '../../lib/ipc/errors';
import {
    MAX_VISIBLE_MEMORY_RECORDS,
    MAX_VISIBLE_PROMPT_BLOCKS,
    OrchestrationController,
    emptyOrchestrationWorkspace,
    moveBlockByDrop,
    type OrchestrationCapableClient,
} from './orchestration-controller';

afterEach(() => {
    vi.restoreAllMocks();
});

function workspace(): OrchestrationWorkspaceDto {
    const value = emptyOrchestrationWorkspace('conversation-1', 'branch-1');
    value.prompt_presets = [
        {
            id: 'prompt-1',
            name: '기본 프롬프트',
            schema_version: 1,
            block_count: 2,
            default_generation_preset_id: null,
        },
    ];
    value.room_config.prompt_preset_id = 'prompt-1';
    value.room_config_revision = 4;
    value.prompt_preset_revision = 9;
    value.prompt_blocks = Array.from({ length: MAX_VISIBLE_PROMPT_BLOCKS + 2 }, (_, index) => ({
        id: `block-${String(index)}`,
        name: `블록 ${String(index)}`,
        kind: 'static_instruction',
        enabled: true,
        order_editable: true,
        role_hint: 'system' as const,
        placement_zone: 'instructions',
        template_preview: '합성 템플릿',
        condition_summary: null,
        source_label: 'project-owned synthetic',
        provenance_label: 'local',
        priority: index,
        minimum_tokens: null,
        maximum_tokens: 100,
        overflow_policy: 'trim_tail' as const,
        cache_boundary_after: false,
    }));
    value.memory_records = Array.from({ length: MAX_VISIBLE_MEMORY_RECORDS + 2 }, (_, index) => ({
        id: `memory-${String(index)}`,
        conversation_id: 'conversation-1',
        branch_id: 'branch-1',
        kind: 'episodic_event',
        title: `기억 ${String(index)}`,
        summary: '합성 요약',
        importance: 5,
        keywords: ['합성'],
        pinned: false,
        excluded_from_conversation: false,
        excluded_from_character: false,
        source_navigation: {
            conversation_id: 'conversation-1',
            branch_id: 'branch-1',
            start_message_id: `message-${String(index)}`,
            end_message_id: `message-${String(index + 1)}`,
        },
        invalidated_at: null,
        updated_at: '2026-08-03T00:00:00Z',
        revision: index + 1,
    }));
    return value;
}

function editablePromptPreset(): CreatorPromptPresetDocumentDto {
    return {
        id: 'prompt-1',
        name: '편집 프롬프트',
        schema_version: 1,
        blocks: [
            {
                id: 'block-1',
                name: '편집 블록',
                kind: 'static_instruction',
                enabled: true,
                role_hint: 'system',
                authority: 'creator',
                template: {
                    parts: [{ kind: 'text', value: 'project-owned synthetic' }],
                    max_output_chars: 4096,
                },
                condition: null,
                source: { kind: 'template' },
                placement_zone: 'preset_instruction',
                history_selector: null,
                token_policy: {
                    priority: 10,
                    min_tokens: null,
                    max_tokens: 200,
                    reserve_tokens: null,
                },
                overflow_policy: 'trim_tail',
                merge_policy: 'separate_message',
                provenance: {
                    source_kind: 'user_created',
                    source_id: null,
                    source_hash: null,
                    author: null,
                    license: null,
                    imported_at: null,
                },
            },
        ],
        controls: [],
        default_values: { values: [] },
        default_generation_preset_id: null,
        memory_profile_id: null,
        knowledge_book_ids: [],
        transform_set_ids: [],
        module_ids: [],
        cache_boundaries: [],
        metadata: {
            description: '합성 편집 프롬프트',
            tags: [],
            provenance: {
                source_kind: 'user_created',
                source_id: null,
                source_hash: null,
                author: null,
                license: null,
                imported_at: null,
            },
            created_at: '2026-08-03T00:00:00Z',
            updated_at: '2026-08-03T00:00:00Z',
            local_override_of: null,
        },
    };
}

function taskProfileDocument(): TaskProfileDocumentDto {
    return {
        id: 'task-1',
        kind: 'memory_summary',
        route_id: 'route-1',
        generation_preset_id: 'generation-1',
        fallback_route_ids: [],
        embedding_dimensions: null,
        timeout_ms: 30_000,
        rate_limit: { requests: 2, per_seconds: 60 },
        concurrency_limit: 1,
    };
}

function capableClient(
    overrides: Partial<OrchestrationCapableClient> = {},
): OrchestrationCapableClient {
    return {
        getOrchestrationWorkspace: vi.fn().mockResolvedValue(workspace()),
        saveRoomOrchestrationConfig: vi.fn((input: SaveRoomOrchestrationConfigInput) =>
            Promise.resolve({
                room_config: {
                    ...workspace().room_config,
                    prompt_preset_id: input.prompt_preset_id,
                    generation_preset_id: input.generation_preset_id,
                    creator_values: input.creator_values,
                    variable_overrides: input.variable_overrides,
                    response_length: input.response_length,
                    creativity: input.creativity,
                    reasoning_effort: input.reasoning_effort,
                    memory_enabled: input.memory_enabled,
                    knowledge_enabled: input.knowledge_enabled,
                },
                revision: 5,
                generation_target: null,
            }),
        ),
        reorderPromptBlocks: vi.fn((input: ReorderPromptBlocksInput) => {
            const blocks: PromptBlockDto[] = workspace().prompt_blocks;
            return Promise.resolve({
                blocks: input.ordered_block_ids.flatMap((id) => {
                    const block = blocks.find((candidate) => candidate.id === id);
                    return block ? [block] : [];
                }),
                revision: input.expected_revision + 1,
            });
        }),
        ...overrides,
    } as unknown as OrchestrationCapableClient;
}

function deferred<Value>(): {
    promise: Promise<Value>;
    resolve: (value: Value) => void;
} {
    let resolvePromise!: (value: Value) => void;
    const promise = new Promise<Value>((resolve) => {
        resolvePromise = resolve;
    });
    return { promise, resolve: resolvePromise };
}

describe('OrchestrationController', () => {
    it('reaches ready and loads editable documents with the production live client shape', async () => {
        const commands: string[] = [];
        const transport: LorepiaTransport = {
            invoke: (commandName) => {
                commands.push(commandName);
                if (commandName === 'get_orchestration_workspace') {
                    return Promise.resolve(workspace());
                }
                if (commandName === 'get_editable_prompt_preset') {
                    return Promise.resolve({
                        value: editablePromptPreset(),
                        revision: 9,
                        created_at: '2026-08-03T00:00:00Z',
                        updated_at: '2026-08-03T00:00:00Z',
                        deleted_at: null,
                    });
                }
                if (commandName === 'list_task_profiles') {
                    return Promise.resolve([
                        {
                            value: taskProfileDocument(),
                            revision: 3,
                            created_at: '2026-08-03T00:00:00Z',
                            updated_at: '2026-08-03T00:00:00Z',
                            deleted_at: null,
                        },
                    ]);
                }
                if (
                    commandName === 'list_memory_profiles' ||
                    commandName === 'list_knowledge_books' ||
                    commandName === 'list_transform_sets' ||
                    commandName === 'list_interaction_rule_sets' ||
                    commandName === 'list_content_modules' ||
                    commandName === 'list_interaction_proposals'
                ) {
                    return Promise.resolve([]);
                }
                if (commandName === 'expire_interaction_proposals') {
                    return Promise.resolve({
                        conversation_id: 'conversation-1',
                        branch_id: 'branch-1',
                        current_state_revision: 0,
                        expired_proposals: [],
                        has_more_expired: false,
                    });
                }
                return Promise.reject(new Error(`unexpected command: ${commandName}`));
            },
            createChatChannel: () => ({}),
            listen: () => Promise.resolve(() => undefined),
        };
        const controller = new OrchestrationController(new LiveLorepiaClient(transport));

        await controller.loadContext('conversation-1', 'branch-1');

        expect(get(controller.state)).toMatchObject({
            phase: 'ready',
            editable_prompt_preset: { value: { id: 'prompt-1' }, revision: 9 },
            editable_task_profiles: [{ value: { id: 'task-1' }, expected_revision: 3 }],
            editable_creator_documents_loading: false,
            editable_creator_documents_error: null,
        });
        expect(commands).toContain('get_orchestration_workspace');
        expect(commands).toContain('get_editable_prompt_preset');
        expect(commands).toContain('list_task_profiles');
        expect(commands).toContain('list_content_modules');
    });

    it('reports an unavailable Core boundary without manufacturing success', async () => {
        const controller = new OrchestrationController({} as LorepiaClient);

        await controller.loadContext('conversation-1', 'branch-1');
        expect(get(controller.state)).toMatchObject({
            phase: 'unavailable',
            dirty_room_config: false,
            error: '현재 Core가 프롬프트 오케스트레이션 API를 제공하지 않습니다.',
        });

        controller.stageRoomConfig({ creativity: 999 });
        expect(get(controller.state).workspace.room_config.creativity).toBe(50);
        expect(get(controller.state).dirty_room_config).toBe(false);
        await expect(controller.saveRoomConfig()).resolves.toBe(false);
        expect(get(controller.state).error).toContain('저장할 수 없습니다');
    });

    it('bounds large Core collections and saves an explicit room draft', async () => {
        const client = capableClient();
        const controller = new OrchestrationController(client);

        await controller.loadContext('conversation-1', 'branch-1');
        let state = get(controller.state);
        expect(state.phase).toBe('ready');
        expect(state.workspace.prompt_blocks).toHaveLength(MAX_VISIBLE_PROMPT_BLOCKS);
        expect(state.workspace.memory_records).toHaveLength(MAX_VISIBLE_MEMORY_RECORDS);
        expect(state.list_truncation.prompt_blocks).toBe(true);
        expect(state.list_truncation.memory_records).toBe(true);

        controller.stageRoomConfig({
            generation_preset_id: 'generation-1',
        });
        controller.stageCreatorControl('tone', 'warm');
        expect(get(controller.state).dirty_room_config).toBe(true);

        await expect(controller.saveRoomConfig()).resolves.toBe(true);
        state = get(controller.state);
        expect(state.dirty_room_config).toBe(false);
        expect(state.workspace.room_config.creator_values).toEqual({ tone: 'warm' });
        expect(client.saveRoomOrchestrationConfig).toHaveBeenCalledWith(
            expect.objectContaining({
                expected_revision: 4,
                generation_preset_id: 'generation-1',
                creator_values: { tone: 'warm' },
            }),
        );
        const save = client.saveRoomOrchestrationConfig;
        if (save === undefined) throw new Error('save API missing from capable test client');
        const savedInput = vi.mocked(save).mock.calls[0]?.[0];
        expect(savedInput).toBeDefined();
        expect(savedInput).toMatchObject({
            response_length: 'balanced',
            creativity: 50,
            reasoning_effort: 'provider_default',
            memory_enabled: true,
            knowledge_enabled: true,
        });
    });

    it('persists keyboard block reordering through stable block identifiers', async () => {
        const client = capableClient();
        const controller = new OrchestrationController(client);
        await controller.loadContext('conversation-1', 'branch-1');

        await expect(controller.movePromptBlock('block-1', -1)).resolves.toBe(true);
        expect(
            get(controller.state)
                .workspace.prompt_blocks.slice(0, 2)
                .map(({ id }) => id),
        ).toEqual(['block-1', 'block-0']);
        const reorder = client.reorderPromptBlocks;
        if (reorder === undefined) throw new Error('reorder API missing from capable test client');
        const reorderInput = vi.mocked(reorder).mock.calls[0]?.[0];
        expect(reorderInput).toMatchObject({
            prompt_preset_id: 'prompt-1',
            expected_revision: 9,
        });
        expect(reorderInput?.ordered_block_ids).toEqual(
            expect.arrayContaining(['block-0', 'block-1']),
        );
    });

    it('does not apply a completed mutation to a newly selected room', async () => {
        const pendingSave = deferred<SaveRoomOrchestrationConfigResult>();
        const client = capableClient({
            getOrchestrationWorkspace: vi.fn((conversationId: string, branchId: string) => {
                const result = workspace();
                result.room_config.conversation_id = conversationId;
                result.room_config.branch_id = branchId;
                return Promise.resolve(result);
            }),
            saveRoomOrchestrationConfig: vi.fn(() => pendingSave.promise),
        });
        const controller = new OrchestrationController(client);
        await controller.loadContext('conversation-1', 'branch-1');
        controller.stageRoomConfig({ generation_preset_id: 'generation-1' });
        const save = controller.saveRoomConfig();

        await controller.loadContext('conversation-2', 'branch-2');
        pendingSave.resolve({
            room_config: {
                ...workspace().room_config,
                conversation_id: 'conversation-1',
                branch_id: 'branch-1',
                generation_preset_id: 'generation-1',
            },
            revision: 5,
            generation_target: null,
        });
        await expect(save).resolves.toBe(false);

        expect(get(controller.state).workspace.room_config).toMatchObject({
            conversation_id: 'conversation-2',
            branch_id: 'branch-2',
            generation_preset_id: null,
        });
    });

    it('preserves a newer same-room draft while an older save is in flight', async () => {
        const pendingSave = deferred<SaveRoomOrchestrationConfigResult>();
        const client = capableClient({
            saveRoomOrchestrationConfig: vi.fn(() => pendingSave.promise),
        });
        const controller = new OrchestrationController(client);
        await controller.loadContext('conversation-1', 'branch-1');
        controller.stageRoomConfig({ generation_preset_id: 'generation-1' });
        const save = controller.saveRoomConfig();
        controller.stageRoomConfig({ generation_preset_id: 'generation-2' });

        pendingSave.resolve({
            room_config: {
                ...workspace().room_config,
                generation_preset_id: 'generation-1',
            },
            revision: 5,
            generation_target: null,
        });
        await expect(save).resolves.toBe(true);
        expect(get(controller.state)).toMatchObject({
            saving: false,
            dirty_room_config: true,
            announcement:
                '저장 중 추가로 바꾼 값이 있습니다. 새 변경 사항은 아직 저장되지 않았습니다.',
        });
        expect(get(controller.state).workspace.room_config.generation_preset_id).toBe(
            'generation-2',
        );
    });

    it('does not optimistically reorder when Core cannot persist the change', async () => {
        const client = capableClient({ reorderPromptBlocks: undefined });
        const controller = new OrchestrationController(client);
        await controller.loadContext('conversation-1', 'branch-1');
        const before = get(controller.state).workspace.prompt_blocks.map(({ id }) => id);

        await expect(controller.movePromptBlock('block-1', -1)).resolves.toBe(false);
        expect(get(controller.state).workspace.prompt_blocks.map(({ id }) => id)).toEqual(before);
        expect(get(controller.state).error).toContain('순서 저장 API');
    });

    it('binds prompt preview to the reviewed branch head, target, variables, and plan hash', async () => {
        const operationNonce = '00000000-0000-4000-8000-000000000041';
        vi.spyOn(globalThis.crypto, 'randomUUID').mockReturnValue(operationNonce);
        const resolvePromptPreview = vi.fn().mockResolvedValue({
            generation_attempt_id: 'attempt-1',
            plan_hash: 'sha256:new-plan',
        });
        const loaded = workspace();
        loaded.expected_head = 'message-head-1';
        loaded.generation_target = {
            model_route_id: 'route-room-b',
            generation_preset_id: 'generation-room-b',
        };
        loaded.room_config.variable_overrides = {
            values: [
                {
                    variable: { scope: 'conversation', namespace: null, id: 'tone' },
                    value: { type: 'text', value: 'warm' },
                },
            ],
        };
        const client = capableClient({
            getOrchestrationWorkspace: vi.fn().mockResolvedValue(loaded),
            resolvePromptPreview,
        });
        const controller = new OrchestrationController(client);
        await controller.loadContext('conversation-1', 'branch-1');

        await controller.resolvePlanPreview('다음 합성 메시지');

        expect(resolvePromptPreview).toHaveBeenCalledWith({
            conversation_id: 'conversation-1',
            branch_id: 'branch-1',
            expected_head: 'message-head-1',
            user_text: '다음 합성 메시지',
            generation_target: {
                model_route_id: 'route-room-b',
                generation_preset_id: 'generation-room-b',
            },
            prompt_preset_id: 'prompt-1',
            variable_overrides: loaded.room_config.variable_overrides,
            expected_plan_hash: null,
            operation_nonce: operationNonce,
        });
        expect(get(controller.state).plan_operation_nonce).toBe(operationNonce);
        expect(get(controller.state).plan_generation_attempt_id).toBe('attempt-1');
        expect(controller.reviewedPromptSendInput()).toEqual({
            conversation_id: 'conversation-1',
            branch_id: 'branch-1',
            expected_head: 'message-head-1',
            user_text: '다음 합성 메시지',
            generation_target: {
                model_route_id: 'route-room-b',
                generation_preset_id: 'generation-room-b',
            },
            prompt_preset_id: 'prompt-1',
            variable_overrides: loaded.room_config.variable_overrides,
            expected_plan_hash: 'sha256:new-plan',
            generation_attempt_id: 'attempt-1',
        });
        controller.clearPlanPreview();
        expect(controller.reviewedPromptSendInput()).toBeNull();
        expect(get(controller.state).plan_operation_nonce).toBe(operationNonce);
        expect(get(controller.state).plan_generation_attempt_id).toBe('attempt-1');
    });

    it('retains a caller nonce across loss and retry, rotates explicitly, and rejects a stale response', async () => {
        const operationNonceA = '00000000-0000-4000-8000-000000000051';
        const operationNonceB = '00000000-0000-4000-8000-000000000052';
        const operationNonceC = '00000000-0000-4000-8000-000000000053';
        const operationNonceD = '00000000-0000-4000-8000-000000000054';
        const randomUUID = vi
            .spyOn(globalThis.crypto, 'randomUUID')
            .mockReturnValueOnce(operationNonceA)
            .mockReturnValueOnce(operationNonceB)
            .mockReturnValueOnce(operationNonceC)
            .mockReturnValueOnce(operationNonceD);
        const first = deferred<{ generation_attempt_id: string; plan_hash: string }>();
        const second = deferred<{ generation_attempt_id: string; plan_hash: string }>();
        const resolvePromptPreview = vi
            .fn()
            .mockReturnValueOnce(first.promise)
            .mockReturnValueOnce(second.promise)
            .mockRejectedValueOnce(new Error('synthetic response loss'))
            .mockResolvedValueOnce({
                generation_attempt_id: 'attempt-c',
                plan_hash: 'sha256:plan-c',
            })
            .mockResolvedValueOnce({
                generation_attempt_id: 'attempt-d',
                plan_hash: 'sha256:plan-d',
            });
        const loaded = workspace();
        loaded.generation_target = {
            model_route_id: 'route-room-b',
            generation_preset_id: 'generation-room-b',
        };
        const controller = new OrchestrationController(
            capableClient({
                getOrchestrationWorkspace: vi.fn().mockResolvedValue(loaded),
                resolvePromptPreview,
            }),
        );
        await controller.loadContext('conversation-1', 'branch-1');

        const requestA = controller.resolvePlanPreview('작업 A');
        expect(get(controller.state)).toMatchObject({
            plan_operation_nonce: operationNonceA,
            plan_preview_request: {
                user_text: '작업 A',
                operation_nonce: operationNonceA,
            },
        });

        const requestB = controller.resolveNewPlanPreview('작업 B');
        expect(get(controller.state)).toMatchObject({
            plan_operation_nonce: operationNonceB,
            plan_preview_request: {
                user_text: '작업 B',
                expected_plan_hash: null,
                operation_nonce: operationNonceB,
            },
        });
        first.resolve({ generation_attempt_id: 'attempt-stale', plan_hash: 'sha256:stale' });
        await expect(requestA).resolves.toBeNull();
        expect(get(controller.state).workspace.plan_preview).toBeNull();

        second.resolve({ generation_attempt_id: 'attempt-b', plan_hash: 'sha256:plan-b' });
        await expect(requestB).resolves.toMatchObject({ generation_attempt_id: 'attempt-b' });
        expect(controller.reviewedPromptSendInput()).toMatchObject({
            user_text: '작업 B',
            expected_plan_hash: 'sha256:plan-b',
            generation_attempt_id: 'attempt-b',
        });
        expect(controller.reviewedPromptSendInput()).not.toHaveProperty('operation_nonce');

        controller.clearPlanPreview();
        await expect(controller.resolvePlanPreview('작업 B')).resolves.toBeNull();
        expect(get(controller.state)).toMatchObject({
            plan_operation_nonce: operationNonceB,
            plan_generation_attempt_id: 'attempt-b',
            plan_preview_request: {
                user_text: '작업 B',
                generation_attempt_id: 'attempt-b',
            },
            error: '프롬프트 오케스트레이션 작업을 완료하지 못했습니다.',
        });
        expect(get(controller.state).plan_preview_request).not.toHaveProperty('operation_nonce');

        await expect(controller.resolveNewPlanPreview('작업 C')).resolves.toMatchObject({
            generation_attempt_id: 'attempt-c',
        });
        expect(get(controller.state).plan_operation_nonce).toBe(operationNonceC);
        controller.completePlanOperation();
        expect(get(controller.state)).toMatchObject({
            plan_operation_nonce: null,
            plan_generation_attempt_id: null,
            plan_preview_request: null,
            workspace: { plan_preview: null },
        });
        expect(randomUUID).toHaveBeenCalledTimes(3);

        await expect(controller.resolvePlanPreview('작업 D')).resolves.toMatchObject({
            generation_attempt_id: 'attempt-d',
        });
        expect(get(controller.state).plan_operation_nonce).toBe(operationNonceD);
        expect(randomUUID).toHaveBeenCalledTimes(4);
    });

    it('resumes an approval-sealed attempt after controller restart without minting a nonce', async () => {
        const operationNonce = '00000000-0000-4000-8000-000000000055';
        const randomUUID = vi
            .spyOn(globalThis.crypto, 'randomUUID')
            .mockReturnValue(operationNonce);
        const loaded = workspace();
        loaded.generation_target = {
            model_route_id: 'route-room-b',
            generation_preset_id: 'generation-room-b',
        };
        const firstResolve = vi.fn().mockResolvedValue({
            generation_attempt_id: 'attempt-restart-resume',
            plan_hash: 'sha256:first-plan',
        });
        const firstController = new OrchestrationController(
            capableClient({
                getOrchestrationWorkspace: vi.fn().mockResolvedValue(loaded),
                resolvePromptPreview: firstResolve,
            }),
        );
        await firstController.loadContext('conversation-1', 'branch-1');
        await firstController.resolvePlanPreview('재시작 전 요청');

        const resumedResolve = vi.fn().mockResolvedValue({
            generation_attempt_id: 'attempt-restart-resume',
            plan_hash: 'sha256:resumed-plan',
        });
        const restartedController = new OrchestrationController(
            capableClient({
                getOrchestrationWorkspace: vi.fn().mockResolvedValue(loaded),
                resolvePromptPreview: resumedResolve,
            }),
        );
        await restartedController.loadContext('conversation-1', 'branch-1');
        await restartedController.resumePlanPreview('attempt-restart-resume', '재시작 뒤 요청');

        expect(firstResolve).toHaveBeenCalledWith(
            expect.objectContaining({
                operation_nonce: operationNonce,
            }),
        );
        expect(firstResolve.mock.calls[0]?.[0]).not.toHaveProperty('generation_attempt_id');
        expect(resumedResolve).toHaveBeenCalledWith({
            conversation_id: 'conversation-1',
            branch_id: 'branch-1',
            expected_head: null,
            user_text: '재시작 뒤 요청',
            generation_target: {
                model_route_id: 'route-room-b',
                generation_preset_id: 'generation-room-b',
            },
            prompt_preset_id: 'prompt-1',
            variable_overrides: { values: [] },
            expected_plan_hash: null,
            generation_attempt_id: 'attempt-restart-resume',
        });
        expect(resumedResolve.mock.calls[0]?.[0]).not.toHaveProperty('operation_nonce');
        expect(randomUUID).toHaveBeenCalledOnce();
        expect(restartedController.reviewedPromptSendInput()).toMatchObject({
            generation_attempt_id: 'attempt-restart-resume',
            expected_plan_hash: 'sha256:resumed-plan',
        });
        expect(restartedController.reviewedPromptSendInput()).not.toHaveProperty('operation_nonce');
    });

    it('uses the exact memory revision for a granular patch', async () => {
        const initialRecord = workspace().memory_records[0];
        if (initialRecord === undefined) throw new Error('synthetic memory fixture is empty');
        const saved = {
            ...initialRecord,
            summary: '수정된 합성 요약',
            revision: 2,
        };
        const patchMemoryRecord = vi.fn().mockResolvedValue(saved);
        const client = capableClient({ patchMemoryRecord });
        const controller = new OrchestrationController(client);
        await controller.loadContext('conversation-1', 'branch-1');

        await expect(
            controller.updateMemoryRecord('memory-0', { summary: '수정된 합성 요약' }),
        ).resolves.toBe(true);

        expect(patchMemoryRecord).toHaveBeenCalledWith({
            memory_record_id: 'memory-0',
            patch: { summary: '수정된 합성 요약' },
            expected_revision: 1,
        });
    });

    it('changes exactly one memory exclusion scope with the exact revision', async () => {
        const initialRecord = workspace().memory_records[0];
        if (initialRecord === undefined) throw new Error('synthetic memory fixture is empty');
        const saved = {
            ...initialRecord,
            excluded_from_character: true,
            revision: 2,
        };
        const setMemoryRecordExclusion = vi.fn().mockResolvedValue(saved);
        const client = capableClient({ setMemoryRecordExclusion });
        const controller = new OrchestrationController(client);
        await controller.loadContext('conversation-1', 'branch-1');

        await expect(
            controller.setMemoryRecordExclusion('memory-0', 'character', true),
        ).resolves.toBe(true);

        expect(setMemoryRecordExclusion).toHaveBeenCalledWith({
            memory_record_id: 'memory-0',
            scope: 'character',
            excluded: true,
            expected_revision: 1,
        });
        expect(get(controller.state).workspace.memory_records[0]).toMatchObject({
            excluded_from_conversation: false,
            excluded_from_character: true,
            revision: 2,
        });
    });

    it('deletes one visible memory record with its exact CAS revision', async () => {
        const deleteMemoryRecord = vi.fn().mockResolvedValue(undefined);
        const client = capableClient({ deleteMemoryRecord });
        const controller = new OrchestrationController(client);
        await controller.loadContext('conversation-1', 'branch-1');

        await expect(controller.deleteMemoryRecord('memory-0')).resolves.toBe(true);
        expect(deleteMemoryRecord).toHaveBeenCalledWith({
            memory_record_id: 'memory-0',
            expected_revision: 1,
        });
        expect(
            get(controller.state).workspace.memory_records.some(
                (record) => record.id === 'memory-0',
            ),
        ).toBe(false);
    });

    it('persists memory controls through exact live envelopes and reopens the durable projection', async () => {
        type MemoryRecord = OrchestrationWorkspaceDto['memory_records'][number];
        interface Invocation {
            commandName: string;
            args?: Record<string, unknown>;
        }

        const initialRecord = workspace().memory_records[0];
        if (initialRecord === undefined) throw new Error('synthetic memory fixture is empty');
        let durableRecord: MemoryRecord | null = structuredClone(initialRecord);
        let malformedMemoryResponse: MemoryRecord | null = null;
        const invocations: Invocation[] = [];
        const revisionConflict = (): LorepiaClientError =>
            new LorepiaClientError({
                code: 'revision_conflict',
                message_key: 'error.memory_record_revision_conflict',
                recoverable: true,
                operation_id: 'synthetic-memory-cas',
                field_errors: [
                    {
                        field: 'expected_revision',
                        message_key: 'error.expected_revision_stale',
                    },
                ],
            });
        const transport: LorepiaTransport = {
            invoke: (commandName, args) => {
                invocations.push({ commandName, args: structuredClone(args) });
                if (commandName === 'get_orchestration_workspace') {
                    const projection = workspace();
                    projection.memory_records =
                        durableRecord === null ? [] : [structuredClone(durableRecord)];
                    return Promise.resolve(projection);
                }
                if (commandName === 'get_editable_prompt_preset') {
                    return Promise.resolve({
                        value: editablePromptPreset(),
                        revision: 9,
                        created_at: '2026-08-03T00:00:00Z',
                        updated_at: '2026-08-03T00:00:00Z',
                        deleted_at: null,
                    });
                }
                if (
                    commandName === 'list_task_profiles' ||
                    commandName === 'list_memory_profiles' ||
                    commandName === 'list_knowledge_books' ||
                    commandName === 'list_transform_sets' ||
                    commandName === 'list_interaction_rule_sets' ||
                    commandName === 'list_content_modules' ||
                    commandName === 'list_interaction_proposals'
                ) {
                    return Promise.resolve([]);
                }
                if (commandName === 'expire_interaction_proposals') {
                    return Promise.resolve({
                        conversation_id: 'conversation-1',
                        branch_id: 'branch-1',
                        current_state_revision: 0,
                        expired_proposals: [],
                        has_more_expired: false,
                    });
                }
                if (commandName === 'patch_memory_record') {
                    const request = args?.request as {
                        memory_record_id: string;
                        patch: Partial<MemoryRecord>;
                        expected_revision: number;
                    };
                    const currentRecord = durableRecord;
                    if (
                        currentRecord?.id !== request.memory_record_id ||
                        request.expected_revision !== currentRecord.revision
                    ) {
                        return Promise.reject(revisionConflict());
                    }
                    if (malformedMemoryResponse !== null) {
                        const response = structuredClone(malformedMemoryResponse);
                        malformedMemoryResponse = null;
                        return Promise.resolve(response);
                    }
                    durableRecord = {
                        ...currentRecord,
                        ...request.patch,
                        revision: currentRecord.revision + 1,
                    };
                    return Promise.resolve(structuredClone(durableRecord));
                }
                if (commandName === 'set_memory_record_exclusion') {
                    const request = args?.request as {
                        memory_record_id: string;
                        scope: 'conversation' | 'character';
                        excluded: boolean;
                        expected_revision: number;
                    };
                    const currentRecord = durableRecord;
                    if (
                        currentRecord?.id !== request.memory_record_id ||
                        request.expected_revision !== currentRecord.revision
                    ) {
                        return Promise.reject(revisionConflict());
                    }
                    if (malformedMemoryResponse !== null) {
                        const response = structuredClone(malformedMemoryResponse);
                        malformedMemoryResponse = null;
                        return Promise.resolve(response);
                    }
                    durableRecord = {
                        ...currentRecord,
                        excluded_from_conversation:
                            request.scope === 'conversation'
                                ? request.excluded
                                : currentRecord.excluded_from_conversation,
                        excluded_from_character:
                            request.scope === 'character'
                                ? request.excluded
                                : currentRecord.excluded_from_character,
                        revision: currentRecord.revision + 1,
                    };
                    return Promise.resolve(structuredClone(durableRecord));
                }
                if (commandName === 'delete_memory_record') {
                    const request = args?.request as {
                        memory_record_id: string;
                        expected_revision: number;
                    };
                    const currentRecord = durableRecord;
                    if (
                        currentRecord?.id !== request.memory_record_id ||
                        request.expected_revision !== currentRecord.revision
                    ) {
                        return Promise.reject(revisionConflict());
                    }
                    durableRecord = null;
                    return Promise.resolve(undefined);
                }
                return Promise.reject(new Error(`unexpected command: ${commandName}`));
            },
            createChatChannel: () => ({}),
            listen: () => Promise.resolve(() => undefined),
        };
        const liveClient = new LiveLorepiaClient(transport);
        const controller = new OrchestrationController(liveClient);

        await controller.loadContext('conversation-1', 'branch-1');
        await expect(
            controller.updateMemoryRecord('memory-0', {
                title: '수정된 합성 기억',
                summary: '영속 합성 요약',
                importance: 8,
                keywords: ['합성', '영속'],
            }),
        ).resolves.toBe(true);
        await expect(controller.setMemoryRecordPinned('memory-0', true)).resolves.toBe(true);
        await expect(
            controller.setMemoryRecordExclusion('memory-0', 'conversation', true),
        ).resolves.toBe(true);
        await expect(
            controller.setMemoryRecordExclusion('memory-0', 'character', true),
        ).resolves.toBe(true);

        const beforeConflict = structuredClone(get(controller.state).workspace.memory_records[0]);
        const durableBeforeExternalWrite = durableRecord;
        durableRecord = {
            ...durableBeforeExternalWrite,
            summary: '백엔드에서 먼저 저장된 합성 요약',
            revision: durableBeforeExternalWrite.revision + 1,
        };
        await expect(
            controller.updateMemoryRecord('memory-0', { summary: '반영되면 안 되는 로컬 요약' }),
        ).resolves.toBe(false);
        expect(get(controller.state).workspace.memory_records[0]).toEqual(beforeConflict);
        expect(get(controller.state).error).toBe('error.memory_record_revision_conflict');

        const reopenedController = new OrchestrationController(liveClient);
        await reopenedController.loadContext('conversation-1', 'branch-1');
        expect(get(reopenedController.state).workspace.memory_records).toEqual([
            expect.objectContaining({
                id: 'memory-0',
                title: '수정된 합성 기억',
                summary: '백엔드에서 먼저 저장된 합성 요약',
                importance: 8,
                keywords: ['합성', '영속'],
                pinned: true,
                excluded_from_conversation: true,
                excluded_from_character: true,
                revision: 6,
            }),
        ]);

        const durableProjection = structuredClone(durableRecord);
        const beforeMalformedResponses = structuredClone(
            get(reopenedController.state).workspace.memory_records[0],
        );
        malformedMemoryResponse = {
            ...durableProjection,
            id: 'memory-from-another-request',
            revision: 7,
        };
        await expect(
            reopenedController.updateMemoryRecord('memory-0', {
                summary: '잘못된 ID 응답이 반영하면 안 되는 요약',
            }),
        ).resolves.toBe(false);
        expect(get(reopenedController.state)).toMatchObject({
            announcement: '',
            error: 'Core가 요청 권한과 일치하지 않는 장기기억 응답을 반환했습니다.',
        });
        expect(get(reopenedController.state).workspace.memory_records[0]).toEqual(
            beforeMalformedResponses,
        );

        malformedMemoryResponse = structuredClone(durableProjection);
        await expect(reopenedController.setMemoryRecordPinned('memory-0', false)).resolves.toBe(
            false,
        );
        expect(get(reopenedController.state)).toMatchObject({
            announcement: '',
            error: 'Core가 요청 권한과 일치하지 않는 장기기억 응답을 반환했습니다.',
        });
        expect(get(reopenedController.state).workspace.memory_records[0]).toEqual(
            beforeMalformedResponses,
        );

        malformedMemoryResponse = {
            ...durableProjection,
            revision: 5,
        };
        await expect(
            reopenedController.setMemoryRecordExclusion('memory-0', 'conversation', false),
        ).resolves.toBe(false);
        expect(get(reopenedController.state)).toMatchObject({
            announcement: '',
            error: 'Core가 요청 권한과 일치하지 않는 장기기억 응답을 반환했습니다.',
        });
        expect(get(reopenedController.state).workspace.memory_records[0]).toEqual(
            beforeMalformedResponses,
        );

        await expect(reopenedController.deleteMemoryRecord('memory-0')).resolves.toBe(true);
        expect(get(reopenedController.state).workspace.memory_records).toEqual([]);
        const afterRestartController = new OrchestrationController(liveClient);
        await afterRestartController.loadContext('conversation-1', 'branch-1');
        expect(get(afterRestartController.state).workspace.memory_records).toEqual([]);

        const memoryBoundaryInvocations = invocations.filter(({ commandName }) =>
            [
                'get_orchestration_workspace',
                'patch_memory_record',
                'set_memory_record_exclusion',
                'delete_memory_record',
            ].includes(commandName),
        );
        expect(memoryBoundaryInvocations).toEqual([
            {
                commandName: 'get_orchestration_workspace',
                args: {
                    request: {
                        conversation_id: 'conversation-1',
                        branch_id: 'branch-1',
                    },
                },
            },
            {
                commandName: 'patch_memory_record',
                args: {
                    request: {
                        memory_record_id: 'memory-0',
                        patch: {
                            title: '수정된 합성 기억',
                            summary: '영속 합성 요약',
                            importance: 8,
                            keywords: ['합성', '영속'],
                        },
                        expected_revision: 1,
                    },
                },
            },
            {
                commandName: 'patch_memory_record',
                args: {
                    request: {
                        memory_record_id: 'memory-0',
                        patch: { pinned: true },
                        expected_revision: 2,
                    },
                },
            },
            {
                commandName: 'set_memory_record_exclusion',
                args: {
                    request: {
                        memory_record_id: 'memory-0',
                        scope: 'conversation',
                        excluded: true,
                        expected_revision: 3,
                    },
                },
            },
            {
                commandName: 'set_memory_record_exclusion',
                args: {
                    request: {
                        memory_record_id: 'memory-0',
                        scope: 'character',
                        excluded: true,
                        expected_revision: 4,
                    },
                },
            },
            {
                commandName: 'patch_memory_record',
                args: {
                    request: {
                        memory_record_id: 'memory-0',
                        patch: { summary: '반영되면 안 되는 로컬 요약' },
                        expected_revision: 5,
                    },
                },
            },
            {
                commandName: 'get_orchestration_workspace',
                args: {
                    request: {
                        conversation_id: 'conversation-1',
                        branch_id: 'branch-1',
                    },
                },
            },
            {
                commandName: 'patch_memory_record',
                args: {
                    request: {
                        memory_record_id: 'memory-0',
                        patch: { summary: '잘못된 ID 응답이 반영하면 안 되는 요약' },
                        expected_revision: 6,
                    },
                },
            },
            {
                commandName: 'patch_memory_record',
                args: {
                    request: {
                        memory_record_id: 'memory-0',
                        patch: { pinned: false },
                        expected_revision: 6,
                    },
                },
            },
            {
                commandName: 'set_memory_record_exclusion',
                args: {
                    request: {
                        memory_record_id: 'memory-0',
                        scope: 'conversation',
                        excluded: false,
                        expected_revision: 6,
                    },
                },
            },
            {
                commandName: 'delete_memory_record',
                args: {
                    request: {
                        memory_record_id: 'memory-0',
                        expected_revision: 6,
                    },
                },
            },
            {
                commandName: 'get_orchestration_workspace',
                args: {
                    request: {
                        conversation_id: 'conversation-1',
                        branch_id: 'branch-1',
                    },
                },
            },
        ]);
    });

    it.each([
        { approved: true, decision: 'approve' as const, status: 'approved' as const },
        { approved: false, decision: 'reject' as const, status: 'rejected' as const },
    ])(
        'binds an ordinary $decision decision to both durable CAS revisions',
        async ({ approved, decision, status }) => {
            const loaded = workspace();
            const pending = {
                conversation_id: 'conversation-1',
                branch_id: 'branch-1',
                state_revision: 11,
                proposal_revision: 4,
                proposal: {
                    id: 'proposal-1',
                    title: '합성 상태 변경',
                    body: '현재 상태를 바꾸는 제안입니다.',
                    status: 'pending' as const,
                    source_interaction_state_revision: 10,
                    requested_at_epoch_seconds: 1,
                    expires_at_epoch_seconds: 60,
                    decided_at_epoch_seconds: null,
                },
            };
            loaded.interaction_proposals = [pending];
            const decideInteractionProposal = vi.fn().mockResolvedValue({
                proposal: {
                    ...pending.proposal,
                    status,
                    decided_at_epoch_seconds: 2,
                },
                state_revision: 12,
                effects: [],
            });
            const client = capableClient({
                getOrchestrationWorkspace: vi.fn().mockResolvedValue(loaded),
                expireInteractionProposals: vi.fn().mockResolvedValue({
                    conversation_id: 'conversation-1',
                    branch_id: 'branch-1',
                    current_state_revision: 11,
                    expired_proposals: [],
                    has_more_expired: false,
                }),
                listInteractionProposals: vi.fn().mockResolvedValue([pending]),
                decideInteractionProposal,
            });
            const controller = new OrchestrationController(client);
            await controller.loadContext('conversation-1', 'branch-1');

            await expect(controller.decideProposal('proposal-1', approved)).resolves.toBe(true);
            expect(decideInteractionProposal).toHaveBeenCalledWith({
                conversation_id: 'conversation-1',
                branch_id: 'branch-1',
                proposal_record_id: 'proposal-1',
                expected_state_revision: 11,
                expected_proposal_revision: 4,
                decision,
            });
            expect(get(controller.state)).toMatchObject({
                busy_interaction_proposal_id: null,
                workspace: { interaction_state_revision: 12, interaction_proposals: [] },
            });
        },
    );

    it('keeps a stale ordinary proposal pending when either CAS revision is rejected', async () => {
        const loaded = workspace();
        const pending = {
            conversation_id: 'conversation-1',
            branch_id: 'branch-1',
            state_revision: 11,
            proposal_revision: 4,
            proposal: {
                id: 'proposal-stale',
                title: '오래된 제안',
                body: '합성 CAS 충돌',
                status: 'pending' as const,
                source_interaction_state_revision: 10,
                requested_at_epoch_seconds: 1,
                expires_at_epoch_seconds: null,
                decided_at_epoch_seconds: null,
            },
        };
        loaded.interaction_proposals = [pending];
        const decideInteractionProposal = vi.fn().mockRejectedValue({
            code: 'invalid_input',
            message_key: 'error.invalid_input',
            recoverable: true,
            operation_id: 'synthetic-stale-cas',
            field_errors: [],
        });
        const controller = new OrchestrationController(
            capableClient({
                getOrchestrationWorkspace: vi.fn().mockResolvedValue(loaded),
                expireInteractionProposals: vi.fn().mockResolvedValue({
                    conversation_id: 'conversation-1',
                    branch_id: 'branch-1',
                    current_state_revision: 11,
                    expired_proposals: [],
                    has_more_expired: false,
                }),
                listInteractionProposals: vi.fn().mockResolvedValue([pending]),
                decideInteractionProposal,
            }),
        );
        await controller.loadContext('conversation-1', 'branch-1');

        await expect(controller.decideProposal('proposal-stale', true)).resolves.toBe(false);
        expect(decideInteractionProposal).toHaveBeenCalledWith(
            expect.objectContaining({
                expected_state_revision: 11,
                expected_proposal_revision: 4,
            }),
        );
        expect(get(controller.state)).toMatchObject({
            busy_interaction_proposal_id: null,
            error: 'error.invalid_input',
            workspace: {
                interaction_proposals: [
                    { proposal_revision: 4, proposal: { id: 'proposal-stale', status: 'pending' } },
                ],
            },
        });
    });

    it('keeps the exact proposal pending when the ordinary decision receipt is corrupted', async () => {
        const loaded = workspace();
        const pending = {
            conversation_id: 'conversation-1',
            branch_id: 'branch-1',
            state_revision: 11,
            proposal_revision: 4,
            proposal: {
                id: 'proposal-corrupted-receipt',
                title: '검토한 제안',
                body: '불변 본문',
                status: 'pending' as const,
                source_interaction_state_revision: 10,
                requested_at_epoch_seconds: 1,
                expires_at_epoch_seconds: null,
                decided_at_epoch_seconds: null,
            },
        };
        loaded.interaction_proposals = [pending];
        const controller = new OrchestrationController(
            capableClient({
                getOrchestrationWorkspace: vi.fn().mockResolvedValue(loaded),
                expireInteractionProposals: vi.fn().mockResolvedValue({
                    conversation_id: 'conversation-1',
                    branch_id: 'branch-1',
                    current_state_revision: 11,
                    expired_proposals: [],
                    has_more_expired: false,
                }),
                listInteractionProposals: vi.fn().mockResolvedValue([pending]),
                decideInteractionProposal: vi.fn().mockResolvedValue({
                    proposal: {
                        ...pending.proposal,
                        body: '변조된 본문',
                        status: 'approved',
                        decided_at_epoch_seconds: 2,
                    },
                    state_revision: Number.MAX_SAFE_INTEGER + 1,
                    effects: [],
                }),
            }),
        );
        await controller.loadContext('conversation-1', 'branch-1');

        await expect(controller.decideProposal('proposal-corrupted-receipt', true)).resolves.toBe(
            false,
        );
        expect(get(controller.state)).toMatchObject({
            busy_interaction_proposal_id: null,
            workspace: {
                interaction_state_revision: 11,
                interaction_proposals: [
                    { proposal: { id: 'proposal-corrupted-receipt', status: 'pending' } },
                ],
            },
        });
    });

    it.each([
        {
            name: 'duplicate IDs',
            mutate: (item: InteractionProposalListItemDto) => [item, structuredClone(item)],
        },
        {
            name: 'a source revision newer than the room state',
            mutate: (item: InteractionProposalListItemDto) => [
                {
                    ...item,
                    proposal: {
                        ...item.proposal,
                        source_interaction_state_revision: item.state_revision + 1,
                    },
                },
            ],
        },
    ])('rejects pending proposal pages with $name', async ({ mutate }) => {
        const pending: InteractionProposalListItemDto = {
            conversation_id: 'conversation-1',
            branch_id: 'branch-1',
            state_revision: 11,
            proposal_revision: 4,
            proposal: {
                id: 'proposal-authority',
                title: '합성 상태 변경',
                body: '신뢰할 수 있는 방 권한에만 속해야 합니다.',
                status: 'pending',
                source_interaction_state_revision: 10,
                requested_at_epoch_seconds: 1,
                expires_at_epoch_seconds: 60,
                decided_at_epoch_seconds: null,
            },
        };
        const controller = new OrchestrationController(
            capableClient({
                expireInteractionProposals: vi.fn().mockResolvedValue({
                    conversation_id: 'conversation-1',
                    branch_id: 'branch-1',
                    current_state_revision: 11,
                    expired_proposals: [],
                    has_more_expired: false,
                }),
                listInteractionProposals: vi.fn().mockResolvedValue(mutate(pending)),
            }),
        );

        await controller.loadContext('conversation-1', 'branch-1');

        expect(get(controller.state)).toMatchObject({
            phase: 'error',
            workspace: { interaction_proposals: [] },
        });
        expect(get(controller.state).error).toBe(
            '프롬프트 오케스트레이션 작업을 완료하지 못했습니다.',
        );
    });

    it('loads, stages, and CAS-saves only the creator-owned prompt document', async () => {
        const initial = editablePromptPreset();
        const refreshed = structuredClone(initial);
        const refreshedBlock = refreshed.blocks[0];
        if (refreshedBlock === undefined) throw new Error('editable fixture is empty');
        refreshed.blocks[0] = {
            ...refreshedBlock,
            role_hint: 'developer',
        };
        refreshed.cache_boundaries = [
            {
                id: 'cache-block-1',
                after_block_id: 'block-1',
                role_filter: { kind: 'all' },
                ttl: 'provider_default',
                mode: 'automatic',
            },
        ];
        const revisioned = (value: CreatorPromptPresetDocumentDto, revision: number) => ({
            value,
            revision,
            created_at: '2026-08-03T00:00:00Z',
            updated_at: '2026-08-03T00:00:00Z',
            deleted_at: null,
        });
        const getEditablePromptPreset = vi
            .fn()
            .mockResolvedValueOnce(revisioned(initial, 9))
            .mockResolvedValueOnce(revisioned(refreshed, 10));
        const upsertPromptPreset = vi.fn((input: UpsertPromptPresetInput) => {
            void input;
            return Promise.resolve({
                value: {
                    id: 'prompt-1',
                    name: '편집 프롬프트',
                    schema_version: 1,
                    block_count: 2,
                    default_generation_preset_id: null,
                },
                revision: 10,
                created_at: '2026-08-03T00:00:00Z',
                updated_at: '2026-08-03T00:01:00Z',
                deleted_at: null,
            });
        });
        const client = capableClient({
            getEditablePromptPreset,
            upsertPromptPreset,
            listTaskProfiles: vi.fn().mockResolvedValue([]),
        });
        const controller = new OrchestrationController(client);
        await controller.loadContext('conversation-1', 'branch-1');

        expect(controller.stageEditablePromptBlock('block-1', { role_hint: 'developer' })).toBe(
            true,
        );
        expect(controller.setEditablePromptCacheBoundary('block-1', true)).toBe(true);
        await expect(controller.saveEditablePromptPreset()).resolves.toBe(true);

        const savedPromptInput = upsertPromptPreset.mock.calls[0]?.[0];
        expect(savedPromptInput?.expected_revision).toBe(9);
        expect(savedPromptInput?.value.id).toBe('prompt-1');
        expect(savedPromptInput?.value.blocks[0]).toMatchObject({
            id: 'block-1',
            authority: 'creator',
            placement_zone: 'preset_instruction',
            role_hint: 'developer',
        });
        expect(savedPromptInput?.value.cache_boundaries[0]).toMatchObject({
            after_block_id: 'block-1',
            mode: 'automatic',
        });
        expect(JSON.stringify(savedPromptInput)).not.toContain('application_policy');
        expect(get(controller.state)).toMatchObject({
            editable_prompt_preset_dirty: false,
            editable_prompt_preset: { revision: 10 },
        });
    });

    it('keeps prompt edits made while a save is in flight on top of the saved CAS revision', async () => {
        const initial = editablePromptPreset();
        const persisted = structuredClone(initial);
        const persistedBlock = persisted.blocks[0];
        if (persistedBlock === undefined) throw new Error('editable fixture is empty');
        persisted.blocks[0] = { ...persistedBlock, role_hint: 'developer' };
        const revisioned = (value: CreatorPromptPresetDocumentDto, revision: number) => ({
            value,
            revision,
            created_at: '2026-08-03T00:00:00Z',
            updated_at: '2026-08-03T00:01:00Z',
            deleted_at: null,
        });
        const savedSummary = {
            value: {
                id: 'prompt-1',
                name: '편집 프롬프트',
                schema_version: 1,
                block_count: 1,
                default_generation_preset_id: null,
            },
            revision: 10,
            created_at: '2026-08-03T00:00:00Z',
            updated_at: '2026-08-03T00:01:00Z',
            deleted_at: null,
        };
        const pendingSave = deferred<typeof savedSummary>();
        const upsertPromptPreset = vi
            .fn<(input: UpsertPromptPresetInput) => Promise<typeof savedSummary>>()
            .mockImplementation(() => pendingSave.promise);
        const controller = new OrchestrationController(
            capableClient({
                getEditablePromptPreset: vi
                    .fn()
                    .mockResolvedValueOnce(revisioned(initial, 9))
                    .mockResolvedValueOnce(revisioned(persisted, 10)),
                upsertPromptPreset,
                listTaskProfiles: vi.fn().mockResolvedValue([]),
            }),
        );
        await controller.loadContext('conversation-1', 'branch-1');
        expect(controller.stageEditablePromptBlock('block-1', { role_hint: 'developer' })).toBe(
            true,
        );

        const saving = controller.saveEditablePromptPreset();
        await vi.waitFor(() => expect(upsertPromptPreset).toHaveBeenCalledOnce());
        expect(
            controller.stageEditablePromptBlock('block-1', {
                name: '저장 중 추가로 바꾼 이름',
            }),
        ).toBe(true);
        pendingSave.resolve(savedSummary);

        await expect(saving).resolves.toBe(true);
        expect(upsertPromptPreset.mock.calls[0]?.[0].value.blocks[0]?.name).toBe('편집 블록');
        expect(get(controller.state)).toMatchObject({
            editable_prompt_preset_dirty: true,
            editable_prompt_preset_loading: false,
            editable_prompt_preset: {
                revision: 10,
                value: {
                    blocks: [
                        {
                            name: '저장 중 추가로 바꾼 이름',
                            role_hint: 'developer',
                        },
                    ],
                },
            },
        });
        expect(get(controller.state).announcement).toContain('아직 저장되지 않았습니다');
    });

    it('edits task target, fallback, limits, and uses exact task CAS revisions', async () => {
        const taskProfile = taskProfileDocument();
        const listTaskProfiles = vi.fn().mockResolvedValue([
            {
                value: taskProfile,
                revision: 3,
                created_at: '2026-08-03T00:00:00Z',
                updated_at: '2026-08-03T00:00:00Z',
                deleted_at: null,
            },
        ]);
        const upsertTaskProfile = vi.fn((input: UpsertTaskProfileInput) =>
            Promise.resolve({
                value: input.value,
                revision: 4,
                created_at: '2026-08-03T00:00:00Z',
                updated_at: '2026-08-03T00:01:00Z',
                deleted_at: null,
            }),
        );
        const deleteTaskProfile = vi.fn().mockResolvedValue({
            value: taskProfile,
            revision: 5,
            created_at: '2026-08-03T00:00:00Z',
            updated_at: '2026-08-03T00:02:00Z',
            deleted_at: '2026-08-03T00:02:00Z',
        });
        const client = capableClient({
            getEditablePromptPreset: vi.fn().mockResolvedValue({
                value: editablePromptPreset(),
                revision: 9,
                created_at: '2026-08-03T00:00:00Z',
                updated_at: '2026-08-03T00:00:00Z',
                deleted_at: null,
            }),
            listTaskProfiles,
            upsertTaskProfile,
            deleteTaskProfile,
        });
        const controller = new OrchestrationController(client);
        await controller.loadContext('conversation-1', 'branch-1');

        expect(
            controller.stageTaskProfile('task-1', {
                route_id: 'route-2',
                fallback_route_ids: ['route-fallback'],
                timeout_ms: 45_000,
                concurrency_limit: 2,
            }),
        ).toBe(true);
        await expect(controller.saveTaskProfile('task-1')).resolves.toBe(true);
        expect(upsertTaskProfile).toHaveBeenCalledWith({
            value: {
                ...taskProfile,
                route_id: 'route-2',
                fallback_route_ids: ['route-fallback'],
                timeout_ms: 45_000,
                concurrency_limit: 2,
            },
            expected_revision: 3,
        });

        await expect(controller.deleteTaskProfile('task-1')).resolves.toBe(true);
        expect(deleteTaskProfile).toHaveBeenCalledWith({
            task_profile_id: 'task-1',
            expected_revision: 4,
        });
    });

    it('keeps a newer task-profile draft above an in-flight save and advances its next CAS', async () => {
        const taskProfile = taskProfileDocument();
        const firstSave = deferred<{
            value: TaskProfileDocumentDto;
            revision: number;
            created_at: string;
            updated_at: string;
            deleted_at: null;
        }>();
        const upsertTaskProfile = vi
            .fn<
                (input: UpsertTaskProfileInput) => Promise<{
                    value: TaskProfileDocumentDto;
                    revision: number;
                    created_at: string;
                    updated_at: string;
                    deleted_at: null;
                }>
            >()
            .mockImplementationOnce(() => firstSave.promise)
            .mockImplementationOnce((input) =>
                Promise.resolve({
                    value: input.value,
                    revision: 5,
                    created_at: '2026-08-03T00:00:00Z',
                    updated_at: '2026-08-03T00:02:00Z',
                    deleted_at: null,
                }),
            );
        const client = capableClient({
            listTaskProfiles: vi.fn().mockResolvedValue([
                {
                    value: taskProfile,
                    revision: 3,
                    created_at: '2026-08-03T00:00:00Z',
                    updated_at: '2026-08-03T00:00:00Z',
                    deleted_at: null,
                },
            ]),
            upsertTaskProfile,
        });
        const controller = new OrchestrationController(client);
        await controller.loadContext('conversation-1', 'branch-1');
        controller.stageTaskProfile('task-1', { route_id: 'route-submitted' });

        const saving = controller.saveTaskProfile('task-1');
        await vi.waitFor(() => expect(upsertTaskProfile).toHaveBeenCalledOnce());
        controller.stageTaskProfile('task-1', {
            route_id: 'route-newer',
            timeout_ms: 45_000,
        });
        firstSave.resolve({
            value: { ...taskProfile, route_id: 'route-submitted' },
            revision: 4,
            created_at: '2026-08-03T00:00:00Z',
            updated_at: '2026-08-03T00:01:00Z',
            deleted_at: null,
        });

        await expect(saving).resolves.toBe(true);
        expect(get(controller.state).editable_task_profiles[0]).toMatchObject({
            value: { route_id: 'route-newer', timeout_ms: 45_000 },
            expected_revision: 4,
            dirty: true,
        });
        expect(get(controller.state).announcement).toContain('아직 저장되지 않았습니다');

        await expect(controller.saveTaskProfile('task-1')).resolves.toBe(true);
        expect(upsertTaskProfile.mock.calls[1]?.[0]).toMatchObject({
            value: { route_id: 'route-newer', timeout_ms: 45_000 },
            expected_revision: 4,
        });
        expect(get(controller.state).editable_task_profiles[0]).toMatchObject({
            expected_revision: 5,
            dirty: false,
        });
    });

    it('requires bounded embedding dimensions and removes fallback routes for memory embedding', async () => {
        const taskProfile = taskProfileDocument();
        taskProfile.fallback_route_ids = ['route-fallback'];
        const upsertTaskProfile = vi.fn((input: UpsertTaskProfileInput) =>
            Promise.resolve({
                value: input.value,
                revision: 4,
                created_at: '2026-08-03T00:00:00Z',
                updated_at: '2026-08-03T00:01:00Z',
                deleted_at: null,
            }),
        );
        const client = capableClient({
            getEditablePromptPreset: vi.fn().mockResolvedValue({
                value: editablePromptPreset(),
                revision: 9,
                created_at: '2026-08-03T00:00:00Z',
                updated_at: '2026-08-03T00:00:00Z',
                deleted_at: null,
            }),
            listTaskProfiles: vi.fn().mockResolvedValue([
                {
                    value: taskProfile,
                    revision: 3,
                    created_at: '2026-08-03T00:00:00Z',
                    updated_at: '2026-08-03T00:00:00Z',
                    deleted_at: null,
                },
            ]),
            upsertTaskProfile,
        });
        const controller = new OrchestrationController(client);
        await controller.loadContext('conversation-1', 'branch-1');

        expect(
            controller.stageTaskProfile('task-1', {
                kind: 'memory_embedding',
            }),
        ).toBe(true);
        expect(get(controller.state).editable_task_profiles[0]?.value).toMatchObject({
            kind: 'memory_embedding',
            fallback_route_ids: [],
            embedding_dimensions: null,
        });
        await expect(controller.saveTaskProfile('task-1')).resolves.toBe(false);
        expect(get(controller.state).editable_task_profiles_error).toBe(
            '메모리 임베딩 차원은 1에서 32768 사이의 정수여야 합니다.',
        );
        expect(upsertTaskProfile).not.toHaveBeenCalled();

        controller.stageTaskProfile('task-1', { embedding_dimensions: 32_769 });
        await expect(controller.saveTaskProfile('task-1')).resolves.toBe(false);
        expect(upsertTaskProfile).not.toHaveBeenCalled();

        expect(
            controller.stageTaskProfile('task-1', {
                embedding_dimensions: 1536,
                fallback_route_ids: ['must-be-removed'],
            }),
        ).toBe(true);
        expect(get(controller.state).editable_task_profiles[0]?.value).toMatchObject({
            kind: 'memory_embedding',
            fallback_route_ids: [],
            embedding_dimensions: 1536,
        });
        await expect(controller.saveTaskProfile('task-1')).resolves.toBe(true);
        expect(upsertTaskProfile).toHaveBeenCalledWith({
            value: {
                ...taskProfile,
                kind: 'memory_embedding',
                fallback_route_ids: [],
                embedding_dimensions: 1536,
            },
            expected_revision: 3,
        });
    });

    it('clears embedding dimensions when changing to a non-embedding task kind', async () => {
        const taskProfile = {
            ...taskProfileDocument(),
            kind: 'memory_embedding' as const,
            embedding_dimensions: 3072,
        };
        const client = capableClient({
            getEditablePromptPreset: vi.fn().mockResolvedValue({
                value: editablePromptPreset(),
                revision: 9,
                created_at: '2026-08-03T00:00:00Z',
                updated_at: '2026-08-03T00:00:00Z',
                deleted_at: null,
            }),
            listTaskProfiles: vi.fn().mockResolvedValue([
                {
                    value: taskProfile,
                    revision: 3,
                    created_at: '2026-08-03T00:00:00Z',
                    updated_at: '2026-08-03T00:00:00Z',
                    deleted_at: null,
                },
            ]),
        });
        const controller = new OrchestrationController(client);
        await controller.loadContext('conversation-1', 'branch-1');

        expect(controller.stageTaskProfile('task-1', { kind: 'translation' })).toBe(true);
        expect(get(controller.state).editable_task_profiles[0]?.value).toMatchObject({
            kind: 'translation',
            embedding_dimensions: null,
        });
    });

    it('refuses to move a Core-owned ApplicationPolicy projection', async () => {
        const loaded = workspace();
        const policy = loaded.prompt_blocks[1];
        if (policy === undefined) throw new Error('synthetic fixture requires a policy block');
        loaded.prompt_blocks[1] = { ...policy, order_editable: false };
        const client = capableClient({
            getOrchestrationWorkspace: vi.fn().mockResolvedValue(loaded),
        });
        const controller = new OrchestrationController(client);
        await controller.loadContext('conversation-1', 'branch-1');

        await expect(controller.movePromptBlock('block-1', -1)).resolves.toBe(false);
        expect(client.reorderPromptBlocks).not.toHaveBeenCalled();
    });
});

describe('moveBlockByDrop', () => {
    it('is deterministic and leaves unknown drag targets unchanged', () => {
        const blocks = workspace().prompt_blocks.slice(0, 3);
        expect(moveBlockByDrop(blocks, 'block-0', 'block-2').map(({ id }) => id)).toEqual([
            'block-1',
            'block-2',
            'block-0',
        ]);
        expect(moveBlockByDrop(blocks, 'missing', 'block-2')).toBe(blocks);
    });

    it('rejects a drag target in a different placement zone', () => {
        const blocks = workspace().prompt_blocks.slice(0, 3);
        const third = blocks[2];
        if (third === undefined) throw new Error('synthetic fixture must contain three blocks');
        blocks[2] = { ...third, placement_zone: 'recent_history' };
        expect(moveBlockByDrop(blocks, 'block-0', 'block-2')).toBe(blocks);
    });
});
