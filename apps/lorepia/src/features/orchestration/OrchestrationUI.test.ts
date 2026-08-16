import { get } from 'svelte/store';
import { cleanup, fireEvent, render, screen, waitFor, within } from '@testing-library/svelte';
import { afterEach, describe, expect, it, vi } from 'vitest';

import {
    INITIAL_APP_STATE,
    LorepiaAppController,
    type LorepiaAppState,
} from '../../app/app-controller';
import type {
    CreatorTransformSetDocumentDto,
    GenerationPresetDto,
    LorepiaClient,
    ModelRouteDto,
    OrchestrationWorkspaceSnapshotDto,
    PromptPlanPreviewDto,
    RetryableGenerationAttemptDto,
    SaveRoomOrchestrationConfigInput,
    SaveRoomOrchestrationConfigResult,
} from '../../lib/ipc/contracts';
import { LiveLorepiaClient, type LorepiaTransport } from '../../lib/ipc/client';
import { LorepiaClientError } from '../../lib/ipc/errors';
import ChatPane from '../chat/ChatPane.svelte';
import ProviderSettings from '../providers/ProviderSettings.svelte';
import OrchestrationQuickDrawer from './OrchestrationQuickDrawer.svelte';
import OrchestrationStudio from './OrchestrationStudio.svelte';
import {
    INITIAL_ORCHESTRATION_STATE,
    OrchestrationController,
    emptyOrchestrationWorkspace,
    type OrchestrationState,
} from './orchestration-controller';
import {
    ContentPackageController,
    INITIAL_CONTENT_PACKAGE_STATE,
    type ContentPackageState,
} from './content-package-controller';

afterEach(() => {
    cleanup();
    vi.restoreAllMocks();
});

const GENERATION_PRESET: GenerationPresetDto = {
    id: 'generation-1',
    model_route_id: 'route-1',
    display_name: '균형 생성',
    values: [],
    reasoning: {
        mode: 'provider_default',
        effort: null,
        budget_tokens: null,
        summary: 'provider_default',
        preserve_opaque_state: false,
    },
    prompt_cache: {
        mode: 'provider_default',
        ttl_kind: 'provider_default',
        ttl_seconds: null,
        context_reference: null,
    },
    created_at: '2026-08-03T00:00:00Z',
    updated_at: '2026-08-03T00:00:00Z',
};

const MODEL_ROUTE: ModelRouteDto = {
    id: 'route-1',
    connection_id: 'connection-1',
    api_family: 'open_ai_responses',
    model_id: 'synthetic-model-1',
    display_name: '합성 모델 1',
    route_config: {
        deployment_id: null,
        region: null,
        endpoint_path: null,
        values: [],
    },
    status: 'available',
    miss_count: 0,
    metadata_source: 'project_owned_synthetic',
    metadata_observed_at: '2026-08-03T00:00:00Z',
    first_seen_at: '2026-08-03T00:00:00Z',
    last_seen_at: '2026-08-03T00:00:00Z',
};

function appState(): LorepiaAppState {
    const state = structuredClone(INITIAL_APP_STATE);
    state.selected_conversation = {
        id: 'conversation-1',
        character_id: 'character-1',
        title: '합성 대화',
        created_at: '2026-08-03T00:00:00Z',
        updated_at: '2026-08-03T00:00:00Z',
    };
    state.conversation_state = {
        conversation_id: 'conversation-1',
        active_branch_id: 'branch-1',
        selected_mode: 'chat',
        updated_at: '2026-08-03T00:00:00Z',
    };
    state.providers.phase = 'ready';
    state.providers.workspace.routes = [MODEL_ROUTE];
    state.providers.workspace.presets = [GENERATION_PRESET];
    state.providers.workspace.settings.selected_model_route_id = 'route-1';
    state.providers.workspace.settings.selected_generation_preset_id = 'generation-1';
    return state;
}

function orchestrationState(): OrchestrationState {
    const workspace = emptyOrchestrationWorkspace('conversation-1', 'branch-1');
    workspace.prompt_presets = [
        {
            id: 'prompt-1',
            name: '합성 프롬프트',
            schema_version: 1,
            block_count: 1,
            default_generation_preset_id: null,
        },
    ];
    workspace.room_config.prompt_preset_id = 'prompt-1';
    workspace.generation_target = {
        model_route_id: 'route-1',
        generation_preset_id: 'generation-1',
    };
    workspace.prompt_blocks = [
        {
            id: 'block-1',
            name: '안전 정책',
            kind: 'static_instruction',
            enabled: true,
            order_editable: false,
            role_hint: 'system',
            placement_zone: 'A. 앱 정책',
            template_preview: '<img src=x onerror=alert(1)>',
            condition_summary: '항상',
            source_label: '합성 로컬 프리셋',
            provenance_label: 'local',
            priority: 100,
            minimum_tokens: 10,
            maximum_tokens: 100,
            overflow_policy: 'reject',
            cache_boundary_after: true,
        },
        {
            id: 'block-2',
            name: '최근 대화',
            kind: 'history_slice',
            enabled: true,
            order_editable: true,
            role_hint: 'user',
            placement_zone: 'G. 최근 대화',
            template_preview: null,
            condition_summary: '최근 12턴',
            source_label: '현재 분기',
            provenance_label: 'conversation',
            priority: 90,
            minimum_tokens: null,
            maximum_tokens: 1000,
            overflow_policy: 'keep_latest_items',
            cache_boundary_after: false,
        },
    ];
    workspace.creator_controls = [
        {
            id: 'tone',
            label: '말투',
            description: '답변의 말투',
            kind: 'select',
            value: '따뜻함',
            choices: ['따뜻함', '간결함'],
            minimum: null,
            maximum: null,
            step: null,
        },
    ];
    workspace.memory_records = [
        {
            id: 'memory-1',
            conversation_id: 'conversation-1',
            branch_id: 'branch-1',
            kind: 'episodic_event',
            title: '첫 만남',
            summary: '합성된 첫 만남',
            importance: 7,
            keywords: ['합성'],
            pinned: false,
            excluded_from_conversation: false,
            excluded_from_character: false,
            source_navigation: {
                conversation_id: 'conversation-1',
                branch_id: 'branch-1',
                start_message_id: 'message-1',
                end_message_id: 'message-2',
            },
            invalidated_at: null,
            updated_at: '2026-08-03T00:00:00Z',
            revision: 3,
        },
    ];
    workspace.content_modules = [
        {
            id: 'module-1',
            name: '합성 모듈',
            version: '1.0.0',
            source_label: 'project-owned synthetic',
            license_label: 'LicenseRef-Unknown',
            redistribution_status: 'unknown',
            conflicts: [],
            required_capabilities: ['prompt_fragments'],
            components: [
                {
                    id: 'component-1',
                    kind: 'prompt',
                    name: '합성 블록',
                    selected: true,
                    enabled: false,
                },
            ],
            active_revision: 1,
            available_revision: 2,
            revision: 4,
            state_revision: 5,
            merge_review_sha256: 'sha256:module-review',
            merge_plan_sha256: 'sha256:module-plan',
        },
    ];
    workspace.plan_preview = {
        generation_attempt_id: 'generation-attempt-1',
        plan_id: 'plan-1',
        plan_hash: 'sha256:synthetic-plan',
        prompt_preset_id: 'prompt-1',
        prompt_preset_revision: 2,
        generation_target: {
            model_route_id: 'route-1',
            generation_preset_id: 'generation-1',
        },
        estimated_input_tokens: 42,
        available_input_tokens: 126976,
        token_estimator_id: 'synthetic-token-estimator-v1',
        token_estimate_exact: true,
        messages: [
            {
                sequence: 0,
                block_id: 'block-1',
                block_kind: 'static_instruction',
                requested_role: 'developer',
                effective_role: 'system',
                estimated_tokens: 8,
                source_message_ids: [],
                truncated: false,
            },
            {
                sequence: 1,
                block_id: 'block-2',
                block_kind: 'history_slice',
                requested_role: 'system',
                effective_role: 'system',
                estimated_tokens: 24,
                source_message_ids: ['message-1', 'message-2'],
                truncated: false,
            },
        ],
        provider_family: 'anthropic_messages',
        provider_messages: [
            {
                sequence: 0,
                block_id: 'block-1',
                effective_role: 'system',
                wire_role: 'system',
                placement: 'message',
                estimated_tokens: 8,
            },
            {
                sequence: 1,
                block_id: 'block-2',
                effective_role: 'system',
                wire_role: 'system',
                placement: 'message',
                estimated_tokens: 24,
            },
        ],
        provider_cache_boundaries: [
            {
                boundary_id: 'cache-1',
                after_block_id: 'block-1',
                after_message_sequence: 0,
                role_filter: { kind: 'all' },
                ttl: 'provider_default',
                mode: 'automatic',
                disposition: {
                    disposition: 'mapped',
                    strategy: 'anthropic_inline_breakpoint',
                },
            },
        ],
        blocks: [
            {
                block_id: 'block-1',
                block_kind: 'static_instruction',
                source: {
                    authority: 'application',
                    source_kind: 'application_built_in',
                    source_id: 'lorepia.application-policy.v1',
                    source_revision: null,
                    source_hash: null,
                },
                status: 'included',
                original_estimated_tokens: 8,
                final_estimated_tokens: 8,
                produced_message_count: 1,
                knowledge_evidence: [],
                memory_record_ids: [],
                memory_evidence: [],
                truncated: false,
            },
        ],
        cache_directives: [
            {
                boundary_id: 'cache-1',
                after_block_id: 'block-1',
                after_message_sequence: 0,
                role_filter: { kind: 'all' },
                ttl: 'provider_default',
                mode: 'automatic',
                status: 'applied',
            },
        ],
        role_mappings: [
            {
                block_id: 'block-1',
                requested_role: 'developer',
                effective_role: 'system',
            },
        ],
        overflow: [],
        warnings: [],
        truncated: false,
        applied_parameters: [
            { field: 'temperature', value_kind: 'number', item_count: null },
            { field: 'max_output_tokens', value_kind: 'number', item_count: null },
        ],
        prompt_diff: [
            {
                sequence: 0,
                block_id: 'block-1',
                requested_role: 'developer',
                effective_role: 'system',
                wire_role: 'system',
                placement: 'message',
            },
        ],
    };
    return {
        ...structuredClone(INITIAL_ORCHESTRATION_STATE),
        phase: 'ready',
        context_key: 'conversation-1:branch-1',
        workspace,
        knowledge_simulation: {
            sample_text: '합성 지식 검사',
            entries: [
                {
                    id: 'knowledge-1',
                    source_kind: 'knowledge',
                    title: '합성 세계관 항목',
                    selected: true,
                    reason: 'recursive parent knowledge-root matched',
                    score: 0.91,
                    estimated_tokens: 12,
                    placement: 'retrieved_context',
                },
                {
                    id: 'knowledge-2',
                    source_kind: 'knowledge',
                    title: '제외된 합성 항목',
                    selected: false,
                    reason: 'token budget exhausted',
                    score: 0.2,
                    estimated_tokens: 20,
                    placement: null,
                },
            ],
            total_estimated_tokens: 12,
            truncated: false,
        },
    };
}

