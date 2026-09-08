import { cleanup, fireEvent, render, screen, waitFor } from '@testing-library/svelte';
import { afterEach, describe, expect, it, vi } from 'vitest';
import { writable } from 'svelte/store';
import { INITIAL_APP_STATE, type LorepiaAppController } from '../../app/app-controller';
import MobileHome from '../../features/library/MobileHome.svelte';
import MobileConversations from '../../features/conversations/MobileConversations.svelte';
import { t } from '../../lib/i18n';
import type { CharacterDto, ConversationDto, LorepiaClient } from '../../lib/ipc/contracts';

afterEach(cleanup);

const aria: CharacterDto = {
    id: 'aria',
    name: 'Aria',
    description: 'Librarian',
    source_hash: 'synthetic',
    avatar_asset_id: null,
    created_at: '2026-08-03T00:00:00Z',
};
const kai: CharacterDto = { ...aria, id: 'kai', name: 'Kai', description: 'Mechanic' };
const first: ConversationDto = {
    id: 'first',
    character_id: aria.id,
    title: 'The archive',
    created_at: '2026-08-03T00:00:00Z',
    updated_at: '2026-08-03T00:00:00Z',
};
const recent: ConversationDto = {
    ...first,
    id: 'recent',
    character_id: kai.id,
    title: 'The workshop',
    updated_at: '2026-08-04T00:00:00Z',
};

function fixture() {
    const state = structuredClone(INITIAL_APP_STATE);
    state.library = { phase: 'ready', error: null, characters: [aria, kai] };
    state.providers.phase = 'ready';
    state.selected_character = aria;
    state.selected_conversation = first;
    state.conversations = { phase: 'ready', error: null, items: [first] };
    const store = writable(state);
    const selectCharacter = vi.fn((character: CharacterDto) => {
        store.set({ ...state, selected_character: character });
        return Promise.resolve();
    });
    const selectConversation = vi
        .fn<(_conversation: ConversationDto) => Promise<boolean>>()
        .mockResolvedValue(true);
    const beginImport = vi.fn();
    const controller = {
        state: store,
        selectCharacter,
        selectConversation,
        beginImport,
    } as unknown as LorepiaAppController;
    const listConversations = vi.fn().mockResolvedValue([first, recent]);
    const client = { listConversations } as unknown as LorepiaClient;
    return {
        state,
        controller,
        client,
        listConversations,
        selectCharacter,
        selectConversation,
        beginImport,
    };
}

describe('mobile destination workflows', () => {
    it('filters characters and restores search focus and the full list on Escape', async () => {
        const props = fixture();
        render(MobileHome, { ...props, onOpenChat: vi.fn(), onOpenSettings: vi.fn() });
        const trigger = screen.getByRole('button', { name: t('library.search.label') });
        await fireEvent.click(trigger);
        const input = screen.getByRole('searchbox', { name: t('library.search.label') });
        expect(input).toHaveFocus();
        await fireEvent.input(input, { target: { value: 'Mechanic' } });
        expect(screen.queryByRole('button', { name: /Aria Librarian/ })).not.toBeInTheDocument();
        expect(screen.getByRole('button', { name: /Kai Mechanic/ })).toBeVisible();
        await fireEvent.keyDown(input, { key: 'Escape' });
        expect(trigger).toHaveFocus();
        expect(screen.queryByRole('searchbox')).not.toBeInTheDocument();
        expect(screen.getByRole('button', { name: /Aria Librarian/ })).toBeVisible();
    });

    it('opens a character through the existing selection workflow and preserves import and resume actions', async () => {
        const props = fixture();
        const onOpenChat = vi.fn();
        const onOpenSettings = vi.fn();
        render(MobileHome, { ...props, onOpenChat, onOpenSettings });
        await fireEvent.click(screen.getByRole('button', { name: /Aria Librarian/ }));
        await waitFor(() => expect(onOpenChat).toHaveBeenCalledWith(true));
        expect(props.selectCharacter).toHaveBeenCalledWith(aria);
        expect(props.selectConversation).toHaveBeenCalledWith(first);
        expect(props.selectCharacter.mock.invocationCallOrder[0] ?? Infinity).toBeLessThan(
            props.selectConversation.mock.invocationCallOrder[0] ?? 0,
        );
        const importButton = screen.getAllByRole('button', { name: t('library.empty.import') })[0];
        if (!importButton) throw new Error('Import action is missing');
        await fireEvent.click(importButton);
        expect(props.beginImport).toHaveBeenCalledOnce();
        await fireEvent.click(
            screen.getByRole('button', { name: new RegExp(t('mobile.home.connect')) }),
        );
        expect(onOpenSettings).toHaveBeenCalledOnce();
        await fireEvent.click(
            screen.getByRole('button', { name: new RegExp(t('mobile.home.continue')) }),
        );
        expect(onOpenChat).toHaveBeenCalledTimes(2);
    });

    it('loads all conversations, filters by character, and opens only after selection succeeds', async () => {
        const props = fixture();
        const onOpenChat = vi.fn();
        props.selectConversation.mockResolvedValue(false);
        const rendered = render(MobileConversations, { ...props, onOpenChat });
        const row = await screen.findByRole('button', { name: /The workshop/ });
        expect(props.listConversations).toHaveBeenCalledWith(null);
        expect(rendered.container.querySelector('.mobile-card-list li')).toContainElement(row);
        await fireEvent.click(
            screen.getByRole('combobox', { name: new RegExp(t('conversation.filter.label')) }),
        );
        await fireEvent.click(screen.getByRole('option', { name: kai.name }));
        expect(screen.queryByRole('button', { name: /The archive/ })).not.toBeInTheDocument();
        await fireEvent.click(row);
        await waitFor(() => expect(props.selectConversation).toHaveBeenCalledWith(recent));
        expect(props.selectCharacter).toHaveBeenCalledWith(kai);
        expect(onOpenChat).not.toHaveBeenCalled();
        props.selectConversation.mockResolvedValue(true);
        await fireEvent.click(row);
        await waitFor(() => expect(onOpenChat).toHaveBeenCalledOnce());
    });

    it('keeps the selected character conversations accessible when the cross-character index fails', async () => {
        const props = fixture();
        vi.mocked(props.listConversations).mockRejectedValue(new Error('offline'));
        render(MobileConversations, { ...props, onOpenChat: vi.fn() });
        await waitFor(() => expect(props.listConversations).toHaveBeenCalledWith(null));
        expect(screen.getByRole('button', { name: /The archive/ })).toBeVisible();
        await fireEvent.click(screen.getByRole('button', { name: t('mobile.chat.search') }));
        const input = screen.getByRole('searchbox', { name: t('mobile.chat.search') });
        expect(input).toHaveFocus();
        await fireEvent.input(input, { target: { value: 'no match' } });
        expect(screen.getByText(t('mobile.chat.no_results'))).toBeVisible();
        await fireEvent.keyDown(input, { key: 'Escape' });
        expect(screen.getByRole('button', { name: t('mobile.chat.search') })).toHaveFocus();
        expect(screen.getByRole('button', { name: /The archive/ })).toBeVisible();
    });
});
