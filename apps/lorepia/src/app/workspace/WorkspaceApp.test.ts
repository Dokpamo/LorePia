import { cleanup, fireEvent, render, screen, waitFor } from '@testing-library/svelte';
import { afterEach, beforeAll, describe, expect, it, vi } from 'vitest';
import WorkspaceApp from './WorkspaceApp.svelte';
import { createPreviewClient } from '../../preview/mock-client';
import { t } from '../../lib/i18n';

beforeAll(() => {
    if (!Element.prototype.getAnimations)
        Object.defineProperty(Element.prototype, 'getAnimations', {
            configurable: true,
            value: () => [],
        });
});
afterEach(cleanup);

describe('connected workspace', () => {
    it('loads characters and histories through the client and opens a real conversation', async () => {
        const client = createPreviewClient();
        const listCharacters = vi.spyOn(client, 'listCharacters');
        const listConversations = vi.spyOn(client, 'listConversations');
        const openConversation = vi.spyOn(client, 'openExistingConversation');
        const character = (await client.listCharacters())[0]!;
        const histories = await client.listConversations(character.id);
        const view = render(WorkspaceApp, { client });
        await screen.findByRole('button', {
            name: t('uiPreview.cardSelect', { name: character.name }),
        });
        expect(listCharacters).toHaveBeenCalled();
        await waitFor(() => expect(listConversations).toHaveBeenCalledWith(character.id));
        const row = await screen.findByRole('button', {
            name: new RegExp('^' + histories[0]!.title + ' ·'),
        });
        await fireEvent.click(row);
        await screen.findByRole('textbox', { name: t('uiPreview.message') });
        expect(openConversation).toHaveBeenCalledWith(histories[0]!.id);
        expect(view.container.querySelector('.ui-chat > .ui-compose')).not.toBeNull();
        expect(screen.queryByText('서연')).toBeNull();
    });
    it('shows an empty library without injecting sample characters', async () => {
        const client = createPreviewClient();
        vi.spyOn(client, 'listCharacters').mockResolvedValue([]);
        render(WorkspaceApp, { client });
        await screen.findByText(t('workspace.emptyLibrary'));
        expect(screen.queryByRole('button', { name: /캐릭터 선택$/ })).toBeNull();
        expect(screen.getByRole('button', { name: t('workspace.importCard') })).toBeEnabled();
    });
    it('imports from the native source picker and keeps its review before committing', async () => {
        const client = createPreviewClient();
        vi.spyOn(client, 'listCharacters').mockResolvedValue([]);
        const pick = vi.spyOn(client, 'selectImportSource').mockResolvedValue(null);
        const commit = vi.fn(() => Promise.reject(new Error('Commit requires explicit review')));
        client.commitImport = commit;
        render(WorkspaceApp, { client });
        const button = await screen.findByRole('button', { name: t('workspace.importCard') });
        await waitFor(() => expect(button).toBeEnabled());
        await fireEvent.click(button);
        await waitFor(() => expect(pick).toHaveBeenCalled());
        expect(commit).not.toHaveBeenCalled();
    });
    it('persists the chat/story choice through the controller and reads it back on reopen', async () => {
        const client = createPreviewClient();
        const character = (await client.listCharacters())[0]!;
        const conversation = (await client.listConversations(character.id))[0]!;
        const save = vi.spyOn(client, 'setConversationMode');
        render(WorkspaceApp, { client });
        await fireEvent.click(
            await screen.findByRole('button', {
                name: new RegExp('^' + conversation.title + ' ·'),
            }),
        );
        await fireEvent.click(
            await screen.findByRole('button', { name: t('uiPreview.roomSettings') }),
        );
        await fireEvent.click(screen.getByRole('button', { name: t('workspace.roomMode') }));
        await fireEvent.click(screen.getByRole('radio', { name: t('uiPreview.storyMode') }));
        await waitFor(() => expect(save).toHaveBeenCalledWith(conversation.id, 'story'));
        expect((await client.getConversationState(conversation.id)).selected_mode).toBe('story');
    });
});
