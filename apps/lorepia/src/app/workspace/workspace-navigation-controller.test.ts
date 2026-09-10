import { describe, expect, it, vi } from 'vitest';
import { get } from 'svelte/store';
import { LorepiaAppController } from '../app-controller';
import { createPreviewClient } from '../../preview/mock-client';
import { WorkspaceNavigationController } from './workspace-navigation-controller';

describe('global conversation navigation', () => {
    it('cancels a delayed cross-character selection when another conversation is chosen', async () => {
        const client = createPreviewClient();
        const conversations = await client.listConversations(null);
        const first = conversations[0];
        const second = conversations.find((item) => item.character_id !== first?.character_id);
        if (!first || !second) throw new Error('Conversation fixtures missing');
        const list = client.listConversations.bind(client);
        let release!: () => void;
        const barrier = new Promise<void>((resolve) => (release = resolve));
        vi.spyOn(client, 'listConversations').mockImplementation(async (id) => {
            if (id === second.character_id) await barrier;
            return list(id);
        });
        const open = vi.spyOn(client, 'openExistingConversation');
        const app = new LorepiaAppController(client);
        await app.start();
        const navigation = new WorkspaceNavigationController(client, app, () => get(app.state));
        await navigation.load();
        const stale = navigation.selectConversation(second.id);
        expect(await navigation.selectConversation(first.id)).toBe(true);
        release();
        expect(await stale).toBe(false);
        expect(get(app.state).selected_conversation?.id).toBe(first.id);
        expect(open).not.toHaveBeenCalledWith(second.id);
        navigation.destroy();
        app.destroy();
    });
    it('keeps the tab destination when selection finishes after leaving the list', async () => {
        const client = createPreviewClient();
        const [character] = await client.listCharacters();
        if (!character) throw new Error('Character fixture missing');
        const app = new LorepiaAppController(client);
        await app.start();
        const navigation = new WorkspaceNavigationController(client, app, () => get(app.state));
        const selecting = navigation.selectCharacter(character.id);
        navigation.cancelNavigation();
        expect(await selecting).toBe(false);
        navigation.destroy();
        app.destroy();
    });
});
