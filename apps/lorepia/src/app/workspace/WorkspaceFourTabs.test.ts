import { cleanup, fireEvent, render, screen, waitFor, within } from '@testing-library/svelte';
import { afterEach, describe, expect, it, vi } from 'vitest';
import WorkspaceApp from './WorkspaceApp.svelte';
import { createPreviewClient } from '../../preview/mock-client';
import { t } from '../../lib/i18n';

afterEach(cleanup);
const tab = (name: string) =>
    within(screen.getByRole('navigation', { name: t('navigation.label') })).getByRole('button', {
        name,
    });
async function openSearch(label: string) {
    await fireEvent.click(await screen.findByRole('button', { name: label }));
    const input = await screen.findByRole('searchbox', { name: label });
    await waitFor(() => expect(input).toHaveFocus());
    expect(screen.queryByRole('navigation')).toBeNull();
    return input;
}
async function closeSearch(label: string) {
    const page = screen.getByRole('dialog', { name: label });
    await fireEvent.click(within(page).getByRole('button', { name: t('uiPreview.back') }));
    await waitFor(() => expect(screen.queryByRole('dialog', { name: label })).toBeNull());
    expect(screen.getByRole('button', { name: label })).toHaveFocus();
}
async function edit(label: string, value: string) {
    await fireEvent.click(screen.getByRole('button', { name: label }));
    await fireEvent.input(await screen.findByRole('textbox', { name: label }), {
        target: { value },
    });
    await fireEvent.click(screen.getByRole('button', { name: t('uiPreview.editDone') }));
    await waitFor(() => expect(screen.queryByRole('textbox', { name: label })).toBeNull());
}
describe('four root destinations', () => {
    it('sorts all chats from the header and keeps search and sorting across tab changes', async () => {
        const client = createPreviewClient();
        const [first, second] = await client.listCharacters();
        if (!first || !second) throw new Error('Missing characters');
        const items = [
            {
                id: 'chat-study',
                character_id: first.id,
                title: 'Study',
                updated_at: '2026-09-07T12:00:00Z',
            },
            {
                id: 'chat-zoo',
                character_id: second.id,
                title: 'Zoo',
                updated_at: '2026-09-08T12:00:00Z',
            },
            {
                id: 'chat-library',
                character_id: first.id,
                title: 'Library',
                updated_at: '2026-09-06T12:00:00Z',
            },
        ].map((item) => ({ ...item, created_at: '2026-09-01T12:00:00Z' }));
        vi.spyOn(client, 'listConversations').mockImplementation((characterId) =>
            Promise.resolve(
                items.filter((item) => !characterId || item.character_id === characterId),
            ),
        );
        render(WorkspaceApp, { client });
        await screen.findByRole('button', {
            name: t('uiPreview.cardSelect', { name: first.name }),
        });
        await fireEvent.click(tab(t('navigation.chats')));
        const list = within(screen.getByRole('region', { name: t('uiPreview.history') }));
        const names = items.map(
            (item) =>
                `${item.title} · ${item.character_id === first.id ? first.name : second.name}`,
        );
        const rows = () => list.getAllByRole('button', { name: (name) => names.includes(name) });
        await waitFor(() => expect(rows()).toHaveLength(3));
        expect(rows()[0]).toHaveAccessibleName(names[1]);
        expect(screen.queryByRole('group', { name: t('navigation.filterCharacters') })).toBeNull();
        await fireEvent.click(screen.getByRole('button', { name: t('navigation.sortChats') }));
        await fireEvent.click(
            await screen.findByRole('radio', { name: t('navigation.sortOldest') }),
        );
        expect(rows()[0]).toHaveAccessibleName(names[2]);
        const search = await openSearch(t('navigation.searchChats'));
        const searchPage = within(
            screen.getByRole('dialog', { name: t('navigation.searchChats') }),
        );
        const results = () =>
            searchPage.getAllByRole('button', { name: (name) => names.includes(name) });
        expect(results()).toHaveLength(3);
        await fireEvent.input(search, { target: { value: 'STUDY' } });
        expect(results()).toHaveLength(1);
        await fireEvent.click(
            searchPage.getByRole('button', { name: t('navigation.clearSearch') }),
        );
        expect(search).toHaveFocus();
        expect(search).toHaveValue('');
        expect(results()[0]).toHaveAccessibleName(names[2]);
        await closeSearch(t('navigation.searchChats'));
        expect(rows()[0]).toHaveAccessibleName(names[2]);
        await fireEvent.click(tab(t('navigation.home')));
        await fireEvent.click(tab(t('navigation.chats')));
        expect(rows()).toHaveLength(3);
        expect(rows()[0]).toHaveAccessibleName(names[2]);
        await fireEvent.click(screen.getByRole('button', { name: t('navigation.sortChats') }));
        await fireEvent.click(
            await screen.findByRole('radio', { name: t('navigation.sortTitle') }),
        );
        expect(rows().map((row) => row.getAttribute('aria-label'))).toEqual([
            names[2],
            names[0],
            names[1],
        ]);
        expect(items.map((item) => item.id)).toEqual(['chat-study', 'chat-zoo', 'chat-library']);
    });
    it('sorts the library from the header and clears search without showing tag pills', async () => {
        const client = createPreviewClient();
        const characters = await client.listCharacters();
        const oldest = [...characters].sort((a, b) => a.created_at.localeCompare(b.created_at));
        const first = oldest[0];
        const last = oldest.at(-1);
        if (!first || !last) throw new Error('Missing dated characters');
        render(WorkspaceApp, { client });
        await screen.findByRole('button', {
            name: t('uiPreview.cardSelect', { name: first.name }),
        });
        expect(screen.queryByRole('group', { name: t('navigation.characterTags') })).toBeNull();
        expect(screen.queryByRole('searchbox')).toBeNull();
        const cards = () =>
            screen.getAllByRole('button', {
                name: (name) =>
                    characters.some(
                        (character) => name === t('uiPreview.cardSelect', { name: character.name }),
                    ),
            });
        expect(cards()[0]).toHaveAccessibleName(t('uiPreview.cardSelect', { name: last.name }));
        expect(cards()).toHaveLength(characters.length);
        await fireEvent.click(screen.getByRole('button', { name: t('navigation.sortCharacters') }));
        await fireEvent.click(
            await screen.findByRole('radio', { name: t('navigation.sortOldest') }),
        );
        expect(cards()[0]).toHaveAccessibleName(t('uiPreview.cardSelect', { name: first.name }));
        const search = await openSearch(t('navigation.searchCharacters'));
        await fireEvent.input(search, { target: { value: last.name } });
        expect(cards()).toHaveLength(1);
        await fireEvent.click(screen.getByRole('button', { name: t('navigation.clearSearch') }));
        expect(search).toHaveValue('');
        expect(search).toHaveFocus();
        expect(cards()).toHaveLength(characters.length);
        expect(cards()[0]).toHaveAccessibleName(t('uiPreview.cardSelect', { name: first.name }));
        await closeSearch(t('navigation.searchCharacters'));
        expect(screen.queryByRole('searchbox')).toBeNull();
        expect(cards()).toHaveLength(characters.length);
    });
    it('returns chat detail through its search page and keeps both queries and the draft', async () => {
        const client = createPreviewClient();
        const [character] = await client.listCharacters();
        if (!character) throw new Error('Missing fixture');
        render(WorkspaceApp, { client });
        await screen.findByRole('button', {
            name: t('uiPreview.cardSelect', { name: character.name }),
        });
        const search = await openSearch(t('navigation.searchCharacters'));
        await fireEvent.input(search, { target: { value: character.name } });
        await closeSearch(t('navigation.searchCharacters'));
        await fireEvent.click(tab(t('navigation.chats')));
        const [conversation] = await client.listConversations(character.id);
        if (!conversation) throw new Error('Missing conversation');
        const chatSearch = await openSearch(t('navigation.searchChats'));
        await fireEvent.input(chatSearch, { target: { value: conversation.title } });
        await fireEvent.click(
            await screen.findByRole('button', {
                name: `${conversation.title} · ${character.name}`,
            }),
        );
        const composer = await screen.findByRole('textbox', { name: t('uiPreview.message') });
        await fireEvent.input(composer, { target: { value: 'A draft to retain' } });
        expect(screen.queryByRole('navigation')).toBeNull();
        await fireEvent.keyDown(window, { key: 'Escape' });
        expect(
            await screen.findByRole('searchbox', { name: t('navigation.searchChats') }),
        ).toHaveValue(conversation.title);
        expect(screen.queryByRole('navigation')).toBeNull();
        await fireEvent.keyDown(window, { key: 'Escape' });
        await screen.findByRole('navigation');
        expect(tab(t('navigation.chats'))).toHaveAttribute('aria-current', 'page');
        await fireEvent.click(tab(t('navigation.home')));
        expect(await openSearch(t('navigation.searchCharacters'))).toHaveValue(character.name);
        await closeSearch(t('navigation.searchCharacters'));
        await fireEvent.click(tab(t('navigation.chats')));
        expect(await openSearch(t('navigation.searchChats'))).toHaveValue(conversation.title);
        await fireEvent.click(
            await screen.findByRole('button', {
                name: `${conversation.title} · ${character.name}`,
            }),
        );
        expect(await screen.findByRole('textbox', { name: t('uiPreview.message') })).toHaveValue(
            'A draft to retain',
        );
    });
    it('creates a material from the Create root with no selected character or conversation', async () => {
        const client = createPreviewClient();
        vi.spyOn(client, 'listCharacters').mockResolvedValue([]);
        const save = vi.spyOn(client, 'upsertKnowledgeBook');
        render(WorkspaceApp, { client });
        await screen.findByText(t('workspace.emptyLibrary'));
        await fireEvent.click(tab(t('navigation.create')));
        await fireEvent.click(
            screen.getByRole('button', { name: new RegExp('^' + t('navigation.createLorebook')) }),
        );
        const create = await screen.findByRole('button', { name: t('navigation.createMaterial') });
        await waitFor(() => expect(create).toBeEnabled());
        await fireEvent.click(create);
        await edit(t('navigation.name'), 'A new world');
        await fireEvent.click(screen.getByRole('button', { name: t('navigation.save') }));
        await waitFor(() => expect(save).toHaveBeenCalledOnce());
        expect(save.mock.calls[0]?.[0].value.name).toBe('A new world');
        expect(save.mock.calls[0]?.[0].expected_revision).toBeNull();
        await screen.findByText(t('navigation.saved'));
    });
});
