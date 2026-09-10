import { get } from 'svelte/store';

import type { LorepiaAppController } from '../../app/app-controller';
import type { CharacterDto } from '../../lib/ipc/contracts';

/** Open the latest room for a character, or create its first room. */
export async function enterCharacter(
    controller: LorepiaAppController,
    character: CharacterDto,
): Promise<boolean> {
    await controller.selectCharacter(character);
    const state = get(controller.state);
    if (state.selected_character?.id !== character.id || state.conversations.phase !== 'ready') {
        return false;
    }
    const conversation = state.conversations.items[0];
    return conversation === undefined
        ? controller.openNewConversation()
        : controller.selectConversation(conversation);
}
