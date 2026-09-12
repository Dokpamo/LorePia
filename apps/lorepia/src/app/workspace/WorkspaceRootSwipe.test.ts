import { cleanup, fireEvent, render, screen, waitFor, within } from '@testing-library/svelte';
import { afterEach, describe, expect, it } from 'vitest';
import { t } from '../../lib/i18n';
import { createPreviewClient } from '../../preview/mock-client';
import WorkspaceApp from './WorkspaceApp.svelte';

afterEach(cleanup);

function rootGeometry(container: HTMLElement) {
    const roots = container.querySelector<HTMLElement>('.seed-roots');
    if (!roots) throw new Error('Missing root destinations');
    Object.defineProperty(roots, 'clientWidth', { value: 393 });
    const captures = new Set<number>();
    roots.setPointerCapture = (id) => captures.add(id);
    roots.hasPointerCapture = (id) => captures.has(id);
    roots.releasePointerCapture = (id) => captures.delete(id);
    return roots;
}
async function pointer(target: HTMLElement | Window, type: string, x: number, time: number) {
    const event = new MouseEvent(type, {
        bubbles: true,
        cancelable: true,
        clientX: x,
        clientY: 300,
        button: 0,
        detail: 1,
    });
    for (const [key, value] of Object.entries({ pointerId: 1, isPrimary: true, timeStamp: time }))
        Object.defineProperty(event, key, { value });
    await fireEvent(target, event);
}
async function swipe(target: HTMLElement, direction: -1 | 1) {
    await pointer(target, 'pointerdown', 200, 0);
    await pointer(window, 'pointermove', 200 + direction * 170, 300);
    await pointer(window, 'pointerup', 200 + direction * 170, 600);
    // The browser can dispatch the release click on the original row.
    await fireEvent.click(target, { detail: 1 });
}
function tab(name: string) {
    return within(screen.getByRole('navigation', { name: t('navigation.label') })).getByRole(
        'button',
        { name },
    );
}

describe('root swipe integration', () => {
    it('swipes every root both ways from real card and settings buttons without opening them', async () => {
        const client = createPreviewClient();
        const [character] = await client.listCharacters();
        if (!character) throw new Error('Missing character');
        const [conversation] = await client.listConversations(character.id);
        if (!conversation) throw new Error('Missing conversation');
        const { container } = render(WorkspaceApp, { client });
        const roots = rootGeometry(container);
        const card = await screen.findByRole('button', {
            name: t('uiPreview.cardSelect', { name: character.name }),
        });
        const homeScroll = roots.querySelector<HTMLElement>('[data-root-tab="home"] .seed-scroll');
        if (!homeScroll) throw new Error('Missing home scroll');
        homeScroll.scrollTop = 120;
        const chat = () =>
            screen.findByRole('button', { name: `${conversation.title} · ${character.name}` });
        const create = () =>
            screen.getByRole('button', { name: new RegExp('^' + t('navigation.createLorebook')) });

        await swipe(card, -1);
        expect(tab(t('navigation.chats'))).toHaveAttribute('aria-current', 'page');
        await swipe(await chat(), -1);
        expect(tab(t('navigation.create'))).toHaveAttribute('aria-current', 'page');
        await swipe(create(), -1);
        expect(tab(t('navigation.settings'))).toHaveAttribute('aria-current', 'page');
        await swipe(
            await screen.findByRole(
                'button',
                { name: t('uiPreview.aiConnection') },
                { timeout: 5000 },
            ),
            1,
        );
        expect(tab(t('navigation.create'))).toHaveAttribute('aria-current', 'page');
        await swipe(create(), 1);
        expect(tab(t('navigation.chats'))).toHaveAttribute('aria-current', 'page');
        await swipe(await chat(), 1);
        expect(tab(t('navigation.home'))).toHaveAttribute('aria-current', 'page');
        expect(screen.queryByRole('dialog')).toBeNull();
        expect(screen.getByRole('button', { name: card.getAttribute('aria-label') ?? '' })).toBe(
            card,
        );
        expect(homeScroll.scrollTop).toBe(120);
        expect(roots.querySelectorAll(':scope > [aria-hidden="false"]')).toHaveLength(1);
    });

    it('keeps root swiping disabled in settings detail and search, then restores it on return', async () => {
        const client = createPreviewClient();
        const { container } = render(WorkspaceApp, { client });
        const roots = rootGeometry(container);
        await fireEvent.click(tab(t('navigation.settings')));
        await fireEvent.click(
            await screen.findByRole(
                'button',
                { name: t('uiPreview.aiConnection') },
                { timeout: 5000 },
            ),
        );
        const ai = await screen.findByRole('dialog', { name: t('workspaceAi.title') });
        expect(roots).toHaveAttribute('data-root-swipe-enabled', 'false');
        await swipe(ai, -1);
        expect(ai).toBeVisible();
        expect(roots.style.getPropertyValue('--seed-active-tab')).toBe('3');
        await fireEvent.click(within(ai).getByRole('button', { name: t('uiPreview.back') }));
        await waitFor(() => expect(screen.queryByRole('dialog')).toBeNull());
        expect(roots).toHaveAttribute('data-root-swipe-enabled', 'true');

        await fireEvent.click(tab(t('navigation.home')));
        await fireEvent.click(
            screen.getByRole('button', { name: t('navigation.searchCharacters') }),
        );
        const search = await screen.findByRole('dialog', {
            name: t('navigation.searchCharacters'),
        });
        expect(roots).toHaveAttribute('data-root-swipe-enabled', 'false');
        await swipe(search, -1);
        expect(search).toBeVisible();
        expect(roots.style.getPropertyValue('--seed-active-tab')).toBe('0');
        await fireEvent.click(within(search).getByRole('button', { name: t('uiPreview.back') }));
        await waitFor(() => expect(screen.queryByRole('dialog')).toBeNull());
        expect(roots).toHaveAttribute('data-root-swipe-enabled', 'true');
    });
});
