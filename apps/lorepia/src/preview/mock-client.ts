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

/*
 * A turn built to break a narrow column: an unbroken URL, a long identifier,
 * and a code block whose lines run past any phone. The preview exists to catch
 * exactly this before a device does.
 */
const STRESS_MESSAGE = {
    id: 'msg-4',
    role: 'assistant',
    content:
        '참고 링크: https://archive.example.com/collections/maritime/1801/silver-strait/logbook-transcription-volume-four-appendix.html\n\n식별자 `provider-connection-openai-responses-eu-west-1-deployment-0f3c9a21b7d84e6fa5c2` 를 쓰세요.\n\n```\ncurl -sS -X POST https://api.example.com/v1/responses -H "content-type: application/json" -d \'{"model":"gpt-x","input":"..."}\'\n```',
    status: 'complete',
};

const BOOTSTRAP = {
    app_version: '0.1.0',
    shell_api_version: 2,
    core_api_version: 9,
    chat_event_version: 4,
    core_version: '0.1.0',
    platform: 'macos',
    health: { database_open: true, data_root_writable: true },
};

/*
 * A provider workspace with real-shaped content. Empty arrays render empty
 * panels, which hide every layout problem the settings screen can have — the
 * identifiers and origins here are deliberately long, because that is what
 * actually strains a phone-width column.
 */
const CONNECTION = {
    id: 'conn-openai-responses-eu-west-1',
    template_id: 'tmpl-openai-responses',
    template_version: 3,
    display_name: 'OpenAI Responses (eu-west-1)',
    api_origin: 'https://api.openai-compatible-gateway.example.com',
    api_base_path: '/v1/responses',
    network_mode: 'public',
    local_network_approval: null,
    config_values: [],
    credential_binding_required: true,
    credential_status: 'present',
    credential_scope: null,
    approved_credential_origins: ['https://api.openai-compatible-gateway.example.com'],
    timeout_seconds: 60,
};

const ROUTE = {
    id: 'route-primary-long-lived-deployment',
    connection_id: CONNECTION.id,
    api_family: 'openai_responses',
    model_id: 'gpt-x-2026-08-01-preview-eu-west-1-deployment-0f3c9a21b7d84e6f',
    display_name: '기본 응답 모델',
    route_config: { deployment_id: null, region: 'eu-west-1', endpoint_path: null, values: [] },
    status: 'active',
    miss_count: 0,
    metadata_source: 'catalog',
};

const PRESET = {
    id: 'preset-balanced',
    model_route_id: ROUTE.id,
    display_name: '균형',
    values: [],
    reasoning: {
        mode: 'off',
        effort: null,
        budget_tokens: null,
        summary: 'none',
        preserve_opaque_state: false,
    },
    prompt_cache: { mode: 'off', ttl_kind: 'none' },
};

const PROVIDER_OVERVIEW = {
    settings: {
        preserve_partial_generations: true,
        selected_provider_profile_id: null,
        selected_model_route_id: ROUTE.id,
        selected_generation_preset_id: PRESET.id,
    },
    templates: [],
    connections: [CONNECTION],
    legacy_profiles: [],
};

/*
 * Catalog revisions carry a snapshot hash: sixty-four characters with nowhere
 * to break. An empty history hides every layout question that string asks, and
 * that is exactly how a real overflow reached the app unnoticed.
 */
const CATALOG_HISTORY = {
    history_schema_version: 1,
    active_revision: 4,
    revisions: [
        {
            revision: 4,
            captured_at: '2026-08-16T00:00:00Z',
            snapshot_sha256: 'e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855',
            signed_revisions: [4],
            active: true,
        },
        {
            revision: 3,
            captured_at: '2026-08-10T00:00:00Z',
            snapshot_sha256: '9f86d081884c7d659a2feaa0c55ad015a3bf4f1b2b0b822cd15d6c15b0f00a08',
            signed_revisions: [],
            active: false,
        },
    ],
    activations: [],
    next_before_revision: null,
    next_before_state_version: null,
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
    listBranchMessages: [...MESSAGES, STRESS_MESSAGE],
    listMessages: [...MESSAGES, STRESS_MESSAGE],
    getCharacterGreetingCatalog: {
        character_id: 'char-1',
        character_content_revision_id: 'rev-1',
        greetings: [],
    },
    getProviderOverview: PROVIDER_OVERVIEW,
    listModelRoutes: [ROUTE],
    listGenerationPresets: [PRESET],
    credentialStatus: { status: 'present' },
    providerCatalogStatus: null,
    providerCatalogHistory: CATALOG_HISTORY,
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
