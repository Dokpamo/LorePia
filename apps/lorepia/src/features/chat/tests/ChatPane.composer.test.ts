import { cleanup, fireEvent, render, screen, waitFor, within } from '@testing-library/svelte';
import { tick } from 'svelte';
import { afterEach, describe, expect, it, vi, type MockInstance } from 'vitest';

import type { LorepiaClient } from '../../../lib/ipc/contracts';
import { LorepiaAppController } from '../../../app/app-controller';
import {
    INITIAL_ORCHESTRATION_STATE,
    OrchestrationController,
} from '../../orchestration/orchestration-controller';
import '../../../styles/app.css';
import ChatPane from '../ChatPane.svelte';
import { chatReadyState } from './chat-pane-state-builder';

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

describe('ChatPane transcript chrome', () => {
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
});

describe('ChatPane composer', () => {
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
});
