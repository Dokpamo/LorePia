import type { CharacterGreetingDetailInput } from '../../lib/ipc/contracts/character';
import { cleanup, fireEvent, render, screen, waitFor, within } from '@testing-library/svelte';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import { t } from '../../lib/i18n';
import { createPreviewClient } from '../../preview/mock-client';
import CharacterOverview from './CharacterOverview.svelte';

beforeEach(() => {
    vi.stubGlobal(
        'ResizeObserver',
        class {
            observe = vi.fn();
            disconnect = vi.fn();
            unobserve = vi.fn();
        },
    );
});
afterEach(() => {
    cleanup();
    vi.unstubAllGlobals();
});

async function setup() {
    const client = createPreviewClient();
    const [record] = await client.listCharacters();
    if (!record || !client.getCharacterRenderProfile) throw new Error('Missing profile fixture');
    const profile = await client.getCharacterRenderProfile(record.id);
    const catalog = await client.getCharacterGreetingCatalog(record.id);
    const greetings = [
        ...catalog.greetings,
        { id: 'alternate-extra', kind: 'alternate' as const, enabled: true },
    ];
    const excerpt = 'A quiet station. <img src=x onerror=alert(1)> {{user}}';
    vi.spyOn(client, 'getCharacterRenderProfile').mockResolvedValue({
        ...profile,
        creator: 'Imported author',
        creator_notes: 'A note from the author.',
        greeting_previews: greetings.map((greeting, index) => ({
            id: greeting.id,
            title: index === 1 ? 'The station' : null,
            excerpt: index === 1 ? excerpt : `An opening excerpt ${String(index + 1)}.`,
        })),
    });
    const description = 'An imported work description. '.repeat(40) + '<script>inert</script>';
    const onselectGreeting = vi.fn();
    const onaction = vi.fn();
    const view = render(CharacterOverview, {
        client,
        character: {
            id: record.id,
            name: record.name,
            description,
            thumbnail: 'A',
            subpage: false,
            histories: [],
        },
        greetings,
        greetingRevisionId: catalog.character_content_revision_id,
        selectedGreetingId: greetings[0]?.id ?? null,
        onselectGreeting,
        onclose: vi.fn(),
        onaction,
        onchat: vi.fn(),
    });
    const dialog = await screen.findByRole('dialog', { name: t('navigation.characterInfo') });
    await within(dialog).findByText('Imported author');
    return { ...view, client, dialog, greetings, onselectGreeting, onaction, description, excerpt };
}

it('keeps only chat history and related plugins in the play links and restores the profile after plugins', async () => {
    const { dialog } = await setup();
    const links = dialog.querySelector('.seed-profile-play-links');
    if (!(links instanceof HTMLElement)) throw new Error('Missing play links');
    expect(within(links).getAllByRole('button')).toHaveLength(2);
    expect(within(links).getByRole('button', { name: t('navigation.viewChats') })).toBeVisible();
    expect(
        within(dialog).queryByRole('heading', { name: t('navigation.manageCharacter') }),
    ).toBeNull();
    expect(within(dialog).queryByRole('button', { name: t('uiPreview.cardSettings') })).toBeNull();
    const trigger = within(links).getByRole('button', {
        name: new RegExp(t('settings.section.plugins.title')),
    });
    const body = dialog.querySelector<HTMLElement>('.ui-overlay-body');
    if (!body) throw new Error('Missing profile body');
    body.scrollTop = 760;
    await fireEvent.click(trigger);
    const plugins = await screen.findByRole('dialog', {
        name: t('settings.section.plugins.title'),
    });
    expect(dialog).toHaveProperty('inert', true);
    expect(await within(plugins).findByRole('article')).toHaveTextContent(
        t('navigation.pluginScope.character'),
    );
    await fireEvent.click(within(plugins).getByRole('button', { name: t('uiPreview.back') }));
    await waitFor(() => expect(screen.getAllByRole('dialog')).toEqual([dialog]));
    await waitFor(() => expect(trigger).toHaveFocus());
    expect(body.scrollTop).toBe(760);
});

