import { cleanup, fireEvent, render, screen, waitFor, within } from '@testing-library/svelte';
import { get } from 'svelte/store';
import { afterEach, describe, expect, it, vi, type MockInstance } from 'vitest';

import type {
    ChatEventKindDto,
    ChatStreamItemDto,
    LorepiaClient,
    MessageDto,
} from '../../lib/ipc/contracts';
import { INITIAL_APP_STATE, LorepiaAppController } from '../../app/app-controller';
import '../../styles/app.css';
import ChatPane from './ChatPane.svelte';
import { chatReadyState } from './tests/chat-pane-state-builder';

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

describe('ChatPane empty state', () => {
    it('uses an open canvas instead of a full-pane dashed placeholder frame', () => {
        const controller = new LorepiaAppController({} as LorepiaClient);
        const rendered = render(ChatPane, {
            appState: structuredClone(INITIAL_APP_STATE),
            controller,
        });
        const placeholder = rendered.container.querySelector<HTMLElement>('.chat-placeholder');
        if (placeholder === null) throw new Error('chat placeholder is missing');

        expect(screen.getByRole('heading', { name: '채팅' })).toBeInTheDocument();
        expect(screen.queryByText(/로컬 Core/)).not.toBeInTheDocument();
        expect(
            screen.queryByText('캐릭터와 대화를 선택하면 이어서 이야기할 수 있어요.'),
        ).not.toBeInTheDocument();
        expect(getComputedStyle(placeholder).borderStyle).toBe('none');
        expect(getComputedStyle(placeholder).borderWidth).toBe('0px');
        controller.destroy();
    });
});

describe('ChatPane error notices', () => {
    it('floats persistent chat errors inside the canvas and lets the user dismiss them', async () => {
        const appState = chatReadyState();
        appState.chat = {
            ...appState.chat,
            phase: 'error',
            error: '생성 설정을 적용하지 못했습니다.',
        };
        const controller = new LorepiaAppController({} as LorepiaClient);
        const rendered = render(ChatPane, { appState, controller, desktop: true });

        const region = screen.getByRole('region', { name: '채팅 오류 알림' });
        expect(region.closest('.chat-pane')).toBeInTheDocument();
        expect(within(region).getByRole('alert')).toHaveTextContent(
            '생성 설정을 적용하지 못했습니다.',
        );
        expect(rendered.container.querySelector('.chat-pane > .state-panel.error')).toBeNull();

        await fireEvent.click(screen.getByRole('button', { name: '채팅 오류 닫기' }));
        expect(screen.queryByRole('region', { name: '채팅 오류 알림' })).not.toBeInTheDocument();
        controller.destroy();
    });
});

