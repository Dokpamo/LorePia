import { get } from 'svelte/store';
import { describe, expect, it, vi } from 'vitest';
import { createPreviewClient } from '../../preview/mock-client';
import { t } from '../../lib/i18n';
import { LorepiaAppController } from '../app-controller';
import { nextConversationTitle } from '../operations/conversation-title';
import { deferred } from './app-controller-test-support';
import type { ConversationDto } from '../../lib/ipc/contracts';

async function setup() {
    const client = createPreviewClient();
    const controller = new LorepiaAppController(client);
    const [character] = await client.listCharacters();
    const [persona] = await client.listPersonas({ limit: 100 });
    if (!character || !persona) throw new Error('Missing demo fixture');
    await controller.selectCharacter(character);
    return { client, controller, character, persona };
}

describe('new conversation setup', () => {
    it('numbers default titles after the highest existing suffix and preserves custom names', async () => {
        const base = t('uiPreview.newChat');
        expect(nextConversationTitle([], base)).toBe(base);
        expect(
            nextConversationTitle(
                [{ title: base }, { title: `${base} 3` }, { title: `${base} 02` }],
                base,
            ),
        ).toBe(`${base} 4`);
        const { client, controller } = await setup();
        const create = vi.spyOn(client, 'createConversation');
        for (const title of [base, `${base} 2`, `${base} 3`]) {
            expect(await controller.openNewConversation()).toBe(true);
            expect(get(controller.state).selected_conversation?.title).toBe(title);
        }
        expect(await controller.openNewConversation('  My own story  ', 'story')).toBe(true);
        expect(create.mock.lastCall?.slice(1, 3)).toEqual(['My own story', 'story']);
        controller.destroy();
    });

    it('pins the chosen persona before exposing the greeting-bound story conversation', async () => {
        const { client, controller, character, persona } = await setup();
        const catalog = await client.getCharacterGreetingCatalog(character.id);
        const greeting = catalog.greetings.find(
            (item) => item.kind === 'alternate' && item.enabled,
        );
        if (!greeting) throw new Error('Missing alternate');
        controller.selectGreeting(greeting.id);
        const create = vi.spyOn(client, 'createConversation');
        const select = vi.spyOn(client, 'selectConversationPersona');
        const load = vi.spyOn(client, 'getConversationState');
        expect(await controller.openNewConversation(undefined, 'story', persona.value.id)).toBe(
            true,
        );
        const selected = get(controller.state).selected_conversation;
        if (!selected) throw new Error('Room missing');
        expect(create).toHaveBeenCalledWith(character.id, t('uiPreview.newChat'), 'story', {
            character_content_revision_id: catalog.character_content_revision_id,
            greeting_id: greeting.id,
        });
        expect(select).toHaveBeenCalledWith({
            conversation_id: selected.id,
            persona_id: persona.value.id,
            expected_state_revision: null,
        });
        expect(select.mock.invocationCallOrder[0]).toBeLessThan(
            load.mock.invocationCallOrder[0] ?? 0,
        );
        expect(
            (await client.getConversationPersonaSelection({ conversation_id: selected.id }))
                .selected_persona?.value,
        ).toEqual(persona.value);
        expect(get(controller.state).conversation_state?.selected_mode).toBe('story');
        expect(get(controller.state).pending_conversation_start).toBeNull();
        controller.destroy();
    });

    it.each([false, true])(
        'retries the same durable room when persona acknowledgement fails (committed: %s)',
        async (committed) => {
            const { client, controller, persona } = await setup();
            const create = vi.spyOn(client, 'createConversation');
            const apply = client.selectConversationPersona.bind(client);
            const select = vi
                .spyOn(client, 'selectConversationPersona')
                .mockImplementationOnce(async (input) => {
                    if (committed) await apply(input);
                    throw new Error('response lost');
                });
            expect(await controller.openNewConversation(undefined, 'chat', persona.value.id)).toBe(
                false,
            );
            const pending = get(controller.state).pending_conversation_start;
            expect(pending).not.toBeNull();
            expect(get(controller.state).selected_conversation).toBeNull();
            expect(await controller.openNewConversation(undefined, 'chat', persona.value.id)).toBe(
                true,
            );
            expect(create).toHaveBeenCalledTimes(1);
            expect(select).toHaveBeenCalledTimes(committed ? 1 : 2);
            expect(get(controller.state).selected_conversation?.id).toBe(pending?.conversation.id);
            controller.destroy();
        },
    );

    it('allows only one in-flight start and ignores its result after changing character', async () => {
        const { client, controller, persona } = await setup();
        const receipt = deferred<ConversationDto>();
        const create = vi.spyOn(client, 'createConversation').mockReturnValue(receipt.promise);
        const select = vi.spyOn(client, 'selectConversationPersona');
        const start = controller.openNewConversation(undefined, 'chat', persona.value.id);
        expect(await controller.openNewConversation()).toBe(false);
        const other = (await client.listCharacters())[1];
        if (!other) throw new Error('Missing second character');
        await controller.selectCharacter(other);
        receipt.resolve({
            id: 'stale-room',
            character_id: 'stale-character',
            title: 'stale',
            created_at: '',
            updated_at: '',
        });
        expect(await start).toBe(false);
        expect(create).toHaveBeenCalledTimes(1);
        expect(select).not.toHaveBeenCalled();
        expect(get(controller.state).selected_character?.id).toBe(other.id);
        expect(get(controller.state).selected_conversation).toBeNull();
        expect(get(controller.state).pending_conversation_start).toBeNull();
        controller.destroy();
    });
});
