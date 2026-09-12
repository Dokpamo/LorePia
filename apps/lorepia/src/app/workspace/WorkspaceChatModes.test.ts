import { cleanup, fireEvent, screen, waitFor, within } from '@testing-library/svelte';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import { t } from '../../lib/i18n';
import {
    CHAT_DISPLAY_MODES,
    chatDisplayPreferences,
    type ChatDisplayMode,
} from '../../lib/chat-display';
import { createPreviewClient } from '../../preview/mock-client';
import { openWorkspaceChat } from '../../tests/workspace-chat';

beforeEach(() => {
    chatDisplayPreferences.set({});
    localStorage.clear();
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
    chatDisplayPreferences.set({});
    localStorage.clear();
    vi.restoreAllMocks();
    vi.unstubAllGlobals();
});

async function choose(mode: ChatDisplayMode) {
    await fireEvent.click(screen.getByRole('button', { name: t('uiPreview.roomSettings') }));
    const option = CHAT_DISPLAY_MODES.find((item) => item.value === mode);
    if (!option) throw new Error('Missing display mode');
    expect(screen.getAllByRole('radio')).toHaveLength(3);
    await fireEvent.click(
        screen.getByRole('radio', { name: `${t(option.label)} ${t(option.hint)}` }),
    );
    await fireEvent.click(screen.getByRole('button', { name: t('uiPreview.save') }));
}

it('switches all three layouts, preserves native mode mapping, and remembers the room on reopen', async () => {
    const { client, container, conversation, unmount } = await openWorkspaceChat();
    const save = vi.spyOn(client, 'setConversationMode');
    const page = container.querySelector('.ui-chat');
    expect(page).toHaveAttribute('data-conversation-mode', 'chat');
    expect(container.querySelector('.ui-date')).toBeNull();
    await choose('default');
    await waitFor(() => expect(page).toHaveAttribute('data-conversation-mode', 'default'));
    expect(save).not.toHaveBeenCalled();
    expect(container.querySelectorAll('.ui-message-speaker').length).toBeGreaterThan(0);
    expect(
        within(screen.getByRole('log')).getAllByRole('group', { name: t('uiPreview.messageTools') })
            .length,
    ).toBeGreaterThan(0);
    await choose('story');
    await waitFor(() => expect(page).toHaveAttribute('data-conversation-mode', 'story'));
    expect(save).toHaveBeenLastCalledWith(conversation.id, 'story');
    expect(container.querySelector('.ui-message-speaker')).toBeNull();
    expect(container.querySelector('.ui-message-tools')).toBeNull();
    await choose('chat');
    await waitFor(() => expect(page).toHaveAttribute('data-conversation-mode', 'chat'));
    expect(save).toHaveBeenLastCalledWith(conversation.id, 'chat');
    await choose('default');
    await waitFor(() => expect(page).toHaveAttribute('data-conversation-mode', 'default'));
    unmount();
    chatDisplayPreferences.set({});
    const reopened = await openWorkspaceChat(client);
    expect(reopened.container.querySelector('.ui-chat')).toHaveAttribute(
        'data-conversation-mode',
        'default',
    );
}, 15000);

it('leaves the chosen layout unchanged when the native mode save fails', async () => {
    const { client, container, conversation } = await openWorkspaceChat();
    await choose('default');
    await waitFor(() =>
        expect(container.querySelector('.ui-chat')).toHaveAttribute(
            'data-conversation-mode',
            'default',
        ),
    );
    vi.spyOn(client, 'setConversationMode').mockRejectedValue(new Error('unavailable'));
    await choose('story');
    await screen.findByRole('alert');
    expect(localStorage.getItem('lorepia.chatDisplay.' + conversation.id)).toBe('default');
    expect(container.querySelector('.ui-chat')).toHaveAttribute(
        'data-conversation-mode',
        'default',
    );
});

it('shows the selected persona on user sections and opens branch choices from their inline tools', async () => {
    const client = createPreviewClient();
    const [character] = await client.listCharacters();
    const [conversation] = await client.listConversations(character?.id ?? '');
    const [persona] = await client.listPersonas({ limit: 100 });
    if (!conversation || !persona) throw new Error('Missing demo conversation/persona');
    const selection = await client.getConversationPersonaSelection({
        conversation_id: conversation.id,
    });
    await client.selectConversationPersona({
        conversation_id: conversation.id,
        persona_id: persona.value.id,
        expected_state_revision: selection.state_revision,
    });
    const { container } = await openWorkspaceChat(client);
    await choose('default');
    await waitFor(() =>
        expect(container.querySelector('.ui-user-turn .ui-message-speaker')).toHaveTextContent(
            persona.value.name,
        ),
    );
    const user = container.querySelector<HTMLElement>('.ui-user-turn');
    if (!user) throw new Error('Missing user message');
    await fireEvent.click(within(user).getByRole('button', { name: t('uiPreview.branch') }));
    expect(await screen.findByRole('menuitem', { name: t('uiPreview.branchFrom') })).toBeEnabled();
    await fireEvent.keyDown(screen.getByRole('menu'), { key: 'Escape' });
    await waitFor(() => expect(screen.queryByRole('menu')).toBeNull());
    expect(within(user).getByRole('button', { name: t('uiPreview.branch') })).toHaveFocus();
});