describe('ChatPane transcript chrome', () => {
    it('exposes the selected conversation mode for mode-specific transcript styling', async () => {
        const appState = chatReadyState();
        appState.messages.items = [
            {
                id: 'assistant-mode-style',
                conversation_id: 'conversation-1',
                parent_id: null,
                role: 'assistant',
                content: '모드에 따라 표시 방식을 바꿔 주세요.',
                status: 'complete',
                generation_id: 'generation-1',
                created_at: '2026-08-03T19:47:00',
            },
        ];
        const controller = new LorepiaAppController({} as LorepiaClient);
        const rendered = render(ChatPane, { appState, controller, desktop: true });
        const pane = screen.getByRole('region', { name: '첫 대화' });

        expect(pane).toHaveAttribute('data-conversation-mode', 'chat');

        const storyState = structuredClone(appState);
        if (storyState.conversation_state === null) {
            throw new Error('conversation state missing');
        }
        storyState.conversation_state.selected_mode = 'story';

        await rendered.rerender({
            appState: storyState,
            controller,
            desktop: true,
        });

        expect(pane).toHaveAttribute('data-conversation-mode', 'story');
        controller.destroy();
    });

    it('renders calendar-day separators and a time for every persisted message', () => {
        const appState = chatReadyState();
        appState.messages.items = [
            {
                id: 'user-day-one',
                conversation_id: 'conversation-1',
                parent_id: null,
                role: 'user',
                content: '첫날 메시지',
                status: 'complete',
                generation_id: null,
                created_at: '2026-08-02T09:14:00',
            },
            {
                id: 'assistant-day-two',
                conversation_id: 'conversation-1',
                parent_id: 'user-day-one',
                role: 'assistant',
                content: '다음날 메시지',
                status: 'complete',
                generation_id: 'generation-1',
                created_at: '2026-08-03T19:47:00',
            },
        ];

        const controller = new LorepiaAppController({} as LorepiaClient);
        const rendered = render(ChatPane, { appState, controller });
        const separators = screen.getAllByRole('separator');
        const messageTimes = rendered.container.querySelectorAll<HTMLTimeElement>('.message-time');

        expect(separators).toHaveLength(2);
        expect(separators[0]).toHaveTextContent('2026년 8월 2일');
        expect(separators[1]).toHaveTextContent('2026년 8월 3일');
        expect(messageTimes).toHaveLength(2);
        expect(messageTimes[0]).toHaveAttribute('datetime', '2026-08-02T09:14:00');
        expect(messageTimes[0]).toHaveTextContent('09:14');
        expect(messageTimes[1]).toHaveTextContent('19:47');
        controller.destroy();
    });

    it('keeps each original day pill in the message flow so CSS can pin it without a duplicate', () => {
        const appState = chatReadyState();
        appState.messages.items = [
            {
                id: 'thursday-one',
                conversation_id: 'conversation-1',
                parent_id: null,
                role: 'assistant',
                content: '목요일 첫 메시지',
                status: 'complete',
                generation_id: 'generation-thursday-one',
                created_at: '2026-08-20T19:00:00',
            },
            {
                id: 'thursday-two',
                conversation_id: 'conversation-1',
                parent_id: 'thursday-one',
                role: 'user',
                content: '목요일 두 번째 메시지',
                status: 'complete',
                generation_id: null,
                created_at: '2026-08-20T19:02:00',
            },
            {
                id: 'monday-one',
                conversation_id: 'conversation-1',
                parent_id: 'thursday-two',
                role: 'assistant',
                content: '월요일 첫 메시지',
                status: 'complete',
                generation_id: 'generation-monday-one',
                created_at: '2026-08-24T21:37:00',
            },
        ];

        const { controller } = renderChat(appState);
        const thursdayOne = document.querySelector<HTMLElement>('[data-message-id="thursday-one"]');
        const thursdayTwo = document.querySelector<HTMLElement>('[data-message-id="thursday-two"]');
        const mondayOne = document.querySelector<HTMLElement>('[data-message-id="monday-one"]');
        const thursdayDivider = document.querySelector<HTMLElement>(
            '[data-message-day-divider="2026-08-20"]',
        );
        const mondayDivider = document.querySelector<HTMLElement>(
            '[data-message-day-divider="2026-08-24"]',
        );
        if (
            thursdayOne === null ||
            thursdayTwo === null ||
            mondayOne === null ||
            thursdayDivider === null ||
            mondayDivider === null
        ) {
            throw new Error('date divider fixture is incomplete');
        }

        expect(document.querySelector('.message-date-follower')).not.toBeInTheDocument();
        expect(thursdayDivider.nextElementSibling).toBe(thursdayOne);
        expect(thursdayOne.nextElementSibling).toBe(thursdayTwo);
        expect(mondayDivider.nextElementSibling).toBe(mondayOne);
        expect(screen.getAllByRole('separator')).toHaveLength(2);
        expect(screen.getAllByText('2026년 8월 20일 목요일')).toHaveLength(1);
        expect(screen.getAllByText('2026년 8월 24일 월요일')).toHaveLength(1);
        controller.destroy();
    });
});