function controller(): OrchestrationController {
    return new OrchestrationController({} as LorepiaClient);
}

function deferred<Value>(): {
    promise: Promise<Value>;
    resolve: (value: Value) => void;
} {
    let resolve!: (value: Value) => void;
    const promise = new Promise<Value>((accept) => {
        resolve = accept;
    });
    return { promise, resolve };
}

const LIVE_STUDIO_TRANSFORM_SET: CreatorTransformSetDocumentDto = {
    id: 'set-1',
    name: '합성 표시 변환',
    enabled: true,
    rules: [
        {
            id: 'rule-1',
            name: '달빛 변환',
            enabled: true,
            phase: 'display_only',
            order: 0,
            pattern: { pattern: '달빛', case_insensitive: false },
            replacement: '은빛',
            condition: null,
            max_replacements: 1,
            input_limit: 1024,
            output_limit: 1024,
        },
    ],
    max_rules_per_phase: 10,
    max_output_chars: 4096,
};

function liveStudioSnapshot(): OrchestrationWorkspaceSnapshotDto {
    const base = emptyOrchestrationWorkspace('conversation-1', 'branch-1');
    return {
        expected_head: null,
        room_config_revision: 4,
        prompt_preset_revision: null,
        interaction_state_revision: 0,
        generation_target: {
            model_route_id: 'route-room-b',
            generation_preset_id: 'generation-room-b',
        },
        prompt_presets: [],
        room_config: {
            ...base.room_config,
            variable_overrides: {
                values: [
                    {
                        variable: { scope: 'conversation', namespace: null, id: 'tone' },
                        value: { type: 'text', value: '은은함' },
                    },
                ],
            },
        },
        prompt_blocks: [],
        creator_controls: [],
        knowledge_book_ids: ['book-1'],
        memory_records: [],
    };
}

interface LiveStudioCommand {
    commandName: string;
    args?: Record<string, unknown>;
}

