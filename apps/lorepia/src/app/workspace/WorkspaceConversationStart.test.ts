import { cleanup, fireEvent, render, screen, waitFor, within } from '@testing-library/svelte';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import WorkspaceApp from './WorkspaceApp.svelte';
import { createPreviewClient } from '../../preview/mock-client';
import { t } from '../../lib/i18n';
import type { PersonaDto } from '../../features/personas/persona-contracts';

beforeEach(() =>
    vi.stubGlobal(
        'ResizeObserver',
        class {
            observe = vi.fn();
            unobserve = vi.fn();
            disconnect = vi.fn();
        },
    ),
);
afterEach(() => {
    cleanup();
    vi.unstubAllGlobals();
});

async function openSetup(client = createPreviewClient()) {
    const [character] = await client.listCharacters();
    if (!character) throw new Error('Missing demo character');
    const create = vi.spyOn(client, 'createConversation');
    const select = vi.spyOn(client, 'selectConversationPersona');
    const view = render(WorkspaceApp, { client });
    await fireEvent.click(
        await screen.findByRole('button', {
            name: t('uiPreview.cardSelect', { name: character.name }),
        }),
    );
    const profile = await screen.findByRole('dialog', { name: t('navigation.characterInfo') });
    const start = within(profile).getByRole('button', { name: t('navigation.startChat') });
    await waitFor(() => expect(start).toBeEnabled());
    await fireEvent.click(start);
    const panel = await screen.findByRole('dialog', { name: t('uiPreview.newChat') });
    return { ...view, client, character, create, select, panel, profile };
}

async function chooseStory(panel: HTMLElement) {
    const opener = within(panel).getByRole('button', { name: t('uiPreview.conversationMode') });
    await fireEvent.click(opener);
    const story = await screen.findByRole('radio', { name: t('uiPreview.storyMode') });
    expect(story).toHaveAccessibleDescription(t('uiPreview.storyModeHint'));
    await fireEvent.click(story);
    await waitFor(() => expect(screen.getAllByRole('dialog')).toEqual([panel]));
    expect(opener).toHaveTextContent(t('uiPreview.storyMode'));
}

it('keeps all start choices local until Start and applies them to the new conversation', async () => {
    const { client, character, create, select, panel } = await openSetup();
    const [persona] = await client.listPersonas({ limit: 100 });
    if (!persona || !client.getCharacterRenderProfile)
        throw new Error('Missing demo persona/profile');
    const profile = await client.getCharacterRenderProfile(character.id);
    const catalog = await client.getCharacterGreetingCatalog(character.id);
    const alternate = catalog.greetings.find((item) => item.kind === 'alternate' && item.enabled);
    if (!alternate) throw new Error('Missing starting scene');
    const title = profile.greeting_previews?.find((item) => item.id === alternate.id)?.title;
    if (!title) throw new Error('Missing scene title');
    expect(within(panel).getByRole('button', { name: t('uiPreview.chatName') })).toHaveTextContent(
        t('uiPreview.newChat'),
    );
    const personas = within(panel).getByRole('button', { name: t('persona.title') });
    await waitFor(() => expect(personas).toBeEnabled());
    await fireEvent.click(personas);
    const personaOption = await screen.findByRole('radio', { name: persona.value.name });
    expect(personaOption).toHaveAccessibleDescription(persona.value.description);
    await fireEvent.click(personaOption);
    await waitFor(() => expect(screen.getAllByRole('dialog')).toEqual([panel]));
    await fireEvent.click(
        within(panel).getByRole('button', { name: t('workspace.startingScene') }),
    );
    await fireEvent.click(await screen.findByRole('radio', { name: title }));
    await waitFor(() => expect(screen.getAllByRole('dialog')).toEqual([panel]));
    await chooseStory(panel);
    expect(create).not.toHaveBeenCalled();
    expect(select).not.toHaveBeenCalled();
    await fireEvent.click(within(panel).getByRole('button', { name: t('uiPreview.startChat') }));
    await waitFor(() => expect(screen.queryByRole('dialog')).toBeNull());
    expect(create).toHaveBeenCalledWith(character.id, t('uiPreview.newChat'), 'story', {
        character_content_revision_id: catalog.character_content_revision_id,
        greeting_id: alternate.id,
    });
    expect(select).toHaveBeenCalledWith(expect.objectContaining({ persona_id: persona.value.id }));
    expect(screen.getByRole('textbox', { name: t('uiPreview.message') })).toBeVisible();
});

