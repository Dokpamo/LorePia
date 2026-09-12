import { cleanup, render, waitFor } from '@testing-library/svelte';
import { afterEach, expect, it, vi } from 'vitest';
import CardRoomSurface from '../../app/workspace/runtime/CardRoomSurface.svelte';
import type { InteractionRoomCapableClient } from './interaction-room-controller';
import type { ProviderWorkspaceDto } from '../../lib/ipc/contracts';
import { PortableRuntimeLifecycle } from './portable-runtime-lifecycle.svelte';
import { message, profile } from './tests/portable-runtime-fixtures';

afterEach(cleanup);

it.each([false, true])(
    'preserves global card macro indices for 2,000 messages, live=%s',
    async (live) => {
        const messages = Array.from({ length: 128 }, (_, index) =>
            message(String(index + 1872), 'assistant', 'Hello'),
        );
        const displayMessages = live ? messages.slice(0, -1) : messages;
        const runtime = new PortableRuntimeLifecycle({
            currentMessages: () => messages,
            currentMessageWindow: () => ({
                start_index: 1872,
                total_messages: 2000,
                head_message_id: '1999',
            }),
            displayMessages: () => displayMessages,
            providerWorkspace: () =>
                ({
                    presets: [],
                    routes: [],
                    legacy_profiles: [],
                }) as unknown as ProviderWorkspaceDto,
            primarySelection: () => null,
            sendMessage: () => Promise.resolve(false),
            onNotice: vi.fn(),
        });
        const client = {
            getCharacterRenderProfile: vi.fn().mockResolvedValue({
                ...profile(),
                background_markup: '<div id="indices">{{chat_index}}/{{lastmessageid}}</div>',
            }),
        } as unknown as InteractionRoomCapableClient;
        const dispose = runtime.loadProfile(client, 'character', 'conversation', 'branch');
        try {
            await waitFor(() => expect(runtime.profileLoading).toBe(false));
            await runtime.approveDisplay();
            const view = render(CardRoomSurface, { runtime, client });
            await waitFor(() => {
                const frame = view.container.querySelector('iframe');
                const doc = new DOMParser().parseFromString(frame?.srcdoc ?? '', 'text/html');
                expect(doc.querySelector('#indices')?.textContent).toBe(
                    live ? '1998/1998' : '1999/1999',
                );
            });
        } finally {
            dispose();
        }
    },
);
