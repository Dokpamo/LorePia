import { describe, expect, it, vi } from 'vitest';
import { INITIAL_APP_STATE } from '../app-state';
import { createAppControllerFixture } from '../tests/app-controller-test-support';
import { ConversationProjection } from './workspace-projection';
import type { MessageDto } from '../../lib/ipc/contracts';
import { queryCharacterLibrary } from '../../ui/navigation/character-library-query';
import { queryConversationLibrary } from '../../ui/navigation/conversation-library-query';

const fixture = createAppControllerFixture();
function history(count = 5000) {
    const messages: MessageDto[] = Array.from({ length: count }, (_, index) => ({
        id: `saved-${String(index)}`,
        conversation_id: fixture.conversation.id,
        parent_id: null,
        role: 'assistant',
        content: `Saved ${String(index)}`,
        status: 'complete',
        generation_id: null,
        created_at: fixture.conversation.created_at,
    }));
    const state = structuredClone(INITIAL_APP_STATE);
    state.selected_conversation = fixture.conversation;
    state.conversation_state = fixture.conversationState;
    state.messages = { phase: 'ready', error: null, items: messages };
    state.chat.live_assistant_message_id = 'streaming';
    state.chat.streaming_text = 'first';
    return state;
}

describe('streaming projection cost', () => {
    it('retains all 5,000 saved view identities and the scroll collection over 100 tokens', () => {
        const state = history();
        const projection = new ConversationProjection();
        const initial = projection.project(state);
        if (!initial) throw new Error('Missing projection');
        const read = vi.spyOn(state.messages.items, 'filter');
        for (let token = 0; token < 100; token++) {
            const next = projection.project({
                ...state,
                chat: { ...state.chat, streaming_text: `token ${String(token)}` },
            });
            if (!next) throw new Error('Missing projection');
            expect(next.scrollMessages).toBe(initial.scrollMessages);
            expect(next.messages[0]).toBe(initial.messages[0]);
            expect(next.messages[4999]).toBe(initial.messages[4999]);
            expect(next.messages.at(-1)?.text).toBe(`token ${String(token)}`);
        }
        expect(read).not.toHaveBeenCalled();
    });

    it('invalidates saved edits, terminal transitions and conversation switches', () => {
        const state = history(2);
        const projection = new ConversationProjection();
        const initial = projection.project(state);
        if (!initial) throw new Error('Missing projection');
        state.messages = {
            ...state.messages,
            items: state.messages.items.map((item, index) =>
                index === 0 ? { ...item, content: 'Edited' } : item,
            ),
        };
        const edited = projection.project(state);
        if (!edited) throw new Error('Missing projection');
        expect(edited.scrollMessages).not.toBe(initial.scrollMessages);
        expect(edited.messages[0]?.text).toBe('Edited');
        state.chat.live_assistant_message_id = null;
        expect(projection.project(state)?.messages).toHaveLength(2);
        state.selected_conversation = null;
        expect(projection.project(state)).toBeNull();
        state.selected_conversation = { ...fixture.conversation, id: 'another-room' };
        state.messages.items = [];
        expect(projection.project(state)?.messages).toEqual([]);
    });
});

describe('library sorting cost', () => {
    it('parses each date once and does not normalize descriptions for an empty search', () => {
        const description = vi.fn(() => 'long description'.repeat(1000));
        const cards = Array.from({ length: 1000 }, (_, index) => ({
            id: `card-${String(index)}`,
            name: `Card ${String(1000 - index)}`,
            get description() {
                return description();
            },
            thumbnail: '',
            subpage: false,
            histories: [],
            createdAt: fixture.character.created_at,
        }));
        const parse = vi.spyOn(Date, 'parse');
        expect(queryCharacterLibrary(cards, '', 'newest')).toHaveLength(1000);
        expect(description).not.toHaveBeenCalled();
        expect(parse).toHaveBeenCalledTimes(1000);
        parse.mockClear();
        queryCharacterLibrary(cards, '', 'name');
        expect(parse).not.toHaveBeenCalled();
        const chats = cards.map((item) => ({
            id: item.id,
            title: item.name,
            characterId: 'card',
            characterName: 'Name',
            date: '',
            updatedAt: fixture.conversation.updated_at,
        }));
        queryConversationLibrary(chats, '', '', 'newest');
        expect(parse).toHaveBeenCalledTimes(1000);
        parse.mockRestore();
    });
});

it('reuses branch views across stream batches and invalidates renamed/reordered branches', () => {
    const state = history(2);
    state.branches = Array.from({ length: 500 }, (_, index) => ({
        ...fixture.branch,
        id: `b-${String(index)}`,
        title: `Branch ${String(index)}`,
    }));
    const projection = new ConversationProjection();
    const initial = projection.project(state);
    const map = vi.spyOn(state.branches, 'map');
    for (let index = 0; index < 1000; index++) {
        state.chat.streaming_text = String(index);
        expect(projection.project(state)?.branches).toBe(initial?.branches);
    }
    expect(map).not.toHaveBeenCalled();
    state.branches = state.branches
        .slice()
        .reverse()
        .map((branch) => ({ ...branch, title: 'Renamed' }));
    const updated = projection.project(state)?.branches;
    expect(updated).not.toBe(initial?.branches);
    expect(updated?.[0]).toMatchObject({ id: 'b-499', title: 'Renamed' });
});
