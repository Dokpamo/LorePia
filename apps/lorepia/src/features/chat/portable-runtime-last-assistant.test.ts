import { cleanup, render, waitFor } from '@testing-library/svelte';
import { afterEach, expect, it, vi } from 'vitest';
import CardRoomSurface from '../../app/workspace/runtime/CardRoomSurface.svelte';
import type { InteractionRoomCapableClient } from './interaction-room-controller';
import type { MessageDto, ProviderWorkspaceDto } from '../../lib/ipc/contracts';
import type { PortableCharacterRuntime } from './portable-runtime';
import { PortableRuntimeLifecycle } from './portable-runtime-lifecycle.svelte';
import { message, profile } from './tests/portable-runtime-fixtures';

afterEach(cleanup);

it.each(['older', 'tail', 'live'] as const)(
    'preserves last assistant card text for a %s aggregate',
    async (source) => {
        const older = message('older', 'assistant', 'original older text');
        const tail = message('tail', 'assistant', 'tail text');
        const current = Array.from({ length: 128 }, (_, index) =>
            message(String(index), 'user', 'user text'),
        );
        if (source !== 'older') current[127] = tail;
        const display = source === 'live' ? current.slice(0, -1) : current;
        const fallback = source === 'live' ? tail : older;
        const runtime = new PortableRuntimeLifecycle({
            currentMessages: () => current,
            currentMessageWindow: () => ({
                start_index: 1872,
                total_messages: 2000,
                head_message_id: current[127]?.id ?? null,
            }),
            currentLastAssistantMessage: () => fallback,
            displayMessages: () => display,
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
        const markup = '<div id="last-assistant">{{lastcharmessage}}</div>';
        const client = {
            getCharacterRenderProfile: vi
                .fn()
                .mockResolvedValue({ ...profile(), background_markup: markup }),
        } as unknown as InteractionRoomCapableClient;
        const dispose = runtime.loadProfile(client, 'character', 'conversation', 'branch');
        try {
            await waitFor(() => expect(runtime.profileLoading).toBe(false));
            await runtime.approveDisplay();
            const effectiveText = vi.fn((item: MessageDto) =>
                item.id === older.id ? 'overridden older text' : item.content,
            );
            runtime.runtime = {
                effectiveText,
                variables: {},
                backgroundMarkup: markup,
            } as unknown as PortableCharacterRuntime;
            const view = render(CardRoomSurface, { runtime, client });
            await waitFor(() => {
                const frame = view.container.querySelector('iframe');
                const doc = new DOMParser().parseFromString(frame?.srcdoc ?? '', 'text/html');
                expect(doc.querySelector('#last-assistant')?.textContent).toBe(
                    source === 'older'
                        ? 'overridden older text'
                        : source === 'tail'
                          ? 'tail text'
                          : '',
                );
            });
            if (source === 'older') expect(effectiveText).toHaveBeenCalledWith(older);
            if (source === 'live') expect(effectiveText).not.toHaveBeenCalled();
            expect(current).toHaveLength(128);
        } finally {
            dispose();
        }
    },
);
