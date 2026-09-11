import { describe, expect, it, vi } from 'vitest';
import { waitFor } from '@testing-library/svelte';
import { PortableRuntimeLifecycle } from './portable-runtime-lifecycle.svelte';
import type { InteractionRoomCapableClient } from './interaction-room-controller';
import type { CharacterRenderProfileDto, ProviderWorkspaceDto } from '../../lib/ipc/contracts';
import { profile } from './tests/portable-runtime-fixtures';

function lifecycle(): PortableRuntimeLifecycle {
    return new PortableRuntimeLifecycle({
        currentMessages: () => [],
        displayMessages: () => [],
        providerWorkspace: () =>
            ({ presets: [], routes: [], legacy_profiles: [] }) as unknown as ProviderWorkspaceDto,
        primarySelection: () => null,
        sendMessage: () => Promise.resolve(false),
        onNotice: vi.fn(),
    });
}

describe('portable profile lifecycle', () => {
    it('makes a failed profile load retryable instead of silently removing the card screen', async () => {
        const runtime = lifecycle();
        const getCharacterRenderProfile = vi
            .fn()
            .mockRejectedValueOnce({ code: 'busy' })
            .mockResolvedValue(profile());
        const dispose = runtime.loadProfile(
            { getCharacterRenderProfile } as unknown as InteractionRoomCapableClient,
            'character',
            'conversation',
            'branch',
        );
        await waitFor(() => expect(runtime.profileError).not.toBeNull());
        expect(runtime.profileLoading).toBe(false);
        runtime.retryProfile();
        await waitFor(() => expect(runtime.profile?.character_id).toBe('character'));
        expect(runtime.profileError).toBeNull();
        expect(getCharacterRenderProfile).toHaveBeenCalledTimes(2);
        dispose();
    });

    it('ignores old room results and grants only the explicitly enabled display capabilities', async () => {
        const runtime = lifecycle();
        let resolveOld: (value: CharacterRenderProfileDto) => void = vi.fn();
        const first = new Promise<CharacterRenderProfileDto>((resolve) => {
            resolveOld = resolve;
        });
        const getCharacterRenderProfile = vi
            .fn()
            .mockReturnValueOnce(first)
            .mockResolvedValue({ ...profile(), character_id: 'next' });
        const client = { getCharacterRenderProfile } as unknown as InteractionRoomCapableClient;
        const closeOld = runtime.loadProfile(client, 'character', 'old-room', 'old-branch');
        closeOld();
        const closeNext = runtime.loadProfile(client, 'next', 'new-room', 'new-branch');
        await waitFor(() => expect(runtime.profile?.character_id).toBe('next'));
        resolveOld(profile());
        await first;
        expect(runtime.profile?.character_id).toBe('next');
        expect(runtime.activeGrant).toBeNull();
        await runtime.approveDisplay();
        expect(runtime.activeGrant?.capabilities).toContain('ui:write');
        expect(runtime.activeGrant?.capabilities).toContain('state:readwrite');
        expect(runtime.activeGrant?.capabilities).not.toContain('model:primary');
        expect(runtime.activeGrant?.capabilities).not.toContain('model:auxiliary');
        expect(runtime.activeGrant?.capabilities).toContain('chat:write');
        closeNext();
    });
});
