import { get } from 'svelte/store';
import { describe, expect, it, vi } from 'vitest';
import { LorepiaAppController } from '../app-controller';
import { createAppControllerFixture, deferred } from '../tests/app-controller-test-support';
import { LiveChatSession } from './live-chat-session.svelte';
import { conversationView } from './workspace-projection';

const fixture = createAppControllerFixture();
async function setup() {
    const sendMessage = vi.fn(() => Promise.resolve({ generation_id: 'generation-live' }));
    const client = fixture.mockClient({ sendMessage });
    const controller = new LorepiaAppController(client);
    await controller.start();
    await controller.selectCharacter(fixture.character);
    await controller.selectConversation(fixture.conversation);
    const session = new LiveChatSession(controller, () => get(controller.state));
    return { controller, session, sendMessage };
}

describe('live workspace chat bridge', () => {
    it('sends through the existing controller with the selected durable room and branch', async () => {
        const { controller, session, sendMessage } = await setup();
        session.draft = 'connected input';
        expect(await session.send((text) => controller.sendMessage(text))).toBe(true);
        expect(sendMessage).toHaveBeenCalledWith(
            expect.objectContaining({
                conversation_id: fixture.conversation.id,
                branch_id: fixture.branch.id,
            }),
            expect.any(String),
            expect.any(Function),
        );
        expect(session.draft).toBe('');
        expect(session.busy).toBe(true);
        controller.destroy();
    });
    it('retains input after rejection and after newer typing during an acknowledgement', async () => {
        const { controller, session } = await setup();
        session.draft = 'retain rejected draft';
        expect(await session.send(() => Promise.resolve(false))).toBe(false);
        expect(session.draft).toBe('retain rejected draft');
        const pending = deferred<boolean>();
        const sent = session.send(() => pending.promise);
        session.draft = 'newer draft';
        pending.resolve(true);
        await sent;
        expect(session.draft).toBe('newer draft');
        controller.destroy();
    });
    it('does not clear another conversation draft after switching during a send', async () => {
        const { controller, session } = await setup();
        const oldScope = session.scope;
        session.draft = 'first room draft';
        const pending = deferred<boolean>();
        const sent = session.send(() => pending.promise);
        await controller.selectCharacter({ ...fixture.character, id: 'another-character' });
        session.draft = 'another room draft';
        pending.resolve(true);
        await sent;
        expect(session.draft).toBe('another room draft');
        expect(session.drafts[oldScope]).toBe('');
        controller.destroy();
    });
    it('projects verified streaming state once and keeps stored message metadata', async () => {
        const { controller } = await setup();
        const state = get(controller.state);
        state.messages.items = [
            {
                id: 'response-1',
                conversation_id: fixture.conversation.id,
                parent_id: null,
                role: 'assistant',
                content: 'stored prefix',
                status: 'pending',
                generation_id: 'generation-live',
                created_at: '2026-09-08T12:00:00Z',
            },
        ];
        state.chat.live_assistant_message_id = 'response-1';
        state.chat.streaming_text = 'verified stream';
        state.chat.active_generation_id = 'generation-live';
        const view = conversationView(state);
        expect(view?.messages).toHaveLength(1);
        expect(view?.messages[0]).toMatchObject({
            text: 'verified stream',
            status: 'pending',
            source: { created_at: '2026-09-08T12:00:00Z' },
        });
        expect(view?.messages.some((item) => item.sample)).toBe(false);
        controller.destroy();
    });
});
