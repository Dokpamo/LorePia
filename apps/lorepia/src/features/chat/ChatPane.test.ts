import { cleanup, fireEvent, render, screen, waitFor, within } from '@testing-library/svelte';
import { tick } from 'svelte';
import { get } from 'svelte/store';
import { afterEach, describe, expect, it, vi, type MockInstance } from 'vitest';

import type {
    CharacterRenderProfileDto,
    ChatEventKindDto,
    ChatStreamItemDto,
    LorepiaClient,
    MessageDto,
} from '../../lib/ipc/contracts';
import {
    INITIAL_APP_STATE,
    LorepiaAppController,
    type LorepiaAppState,
} from '../../app/app-controller';
import '../../styles/app.css';
import {
    INITIAL_ORCHESTRATION_STATE,
    OrchestrationController,
} from '../orchestration/orchestration-controller';
import ChatPane from './ChatPane.svelte';
import { PortableCharacterRuntime } from './portable-runtime';

class ControlledResizeObserver implements ResizeObserver {
    static instances: ControlledResizeObserver[] = [];

    readonly observed = new Set<Element>();

    constructor(private readonly callback: ResizeObserverCallback) {
        ControlledResizeObserver.instances.push(this);
    }

    static reset(): void {
        ControlledResizeObserver.instances = [];
    }

    static observing(target: Element): ControlledResizeObserver | undefined {
        return ControlledResizeObserver.instances.find((observer) => observer.observed.has(target));
    }

    observe(target: Element): void {
        this.observed.add(target);
    }

    unobserve(target: Element): void {
        this.observed.delete(target);
    }

    disconnect(): void {
        this.observed.clear();
    }

    emit(target: Element, width: number, height: number): void {
        const contentRect = {
            x: 0,
            y: 0,
            width,
            height,
            top: 0,
            right: width,
            bottom: height,
            left: 0,
            toJSON: () => ({}),
        };
        this.callback(
            [
                {
                    target,
                    contentRect,
                    borderBoxSize: [{ inlineSize: width, blockSize: height }],
                    contentBoxSize: [{ inlineSize: width, blockSize: height }],
                    devicePixelContentBoxSize: [],
                },
            ],
            this,
        );
    }
}

afterEach(() => {
    cleanup();
    vi.restoreAllMocks();
    vi.unstubAllGlobals();
    ControlledResizeObserver.reset();
});

