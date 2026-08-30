import { cleanup, fireEvent, render, screen, waitFor, within } from '@testing-library/svelte';
import { afterEach, describe, expect, it, vi, type MockInstance } from 'vitest';

import type { LorepiaClient } from '../../../lib/ipc/contracts';
import { t } from '../../../lib/i18n';
import { LorepiaAppController } from '../../../app/app-controller';
import {
    INITIAL_ORCHESTRATION_STATE,
    OrchestrationController,
} from '../../orchestration/orchestration-controller';
import '../../../styles/app.css';
import ChatPane from '../ChatPane.svelte';
import { chatReadyState } from './chat-pane-state-builder';

afterEach(() => {
    cleanup();
    vi.restoreAllMocks();
    vi.unstubAllGlobals();
});

interface RenderedChat {
    controller: LorepiaAppController;
    sendMessage: MockInstance<LorepiaAppController['sendMessage']>;
}

function renderChat(appState = chatReadyState(), client?: LorepiaClient): RenderedChat {
    const controller = new LorepiaAppController({} as LorepiaClient);
    const sendMessage = vi.spyOn(controller, 'sendMessage').mockResolvedValue(true);
    render(ChatPane, { appState, controller, client });
    return { controller, sendMessage };
}

function renderChatWithSettings(
    appState = chatReadyState(),
    client?: LorepiaClient,
): RenderedChat & { orchestrationController: OrchestrationController } {
    const controller = new LorepiaAppController({} as LorepiaClient);
    const sendMessage = vi.spyOn(controller, 'sendMessage').mockResolvedValue(true);
    const orchestrationController = new OrchestrationController({} as LorepiaClient);
    render(ChatPane, {
        appState,
        controller,
        client,
        orchestrationState: {
            ...structuredClone(INITIAL_ORCHESTRATION_STATE),
            phase: 'ready',
        },
        orchestrationController,
    });
    return { controller, sendMessage, orchestrationController };
}

async function swipePointer(
    target: Element,
    {
        startX,
        startY,
        endX,
        endY,
        pointerId = 7,
    }: {
        startX: number;
        startY: number;
        endX: number;
        endY: number;
        pointerId?: number;
    },
): Promise<void> {
    await fireEvent.pointerDown(target, {
        pointerId,
        isPrimary: true,
        button: 0,
        clientX: startX,
        clientY: startY,
    });
    await fireEvent.pointerMove(target, {
        pointerId,
        isPrimary: true,
        buttons: 1,
        clientX: endX,
        clientY: endY,
    });
    await fireEvent.pointerUp(target, {
        pointerId,
        isPrimary: true,
        button: 0,
        clientX: endX,
        clientY: endY,
    });
}

