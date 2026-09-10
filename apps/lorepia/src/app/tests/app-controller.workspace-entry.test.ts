import { get } from 'svelte/store';
import { describe, expect, it, vi } from 'vitest';
import { LorepiaAppController } from '../app-controller';
import { createAppControllerFixture, deferred } from './app-controller-test-support';
import type { CharacterGreetingCatalogDto } from '../../lib/ipc/contracts';

describe('workspace conversation entry', () => {
    it('keeps a deliberate conversation choice while an initial greeting catalog is pending', async () => {
        const { character, conversation, greetingCatalog, mockClient } =
            createAppControllerFixture();
        const catalog = deferred<CharacterGreetingCatalogDto>();
        const opened = { ...conversation, id: 'chosen-room' };
        const open = vi.fn(() => Promise.resolve(opened));
        const controller = new LorepiaAppController(
            mockClient({
                getCharacterGreetingCatalog: () => catalog.promise,
                openExistingConversation: open,
            }),
        );
        const initial = controller.selectCharacter(character, true);
        await controller.selectConversation(opened);
        catalog.resolve(greetingCatalog);
        await initial;
        expect(open).toHaveBeenCalledTimes(1);
        expect(open).toHaveBeenCalledWith(opened.id);
        expect(get(controller.state).selected_conversation?.id).toBe(opened.id);
        controller.destroy();
    });

    it('passes the mockup title and mode through the same greeting revision boundary', async () => {
        const { character, conversation, greetingCatalog, mockClient } =
            createAppControllerFixture();
        const create = vi.fn(() => Promise.resolve(conversation));
        const controller = new LorepiaAppController(mockClient({ createConversation: create }));
        await controller.selectCharacter(character);
        await controller.openNewConversation('  A named story  ', 'story');
        expect(create).toHaveBeenCalledWith(character.id, 'A named story', 'story', {
            character_content_revision_id: greetingCatalog.character_content_revision_id,
            greeting_id: 'default-enabled',
        });
        controller.destroy();
    });
});
