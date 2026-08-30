import { cleanup, fireEvent, render, screen, waitFor } from '@testing-library/svelte';
import { tick } from 'svelte';
import { afterEach, describe, expect, it, vi, type MockInstance } from 'vitest';

import type { LorepiaClient, MessageDto } from '../../../lib/ipc/contracts';
import { LorepiaAppController } from '../../../app/app-controller';
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

describe('ChatPane live response', () => {
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
});
