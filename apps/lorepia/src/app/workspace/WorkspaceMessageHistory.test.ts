import type { BranchMessagesPageDto } from '../../lib/ipc/contracts/message-history';
import { cleanup, fireEvent, render, screen, waitFor } from '@testing-library/svelte';
import { afterEach, expect, it, vi } from 'vitest';
import WorkspaceApp from './WorkspaceApp.svelte';
import { createPreviewClient } from '../../preview/mock-client';
import { deferred } from '../tests/app-controller-test-support';
import { t } from '../../lib/i18n';
import type { MessageDto } from '../../lib/ipc/contracts';

afterEach(() => {
    cleanup();
    vi.restoreAllMocks();
    vi.unstubAllGlobals();
});
it('shows 15 recent pairs first, prefetches older history near the edge and returns to latest', async () => {
    vi.stubGlobal(
        'ResizeObserver',
        class {
            observe = vi.fn();
            disconnect = vi.fn();
        },
    );
    const client = createPreviewClient();
    const [conversation] = await client.listConversations(null);
    if (!conversation || !client.listBranchMessagesPage) throw new Error('fixture missing');
    const messages = Array.from({ length: 2000 }, (_, index): MessageDto => ({
        id: `history-${String(index)}`,
        conversation_id: conversation.id,
        parent_id: index ? `history-${String(index - 1)}` : null,
        role: index % 2 ? 'assistant' : 'user',
        content: `History text ${String(index)}`,
        status: 'complete',
        generation_id: null,
        created_at: '2026-09-12T00:00:00Z',
    }));
    vi.spyOn(client, 'listBranchMessages').mockResolvedValue(messages);
    const original = client.listBranchMessagesPage.bind(client);
    const context = deferred<BranchMessagesPageDto>();
    const pages = vi
        .spyOn(client, 'listBranchMessagesPage')
        .mockImplementation((input) => (input.limit === 128 ? context.promise : original(input)));
    render(WorkspaceApp, { client });
    await fireEvent.click(screen.getByRole('button', { name: t('navigation.chats') }));
    await fireEvent.click(
        await screen.findByRole('button', { name: new RegExp('^' + conversation.title + ' ·') }),
    );
    const log = await screen.findByRole('log', { name: t('uiPreview.messages') });
    await waitFor(() => expect(log.querySelector('[data-message-id]')).not.toBeNull());
    expect(log.querySelectorAll('[data-message-id]').length).toBeLessThanOrEqual(30);
    expect(log).toHaveAttribute('aria-busy', 'true');
    expect(log.querySelector('[data-message-id="history-0"]')).toBeNull();
    const state = await client.getConversationState(conversation.id);
    context.resolve(await original({ branch_id: state.active_branch_id, limit: 128 }));
    await waitFor(() => expect(log).toHaveAttribute('aria-busy', 'false'));
    Object.defineProperty(log, 'clientHeight', { value: 600, configurable: true });
    Object.defineProperty(log, 'scrollHeight', { value: 6000, configurable: true });
    for (let index = 0; index < 3; index += 1) {
        await fireEvent.scroll(log, { target: { scrollTop: 100 } });
        await waitFor(() =>
            expect(pages.mock.calls.filter(([input]) => input.before_message_id).length).toBe(
                index + 1,
            ),
        );
        await waitFor(() =>
            expect(
                log.querySelector('[aria-posinset="' + String(1941 - index * 30) + '"]'),
            ).not.toBeNull(),
        );
    }
    expect(log.querySelectorAll('[data-message-id]').length).toBeLessThanOrEqual(80);
    pages.mockRejectedValueOnce(new Error('temporary history failure'));
    await fireEvent.scroll(log, { target: { scrollTop: 100 } });
    await screen.findByText(t('ux.loading.failed'));
    const failedCalls = pages.mock.calls.length;
    for (let index = 0; index < 5; index += 1)
        await fireEvent.scroll(log, { target: { scrollTop: 100 + index } });
    expect(pages).toHaveBeenCalledTimes(failedCalls);
    log.scrollTo = vi.fn();
    const latest = screen.getByRole('button', { name: t('uiPreview.latestMessage') });
    await fireEvent.click(latest);
    await waitFor(() =>
        expect(pages.mock.calls.at(-1)?.[0]).toEqual({
            branch_id: state.active_branch_id,
            limit: 30,
        }),
    );
});
