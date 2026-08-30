import type {
    CreatorTransformSetDocumentDto,
    OrchestrationWorkspaceSnapshotDto,
    PromptPlanPreviewDto,
    RetryableGenerationAttemptDto,
} from '../../../lib/ipc/contracts';
import { LiveLorepiaClient, type LorepiaTransport } from '../../../lib/ipc/client';
import { LorepiaClientError } from '../../../lib/ipc/errors';
import { emptyOrchestrationWorkspace } from '../orchestration-controller';

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

export function liveStudioSnapshot(): OrchestrationWorkspaceSnapshotDto {
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

export interface LiveStudioCommand {
    commandName: string;
    args?: Record<string, unknown>;
}

export function liveStudioClient(
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
                        value: structuredClone(LIVE_STUDIO_TRANSFORM_SET),
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