it('opens the complete description as a page and restores the original scroll and trigger', async () => {
    const { dialog, description, onaction } = await setup();
    const body = dialog.querySelector('.ui-overlay-body');
    if (!(body instanceof HTMLElement)) throw new Error('Missing profile scroll body');
    body.scrollTop = 312;
    const more = within(dialog).getByRole('button', { name: t('navigation.readProfile') });
    expect(more).not.toHaveAttribute('aria-expanded');
    expect(more.querySelector('svg')).toBeInTheDocument();
    await fireEvent.click(more);
    const reading = await screen.findByRole('dialog', { name: t('navigation.workDescription') });
    expect(dialog).toHaveProperty('inert', true);
    expect(within(reading).getByText(description.replace(/\s+/g, ' '))).toBeVisible();
    expect(reading.querySelector('script')).toBeNull();
    expect(onaction).not.toHaveBeenCalled();
    await fireEvent.click(within(reading).getByRole('button', { name: t('uiPreview.back') }));
    await waitFor(() => expect(screen.getAllByRole('dialog')).toEqual([dialog]));
    await waitFor(() => expect(more).toHaveFocus());
    expect(body.scrollTop).toBe(312);
    expect(dialog.querySelector('.seed-profile-description-clamped')).toBeInTheDocument();
});

it('keeps the representative selection after browsing originals and focuses the newly selected hero image', async () => {
    const { dialog, client } = await setup();
    client.resolveAssetDelivery = vi.fn().mockRejectedValue(new Error('No test media'));
    expect(within(dialog).queryByRole('toolbar')).toBeNull();
    expect(dialog.querySelectorAll('.seed-profile-slide').length).toBeGreaterThan(1);
    const opener = dialog.querySelector<HTMLButtonElement>(
        '.seed-profile-slide[aria-hidden="false"]',
    );
    if (!opener) throw new Error('Missing representative image');
    await fireEvent.click(opener);
    const viewer = await screen.findByRole('dialog', { name: t('navigation.imageViewer') });
    await fireEvent.keyDown(viewer, { key: 'ArrowRight' });
    await fireEvent.keyDown(viewer, { key: 'Escape' });
    await waitFor(() => expect(screen.getAllByRole('dialog')).toEqual([dialog]));
    const selected = dialog.querySelector<HTMLButtonElement>(
        '.seed-profile-slide[aria-hidden="false"]',
    );
    if (!selected) throw new Error('Missing selected image');
    expect(selected).not.toBe(opener);
    await waitFor(() => expect(selected).toHaveFocus());
});

it('reads a hidden opening before selecting it, retaining the list and focus on back', async () => {
    const { dialog, greetings, onselectGreeting, onaction, client } = await setup();
    const last = greetings.at(-1);
    if (!last) throw new Error('No opening');
    client.getCharacterGreetingDetail = vi.fn((input: CharacterGreetingDetailInput) =>
        Promise.resolve({ ...input, text: 'The entire final scene.' }),
    );
    expect(within(dialog).queryAllByRole('radio')).toHaveLength(0);
    const more = within(dialog).getByRole('button', {
        name: t('navigation.moreStarts', { number: greetings.length }),
    });
    await fireEvent.click(more);
    const page = await screen.findByRole('dialog', { name: t('navigation.profileIntroduction') });
    const list = page.querySelector<HTMLElement>('.ui-overlay-body');
    if (!list) throw new Error('No list body');
    list.scrollTop = 160;
    const row = within(page).getByRole('button', { name: /An opening excerpt 4/ });
    await fireEvent.click(row);
    const reader = await screen.findByRole('dialog', {
        name: t('navigation.numberedStart', { number: 3 }),
    });
    expect(await within(reader).findByText('The entire final scene.')).toBeVisible();
    expect(page).toHaveProperty('inert', true);
    expect(onselectGreeting).not.toHaveBeenCalled();
    await fireEvent.click(within(reader).getByRole('button', { name: t('navigation.startChat') }));
    expect(onselectGreeting).toHaveBeenCalledExactlyOnceWith(last.id);
    expect(onaction).toHaveBeenCalledWith('new-chat', expect.any(HTMLButtonElement));
    await fireEvent.click(within(reader).getByRole('button', { name: t('uiPreview.back') }));
    await waitFor(() => expect(screen.getAllByRole('dialog')).toEqual([page]));
    await waitFor(() => expect(row).toHaveFocus());
    expect(list.scrollTop).toBe(160);
    await fireEvent.click(within(page).getByRole('button', { name: t('uiPreview.back') }));
    await waitFor(() => expect(screen.getAllByRole('dialog')).toEqual([dialog]));
    await waitFor(() => expect(more).toHaveFocus());
});

