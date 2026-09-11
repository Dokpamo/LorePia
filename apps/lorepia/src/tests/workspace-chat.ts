import { fireEvent, render, screen } from '@testing-library/svelte';
import WorkspaceApp from '../app/workspace/WorkspaceApp.svelte';
import { createPreviewClient } from '../preview/mock-client';
import type { LorepiaClient } from '../lib/ipc/contracts';
import { t } from '../lib/i18n';

export async function openWorkspaceChat(client: LorepiaClient = createPreviewClient()) {
    const [character] = await client.listCharacters();
    if (!character) throw new Error('Missing demo character');
    const [conversation] = await client.listConversations(character.id);
    if (!conversation) throw new Error('Missing demo conversation');
    const view = render(WorkspaceApp, { client });
    await fireEvent.click(screen.getByRole('button', { name: t('navigation.chats') }));
    await fireEvent.click(
        await screen.findByRole('button', {
            name: (name) => name.startsWith(`${conversation.title} ·`),
        }),
    );
    const input = await screen.findByRole('textbox', { name: t('uiPreview.message') });
    return { ...view, client, character, conversation, input };
}
