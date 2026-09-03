import { describe, expect, it } from 'vitest';

import { LiveLorepiaClient, type LorepiaTransport } from './client';

class RecordingTransport implements LorepiaTransport {
    readonly calls: { commandName: string; args?: Record<string, unknown> }[] = [];

    invoke(commandName: string, args?: Record<string, unknown>): Promise<unknown> {
        this.calls.push({ commandName, args });
        return Promise.resolve(null);
    }

    createChatChannel(): unknown {
        return {};
    }

    listen(): Promise<() => void> {
        return Promise.resolve(() => undefined);
    }
}

describe('content import client', () => {
    it('sends only the two fixed resource policies through the picker contract', async () => {
        const transport = new RecordingTransport();
        const client = new LiveLorepiaClient(transport);

        await client.selectImportSource();
        await client.selectImportSource('user_approved_large');

        expect(transport.calls).toEqual([
            {
                commandName: 'pick_import',
                args: { request: { resource_policy: 'standard' } },
            },
            {
                commandName: 'pick_import',
                args: { request: { resource_policy: 'user_approved_large' } },
            },
        ]);
    });
});
