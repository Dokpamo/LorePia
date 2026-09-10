import { expect, it, vi } from 'vitest';
import { LiveLorepiaClient, LOREPIA_COMMANDS, type LorepiaTransport } from './client';

it('reads aggregate storage counts without renderer paths or inputs', async () => {
    const counts = { characters: 2, conversations: 3, messages: 40, import_jobs: 0 };
    const invoke = vi.fn().mockResolvedValue(counts);
    const transport: LorepiaTransport = {
        invoke,
        createChatChannel: () => null,
        listen: () => Promise.resolve(() => undefined),
    };
    const client = new LiveLorepiaClient(transport);
    expect(await client.getStorageOverview()).toEqual(counts);
    expect(invoke.mock.calls).toEqual([[LOREPIA_COMMANDS.getStorageOverview, undefined]]);
});