describe('ChatPane live response', () => {
    it('suppresses only the live pending checkpoint and keeps an empty snapshot visibly active', async () => {
        const appState = chatReadyState();
        appState.messages.items = [
            {
                id: 'pending-assistant',
                conversation_id: 'conversation-1',
                parent_id: null,
                role: 'assistant',
                content: '저장된 체크포인트',
                status: 'pending',
                generation_id: 'generation-1',
                created_at: '2026-08-02T00:00:00Z',
            },
        ];
        const controller = new LorepiaAppController({} as LorepiaClient);
        const rendered = render(ChatPane, { appState, controller });

        expect(screen.getByText('저장된 체크포인트')).toBeInTheDocument();
        await rendered.rerender({
            appState: {
                ...appState,
                chat: {
                    ...appState.chat,
                    active_generation_id: 'generation-1',
                    live_assistant_message_id: 'pending-assistant',
                    streaming_text: '권위 있는 전체 응답',
                },
            },
            controller,
        });

        expect(screen.queryByText('저장된 체크포인트')).not.toBeInTheDocument();
        expect(screen.getAllByText('권위 있는 전체 응답')).toHaveLength(1);
        await rendered.rerender({
            appState: {
                ...appState,
                chat: {
                    ...appState.chat,
                    active_generation_id: 'generation-1',
                    live_assistant_message_id: 'pending-assistant',
                    streaming_text: '',
                    reasoning_text: '',
                },
            },
            controller,
        });

        expect(screen.getByLabelText('생성 중인 응답')).toBeInTheDocument();
        expect(screen.queryByText('저장된 체크포인트')).not.toBeInTheDocument();
        controller.destroy();
    });

    it('routes reasoning and answer deltas through the controller into separate labeled UI', async () => {
        const initialState = chatReadyState();
        const selectedConversation = initialState.selected_conversation;
        const conversationState = initialState.conversation_state;
        if (selectedConversation === null || conversationState === null) {
            throw new Error('synthetic chat fixture is incomplete');
        }
        const branch = {
            id: conversationState.active_branch_id,
            conversation_id: selectedConversation.id,
            title: null,
            fork_message_id: null,
            head_message_id: null,
            created_at: '2026-08-02T00:00:00Z',
            updated_at: '2026-08-02T00:00:00Z',
        };
        let onItem: ((item: ChatStreamItemDto) => void) | null = null;
        const client = {
            disposeChatStream: vi.fn().mockResolvedValue(false),
            openExistingConversation: vi.fn().mockResolvedValue(selectedConversation),
            getConversationState: vi.fn().mockResolvedValue(conversationState),
            listBranches: vi.fn().mockResolvedValue([branch]),
            listBranchMessages: vi.fn().mockResolvedValue([]),
            listRetryableMemoryQueryEmbeddings: vi.fn().mockResolvedValue([]),
            listInterruptedMemoryJobs: vi.fn().mockResolvedValue([]),
            sendMessage: vi.fn(
                (
                    _input: unknown,
                    _streamId: string,
                    listener: (item: ChatStreamItemDto) => void,
                ) => {
                    onItem = listener;
                    return Promise.resolve({ generation_id: 'generation-1' });
                },
            ),
        } as unknown as LorepiaClient;
        const controller = new LorepiaAppController(client);
        expect(await controller.selectConversation(selectedConversation)).toBe(true);
        controller.setRoomGenerationTarget(selectedConversation.id, branch.id, {
            model_route_id: 'route-1',
            generation_preset_id: 'preset-1',
        });
        const rendered = render(ChatPane, { appState: get(controller.state), controller });

        expect(await controller.sendMessage('안녕')).toBe(true);
        const listener = onItem as unknown as (item: ChatStreamItemDto) => void;
        const event = (sequence: number, kind: ChatEventKindDto): ChatStreamItemDto => ({
            type: 'event',
            payload: {
                event_version: 4,
                generation_id: 'generation-1',
                conversation_id: selectedConversation.id,
                branch_id: branch.id,
                assistant_message_id: 'message-1',
                sequence,
                emitted_at: '2026-08-02T00:00:00Z',
                kind,
            },
        });

        listener(
            event(1, {
                type: 'reasoning_delta',
                payload: '먼저 등장인물의 상황을 확인합니다.',
            }),
        );
        await vi.waitFor(() =>
            expect(get(controller.state).chat).toMatchObject({
                reasoning_text: '먼저 등장인물의 상황을 확인합니다.',
                streaming_text: '',
            }),
        );
        await rendered.rerender({ appState: get(controller.state) });

        const reasoningRegion = screen.getByText('추론 과정').closest('details');
        expect(reasoningRegion).toHaveAttribute('open');
        expect(reasoningRegion).toHaveTextContent('먼저 등장인물의 상황을 확인합니다.');
        expect(screen.queryByRole('region', { name: '생성 중인 답변' })).not.toBeInTheDocument();

        listener(
            event(2, {
                type: 'text_delta',
                payload: '라온은 조심스럽게 문을 열었다.',
            }),
        );
        await vi.waitFor(() =>
            expect(get(controller.state).chat).toMatchObject({
                reasoning_text: '먼저 등장인물의 상황을 확인합니다.',
                streaming_text: '라온은 조심스럽게 문을 열었다.',
            }),
        );
        await rendered.rerender({ appState: get(controller.state) });

        const answerRegion = screen.getByRole('region', { name: '생성 중인 답변' });
        expect(reasoningRegion).not.toHaveTextContent('라온은 조심스럽게 문을 열었다.');
        expect(answerRegion).toHaveTextContent('라온은 조심스럽게 문을 열었다.');
        expect(answerRegion).not.toHaveTextContent('먼저 등장인물의 상황을 확인합니다.');
        controller.destroy();
    });

    it('renders a labeled, collapsible reasoning-only stream instead of the empty state', () => {
        const appState = chatReadyState();
        appState.chat.reasoning_text = '응답의 근거를 정리하는 중입니다.';

        const { controller } = renderChat(appState);

        expect(screen.queryByText('새로운 이야기의 첫 문장을 보내보세요.')).not.toBeInTheDocument();
        const reasoningLabel = screen.getByText('추론 과정');
        const reasoningRegion = reasoningLabel.closest('details');
        expect(reasoningRegion).toHaveAttribute('open');
        expect(reasoningRegion).toHaveTextContent('응답의 근거를 정리하는 중입니다.');
        controller.destroy();
    });

    it('keeps reasoning separate from the streamed answer', () => {
        const appState = chatReadyState();
        appState.chat.reasoning_text = '먼저 등장인물의 상황을 확인합니다.';
        appState.chat.streaming_text = '라온은 조심스럽게 문을 열었다.';

        const { controller } = renderChat(appState);

        const reasoningRegion = screen.getByText('추론 과정').closest('details');
        const answerRegion = screen.getByRole('region', { name: '생성 중인 답변' });
        expect(reasoningRegion).toHaveTextContent('먼저 등장인물의 상황을 확인합니다.');
        expect(reasoningRegion).not.toContainElement(answerRegion);
        expect(answerRegion).toHaveTextContent('라온은 조심스럽게 문을 열었다.');
        controller.destroy();
    });

    it('announces bounded reasoning and answer phases without replaying generated content', async () => {
        const appState = chatReadyState();
        appState.chat = {
            ...appState.chat,
            active_generation_id: 'generation-1',
            live_assistant_message_id: 'pending-assistant',
            reasoning_text: '응답을 작성하기 전에 내부 추론을 정리합니다.',
        };
        appState.messages.items = [
            {
                id: 'pending-assistant',
                conversation_id: 'conversation-1',
                parent_id: null,
                role: 'assistant',
                content: '',
                status: 'pending',
                generation_id: 'generation-1',
                created_at: '2026-08-02T00:00:00Z',
            },
        ];
        const controller = new LorepiaAppController({} as LorepiaClient);
        const rendered = render(ChatPane, { appState, controller });

        const responseStatus = await screen.findByLabelText('응답 생성 상태');
        expect(responseStatus).toHaveAttribute('aria-live', 'polite');
        expect(responseStatus).toHaveAttribute('aria-atomic', 'true');
        expect(responseStatus).toHaveTextContent('응답의 추론을 생성하고 있습니다.');
        expect(responseStatus).not.toHaveTextContent(appState.chat.reasoning_text);

        const answer = '라온은 조심스럽게 문을 열었다.';
        await rendered.rerender({
            appState: {
                ...appState,
                chat: { ...appState.chat, streaming_text: answer },
            },
            controller,
        });

        await waitFor(() =>
            expect(responseStatus).toHaveTextContent('응답 본문 생성을 시작했습니다.'),
        );
        expect(responseStatus).not.toHaveTextContent(answer);
        controller.destroy();
    });

    it('announces a terminal durable assistant replacement exactly once', async () => {
        const appState = chatReadyState();
        appState.chat = {
            ...appState.chat,
            active_generation_id: 'generation-1',
            live_assistant_message_id: 'pending-assistant',
            streaming_text: '내린 비를 바라봅니다.',
            reasoning_text: '날씨를 확인합니다.',
        };
        const pendingMessage: MessageDto = {
            id: 'pending-assistant',
            conversation_id: 'conversation-1',
            parent_id: null,
            role: 'assistant',
            content: '내린 비를 바라봅니다.',
            status: 'pending',
            generation_id: 'generation-1',
            created_at: '2026-08-02T00:00:00Z',
        };
        appState.messages.items = [pendingMessage];
        const controller = new LorepiaAppController({} as LorepiaClient);
        const rendered = render(ChatPane, { appState, controller });
        const responseStatus = await screen.findByLabelText('응답 생성 상태');
        await waitFor(() =>
            expect(responseStatus).toHaveTextContent('응답 본문 생성을 시작했습니다.'),
        );

        const completedMessage: MessageDto = {
            ...pendingMessage,
            content: '내린 비를 바봅니다.',
            status: 'complete',
        };
        await rendered.rerender({
            appState: {
                ...appState,
                messages: { phase: 'ready', error: null, items: [completedMessage] },
                chat: {
                    ...appState.chat,
                    phase: 'idle',
                    active_generation_id: null,
                    live_assistant_message_id: null,
                    streaming_text: '',
                    reasoning_text: '',
                },
            },
            controller,
        });

        await waitFor(() => expect(responseStatus).toHaveTextContent('응답 생성이 완료됐습니다.'));
        expect(screen.getAllByText('응답 생성이 완료됐습니다.')).toHaveLength(1);
        expect(responseStatus).not.toHaveTextContent(completedMessage.content);
        controller.destroy();
    });
});

