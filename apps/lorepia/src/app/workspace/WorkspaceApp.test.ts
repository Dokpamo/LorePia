import { cleanup, fireEvent, render, screen, waitFor } from '@testing-library/svelte';
import { afterEach, beforeAll, describe, expect, it, vi } from 'vitest';
import WorkspaceApp from './WorkspaceApp.svelte';
import { createPreviewClient } from '../../preview/mock-client';
import { t } from '../../lib/i18n';
import type { LorepiaClient } from '../../lib/ipc/contracts';
import { createSampleCharacters } from '../../preview/ui/sample-data';

function required<T>(value: T | undefined): T {
    if (value === undefined) throw new Error('Fixture is empty');
    return value;
}

beforeAll(() => {
    if (typeof Reflect.get(Element.prototype, 'getAnimations') !== 'function')
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
        const character = required((await client.listCharacters())[0]);
        const histories = await client.listConversations(character.id);
        const view = render(WorkspaceApp, { client });
        await screen.findByRole('button', {
            name: t('uiPreview.cardSelect', { name: character.name }),
        });
        expect(listCharacters).toHaveBeenCalled();
        await waitFor(() => expect(listConversations).toHaveBeenCalledWith(character.id));
        await fireEvent.click(screen.getByRole('button', { name: t('navigation.chats') }));
        const row = await screen.findByRole('button', {
            name: new RegExp('^' + required(histories[0]).title + ' ·'),
        });
        await fireEvent.click(row);
        await screen.findByRole('textbox', { name: t('uiPreview.message') });
        expect(openConversation).toHaveBeenCalledWith(required(histories[0]).id);
        expect(view.container.querySelector('.ui-chat > .ui-compose')).not.toBeNull();
        expect(screen.queryByText(required(createSampleCharacters()[0]).name)).toBeNull();
    });
    it('shows an empty library without injecting sample characters', async () => {
        const client = createPreviewClient();
        vi.spyOn(client, 'listCharacters').mockResolvedValue([]);
        render(WorkspaceApp, { client });
        await screen.findByText(t('workspace.emptyLibrary'));
        expect(
            screen.queryByRole('button', {
                name: t('uiPreview.cardSelect', {
                    name: required(createSampleCharacters()[0]).name,
                }),
            }),
        ).toBeNull();
        expect(screen.getByRole('button', { name: t('workspace.importCard') })).toBeEnabled();
    });
    it('resolves a character avatar hash through the approved digest selector', async () => {
        const client = createPreviewClient();
        const character = required((await client.listCharacters())[0]);
        const digest = 'ab'.repeat(32);
        vi.spyOn(client, 'listCharacters').mockResolvedValue([
            { ...character, avatar_asset_id: digest },
        ]);
        const resolveAsset = vi
            .fn<LorepiaClient['resolveAssetDelivery']>()
            .mockRejectedValue(new Error('Unavailable test media'));
        client.resolveAssetDelivery = resolveAsset;
        render(WorkspaceApp, { client });
        await waitFor(() =>
            expect(resolveAsset).toHaveBeenCalledWith({
                selector: { kind: 'sha256', sha256: digest },
            }),
        );
        expect(
            resolveAsset.mock.calls.every(([request]) => request.selector.kind === 'sha256'),
        ).toBe(true);
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
        const character = required((await client.listCharacters())[0]);
        const conversation = required((await client.listConversations(character.id))[0]);
        const save = vi.spyOn(client, 'setConversationMode');
        const view = render(WorkspaceApp, { client });
        await fireEvent.click(screen.getByRole('button', { name: t('navigation.chats') }));
        await fireEvent.click(
            await screen.findByRole('button', {
                name: new RegExp('^' + conversation.title + ' ·'),
            }),
        );
        await waitFor(() =>
            expect(view.container.querySelector('.seed-detail-active')).not.toBeNull(),
        );
        await fireEvent.click(
            await screen.findByRole('button', { name: t('uiPreview.roomSettings') }),
        );
        expect(screen.queryByRole('textbox', { name: t('uiPreview.message') })).toBeNull();
        await fireEvent.click(
            screen.getByRole('radio', { name: new RegExp(t('uiPreview.storyMode')) }),
        );
        await fireEvent.click(screen.getByRole('button', { name: t('uiPreview.save') }));
        await waitFor(() => expect(save).toHaveBeenCalledWith(conversation.id, 'story'));
        expect((await client.getConversationState(conversation.id)).selected_mode).toBe('story');
    });
    it('forwards the stored message identity and shows a rejected branch as readable feedback', async () => {
        const client = createPreviewClient();
        const character = required((await client.listCharacters())[0]);
        const conversation = required((await client.listConversations(character.id))[0]);
        const branch = vi.spyOn(client, 'createBranch').mockRejectedValue({
            code: 'not_found',
            message_key: 'error.not_found',
            recoverable: false,
            operation_id: null,
            field_errors: [],
        });
        render(WorkspaceApp, { client });
        await fireEvent.click(screen.getByRole('button', { name: t('navigation.chats') }));
        await fireEvent.click(
            await screen.findByRole('button', {
                name: new RegExp('^' + conversation.title + ' ·'),
            }),
        );
        const menu = required(
            (
                await screen.findAllByRole('button', {
                    name: t('uiPreview.messageMenu'),
                })
            )[0],
        );
        const messageId = menu.closest('[data-message-id]')?.getAttribute('data-message-id');
        expect(messageId).toBeTruthy();
        await fireEvent.click(menu);
        const fork = screen.getByRole('button', { name: t('uiPreview.branchFrom') });
        await waitFor(() => expect(fork).toBeEnabled());
        await fireEvent.click(fork);
        await screen.findByText(t('workspace.errorNotFound'));
        expect(branch).toHaveBeenCalledWith(conversation.id, messageId, null);
    });
});
