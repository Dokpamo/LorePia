import { get } from 'svelte/store';
import { cleanup, fireEvent, render, screen, waitFor, within } from '@testing-library/svelte';
import { afterEach, describe, expect, it, vi } from 'vitest';

import { LorepiaAppController } from '../../app/app-controller';
import type { LorepiaClient } from '../../lib/ipc/contracts';
import OrchestrationStudio from './OrchestrationStudio.svelte';
import { OrchestrationController } from './orchestration-controller';
import { appState, controller, orchestrationState } from './tests/fixtures';
import { liveStudioClient } from './tests/live-studio-fixtures';

afterEach(() => {
    cleanup();
    vi.restoreAllMocks();
});

describe('OrchestrationStudio', () => {
    it('routes knowledge and transform previews through the production live client', async () => {
        const fixture = liveStudioClient();
        const orchestrationController = new OrchestrationController(fixture.client);
        await orchestrationController.loadContext('conversation-1', 'branch-1');
        const readyState = get(orchestrationController.state);
        expect(readyState.phase).toBe('ready');
        const props = {
            section: 'memory' as const,
            detailPage: 'knowledge',
            client: fixture.client,
            appState: appState(),
            orchestrationState: readyState,
            controller: orchestrationController,
        };
        const rendered = render(OrchestrationStudio, props);

        await fireEvent.input(screen.getByLabelText('검사할 문장'), {
            target: { value: '달빛 지식 확인' },
        });
        expect(screen.getByRole('toolbar', { name: '세계관 지식 시뮬레이션 작업' })).toHaveClass(
            'fixed',
        );
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

        await rendered.rerender({
            ...props,
            detailPage: 'transforms',
            orchestrationState: get(orchestrationController.state),
        });
        await fireEvent.input(screen.getByLabelText('규칙 ID'), {
            target: { value: 'rule-1' },
        });
        await fireEvent.input(screen.getByLabelText('합성 테스트 입력'), {
            target: { value: '달빛' },
        });
        expect(screen.getByRole('toolbar', { name: '안전한 변환 미리보기 작업' })).toHaveClass(
            'fixed',
        );
        await fireEvent.click(screen.getByRole('button', { name: '변환 diff 만들기' }));
        await waitFor(() => {
            expect(get(orchestrationController.state).transform_preview).not.toBeNull();
        });
        await rendered.rerender({
            ...props,
            detailPage: 'transforms',
            orchestrationState: get(orchestrationController.state),
        });

        expect(screen.getByText('은빛')).toBeInTheDocument();
        const transformCard = screen.getByRole('region', { name: '안전한 변환 미리보기' });
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

    it('shows live knowledge errors and Core transform fail-open output without inventing results', async () => {
        const fixture = liveStudioClient({ knowledgeError: true, transformFailure: true });
        const orchestrationController = new OrchestrationController(fixture.client);
        await orchestrationController.loadContext('conversation-1', 'branch-1');
        const props = {
            section: 'memory' as const,
            detailPage: 'knowledge',
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

        await rendered.rerender({
            ...props,
            detailPage: 'transforms',
            orchestrationState: get(orchestrationController.state),
        });
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
            detailPage: 'transforms',
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
        const updateMemoryRecord = vi
            .spyOn(studioController, 'updateMemoryRecord')
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
        const rendered = render(OrchestrationStudio, {
            section: 'memory',
            detailPage: 'records',
            appState: readyAppState,
            orchestrationState: readyOrchestrationState,
            controller: studioController,
        });

        expect(screen.getByText(/기억 작업 감시 중/)).toHaveTextContent('중단 복구 2건 · 완료 5건');
        const memoryList = screen.getByRole('list', { name: '장기기억 목록' });
        expect(within(memoryList).getAllByRole('button')).toHaveLength(1);
        await fireEvent.click(
            within(screen.getByRole('list', { name: '장기기억 목록' })).getByRole('button', {
                name: /첫 만남/,
            }),
        );

        expect(screen.getByRole('region', { name: '장기기억 편집' })).toBeInTheDocument();
        expect(screen.getByText('현재 대화 선택에서 제외되어 있습니다.')).toBeInTheDocument();
        const memoryActions = screen.getByRole('toolbar', { name: '장기기억 편집 작업' });
        expect(memoryActions).toHaveClass('fixed');
        expect(within(memoryActions).getByRole('button', { name: '삭제' })).toBeInTheDocument();
        expect(within(memoryActions).getByRole('button', { name: '저장' })).toBeInTheDocument();

        await fireEvent.input(screen.getByLabelText('요약'), {
            target: { value: '수정한 첫 만남' },
        });
        await fireEvent.click(within(memoryActions).getByRole('button', { name: '저장' }));
        expect(updateMemoryRecord).toHaveBeenCalledWith('memory-1', {
            summary: '수정한 첫 만남',
        });

        await fireEvent.click(
            within(screen.getByRole('list', { name: '장기기억 목록' })).getByRole('button', {
                name: /첫 만남/,
            }),
        );
        await fireEvent.click(screen.getByRole('button', { name: '삭제' }));
        expect(screen.getByRole('button', { name: '삭제 확인' })).toBeInTheDocument();
        expect(screen.getByRole('button', { name: '취소' })).toBeInTheDocument();
        await fireEvent.click(screen.getByRole('button', { name: '삭제 확인' }));
        expect(deleteMemoryRecord).toHaveBeenCalledWith('memory-1');

        await rendered.rerender({
            section: 'memory',
            detailPage: 'knowledge',
            appState: readyAppState,
            orchestrationState: readyOrchestrationState,
            controller: studioController,
        });
        expect(screen.getByText('recursive parent knowledge-root matched')).toBeInTheDocument();
        expect(screen.getByText('token budget exhausted')).toBeInTheDocument();
        expect(screen.getByText(/knowledge · 선택 · 12 tokens/)).toHaveTextContent(
            'score 0.91 · 배치 retrieved_context',
        );
        expect(screen.getByText(/knowledge · 제외 · 20 tokens/)).toHaveTextContent(
            'score 0.2 · 배치 없음',
        );
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

        const knowledgeProps = {
            section: 'memory' as const,
            detailPage: 'knowledge',
            appState: appState(),
            orchestrationState: state,
            controller: controller(),
        };
        const rendered = render(OrchestrationStudio, knowledgeProps);

        expect(screen.getByText(/선택 근거 일부가 축약되었습니다/)).toHaveTextContent(
            '전체 후보 목록으로 해석하지 마세요',
        );

        await rendered.rerender({
            ...knowledgeProps,
            section: 'diagnostics',
            detailPage: 'selection',
        });
        expect(
            screen.getByRole('region', { name: '현재 방의 지식·기억 선택 근거' }),
        ).toBeInTheDocument();
        expect(screen.getByText('현재 방 달빛 지식')).toBeInTheDocument();
        expect(screen.getByText('semantic score below threshold')).toBeInTheDocument();
        expect(screen.getByText(/처음 300개 선택 근거만 표시합니다/)).toHaveTextContent(
            '전체 후보 목록으로 해석하지 마세요',
        );

        await rendered.rerender({
            ...knowledgeProps,
            section: 'diagnostics',
            detailPage: 'plan',
        });
        expect(screen.getByText('세계관 지식 선택 근거')).toBeInTheDocument();
        expect(screen.getByText('plan-knowledge-1 · 선택')).toBeInTheDocument();
        expect(screen.getByText(/"kind": "keyword"/)).toBeInTheDocument();
        expect(document.body.textContent).not.toContain('"matched"');
        expect(screen.getByText('plan-knowledge-2 · 제외').closest('li')).toHaveTextContent(
            'knowledge_remaining_token_budget',
        );
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
            section: 'diagnostics',
            detailPage: 'plan',
            client: approvalClient,
            appState: readyAppState,
            orchestrationState: orchestrationState(),
            controller: orchestrationController,
            appController,
        });
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
            section: 'prompt',
            detailPage: 'profiles',
            appState: appState(),
            orchestrationState: state,
            controller: orchestrationController,
        });

        await fireEvent.click(screen.getByRole('button', { name: /embedding-task/ }));
        expect(screen.getByRole('form', { name: '작업 프로필 편집' })).toBeInTheDocument();
        const profileActions = screen.getByRole('toolbar', { name: '작업 프로필 편집 작업' });
        expect(profileActions).toHaveClass('fixed');
        expect(within(profileActions).getByRole('button', { name: '삭제' })).toBeEnabled();
        const dimensions = screen.getByRole('spinbutton', { name: '임베딩 차원' });
        expect(dimensions).toBeRequired();
        expect(dimensions).toHaveAttribute('min', '1');
        expect(dimensions).toHaveAttribute('max', '32768');
        expect(screen.queryByLabelText(/Fallback route IDs/)).not.toBeInTheDocument();
        expect(
            screen.getByText('메모리 임베딩 차원은 1에서 32768 사이의 정수여야 합니다.'),
        ).toBeInTheDocument();
        expect(within(profileActions).getByRole('button', { name: '저장' })).toBeDisabled();

        await fireEvent.input(dimensions, { target: { value: '1536' } });
        expect(stage).toHaveBeenCalledWith('embedding-task', {
            embedding_dimensions: 1536,
        });
    });
});