function liveStudioClient(
    options: {
        knowledgeError?: boolean;
        promptPreview?: PromptPlanPreviewDto;
        retryableGenerationAttempts?: RetryableGenerationAttemptDto[];
        reviewedSendError?: Error;
        transformFailure?: boolean;
    } = {},
): { client: LiveLorepiaClient; commands: LiveStudioCommand[] } {
    const reviewedChannel = { kind: 'reviewed-prompt-test-channel' };
    const commands: LiveStudioCommand[] = [];
    const transport: LorepiaTransport = {
        invoke: (commandName, args) => {
            commands.push({ commandName, args });
            if (commandName === 'open_existing_conversation') {
                return Promise.resolve({
                    id: 'conversation-1',
                    character_id: 'character-1',
                    title: '합성 대화',
                    created_at: '2026-08-03T00:00:00Z',
                    updated_at: '2026-08-03T00:00:00Z',
                });
            }
            if (commandName === 'get_conversation_state') {
                return Promise.resolve({
                    conversation_id: 'conversation-1',
                    active_branch_id: 'branch-1',
                    selected_mode: 'chat',
                    updated_at: '2026-08-03T00:00:00Z',
                });
            }
            if (commandName === 'list_branches') {
                return Promise.resolve([
                    {
                        id: 'branch-1',
                        conversation_id: 'conversation-1',
                        title: null,
                        fork_message_id: null,
                        head_message_id: null,
                        created_at: '2026-08-03T00:00:00Z',
                        updated_at: '2026-08-03T00:00:00Z',
                    },
                ]);
            }
            if (
                commandName === 'list_branch_messages' ||
                commandName === 'list_retryable_memory_query_embeddings' ||
                commandName === 'list_interrupted_memory_jobs'
            ) {
                return Promise.resolve([]);
            }
            if (commandName === 'resolve_prompt_preview' && options.promptPreview !== undefined) {
                return Promise.resolve(structuredClone(options.promptPreview));
            }
            if (commandName === 'send_reviewed_prompt') {
                return options.reviewedSendError === undefined
                    ? Promise.resolve({ generation_id: 'generation-live-reviewed' })
                    : Promise.reject(options.reviewedSendError);
            }
            if (commandName === 'dispose_chat_stream') {
                return Promise.resolve(true);
            }
            if (commandName === 'get_orchestration_workspace') {
                return Promise.resolve(liveStudioSnapshot());
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
            if (commandName === 'expire_generation_attempt_proposals') {
                return Promise.resolve({
                    conversation_id: 'conversation-1',
                    source_branch_id: 'branch-1',
                    decisions: [],
                    has_more_due: false,
                });
            }
            if (commandName === 'list_generation_attempt_proposals') {
                return Promise.resolve([]);
            }
            if (commandName === 'list_retryable_generation_attempts') {
                return Promise.resolve(structuredClone(options.retryableGenerationAttempts ?? []));
            }
            if (
                commandName === 'list_interaction_proposals' ||
                commandName === 'list_task_profiles' ||
                commandName === 'list_memory_profiles' ||
                commandName === 'list_knowledge_books' ||
                commandName === 'list_interaction_rule_sets' ||
                commandName === 'list_content_modules'
            ) {
                return Promise.resolve([]);
            }
            if (commandName === 'list_transform_sets') {
                return Promise.resolve([
                    {
                        value: LIVE_STUDIO_TRANSFORM_SET,
                        revision: 3,
                        created_at: '2026-08-03T00:00:00Z',
                        updated_at: '2026-08-03T00:00:00Z',
                        deleted_at: null,
                    },
                ]);
            }
            if (commandName === 'simulate_knowledge_activation') {
                if (options.knowledgeError === true) {
                    return Promise.reject(
                        new LorepiaClientError({
                            code: 'simulation_failed',
                            message_key: 'error.simulation_failed',
                            recoverable: true,
                            operation_id: 'synthetic-knowledge-operation',
                            field_errors: [],
                        }),
                    );
                }
                const reason = { kind: 'keyword' as const, matched: '달빛' };
                return Promise.resolve({
                    selected: [
                        {
                            entry_id: 'entry-1',
                            content: 'project-owned synthetic private projection sentinel',
                            placement: 'retrieved_context',
                            estimated_tokens: 7,
                            recursion_depth: 0,
                            reasons: [reason],
                        },
                    ],
                    evidence: [
                        {
                            entry_id: 'entry-1',
                            selected: true,
                            reasons: [reason],
                            estimated_tokens: 7,
                            exclusion_reason: null,
                        },
                    ],
                    used_tokens: 7,
                    token_budget: 100,
                    truncated: false,
                });
            }
            if (commandName === 'preview_transform_rule') {
                const diff = {
                    unchanged_prefix_chars: 0,
                    before_fragment: '달빛',
                    after_fragment: '은빛',
                    unchanged_suffix_chars: 0,
                    truncated: false,
                };
                if (options.transformFailure === true) {
                    return Promise.resolve({
                        phase: 'display_only',
                        original: '달빛',
                        output: '달빛',
                        changed: false,
                        rendering: 'native_plain_text',
                        reports: [
                            {
                                trace: {
                                    rule_id: 'rule-1',
                                    applied: false,
                                    replacements: 0,
                                    input_chars: 2,
                                    output_chars: 2,
                                    error: '정규식 오류',
                                },
                                status: 'failed',
                                diff: null,
                            },
                        ],
                        diff: null,
                        error: { code: 'invalid_regex', message: '정규식 오류' },
                        truncated: false,
                    });
                }
                return Promise.resolve({
                    phase: 'display_only',
                    original: '달빛',
                    output: '은빛',
                    changed: true,
                    rendering: 'native_plain_text',
                    reports: [
                        {
                            trace: {
                                rule_id: 'rule-1',
                                applied: true,
                                replacements: 1,
                                input_chars: 2,
                                output_chars: 2,
                                error: null,
                            },
                            status: 'applied',
                            diff,
                        },
                    ],
                    diff,
                    error: null,
                    truncated: false,
                });
            }
            return Promise.reject(new Error(`unexpected live Studio command: ${commandName}`));
        },
        createChatChannel: () => reviewedChannel,
        listen: () => Promise.resolve(() => undefined),
    };
    return { client: new LiveLorepiaClient(transport), commands };
}

describe('OrchestrationController room prompt sources', () => {
    it('preserves a newer draft across a deferred save and advances the next CAS revision', async () => {
        const snapshot = liveStudioSnapshot();
        const firstSave = deferred<SaveRoomOrchestrationConfigResult>();
        const inputs: SaveRoomOrchestrationConfigInput[] = [];
        const saveRoomOrchestrationConfig = vi.fn(
            (
                input: SaveRoomOrchestrationConfigInput,
            ): Promise<SaveRoomOrchestrationConfigResult> => {
                inputs.push(structuredClone(input));
                if (inputs.length === 1) return firstSave.promise;
                const { expected_revision: _expectedRevision, ...roomValues } = input;
                void _expectedRevision;
                return Promise.resolve({
                    room_config: {
                        ...snapshot.room_config,
                        ...roomValues,
                    },
                    revision: 6,
                    generation_target: {
                        model_route_id: 'route-saved',
                        generation_preset_id: 'generation-saved',
                    },
                });
            },
        );
        const orchestrationController = new OrchestrationController({
            getOrchestrationWorkspace: vi.fn().mockResolvedValue(snapshot),
            saveRoomOrchestrationConfig,
        } as unknown as LorepiaClient);

        await orchestrationController.loadContext('conversation-1', 'branch-1');
        orchestrationController.stageRoomConfig({
            user_name_override: '별이',
            author_note: '첫 저장 초안',
            group_context: '별이와 달이 함께 대화한다.',
            template_slots: [{ name: 'tone', value: '차분하게' }],
        });
        const pendingResult = orchestrationController.saveRoomConfig();
        orchestrationController.stageRoomConfig({
            author_note: '저장 중 작성한 더 새로운 초안',
            template_slots: [{ name: 'tone', value: '조금 더 밝게' }],
        });

        const firstInput = inputs[0];
        if (firstInput === undefined) throw new Error('first room save was not dispatched');
        const { expected_revision: _expectedRevision, ...firstRoomValues } = firstInput;
        void _expectedRevision;
        firstSave.resolve({
            room_config: {
                ...snapshot.room_config,
                ...firstRoomValues,
            },
            revision: 5,
            generation_target: {
                model_route_id: 'route-saved',
                generation_preset_id: 'generation-saved',
            },
        });

        await expect(pendingResult).resolves.toBe(true);
        const afterDeferredSave = get(orchestrationController.state);
        expect(inputs[0]).toMatchObject({
            expected_revision: 4,
            user_name_override: '별이',
            author_note: '첫 저장 초안',
            group_context: '별이와 달이 함께 대화한다.',
            template_slots: [{ name: 'tone', value: '차분하게' }],
        });
        expect(afterDeferredSave.workspace.room_config).toMatchObject({
            user_name_override: '별이',
            author_note: '저장 중 작성한 더 새로운 초안',
            group_context: '별이와 달이 함께 대화한다.',
            template_slots: [{ name: 'tone', value: '조금 더 밝게' }],
        });
        expect(afterDeferredSave.workspace.room_config_revision).toBe(5);
        expect(afterDeferredSave.workspace.generation_target).toEqual({
            model_route_id: 'route-saved',
            generation_preset_id: 'generation-saved',
        });
        expect(afterDeferredSave.dirty_room_config).toBe(true);

        await expect(orchestrationController.saveRoomConfig()).resolves.toBe(true);
        expect(inputs[1]).toMatchObject({
            expected_revision: 5,
            user_name_override: '별이',
            author_note: '저장 중 작성한 더 새로운 초안',
            group_context: '별이와 달이 함께 대화한다.',
            template_slots: [{ name: 'tone', value: '조금 더 밝게' }],
        });
        expect(get(orchestrationController.state).dirty_room_config).toBe(false);
    });
});

function contentPackageState(): ContentPackageState {
    return {
        ...structuredClone(INITIAL_CONTENT_PACKAGE_STATE),
        phase: 'ready',
        status: 'inspected',
        revision: 3,
        inspection: {
            import_id: 'import-1',
            revision: 3,
            package_plan_hash: 'a'.repeat(64),
            review_sha256: 'b'.repeat(64),
            capability_review_sha256: 'c'.repeat(64),
            source_size_bytes: 2048,
            total_uncompressed_size_bytes: 4096,
            asset_count: 0,
            local_import_allowed: true,
            redistribution_status: 'denied_by_manifest',
            manifest: {
                package_id: 'package.synthetic',
                name: '<img src=x onerror=alert(1)>',
                version: '1.0.0',
                author: 'project-owned synthetic',
                license: 'LicenseRef-Private',
                redistribution_allowed: false,
                required_app_version: null,
                required_capabilities: ['prompt_fragments'],
            },
            components: [
                {
                    id: 'component-safe',
                    kind: 'prompt_preset',
                    disposition: 'importable',
                    required_capabilities: [],
                    dependency_ids: [],
                    conflict_ids: [],
                    asset_count: 0,
                },
                {
                    id: 'component-quarantined',
                    kind: 'transform_set',
                    disposition: 'quarantined',
                    required_capabilities: [],
                    dependency_ids: [],
                    conflict_ids: [],
                    asset_count: 0,
                },
            ],
            issues: [
                {
                    severity: 'warning',
                    code: 'quarantined-transform',
                    message: '실행 가능한 변환은 비활성 격리됨',
                },
                {
                    severity: 'warning',
                    code: 'unsupported-html',
                    message: '임의 HTML',
                },
            ],
            capability_decisions: [],
        },
    };
}

function contentPackageSelectionState(): ContentPackageState {
    return {
        ...contentPackageState(),
        phase: 'selection_ready',
        status: 'awaiting_review',
        revision: 4,
        selected_component_ids: ['component-safe'],
        required_capabilities: [],
        selection: {
            content_selection_plan_hash: 'd'.repeat(64),
            import_plan_sha256: 'e'.repeat(64),
            normalization_evidence_sha256: 'f'.repeat(64),
            normalization_evidence: [],
            target_review: {
                target_review_sha256: '1'.repeat(64),
                documents: [
                    {
                        source_component_id: 'component-safe',
                        component_document_ordinal: 0,
                        document_index: 0,
                        document_kind: 'prompt_preset',
                        target_object_id: 'prompt-existing',
                        disposition: 'update',
                        expected_target_revision_id: 'prompt-revision-7',
                        expected_target_state_revision: 8,
                        source_component_sha256: '2'.repeat(64),
                        document_sha256: '3'.repeat(64),
                    },
                    {
                        source_component_id: 'component-safe',
                        component_document_ordinal: 1,
                        document_index: 1,
                        document_kind: 'prompt_preset',
                        target_object_id: 'prompt-new',
                        disposition: 'create',
                        expected_target_revision_id: null,
                        expected_target_state_revision: null,
                        source_component_sha256: '2'.repeat(64),
                        document_sha256: '4'.repeat(64),
                    },
                ],
            },
        },
    };
}

function completedContentPackageState(): ContentPackageState {
    return {
        ...structuredClone(INITIAL_CONTENT_PACKAGE_STATE),
        result: {
            import_id: 'import-1',
            package_id: 'package.synthetic',
            status: 'completed',
            revision: 5,
            committed_document_ids: ['prompt-1'],
            asset_ids: [],
        },
    };
}

function restartedCompletedExportState(): ContentPackageState {
    return {
        ...structuredClone(INITIAL_CONTENT_PACKAGE_STATE),
        completed_package_exports: [
            {
                kind: 'lorepia_package',
                source_id: 'import-2',
                sha256: '8'.repeat(64),
                size_bytes: 8192,
                suggested_file_name: 'newer.lorepia.zip',
            },
            {
                kind: 'lorepia_package',
                source_id: 'import-1',
                sha256: '9'.repeat(64),
                size_bytes: 4096,
                suggested_file_name: 'older.lorepia.zip',
            },
        ],
    };
}

describe('OrchestrationQuickDrawer', () => {
    it('provides accessible quick controls and explicit save without silently persisting', async () => {
        const orchestrationController = controller();
        const stage = vi.spyOn(orchestrationController, 'stageRoomConfig');
        const save = vi.spyOn(orchestrationController, 'saveRoomConfig').mockResolvedValue(true);
        const readyAppState = appState();
        readyAppState.providers.workspace.routes.push({
            ...MODEL_ROUTE,
            id: 'route-2',
            model_id: 'synthetic-model-2',
            display_name: '합성 모델 2',
        });
        readyAppState.providers.workspace.presets.push({
            ...GENERATION_PRESET,
            id: 'generation-2',
            model_route_id: 'route-2',
            display_name: '합성 모델 2 균형 생성',
        });
        render(OrchestrationQuickDrawer, {
            appState: readyAppState,
            orchestrationState: { ...orchestrationState(), dirty_room_config: true },
            controller: orchestrationController,
        });

        const toggle = screen.getByRole('button', { name: /생성 설정/ });
        await fireEvent.click(toggle);
        const drawer = screen.getByRole('complementary', { name: '프롬프트와 생성' });
        expect(drawer).toBeInTheDocument();

        expect(within(drawer).getByLabelText('모델')).toHaveValue('route-1');
        await fireEvent.change(within(drawer).getByLabelText('모델'), {
            target: { value: 'route-2' },
        });
        await fireEvent.change(within(drawer).getByLabelText('생성 프리셋'), {
            target: { value: 'generation-1' },
        });
        await fireEvent.click(within(drawer).getByLabelText('길게'));
        await fireEvent.input(within(drawer).getByRole('slider', { name: /창의성/ }), {
            target: { value: '73' },
        });
        await fireEvent.change(within(drawer).getByLabelText('추론 강도'), {
            target: { value: 'extra_high' },
        });
        await fireEvent.click(within(drawer).getByLabelText('장기기억 사용'));
        await fireEvent.click(within(drawer).getByLabelText('세계관 지식 사용'));
        expect(stage).toHaveBeenCalledWith({ generation_preset_id: 'generation-2' });
        expect(stage).toHaveBeenCalledWith({ generation_preset_id: 'generation-1' });
        expect(stage).toHaveBeenCalledWith({ response_length: 'long' });
        expect(stage).toHaveBeenCalledWith({ creativity: 73 });
        expect(stage).toHaveBeenCalledWith({ reasoning_effort: 'extra_high' });
        expect(within(drawer).getByLabelText('장기기억 사용')).toBeEnabled();
        expect(stage).toHaveBeenCalledWith({ memory_enabled: false });
        expect(stage).toHaveBeenCalledWith({ knowledge_enabled: false });

        await fireEvent.click(within(drawer).getByRole('button', { name: '방 설정 저장' }));
        expect(save).toHaveBeenCalledOnce();

        await fireEvent.keyDown(window, { key: 'Escape' });
        expect(screen.queryByRole('complementary')).not.toBeInTheDocument();
    });

    it('filters large block sets and navigates their zone minimap', async () => {
        const state = orchestrationState();
        const recentBlock = state.workspace.prompt_blocks[1];
        if (recentBlock === undefined) throw new Error('recent block fixture is missing');
        recentBlock.enabled = false;
        render(OrchestrationStudio, {
            appState: appState(),
            orchestrationState: state,
            controller: controller(),
        });
        const blockCard = screen.getByRole('heading', { name: '프롬프트 블록' }).closest('section');
        if (blockCard === null) throw new Error('prompt block card is missing');
        const blockUi = within(blockCard);
        const minimap = blockUi.getByRole('navigation', { name: '프롬프트 블록 미니맵' });
        expect(minimap).toBeInTheDocument();
        expect(
            within(minimap).getByRole('button', { name: 'A. 앱 정책 구역으로 이동' }),
        ).toHaveAttribute('title', 'A. 앱 정책: 전체 1개, 사용 1개');
        expect(
            within(minimap).getByRole('button', { name: 'G. 최근 대화 구역으로 이동' }),
        ).toHaveAttribute('title', 'G. 최근 대화: 전체 1개, 사용 0개');

        await fireEvent.click(
            within(minimap).getByRole('button', { name: 'G. 최근 대화 구역으로 이동' }),
        );
        expect(blockUi.getByRole('combobox', { name: '블록 구역 필터' })).toHaveValue(
            'G. 최근 대화',
        );
        expect(blockUi.getByText('최근 대화')).toBeInTheDocument();
        expect(blockUi.queryByText('안전 정책')).not.toBeInTheDocument();

        await fireEvent.change(blockUi.getByRole('combobox', { name: '블록 활성 상태 필터' }), {
            target: { value: 'enabled' },
        });
        expect(blockUi.getByText('표시할 프롬프트 블록이 없습니다.')).toBeInTheDocument();
        await fireEvent.click(blockUi.getByRole('button', { name: '블록 필터 초기화' }));
        await fireEvent.click(
            within(minimap).getByRole('button', { name: 'A. 앱 정책 구역으로 이동' }),
        );
        const zoneHeading = blockUi.getByRole('heading', { name: 'A. 앱 정책' });
        expect(zoneHeading).toHaveFocus();
        expect(blockUi.getByText('안전 정책')).toBeInTheDocument();
        expect(blockUi.queryByText('최근 대화')).not.toBeInTheDocument();
    });
});

describe('OrchestrationStudio', () => {
    it('renders imported markup only as text and keeps Core policy blocks read-only', async () => {
        const orchestrationController = controller();
        const move = vi.spyOn(orchestrationController, 'movePromptBlock').mockResolvedValue(true);
        render(OrchestrationStudio, {
            appState: appState(),
            orchestrationState: orchestrationState(),
            controller: orchestrationController,
        });

        await fireEvent.input(screen.getByRole('searchbox', { name: '블록 검색' }), {
            target: { value: '안전 정책' },
        });
        expect(screen.getByText('안전 정책')).toBeInTheDocument();
        expect(screen.queryByText('최근 대화')).not.toBeInTheDocument();

        await fireEvent.click(screen.getByText('조건·토큰·오버플로 세부정보'));
        expect(screen.getByText('<img src=x onerror=alert(1)>')).toBeInTheDocument();
        expect(document.querySelector('img')).toBeNull();

        const movePolicy = screen.getByRole('button', { name: '안전 정책 블록 아래로 이동' });
        expect(movePolicy).toBeDisabled();
        expect(move).not.toHaveBeenCalled();
    });

    it('stages bounded room prompt sources and saves them explicitly', async () => {
        const readyState = orchestrationState();
        readyState.dirty_room_config = true;
        readyState.workspace.room_config.user_name_override = '별이';
        readyState.workspace.room_config.author_note = '차분한 장면을 유지한다.';
        readyState.workspace.room_config.group_context = '별이와 달이가 함께 대화한다.';
        readyState.workspace.room_config.template_slots = [{ name: 'tone', value: '차분하게' }];
        const orchestrationController = controller();
        const stage = vi.spyOn(orchestrationController, 'stageRoomConfig');
        const save = vi.spyOn(orchestrationController, 'saveRoomConfig').mockResolvedValue(true);
        render(OrchestrationStudio, {
            appState: appState(),
            orchestrationState: readyState,
            controller: orchestrationController,
        });

        const userName = screen.getByLabelText('사용자 표시 이름');
        const authorNote = screen.getByLabelText('작가 메모');
        const groupContext = screen.getByLabelText('그룹 문맥');
        const slotName = screen.getByLabelText('템플릿 슬롯 1 이름');
        const slotValue = screen.getByLabelText('템플릿 슬롯 1 값');
        expect(userName).toHaveAttribute('maxlength', '128');
        expect(authorNote).toHaveAttribute('maxlength', '32768');
        expect(groupContext).toHaveAttribute('maxlength', '32768');
        expect(slotName).toHaveAttribute('maxlength', '128');
        expect(slotValue).toHaveAttribute('maxlength', '32768');

        await fireEvent.input(userName, { target: { value: '새 별칭' } });
        await fireEvent.input(authorNote, { target: { value: '새 작가 메모' } });
        await fireEvent.input(groupContext, { target: { value: '새 그룹 문맥' } });
        await fireEvent.input(slotName, { target: { value: 'voice' } });
        await fireEvent.input(slotValue, { target: { value: '명랑하게' } });
        expect(stage).toHaveBeenCalledWith({ user_name_override: '새 별칭' });
        expect(stage).toHaveBeenCalledWith({ author_note: '새 작가 메모' });
        expect(stage).toHaveBeenCalledWith({ group_context: '새 그룹 문맥' });
        expect(stage).toHaveBeenCalledWith({
            template_slots: [{ name: 'voice', value: '차분하게' }],
        });
        expect(stage).toHaveBeenCalledWith({
            template_slots: [{ name: 'tone', value: '명랑하게' }],
        });

        await fireEvent.click(screen.getByRole('button', { name: '슬롯 추가' }));
        expect(stage).toHaveBeenCalledWith({
            template_slots: [
                { name: 'tone', value: '차분하게' },
                { name: '', value: '' },
            ],
        });
        await fireEvent.click(screen.getByRole('button', { name: '방별 프롬프트 소스 저장' }));
        expect(save).toHaveBeenCalledOnce();
    });

    it('blocks duplicate template-slot names and caps the slot editor at 128 entries', () => {
        const duplicateState = orchestrationState();
        duplicateState.dirty_room_config = true;
        duplicateState.workspace.room_config.template_slots = [
            { name: 'tone', value: '차분하게' },
            { name: 'tone', value: '명랑하게' },
        ];
        render(OrchestrationStudio, {
            appState: appState(),
            orchestrationState: duplicateState,
            controller: controller(),
        });

        expect(screen.getByRole('alert')).toHaveTextContent('중복되었습니다');
        expect(screen.getByRole('button', { name: '방별 프롬프트 소스 저장' })).toBeDisabled();

        cleanup();
        const cappedState = orchestrationState();
        cappedState.workspace.room_config.template_slots = Array.from(
            { length: 128 },
            (_, index) => ({ name: `slot_${String(index)}`, value: '' }),
        );
        render(OrchestrationStudio, {
            appState: appState(),
            orchestrationState: cappedState,
            controller: controller(),
        });

        expect(screen.getByText('슬롯 128/128')).toBeInTheDocument();
        expect(screen.getByRole('button', { name: '슬롯 추가' })).toBeDisabled();
    });

    it('routes knowledge and transform previews through the production live client', async () => {
        const fixture = liveStudioClient();
        const orchestrationController = new OrchestrationController(fixture.client);
        await orchestrationController.loadContext('conversation-1', 'branch-1');
        const readyState = get(orchestrationController.state);
        expect(readyState.phase).toBe('ready');
        const props = {
            client: fixture.client,
            appState: appState(),
            orchestrationState: readyState,
            controller: orchestrationController,
        };
        const rendered = render(OrchestrationStudio, props);

        await fireEvent.input(screen.getByLabelText('검사할 문장'), {
            target: { value: '달빛 지식 확인' },
        });
        await fireEvent.click(screen.getByRole('button', { name: '활성화 시뮬레이션' }));
        await waitFor(() => {
            expect(get(orchestrationController.state).knowledge_simulation).not.toBeNull();
        });
        await rendered.rerender({
            ...props,
            orchestrationState: get(orchestrationController.state),
        });

        expect(screen.getByText('entry-1')).toBeInTheDocument();
        expect(screen.getByText('keyword')).toBeInTheDocument();
        expect(screen.getByText(/knowledge · 선택 · 7 tokens/)).toHaveTextContent(
            /score 없음 · 배치 retrieved_context/,
        );
        expect(
            screen.queryByText('project-owned synthetic private projection sentinel'),
        ).not.toBeInTheDocument();
        expect(
            fixture.commands.find(
                ({ commandName }) => commandName === 'simulate_knowledge_activation',
            ),
        ).toEqual({
            commandName: 'simulate_knowledge_activation',
            args: {
                request: {
                    knowledge_book_id: 'book-1',
                    sample_texts: ['달빛 지식 확인'],
                    manual_entry_ids: [],
                    semantic_scores: [],
                    variables: {
                        values: [
                            {
                                variable: { scope: 'conversation', namespace: null, id: 'tone' },
                                value: { type: 'text', value: '은은함' },
                            },
                        ],
                    },
                    supported_capabilities: [],
                    token_estimates: [],
                    activation_seed: 0,
                },
            },
        });

        await fireEvent.input(screen.getByLabelText('규칙 ID'), {
            target: { value: 'rule-1' },
        });
        await fireEvent.input(screen.getByLabelText('합성 테스트 입력'), {
            target: { value: '달빛' },
        });
        await fireEvent.click(screen.getByRole('button', { name: '변환 diff 만들기' }));
        await waitFor(() => {
            expect(get(orchestrationController.state).transform_preview).not.toBeNull();
        });
        await rendered.rerender({
            ...props,
            orchestrationState: get(orchestrationController.state),
        });

        expect(screen.getByText('은빛')).toBeInTheDocument();
        const transformCard = screen
            .getByRole('heading', { name: '안전한 변환 미리보기' })
            .closest('section');
        if (transformCard === null) throw new Error('transform preview card is missing');
        expect(transformCard).toHaveTextContent('출처 set set-1 · rule rule-1');
        expect(within(transformCard).getByText('rule-1 · applied')).toBeInTheDocument();
        expect(transformCard).toHaveTextContent('치환 1회 · 2 → 2자');
        expect(
            fixture.commands.find(({ commandName }) => commandName === 'preview_transform_rule'),
        ).toEqual({
            commandName: 'preview_transform_rule',
            args: {
                request: {
                    transform_set_id: 'set-1',
                    transform_rule_id: 'rule-1',
                    sample_text: '달빛',
                    variables: {
                        values: [
                            {
                                variable: { scope: 'conversation', namespace: null, id: 'tone' },
                                value: { type: 'text', value: '은은함' },
                            },
                        ],
                    },
                    supported_capabilities: [],
                    approved_import_source_ids: [],
                    allow_resolved_prompt: false,
                },
            },
        });
    });

    it('retains one live reviewed plan across expert diagnostics and a backend send failure', async () => {
        const operationNonce = '00000000-0000-4000-8000-000000000017';
        const baselinePreview = orchestrationState().workspace.plan_preview;
        if (baselinePreview === null) throw new Error('synthetic plan preview is missing');
        const promptPreview = structuredClone(baselinePreview);
        promptPreview.generation_attempt_id = 'generation-attempt-live-reviewed';
        promptPreview.plan_id = 'plan-live-reviewed';
        promptPreview.plan_hash = 'sha256:live-reviewed-plan';
        promptPreview.prompt_preset_id = 'prompt-live-core-resolved';
        promptPreview.prompt_preset_revision = 17;
        promptPreview.generation_target = {
            model_route_id: 'route-room-b',
            generation_preset_id: 'generation-room-b',
        };
        promptPreview.estimated_input_tokens = 137;
        promptPreview.available_input_tokens = 4096;
        promptPreview.token_estimator_id = 'live-token-estimator-v7';
        promptPreview.token_estimate_exact = false;
        const resolvedBlock = promptPreview.blocks[0];
        if (resolvedBlock === undefined) throw new Error('synthetic plan block is missing');
        resolvedBlock.source = {
            authority: 'creator',
            source_kind: 'user_created',
            source_id: 'prompt-preset-live-17',
            source_revision: '17',
            source_hash: 'b'.repeat(64),
        };
        resolvedBlock.original_estimated_tokens = 144;
        resolvedBlock.final_estimated_tokens = 96;
        resolvedBlock.knowledge_evidence = [
            {
                entry_id: 'knowledge-live-selected',
                selected: true,
                reasons: [{ kind: 'keyword' }],
                estimated_tokens: 11,
                exclusion_code: null,
            },
        ];
        resolvedBlock.memory_record_ids = ['memory-live-pinned'];
        resolvedBlock.memory_evidence = [
            {
                record_id: 'memory-live-pinned',
                selected: true,
                lane: 'pinned',
                rank_millionths: 975_000,
                estimated_tokens: 13,
                reasons: [{ kind: 'pinned' }],
                exclusion_code: null,
            },
        ];
        promptPreview.role_mappings = [
            {
                block_id: 'block-1',
                requested_role: 'developer',
                effective_role: 'system',
            },
        ];
        promptPreview.cache_directives = [
            {
                boundary_id: 'cache-live-1',
                after_block_id: 'block-1',
                after_message_sequence: 0,
                role_filter: { kind: 'all' },
                ttl: 'provider_default',
                mode: 'automatic',
                status: 'applied',
            },
        ];
        promptPreview.overflow = [
            {
                block_id: 'block-1',
                policy: 'trim_tail',
                tokens_before: 144,
                tokens_after: 96,
            },
        ];
        promptPreview.warnings = ['cache_reuse_suboptimal'];
        promptPreview.truncated = true;

        const fixture = liveStudioClient({
            promptPreview,
            reviewedSendError: Object.assign(new Error('synthetic reviewed send rejection'), {
                code: 'reviewed_prompt_rejected',
                message_key: 'error.reviewed_prompt_rejected',
                recoverable: true,
                operation_id: 'operation-live-reviewed-failure',
                field_errors: [],
            }),
        });
        const orchestrationController = new OrchestrationController(fixture.client);
        await orchestrationController.loadContext('conversation-1', 'branch-1');
        const selectedConversation = appState().selected_conversation;
        if (selectedConversation === null) throw new Error('synthetic conversation is missing');
        const appController = new LorepiaAppController(fixture.client);
        await expect(appController.selectConversation(selectedConversation)).resolves.toBe(true);
        vi.spyOn(globalThis.crypto, 'randomUUID').mockReturnValue(operationNonce);
        const props = {
            appState: get(appController.state),
            orchestrationState: get(orchestrationController.state),
            controller: orchestrationController,
            appController,
        };
        const rendered = render(OrchestrationStudio, props);

        await fireEvent.click(screen.getByRole('tab', { name: '전문가' }));
        const reviewedText = '달빛 아래의 실제 검토 전송';
        await fireEvent.input(screen.getByLabelText('다음 사용자 메시지'), {
            target: { value: reviewedText },
        });
        await fireEvent.click(screen.getByRole('button', { name: '계획 다시 계산' }));
        await waitFor(() => {
            expect(get(orchestrationController.state).workspace.plan_preview?.plan_hash).toBe(
                promptPreview.plan_hash,
            );
        });
        await rendered.rerender({
            ...props,
            orchestrationState: get(orchestrationController.state),
        });

        expect(screen.getByText(/137 · live-token-estimator-v7 · estimate/)).toBeInTheDocument();
        expect(screen.getByText('4096')).toBeInTheDocument();
        expect(screen.getByText('generation-attempt-live-reviewed')).toBeInTheDocument();
        expect(screen.getByText(operationNonce)).toBeInTheDocument();
        const provenance = screen.getByText(/prompt-preset-live-17/).closest('td');
        expect(provenance).toHaveTextContent('creator · user_created');
        expect(provenance).toHaveTextContent('rev 17');
        expect(screen.getByText(/block-1: developer → system/)).toBeInTheDocument();
        expect(screen.getByText(/block-1 뒤 · automatic · applied/)).toHaveTextContent(
            'provider_default',
        );
        expect(screen.getByText(/block-1 · trim_tail · 144 → 96/)).toBeInTheDocument();
        expect(screen.getByText('knowledge-live-selected · 선택')).toBeInTheDocument();
        expect(screen.getByText(/memory-live-pinned · 선택 · lane pinned/)).toHaveTextContent(
            'rank 975000 · 13 tokens',
        );
        expect(screen.getByText('cache_reuse_suboptimal')).toBeInTheDocument();
        expect(screen.getByText(/매핑 anthropic_inline_breakpoint/)).toBeInTheDocument();
        expect(screen.getByText(/안전한 표시 한도에 따라/)).toBeInTheDocument();
        const providerStructureDetails = screen
            .getByText('제공자 변환 구조 (2개)')
            .closest('details');
        if (providerStructureDetails === null) {
            throw new Error('provider structure details are missing');
        }
        expect(providerStructureDetails).not.toHaveTextContent(operationNonce);
        expect(promptPreview).not.toHaveProperty('provider_request');
        expect(promptPreview).not.toHaveProperty('effective_messages');

        await fireEvent.click(screen.getByRole('button', { name: '검토한 계획으로 전송' }));
        await waitFor(() => {
            expect(get(appController.state).chat).toMatchObject({
                phase: 'error',
                error: 'error.reviewed_prompt_rejected',
                active_generation_id: null,
            });
        });

        const expectedPromptInput = {
            conversation_id: 'conversation-1',
            branch_id: 'branch-1',
            expected_head: null,
            user_text: reviewedText,
            generation_target: {
                model_route_id: 'route-room-b',
                generation_preset_id: 'generation-room-b',
            },
            prompt_preset_id: null,
            variable_overrides: {
                values: [
                    {
                        variable: { scope: 'conversation', namespace: null, id: 'tone' },
                        value: { type: 'text', value: '은은함' },
                    },
                ],
            },
        };
        expect(
            fixture.commands.filter(({ commandName }) =>
                ['resolve_prompt_preview', 'send_reviewed_prompt'].includes(commandName),
            ),
        ).toEqual([
            {
                commandName: 'resolve_prompt_preview',
                args: {
                    request: {
                        ...expectedPromptInput,
                        expected_plan_hash: null,
                        operation_nonce: operationNonce,
                    },
                },
            },
            {
                commandName: 'send_reviewed_prompt',
                args: {
                    input: {
                        ...expectedPromptInput,
                        expected_plan_hash: 'sha256:live-reviewed-plan',
                        generation_attempt_id: 'generation-attempt-live-reviewed',
                    },
                    streamId: '00000000-0000-4000-8000-000000000017',
                    onEvent: { kind: 'reviewed-prompt-test-channel' },
                },
            },
        ]);
        expect(get(orchestrationController.state).workspace.plan_preview?.plan_hash).toBe(
            'sha256:live-reviewed-plan',
        );
        expect(orchestrationController.reviewedPromptSendInput()).toEqual({
            ...expectedPromptInput,
            expected_plan_hash: 'sha256:live-reviewed-plan',
            generation_attempt_id: 'generation-attempt-live-reviewed',
        });
        await rendered.rerender({
            ...props,
            appState: get(appController.state),
            orchestrationState: get(orchestrationController.state),
        });
        expect(screen.getByLabelText('다음 사용자 메시지')).toHaveValue(reviewedText);
        expect(screen.getByRole('button', { name: '검토한 계획으로 전송' })).toBeEnabled();

        render(ChatPane, {
            appState: get(appController.state),
            controller: appController,
        });
        expect(screen.getByRole('alert')).toHaveTextContent('error.reviewed_prompt_rejected');
        expect(get(appController.state).announcement).not.toContain('전송했습니다');
        appController.destroy();
    });

    it('reuses the preview nonce after input invalidation and rotates only for a new operation', async () => {
        const preview = orchestrationState().workspace.plan_preview;
        if (preview === null) throw new Error('synthetic plan preview is missing');
        const firstNonce = '00000000-0000-4000-8000-000000000061';
        const secondNonce = '00000000-0000-4000-8000-000000000062';
        const randomUUID = vi
            .spyOn(globalThis.crypto, 'randomUUID')
            .mockReturnValueOnce(firstNonce)
            .mockReturnValueOnce(secondNonce);
        const fixture = liveStudioClient({ promptPreview: preview });
        const orchestrationController = new OrchestrationController(fixture.client);
        await orchestrationController.loadContext('conversation-1', 'branch-1');
        const props = {
            client: fixture.client,
            appState: appState(),
            orchestrationState: get(orchestrationController.state),
            controller: orchestrationController,
        };
        const rendered = render(OrchestrationStudio, props);

        await fireEvent.click(screen.getByRole('tab', { name: '전문가' }));
        await fireEvent.input(screen.getByLabelText('다음 사용자 메시지'), {
            target: { value: '첫 미리보기 입력' },
        });
        await fireEvent.click(screen.getByRole('button', { name: '계획 다시 계산' }));
        await waitFor(() => {
            expect(get(orchestrationController.state).plan_operation_nonce).toBe(firstNonce);
        });
        await rendered.rerender({
            ...props,
            orchestrationState: get(orchestrationController.state),
        });
        expect(screen.getByText(firstNonce)).toBeInTheDocument();

        await fireEvent.input(screen.getByLabelText('다음 사용자 메시지'), {
            target: { value: '수정된 같은 작업 입력' },
        });
        expect(get(orchestrationController.state)).toMatchObject({
            plan_operation_nonce: firstNonce,
            workspace: { plan_preview: null },
        });
        await fireEvent.click(screen.getByRole('button', { name: '계획 다시 계산' }));
        await waitFor(() => {
            expect(
                fixture.commands.filter(
                    ({ commandName }) => commandName === 'resolve_prompt_preview',
                ),
            ).toHaveLength(2);
        });
        await fireEvent.click(screen.getByRole('button', { name: '새 작업 미리보기' }));
        await waitFor(() => {
            expect(get(orchestrationController.state).plan_operation_nonce).toBe(secondNonce);
        });

        const previewRequests = fixture.commands
            .filter(({ commandName }) => commandName === 'resolve_prompt_preview')
            .map(({ args }) => args?.request);
        expect(previewRequests).toHaveLength(3);
        expect(previewRequests).toEqual([
            expect.objectContaining({
                user_text: '첫 미리보기 입력',
                expected_plan_hash: null,
                operation_nonce: firstNonce,
            }),
            expect.objectContaining({
                user_text: '수정된 같은 작업 입력',
                expected_plan_hash: null,
                generation_attempt_id: 'generation-attempt-1',
            }),
            expect.objectContaining({
                user_text: '수정된 같은 작업 입력',
                expected_plan_hash: null,
                operation_nonce: secondNonce,
            }),
        ]);
        expect(previewRequests[0]).not.toHaveProperty('generation_attempt_id');
        expect(previewRequests[1]).not.toHaveProperty('operation_nonce');
        expect(previewRequests[2]).not.toHaveProperty('generation_attempt_id');
        expect(randomUUID).toHaveBeenCalledTimes(2);
    });

    it('reloads a safe attempt projection after controller recreation and resumes by ID only', async () => {
        const originalPreview = orchestrationState().workspace.plan_preview;
        if (originalPreview === null) throw new Error('synthetic plan preview is missing');
        const preview = structuredClone(originalPreview);
        preview.generation_attempt_id = 'generation-approval-retry';
        const operationNonce = '00000000-0000-4000-8000-000000000071';
        const randomUUID = vi
            .spyOn(globalThis.crypto, 'randomUUID')
            .mockReturnValue(operationNonce);
        const fixture = liveStudioClient({
            promptPreview: preview,
            retryableGenerationAttempts: [
                {
                    generation_id: 'generation-approval-retry',
                    status: 'before_generation_applied',
                    created_at: '2026-08-10T00:00:00Z',
                    updated_at: '2026-08-10T00:00:01Z',
                },
            ],
        });
        const firstController = new OrchestrationController(fixture.client);
        await firstController.loadContext('conversation-1', 'branch-1');
        await firstController.resolvePlanPreview('승인 전 원래 입력');

        const orchestrationController = new OrchestrationController(fixture.client);
        await orchestrationController.loadContext('conversation-1', 'branch-1');
        render(OrchestrationStudio, {
            client: fixture.client,
            appState: appState(),
            orchestrationState: get(orchestrationController.state),
            controller: orchestrationController,
        });

        await fireEvent.click(screen.getByRole('tab', { name: '전문가' }));
        await fireEvent.input(screen.getByLabelText('다음 사용자 메시지'), {
            target: { value: '승인 뒤 재시도 입력' },
        });
        const retry = await screen.findByRole('button', {
            name: '최종 계획 다시 검토: 생성 시도 generation-approval-retry',
        });
        await fireEvent.click(retry);
        await waitFor(() => {
            expect(
                fixture.commands.filter(
                    ({ commandName }) => commandName === 'resolve_prompt_preview',
                ),
            ).toHaveLength(2);
        });

        expect(
            fixture.commands
                .filter(({ commandName }) => commandName === 'resolve_prompt_preview')
                .map(({ args }) => args?.request),
        ).toEqual([
            expect.objectContaining({
                user_text: '승인 전 원래 입력',
                operation_nonce: operationNonce,
            }),
            expect.objectContaining({
                user_text: '승인 뒤 재시도 입력',
                generation_attempt_id: 'generation-approval-retry',
            }),
        ]);
        const approvalRetryRequests = fixture.commands
            .filter(({ commandName }) => commandName === 'resolve_prompt_preview')
            .map(({ args }) => args?.request);
        expect(approvalRetryRequests[0]).not.toHaveProperty('generation_attempt_id');
        expect(approvalRetryRequests[1]).not.toHaveProperty('operation_nonce');
        expect(
            fixture.commands.filter(
                ({ commandName }) => commandName === 'list_retryable_generation_attempts',
            ),
        ).toEqual([
            {
                commandName: 'list_retryable_generation_attempts',
                args: {
                    request: {
                        conversation_id: 'conversation-1',
                        source_branch_id: 'branch-1',
                        limit: 100,
                    },
                },
            },
        ]);
        expect(
            fixture.commands.some(({ commandName }) => commandName === 'send_reviewed_prompt'),
        ).toBe(false);
        expect(randomUUID).toHaveBeenCalledOnce();
    });

    it('shows live knowledge errors and Core transform fail-open output without inventing results', async () => {
        const fixture = liveStudioClient({ knowledgeError: true, transformFailure: true });
        const orchestrationController = new OrchestrationController(fixture.client);
        await orchestrationController.loadContext('conversation-1', 'branch-1');
        const props = {
            client: fixture.client,
            appState: appState(),
            orchestrationState: get(orchestrationController.state),
            controller: orchestrationController,
        };
        const rendered = render(OrchestrationStudio, props);

        await fireEvent.input(screen.getByLabelText('검사할 문장'), {
            target: { value: '실패할 지식 확인' },
        });
        await fireEvent.click(screen.getByRole('button', { name: '활성화 시뮬레이션' }));
        await waitFor(() => {
            expect(get(orchestrationController.state).error).toBe('error.simulation_failed');
        });
        await rendered.rerender({
            ...props,
            orchestrationState: get(orchestrationController.state),
        });

        expect(screen.getByRole('alert')).toHaveTextContent('error.simulation_failed');
        expect(get(orchestrationController.state).knowledge_simulation).toBeNull();
        expect(screen.getByText('아직 실행하지 않았습니다.')).toBeInTheDocument();

        await fireEvent.input(screen.getByLabelText('규칙 ID'), {
            target: { value: 'rule-1' },
        });
        await fireEvent.input(screen.getByLabelText('합성 테스트 입력'), {
            target: { value: '달빛' },
        });
        await fireEvent.click(screen.getByRole('button', { name: '변환 diff 만들기' }));
        await waitFor(() => {
            expect(get(orchestrationController.state).transform_preview).toMatchObject({
                input: '달빛',
                output: '달빛',
                used_original: true,
            });
        });
        await rendered.rerender({
            ...props,
            orchestrationState: get(orchestrationController.state),
        });

        expect(
            screen.getByText('변환 오류로 byte-identical 원문을 유지했습니다.'),
        ).toBeInTheDocument();
        expect(screen.getAllByText('정규식 오류')).not.toHaveLength(0);
        expect(screen.getByRole('alert')).toHaveTextContent('invalid_regex: 정규식 오류');
    });

    it('shows bounded memory-worker status, memory deletion, and knowledge evidence', async () => {
        const readyAppState = appState();
        const readyOrchestrationState = orchestrationState();
        const studioController = controller();
        const deleteMemoryRecord = vi
            .spyOn(studioController, 'deleteMemoryRecord')
            .mockResolvedValue(true);
        const memoryRecord = readyOrchestrationState.workspace.memory_records[0];
        if (memoryRecord === undefined) throw new Error('memory fixture is missing');
        memoryRecord.excluded_from_conversation = true;
        readyAppState.memory_supervisor = {
            phase: 'ready',
            error: null,
            status: {
                sequence: 7,
                phase: 'running',
                recovered_interrupted_jobs: 2,
                completed_jobs: 5,
            },
        };
        render(OrchestrationStudio, {
            appState: readyAppState,
            orchestrationState: readyOrchestrationState,
            controller: studioController,
        });

        expect(screen.getByText(/기억 작업 감시 중/)).toHaveTextContent('중단 복구 2건 · 완료 5건');
        expect(screen.getByText('현재 대화 선택 제외됨')).toBeInTheDocument();
        expect(screen.queryByRole('button', { name: /선택.*제외/ })).not.toBeInTheDocument();
        expect(screen.getByText('recursive parent knowledge-root matched')).toBeInTheDocument();
        expect(screen.getByText('token budget exhausted')).toBeInTheDocument();
        expect(screen.getByText(/knowledge · 선택 · 12 tokens/)).toHaveTextContent(
            'score 0.91 · 배치 retrieved_context',
        );
        expect(screen.getByText(/knowledge · 제외 · 20 tokens/)).toHaveTextContent(
            'score 0.2 · 배치 없음',
        );
        await fireEvent.click(screen.getByRole('button', { name: '삭제' }));
        await fireEvent.click(screen.getByRole('button', { name: '삭제 확인' }));
        expect(deleteMemoryRecord).toHaveBeenCalledWith('memory-1');
    });

    it('shows snapshot and final-plan knowledge evidence with explicit truncation warnings', async () => {
        const state = orchestrationState();
        const knowledgeSimulation = state.knowledge_simulation;
        if (knowledgeSimulation === null)
            throw new Error('knowledge simulation fixture is missing');
        state.knowledge_simulation = {
            ...knowledgeSimulation,
            truncated: true,
        };
        state.workspace.selection_evidence = [
            {
                id: 'snapshot-knowledge-1',
                source_kind: 'knowledge',
                title: '현재 방 달빛 지식',
                selected: false,
                reason: 'semantic score below threshold',
                score: 0.41,
                estimated_tokens: 17,
                placement: null,
            },
        ];
        state.list_truncation.selection_evidence = true;
        const firstBlock = state.workspace.plan_preview?.blocks[0];
        if (firstBlock === undefined) throw new Error('plan block fixture is missing');
        firstBlock.knowledge_evidence = [
            {
                entry_id: 'plan-knowledge-1',
                selected: true,
                reasons: [{ kind: 'keyword' }],
                estimated_tokens: 9,
                exclusion_code: null,
            },
            {
                entry_id: 'plan-knowledge-2',
                selected: false,
                reasons: [],
                estimated_tokens: 21,
                exclusion_code: 'knowledge_remaining_token_budget',
            },
        ];

        render(OrchestrationStudio, {
            appState: appState(),
            orchestrationState: state,
            controller: controller(),
        });

        expect(screen.getByText(/선택 근거 일부가 축약되었습니다/)).toHaveTextContent(
            '전체 후보 목록으로 해석하지 마세요',
        );
        await fireEvent.click(screen.getByRole('tab', { name: '전문가' }));
        expect(
            screen.getByRole('heading', { name: '현재 방의 지식·기억 선택 근거' }),
        ).toBeInTheDocument();
        expect(screen.getByText('현재 방 달빛 지식')).toBeInTheDocument();
        expect(screen.getByText('semantic score below threshold')).toBeInTheDocument();
        expect(screen.getByText('세계관 지식 선택 근거')).toBeInTheDocument();
        expect(screen.getByText('plan-knowledge-1 · 선택')).toBeInTheDocument();
        expect(screen.getByText(/"kind": "keyword"/)).toBeInTheDocument();
        expect(document.body.textContent).not.toContain('"matched"');
        expect(screen.getByText('plan-knowledge-2 · 제외').closest('li')).toHaveTextContent(
            'knowledge_remaining_token_budget',
        );
        expect(screen.getByText(/처음 300개 선택 근거만 표시합니다/)).toHaveTextContent(
            '전체 후보 목록으로 해석하지 마세요',
        );
    });

    it('shows the safe module lifecycle boundary and a bounded, escaped final plan preview in expert mode', async () => {
        const orchestrationController = controller();
        render(OrchestrationStudio, {
            appState: appState(),
            orchestrationState: orchestrationState(),
            controller: orchestrationController,
        });

        const advancedTab = screen.getByRole('tab', { name: '고급' });
        await fireEvent.keyDown(advancedTab, { key: 'ArrowRight' });
        const expertTab = screen.getByRole('tab', { name: '전문가' });
        expect(expertTab).toHaveAttribute('aria-selected', 'true');
        expect(expertTab).toHaveFocus();
        expect(
            screen.getByRole('heading', { name: '콘텐츠 모듈 활성화·롤백' }),
        ).toBeInTheDocument();
        expect(
            screen.getAllByText(/해시로 고정된 콘텐츠 모듈 활성화·롤백 API/).length,
        ).toBeGreaterThan(0);
        expect(screen.getByText('sha256:synthetic-plan')).toBeInTheDocument();
        expect(screen.queryByText('<script>never execute</script>')).not.toBeInTheDocument();
        expect(document.querySelector('script:not([src])')).toBeNull();
        expect(screen.getByText(/사용자가 요청할 때만 실제 생성/)).toBeInTheDocument();
        expect(screen.queryByText('private policy body')).not.toBeInTheDocument();
        expect(screen.getAllByText(/developer → system/).length).toBeGreaterThan(0);
        expect(screen.getByText(/제공자 계열/)).toHaveTextContent('anthropic_messages');
        expect(screen.getByText(/매핑 anthropic_inline_breakpoint/)).toBeInTheDocument();
        expect(screen.getByText(/lorepia\.application-policy\.v1/)).toBeInTheDocument();
        expect(screen.getByText('8 / 8')).toBeInTheDocument();
        expect(screen.getByText(/비공개 프롬프트 본문과 원시 제공자 요청/)).toBeInTheDocument();

        const messageSummary = screen.getByText('최종 메시지 구조 (2개)');
        const messageDetails = messageSummary.closest('details');
        if (messageDetails === null) throw new Error('message structure details are missing');
        expect(messageDetails).not.toHaveAttribute('open');
        await fireEvent.click(messageSummary);
        expect(within(messageDetails).getByText(/순서 0 · 블록 block-1/)).toBeInTheDocument();
        expect(
            screen.queryByText('project-owned synthetic effective instruction'),
        ).not.toBeInTheDocument();
        await fireEvent.click(screen.getByText('제공자 변환 구조 (2개)'));
        expect(screen.queryByText(/"model": "synthetic-model"/)).not.toBeInTheDocument();
        expect(screen.getByText('temperature')).toBeInTheDocument();
        expect(screen.getByText(/developer → system → system · message/)).toBeInTheDocument();

        await fireEvent.change(screen.getByRole('combobox', { name: '표시 필터' }), {
            target: { value: 'parameters' },
        });
        expect(screen.queryByText('최종 메시지 구조 (2개)')).not.toBeInTheDocument();
        expect(document.body.textContent.toLocaleLowerCase()).not.toContain('authorization');
        expect(document.body.textContent.toLocaleLowerCase()).not.toContain('api_key');
        expect(document.body.textContent).not.toContain('/Users/');
    });

    it('shows only content-free, hash-verified DisplayOnly diagnostics after message reopen', async () => {
        const reopenedAppState = appState();
        reopenedAppState.messages = {
            phase: 'ready',
            error: null,
            items: [
                {
                    id: 'message-user-1',
                    conversation_id: 'conversation-1',
                    parent_id: null,
                    role: 'user',
                    content: 'reopened user message',
                    status: 'complete',
                    generation_id: null,
                    created_at: '2026-08-03T00:00:00Z',
                },
                {
                    id: 'message-assistant-1',
                    conversation_id: 'conversation-1',
                    parent_id: 'message-user-1',
                    role: 'assistant',
                    content: 'DISPLAY_CONTENT_CANARY_MUST_NOT_ENTER_DIAGNOSTICS',
                    status: 'complete',
                    generation_id: 'generation-1',
                    created_at: '2026-08-03T00:00:01Z',
                    display_projection: {
                        canonical_content_sha256: 'c'.repeat(64),
                        display_content_sha256: 'd'.repeat(64),
                        diagnostics_sha256: 'e'.repeat(64),
                        diagnostics: [
                            {
                                set_revision_id: 'transform-set-revision-7',
                                rule_id: 'display-rule-1',
                                stage: 'display_only',
                                disposition: 'applied',
                                code: null,
                                before_sha256: 'a'.repeat(64),
                                after_sha256: 'b'.repeat(64),
                                recorded_at: '2026-08-03T00:00:01Z',
                            },
                        ],
                    },
                },
            ],
        };
        render(OrchestrationStudio, {
            appState: reopenedAppState,
            orchestrationState: orchestrationState(),
            controller: controller(),
        });

        await fireEvent.click(screen.getByRole('tab', { name: '전문가' }));
        const diagnosticsCard = screen
            .getByRole('heading', { name: '메시지 표시 변환 진단' })
            .closest('section');
        if (diagnosticsCard === null) throw new Error('display diagnostics card is missing');
        const card = within(diagnosticsCard);
        expect(card.getByText('display_only · applied')).toBeInTheDocument();
        expect(card.getByText('transform-set-revision-7')).toBeInTheDocument();
        expect(card.getByText('display-rule-1')).toBeInTheDocument();
        expect(card.getByText('c'.repeat(64))).toBeInTheDocument();
        expect(card.getByText('d'.repeat(64))).toBeInTheDocument();
        expect(card.getByText('e'.repeat(64))).toBeInTheDocument();
        expect(diagnosticsCard).not.toHaveTextContent(
            'DISPLAY_CONTENT_CANARY_MUST_NOT_ENTER_DIAGNOSTICS',
        );
        expect(diagnosticsCard).toHaveTextContent('2026-08-03T00:00:01Z');
    });

    it('dispatches only the retained reviewed token and refreshes attempt approvals after rejection', async () => {
        const orchestrationController = controller();
        const reviewedInput = {
            conversation_id: 'conversation-1',
            branch_id: 'branch-1',
            expected_head: null,
            user_text: '합성 검토 전송',
            generation_target: {
                model_route_id: 'route-1',
                generation_preset_id: 'generation-1',
            },
            prompt_preset_id: 'prompt-1',
            variable_overrides: { values: [] },
            expected_plan_hash: 'sha256:synthetic-plan',
            generation_attempt_id: 'generation-attempt-1',
        };
        expect(reviewedInput).not.toHaveProperty('operation_nonce');
        vi.spyOn(orchestrationController, 'reviewedPromptSendInput').mockReturnValue(reviewedInput);
        const clearPreview = vi.spyOn(orchestrationController, 'clearPlanPreview');
        const completeOperation = vi.spyOn(orchestrationController, 'completePlanOperation');
        const appController = new LorepiaAppController({} as LorepiaClient);
        const sendReviewed = vi.spyOn(appController, 'sendReviewedPrompt').mockResolvedValue(false);
        const expireGenerationAttemptProposals = vi.fn().mockResolvedValue({
            conversation_id: 'conversation-1',
            source_branch_id: 'branch-1',
            decisions: [],
            has_more_due: false,
        });
        const listGenerationAttemptProposals = vi.fn().mockResolvedValue([]);
        const approvalClient = {
            expireGenerationAttemptProposals,
            listGenerationAttemptProposals,
            listRetryableGenerationAttempts: vi.fn().mockResolvedValue([]),
            decideGenerationAttemptProposal: vi.fn(),
        } as unknown as LorepiaClient;

        render(OrchestrationStudio, {
            client: approvalClient,
            appState: appState(),
            orchestrationState: orchestrationState(),
            controller: orchestrationController,
            appController,
        });

        await fireEvent.click(screen.getByRole('tab', { name: '전문가' }));
        await waitFor(() => expect(listGenerationAttemptProposals).toHaveBeenCalledOnce());
        const sendButton = screen.getByRole('button', { name: '검토한 계획으로 전송' });
        expect(sendButton).toBeEnabled();
        await fireEvent.click(sendButton);

        await waitFor(() => {
            expect(sendReviewed).toHaveBeenCalledWith(reviewedInput);
            expect(listGenerationAttemptProposals).toHaveBeenCalledTimes(2);
        });
        expect(clearPreview).not.toHaveBeenCalled();
        expect(completeOperation).not.toHaveBeenCalled();
        appController.destroy();
    });

    it('refreshes room-scoped embedding retries after plan preview and exposes the shared retry panel', async () => {
        const readyAppState = appState();
        readyAppState.memory_query_retries.candidates = [
            {
                id: 'query-embedding-preview',
                status: 'failed',
                revision: 6,
                conversation_id: 'conversation-1',
                branch_id: 'branch-1',
                error_code: 'provider_unavailable',
                requires_unknown_outcome_acknowledgement: false,
            },
        ];
        readyAppState.memory_query_retries.phase = 'ready';
        const appController = new LorepiaAppController({} as LorepiaClient);
        const refreshRetries = vi
            .spyOn(appController, 'refreshMemoryQueryRetries')
            .mockResolvedValue(undefined);
        const orchestrationController = controller();
        const resolvePreview = vi
            .spyOn(orchestrationController, 'resolvePlanPreview')
            .mockResolvedValue(null);
        const expireGenerationAttemptProposals = vi.fn().mockResolvedValue({
            conversation_id: 'conversation-1',
            source_branch_id: 'branch-1',
            decisions: [],
            has_more_due: false,
        });
        const listGenerationAttemptProposals = vi.fn().mockResolvedValue([]);
        const approvalClient = {
            expireGenerationAttemptProposals,
            listGenerationAttemptProposals,
            listRetryableGenerationAttempts: vi.fn().mockResolvedValue([]),
            decideGenerationAttemptProposal: vi.fn(),
        } as unknown as LorepiaClient;

        render(OrchestrationStudio, {
            client: approvalClient,
            appState: readyAppState,
            orchestrationState: orchestrationState(),
            controller: orchestrationController,
            appController,
        });

        await fireEvent.click(screen.getByRole('tab', { name: '전문가' }));
        expect(
            screen.getByRole('heading', { name: '기억 검색 준비가 중단되었습니다' }),
        ).toBeInTheDocument();
        await waitFor(() => expect(listGenerationAttemptProposals).toHaveBeenCalledOnce());
        await fireEvent.input(screen.getByRole('textbox', { name: '다음 사용자 메시지' }), {
            target: { value: '합성 미리보기 요청' },
        });
        const resolveButton = screen.getByRole('button', { name: '계획 다시 계산' });
        expect(resolveButton).toBeEnabled();
        await fireEvent.click(resolveButton);

        await waitFor(() => {
            expect(resolvePreview).toHaveBeenCalledWith('합성 미리보기 요청');
            expect(refreshRetries).toHaveBeenCalledOnce();
            expect(listGenerationAttemptProposals).toHaveBeenCalledTimes(2);
        });
        const [resolveOrder] = resolvePreview.mock.invocationCallOrder;
        const [refreshOrder] = refreshRetries.mock.invocationCallOrder;
        if (resolveOrder === undefined || refreshOrder === undefined) {
            throw new Error('preview and retry refresh calls were not recorded');
        }
        expect(resolveOrder).toBeLessThan(refreshOrder);
        appController.destroy();
    });

    it('keeps the orchestration studio available when provider loading fails', () => {
        const failedProviderState = appState();
        failedProviderState.providers.phase = 'error';
        failedProviderState.providers.error = 'synthetic provider failure';

        render(ProviderSettings, {
            appState: failedProviderState,
            controller: new LorepiaAppController({} as LorepiaClient),
            orchestrationState: orchestrationState(),
            orchestrationController: controller(),
        });

        expect(screen.getByRole('heading', { name: '프롬프트 제작실' })).toBeInTheDocument();
        expect(screen.getByText('synthetic provider failure')).toBeInTheDocument();
    });

    it('requires bounded dimensions and disables fallback routes for memory embedding profiles', async () => {
        const state = orchestrationState();
        state.editable_task_profiles = [
            {
                value: {
                    id: 'embedding-task',
                    kind: 'memory_embedding',
                    route_id: 'route-1',
                    generation_preset_id: 'generation-1',
                    fallback_route_ids: [],
                    embedding_dimensions: null,
                    timeout_ms: 30_000,
                    rate_limit: { requests: 1, per_seconds: 60 },
                    concurrency_limit: 1,
                },
                expected_revision: 2,
                dirty: true,
            },
        ];
        const orchestrationController = controller();
        const stage = vi.spyOn(orchestrationController, 'stageTaskProfile');

        render(OrchestrationStudio, {
            appState: appState(),
            orchestrationState: state,
            controller: orchestrationController,
        });

        const dimensions = screen.getByRole('spinbutton', { name: '임베딩 차원' });
        expect(dimensions).toBeRequired();
        expect(dimensions).toHaveAttribute('min', '1');
        expect(dimensions).toHaveAttribute('max', '32768');
        expect(screen.getByLabelText(/Fallback route IDs/)).toBeDisabled();
        expect(
            screen.getByText('메모리 임베딩 차원은 1에서 32768 사이의 정수여야 합니다.'),
        ).toBeInTheDocument();
        expect(screen.getByRole('button', { name: '저장' })).toBeDisabled();

        await fireEvent.input(dimensions, { target: { value: '1536' } });
        expect(stage).toHaveBeenCalledWith('embedding-task', {
            embedding_dimensions: 1536,
        });
    });

    it('renders only the safe selective package review and disables quarantined components', async () => {
        render(OrchestrationStudio, {
            appState: appState(),
            orchestrationState: orchestrationState(),
            controller: controller(),
            contentPackageState: contentPackageState(),
            contentPackageController: new ContentPackageController({} as LorepiaClient),
        });

        await fireEvent.click(screen.getByRole('tab', { name: '전문가' }));

        expect(
            screen.getByRole('heading', { name: 'LorePia 패키지 선택 가져오기' }),
        ).toBeInTheDocument();
        expect(
            screen.getByRole('heading', { name: /<img src=x onerror=alert\(1\)>/ }),
        ).toBeInTheDocument();
        expect(document.querySelector('img')).toBeNull();
        expect(screen.getByLabelText(/component-safe/)).toBeEnabled();
        expect(screen.getByLabelText(/component-quarantined/)).toBeDisabled();
        expect(screen.getByText('실행 가능한 변환은 비활성 격리됨')).toBeInTheDocument();
        expect(document.body.textContent).not.toContain('/Users/');
        expect(document.body.textContent).not.toContain('raw-script-body');
    });

    it('exports only the just-completed package and renders safe delivery evidence', async () => {
        const packageController = new ContentPackageController({} as LorepiaClient);
        const exportCompletedPackage = vi
            .spyOn(packageController, 'exportCompletedPackage')
            .mockResolvedValue(true);
        const packageState = completedContentPackageState();
        packageState.export_receipt = {
            kind: 'lorepia_package',
            source_id: 'import-1',
            sha256: '9'.repeat(64),
            size_bytes: 8192,
            file_name: 'package.synthetic.lorepia.zip',
        };
        render(OrchestrationStudio, {
            appState: appState(),
            orchestrationState: orchestrationState(),
            controller: controller(),
            contentPackageState: packageState,
            contentPackageController: packageController,
        });

        await fireEvent.click(screen.getByRole('tab', { name: '전문가' }));
        expect(screen.getByRole('heading', { name: '가져오기 완료' })).toBeInTheDocument();
        expect(screen.getByRole('heading', { name: '최근 패키지 내보내기' })).toBeInTheDocument();
        expect(screen.getByText('파일명 package.synthetic.lorepia.zip')).toBeVisible();
        expect(screen.getByText('크기 8192바이트')).toBeVisible();
        expect(screen.getByText('9'.repeat(64))).toBeVisible();
        const exportButton = screen.getByRole('button', { name: '완료된 패키지 내보내기' });
        await fireEvent.click(exportButton);
        expect(exportCompletedPackage).toHaveBeenCalledOnce();
        expect(document.body.textContent).not.toContain('/Users/');
        expect(document.body.textContent).not.toContain('raw bytes');

        cleanup();
        packageState.exporting_import_id = 'import-1';
        render(OrchestrationStudio, {
            appState: appState(),
            orchestrationState: orchestrationState(),
            controller: controller(),
            contentPackageState: packageState,
            contentPackageController: packageController,
        });
        await fireEvent.click(screen.getByRole('tab', { name: '전문가' }));
        expect(screen.getByRole('button', { name: '내보내는 중…' })).toBeDisabled();
        expect(screen.getByRole('status')).toHaveTextContent(
            '운영체제 저장 위치를 선택하고 있습니다.',
        );
    });

    it('renders the restart-safe completed package catalog in backend order and exports a row', async () => {
        const packageController = new ContentPackageController({} as LorepiaClient);
        const exportFromCatalog = vi
            .spyOn(packageController, 'exportCompletedPackageFromCatalog')
            .mockResolvedValue(true);
        const reload = vi
            .spyOn(packageController, 'loadCompletedPackageExports')
            .mockResolvedValue(true);
        const packageState = restartedCompletedExportState();
        render(OrchestrationStudio, {
            appState: appState(),
            orchestrationState: orchestrationState(),
            controller: controller(),
            contentPackageState: packageState,
            contentPackageController: packageController,
        });

        await fireEvent.click(screen.getByRole('tab', { name: '전문가' }));
        const catalog = screen.getByRole('list', { name: '완료된 패키지 내보내기 목록' });
        const rows = within(catalog).getAllByRole('listitem');
        const [newerRow, olderRow] = rows;
        if (newerRow === undefined || olderRow === undefined) {
            throw new Error('synthetic completed export rows are missing');
        }
        expect(newerRow).toHaveTextContent('newer.lorepia.zip');
        expect(olderRow).toHaveTextContent('older.lorepia.zip');
        expect(within(newerRow).getByText('8'.repeat(64))).toBeVisible();
        expect(within(olderRow).getByText('크기 4096바이트')).toBeVisible();
        expect(screen.queryByRole('heading', { name: '가져오기 완료' })).toBeNull();

        await fireEvent.click(
            screen.getByRole('button', { name: 'older.lorepia.zip 완료 패키지 내보내기' }),
        );
        expect(exportFromCatalog).toHaveBeenCalledWith('import-1');
        await fireEvent.click(screen.getByRole('button', { name: '목록 새로고침' }));
        expect(reload).toHaveBeenCalledOnce();
    });

    it('bounds a corrupt oversized completed package catalog before rendering actions', async () => {
        const packageState = restartedCompletedExportState();
        packageState.completed_package_exports = Array.from({ length: 101 }, (_, index) => ({
            kind: 'lorepia_package' as const,
            source_id: `import-${String(index)}`,
            sha256: 'a'.repeat(64),
            size_bytes: index + 1,
            suggested_file_name: `package-${String(index)}.lorepia.zip`,
        }));
        render(OrchestrationStudio, {
            appState: appState(),
            orchestrationState: orchestrationState(),
            controller: controller(),
            contentPackageState: packageState,
            contentPackageController: new ContentPackageController({} as LorepiaClient),
        });

        await fireEvent.click(screen.getByRole('tab', { name: '전문가' }));
        expect(screen.getByText('package-99.lorepia.zip')).toBeVisible();
        expect(screen.queryByText('package-100.lorepia.zip')).toBeNull();
        expect(screen.getByText(/처음 100개 완료 패키지만 표시합니다/)).toBeVisible();
        expect(screen.getAllByRole('button', { name: /완료 패키지 내보내기$/ })).toHaveLength(100);
    });

    it('shows the exact target review and keeps approval disabled until every update target is confirmed', async () => {
        const packageController = new ContentPackageController({} as LorepiaClient);
        const toggleConfirmation = vi
            .spyOn(packageController, 'toggleUpdateTargetConfirmation')
            .mockReturnValue(true);
        const packageState = contentPackageSelectionState();
        render(OrchestrationStudio, {
            appState: appState(),
            orchestrationState: orchestrationState(),
            controller: controller(),
            contentPackageState: packageState,
            contentPackageController: packageController,
        });

        await fireEvent.click(screen.getByRole('tab', { name: '전문가' }));
        expect(screen.getByRole('heading', { name: '대상 쓰기 검토' })).toBeInTheDocument();
        expect(screen.getByText('1'.repeat(64))).toBeInTheDocument();
        expect(screen.getAllByText('2'.repeat(64))).toHaveLength(2);
        expect(screen.getByText('3'.repeat(64))).toBeInTheDocument();
        expect(screen.getByText('prompt-revision-7')).toBeInTheDocument();
        expect(screen.getByText(/기대 상태 CAS\s*8/)).toBeInTheDocument();
        expect(screen.getByText(/component-safe · 전체 문서 인덱스 0/)).toBeInTheDocument();
        expect(screen.getByText(/새 대상 생성 — 별도 업데이트 확인 불필요/)).toBeInTheDocument();
        const updateConfirmation = screen.getByLabelText('prompt-existing 기존 대상 업데이트 확인');
        expect(updateConfirmation).toBeEnabled();
        expect(
            screen.getByRole('button', { name: '표시된 근거와 기능 명시적 승인' }),
        ).toBeDisabled();
        await fireEvent.click(updateConfirmation);
        expect(toggleConfirmation).toHaveBeenCalledWith('component-safe', 0);

        cleanup();
        packageState.confirmed_update_targets = [
            {
                source_component_id: 'component-safe',
                component_document_ordinal: 0,
                target_object_id: 'prompt-existing',
                expected_target_revision_id: 'prompt-revision-7',
                expected_target_state_revision: 8,
            },
        ];
        render(OrchestrationStudio, {
            appState: appState(),
            orchestrationState: orchestrationState(),
            controller: controller(),
            contentPackageState: packageState,
            contentPackageController: packageController,
        });
        await fireEvent.click(screen.getByRole('tab', { name: '전문가' }));
        expect(
            screen.getByRole('button', { name: '표시된 근거와 기능 명시적 승인' }),
        ).toBeEnabled();
    });

    it('bounds target-review rendering and does not expose an unconfirmed hidden update', async () => {
        const packageState = contentPackageSelectionState();
        const selectionReview = packageState.selection;
        if (selectionReview === null) throw new Error('synthetic package selection is missing');
        selectionReview.target_review.documents = Array.from({ length: 201 }, (_, index) => ({
            source_component_id: 'component-safe',
            component_document_ordinal: index,
            document_index: index,
            document_kind: 'prompt_preset' as const,
            target_object_id: `prompt-target-${String(index)}`,
            disposition: 'update' as const,
            expected_target_revision_id: `prompt-revision-${String(index)}`,
            expected_target_state_revision: index + 1,
            source_component_sha256: '2'.repeat(64),
            document_sha256: '3'.repeat(64),
        }));
        render(OrchestrationStudio, {
            appState: appState(),
            orchestrationState: orchestrationState(),
            controller: controller(),
            contentPackageState: packageState,
            contentPackageController: new ContentPackageController({} as LorepiaClient),
        });

        await fireEvent.click(screen.getByRole('tab', { name: '전문가' }));
        expect(screen.getByText(/처음 200개 대상 문서만 표시합니다/)).toBeInTheDocument();
        expect(screen.getByText('prompt-target-199')).toBeInTheDocument();
        expect(screen.queryByText('prompt-target-200')).not.toBeInTheDocument();
        expect(
            screen.getByRole('button', { name: '표시된 근거와 기능 명시적 승인' }),
        ).toBeDisabled();
    });
});