it('shows the unsaved dialog after departure, returns with its choices, and discards without reopening', async () => {
    const { panel, profile, create } = await openSetup();
    await chooseStory(panel);
    await fireEvent.click(within(panel).getByRole('button', { name: t('uiPreview.back') }));
    const confirm = await screen.findByRole('alertdialog');
    expect(panel).toHaveAttribute('data-back-dismissed', 'true');
    expect(panel.style.getPropertyValue('--ui-back-offset')).toBe('100%');
    expect(panel).toHaveAttribute('aria-hidden', 'true');
    expect(profile.isConnected).toBe(true);
    await fireEvent.click(
        within(confirm).getByRole('button', { name: t('uiPreview.keepEditing') }),
    );
    await waitFor(() => expect(screen.queryByRole('alertdialog')).toBeNull());
    expect(panel).not.toHaveAttribute('data-back-dismissed');
    expect(
        within(panel).getByRole('button', { name: t('uiPreview.conversationMode') }),
    ).toHaveTextContent(t('uiPreview.storyMode'));
    await fireEvent.click(within(panel).getByRole('button', { name: t('uiPreview.back') }));
    await fireEvent.click(
        await screen.findByRole('button', { name: t('uiPreview.discardChanges') }),
    );
    await waitFor(() => expect(screen.getAllByRole('dialog')).toEqual([profile]));
    expect(create).not.toHaveBeenCalled();
});

it('offers persona creation from an empty catalog while still allowing a start without one', async () => {
    const client = createPreviewClient();
    const catalog = await client.listPersonaPage({ limit: 100, after: null });
    if (catalog.kind !== 'page') throw new Error('Missing persona catalog');
    vi.spyOn(client, 'listPersonaPage').mockResolvedValue({
        ...catalog,
        items: [],
        next_cursor: null,
    });
    const { panel, create, select } = await openSetup(client);
    const persona = await within(panel).findByRole('button', { name: t('persona.title') });
    await waitFor(() => expect(persona).toBeEnabled());
    expect(persona).toHaveTextContent(t('workspace.noPersona'));
    await fireEvent.click(persona);
    expect(
        await screen.findByRole('button', { name: t('persona.editor.create_button') }),
    ).toBeEnabled();
    await fireEvent.click(screen.getByRole('radio', { name: t('workspace.noPersona') }));
    await waitFor(() => expect(screen.getAllByRole('dialog')).toEqual([panel]));
    await fireEvent.click(within(panel).getByRole('button', { name: t('uiPreview.startChat') }));
    await waitFor(() => expect(screen.queryByRole('dialog')).toBeNull());
    expect(create).toHaveBeenCalledTimes(1);
    expect(select).not.toHaveBeenCalled();
});

it('creates a saved persona through the full-screen editor and carries its exact identity into Start', async () => {
    const { client, panel, select, create } = await openSetup();
    const add = vi.spyOn(client, 'createPersona');
    const opener = within(panel).getByRole('button', { name: t('persona.title') });
    await waitFor(() => expect(opener).toBeEnabled());
    await fireEvent.click(opener);
    await fireEvent.click(
        await screen.findByRole('button', { name: t('persona.editor.create_button') }),
    );
    const creator = await screen.findByRole('dialog', { name: t('persona.editor.new') });
    expect(panel).toHaveProperty('inert', true);
    for (const [label, value] of [
        [t('persona.editor.name'), 'Traveler'],
        [t('persona.editor.description'), 'A traveler documenting unfamiliar cities'],
    ]) {
        await fireEvent.click(within(creator).getByRole('button', { name: label }));
        await fireEvent.input(await screen.findByRole('textbox', { name: label }), {
            target: { value },
        });
        await fireEvent.click(screen.getByRole('button', { name: t('uiPreview.editDone') }));
        await waitFor(() => expect(screen.getAllByRole('dialog')).toEqual([creator]));
    }
    await fireEvent.click(
        within(creator).getByRole('button', { name: t('persona.editor.submit_create') }),
    );
    await waitFor(() => expect(screen.getAllByRole('dialog')).toEqual([panel]));
    expect(add).toHaveBeenCalledExactlyOnceWith({
        name: 'Traveler',
        description: 'A traveler documenting unfamiliar cities',
    });
    const created = (await add.mock.results[0]?.value) as PersonaDto | undefined;
    expect(created).toBeDefined();
    expect(opener).toHaveTextContent('Traveler');
    expect(create).not.toHaveBeenCalled();
    await fireEvent.click(within(panel).getByRole('button', { name: t('uiPreview.startChat') }));
    await waitFor(() =>
        expect(select).toHaveBeenCalledWith(
            expect.objectContaining({ persona_id: created?.value.id }),
        ),
    );
});
