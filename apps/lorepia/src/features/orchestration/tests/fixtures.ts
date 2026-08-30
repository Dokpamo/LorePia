import { INITIAL_APP_STATE, type LorepiaAppState } from '../../../app/app-controller';
import type { GenerationPresetDto, LorepiaClient, ModelRouteDto } from '../../../lib/ipc/contracts';
import {
    INITIAL_ORCHESTRATION_STATE,
    OrchestrationController,
    emptyOrchestrationWorkspace,
    type OrchestrationState,
} from '../orchestration-controller';

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

export function generationPreset(): GenerationPresetDto {
    return structuredClone(GENERATION_PRESET);
}

export function modelRoute(): ModelRouteDto {
    return structuredClone(MODEL_ROUTE);
}

export function appState(): LorepiaAppState {
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
    state.providers.workspace.routes = [modelRoute()];
    state.providers.workspace.presets = [generationPreset()];
    state.providers.workspace.settings.selected_model_route_id = 'route-1';
    state.providers.workspace.settings.selected_generation_preset_id = 'generation-1';
    return state;
}

export function orchestrationState(): OrchestrationState {
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

export function controller(): OrchestrationController {
    return new OrchestrationController({} as LorepiaClient);
}
