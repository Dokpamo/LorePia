import { cleanup, fireEvent, render, screen } from '@testing-library/svelte';
import { tick } from 'svelte';
import { afterEach, expect, it, vi } from 'vitest';
import ChatPage from './ChatPage.svelte';
import { t } from '../../lib/i18n';
import type { MessageHistoryState } from '../../app/controllers/message-history-controller';

afterEach(() => {
    cleanup();
    vi.restoreAllMocks();
    vi.unstubAllGlobals();
});
it('offers manual older navigation only for visible underfilled history, outside the measured list', async () => {
    let viewport = 6000;
    const observers: { nodes: Set<Element>; emit: () => void }[] = [];
    vi.stubGlobal(
        'ResizeObserver',
        class {
            nodes = new Set<Element>();
            constructor(callback: ResizeObserverCallback) {
                observers.push({
                    nodes: this.nodes,
                    emit: () => callback([], this as unknown as ResizeObserver),
                });
            }
            observe(node: Element) {
                this.nodes.add(node);
            }
            disconnect() {
                this.nodes.clear();
            }
        },
    );
    vi.spyOn(HTMLElement.prototype, 'clientHeight', 'get').mockImplementation(() => viewport);
    vi.spyOn(HTMLElement.prototype, 'scrollHeight', 'get').mockReturnValue(3000);
    const loadHistory = vi.fn().mockResolvedValue(undefined);
    const history: MessageHistoryState = {
        scope: 'conversation:branch',
        items: [],
        start_index: 1910,
        total_messages: 2000,
        head_message_id: 'm-1999',
        has_older: true,
        has_newer: true,
        loading: false,
        error: null,
        paged: true,
    };
    const props = {
        character: {
            id: 'character',
            name: 'Character',
            description: '',
            thumbnail: '',
            subpage: false,
            histories: [],
        },
        conversation: {
            id: 'conversation',
            title: '',
            date: '2026-09-12',
            activeBranchId: 'branch',
            messageOffset: 1910,
            totalMessages: 2000,
            messages: Array.from({ length: 90 }, (_, index) => ({
                id: `m-${String(1910 + index)}`,
                role: 'system' as const,
                text: 'short',
            })),
        },
        session: {
            busy: false,
            branches: () => [],
            loadHistory,
            selectBranch: vi.fn(),
            fork: vi.fn(),
            edit: () => vi.fn(),
            removeFrom: vi.fn(),
            regenerate: vi.fn(),
            stop: vi.fn(),
        },
        history,
        draft: '',
        onnavigate: vi.fn(),
        ondraft: vi.fn(),
        onsend: vi.fn(),
        onsettings: vi.fn(),
        oninteract: vi.fn(),
    };
    const view = render(ChatPage, props);
    await tick();
    await tick();
    const button = screen.getByRole('button', { name: t('pagination.older_messages') });
    const log = screen.getByRole('log');
    expect(log.contains(button)).toBe(false);
    expect(loadHistory).not.toHaveBeenCalled();
    loadHistory.mockClear();
    await fireEvent.click(button);
    expect(loadHistory).toHaveBeenCalledExactlyOnceWith('older');
    await view.rerender({ ...props, history: { ...history, loading: true } });
    expect(screen.queryByRole('button', { name: t('pagination.older_messages') })).toBeNull();
    await view.rerender({ ...props, history: { ...history, has_older: false } });
    expect(screen.queryByRole('button', { name: t('pagination.older_messages') })).toBeNull();
    await view.rerender(props);
    const resize = () =>
        observers
            .filter((observer) => observer.nodes.has(log))
            .forEach((observer) => observer.emit());
    viewport = 900;
    resize();
    await tick();
    await tick();
    expect(screen.queryByRole('button', { name: t('pagination.older_messages') })).toBeNull();
    viewport = 0;
    resize();
    await tick();
    expect(screen.queryByRole('button', { name: t('pagination.older_messages') })).toBeNull();
    viewport = 6000;
    resize();
    await tick();
    await tick();
    expect(screen.getByRole('button', { name: t('pagination.older_messages') })).toBeVisible();
    view.unmount();
    resize();
    await tick();
    expect(observers.every((observer) => observer.nodes.size === 0)).toBe(true);
    expect(loadHistory).toHaveBeenCalledOnce();
});
