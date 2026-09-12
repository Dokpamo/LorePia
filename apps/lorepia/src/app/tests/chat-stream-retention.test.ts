import { expect, it, vi } from 'vitest';
import { ChatStreamController } from '../controllers/chat-stream-controller';
import { INITIAL_APP_STATE, type LorepiaAppState } from '../app-state';
import { createAppControllerFixture, deferred } from './app-controller-test-support';

it('does not retain or apply retired-stream payloads while reconciliation disposal waits', async () => {
    const { conversation, conversationState, branch, mockClient, textEvent } =
        createAppControllerFixture();
    const disposal = deferred<boolean>();
    let state: LorepiaAppState = {
        ...structuredClone(INITIAL_APP_STATE),
        selected_conversation: conversation,
        conversation_state: conversationState,
        branches: [branch],
    };
    const client = mockClient({ disposeChatStream: () => disposal.promise });
    const stream = new ChatStreamController(
        {
            client,
            readState: () => state,
            update: (update) => {
                state = update(state);
            },
            announce: vi.fn(),
            errorLabel: () => 'error',
        },
        { refreshMemoryQueryRetries: () => Promise.resolve() },
    );
    const { epoch, streamId } = stream.prepareStream(conversation.id, branch.id, 'generation-1');
    const recovery = stream.reconcile('generation-1', epoch, streamId, 'sequence_gap', 1);
    const items = Array.from({ length: 1000 }, (_, index) =>
        textEvent(index + 2, 'x'.repeat(8192)),
    );
    for (const item of items) stream.acceptStreamItem(item, epoch, streamId);
    expect(state.chat.streaming_text).toBe('');
    // Inspect retention, not a private field name: no controller-owned queue may retain these retired items.
    for (const value of Object.values(stream)) {
        if (Array.isArray(value))
            expect(
                value.some((item: unknown) => items.includes(item as (typeof items)[number])),
            ).toBe(false);
    }
    disposal.resolve(true);
    await recovery;
    expect(state.messages.phase).toBe('ready');
    expect(state.chat.streaming_text).toBe('');
    stream.detachStream();
});