describe('ChatPane composer', () => {
    it('surfaces a blocked generation reattachment and keeps new sends unavailable', () => {
        const appState = chatReadyState();
        appState.chat = {
            ...appState.chat,
            phase: 'error',
            error: '진행 중이던 응답 스트림에 다시 연결할 수 없습니다.',
            active_generation_id: 'generation-1',
        };

        const { controller } = renderChat(appState);

        expect(screen.getByRole('alert')).toHaveTextContent(
            '진행 중이던 응답 스트림에 다시 연결할 수 없습니다.',
        );
        expect(screen.getByRole('textbox', { name: '메시지' })).toBeDisabled();
        expect(screen.getByRole('button', { name: '응답 생성 취소' })).toBeInTheDocument();
        expect(screen.queryByRole('button', { name: '메시지 보내기' })).not.toBeInTheDocument();
        controller.destroy();
    });

    it('keeps the no-result resubmission instruction visible after a retry receipt', () => {
        const appState = chatReadyState();
        appState.memory_query_retries.notice =
            '임베딩 준비만 다시 대기열에 넣었습니다. 미리보기나 메시지 결과는 만들지 않았습니다. 원래 계획 미리보기 또는 메시지 전송·편집·재생성을 다시 실행하세요.';

        const { controller } = renderChat(appState);

        expect(screen.getByRole('status')).toHaveTextContent(
            '미리보기나 메시지 결과는 만들지 않았습니다',
        );
        expect(screen.getByRole('status')).toHaveTextContent(
            '계획 미리보기 또는 메시지 전송·편집·재생성',
        );
        controller.destroy();
    });
});
