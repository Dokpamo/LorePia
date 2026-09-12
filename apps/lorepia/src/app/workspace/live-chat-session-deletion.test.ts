import { expect, it, vi } from 'vitest';
import { LiveChatSession } from './live-chat-session.svelte';
import { INITIAL_APP_STATE } from '../app-state';
import type { LorepiaAppController } from '../app-controller';
import { deferred } from '../../tests/deferred';

it('waits for successful same-scope deletion cleanup and preserves failures and different scopes', async () => {
    const state = structuredClone(INITIAL_APP_STATE);
    state.messages.phase = 'ready';
    const drain = deferred<undefined>();
    const entered = deferred<undefined>();
    const removed = vi.fn(() => {
        entered.resolve(undefined);
        return drain.promise;
    });
    const removeMessage = vi.fn().mockResolvedValue({ mutationCommitted: true, scopeKey: ':' });
    const session = new LiveChatSession(
        { removeMessage } as unknown as LorepiaAppController,
        () => state,
        { onMutation: vi.fn(), onRemoved: removed },
    );
    const first = session.removeFrom('message');
    await entered.promise;
    expect(removed).toHaveBeenCalledWith(':');
    expect(session.busy).toBe(true);
    drain.resolve(undefined);
    await first;
    expect(session.busy).toBe(false);
    removeMessage.mockResolvedValueOnce({ mutationCommitted: false, scopeKey: ':' });
    await session.removeFrom('message');
    removeMessage.mockResolvedValueOnce({ mutationCommitted: true, scopeKey: 'old:branch' });
    await session.removeFrom('message');
    removeMessage.mockRejectedValueOnce(new Error('failed'));
    await expect(session.removeFrom('message')).rejects.toThrow('failed');
    expect(removed).toHaveBeenCalledOnce();
    expect(session.busy).toBe(false);
});
