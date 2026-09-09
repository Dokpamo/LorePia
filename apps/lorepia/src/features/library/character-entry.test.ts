import { t } from '../../lib/i18n';
import { get } from 'svelte/store';
import { describe, expect, it, vi } from 'vitest';

import { LorepiaAppController } from '../../app/app-controller';
import { createAppControllerFixture } from '../../app/tests/app-controller-test-support';
import type { ConversationBranchDto, ConversationDto } from '../../lib/ipc/contracts';
import { enterCharacter } from './character-entry';

const { character, conversation, conversationState, branch, greetingCatalog, mockClient } =
    createAppControllerFixture();

describe('character conversation entry', () => {
    it('opens the first listed existing room before entering chat', async () => {
        const openExistingConversation = vi.fn(() => Promise.resolve(conversation));
        const controller = new LorepiaAppController(mockClient({ openExistingConversation }));

        await expect(enterCharacter(controller, character)).resolves.toBe(true);

        expect(openExistingConversation).toHaveBeenCalledWith(conversation.id);
        expect(get(controller.state).selected_conversation).toEqual(conversation);
        controller.destroy();
    });

    it('creates and opens a first room when the character has none', async () => {
        const created: ConversationDto = { ...conversation, id: 'conversation-created' };
        const createdBranch: ConversationBranchDto = {
            ...branch,
            id: 'branch-created',
            conversation_id: created.id,
        };
        const createConversation = vi.fn(() => Promise.resolve(created));
        const controller = new LorepiaAppController(
            mockClient({
                listConversations: () => Promise.resolve([]),
                createConversation,
                getConversationState: () =>
                    Promise.resolve({
                        ...conversationState,
                        conversation_id: created.id,
                        active_branch_id: createdBranch.id,
                    }),
                listBranches: () => Promise.resolve([createdBranch]),
            }),
        );

        await expect(enterCharacter(controller, character)).resolves.toBe(true);

        expect(createConversation).toHaveBeenCalledWith(
            character.id,
            t('uiPreview.newChat'),
            'chat',
            {
                character_content_revision_id: greetingCatalog.character_content_revision_id,
                greeting_id: 'default-enabled',
            },
        );
        expect(get(controller.state).selected_conversation).toEqual(created);
        controller.destroy();
    });

    it('does not create a duplicate when the room list failed to load', async () => {
        const createConversation = vi.fn();
        const controller = new LorepiaAppController(
            mockClient({
                listConversations: () => Promise.reject(new Error('list failed')),
                createConversation,
            }),
        );

        await expect(enterCharacter(controller, character)).resolves.toBe(false);

        expect(createConversation).not.toHaveBeenCalled();
        controller.destroy();
    });
});
