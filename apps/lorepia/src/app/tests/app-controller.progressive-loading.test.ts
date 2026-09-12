import { get } from 'svelte/store';
import { waitFor } from '@testing-library/svelte';
import { describe, expect, it, vi } from 'vitest';
import { LorepiaAppController } from '../app-controller';
import { LiveChatSession } from '../workspace/live-chat-session.svelte';
import { WorkspaceNavigationController } from '../workspace/workspace-navigation-controller';
import { createAppControllerFixture, deferred } from './app-controller-test-support';
import type { ConversationBranchDto, MessageDto } from '../../lib/ipc/contracts';

const fixture = createAppControllerFixture();
const message: MessageDto = {
    id: 'saved-message',
    conversation_id: fixture.conversation.id,
    parent_id: null,
    role: 'assistant',
    content: 'Saved conversation',
    status: 'complete',
    generation_id: null,
    created_at: fixture.conversation.created_at,
};

describe('visible conversation data before optional work', () => {
    it('opens a room while character metadata is pending, then still accepts its greeting catalog', async () => {
        const greetings = deferred<typeof fixture.greetingCatalog>();
        const catalog = deferred<(typeof fixture.conversation)[]>();
        const client = fixture.mockClient({
            getCharacterGreetingCatalog: () => greetings.promise,
            listConversations: (id) =>
                id ? catalog.promise : Promise.resolve([fixture.conversation]),
        });
        const app = new LorepiaAppController(client);
        await app.start();
        const navigation = new WorkspaceNavigationController(client, app, () => get(app.state));
        await navigation.load();
        expect(await navigation.selectConversation(fixture.conversation.id)).toBe(true);
        expect(get(app.state).selected_conversation?.id).toBe(fixture.conversation.id);
        expect(get(app.state).greeting_catalog.phase).toBe('loading');
        await waitFor(() => expect(get(app.state).messages.phase).toBe('ready'));
        greetings.resolve(fixture.greetingCatalog);
        catalog.resolve([fixture.conversation]);
        await waitFor(() => expect(get(app.state).greeting_catalog.phase).toBe('ready'));
        navigation.destroy();
        app.destroy();
    });

    it.each(['ready', 'failed'] as const)(
        'preserves the opened room receipt when an earlier character list is %s',
        async (outcome) => {
            const catalog = deferred<(typeof fixture.conversation)[]>();
            const opened = { ...fixture.conversation, title: 'Latest saved room title' };
            const app = new LorepiaAppController(
                fixture.mockClient({
                    listConversations: () => catalog.promise,
                    openExistingConversation: () => Promise.resolve(opened),
                }),
            );
            await app.start();
            const selecting = app.selectCharacter(fixture.character);
            await app.selectConversation(fixture.conversation);
            expect(get(app.state).conversations.items).toEqual([opened]);
            if (outcome === 'ready') catalog.resolve([fixture.conversation]);
            else catalog.reject(new Error('Character list read failed'));
            await selecting;
            expect(get(app.state).conversations.items).toEqual([opened]);
            expect(get(app.state).conversations.phase).toBe(
                outcome === 'ready' ? 'ready' : 'error',
            );
            app.destroy();
        },
    );

    it.each(['ready', 'failed'] as const)(
        'renders saved messages while branch metadata is %s',
        async (outcome) => {
            const branches = deferred<ConversationBranchDto[]>();
            const read = vi.fn().mockResolvedValue([message]);
            const app = new LorepiaAppController(
                fixture.mockClient({
                    listBranches: () => branches.promise,
                    listBranchMessages: read,
                }),
            );
            await app.start();
            await app.selectCharacter(fixture.character);
            const session = new LiveChatSession(app, () => get(app.state));
            const opening = app.selectConversation(fixture.conversation);
            await waitFor(() => expect(get(app.state).messages.items).toEqual([message]));
            expect(read).toHaveBeenCalledWith(fixture.branch.id);
            expect(session.busy).toBe(true);
            expect(session.canStop).toBe(false);
            if (outcome === 'ready') branches.resolve([fixture.branch]);
            else branches.reject(new Error('Branch read failed'));
            expect(await opening).toBe(outcome === 'ready');
            expect(get(app.state).messages.items).toEqual([message]);
            expect(session.busy).toBe(outcome !== 'ready');
            app.destroy();
        },
    );

    it('stops obsolete downstream reads after another character is selected', async () => {
        const state = deferred<typeof fixture.conversationState>();
        const read = vi.fn().mockResolvedValue([]);
        const app = new LorepiaAppController(
            fixture.mockClient({
                getConversationState: () => state.promise,
                listBranchMessages: read,
            }),
        );
        await app.start();
        await app.selectCharacter(fixture.character);
        const opening = app.selectConversation(fixture.conversation);
        await Promise.resolve();
        await app.selectCharacter({ ...fixture.character, id: 'next-character' });
        state.resolve(fixture.conversationState);
        expect(await opening).toBe(false);
        expect(read).not.toHaveBeenCalled();
        expect(get(app.state).messages.items).toEqual([]);
        app.destroy();
    });

    it('coalesces catalog invalidations and refreshes again after an in-flight snapshot', async () => {
        const first = deferred<(typeof fixture.conversation)[]>();
        const list = vi
            .fn()
            .mockReturnValueOnce(first.promise)
            .mockResolvedValue([fixture.conversation]);
        const client = fixture.mockClient({ listConversations: list });
        const app = new LorepiaAppController(client);
        const navigation = new WorkspaceNavigationController(client, app, () => get(app.state));
        const loading = navigation.load();
        for (let i = 0; i < 30; i++) void navigation.load();
        expect(list).toHaveBeenCalledTimes(1);
        first.resolve([]);
        await loading;
        expect(list).toHaveBeenCalledTimes(2);
        expect(get(navigation.state).items).toEqual([fixture.conversation]);
        navigation.destroy();
        await navigation.load();
        expect(list).toHaveBeenCalledTimes(2);
        app.destroy();
    });
});
