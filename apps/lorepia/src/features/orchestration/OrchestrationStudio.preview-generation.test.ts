import { get } from 'svelte/store';
import { cleanup, fireEvent, render, screen, waitFor } from '@testing-library/svelte';
import { afterEach, describe, expect, it, vi } from 'vitest';

import { LorepiaAppController } from '../../app/app-controller';
import type { LorepiaClient } from '../../lib/ipc/contracts';
import ChatPane from '../chat/ChatPane.svelte';
import OrchestrationStudio from './OrchestrationStudio.svelte';
import { OrchestrationController } from './orchestration-controller';
import { appState, controller, orchestrationState } from './tests/fixtures';
import { liveStudioClient } from './tests/live-studio-fixtures';

afterEach(() => {
    cleanup();
    vi.restoreAllMocks();
});

describe('OrchestrationStudio', () => {
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
            section: 'diagnostics' as const,
            detailPage: 'plan',
            appState: get(appController.state),
            orchestrationState: get(orchestrationController.state),
            controller: orchestrationController,
            appController,
        };
        const rendered = render(OrchestrationStudio, props);
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
            section: 'diagnostics' as const,
            detailPage: 'plan',
            client: fixture.client,
            appState: appState(),
            orchestrationState: get(orchestrationController.state),
            controller: orchestrationController,
        };
        const rendered = render(OrchestrationStudio, props);
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
            section: 'diagnostics',
            detailPage: 'plan',
            client: fixture.client,
            appState: appState(),
            orchestrationState: get(orchestrationController.state),
            controller: orchestrationController,
        });
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
            section: 'diagnostics',
            detailPage: 'plan',
            client: approvalClient,
            appState: appState(),
            orchestrationState: orchestrationState(),
            controller: orchestrationController,
            appController,
        });
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
});