it('uses only matching revision previews, preserving literal text and honest title fallbacks', async () => {
    const { dialog, excerpt, rerender } = await setup();
    expect(within(dialog).getByText('The station')).toBeVisible();
    expect(within(dialog).getByText(excerpt)).toBeVisible();
    expect(dialog.querySelector('.seed-profile-start img')).toBeNull();
    expect(within(dialog).getByText(t('navigation.defaultStart'))).toBeVisible();
    expect(within(dialog).getByText(t('navigation.numberedStart', { number: 2 }))).toBeVisible();
    await rerender({ greetingRevisionId: 'another-revision' });
    expect(within(dialog).queryByText('The station')).toBeNull();
    expect(within(dialog).queryByText(excerpt)).toBeNull();
    expect(within(dialog).getByText(t('navigation.numberedStart', { number: 1 }))).toBeVisible();
    expect(within(dialog).getByText('Imported author')).toBeVisible();
});

it.each(['lorebook', 'scripts'] as const)(
    'opens %s as a page and preserves each parent when reading an entry',
    async (kind) => {
        const { dialog, onaction } = await setup();
        const key =
            kind === 'lorebook' ? 'navigation.profileLorebook' : 'navigation.profileScripts';
        const title = t(key);
        const opener = within(dialog).getByRole('button', { name: new RegExp(`^${title}`) });
        const profileBody = dialog.querySelector<HTMLElement>('.ui-overlay-body');
        if (!profileBody) throw new Error('Missing profile body');
        profileBody.scrollTop = 630;
        expect(
            within(dialog).queryByRole('button', {
                name: new RegExp(`^${t('navigation.profileRules')}`),
            }),
        ).toBeNull();
        await fireEvent.click(opener);
        const list = await screen.findByRole('dialog', { name: title });
        expect(dialog).toHaveProperty('inert', true);
        const listBody = list.querySelector<HTMLElement>('.ui-overlay-body');
        if (!listBody) throw new Error('Missing list body');
        listBody.scrollTop = 220;
        const row =
            kind === 'scripts'
                ? within(list).getByRole('button', {
                      name: new RegExp(`^${t('navigation.numberedRule', { number: 1 })}`),
                  })
                : within(list).getAllByRole('button')[1];
        if (!row) throw new Error('Missing entry row');
        if (kind === 'scripts') {
            expect(
                within(list).getByRole('heading', { name: t('navigation.profileRules') }),
            ).toBeVisible();
            expect(within(list).getByText(t('navigation.rulesExplanation'))).toBeVisible();
            expect(row).toHaveTextContent(t('navigation.displayRules'));
        }
        await fireEvent.click(row);
        const detail = screen.getByRole('dialog');
        expect(detail).not.toBe(list);
        expect(list).toHaveProperty('inert', true);
        expect(detail.querySelector('script')).toBeNull();
        if (kind === 'scripts') expect(detail.querySelector('pre')).toHaveTextContent('→ *$1*');
        expect(onaction).not.toHaveBeenCalled();
        await fireEvent.click(within(detail).getByRole('button', { name: t('uiPreview.back') }));
        await waitFor(() => expect(screen.getAllByRole('dialog')).toEqual([list]));
        await waitFor(() => expect(row).toHaveFocus());
        expect(listBody.scrollTop).toBe(220);
        await fireEvent.click(within(list).getByRole('button', { name: t('uiPreview.back') }));
        await waitFor(() => expect(screen.getAllByRole('dialog')).toEqual([dialog]));
        await waitFor(() => expect(opener).toHaveFocus());
        expect(profileBody.scrollTop).toBe(630);
    },
);
