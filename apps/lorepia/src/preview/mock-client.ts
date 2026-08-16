/**
 * Fixture-backed client used only by the design preview page.
 *
 * The real client speaks Tauri IPC, which does not exist in a browser. This
 * Proxy answers the handful of calls the UI makes while rendering and returns
 * an inert default for everything else, so the whole interface can be exercised
 * visually without a native build. It is never imported by `main.ts`.
 */

import type { LorepiaClient } from '../lib/ipc/contracts';

const CHARACTERS = [
    {
        id: 'char-1',
        name: '아리아',
        description: '기록 보관소의 사서. 오래된 이야기를 수집한다.',
        avatar_asset_id: null,
    },
    {
        id: 'char-2',
        name: '카이',
        description: '떠돌이 정비공. 말수가 적다.',
        avatar_asset_id: null,
    },
    {
        id: 'char-3',
        name: '세라',
        description: '항해사. 별자리로 길을 읽는다.',
        avatar_asset_id: null,
    },
];

const CONVERSATIONS = [
    { id: 'conv-1', character_id: 'char-1', title: '잊혀진 서고' },
    { id: 'conv-2', character_id: 'char-1', title: '비 오는 날의 기록' },
];

const MESSAGES = [
    {
        id: 'msg-1',
        role: 'assistant',
        content:
            '*서가 사이로 먼지가 떠오른다.* 어서 오세요. 이 시간에 찾아오는 분은 드문데요.\n\n찾으시는 책이 있나요? 아니면 그냥 **둘러보러** 오셨나요?',
        status: 'complete',
    },
    {
        id: 'msg-2',
        role: 'user',
        content: '오래된 항해 일지를 찾고 있어요.',
        status: 'complete',
    },
    {
        id: 'msg-3',
        role: 'assistant',
        content:
            '항해 일지라… *사다리를 끌어와 3층 서가로 올라간다.*\n\n> 북쪽 항로의 기록은 대부분 소실됐어요.\n\n남은 건 이 정도입니다:\n\n- 1801년 은빛 해협 일지\n- 표류자의 수기 (필사본)\n- `해도 부록 IV` — 훼손이 심합니다\n\n어느 쪽부터 보시겠어요?',
        status: 'complete',
    },
];

const BOOTSTRAP = {
    app_version: '0.1.0',
    shell_api_version: 2,
    core_api_version: 9,
    chat_event_version: 4,
    core_version: '0.1.0',
    platform: 'macos',
    health: { database_open: true, data_root_writable: true },
};

const FIXTURES: Record<string, unknown> = {
    bootstrapSnapshot: BOOTSTRAP,
    listCharacters: CHARACTERS,
    getCharacter: CHARACTERS[0],
    listConversations: CONVERSATIONS,
    openExistingConversation: CONVERSATIONS[0],
    createConversation: CONVERSATIONS[0],
    getConversationState: {
        conversation_id: 'conv-1',
        active_branch_id: 'branch-1',
        selected_mode: 'chat',
        updated_at: '2026-08-16T00:00:00Z',
    },
    listBranches: [
        {
            id: 'branch-1',
            conversation_id: 'conv-1',
            title: null,
            fork_message_id: null,
            head_message_id: 'msg-3',
            created_at: '2026-08-16T00:00:00Z',
        },
    ],
    listBranchMessages: MESSAGES,
    listMessages: MESSAGES,
    getCharacterGreetingCatalog: {
        character_id: 'char-1',
        character_content_revision_id: 'rev-1',
        greetings: [],
    },
    providerWorkspace: {
        settings: {
            preserve_partial_generations: true,
            selected_provider_profile_id: null,
            selected_model_route_id: null,
            selected_generation_preset_id: null,
        },
        profiles: [],
        connections: [],
        templates: [],
        model_routes: [],
        generation_presets: [],
        credential_targets: [],
    },
};

/** Inert defaults keep panels rendering instead of throwing mid-paint. */
function fallback(name: string): unknown {
    if (name.startsWith('list') || name.startsWith('search')) return [];
    if (name.startsWith('subscribe')) return () => undefined;
    return null;
}

export function createPreviewClient(): LorepiaClient {
    return new Proxy({} as LorepiaClient, {
        get(_target, property) {
            const name = String(property);
            if (name === 'then') return undefined;
            return (): Promise<unknown> =>
                Promise.resolve(name in FIXTURES ? FIXTURES[name] : fallback(name));
        },
    });
}
