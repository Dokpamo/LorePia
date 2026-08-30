import { INITIAL_APP_STATE, type LorepiaAppState } from '../../../app/app-controller';

export function chatReadyState(): LorepiaAppState {
    return {
        ...structuredClone(INITIAL_APP_STATE),
        selected_character: {
            id: 'character-1',
            name: '라온',
            description: '',
            source_hash: 'synthetic',
            avatar_asset_id: null,
            created_at: '2026-08-02T00:00:00Z',
        },
        selected_conversation: {
            id: 'conversation-1',
            character_id: 'character-1',
            title: '첫 대화',
            created_at: '2026-08-02T00:00:00Z',
            updated_at: '2026-08-02T00:00:00Z',
        },
        conversation_state: {
            conversation_id: 'conversation-1',
            active_branch_id: 'branch-1',
            selected_mode: 'chat',
            updated_at: '2026-08-02T00:00:00Z',
        },
        messages: { phase: 'ready', error: null, items: [] },
    };
}
