import { cleanup, fireEvent, render, screen, within } from '@testing-library/svelte';
import { afterEach, expect, it, vi } from 'vitest';
import { t } from '../../lib/i18n';
import type { SampleCharacter } from '../workspace/view-types';
import ProfileConversationsPage from './ProfileConversationsPage.svelte';

afterEach(cleanup);
const character: SampleCharacter = {
    id: 'selected',
    name: 'Selected character',
    description: '',
    thumbnail: 'S',
    subpage: false,
    histories: [],
};
const conversations = [
    {
        id: 'old',
        characterId: 'selected',
        characterName: character.name,
        title: 'Older conversation',
        date: '',
        updatedAt: '2025-09-09T00:00:00Z',
    },
    {
        id: 'other',
        characterId: 'other',
        characterName: 'Other character',
        title: 'Unrelated conversation',
        date: '',
        updatedAt: '2026-09-10T00:00:00Z',
    },
    {
        id: 'new',
        characterId: 'selected',
        characterName: character.name,
        title: 'Newest conversation',
        date: '',
        updatedAt: '2026-09-09T00:00:00Z',
    },
];
function setup() {
    const onopen = vi.fn(),
        onnew = vi.fn(),
        onretry = vi.fn();
    const view = render(ProfileConversationsPage, {
        character,
        conversations,
        loading: false,
        error: null,
        onopen,
        onnew,
        onretry,
        onclose: vi.fn(),
    });
    return { ...view, onopen, onnew, onretry };
}
it('shows only the current character conversations, newest first, and opens the selected conversation', async () => {
    const { onopen, rerender } = setup();
    const list = screen.getByRole('region', { name: t('uiPreview.history') });
    expect(
        within(list)
            .getAllByRole('button')
            .map((button) => button.textContent),
    ).toEqual([
        expect.stringContaining('Newest conversation'),
        expect.stringContaining('Older conversation'),
    ]);
    expect(screen.queryByText('Unrelated conversation')).toBeNull();
    await fireEvent.click(within(list).getByRole('button', { name: /^Newest conversation/ }));
    expect(onopen).toHaveBeenCalledExactlyOnceWith('new');
    await rerender({ character: { ...character, id: 'other', name: 'Other character' } });
    expect(screen.queryByText('Newest conversation')).toBeNull();
    expect(screen.getByText('Unrelated conversation')).toBeVisible();
});
it('distinguishes loading and failure from an empty character history and offers a new conversation', async () => {
    const { rerender, onnew, onretry } = setup();
    await rerender({
        conversations: conversations.filter((item) => item.characterId === 'other'),
        loading: true,
    });
    expect(screen.getByRole('status')).toBeVisible();
    expect(screen.queryByText(t('navigation.noChats'))).toBeNull();
    await rerender({ loading: false, error: 'Could not load' });
    expect(screen.getByRole('alert')).toHaveTextContent('Could not load');
    await fireEvent.click(screen.getByRole('button', { name: t('workspace.retry') }));
    expect(onretry).toHaveBeenCalledOnce();
    await rerender({ error: null });
    expect(screen.getByText(t('navigation.noCharacterChatsHint'))).toBeVisible();
    await fireEvent.click(screen.getByRole('button', { name: t('uiPreview.newChat') }));
    expect(onnew).toHaveBeenCalledOnce();
});
