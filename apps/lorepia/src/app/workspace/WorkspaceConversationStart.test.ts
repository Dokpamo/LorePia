import { cleanup, fireEvent, render, screen, waitFor, within } from '@testing-library/svelte';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import WorkspaceApp from './WorkspaceApp.svelte';
import { createPreviewClient } from '../../preview/mock-client';
import { t } from '../../lib/i18n';

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

async function openSetup() {
    const client = createPreviewClient();
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
    await fireEvent.click(await screen.findByRole('radio', { name: persona.value.name }));
    await waitFor(() => expect(screen.getAllByRole('dialog')).toEqual([panel]));
    await fireEvent.click(
        within(panel).getByRole('button', { name: t('workspace.startingScene') }),
    );
    await fireEvent.click(await screen.findByRole('radio', { name: title }));
    await waitFor(() => expect(screen.getAllByRole('dialog')).toEqual([panel]));
    await fireEvent.click(
        within(panel).getByRole('radio', { name: new RegExp(t('uiPreview.storyMode')) }),
    );
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
    await fireEvent.click(
        within(panel).getByRole('radio', { name: new RegExp(t('uiPreview.storyMode')) }),
    );
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
        within(panel).getByRole('radio', { name: new RegExp(t('uiPreview.storyMode')) }),
    ).toBeChecked();
    await fireEvent.click(within(panel).getByRole('button', { name: t('uiPreview.back') }));
    await fireEvent.click(
        await screen.findByRole('button', { name: t('uiPreview.discardChanges') }),
    );
    await waitFor(() => expect(screen.getAllByRole('dialog')).toEqual([profile]));
    expect(create).not.toHaveBeenCalled();
});
