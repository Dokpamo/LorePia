import { render, cleanup } from '@testing-library/svelte';
import { tick } from 'svelte';
import { afterEach, expect, it, vi } from 'vitest';
import ChatTranscript from './ChatTranscript.svelte';
import type { ChatScrollLifecycle } from '../../features/chat/chat-scroll.svelte';
afterEach(() => {
    cleanup();
    vi.restoreAllMocks();
    vi.unstubAllGlobals();
});
it('checks older history after rendering 30 short messages that cannot scroll', async () => {
    vi.spyOn(HTMLElement.prototype, 'clientHeight', 'get').mockReturnValue(900);
    vi.spyOn(HTMLElement.prototype, 'scrollHeight', 'get').mockReturnValue(600);
    const onhistoryedge = vi.fn();
    const scroll = {
        scroller: null,
        virtualWindow: () => ({ start: 0, end: 30, topSpacer: 0, bottomSpacer: 0 }),
        measureMessage: () => ({}),
        measurementEpoch: 0,
    } as unknown as ChatScrollLifecycle;
    render(ChatTranscript, {
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
            date: '',
            activeBranchId: 'branch',
            messageOffset: 1970,
            totalMessages: 2000,
            messages: Array.from({ length: 30 }, (_, index) => ({
                id: `message-${String(index)}`,
                role: 'system' as const,
                text: 'short',
            })),
        },
        session: {
            busy: false,
            branches: () => [],
            selectBranch: vi.fn(),
            fork: vi.fn(),
            edit: () => vi.fn(),
            removeFrom: vi.fn(),
            regenerate: vi.fn(),
            stop: vi.fn(),
        },
        scroll,
        collection: { items: [], ids: [], retainedIds: new Set<string>(), indexesById: {} },
        active: null,
        onactive: vi.fn(),
        onnotice: vi.fn(),
        onwrite: vi.fn(),
        onhistoryedge,
    });
    await tick();
    await tick();
    expect(onhistoryedge).toHaveBeenCalledOnce();
});
