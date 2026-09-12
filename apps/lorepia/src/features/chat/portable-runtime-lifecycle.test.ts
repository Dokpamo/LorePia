import { describe, expect, it, vi } from 'vitest';
import { waitFor } from '@testing-library/svelte';
import { PortableRuntimeLifecycle } from './portable-runtime-lifecycle.svelte';
import type { InteractionRoomCapableClient } from './interaction-room-controller';
import type { CharacterRenderProfileDto, ProviderWorkspaceDto } from '../../lib/ipc/contracts';
import { profile } from './tests/portable-runtime-fixtures';
import { PortableCharacterRuntime } from './portable-runtime';
import { deferred } from '../../tests/deferred';

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

it('does not republish an old runtime when its display refresh finishes after scope cleanup', async () => {
    const owner = lifecycle();
    const client = {
        getCharacterRenderProfile: vi.fn().mockResolvedValue(profile()),
    } as unknown as InteractionRoomCapableClient;
    const stopProfile = owner.loadProfile(client, 'character', 'conversation', 'branch');
    await waitFor(() => expect(owner.profileLoading).toBe(false));
    await owner.approveDisplay();
    const oldDisplay = deferred<undefined>();
    const oldRuntime = {
        setMessages: vi.fn(),
        refreshDisplay: vi.fn(() => oldDisplay.promise),
        close: vi.fn(),
    };
    const newRuntime = {
        setMessages: vi.fn(),
        refreshDisplay: vi.fn().mockResolvedValue(undefined),
        close: vi.fn(),
    };
    const create = vi
        .spyOn(PortableCharacterRuntime, 'create')
        .mockResolvedValueOnce(oldRuntime as unknown as PortableCharacterRuntime)
        .mockResolvedValueOnce(newRuntime as unknown as PortableCharacterRuntime);
    const context = {
        client,
        character: { name: 'Character', description: '' },
        conversationId: 'conversation',
        branchId: 'branch',
    };
    let stopOld: (() => void) | undefined;
    let stopNew: (() => void) | undefined;
    try {
        stopOld = owner.recreate(context);
        await waitFor(() => expect(oldRuntime.refreshDisplay).toHaveBeenCalledOnce());
        stopOld();
        stopNew = owner.recreate({ ...context, branchId: 'new-branch' });
        await waitFor(() =>
            expect(owner.runtime).toMatchObject({ refreshDisplay: newRuntime.refreshDisplay }),
        );
        oldDisplay.resolve(undefined);
        await oldDisplay.promise;
        await new Promise((resolve) => setTimeout(resolve, 0));
        expect(owner.runtime).toMatchObject({ refreshDisplay: newRuntime.refreshDisplay });
        expect(owner.phase).toBe('ready');
        expect(oldRuntime.close).toHaveBeenCalled();
    } finally {
        stopOld?.();
        stopNew?.();
        stopProfile();
        create.mockRestore();
    }
});

it('closes immediately and waits for deletion persistence, rejecting a stale head after lookup', async () => {
    for (const remainsCurrent of [true, false]) {
        const owner = lifecycle();
        const proof = deferred<{ retained_message_ids: string[]; head_message_id: string }>();
        const drain = deferred<undefined>();
        let current = true;
        const retired = {
            messageOverrideIds: ['deleted', 'older'],
            close: vi.fn(),
            forgetDeletedMessages: vi.fn(() => drain.promise),
        };
        owner.runtime = retired as unknown as PortableCharacterRuntime;
        const client = {
            listBranchMessagesPage: vi.fn(() => proof.promise),
        } as unknown as InteractionRoomCapableClient;
        const reset = vi.spyOn(owner, 'resetScope');
        const cleanup = owner.forgetDeletedMessages(
            'conversation:branch',
            'branch',
            'head',
            client,
            () => current,
        );
        expect(retired.close).toHaveBeenCalledOnce();
        expect(owner.runtime).toBeNull();
        current = remainsCurrent;
        proof.resolve({ head_message_id: 'head', retained_message_ids: ['older'] });
        await proof.promise;
        if (current) {
            await waitFor(() =>
                expect(retired.forgetDeletedMessages).toHaveBeenCalledWith(['deleted']),
            );
            expect(reset).not.toHaveBeenCalled();
        }
        drain.resolve(undefined);
        await cleanup;
        expect(retired.forgetDeletedMessages).toHaveBeenCalledTimes(current ? 1 : 0);
        expect(reset).toHaveBeenCalledOnce();
    }
});

it('cleans pending initial creation before publishing a replacement runtime', async () => {
    const owner = lifecycle();
    const lookup = vi.fn().mockResolvedValue({ head_message_id: 'head', retained_message_ids: [] });
    const client = {
        getCharacterRenderProfile: vi.fn().mockResolvedValue(profile()),
        listBranchMessagesPage: lookup,
    } as unknown as InteractionRoomCapableClient;
    const stopProfile = owner.loadProfile(client, 'character', 'conversation', 'branch');
    await waitFor(() => expect(owner.profileLoading).toBe(false));
    await owner.approveDisplay();
    const drain = deferred<undefined>();
    const initial = {
        messageOverrideIds: ['deleted'],
        close: vi.fn(),
        forgetDeletedMessages: vi.fn(() => drain.promise),
        refreshDisplay: vi.fn(),
        setMessages: vi.fn(),
    };
    const next = {
        close: vi.fn(),
        refreshDisplay: vi.fn().mockResolvedValue(undefined),
        setMessages: vi.fn(),
    };
    const create = vi
        .spyOn(PortableCharacterRuntime, 'create')
        .mockResolvedValueOnce(initial as unknown as PortableCharacterRuntime)
        .mockResolvedValueOnce(next as unknown as PortableCharacterRuntime);
    const context = {
        client,
        character: { name: 'Character', description: '' },
        conversationId: 'conversation',
        branchId: 'branch',
    };
    let stop: (() => void) | undefined;
    try {
        await owner.forgetDeletedMessages(
            'conversation:branch',
            'branch',
            'head',
            client,
            () => true,
        );
        stop = owner.recreate(context);
        await waitFor(() =>
            expect(initial.forgetDeletedMessages).toHaveBeenCalledWith(['deleted']),
        );
        expect(initial.close).toHaveBeenCalledOnce();
        expect(initial.refreshDisplay).not.toHaveBeenCalled();
        expect(owner.runtime).toBeNull();
        drain.resolve(undefined);
        await drain.promise;
        await new Promise((resolve) => setTimeout(resolve, 0));
        expect(owner.runtime).toBeNull();
        stop();
        stop = owner.recreate(context);
        await waitFor(() =>
            expect(owner.runtime).toMatchObject({ refreshDisplay: next.refreshDisplay }),
        );
        expect(lookup).toHaveBeenCalledOnce();
    } finally {
        stop?.();
        stopProfile();
        create.mockRestore();
    }
});