function chatReadyState(): LorepiaAppState {
    return {
        ...structuredClone(INITIAL_APP_STATE),
        selected_character: {
            id: 'character-1',
            name: '라온',
            description: '',
            source_hash: 'synthetic',
            avatar_asset_id: null,
            created_at: '2026-08-02T00:00:00Z',
        },
        selected_conversation: {
            id: 'conversation-1',
            character_id: 'character-1',
            title: '첫 대화',
            created_at: '2026-08-02T00:00:00Z',
            updated_at: '2026-08-02T00:00:00Z',
        },
        conversation_state: {
            conversation_id: 'conversation-1',
            active_branch_id: 'branch-1',
            selected_mode: 'chat',
            updated_at: '2026-08-02T00:00:00Z',
        },
        messages: { phase: 'ready', error: null, items: [] },
    };
}

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

        await fireEvent.click(screen.getByRole('button', { name: '대화 설정' }));
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

    it('uses a compact empty input that expands for writing and reveals send after text', async () => {
        const { controller } = renderChat();
        const composer = screen.getByRole('form', { name: '메시지 작성' });
        const field = composer.querySelector('.composer-field');
        const textRegion = composer.querySelector('.composer-text-region');
        const actionRow = composer.querySelector('.composer-action-row');
        const textbox = screen.getByRole('textbox', { name: '메시지' });

        expect(field).not.toBeNull();
        expect(textRegion).not.toBeNull();
        expect(actionRow).not.toBeNull();
        expect(textRegion).toContainElement(textbox);
        expect(textbox).not.toHaveAttribute('placeholder');
        expect(screen.queryByRole('button', { name: '메시지 보내기' })).not.toBeInTheDocument();
        expect(screen.queryByRole('button', { name: '전체화면으로 작성' })).not.toBeInTheDocument();
        const dormantExpand = composer.querySelector<HTMLButtonElement>('.composer-expand-action');
        expect(dormantExpand).toBeDisabled();
        expect(dormantExpand).toHaveAttribute('aria-hidden', 'true');

        Object.defineProperty(textbox, 'scrollHeight', { configurable: true, value: 22 });
        await fireEvent.focus(textbox);
        expect(field).toHaveClass('expanded');
        await waitFor(() => expect(field).toHaveStyle('--composer-text-size: 31px'));
        await fireEvent.input(textbox, { target: { value: '전송할 메시지' } });
        const send = screen.getByRole('button', { name: '메시지 보내기' });
        expect(field).toHaveClass('has-draft');
        expect(actionRow).toContainElement(send);
        await waitFor(() => expect(field).toHaveStyle('--composer-text-size: 31px'));
        expect(send).toHaveClass('send-button');
        expect(send.querySelector('svg')).not.toBeNull();
        expect(screen.queryByRole('button', { name: '전체화면으로 작성' })).not.toBeInTheDocument();

        Object.defineProperty(textbox, 'scrollHeight', { configurable: true, value: 42 });
        await fireEvent.input(textbox, { target: { value: '두 줄로 접힌 전송할 메시지' } });
        const expand = await screen.findByRole('button', { name: '전체화면으로 작성' });
        await waitFor(() => expect(field).toHaveStyle('--composer-text-size: 42px'));
        expect(field).toHaveClass('can-fullscreen');
        expect(actionRow).toContainElement(expand);
        expect(expand).toHaveClass('available');
        expect(expand).toBeEnabled();
        expect(expand).toHaveAttribute('aria-hidden', 'false');
        expect(expand.nextElementSibling).toBe(send);

        await fireEvent.blur(textbox);
        expect(textbox).not.toHaveFocus();
        expect(field).toHaveClass('expanded');
        expect(field).toHaveClass('has-draft');

        await fireEvent.input(textbox, { target: { value: '' } });
        await fireEvent.focus(textbox);
        await fireEvent.blur(textbox);
        expect(field).not.toHaveClass('expanded');
        controller.destroy();
    });

    it('starts and remains expanded in the desktop workspace', async () => {
        const appState = chatReadyState();
        const controller = new LorepiaAppController({} as LorepiaClient);
        const rendered = render(ChatPane, { appState, controller, desktop: true });
        const field = rendered.container.querySelector('.composer-field');
        const textbox = screen.getByRole('textbox', { name: '메시지' });
        const send = screen.getByRole('button', { name: '메시지 보내기' });

        await waitFor(() => expect(field).toHaveClass('expanded'));
        expect(textbox).toHaveAttribute('placeholder', '무엇이든 요청하세요');
        expect(send).toBeDisabled();
        await fireEvent.focus(textbox);
        await fireEvent.blur(textbox);
        expect(field).toHaveClass('expanded');
        controller.destroy();
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

    it('measures line growth from the stable composer width without height feedback', async () => {
        vi.stubGlobal('ResizeObserver', ControlledResizeObserver);
        const { controller } = renderChat();
        const composer = screen.getByRole('form', { name: '메시지 작성' });
        const field = composer.querySelector<HTMLElement>('.composer-field');
        const chatPane = composer.closest<HTMLElement>('.chat-pane');
        const scroller = screen.getByLabelText('메시지 기록');
        const textbox = screen.getByRole<HTMLTextAreaElement>('textbox', { name: '메시지' });
        let measuredHeight = 42;

        expect(field).not.toBeNull();
        expect(chatPane).not.toBeNull();
        Object.defineProperty(textbox, 'scrollHeight', {
            configurable: true,
            get: () => measuredHeight,
        });
        Object.defineProperty(textbox, 'scrollTop', {
            configurable: true,
            writable: true,
            value: 16,
        });
        Object.defineProperties(scroller, {
            clientHeight: { configurable: true, value: 400 },
            scrollHeight: { configurable: true, value: 900 },
            scrollTop: { configurable: true, writable: true, value: 500 },
        });

        await fireEvent.input(textbox, { target: { value: '두 줄 높이의 초안' } });
        await waitFor(() => expect(field).toHaveStyle('--composer-text-size: 42px'));
        expect(textbox.scrollTop).toBe(0);

        const widthObserver = ControlledResizeObserver.observing(composer);
        expect(widthObserver).toBeDefined();
        if (field === null) throw new Error('composer field missing');
        if (widthObserver === undefined) throw new Error('composer resize observer missing');
        expect(ControlledResizeObserver.observing(field)).toBe(widthObserver);
        expect(ControlledResizeObserver.observing(textbox)).toBeUndefined();

        scroller.scrollTop = 0;
        await fireEvent.scroll(scroller);
        textbox.focus();
        await tick();
        expect(textbox).toHaveFocus();
        expect(field).toHaveClass('expanded');
        widthObserver.emit(field, 320, 90);
        await waitFor(() => expect(chatPane).toHaveStyle('--composer-overlay-height: 90px'));
        expect(scroller.scrollTop).toBe(900);

        measuredHeight = 64;
        widthObserver.emit(composer, 320, 90);
        await waitFor(() => expect(field).toHaveStyle('--composer-text-size: 64px'));

        measuredHeight = 86;
        widthObserver.emit(composer, 320, 120);
        await tick();
        expect(field).toHaveStyle('--composer-text-size: 64px');

        widthObserver.emit(composer, 300, 120);
        await waitFor(() => expect(field).toHaveStyle('--composer-text-size: 86px'));
        controller.destroy();
    });

    it('grows an open tool tray downward and moves it only by the composer height increase', async () => {
        vi.stubGlobal('ResizeObserver', ControlledResizeObserver);
        const appState = chatReadyState();
        appState.messages.items = [
            {
                id: 'anchored-message-actions',
                conversation_id: 'conversation-1',
                parent_id: null,
                role: 'assistant',
                content: '도구 위치를 유지해 주세요.',
                status: 'complete',
                generation_id: 'generation-1',
                created_at: '2026-08-03T19:47:00',
            },
        ];
        const { controller } = renderChat(appState);
        const scroller = screen.getByLabelText('메시지 기록');
        const message = screen.getByRole('article', { name: '캐릭터 메시지' });
        const messageRow = message.closest('.message-item');
        const textbox = screen.getByRole<HTMLTextAreaElement>('textbox', { name: '메시지' });
        const composer = screen.getByRole('form', { name: '메시지 작성' });
        const field = composer.querySelector<HTMLElement>('.composer-field');
        const chatPane = composer.closest<HTMLElement>('.chat-pane');

        if (field === null || chatPane === null || !(messageRow instanceof HTMLElement)) {
            throw new Error('chat or composer structure missing');
        }
        Object.defineProperty(textbox, 'scrollHeight', { configurable: true, value: 22 });
        Object.defineProperties(scroller, {
            clientHeight: { configurable: true, value: 400 },
            scrollHeight: { configurable: true, value: 900 },
            scrollTop: { configurable: true, writable: true, value: 120 },
        });
        chatPane.style.setProperty('--composer-overlay-height', '60px');

        message.focus();
        await waitFor(() => expect(messageRow).toHaveClass('actions-open'));
        await tick();
        const messageObserver = ControlledResizeObserver.observing(messageRow);
        if (messageObserver === undefined) throw new Error('message resize observer missing');
        scroller.scrollTop = 500;
        await fireEvent.scroll(scroller);
        messageObserver.emit(messageRow, 320, 140);
        await tick();
        await Promise.resolve();
        await tick();
        expect(scroller.scrollTop).toBe(500);

        scroller.scrollTop = 120;
        await fireEvent.scroll(scroller);
        textbox.focus();
        await tick();
        expect(messageRow).toHaveClass('actions-open');

        const resizeObserver = ControlledResizeObserver.observing(field);
        if (resizeObserver === undefined) throw new Error('composer resize observer missing');
        resizeObserver.emit(field, 320, 100);

        await waitFor(() => expect(chatPane).toHaveStyle('--composer-overlay-height: 100px'));
        expect(scroller.scrollTop).toBe(160);
        expect(messageRow).toHaveClass('actions-open');
        controller.destroy();
    });

    it('repins visible lines after WebKit applies its deferred caret scroll', async () => {
        const pendingFrames = new Map<number, FrameRequestCallback>();
        let nextFrameId = 1;
        vi.stubGlobal(
            'requestAnimationFrame',
            vi.fn((callback: FrameRequestCallback) => {
                const frameId = nextFrameId;
                nextFrameId += 1;
                pendingFrames.set(frameId, callback);
                return frameId;
            }),
        );
        vi.stubGlobal(
            'cancelAnimationFrame',
            vi.fn((frameId: number) => pendingFrames.delete(frameId)),
        );
        const { controller } = renderChat();
        const textbox = screen.getByRole<HTMLTextAreaElement>('textbox', { name: '메시지' });
        Object.defineProperty(textbox, 'scrollHeight', { configurable: true, value: 64 });
        Object.defineProperty(textbox, 'scrollTop', {
            configurable: true,
            writable: true,
            value: 0,
        });

        await fireEvent.input(textbox, { target: { value: '세 번째 줄이 막 생긴 초안' } });
        textbox.scrollTop = 2;

        const runNextFrame = (time: number): void => {
            const nextFrame = pendingFrames.entries().next().value;
            if (nextFrame === undefined) throw new Error('composer scroll anchor frame missing');
            pendingFrames.delete(nextFrame[0]);
            nextFrame[1](time);
        };

        runNextFrame(16);
        expect(textbox.scrollTop).toBe(0);
        textbox.scrollTop = 2;
        runNextFrame(32);
        expect(textbox.scrollTop).toBe(0);
        textbox.scrollTop = 2;
        runNextFrame(48);
        expect(textbox.scrollTop).toBe(0);
        expect(pendingFrames.size).toBe(0);
        controller.destroy();
    });

    it('offers a fullscreen editor only after the draft reaches the normal composer limit', async () => {
        const { controller } = renderChat();
        const composer = screen.getByRole('form', { name: '메시지 작성' });
        const field = composer.querySelector<HTMLElement>('.composer-field');
        const textRegion = composer.querySelector<HTMLElement>('.composer-text-region');
        const actionRow = composer.querySelector('.composer-action-row');
        const textbox = screen.getByRole('textbox', { name: '메시지' });
        if (textRegion === null) throw new Error('composer text region missing');
        textRegion.style.maxHeight = '72px';
        Object.defineProperty(textbox, 'scrollHeight', { configurable: true, value: 128 });
        Object.defineProperty(textbox, 'clientHeight', { configurable: true, value: 72 });

        await fireEvent.input(textbox, {
            target: { value: '길게 작성한 메시지가 일반 입력창의 최대 높이에 도달했습니다.' },
        });
        const expand = await screen.findByRole('button', { name: '전체화면으로 작성' });
        const send = screen.getByRole('button', { name: '메시지 보내기' });
        const add = screen.getByRole('button', { name: '추가' });
        const fullscreenBeforeOpen =
            document.querySelector<HTMLFormElement>('.composer-fullscreen');
        const fullscreenCloseBeforeOpen =
            fullscreenBeforeOpen?.querySelector<HTMLButtonElement>('.composer-fullscreen-close') ??
            null;
        const fullscreenSendBeforeOpen =
            fullscreenBeforeOpen?.querySelector<HTMLButtonElement>('.send-button') ?? null;
        const fullscreenTextboxBeforeOpen =
            fullscreenBeforeOpen?.querySelector<HTMLTextAreaElement>('#chat-draft-fullscreen') ??
            null;
        const fullscreenTextRegionBeforeOpen =
            fullscreenBeforeOpen?.querySelector<HTMLElement>('.composer-fullscreen-text-region') ??
            null;
        if (
            field === null ||
            fullscreenBeforeOpen === null ||
            fullscreenCloseBeforeOpen === null ||
            fullscreenSendBeforeOpen === null ||
            fullscreenTextboxBeforeOpen === null ||
            fullscreenTextRegionBeforeOpen === null
        ) {
            throw new Error('fullscreen morph surfaces missing');
        }
        vi.spyOn(field, 'getBoundingClientRect').mockReturnValue(
            DOMRect.fromRect({ x: 12, y: 600, width: 318, height: 120 }),
        );
        vi.spyOn(fullscreenBeforeOpen, 'getBoundingClientRect').mockReturnValue(
            DOMRect.fromRect({ x: 0, y: 64, width: 342, height: 677 }),
        );
        vi.spyOn(add, 'getBoundingClientRect').mockReturnValue(
            DOMRect.fromRect({ x: 20, y: 680, width: 38, height: 38 }),
        );
        vi.spyOn(send, 'getBoundingClientRect').mockReturnValue(
            DOMRect.fromRect({ x: 282, y: 680, width: 38, height: 38 }),
        );
        vi.spyOn(fullscreenCloseBeforeOpen, 'getBoundingClientRect').mockReturnValue(
            DOMRect.fromRect({ x: 12, y: 80, width: 42, height: 42 }),
        );
        vi.spyOn(fullscreenSendBeforeOpen, 'getBoundingClientRect').mockReturnValue(
            DOMRect.fromRect({ x: 288, y: 80, width: 42, height: 42 }),
        );
        textbox.style.padding = '8px 10px';
        textbox.style.fontSize = '15px';
        textbox.style.lineHeight = '21px';
        fullscreenTextboxBeforeOpen.style.padding = '12px 24px';
        vi.spyOn(textbox, 'getBoundingClientRect').mockReturnValue(
            DOMRect.fromRect({ x: 12, y: 610, width: 318, height: 72 }),
        );
        vi.spyOn(fullscreenTextboxBeforeOpen, 'getBoundingClientRect').mockReturnValue(
            DOMRect.fromRect({ x: 0, y: 130, width: 342, height: 611 }),
        );
        expect(field).toHaveClass('overflows');
        expect(actionRow).toContainElement(expand);
        expect(expand.nextElementSibling).toBe(send);
        await fireEvent.click(expand);

        const fullscreen = screen.getByRole('form', { name: '전체화면 메시지 작성' });
        const fullscreenTextbox = screen.getByRole('textbox', { name: '전체화면 메시지' });
        const fullscreenClose = screen.getByRole('button', { name: '전체화면 입력 닫기' });
        const fullscreenSend = screen.getAllByRole('button', { name: '메시지 보내기' }).at(-1);
        expect(composer).toHaveAttribute('aria-hidden', 'true');
        expect(fullscreen).toHaveClass('open');
        expect(fullscreen.style.getPropertyValue('--composer-origin-top')).toBe('536px');
        expect(fullscreen.style.getPropertyValue('--composer-origin-right')).toBe('12px');
        expect(fullscreen.style.getPropertyValue('--composer-origin-bottom')).toBe('21px');
        expect(fullscreen.style.getPropertyValue('--composer-origin-left')).toBe('12px');
        expect(fullscreenClose.style.getPropertyValue('--composer-control-origin-x')).toBe('6px');
        expect(fullscreenClose.style.getPropertyValue('--composer-control-origin-y')).toBe('598px');
        expect(fullscreenClose.querySelector('.lucide-minimize-2')).not.toBeNull();
        expect(fullscreenSend?.style.getPropertyValue('--composer-control-origin-x')).toBe('-8px');
        expect(fullscreenSend?.style.getPropertyValue('--composer-control-origin-y')).toBe('598px');
        expect(
            fullscreenTextRegionBeforeOpen.style.getPropertyValue('--composer-text-origin-x'),
        ).toBe('-2px');
        expect(
            fullscreenTextRegionBeforeOpen.style.getPropertyValue('--composer-text-origin-y'),
        ).toBe('476px');
        expect(
            fullscreenTextRegionBeforeOpen.style.getPropertyValue(
                '--composer-text-origin-font-size',
            ),
        ).toBe('15px');
        expect(
            fullscreenTextRegionBeforeOpen.style.getPropertyValue(
                '--composer-text-origin-line-height',
            ),
        ).toBe('21px');
        expect(fullscreenTextbox).toHaveValue(
            '길게 작성한 메시지가 일반 입력창의 최대 높이에 도달했습니다.',
        );
        expect(fullscreenTextbox).toHaveFocus();

        await fireEvent.click(fullscreenClose);
        expect(fullscreen).not.toHaveClass('open');
        expect(textbox).toHaveValue('길게 작성한 메시지가 일반 입력창의 최대 높이에 도달했습니다.');
        expect(textbox).toHaveFocus();
        controller.destroy();
    });

    it('uses a leading plus action that opens and focuses the writing surface', async () => {
        const { controller } = renderChat();
        const composer = screen.getByRole('textbox', { name: '메시지' });

        const add = screen.getByRole('button', { name: '추가' });
        expect(add.querySelector('svg')).not.toBeNull();
        await fireEvent.click(add);

        expect(composer).toHaveValue('');
        expect(composer).toHaveFocus();
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

    it('places a live response after the persisted virtual tail spacer', () => {
        const appState = chatReadyState();
        appState.messages.items = Array.from({ length: 160 }, (_, index) => ({
            id: `message-${String(index)}`,
            conversation_id: 'conversation-1',
            parent_id: index === 0 ? null : `message-${String(index - 1)}`,
            role: index % 2 === 0 ? ('user' as const) : ('assistant' as const),
            content: `synthetic-${String(index)}`,
            status: 'complete' as const,
            generation_id: null,
            created_at: '2026-08-02T00:00:00Z',
        }));
        appState.chat.streaming_text = '진행 중인 응답';

        const { controller } = renderChat(appState);
        const list = screen.getByRole('list', { name: '대화 메시지' });
        const live = screen.getByLabelText('생성 중인 응답').closest('li');

        expect(list).toHaveStyle({ paddingBottom: '22px' });
        expect(live).toHaveStyle({ marginTop: '15660px' });
        controller.destroy();
    });
});

describe('ChatPane composer', () => {
    it('discards a pending runtime approval when the selected character changes', async () => {
        const profileA: CharacterRenderProfileDto = {
            character_id: 'character-1',
            character_content_revision_id: 'revision-a',
            assets: [],
            background_markup: '<div>PROFILE-A</div>',
            toggle_schema: '',
            initial_variables: {},
            output_transforms: [],
            display_transforms: [],
            runtime_scripts: [],
            runtime_knowledge: [],
            runtime_script_count: 0,
        };
        const profileB: CharacterRenderProfileDto = {
            ...profileA,
            character_id: 'character-2',
            character_content_revision_id: 'revision-b',
            background_markup: '<div>PROFILE-B</div>',
            runtime_scripts: [
                {
                    id: 'script-b',
                    name: 'Runtime B',
                    event: 'load',
                    language: 'lua',
                    source: '-- must remain blocked',
                    elevated_access: true,
                },
            ],
            runtime_script_count: 1,
        };
        const getCharacterRenderProfile = vi.fn((characterId: string) =>
            Promise.resolve(characterId === 'character-1' ? profileA : profileB),
        );
        const client = { getCharacterRenderProfile } as unknown as LorepiaClient;
        const controller = new LorepiaAppController({} as LorepiaClient);
        const orchestrationController = new OrchestrationController({} as LorepiaClient);
        const appStateA = chatReadyState();
        const rendered = render(ChatPane, {
            appState: appStateA,
            controller,
            client,
            orchestrationState: {
                ...structuredClone(INITIAL_ORCHESTRATION_STATE),
                phase: 'ready',
            },
            orchestrationController,
        });

        await fireEvent.click(screen.getByRole('button', { name: '대화 설정' }));
        const approve = await screen.findByRole('button', {
            name: '선택한 기능만 이번 세션에서 허용',
        });
        let digestPending = false;
        let finishDigest = (value: ArrayBuffer): void => {
            throw new Error(
                `runtime grant digest did not start (${String(value.byteLength)} bytes)`,
            );
        };
        vi.spyOn(globalThis.crypto.subtle, 'digest').mockImplementation(
            () =>
                new Promise<ArrayBuffer>((resolve) => {
                    digestPending = true;
                    finishDigest = resolve;
                }),
        );
        await fireEvent.click(approve);
        await waitFor(() => expect(digestPending).toBe(true));

        const appStateB = chatReadyState();
        const conversationStateB = appStateB.conversation_state;
        if (
            appStateB.selected_character === null ||
            appStateB.selected_conversation === null ||
            conversationStateB === null
        ) {
            throw new Error('chat fixture is incomplete');
        }
        appStateB.selected_character = {
            ...appStateB.selected_character,
            id: 'character-2',
            name: '마루',
        };
        appStateB.selected_conversation = {
            ...appStateB.selected_conversation,
            id: 'conversation-2',
            character_id: 'character-2',
        };
        appStateB.conversation_state = {
            ...conversationStateB,
            conversation_id: 'conversation-2',
        };
        await rendered.rerender({ appState: appStateB });
        await screen.findByRole('checkbox', { name: '고급 카드 권한' });

        finishDigest(new Uint8Array(32).buffer);
        await tick();
        await waitFor(() =>
            expect(
                screen.getByRole('button', {
                    name: '선택한 기능만 이번 세션에서 허용',
                }),
            ).toBeInTheDocument(),
        );
        expect(screen.queryByRole('button', { name: '캐릭터 기능 권한 해제' })).toBeNull();
        expect(document.querySelector('.portable-runtime-background .portable-frame')).toBeNull();

        controller.destroy();
        orchestrationController.destroy();
    });

    it('does not expose chat text or indices to card UI when chat read is denied', async () => {
        const profile: CharacterRenderProfileDto = {
            character_id: 'character-1',
            character_content_revision_id: 'revision-1',
            assets: [],
            background_markup:
                '<button card-btn="safe{{lastcharmessage}}{{chat_index}}{{lastmessageid}}">Run</button>',
            toggle_schema: '',
            initial_variables: {},
            output_transforms: [],
            display_transforms: [
                {
                    pattern: '^(SECRET-CHAT-CONTENT)$',
                    replacement: '<button card-btn="$1">Continue</button>',
                    flags: '',
                },
            ],
            runtime_scripts: [],
            runtime_knowledge: [],
            runtime_script_count: 0,
        };
        const appState = chatReadyState();
        appState.messages.items = [
            {
                id: 'message-secret',
                conversation_id: 'conversation-1',
                parent_id: null,
                role: 'assistant',
                content: 'SECRET-CHAT-CONTENT',
                status: 'complete',
                generation_id: null,
                created_at: '2026-08-02T00:00:00Z',
            },
        ];
        const client = {
            getCharacterRenderProfile: vi.fn().mockResolvedValue(profile),
            resolveAssetDelivery: vi.fn(),
        } as unknown as LorepiaClient;
        const { controller, orchestrationController } = renderChatWithSettings(appState, client);

        await fireEvent.click(screen.getByRole('button', { name: '대화 설정' }));
        const chatRead = await screen.findByRole('checkbox', { name: '현재 대화 읽기' });
        expect(chatRead).toBeChecked();
        await fireEvent.click(chatRead);
        await fireEvent.click(
            screen.getByRole('button', { name: '선택한 기능만 이번 세션에서 허용' }),
        );

        await waitFor(() => {
            const frame = document.querySelector<HTMLIFrameElement>(
                '.portable-runtime-background .portable-frame',
            );
            expect(frame?.srcdoc).toContain('data-portable-action="safe"');
        });
        const frame = document.querySelector<HTMLIFrameElement>(
            '.portable-runtime-background .portable-frame',
        );
        expect(frame?.srcdoc).not.toContain('SECRET-CHAT-CONTENT');
        expect(frame?.srcdoc).not.toContain('data-portable-action="safe10"');
        expect(document.querySelectorAll('.portable-frame')).toHaveLength(1);
        expect(screen.getByText('SECRET-CHAT-CONTENT')).toBeInTheDocument();
        controller.destroy();
        orchestrationController.destroy();
    });

    it('keeps ordinary chat available while imported runtime code remains unapproved', async () => {
        const profile: CharacterRenderProfileDto = {
            character_id: 'character-1',
            character_content_revision_id: 'revision-1',
            assets: [],
            background_markup: '',
            toggle_schema: '',
            initial_variables: {},
            output_transforms: [],
            display_transforms: [],
            runtime_scripts: [
                {
                    id: 'script-1',
                    name: 'Runtime',
                    event: 'load',
                    language: 'lua',
                    source: '-- must remain inert',
                    elevated_access: false,
                },
            ],
            runtime_knowledge: [],
            runtime_script_count: 1,
        };
        const createRuntime = vi.spyOn(PortableCharacterRuntime, 'create');
        const client = {
            getCharacterRenderProfile: vi.fn().mockResolvedValue(profile),
        } as unknown as LorepiaClient;
        const { controller, sendMessage, orchestrationController } = renderChatWithSettings(
            chatReadyState(),
            client,
        );

        await fireEvent.click(screen.getByRole('button', { name: '대화 설정' }));
        await screen.findByRole('button', {
            name: '선택한 기능만 이번 세션에서 허용',
        });
        expect(screen.getByRole('checkbox', { name: '현재 기본 모델 호출' })).not.toBeChecked();
        expect(screen.getByRole('checkbox', { name: '선택한 보조 모델 호출' })).not.toBeChecked();
        const composer = screen.getByRole('textbox', { name: '메시지' });
        await fireEvent.input(composer, { target: { value: '안전 모드 대화' } });
        await fireEvent.click(screen.getByRole('button', { name: '메시지 보내기' }));

        await waitFor(() => expect(sendMessage).toHaveBeenCalledWith('안전 모드 대화'));
        expect(createRuntime).not.toHaveBeenCalled();
        controller.destroy();
        orchestrationController.destroy();
    });

    it('sends customized character runtime values without persisting them as room settings', async () => {
        const profile: CharacterRenderProfileDto = {
            character_id: 'character-1',
            character_content_revision_id: 'revision-1',
            assets: [],
            background_markup: '',
            toggle_schema: 'music=배경음악=toggle',
            initial_variables: { music: '0' },
            output_transforms: [],
            display_transforms: [],
            runtime_scripts: [
                {
                    id: 'script-1',
                    name: 'Runtime',
                    event: 'load',
                    language: 'lua',
                    source: '-- no-op',
                    elevated_access: false,
                },
            ],
            runtime_knowledge: [],
            runtime_script_count: 1,
        };
        let music = '0';
        const setOption = vi.fn((_key: string, value: string) => {
            music = value;
            return Promise.resolve();
        });
        const runtime = {
            toggles: [{ key: 'music', label: '배경음악', kind: 'toggle', choices: [] }],
            get generationVariables() {
                return { music };
            },
            get variables() {
                return { music };
            },
            backgroundMarkup: '',
            auxiliarySelection: null,
            optionValue: (key: string) => (key === 'music' ? music : ''),
            setOption,
            setAuxiliarySelection: vi.fn(),
            setMessages: vi.fn(),
            refreshDisplay: vi.fn().mockResolvedValue(undefined),
            prepareInput: vi.fn((text: string) => Promise.resolve({ text, shouldSend: true })),
            afterOutput: vi.fn().mockResolvedValue(undefined),
            handleAction: vi.fn().mockResolvedValue(undefined),
            displayText: (message: MessageDto) => message.content,
            effectiveText: (message: MessageDto) => message.content,
            close: vi.fn(),
        } as unknown as PortableCharacterRuntime;
        const createRuntime = vi
            .spyOn(PortableCharacterRuntime, 'create')
            .mockResolvedValue(runtime);
        const client = {
            getCharacterRenderProfile: vi.fn().mockResolvedValue(profile),
        } as unknown as LorepiaClient;
        const { controller, sendMessage, orchestrationController } = renderChatWithSettings(
            chatReadyState(),
            client,
        );

        await fireEvent.click(screen.getByRole('button', { name: '대화 설정' }));
        expect(createRuntime).not.toHaveBeenCalled();
        expect(screen.queryByRole('switch', { name: '배경음악' })).not.toBeInTheDocument();
        await fireEvent.click(
            await screen.findByRole('button', { name: '선택한 기능만 이번 세션에서 허용' }),
        );
        const musicToggle = await screen.findByRole('switch', { name: '배경음악' });
        await fireEvent.click(musicToggle);
        await waitFor(() => expect(setOption).toHaveBeenCalledWith('music', '1'));

        const composer = screen.getByRole('textbox', { name: '메시지' });
        await fireEvent.input(composer, { target: { value: '카드 옵션 테스트' } });
        await fireEvent.click(screen.getByRole('button', { name: '메시지 보내기' }));

        await waitFor(() =>
            expect(sendMessage).toHaveBeenCalledWith('카드 옵션 테스트', {
                values: [
                    {
                        variable: { scope: 'character', namespace: null, id: 'music' },
                        value: { type: 'text', value: '1' },
                    },
                ],
            }),
        );
        controller.destroy();
        orchestrationController.destroy();
    });

    it.each(['안녕', 'こんにちは', '你好'])(
        'does not submit %s when Enter confirms an IME composition',
        async (draft) => {
            const { controller, sendMessage } = renderChat();
            const composer = screen.getByRole('textbox', { name: '메시지' });

            await fireEvent.input(composer, { target: { value: draft } });
            await fireEvent.compositionStart(composer);
            await fireEvent.keyDown(composer, {
                key: 'Enter',
                code: 'Enter',
                isComposing: true,
            });

            expect(sendMessage).not.toHaveBeenCalled();
            expect(composer).toHaveValue(draft);
            controller.destroy();
        },
    );

    it('submits plain Enter after composition ends and keeps Shift+Enter as a newline', async () => {
        const { controller, sendMessage } = renderChat();
        const composer = screen.getByRole('textbox', { name: '메시지' });

        await fireEvent.input(composer, { target: { value: '계속 이야기해 줘' } });
        await fireEvent.keyDown(composer, { key: 'Enter', code: 'Enter', shiftKey: true });
        expect(sendMessage).not.toHaveBeenCalled();

        await fireEvent.compositionStart(composer);
        await fireEvent.compositionEnd(composer);
        await fireEvent.keyDown(composer, {
            key: 'Enter',
            code: 'Enter',
            isComposing: false,
        });

        await waitFor(() => {
            expect(sendMessage).toHaveBeenCalledOnce();
        });
        expect(sendMessage).toHaveBeenCalledWith('계속 이야기해 줘');
        await waitFor(() => {
            expect(composer).toHaveValue('');
        });
        controller.destroy();
    });

    it('retains a blocked draft, refreshes attempt approvals, and retries only from a user click', async () => {
        const pending = {
            conversation_id: 'conversation-1',
            source_branch_id: 'branch-1',
            proposed_branch_id: 'branch-proposed',
            generation_id: 'generation-attempt-1',
            aggregate_revision: '1',
            interaction_state_revision: '1',
            pending_proposal_count: 1,
            proposal_revision: '1',
            proposal: {
                id: 'attempt-proposal-1',
                title: '도구 동작 승인',
                body: '검토한 동작만 반영합니다.',
                status: 'pending' as const,
                source_interaction_state_revision: '0',
                requested_at_epoch_seconds: 1,
                expires_at_epoch_seconds: null,
                decided_at_epoch_seconds: null,
            },
        };
        const listGenerationAttemptProposals = vi
            .fn()
            .mockResolvedValueOnce([])
            .mockResolvedValueOnce([pending])
            .mockResolvedValue([]);
        const decideGenerationAttemptProposal = vi.fn().mockResolvedValue({
            ...pending,
            aggregate_revision: '2',
            pending_proposal_count: 0,
            proposal_revision: '2',
            proposal: {
                ...pending.proposal,
                status: 'approved',
                decided_at_epoch_seconds: 2,
            },
            approval_evidence_sha256: 'a'.repeat(64),
            exact_replay: false,
        });
        const client = {
            listInteractionEffects: vi.fn().mockResolvedValue([]),
            subscribeInteractionEffects: vi.fn().mockResolvedValue(vi.fn()),
            acknowledgeInteractionEffect: vi.fn().mockResolvedValue(undefined),
            retryInteractionEffect: vi.fn().mockResolvedValue(undefined),
            expireInteractionProposals: vi.fn().mockResolvedValue({
                conversation_id: 'conversation-1',
                branch_id: 'branch-1',
                current_state_revision: 0,
                expired_proposals: [],
                has_more_expired: false,
            }),
            listInteractionProposals: vi.fn().mockResolvedValue([]),
            listReopenInteractionEffects: vi.fn().mockResolvedValue({
                current_state_revision: 0,
                items: [],
                older_cursor: null,
            }),
            submitInteractionChoice: vi.fn(),
            decideInteractionProposal: vi.fn(),
            expireGenerationAttemptProposals: vi.fn().mockResolvedValue({
                conversation_id: 'conversation-1',
                source_branch_id: 'branch-1',
                decisions: [],
                has_more_due: false,
            }),
            listRetryableGenerationAttempts: vi.fn().mockResolvedValue([]),
            listGenerationAttemptProposals,
            decideGenerationAttemptProposal,
        } as unknown as LorepiaClient;
        const { controller, sendMessage, orchestrationController } = renderChatWithSettings(
            chatReadyState(),
            client,
        );
        const beginNewGenerationOperation = vi.spyOn(controller, 'beginNewGenerationOperation');
        const stageGenerationAttemptRetry = vi.spyOn(controller, 'stageGenerationAttemptRetry');
        sendMessage.mockResolvedValue(false);
        await waitFor(() => expect(listGenerationAttemptProposals).toHaveBeenCalledOnce());

        const composer = screen.getByRole('textbox', { name: '메시지' });
        await fireEvent.input(composer, { target: { value: '승인 뒤에도 유지할 메시지' } });
        await fireEvent.click(screen.getByRole('button', { name: '메시지 보내기' }));

        expect(await screen.findByText('도구 동작 승인')).toBeInTheDocument();
        expect(composer).toHaveValue('승인 뒤에도 유지할 메시지');
        expect(sendMessage).toHaveBeenCalledOnce();

        await fireEvent.click(screen.getByRole('button', { name: '제안 1 승인' }));
        const retry = await screen.findByRole('button', {
            name: '원래 전송·수정·재생성 확인: 생성 시도 generation-attempt-1',
        });
        expect(sendMessage).toHaveBeenCalledOnce();
        await fireEvent.click(retry);
        expect(composer).not.toHaveFocus();
        expect(composer).toHaveValue('승인 뒤에도 유지할 메시지');
        expect(sendMessage).toHaveBeenCalledOnce();
        expect(beginNewGenerationOperation).not.toHaveBeenCalled();
        expect(stageGenerationAttemptRetry).toHaveBeenCalledOnce();
        expect(stageGenerationAttemptRetry).toHaveBeenCalledWith('generation-attempt-1');
        expect(
            screen.getByText(
                '승인된 생성 시도를 준비했습니다. 원래 전송·수정·재생성 작업을 직접 반복하세요.',
            ),
        ).toBeInTheDocument();

        expect(screen.queryByRole('button', { name: '새 생성 작업' })).not.toBeInTheDocument();
        await fireEvent.click(screen.getByRole('button', { name: '대화 설정' }));
        const settings = screen.getByRole('dialog', { name: '대화 설정' });
        await fireEvent.click(within(settings).getByRole('button', { name: '새 생성 작업' }));
        expect(beginNewGenerationOperation).toHaveBeenCalledOnce();
        await waitFor(() => expect(composer).toHaveFocus());
        expect(composer).toHaveValue('승인 뒤에도 유지할 메시지');
        expect(sendMessage).toHaveBeenCalledOnce();
        expect(
            screen.getByText(
                '새 생성 작업으로 전환했습니다. 같은 입력도 새로운 요청으로 처리됩니다.',
            ),
        ).toBeInTheDocument();

        sendMessage.mockResolvedValue(true);
        await fireEvent.click(screen.getByRole('button', { name: '메시지 보내기' }));
        await waitFor(() => expect(listGenerationAttemptProposals).toHaveBeenCalledTimes(3));
        expect(composer).toHaveValue('');
        expect(sendMessage).toHaveBeenCalledTimes(2);
        controller.destroy();
        orchestrationController.destroy();
    });

    it('keeps the visible DOM bounded for 10,000 persisted messages', () => {
        const appState = chatReadyState();
        appState.messages.items = Array.from({ length: 10_000 }, (_, index) => ({
            id: `message-${String(index)}`,
            conversation_id: 'conversation-1',
            parent_id: index === 0 ? null : `message-${String(index - 1)}`,
            role: index % 2 === 0 ? ('user' as const) : ('assistant' as const),
            content: `synthetic-${String(index)}`,
            status: 'complete' as const,
            generation_id: null,
            created_at: '2026-08-02T00:00:00Z',
        }));

        const { controller } = renderChat(appState);
        const renderedMessages = document.querySelectorAll('[data-message-id]');

        expect(renderedMessages.length).toBeGreaterThan(0);
        expect(renderedMessages.length).toBeLessThanOrEqual(80);
        controller.destroy();
    });

    it('uses instant scroll behavior for exact virtual anchor corrections', () => {
        const { controller } = renderChat();

        expect(screen.getByLabelText('메시지 기록').style.scrollBehavior).toBe('auto');
        controller.destroy();
    });

    it('does not rebuild the full message index for an unrelated streaming update', async () => {
        const appState = chatReadyState();
        let idReadCount = 0;
        const items = Array.from({ length: 10_000 }, (_, index) => {
            const message: MessageDto = {
                id: '',
                conversation_id: 'conversation-1',
                parent_id: index === 0 ? null : `message-${String(index - 1)}`,
                role: index % 2 === 0 ? 'user' : 'assistant',
                content: `synthetic-${String(index)}`,
                status: 'complete',
                generation_id: null,
                created_at: '2026-08-02T00:00:00Z',
            };
            Object.defineProperty(message, 'id', {
                configurable: true,
                enumerable: true,
                get: () => {
                    idReadCount += 1;
                    return `message-${String(index)}`;
                },
            });
            return message;
        });
        appState.messages.items = items;
        const controller = new LorepiaAppController({} as LorepiaClient);
        const rendered = render(ChatPane, { appState, controller });
        await tick();
        await Promise.resolve();
        idReadCount = 0;

        await rendered.rerender({
            appState: {
                ...appState,
                chat: { ...appState.chat, streaming_text: '새 스트림 델타' },
            },
            controller,
        });
        await tick();

        expect(idReadCount).toBeLessThan(1_000);
        controller.destroy();
    });

    it('preserves a deep anchor relative to the viewport when width changes row heights', async () => {
        vi.stubGlobal('ResizeObserver', ControlledResizeObserver);
        let messageHeight = 500;
        vi.spyOn(HTMLElement.prototype, 'getBoundingClientRect').mockImplementation(function (
            this: HTMLElement,
        ) {
            if (this.dataset.messageId === undefined) {
                return DOMRect.fromRect();
            }
            const list = this.parentElement;
            const scroller = this.closest<HTMLElement>('.message-scroll');
            if (list === null || scroller === null) {
                return DOMRect.fromRect();
            }
            const rendered = Array.from(list.querySelectorAll<HTMLElement>('[data-message-id]'));
            const localIndex = rendered.indexOf(this);
            const top =
                (Number.parseFloat(list.style.paddingTop) || 0) +
                localIndex * (messageHeight + 12) -
                scroller.scrollTop;
            return {
                x: 0,
                y: top,
                top,
                bottom: top + messageHeight,
                left: 0,
                right: 900,
                width: 900,
                height: messageHeight,
                toJSON: () => ({}),
            };
        });
        const appState = chatReadyState();
        appState.messages.items = Array.from({ length: 300 }, (_, index) => ({
            id: `message-${String(index)}`,
            conversation_id: 'conversation-1',
            parent_id: index === 0 ? null : `message-${String(index - 1)}`,
            role: index % 2 === 0 ? ('user' as const) : ('assistant' as const),
            content: `synthetic-${String(index)}`,
            status: 'complete' as const,
            generation_id: null,
            created_at: '2026-08-02T00:00:00Z',
        }));
        const { controller } = renderChat(appState);
        const scroller = screen.getByLabelText('메시지 기록');
        Object.defineProperties(scroller, {
            clientHeight: { configurable: true, value: 720 },
            scrollHeight: { configurable: true, value: 100_000 },
            getBoundingClientRect: {
                configurable: true,
                value: () => ({
                    top: 0,
                    bottom: 720,
                    left: 0,
                    right: 900,
                    width: 900,
                    height: 720,
                }),
            },
        });

        await waitFor(() => {
            expect(ControlledResizeObserver.observing(scroller)).toBeDefined();
        });
        const scrollerObserver = ControlledResizeObserver.observing(scroller);
        expect(scrollerObserver).toBeDefined();
        scrollerObserver?.emit(scroller, 900, 100_000);
        await waitFor(() => {
            expect(document.querySelectorAll('[data-message-id]')).toHaveLength(80);
        });

        scroller.scrollTop = 1;
        await fireEvent.scroll(scroller);
        for (const message of document.querySelectorAll<HTMLElement>('[data-message-id]')) {
            ControlledResizeObserver.observing(message)?.emit(message, 900, 500);
        }
        await Promise.resolve();
        await tick();
        await tick();

        scrollerObserver?.emit(scroller, 900, 720);
        scroller.scrollTop = 30_720;
        await fireEvent.scroll(scroller);
        let stableAnchor: HTMLElement | undefined;
        await waitFor(() => {
            const rendered = Array.from(
                document.querySelectorAll<HTMLElement>('[data-message-id]'),
            );
            expect(rendered.length).toBeGreaterThan(8);
            stableAnchor = rendered.find((message) => message.getBoundingClientRect().bottom > 0);
            expect(stableAnchor?.dataset.messageId).toBeDefined();
        });
        const stableAnchorId = stableAnchor?.dataset.messageId ?? '';
        const relativeTopBefore = stableAnchor?.getBoundingClientRect().top ?? Number.NaN;

        messageHeight = 700;
        scrollerObserver?.emit(scroller, 520, 720);
        await Promise.resolve();
        await tick();
        await tick();
        for (let pass = 0; pass < 5; pass += 1) {
            for (const message of document.querySelectorAll<HTMLElement>('[data-message-id]')) {
                ControlledResizeObserver.observing(message)?.emit(message, 520, messageHeight);
            }
            await Promise.resolve();
            await tick();
            await tick();
        }

        await waitFor(() => {
            const anchored = document.querySelector<HTMLElement>(
                `[data-message-id="${stableAnchorId}"]`,
            );
            expect(anchored).toBeInTheDocument();
            expect(anchored?.getBoundingClientRect().top).toBeCloseTo(relativeTopBefore, 5);
        });
        controller.destroy();
    });

    it('keeps the pre-delete retained predecessor anchored through deletion and row resize', async () => {
        vi.stubGlobal('ResizeObserver', ControlledResizeObserver);
        const measuredHeights = new Map<string, number>();
        vi.spyOn(HTMLElement.prototype, 'getBoundingClientRect').mockImplementation(function (
            this: HTMLElement,
        ) {
            if (this.dataset.messageId === undefined) {
                return DOMRect.fromRect();
            }
            const list = this.parentElement;
            const scroller = this.closest<HTMLElement>('.message-scroll');
            if (list === null || scroller === null) {
                return DOMRect.fromRect();
            }
            const rendered = Array.from(list.querySelectorAll<HTMLElement>('[data-message-id]'));
            const localIndex = rendered.indexOf(this);
            const messageHeight = measuredHeights.get(this.dataset.messageId) ?? 500;
            const top =
                (Number.parseFloat(list.style.paddingTop) || 0) +
                rendered
                    .slice(0, localIndex)
                    .reduce(
                        (height, message) =>
                            height +
                            (measuredHeights.get(message.dataset.messageId ?? '') ?? 500) +
                            12,
                        0,
                    ) -
                scroller.scrollTop;
            return {
                x: 0,
                y: top,
                top,
                bottom: top + messageHeight,
                left: 0,
                right: 900,
                width: 900,
                height: messageHeight,
                toJSON: () => ({}),
            };
        });
        const appState = chatReadyState();
        appState.messages.items = Array.from({ length: 300 }, (_, index) => ({
            id: `message-${String(index)}`,
            conversation_id: 'conversation-1',
            parent_id: index === 0 ? null : `message-${String(index - 1)}`,
            role: index % 2 === 0 ? ('user' as const) : ('assistant' as const),
            content: `synthetic-${String(index)}`,
            status: 'complete' as const,
            generation_id: null,
            created_at: '2026-08-02T00:00:00Z',
        }));
        const controller = new LorepiaAppController({} as LorepiaClient);
        const rendered = render(ChatPane, { appState, controller });
        const scroller = screen.getByLabelText('메시지 기록');
        Object.defineProperties(scroller, {
            clientHeight: { configurable: true, value: 720 },
            scrollHeight: { configurable: true, value: 100_000 },
            getBoundingClientRect: {
                configurable: true,
                value: () => ({
                    top: 0,
                    bottom: 720,
                    left: 0,
                    right: 900,
                    width: 900,
                    height: 720,
                }),
            },
        });
        await waitFor(() => {
            expect(ControlledResizeObserver.observing(scroller)).toBeDefined();
        });
        const scrollerObserver = ControlledResizeObserver.observing(scroller);
        scrollerObserver?.emit(scroller, 900, 100_000);
        await waitFor(() => {
            expect(document.querySelectorAll('[data-message-id]')).toHaveLength(80);
        });
        scroller.scrollTop = 1;
        await fireEvent.scroll(scroller);
        for (const message of document.querySelectorAll<HTMLElement>('[data-message-id]')) {
            ControlledResizeObserver.observing(message)?.emit(message, 900, 500);
        }
        await Promise.resolve();
        await tick();
        await tick();

        scrollerObserver?.emit(scroller, 900, 720);
        scroller.scrollTop = 30_720;
        await fireEvent.scroll(scroller);
        await tick();
        for (const message of document.querySelectorAll<HTMLElement>('[data-message-id]')) {
            ControlledResizeObserver.observing(message)?.emit(message, 900, 500);
        }
        await Promise.resolve();
        await tick();
        await tick();
        await fireEvent.scroll(scroller);
        await tick();

        const storedAnchor = Array.from(
            document.querySelectorAll<HTMLElement>('[data-message-id]'),
        ).find((message) => message.getBoundingClientRect().bottom > 0);
        const storedAnchorId = storedAnchor?.dataset.messageId;
        expect(storedAnchorId).toBeDefined();
        const storedAnchorIndex = appState.messages.items.findIndex(
            (message) => message.id === storedAnchorId,
        );
        const predecessor = appState.messages.items[storedAnchorIndex - 1];
        if (predecessor === undefined) throw new Error('retained predecessor missing');
        const predecessorBefore = document.querySelector<HTMLElement>(
            `[data-message-id="${predecessor.id}"]`,
        );
        expect(predecessorBefore).toBeInTheDocument();
        const predecessorRelativeTopBefore =
            predecessorBefore?.getBoundingClientRect().top ?? Number.NaN;
        const retainedItems = appState.messages.items.filter(
            (message, index) => index >= 8 && message.id !== storedAnchorId,
        );
        await rendered.rerender({
            appState: {
                ...appState,
                messages: { ...appState.messages, items: retainedItems },
            },
            controller,
        });
        await Promise.resolve();
        await tick();
        await tick();

        const anchoredPredecessor = document.querySelector<HTMLElement>(
            `[data-message-id="${predecessor.id}"]`,
        );
        expect(anchoredPredecessor).toBeInTheDocument();
        expect(anchoredPredecessor?.getBoundingClientRect().top).toBeCloseTo(
            predecessorRelativeTopBefore,
            5,
        );

        const predecessorIndex = retainedItems.findIndex(
            (message) => message.id === predecessor.id,
        );
        const rowAbove = retainedItems[predecessorIndex - 1];
        if (rowAbove === undefined) throw new Error('row above retained predecessor missing');
        const rowAboveElement = document.querySelector<HTMLElement>(
            `[data-message-id="${rowAbove.id}"]`,
        );
        if (rowAboveElement === null) throw new Error('measured row above predecessor missing');
        measuredHeights.set(rowAbove.id, 700);
        ControlledResizeObserver.observing(rowAboveElement)?.emit(rowAboveElement, 900, 700);
        await Promise.resolve();
        await tick();
        await tick();

        const anchoredAfterResize = document.querySelector<HTMLElement>(
            `[data-message-id="${predecessor.id}"]`,
        );
        expect(anchoredAfterResize).toBeInTheDocument();
        expect(anchoredAfterResize?.getBoundingClientRect().top).toBeCloseTo(
            predecessorRelativeTopBefore,
            5,
        );
        controller.destroy();
    });

    it('recomputes bottom proximity when the viewport height changes', async () => {
        vi.stubGlobal('ResizeObserver', ControlledResizeObserver);
        const appState = chatReadyState();
        appState.messages.items = Array.from({ length: 160 }, (_, index) => ({
            id: `message-${String(index)}`,
            conversation_id: 'conversation-1',
            parent_id: index === 0 ? null : `message-${String(index - 1)}`,
            role: index % 2 === 0 ? ('user' as const) : ('assistant' as const),
            content: `synthetic-${String(index)}`,
            status: 'complete' as const,
            generation_id: null,
            created_at: '2026-08-02T00:00:00Z',
        }));
        const controller = new LorepiaAppController({} as LorepiaClient);
        const rendered = render(ChatPane, { appState, controller });
        const scroller = screen.getByLabelText('메시지 기록');
        let rawScrollTop = 0;
        let clientHeight = 100;
        Object.defineProperties(scroller, {
            scrollTop: {
                configurable: true,
                get: () => rawScrollTop,
                set: (value: number) => {
                    rawScrollTop = value;
                },
            },
            clientHeight: { configurable: true, get: () => clientHeight },
            scrollHeight: { configurable: true, get: () => 10_000 },
        });
        await waitFor(() => {
            expect(ControlledResizeObserver.observing(scroller)).toBeDefined();
        });
        const scrollerObserver = ControlledResizeObserver.observing(scroller);
        scrollerObserver?.emit(scroller, 900, clientHeight);
        await tick();
        await Promise.resolve();
        await tick();
        rawScrollTop = 9_000;
        await fireEvent.scroll(scroller);

        clientHeight = 950;
        scrollerObserver?.emit(scroller, 900, clientHeight);
        await rendered.rerender({
            appState: {
                ...appState,
                messages: {
                    ...appState.messages,
                    items: [
                        ...appState.messages.items,
                        {
                            id: 'message-new',
                            conversation_id: 'conversation-1',
                            parent_id: 'message-159',
                            role: 'assistant',
                            content: '새 메시지',
                            status: 'complete',
                            generation_id: null,
                            created_at: '2026-08-02T00:00:01Z',
                        },
                    ],
                },
            },
            controller,
        });

        await waitFor(() => expect(rawScrollTop).toBe(10_000));
        controller.destroy();
    });

    it('recomputes bottom proximity after an exact measurement anchor correction', async () => {
        vi.stubGlobal('ResizeObserver', ControlledResizeObserver);
        let expandedMessageId: string | null = null;
        vi.spyOn(HTMLElement.prototype, 'getBoundingClientRect').mockImplementation(function (
            this: HTMLElement,
        ) {
            if (this.dataset.messageId === undefined) return DOMRect.fromRect();
            const list = this.parentElement;
            const scroller = this.closest<HTMLElement>('.message-scroll');
            if (list === null || scroller === null) return DOMRect.fromRect();
            const rendered = Array.from(list.querySelectorAll<HTMLElement>('[data-message-id]'));
            const localIndex = rendered.indexOf(this);
            const heightFor = (message: HTMLElement): number =>
                message.dataset.messageId === expandedMessageId ? 900 : 96;
            const height = heightFor(this);
            const top =
                (Number.parseFloat(list.style.paddingTop) || 0) +
                rendered
                    .slice(0, localIndex)
                    .reduce((total, message) => total + heightFor(message) + 12, 0) -
                scroller.scrollTop;
            return {
                x: 0,
                y: top,
                top,
                bottom: top + height,
                left: 0,
                right: 900,
                width: 900,
                height,
                toJSON: () => ({}),
            };
        });
        const appState = chatReadyState();
        appState.messages.items = Array.from({ length: 100 }, (_, index) => ({
            id: `message-${String(index)}`,
            conversation_id: 'conversation-1',
            parent_id: index === 0 ? null : `message-${String(index - 1)}`,
            role: index % 2 === 0 ? ('user' as const) : ('assistant' as const),
            content: `synthetic-${String(index)}`,
            status: 'complete' as const,
            generation_id: null,
            created_at: '2026-08-02T00:00:00Z',
        }));
        const controller = new LorepiaAppController({} as LorepiaClient);
        const rendered = render(ChatPane, { appState, controller });
        const scroller = screen.getByLabelText('메시지 기록');
        let rawScrollTop = 0;
        Object.defineProperties(scroller, {
            scrollTop: {
                configurable: true,
                get: () => rawScrollTop,
                set: (value: number) => {
                    rawScrollTop = value;
                },
            },
            clientHeight: { configurable: true, value: 720 },
            scrollHeight: { configurable: true, value: 2_000 },
            getBoundingClientRect: {
                configurable: true,
                value: () => ({
                    top: 0,
                    bottom: 720,
                    left: 0,
                    right: 900,
                    width: 900,
                    height: 720,
                }),
            },
        });
        await waitFor(() => {
            expect(ControlledResizeObserver.observing(scroller)).toBeDefined();
        });
        ControlledResizeObserver.observing(scroller)?.emit(scroller, 900, 720);
        await tick();
        await Promise.resolve();
        rawScrollTop = 500;
        await fireEvent.scroll(scroller);
        await tick();
        const visibleAnchor = Array.from(
            document.querySelectorAll<HTMLElement>('[data-message-id]'),
        ).find((message) => message.getBoundingClientRect().bottom > 0);
        if (visibleAnchor === undefined) throw new Error('visible anchor missing');
        const renderedRows = Array.from(
            document.querySelectorAll<HTMLElement>('[data-message-id]'),
        );
        const anchorIndex = renderedRows.indexOf(visibleAnchor);
        const rowAbove = renderedRows[anchorIndex - 1];
        if (rowAbove === undefined) throw new Error('row above anchor missing');

        expandedMessageId = rowAbove.dataset.messageId ?? null;
        ControlledResizeObserver.observing(rowAbove)?.emit(rowAbove, 900, 900);
        await Promise.resolve();
        await tick();
        await tick();
        expect(rawScrollTop).toBeGreaterThan(1_160);
        expect(rawScrollTop).toBeLessThan(2_000);

        await rendered.rerender({
            appState: {
                ...appState,
                messages: {
                    ...appState.messages,
                    items: [
                        ...appState.messages.items,
                        {
                            id: 'message-new',
                            conversation_id: 'conversation-1',
                            parent_id: 'message-99',
                            role: 'assistant',
                            content: '새 메시지',
                            status: 'complete',
                            generation_id: null,
                            created_at: '2026-08-02T00:00:01Z',
                        },
                    ],
                },
            },
            controller,
        });

        await waitFor(() => expect(rawScrollTop).toBe(2_000));
        controller.destroy();
    });

    it('hands focus to the scroll region before a focused message row is virtualized away', async () => {
        const appState = chatReadyState();
        appState.messages.items = Array.from({ length: 300 }, (_, index) => ({
            id: `message-${String(index)}`,
            conversation_id: 'conversation-1',
            parent_id: index === 0 ? null : `message-${String(index - 1)}`,
            role: index % 2 === 0 ? ('user' as const) : ('assistant' as const),
            content: `synthetic-${String(index)}`,
            status: 'complete' as const,
            generation_id: null,
            created_at: '2026-08-02T00:00:00Z',
        }));
        const { controller } = renderChat(appState);
        const scroller = screen.getByLabelText('메시지 기록');
        Object.defineProperties(scroller, {
            clientHeight: { configurable: true, value: 720 },
            scrollHeight: { configurable: true, value: 100_000 },
        });
        const focusedRow = document.querySelector<HTMLElement>('[data-message-id="message-0"]');
        if (focusedRow === null) throw new Error('focused message fixture missing');
        focusedRow.focus();
        expect(document.activeElement).toBe(focusedRow);

        scroller.scrollTop = 30_720;
        await fireEvent.scroll(scroller);
        await tick();

        expect(document.querySelector('[data-message-id="message-0"]')).not.toBeInTheDocument();
        expect(document.activeElement).toBe(scroller);
        controller.destroy();
    });

    it('focuses a memory source message through the bounded virtual window', async () => {
        const appState = chatReadyState();
        appState.messages.items = Array.from({ length: 160 }, (_, index) => ({
            id: `message-${String(index)}`,
            conversation_id: 'conversation-1',
            parent_id: index === 0 ? null : `message-${String(index - 1)}`,
            role: index % 2 === 0 ? ('user' as const) : ('assistant' as const),
            content: `synthetic-${String(index)}`,
            status: 'complete' as const,
            generation_id: null,
            created_at: '2026-08-02T00:00:00Z',
        }));
        const controller = new LorepiaAppController({} as LorepiaClient);
        const scrollIntoView = vi.fn();
        Object.defineProperty(HTMLElement.prototype, 'scrollIntoView', {
            configurable: true,
            value: scrollIntoView,
        });

        render(ChatPane, {
            appState,
            controller,
            messageFocusRequest: {
                conversation_id: 'conversation-1',
                branch_id: 'branch-1',
                start_message_id: 'message-120',
                end_message_id: 'message-125',
                request_id: 1,
            },
        });

        await waitFor(() => {
            expect(document.querySelector('[data-message-id="message-120"]')).toBeInTheDocument();
        });
        const focused = document.querySelector<HTMLElement>('[data-message-id="message-120"]');
        expect(focused).toHaveClass('memory-source-boundary');
        expect(document.querySelector('[data-message-id="message-125"]')).toHaveClass(
            'memory-source-boundary',
        );
        await waitFor(() => {
            expect(document.activeElement).toBe(focused);
            expect(scrollIntoView).toHaveBeenCalledWith({ block: 'center' });
        });
        expect(
            screen.getByText('장기기억 출처 범위의 첫 메시지로 이동했습니다.'),
        ).toBeInTheDocument();
        controller.destroy();
    });

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
        const remove = vi.spyOn(controller, 'removeMessage').mockResolvedValue();
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
