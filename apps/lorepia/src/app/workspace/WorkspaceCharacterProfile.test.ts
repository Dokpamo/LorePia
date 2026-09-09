import { cleanup, fireEvent, render, screen, waitFor, within } from '@testing-library/svelte';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import WorkspaceApp from './WorkspaceApp.svelte';
import { createPreviewClient } from '../../preview/mock-client';
import { t } from '../../lib/i18n';

beforeEach(() => {
    vi.stubGlobal(
        'ResizeObserver',
        class {
            observe = vi.fn();
            unobserve = vi.fn();
            disconnect = vi.fn();
        },
    );
});
afterEach(() => {
    cleanup();
    vi.unstubAllGlobals();
});

it('opens chat setup from the profile and restores that profile before returning home', async () => {
    const client = createPreviewClient();
    const [character] = await client.listCharacters();
    if (!character) throw new Error('Character fixture missing');
    const createConversation = vi.spyOn(client, 'createConversation');
    if (!client.getCharacterRenderProfile) throw new Error('Missing profile reader');
    const readProfile = client.getCharacterRenderProfile.bind(client);
    client.getCharacterRenderProfile = (id, scope) =>
        readProfile(id, scope).then((profile) => ({
            ...profile,
            creator: 'Imported author',
            creator_notes: '<script>plain author note</script>',
            tags: ['world'],
        }));
    render(WorkspaceApp, { client });
    await fireEvent.click(
        await screen.findByRole('button', {
            name: t('uiPreview.cardSelect', { name: character.name }),
        }),
    );
    const profile = await screen.findByRole('dialog', { name: t('navigation.characterInfo') });
    expect(within(profile).getByRole('heading', { level: 1 })).toHaveTextContent(character.name);
    const author = await within(profile).findByText('Imported author');
    const heading = within(profile).getByRole('heading', { level: 1 });
    expect(heading.compareDocumentPosition(author) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy();
    const noteHeading = within(profile).getByRole('heading', { name: t('navigation.profileNote') });
    expect(
        author.compareDocumentPosition(noteHeading) & Node.DOCUMENT_POSITION_FOLLOWING,
    ).toBeTruthy();
    expect(within(profile).getByText('<script>plain author note</script>')).toBeVisible();
    expect(within(profile).getByText('#world')).toBeVisible();
    expect(profile.querySelector('script')).toBeNull();
    expect(within(profile).getByText(character.description.replace(/\s+/g, ' '))).toBeVisible();
    const greetingCatalog = await client.getCharacterGreetingCatalog(character.id);
    const alternate = greetingCatalog.greetings.find((item) => item.kind === 'alternate');
    if (!alternate) throw new Error('Alternate greeting fixture missing');
    await fireEvent.click(
        within(profile).getByRole('button', { name: t('navigation.numberedStart', { number: 1 }) }),
    );
    const reader = await screen.findByRole('dialog', {
        name: t('navigation.numberedStart', { number: 1 }),
    });
    const start = within(reader).getByRole('button', { name: t('navigation.startChat') });
    await waitFor(() => expect(start).toBeEnabled());
    await fireEvent.click(start);
    const setup = await screen.findByRole('dialog', { name: t('uiPreview.newChat') });
    await waitFor(() => expect(screen.getAllByRole('dialog')).toEqual([setup]));
    expect(profile).toHaveProperty('inert', true);
    expect(createConversation).not.toHaveBeenCalled();
    expect(within(setup).getByText(t('workspace.alternateGreeting', { number: 1 }))).toBeVisible();
    expect(
        within(setup).getByRole('radio', { name: new RegExp(t('uiPreview.chatMode')) }),
    ).toBeVisible();
    await fireEvent.click(within(setup).getByRole('button', { name: t('uiPreview.back') }));
    await waitFor(() => expect(screen.getAllByRole('dialog')).toEqual([reader]));
    expect(reader).toHaveProperty('inert', false);
    await fireEvent.click(within(reader).getByRole('button', { name: t('uiPreview.back') }));
    await waitFor(() => expect(screen.getAllByRole('dialog')).toEqual([profile]));
    await fireEvent.click(within(profile).getByRole('button', { name: t('uiPreview.back') }));
    await waitFor(() => expect(screen.queryByRole('dialog')).toBeNull());
    expect(
        screen.getByRole('button', {
            name: t('uiPreview.cardSelect', { name: character.name }),
        }),
    ).toBeVisible();
});
