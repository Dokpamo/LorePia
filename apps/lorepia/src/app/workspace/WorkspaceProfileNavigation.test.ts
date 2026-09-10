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

it('opens character-specific history and plugins as separate pages and restores the profile scroll', async () => {
    const client = createPreviewClient();
    const [character] = await client.listCharacters();
    if (!character) throw new Error('Missing character');
    const records = (await client.listConversations(null)).map((item, i) => ({
        ...item,
        title: `Conversation ${String(i)}`,
    }));
    client.listConversations = (id) =>
        Promise.resolve(records.filter((item) => !id || item.character_id === id));
    const own = records.filter((item) => item.character_id === character.id);
    const other = records.filter((item) => item.character_id !== character.id);
    expect(own.length).toBeGreaterThan(0);
    expect(other.length).toBeGreaterThan(0);
    render(WorkspaceApp, { client });
    await fireEvent.click(
        await screen.findByRole('button', {
            name: t('uiPreview.cardSelect', { name: character.name }),
        }),
    );
    const profile = await screen.findByRole('dialog', { name: t('navigation.characterInfo') });
    const body = profile.querySelector<HTMLElement>('.ui-overlay-body');
    if (!body) throw new Error('Missing profile body');
    body.scrollTop = 500;
    const opener = within(profile).getByRole('button', { name: t('navigation.viewChats') });
    await fireEvent.click(opener);
    const history = await screen.findByRole('dialog', { name: t('uiPreview.history') });
    expect(profile).toHaveProperty('inert', true);
    for (const item of own) expect(await within(history).findByText(item.title)).toBeVisible();
    for (const item of other) expect(within(history).queryByText(item.title)).toBeNull();
    await fireEvent.click(within(history).getByRole('button', { name: t('uiPreview.back') }));
    await waitFor(() => expect(screen.getAllByRole('dialog')).toEqual([profile]));
    await waitFor(() => expect(opener).toHaveFocus());
    expect(body.scrollTop).toBe(500);
    await fireEvent.click(
        within(profile).getByRole('button', {
            name: new RegExp(t('settings.section.plugins.title')),
        }),
    );
    const plugins = await screen.findByRole('dialog', {
        name: t('settings.section.plugins.title'),
    });
    expect(within(plugins).getByText(character.name)).toBeVisible();
    expect(profile).toHaveProperty('inert', true);
    await fireEvent.click(within(plugins).getByRole('button', { name: t('uiPreview.back') }));
    await waitFor(() => expect(screen.getAllByRole('dialog')).toEqual([profile]));
    expect(body.scrollTop).toBe(500);
});