describe('ChatPane transcript chrome', () => {
    it('shows desktop message tools only while the mouse is inside the turn', async () => {
        const appState = chatReadyState();
        appState.messages.items = [
            {
                id: 'assistant-hover-tools',
                conversation_id: 'conversation-1',
                parent_id: null,
                role: 'assistant',
                content: '마우스가 올라왔을 때만 도구를 보여 주세요.',
                status: 'complete',
                generation_id: 'generation-1',
                created_at: '2026-08-03T19:47:00',
            },
        ];
        const controller = new LorepiaAppController({} as LorepiaClient);
        render(ChatPane, { appState, controller, desktop: true });
        const message = screen.getByRole('article', { name: '캐릭터 메시지' });
        const messageRow = message.closest('.message-item');
        if (!(messageRow instanceof HTMLElement)) throw new Error('message row missing');

        expect(messageRow).not.toHaveClass('actions-hovered');
        await fireEvent.mouseEnter(messageRow);
        expect(messageRow).toHaveClass('actions-hovered');
        await fireEvent.mouseLeave(messageRow);
        expect(messageRow).not.toHaveClass('actions-hovered');

        controller.destroy();
    });

    it('keeps room controls out of the transcript and groups them inside conversation settings', async () => {
        const appState = chatReadyState();
        appState.branches = [
            {
                id: 'branch-1',
                conversation_id: 'conversation-1',
                title: '본편',
                fork_message_id: null,
                head_message_id: null,
                created_at: '2026-08-02T00:00:00Z',
                updated_at: '2026-08-02T00:00:00Z',
            },
            {
                id: 'branch-2',
                conversation_id: 'conversation-1',
                title: '다른 선택',
                fork_message_id: null,
                head_message_id: null,
                created_at: '2026-08-02T00:00:00Z',
                updated_at: '2026-08-02T00:00:00Z',
            },
        ];
        const { controller, orchestrationController } = renderChatWithSettings(appState);
        const setConversationMode = vi
            .spyOn(controller, 'setConversationMode')
            .mockResolvedValue(undefined);
        const selectBranch = vi.spyOn(controller, 'selectBranch').mockResolvedValue(undefined);

        expect(screen.queryByRole('radiogroup', { name: '대화 모드' })).not.toBeInTheDocument();
        expect(screen.queryByRole('button', { name: '새 생성 작업' })).not.toBeInTheDocument();

        await fireEvent.click(screen.getByRole('button', { name: t('quick.toggle') }));
        const settings = screen.getByRole('dialog', { name: '대화 설정' });
        const settingsUi = within(settings);

        expect(settingsUi.getByRole('heading', { name: '대화' })).toBeInTheDocument();
        expect(settingsUi.getByRole('radiogroup', { name: '대화 모드' })).toBeInTheDocument();
        expect(settingsUi.getByRole('combobox', { name: /^분기:/ })).toHaveAttribute(
            'aria-expanded',
            'false',
        );
        expect(
            settingsUi.getByText('현재 입력을 별도의 새 요청으로 처리합니다.'),
        ).toBeInTheDocument();

        await fireEvent.click(settingsUi.getByRole('radio', { name: '스토리' }));
        expect(setConversationMode).toHaveBeenCalledWith('story');
        await fireEvent.click(settingsUi.getByRole('combobox', { name: /^분기:/ }));
        await fireEvent.click(settingsUi.getByRole('option', { name: '다른 선택' }));
        expect(selectBranch).toHaveBeenCalledWith('branch-2');

        controller.destroy();
        orchestrationController.destroy();
    });

    it('opens the mobile utility page with a left swipe and closes it with a right swipe', async () => {
        const { controller } = renderChatWithSettings();
        const pane = document.querySelector<HTMLElement>('.chat-pane');
        if (pane === null) throw new Error('chat pane is missing');
        vi.spyOn(pane, 'getBoundingClientRect').mockReturnValue(new DOMRect(0, 0, 393, 852));

        await swipePointer(pane, {
            startX: 320,
            startY: 300,
            endX: 160,
            endY: 304,
        });
        const utilityPage = await screen.findByRole('dialog', { name: '도구 패널' });
        expect(utilityPage).toHaveClass('open');
        expect(within(utilityPage).getByRole('button', { name: '대화 설정 열기' })).toBeVisible();
        expect(
            within(utilityPage).queryByRole('button', { name: /^프롬프트 프리셋:/ }),
        ).not.toBeInTheDocument();

        vi.spyOn(utilityPage, 'getBoundingClientRect').mockReturnValue(new DOMRect(0, 0, 393, 852));
        await swipePointer(utilityPage, {
            startX: 80,
            startY: 300,
            endX: 250,
            endY: 304,
            pointerId: 8,
        });
        expect(utilityPage).toHaveClass('utility-settling');
        await waitFor(() =>
            expect(screen.queryByRole('dialog', { name: '도구 패널' })).not.toBeInTheDocument(),
        );
        controller.destroy();
    });

    it('keeps requested message tools open while the reader moves into the composer', async () => {
        const appState = chatReadyState();
        appState.messages.items = [
            {
                id: 'message-action-1',
                conversation_id: 'conversation-1',
                parent_id: null,
                role: 'user',
                content: '작업을 열어 주세요.',
                status: 'complete',
                generation_id: null,
                created_at: '2026-08-03T19:47:00',
            },
        ];
        const { controller } = renderChat(appState);
        const message = screen.getByRole('article', { name: '내 메시지' });
        const messageRow = message.closest('.message-item');
        const composer = screen.getByRole('textbox', { name: '메시지' });
        const scroller = screen.getByLabelText('메시지 기록');

        expect(
            screen.queryByRole('button', { name: '내 메시지 작업 보기' }),
        ).not.toBeInTheDocument();
        expect(message).toHaveAttribute('tabindex', '0');
        message.focus();
        expect(message).toHaveFocus();
        await waitFor(() => expect(messageRow).toHaveClass('actions-open'));

        composer.focus();
        expect(composer).toHaveFocus();
        expect(messageRow).toHaveClass('actions-open');

        await fireEvent.pointerDown(scroller);
        await waitFor(() => expect(messageRow).not.toHaveClass('actions-open'));
        controller.destroy();
    });
});

