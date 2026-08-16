import { describe, expect, it } from 'vitest';

import { LiveLorepiaClient, type LorepiaTransport } from '../../lib/ipc/client';

const CATALOG_REVISION = 'a'.repeat(64);

class RecordingTransport implements LorepiaTransport {
    readonly calls: {
        commandName: string;
        args: Record<string, unknown> | undefined;
    }[] = [];

    invoke(commandName: string, args?: Record<string, unknown>): Promise<unknown> {
        this.calls.push({ commandName, args });
        if (commandName === 'list_personas') {
            return Promise.resolve([]);
        }
        if (commandName === 'list_persona_page') {
            return Promise.resolve({
                kind: 'page',
                catalog_revision: CATALOG_REVISION,
                items: [],
                next_cursor: null,
            });
        }
        return Promise.resolve(undefined);
    }

    createChatChannel(): unknown {
        return {};
    }

    listen(): Promise<() => void> {
        return Promise.resolve(() => undefined);
    }
}

describe('Persona IPC client', () => {
    it('keeps raw-list and page-list commands distinct with typed request envelopes', async () => {
        const transport = new RecordingTransport();
        const client = new LiveLorepiaClient(transport);

        await client.createPersona({ name: 'Narrator', description: 'Local persona' });
        await client.updatePersona({
            persona_id: 'persona-1',
            expected_revision: 2,
            name: 'Narrator',
            description: 'Updated local persona',
        });
        await client.getPersona({ persona_id: 'persona-1' });
        await client.listPersonas({ limit: 100 });
        await client.listPersonaPage({
            limit: 100,
            after: {
                catalog_revision: CATALOG_REVISION,
                updated_at: '2026-08-03T00:00:00Z',
                persona_id: 'persona-1',
            },
        });
        await client.deletePersona({ persona_id: 'persona-1', expected_revision: 3 });
        await client.getConversationPersonaSelection({ conversation_id: 'conversation-1' });
        await client.selectConversationPersona({
            conversation_id: 'conversation-1',
            persona_id: 'persona-2',
            expected_state_revision: null,
        });
        await client.clearConversationPersona({
            conversation_id: 'conversation-1',
            expected_state_revision: 1,
        });

        expect(transport.calls).toEqual([
            {
                commandName: 'create_persona',
                args: {
                    request: { name: 'Narrator', description: 'Local persona' },
                },
            },
            {
                commandName: 'update_persona',
                args: {
                    request: {
                        persona_id: 'persona-1',
                        expected_revision: 2,
                        name: 'Narrator',
                        description: 'Updated local persona',
                    },
                },
            },
            {
                commandName: 'get_persona',
                args: { request: { persona_id: 'persona-1' } },
            },
            {
                commandName: 'list_personas',
                args: { request: { limit: 100 } },
            },
            {
                commandName: 'list_persona_page',
                args: {
                    request: {
                        limit: 100,
                        after: {
                            catalog_revision: CATALOG_REVISION,
                            updated_at: '2026-08-03T00:00:00Z',
                            persona_id: 'persona-1',
                        },
                    },
                },
            },
            {
                commandName: 'delete_persona',
                args: {
                    request: { persona_id: 'persona-1', expected_revision: 3 },
                },
            },
            {
                commandName: 'get_conversation_persona_selection',
                args: { request: { conversation_id: 'conversation-1' } },
            },
            {
                commandName: 'select_conversation_persona',
                args: {
                    request: {
                        conversation_id: 'conversation-1',
                        persona_id: 'persona-2',
                        expected_state_revision: null,
                    },
                },
            },
            {
                commandName: 'clear_conversation_persona',
                args: {
                    request: {
                        conversation_id: 'conversation-1',
                        expected_state_revision: 1,
                    },
                },
            },
        ]);
    });
});