describe('ChatPane composer', () => {
    it('requires a distinct acknowledgement before retrying an interrupted memory job', async () => {
        const appState = chatReadyState();
        const job = {
            memory_job_id: 'memory-job-1',
            kind: 'summary' as const,
            revision: 3,
            conversation_id: 'conversation-1',
            branch_id: 'branch-1',
            source_start_message_id: 'message-1',
            source_end_message_id: 'message-2',
            attempt: 1,
            interruption_count: 2,
            last_interrupted_at: '2026-01-01T00:00:00Z',
            last_error_code: 'process_restarted',
        };
        appState.memory_query_retries = {
            phase: 'ready',
            error: null,
            candidates: [],
            interrupted_jobs: [job],
            busy_id: null,
            notice: null,
        };
        const { controller } = renderChat(appState);
        const retry = vi.spyOn(controller, 'retryInterruptedMemoryJob').mockResolvedValue(true);

        await fireEvent.click(screen.getByRole('button', { name: '작업 재시도 검토' }));
        expect(retry).not.toHaveBeenCalled();
        expect(
            screen.getByText(/같은 기억 작업이\s+중복 처리될 수 있음을 확인하세요/),
        ).toBeInTheDocument();

        await fireEvent.click(screen.getByRole('button', { name: '위험을 확인하고 작업 재시도' }));
        expect(retry).toHaveBeenCalledWith(job, true);
        controller.destroy();
    });

    it('requires a distinct acknowledgement before retrying an unknown embedding outcome', async () => {
        const appState = chatReadyState();
        const candidate = {
            id: 'query-embedding-1',
            status: 'interrupted' as const,
            revision: 4,
            conversation_id: 'conversation-1',
            branch_id: 'branch-1',
            error_code: 'provider_unavailable',
            requires_unknown_outcome_acknowledgement: true,
        };
        appState.memory_query_retries = {
            phase: 'ready',
            error: null,
            candidates: [candidate],
            interrupted_jobs: [],
            busy_id: null,
            notice: null,
        };
        const { controller } = renderChat(appState);
        const retry = vi.spyOn(controller, 'retryMemoryQueryEmbedding').mockResolvedValue(true);

        await fireEvent.click(screen.getByRole('button', { name: '재시도 검토' }));
        expect(retry).not.toHaveBeenCalled();
        expect(
            screen.getByText(/같은 임베딩 요청이 중복 처리될 수 있음을 확인하세요/),
        ).toBeInTheDocument();

        await fireEvent.click(screen.getByRole('button', { name: '위험을 확인하고 재시도' }));
        expect(retry).toHaveBeenCalledWith(candidate, true);
        controller.destroy();
    });

    it('retries failed and cancelled embedding preparation without unknown-outcome acknowledgement', async () => {
        const appState = chatReadyState();
        appState.memory_query_retries = {
            phase: 'ready',
            error: null,
            candidates: [
                {
                    id: 'query-embedding-failed',
                    status: 'failed',
                    revision: 2,
                    conversation_id: 'conversation-1',
                    branch_id: 'branch-1',
                    error_code: 'provider_unavailable',
                    requires_unknown_outcome_acknowledgement: false,
                },
                {
                    id: 'query-embedding-cancelled',
                    status: 'cancelled',
                    revision: 3,
                    conversation_id: 'conversation-1',
                    branch_id: 'branch-1',
                    error_code: null,
                    requires_unknown_outcome_acknowledgement: false,
                },
            ],
            interrupted_jobs: [],
            busy_id: null,
            notice: null,
        };
        const { controller } = renderChat(appState);
        const retry = vi.spyOn(controller, 'retryMemoryQueryEmbedding').mockResolvedValue(true);

        const retryButtons = screen.getAllByRole('button', { name: '준비 작업 재시도' });
        await fireEvent.click(retryButtons[0] as HTMLButtonElement);
        await fireEvent.click(retryButtons[1] as HTMLButtonElement);

        expect(retry).toHaveBeenNthCalledWith(
            1,
            appState.memory_query_retries.candidates[0],
            false,
        );
        expect(retry).toHaveBeenNthCalledWith(
            2,
            appState.memory_query_retries.candidates[1],
            false,
        );
        controller.destroy();
    });

    it('renders restored room choices and submits them with the current interaction revision', async () => {
        const submitInteractionChoice = vi.fn().mockResolvedValue({
            choice_effect: {
                effect_id: 'effect-choice-1',
                conversation_id: 'conversation-1',
                branch_id: 'branch-1',
                resulting_state_revision: 8,
                sequence: 3,
                event_created_at: '2026-08-03T00:00:01Z',
                replay_on_reopen: true,
                choice_status: 'consumed',
                selected_choice_id: 'choice-b',
                choice_decided_at_epoch_seconds: 2,
                effect: {
                    kind: 'present_choices',
                    choices: [
                        { id: 'choice-a', label: '왼쪽' },
                        { id: 'choice-b', label: '오른쪽' },
                    ],
                },
            },
            resulting_state_revision: 8,
        });
        const interactionClient = {
            listInteractionEffects: vi.fn().mockResolvedValue([]),
            subscribeInteractionEffects: vi.fn().mockResolvedValue(vi.fn()),
            acknowledgeInteractionEffect: vi.fn().mockResolvedValue(undefined),
            retryInteractionEffect: vi.fn().mockResolvedValue(undefined),
            expireInteractionProposals: vi.fn().mockResolvedValue({
                conversation_id: 'conversation-1',
                branch_id: 'branch-1',
                current_state_revision: 7,
                expired_proposals: [],
                has_more_expired: false,
            }),
            listInteractionProposals: vi.fn().mockResolvedValue([
                {
                    conversation_id: 'conversation-1',
                    branch_id: 'branch-1',
                    state_revision: 7,
                    proposal_revision: 1,
                    proposal: {
                        id: 'proposal-redacted',
                        title: 'Stored proposal unavailable',
                        body: 'The original proposal text cannot be displayed safely.',
                        projection_rejection_reason: 'unsafe_native_text',
                        status: 'pending',
                        source_interaction_state_revision: 7,
                        requested_at_epoch_seconds: 1,
                        expires_at_epoch_seconds: null,
                        decided_at_epoch_seconds: null,
                    },
                },
            ]),
            listReopenInteractionEffects: vi.fn().mockResolvedValue({
                current_state_revision: 7,
                items: [
                    {
                        effect_id: 'effect-rejected-1',
                        conversation_id: 'conversation-1',
                        branch_id: 'branch-1',
                        resulting_state_revision: 6,
                        sequence: 1,
                        event_created_at: '2026-08-02T23:59:59Z',
                        replay_on_reopen: true,
                        choice_status: null,
                        selected_choice_id: null,
                        choice_decided_at_epoch_seconds: null,
                        effect: {
                            kind: 'projection_rejected',
                            reason: 'unsafe_native_text',
                        },
                    },
                    {
                        effect_id: 'effect-choice-1',
                        conversation_id: 'conversation-1',
                        branch_id: 'branch-1',
                        resulting_state_revision: 7,
                        sequence: 2,
                        event_created_at: '2026-08-03T00:00:00Z',
                        replay_on_reopen: true,
                        choice_status: 'pending',
                        selected_choice_id: null,
                        choice_decided_at_epoch_seconds: null,
                        effect: {
                            kind: 'present_choices',
                            choices: [
                                { id: 'choice-a', label: '왼쪽' },
                                { id: 'choice-b', label: '오른쪽' },
                            ],
                        },
                    },
                ],
                older_cursor: null,
            }),
            submitInteractionChoice,
            decideInteractionProposal: vi.fn(),
        } as unknown as LorepiaClient;
        const { controller } = renderChat(chatReadyState(), interactionClient);

        const choice = await screen.findByRole('button', { name: '오른쪽' });
        expect(screen.getByText('안전한 표시 범위를 벗어난 저장 효과를 숨겼습니다.')).toBeVisible();
        expect(screen.getByText('저장 제안 내용을 표시할 수 없음')).toBeVisible();
        expect(screen.getByRole('button', { name: '승인' })).toBeDisabled();
        expect(screen.getByRole('button', { name: '거절' })).toBeEnabled();
        await fireEvent.click(choice);

        await waitFor(() => {
            expect(submitInteractionChoice).toHaveBeenCalledWith({
                conversation_id: 'conversation-1',
                branch_id: 'branch-1',
                effect_id: 'effect-choice-1',
                choice_id: 'choice-b',
                expected_state_revision: 7,
            });
        });
        await waitFor(() => {
            expect(screen.getByText(/선택 반영됨/)).toBeInTheDocument();
        });
        controller.destroy();
    });

    it('reports explicit ordinary-proposal expiry without presenting a false approval', async () => {
        const decideInteractionProposal = vi.fn();
        const interactionClient = {
            listInteractionEffects: vi.fn().mockResolvedValue([]),
            subscribeInteractionEffects: vi.fn().mockResolvedValue(vi.fn()),
            acknowledgeInteractionEffect: vi.fn().mockResolvedValue(undefined),
            retryInteractionEffect: vi.fn().mockResolvedValue(undefined),
            expireInteractionProposals: vi.fn().mockResolvedValue({
                conversation_id: 'conversation-1',
                branch_id: 'branch-1',
                current_state_revision: 8,
                expired_proposals: [
                    {
                        conversation_id: 'conversation-1',
                        branch_id: 'branch-1',
                        state_revision: 8,
                        proposal_revision: 4,
                        proposal: {
                            id: 'proposal-expired',
                            title: '만료된 제안',
                            body: '합성 제안',
                            status: 'expired',
                            source_interaction_state_revision: 7,
                            requested_at_epoch_seconds: 1,
                            expires_at_epoch_seconds: 2,
                            decided_at_epoch_seconds: 2,
                        },
                    },
                ],
                has_more_expired: false,
            }),
            listInteractionProposals: vi.fn().mockResolvedValue([]),
            listReopenInteractionEffects: vi.fn().mockResolvedValue({
                current_state_revision: 8,
                items: [],
                older_cursor: null,
            }),
            submitInteractionChoice: vi.fn(),
            decideInteractionProposal,
        } as unknown as LorepiaClient;
        const { controller } = renderChat(chatReadyState(), interactionClient);

        expect(
            await screen.findByText(
                '만료된 승인 제안을 정리했습니다. 생성을 다시 시도할 수 있습니다.',
            ),
        ).toHaveAttribute('role', 'status');
        expect(screen.queryByText('제안을 승인했습니다.')).not.toBeInTheDocument();
        expect(decideInteractionProposal).not.toHaveBeenCalled();
        controller.destroy();
    });

    it('renders and decides an ordinary room proposal through both reviewed CAS revisions', async () => {
        const pending = {
            conversation_id: 'conversation-1',
            branch_id: 'branch-1',
            state_revision: 7,
            proposal_revision: 3,
            proposal: {
                id: 'proposal-room-1',
                title: '문을 열기',
                body: '현재 방 상태를 변경합니다.',
                status: 'pending' as const,
                source_interaction_state_revision: 7,
                requested_at_epoch_seconds: 1,
                expires_at_epoch_seconds: null,
                decided_at_epoch_seconds: null,
            },
        };
        const decideInteractionProposal = vi.fn().mockResolvedValue({
            proposal: {
                ...pending.proposal,
                status: 'approved',
                decided_at_epoch_seconds: 2,
            },
            state_revision: 8,
            effects: [],
        });
        const interactionClient = {
            listInteractionEffects: vi.fn().mockResolvedValue([]),
            subscribeInteractionEffects: vi.fn().mockResolvedValue(vi.fn()),
            acknowledgeInteractionEffect: vi.fn().mockResolvedValue(undefined),
            retryInteractionEffect: vi.fn().mockResolvedValue(undefined),
            expireInteractionProposals: vi.fn().mockResolvedValue({
                conversation_id: 'conversation-1',
                branch_id: 'branch-1',
                current_state_revision: 7,
                expired_proposals: [],
                has_more_expired: false,
            }),
            listInteractionProposals: vi.fn().mockResolvedValue([pending]),
            listReopenInteractionEffects: vi.fn().mockResolvedValue({
                current_state_revision: 7,
                items: [],
                older_cursor: null,
            }),
            submitInteractionChoice: vi.fn(),
            decideInteractionProposal,
        } as unknown as LorepiaClient;
        const { controller } = renderChat(chatReadyState(), interactionClient);

        expect(await screen.findByText('문을 열기')).toBeInTheDocument();
        await fireEvent.click(screen.getByRole('button', { name: '승인' }));
        await waitFor(() => {
            expect(decideInteractionProposal).toHaveBeenCalledWith({
                conversation_id: 'conversation-1',
                branch_id: 'branch-1',
                proposal_record_id: 'proposal-room-1',
                expected_state_revision: 7,
                expected_proposal_revision: 3,
                decision: 'approve',
            });
        });
        await waitFor(() => expect(screen.queryByText('문을 열기')).not.toBeInTheDocument());
        expect(screen.getByRole('status')).toHaveTextContent('제안을 승인했습니다');
        controller.destroy();
    });

    it('requires explicit bounded expiry draining before ordinary proposal approval', async () => {
        const pending = {
            conversation_id: 'conversation-1',
            branch_id: 'branch-1',
            state_revision: 7,
            proposal_revision: 3,
            proposal: {
                id: 'proposal-pending',
                title: '상태 변경 승인',
                body: '합성 상태만 변경합니다.',
                status: 'pending' as const,
                source_interaction_state_revision: 7,
                requested_at_epoch_seconds: 1,
                expires_at_epoch_seconds: null,
                decided_at_epoch_seconds: null,
            },
        };
        const expireInteractionProposals = vi
            .fn()
            .mockResolvedValueOnce({
                conversation_id: 'conversation-1',
                branch_id: 'branch-1',
                current_state_revision: 7,
                expired_proposals: [],
                has_more_expired: true,
            })
            .mockResolvedValueOnce({
                conversation_id: 'conversation-1',
                branch_id: 'branch-1',
                current_state_revision: 7,
                expired_proposals: [],
                has_more_expired: false,
            });
        const decideInteractionProposal = vi.fn();
        const interactionClient = {
            listInteractionEffects: vi.fn().mockResolvedValue([]),
            subscribeInteractionEffects: vi.fn().mockResolvedValue(vi.fn()),
            acknowledgeInteractionEffect: vi.fn().mockResolvedValue(undefined),
            retryInteractionEffect: vi.fn().mockResolvedValue(undefined),
            expireInteractionProposals,
            listInteractionProposals: vi.fn().mockResolvedValue([pending]),
            listReopenInteractionEffects: vi.fn().mockResolvedValue({
                current_state_revision: 7,
                items: [],
                older_cursor: null,
            }),
            submitInteractionChoice: vi.fn(),
            decideInteractionProposal,
        } as unknown as LorepiaClient;
        const { controller } = renderChat(chatReadyState(), interactionClient);

        expect(await screen.findByText('상태 변경 승인')).toBeInTheDocument();
        expect(screen.getByRole('button', { name: '승인' })).toBeDisabled();
        expect(screen.getByRole('alert')).toHaveTextContent(
            '최신 상태를 모두 정리하기 전에는 다른 제안을 결정할 수 없습니다',
        );
        await fireEvent.click(screen.getByRole('button', { name: '만료 제안 계속 정리' }));
        await waitFor(() => expect(expireInteractionProposals).toHaveBeenCalledTimes(2));
        await waitFor(() => expect(screen.getByRole('button', { name: '승인' })).toBeEnabled());
        expect(decideInteractionProposal).not.toHaveBeenCalled();
        controller.destroy();
    });

    it('exposes explicit edit, regenerate, branch, remove and clipboard actions', async () => {
        const appState = chatReadyState();
        appState.messages.items = [
            {
                id: 'user-1',
                conversation_id: 'conversation-1',
                parent_id: null,
                role: 'user',
                content: '원래 문장',
                status: 'complete',
                generation_id: null,
                created_at: '2026-08-02T00:00:00Z',
            },
            {
                id: 'assistant-1',
                conversation_id: 'conversation-1',
                parent_id: 'user-1',
                role: 'assistant',
                content: '원래 응답',
                status: 'complete',
                generation_id: 'generation-old',
                created_at: '2026-08-02T00:00:01Z',
            },
        ];
        const { controller } = renderChat(appState);
        const edit = vi.spyOn(controller, 'editUserMessage').mockResolvedValue(true);
        const regenerate = vi
            .spyOn(controller, 'regenerateAssistantMessage')
            .mockResolvedValue(true);
        const createBranch = vi.spyOn(controller, 'createBranch').mockResolvedValue();
        const remove = vi.spyOn(controller, 'removeMessage').mockResolvedValue({
            mutationCommitted: true,
            messagesRefreshed: true,
            scopeKey: 'conversation-1:branch-1',
        });
        const writeText = vi.fn().mockResolvedValue(undefined);
        Object.defineProperty(navigator, 'clipboard', {
            configurable: true,
            value: { writeText },
        });

        expect(writeText).not.toHaveBeenCalled();
        const firstCopyButton = screen.getAllByRole('button', { name: '복사' }).at(0);
        if (firstCopyButton === undefined) throw new Error('copy action missing');
        expect(firstCopyButton.querySelector('svg')).not.toBeNull();
        await fireEvent.click(firstCopyButton);
        expect(writeText).toHaveBeenCalledWith('원래 문장');

        const editButton = screen.getByRole('button', { name: '편집' });
        expect(editButton.querySelector('svg')).not.toBeNull();
        await fireEvent.click(editButton);
        const editor = screen.getByRole('textbox', { name: '편집할 메시지' });
        await fireEvent.input(editor, { target: { value: '고친 문장' } });
        await fireEvent.click(screen.getByRole('button', { name: '새 분기로 저장' }));
        await waitFor(() => {
            expect(edit).toHaveBeenCalledWith('user-1', '고친 문장');
        });

        const regenerateButton = screen.getByRole('button', { name: '재생성' });
        expect(regenerateButton.querySelector('svg')).not.toBeNull();
        await fireEvent.click(regenerateButton);
        expect(regenerate).toHaveBeenCalledWith('assistant-1');

        const firstBranchButton = screen.getAllByRole('button', { name: '여기서 분기' }).at(0);
        if (firstBranchButton === undefined) throw new Error('branch action missing');
        expect(firstBranchButton.querySelector('svg')).not.toBeNull();
        await fireEvent.click(firstBranchButton);
        expect(createBranch).toHaveBeenCalledWith('user-1');

        const firstRemoveButton = screen.getAllByRole('button', { name: '여기부터 제거' }).at(0);
        if (firstRemoveButton === undefined) throw new Error('remove action missing');
        expect(firstRemoveButton.querySelector('svg')).not.toBeNull();
        await fireEvent.click(firstRemoveButton);
        const confirmRemoveButton = screen.getByRole('button', { name: '제거 확인' });
        expect(confirmRemoveButton.querySelector('svg')).not.toBeNull();
        expect(screen.getByRole('button', { name: '취소' }).querySelector('svg')).not.toBeNull();
        await fireEvent.click(confirmRemoveButton);
        expect(remove).toHaveBeenCalledWith('user-1');
        controller.destroy();
    });
});
