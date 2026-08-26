import type {
    AppSettingsDto,
    BootstrapDto,
    CharacterDto,
    CharacterGreetingCatalogDto,
    ConversationBranchDto,
    ConversationDto,
    ConversationStateDto,
    CreatorContentModuleDocumentDto,
    CreatorInteractionRuleSetDocumentDto,
    CreatorKnowledgeBookDocumentDto,
    CreatorMemoryProfileDocumentDto,
    CreatorPromptPresetDocumentDto,
    CreatorTransformSetDocumentDto,
    GenerationPresetDto,
    MemorySupervisorStatusDto,
    MessageDto,
    ModelRouteDto,
    OrchestrationWorkspaceDto,
    ProviderCatalogHistoryDto,
    ProviderCatalogStatusDto,
    ProviderConnectionDto,
    ProviderOverviewDto,
    ProviderTemplateDto,
    RevisionedDto,
    TaskProfileDocumentDto,
} from '../lib/ipc/contracts';
import type { PersonaDto } from '../features/personas/persona-contracts';
import { emptyOrchestrationWorkspace } from '../features/orchestration/orchestration-controller';

export const DEMO_INITIAL_CHARACTER_ID = 'character-aria';
export const DEMO_INITIAL_CONVERSATION_ID = 'conversation-archive';

const CREATED_AT = '2026-08-20T09:30:00.000Z';
const UPDATED_AT = '2026-08-24T12:42:00.000Z';
const CATALOG_SHA = 'b'.repeat(64);
const BASELINE_SHA = 'c'.repeat(64);

export const DEMO_BOOTSTRAP: BootstrapDto = {
    app_version: '0.1.0-demo',
    shell_api_version: 2,
    core_version: '0.1.0-demo',
    core_api_version: 9,
    chat_event_version: 4,
    creator_schema_version: 1,
    platform: 'macos',
    health: {
        core_version: '0.1.0-demo',
        database_open: true,
        schema_version: 1,
        data_root_writable: true,
        staging_writable: true,
        recovery_pending: false,
        active_jobs: 0,
    },
    capabilities: {
        file_picker: false,
        credential_store: true,
        native_menu: true,
        notifications: true,
        creator_runtime: true,
    },
};

export const DEMO_MEMORY_SUPERVISOR: MemorySupervisorStatusDto = {
    sequence: 7,
    phase: 'running',
    recovered_interrupted_jobs: 0,
    completed_jobs: 12,
};

export const DEMO_CHARACTERS: CharacterDto[] = [
    {
        id: DEMO_INITIAL_CHARACTER_ID,
        name: '아리아',
        description: '오래된 항해 기록을 지키는 달빛 서고의 사서',
        source_hash: '1'.repeat(64),
        avatar_asset_id: null,
        created_at: '2026-08-20T09:30:00.000Z',
    },
    {
        id: 'character-kai',
        name: '카이',
        description: '도시 외곽을 떠도는 무뚝뚝한 기계 정비공',
        source_hash: '2'.repeat(64),
        avatar_asset_id: null,
        created_at: '2026-08-21T03:15:00.000Z',
    },
    {
        id: 'character-sera',
        name: '세라',
        description: '별의 움직임으로 미지의 항로를 찾는 항해사',
        source_hash: '3'.repeat(64),
        avatar_asset_id: null,
        created_at: '2026-08-22T16:00:00.000Z',
    },
    {
        id: 'character-noa',
        name: '노아',
        description: '꿈에서 본 장면을 짧은 이야기로 기록하는 화가',
        source_hash: '4'.repeat(64),
        avatar_asset_id: null,
        created_at: '2026-08-23T07:40:00.000Z',
    },
];

export const DEMO_CONVERSATIONS: ConversationDto[] = [
    {
        id: DEMO_INITIAL_CONVERSATION_ID,
        character_id: DEMO_INITIAL_CHARACTER_ID,
        title: '잊혀진 서고',
        created_at: '2026-08-20T10:00:00.000Z',
        updated_at: '2026-08-24T12:42:00.000Z',
    },
    {
        id: 'conversation-rain',
        character_id: DEMO_INITIAL_CHARACTER_ID,
        title: '비 오는 날의 기록',
        created_at: '2026-08-21T12:10:00.000Z',
        updated_at: '2026-08-23T18:05:00.000Z',
    },
    {
        id: 'conversation-workshop',
        character_id: 'character-kai',
        title: '고장 난 라디오',
        created_at: '2026-08-22T06:20:00.000Z',
        updated_at: '2026-08-24T08:12:00.000Z',
    },
    {
        id: 'conversation-stars',
        character_id: 'character-sera',
        title: '북쪽 하늘의 신호',
        created_at: '2026-08-23T20:00:00.000Z',
        updated_at: '2026-08-24T00:15:00.000Z',
    },
];

export const DEMO_BRANCHES: ConversationBranchDto[] = [
    {
        id: 'branch-archive-main',
        conversation_id: DEMO_INITIAL_CONVERSATION_ID,
        title: '본편',
        fork_message_id: null,
        head_message_id: 'message-archive-6',
        created_at: '2026-08-20T10:00:00.000Z',
        updated_at: '2026-08-24T12:42:00.000Z',
    },
    {
        id: 'branch-archive-map',
        conversation_id: DEMO_INITIAL_CONVERSATION_ID,
        title: '해도를 먼저 펼친 경우',
        fork_message_id: 'message-archive-3',
        head_message_id: 'message-map-2',
        created_at: '2026-08-24T12:18:00.000Z',
        updated_at: '2026-08-24T12:25:00.000Z',
    },
    {
        id: 'branch-rain-main',
        conversation_id: 'conversation-rain',
        title: null,
        fork_message_id: null,
        head_message_id: 'message-rain-2',
        created_at: '2026-08-21T12:10:00.000Z',
        updated_at: '2026-08-23T18:05:00.000Z',
    },
    {
        id: 'branch-workshop-main',
        conversation_id: 'conversation-workshop',
        title: null,
        fork_message_id: null,
        head_message_id: 'message-workshop-2',
        created_at: '2026-08-22T06:20:00.000Z',
        updated_at: '2026-08-24T08:12:00.000Z',
    },
    {
        id: 'branch-stars-main',
        conversation_id: 'conversation-stars',
        title: null,
        fork_message_id: null,
        head_message_id: 'message-stars-2',
        created_at: '2026-08-23T20:00:00.000Z',
        updated_at: '2026-08-24T00:15:00.000Z',
    },
];

const DEMO_ARCHIVE_THURSDAY_TURNS: readonly {
    role: 'user' | 'assistant';
    content: string;
    minute: number;
}[] = [
    {
        role: 'user',
        content: '세 기록이 같은 항로를 가리키는지 먼저 비교해 볼 수 있을까요?',
        minute: 4,
    },
    {
        role: 'assistant',
        content: '가능해요. 표지의 문장과 기록된 조류 방향부터 나란히 놓아 볼게요.',
        minute: 6,
    },
    {
        role: 'user',
        content: '은빛 해협의 항해 일지에는 마지막 출항일이 언제라고 적혀 있어요?',
        minute: 8,
    },
    {
        role: 'assistant',
        content: '1801년 6월 17일이에요. 다만 선장이 남긴 서명은 그보다 사흘 뒤로 되어 있네요.',
        minute: 10,
    },
    {
        role: 'user',
        content: '항해가 시작된 뒤에 서명했다는 뜻일까요?',
        minute: 12,
    },
    {
        role: 'assistant',
        content:
            '그럴 수도 있지만, 종이 가장자리에 소금기가 거의 없어요. 육지에서 나중에 끼워 넣은 장일 가능성도 있어요.',
        minute: 14,
    },
    {
        role: 'user',
        content: '표류자의 수기와 필체도 비교해 줘요.',
        minute: 16,
    },
    {
        role: 'assistant',
        content:
            '두 기록 모두 숫자 7의 끝을 길게 올려 쓰지만, 잉크 성분은 달라요. 같은 사람이 다른 시기에 썼다고 단정하기는 어렵겠어요.',
        minute: 18,
    },
    {
        role: 'user',
        content: '해도 부록 IV에서 지워진 부분은 복원할 수 있나요?',
        minute: 20,
    },
    {
        role: 'assistant',
        content:
            '*얇은 빛판 위에 해도를 펼친다.* 희미한 압흔이 남아 있어서 항로의 절반 정도는 따라갈 수 있어요.',
        minute: 22,
    },
    {
        role: 'user',
        content: '그 항로가 지도에 없는 섬으로 이어지나요?',
        minute: 24,
    },
    {
        role: 'assistant',
        content:
            '정확히는 섬이 아니라 빈 해역에서 멈춰요. 좌표 옆에는 작은 초승달 표시가 반복되어 있고요.',
        minute: 26,
    },
    {
        role: 'user',
        content: '초승달 표시는 출항 시간일 수도 있겠네요.',
        minute: 28,
    },
    {
        role: 'assistant',
        content:
            '좋은 추측이에요. 세 기록 모두 새벽 썰물 때 방향이 바뀐다고 적혀 있으니 서로 연결되는 단서일 수 있어요.',
        minute: 30,
    },
    {
        role: 'user',
        content: '오늘은 여기까지 정리하고, 다음에는 표류자의 수기를 자세히 읽어볼게요.',
        minute: 32,
    },
    {
        role: 'assistant',
        content:
            '표식과 날짜를 순서대로 묶어 두었어요. 다시 오시면 바로 이어서 살펴볼 수 있게 이 자리에 보관해 둘게요.',
        minute: 34,
    },
];

function archiveThursdayMessageId(index: number): string {
    return `message-archive-thursday-${String(index + 1).padStart(2, '0')}`;
}

export const DEMO_ARCHIVE_THURSDAY_MESSAGES: MessageDto[] = DEMO_ARCHIVE_THURSDAY_TURNS.map(
    (turn, index) => ({
        id: archiveThursdayMessageId(index),
        conversation_id: DEMO_INITIAL_CONVERSATION_ID,
        parent_id: index === 0 ? 'message-archive-3' : archiveThursdayMessageId(index - 1),
        role: turn.role,
        content: turn.content,
        status: 'complete',
        generation_id:
            turn.role === 'assistant'
                ? `generation-archive-thursday-${String(index + 1).padStart(2, '0')}`
                : null,
        created_at: `2026-08-20T10:${String(turn.minute).padStart(2, '0')}:00.000Z`,
    }),
);

export const DEMO_ARCHIVE_MAIN_MESSAGE_IDS = [
    'message-archive-1',
    'message-archive-2',
    'message-archive-3',
    ...DEMO_ARCHIVE_THURSDAY_MESSAGES.map((message) => message.id),
    'message-archive-4',
    'message-archive-5',
    'message-archive-6',
];

export const DEMO_MESSAGES: MessageDto[] = [
    {
        id: 'message-archive-1',
        conversation_id: DEMO_INITIAL_CONVERSATION_ID,
        parent_id: null,
        role: 'assistant',
        content:
            '*서가 사이로 옅은 먼지가 떠오른다.* 어서 오세요. 이 시간에 찾아오는 분은 드문데요.\n\n찾으시는 기록이 있나요?',
        status: 'complete',
        generation_id: 'generation-archive-1',
        created_at: '2026-08-20T10:00:00.000Z',
    },
    {
        id: 'message-archive-2',
        conversation_id: DEMO_INITIAL_CONVERSATION_ID,
        parent_id: 'message-archive-1',
        role: 'user',
        content: '1801년에 사라진 북쪽 항로의 항해 일지를 찾고 있어요.',
        status: 'complete',
        generation_id: null,
        created_at: '2026-08-20T10:01:00.000Z',
    },
    {
        id: 'message-archive-3',
        conversation_id: DEMO_INITIAL_CONVERSATION_ID,
        parent_id: 'message-archive-2',
        role: 'assistant',
        content:
            '북쪽 항로라… *사다리를 끌어와 세 번째 서가로 올라간다.*\n\n> 공식 기록은 대부분 소실됐어요.\n\n남은 단서는 세 가지예요.\n\n- 은빛 해협의 항해 일지\n- 표류자의 수기\n- 훼손된 `해도 부록 IV`',
        status: 'complete',
        generation_id: 'generation-archive-2',
        created_at: '2026-08-20T10:02:00.000Z',
    },
    ...DEMO_ARCHIVE_THURSDAY_MESSAGES,
    {
        id: 'message-archive-4',
        conversation_id: DEMO_INITIAL_CONVERSATION_ID,
        parent_id: 'message-archive-thursday-16',
        role: 'user',
        content: '표류자의 수기부터 보여줘. 지도에 없는 섬이 언급됐는지도 궁금해.',
        status: 'complete',
        generation_id: null,
        created_at: '2026-08-24T12:37:00.000Z',
    },
    {
        id: 'message-archive-5',
        conversation_id: DEMO_INITIAL_CONVERSATION_ID,
        parent_id: 'message-archive-4',
        role: 'assistant',
        content:
            '여기 있어요. 마지막 장 가장자리에 **“새벽마다 위치가 바뀌는 섬”**이라는 메모가 남아 있네요. 잉크가 달라서 원문이 아니라 누군가 나중에 덧붙인 것 같아요.',
        status: 'complete',
        generation_id: 'generation-archive-3',
        created_at: '2026-08-24T12:39:00.000Z',
    },
    {
        id: 'message-archive-6',
        conversation_id: DEMO_INITIAL_CONVERSATION_ID,
        parent_id: 'message-archive-5',
        role: 'assistant',
        content:
            '레이아웃 점검용 긴 문자열도 함께 보관해 둘게요: `provider-connection-openai-responses-eu-west-1-deployment-0f3c9a21b7d84e6fa5c2`',
        status: 'complete',
        generation_id: 'generation-archive-4',
        created_at: '2026-08-24T12:42:00.000Z',
    },
    {
        id: 'message-map-1',
        conversation_id: DEMO_INITIAL_CONVERSATION_ID,
        parent_id: 'message-archive-3',
        role: 'user',
        content: '해도 부록부터 펼쳐볼게.',
        status: 'complete',
        generation_id: null,
        created_at: '2026-08-24T12:19:00.000Z',
    },
    {
        id: 'message-map-2',
        conversation_id: DEMO_INITIAL_CONVERSATION_ID,
        parent_id: 'message-map-1',
        role: 'assistant',
        content: '찢어진 모서리를 맞추니 북동쪽에 지워진 항로 하나가 드러나요.',
        status: 'complete',
        generation_id: 'generation-map-1',
        created_at: '2026-08-24T12:25:00.000Z',
    },
    {
        id: 'message-rain-1',
        conversation_id: 'conversation-rain',
        parent_id: null,
        role: 'user',
        content: '비가 오면 서고는 어떤 냄새가 나?',
        status: 'complete',
        generation_id: null,
        created_at: '2026-08-23T18:03:00.000Z',
    },
    {
        id: 'message-rain-2',
        conversation_id: 'conversation-rain',
        parent_id: 'message-rain-1',
        role: 'assistant',
        content: '젖은 나무와 오래된 종이 냄새가 섞여요. 이상하게 마음이 차분해지죠.',
        status: 'complete',
        generation_id: 'generation-rain-1',
        created_at: '2026-08-23T18:05:00.000Z',
    },
    {
        id: 'message-workshop-1',
        conversation_id: 'conversation-workshop',
        parent_id: null,
        role: 'user',
        content: '이 라디오, 다시 켤 수 있을까?',
        status: 'complete',
        generation_id: null,
        created_at: '2026-08-24T08:10:00.000Z',
    },
    {
        id: 'message-workshop-2',
        conversation_id: 'conversation-workshop',
        parent_id: 'message-workshop-1',
        role: 'assistant',
        content: '*드라이버를 돌려 뒤판을 연다.* 부품은 멀쩡해. 접점만 닦으면 돼.',
        status: 'complete',
        generation_id: 'generation-workshop-1',
        created_at: '2026-08-24T08:12:00.000Z',
    },
    {
        id: 'message-stars-1',
        conversation_id: 'conversation-stars',
        parent_id: null,
        role: 'user',
        content: '북쪽 하늘에서 같은 빛이 세 번 깜빡였어.',
        status: 'complete',
        generation_id: null,
        created_at: '2026-08-24T00:12:00.000Z',
    },
    {
        id: 'message-stars-2',
        conversation_id: 'conversation-stars',
        parent_id: 'message-stars-1',
        role: 'assistant',
        content: '별빛이 아니라 신호일 가능성이 높아. 관측 시간을 기록해 두자.',
        status: 'complete',
        generation_id: 'generation-stars-1',
        created_at: '2026-08-24T00:15:00.000Z',
    },
];

export const DEMO_GREETINGS: Record<string, CharacterGreetingCatalogDto> = Object.fromEntries(
    DEMO_CHARACTERS.map((character) => [
        character.id,
        {
            character_id: character.id,
            character_content_revision_id: `${character.id}-content-r3`,
            greetings: [
                { id: `${character.id}-greeting-default`, kind: 'default', enabled: true },
                { id: `${character.id}-greeting-evening`, kind: 'alternate', enabled: true },
            ],
        },
    ]),
);

export const DEMO_SETTINGS: AppSettingsDto = {
    preserve_partial_generations: true,
    selected_provider_profile_id: null,
    selected_model_route_id: 'route-openai-demo',
    selected_generation_preset_id: 'preset-balanced-demo',
};

export const DEMO_PROVIDER_TEMPLATES: ProviderTemplateDto[] = [
    {
        id: 'template-openai-compatible',
        display_name: 'OpenAI 호환 API',
        manifest_version: 3,
        source: 'bundled',
        api_family: 'openai_responses',
        connection_fields: [
            {
                key: 'api_origin',
                label_key: 'API 주소',
                description_key: 'HTTPS API의 기준 주소',
                value_type: 'url',
                required: true,
            },
        ],
        default_network_mode: 'public',
        default_api_origin: 'https://api.openai.com',
        credential_required: true,
        supports_model_listing: true,
        auth_binding: { kind: 'bearer_header' },
        parameters: [],
    },
    {
        id: 'template-local-compatible',
        display_name: '로컬 호환 서버',
        manifest_version: 1,
        source: 'bundled',
        api_family: 'openai_chat_completions',
        connection_fields: [],
        default_network_mode: 'local_loopback',
        default_api_origin: 'http://127.0.0.1:11434',
        credential_required: false,
        supports_model_listing: true,
        auth_binding: { kind: 'none' },
        parameters: [],
    },
];

export const DEMO_PROVIDER_CONNECTIONS: ProviderConnectionDto[] = [
    {
        id: 'connection-openai-demo',
        template_id: 'template-openai-compatible',
        template_version: 3,
        display_name: '기본 AI 연결',
        api_origin: 'https://api.openai.com',
        api_base_path: '/v1/responses',
        network_mode: 'public',
        local_network_approval: null,
        config_values: [],
        credential_binding_required: true,
        credential_status: 'available',
        credential_scope: {
            allowed_origins: ['https://api.openai.com'],
            auth_binding: { kind: 'bearer_header' },
            redirect_policy: 'same_origin',
        },
        approved_credential_origins: ['https://api.openai.com'],
        timeout_seconds: 60,
        status: 'active',
        created_at: CREATED_AT,
        updated_at: UPDATED_AT,
    },
];

export const DEMO_MODEL_ROUTES: ModelRouteDto[] = [
    {
        id: 'route-openai-demo',
        connection_id: 'connection-openai-demo',
        api_family: 'openai_responses',
        model_id: 'gpt-demo-balanced-2026-08',
        display_name: '균형 응답 모델',
        route_config: {
            deployment_id: null,
            region: null,
            endpoint_path: null,
            values: [],
        },
        status: 'available',
        miss_count: 0,
        metadata_source: 'demo_fixture',
        metadata_observed_at: UPDATED_AT,
        first_seen_at: CREATED_AT,
        last_seen_at: UPDATED_AT,
    },
];

export const DEMO_GENERATION_PRESETS: GenerationPresetDto[] = [
    {
        id: 'preset-balanced-demo',
        model_route_id: 'route-openai-demo',
        display_name: '균형',
        values: [],
        reasoning: {
            mode: 'provider_default',
            effort: null,
            budget_tokens: null,
            summary: 'provider_default',
            preserve_opaque_state: false,
        },
        prompt_cache: {
            mode: 'provider_default',
            ttl_kind: 'provider_default',
            ttl_seconds: null,
            context_reference: null,
        },
        created_at: CREATED_AT,
        updated_at: UPDATED_AT,
    },
    {
        id: 'preset-creative-demo',
        model_route_id: 'route-openai-demo',
        display_name: '창작 집중',
        values: [],
        reasoning: {
            mode: 'provider_default',
            effort: 'medium',
            budget_tokens: null,
            summary: 'provider_default',
            preserve_opaque_state: false,
        },
        prompt_cache: {
            mode: 'provider_default',
            ttl_kind: 'provider_default',
            ttl_seconds: null,
            context_reference: null,
        },
        created_at: CREATED_AT,
        updated_at: UPDATED_AT,
    },
];

export const DEMO_PROVIDER_OVERVIEW: ProviderOverviewDto = {
    settings: DEMO_SETTINGS,
    templates: DEMO_PROVIDER_TEMPLATES,
    connections: DEMO_PROVIDER_CONNECTIONS,
    legacy_profiles: [],
};

export const DEMO_CATALOG_STATUS: ProviderCatalogStatusDto = {
    status_schema_version: 1,
    state_version: 4,
    active_revision: 4,
    active_snapshot_sha256: CATALOG_SHA,
    bundled_baseline_sha256: BASELINE_SHA,
    snapshot_count: 4,
    signed_update_count: 2,
    highest_accepted_revision: 4,
    latest_issued_at: '2026-08-22T00:00:00.000Z',
    active_signed_revisions: [3, 4],
};

export const DEMO_CATALOG_HISTORY: ProviderCatalogHistoryDto = {
    history_schema_version: 1,
    active_revision: 4,
    revisions: [
        {
            revision: 4,
            captured_at: '2026-08-22T00:00:00.000Z',
            snapshot_sha256: CATALOG_SHA,
            signed_revisions: [4],
            active: true,
        },
        {
            revision: 3,
            captured_at: '2026-08-10T00:00:00.000Z',
            snapshot_sha256: 'd'.repeat(64),
            signed_revisions: [3],
            active: false,
        },
    ],
    activations: [],
    next_before_revision: null,
    next_before_state_version: null,
};

export const DEMO_PERSONAS: PersonaDto[] = [
    {
        value: {
            id: 'persona-traveler',
            name: '호기심 많은 여행자',
            description:
                '낯선 장소의 역사와 사람들의 이야기를 꼼꼼히 기록합니다. 위험 앞에서도 질문을 멈추지 않습니다.',
        },
        revision: 3,
        revision_id: 'persona-traveler-revision-3',
        created_at: '2026-08-20T11:00:00.000Z',
        updated_at: '2026-08-24T09:00:00.000Z',
    },
    {
        value: {
            id: 'persona-detective',
            name: '차분한 기록 조사관',
            description: '단서의 출처를 확인하고 성급한 결론을 피하는 조사관입니다.',
        },
        revision: 1,
        revision_id: 'persona-detective-revision-1',
        created_at: '2026-08-22T04:00:00.000Z',
        updated_at: '2026-08-22T04:00:00.000Z',
    },
    {
        value: {
            id: 'persona-writer',
            name: '장면 중심의 작가',
            description: '대사의 리듬과 공간의 분위기를 살려 장면을 풍부하게 이어 갑니다.',
        },
        revision: 2,
        revision_id: 'persona-writer-revision-2',
        created_at: '2026-08-23T01:30:00.000Z',
        updated_at: '2026-08-24T06:15:00.000Z',
    },
];

const DEMO_PROMPT_DOCUMENT: CreatorPromptPresetDocumentDto = {
    id: 'prompt-story-demo',
    name: '몰입형 대화',
    schema_version: 1,
    blocks: [
        {
            id: 'prompt-block-character',
            name: '캐릭터 정체성',
            kind: 'character_identity',
            enabled: true,
            role_hint: 'system',
            authority: 'creator',
            template: {
                parts: [
                    { kind: 'text', value: '당신은 ' },
                    { kind: 'built_in', value: 'character_name' },
                    { kind: 'text', value: '입니다. 설정과 말투를 유지하세요.' },
                ],
                max_output_chars: 2_000,
            },
            condition: null,
            source: { kind: 'template' },
            placement_zone: 'character_context',
            history_selector: null,
            token_policy: {
                priority: 100,
                min_tokens: 32,
                max_tokens: 512,
                reserve_tokens: null,
            },
            overflow_policy: 'trim_tail',
            merge_policy: 'separate_message',
            provenance: {
                source_kind: 'user_created',
                source_id: 'demo-data',
                source_hash: null,
                author: 'LorePia Demo',
                license: null,
                imported_at: null,
            },
        },
    ],
    controls: [],
    default_values: { values: [] },
    default_generation_preset_id: 'preset-balanced-demo',
    memory_profile_id: 'memory-profile-story-demo',
    knowledge_book_ids: ['knowledge-book-archive-demo'],
    transform_set_ids: ['transform-set-display-demo'],
    module_ids: ['module-atmosphere-demo'],
    cache_boundaries: [],
    metadata: {
        description: '캐릭터 정체성과 최근 대화, 장기기억을 균형 있게 조합합니다.',
        tags: ['대화', '스토리', '데모'],
        provenance: {
            source_kind: 'user_created',
            source_id: 'demo-data',
            source_hash: null,
            author: 'LorePia Demo',
            license: null,
            imported_at: null,
        },
        created_at: CREATED_AT,
        updated_at: UPDATED_AT,
        local_override_of: null,
    },
};

function revisioned<Value>(value: Value, revision = 1): RevisionedDto<Value> {
    return {
        value,
        revision,
        created_at: CREATED_AT,
        updated_at: UPDATED_AT,
        deleted_at: null,
    };
}

export const DEMO_EDITABLE_PROMPT_PRESET = revisioned(DEMO_PROMPT_DOCUMENT, 3);

export const DEMO_TASK_PROFILE_DOCUMENTS: RevisionedDto<TaskProfileDocumentDto>[] = [
    revisioned(
        {
            id: 'task-memory-summary-demo',
            kind: 'memory_summary',
            route_id: 'route-openai-demo',
            generation_preset_id: 'preset-balanced-demo',
            fallback_route_ids: [],
            embedding_dimensions: null,
            timeout_ms: 30_000,
            rate_limit: { requests: 10, per_seconds: 60 },
            concurrency_limit: 1,
        },
        2,
    ),
];

export const DEMO_MEMORY_PROFILE_DOCUMENTS: RevisionedDto<CreatorMemoryProfileDocumentDto>[] = [
    revisioned(
        {
            id: 'memory-profile-story-demo',
            name: '스토리 기억',
            summary_task: 'task-memory-summary-demo',
            embedding_task: null,
            turns_per_summary: 8,
            recent_raw_budget: { max_tokens: 2_048 },
            episodic_budget: { max_tokens: 1_024 },
            semantic_budget: { max_tokens: 1_024 },
            retrieval_count: 8,
            recency_weight: 1,
            similarity_weight: 1,
            importance_weight: 1.2,
            preserve_invalidated_records: false,
            summary_schema: '핵심 사건, 관계 변화, 미해결 단서를 각각 한 문장으로 기록',
        },
        2,
    ),
];

export const DEMO_KNOWLEDGE_BOOK_DOCUMENTS: RevisionedDto<CreatorKnowledgeBookDocumentDto>[] = [
    revisioned({
        id: 'knowledge-book-archive-demo',
        name: '달빛 서고 세계관',
        entries: [
            {
                id: 'knowledge-silver-strait',
                name: '은빛 해협',
                content:
                    '북쪽 바다에 있는 안개 해협. 새벽에는 나침반이 정확한 방향을 가리키지 않는다.',
                enabled: true,
                activation: {
                    kind: 'keyword',
                    primary: ['은빛 해협', '북쪽 항로'],
                    secondary: ['항해 일지'],
                    selective: false,
                    case_sensitive: false,
                    whole_word: false,
                },
                priority: 90,
                importance: 8,
                placement: 'retrieved_context',
                token_policy: {
                    priority: 90,
                    min_tokens: null,
                    max_tokens: 240,
                    reserve_tokens: null,
                },
                parent_id: null,
                activation_probability_basis_points: 10_000,
            },
        ],
        scan_depth: 8,
        token_budget: { max_tokens: 1_024 },
        recursive: false,
        max_recursion_depth: 0,
    }),
];

export const DEMO_TRANSFORM_SET_DOCUMENTS: RevisionedDto<CreatorTransformSetDocumentDto>[] = [
    revisioned({
        id: 'transform-set-display-demo',
        name: '표시 문장 정리',
        enabled: true,
        rules: [
            {
                id: 'transform-ellipsis-demo',
                name: '말줄임표 통일',
                enabled: true,
                phase: 'display_only',
                order: 10,
                pattern: { pattern: '\\.\\.\\.+', case_insensitive: false },
                replacement: '…',
                condition: null,
                max_replacements: 20,
                input_limit: 65_536,
                output_limit: 65_536,
            },
        ],
        max_rules_per_phase: 64,
        max_output_chars: 65_536,
    }),
];

export const DEMO_INTERACTION_RULE_DOCUMENTS: RevisionedDto<CreatorInteractionRuleSetDocumentDto>[] =
    [
        revisioned({
            id: 'interaction-rules-archive-demo',
            name: '서고 상호작용',
            rules: [],
            max_actions_per_event: 32,
        }),
    ];

export const DEMO_CONTENT_MODULE_DOCUMENTS: RevisionedDto<CreatorContentModuleDocumentDto>[] = [
    revisioned({
        id: 'module-atmosphere-demo',
        name: '서고 분위기 묘사',
        version: '1.0.0',
        prompt_fragments: [],
        knowledge_book_ids: ['knowledge-book-archive-demo'],
        control_specs: [],
        transform_set_ids: ['transform-set-display-demo'],
        interaction_rule_set_ids: ['interaction-rules-archive-demo'],
        asset_ids: [],
        required_capabilities: ['prompt_fragments', 'knowledge', 'transforms'],
        metadata: {
            author: 'LorePia Demo',
            license: 'LicenseRef-Demo-Only',
            redistribution_allowed: false,
            homepage: null,
            description: '달빛 서고 장면을 점검하기 위한 로컬 데모 모듈입니다.',
            tags: ['서고', '분위기', '데모'],
        },
    }),
];

export function demoConversationState(conversationId: string): ConversationStateDto {
    const branch = DEMO_BRANCHES.find((candidate) => candidate.conversation_id === conversationId);
    if (!branch) throw new Error(`Unknown demo conversation: ${conversationId}`);
    return {
        conversation_id: conversationId,
        active_branch_id: branch.id,
        selected_mode: 'chat',
        updated_at: branch.updated_at,
    };
}

export function createDemoOrchestrationWorkspace(
    conversationId: string,
    branchId: string,
): OrchestrationWorkspaceDto {
    const base = emptyOrchestrationWorkspace(conversationId, branchId);
    const branch = DEMO_BRANCHES.find((candidate) => candidate.id === branchId);
    return {
        ...base,
        expected_head: branch?.head_message_id ?? null,
        room_config_revision: 5,
        prompt_preset_revision: 3,
        interaction_state_revision: 2,
        generation_target: {
            model_route_id: 'route-openai-demo',
            generation_preset_id: 'preset-balanced-demo',
        },
        prompt_presets: [
            {
                id: 'prompt-story-demo',
                name: '몰입형 대화',
                schema_version: 1,
                block_count: 4,
                default_generation_preset_id: 'preset-balanced-demo',
            },
        ],
        room_config: {
            ...base.room_config,
            prompt_preset_id: 'prompt-story-demo',
            generation_preset_id: 'preset-balanced-demo',
            response_length: 'balanced',
            creativity: 0.65,
            reasoning_effort: 'medium',
            memory_enabled: true,
            knowledge_enabled: true,
            creator_values: { tone: '따뜻함', scene_detail: 70 },
            user_name_override: '여행자',
            author_note: '비밀을 한 번에 밝히지 말고 단서로 나누어 제시한다.',
            group_context: null,
            template_slots: [{ name: 'current_goal', value: '사라진 북쪽 항로 조사' }],
        },
        prompt_blocks: [
            {
                id: 'block-character-demo',
                name: '캐릭터 정체성',
                kind: 'character_identity',
                enabled: true,
                order_editable: false,
                role_hint: 'system',
                placement_zone: 'B. 캐릭터 문맥',
                template_preview: '아리아의 성격과 말투를 유지합니다.',
                condition_summary: '항상',
                source_label: '캐릭터 카드',
                provenance_label: 'local',
                priority: 100,
                minimum_tokens: 32,
                maximum_tokens: 512,
                overflow_policy: 'trim_tail',
                cache_boundary_after: true,
            },
            {
                id: 'block-knowledge-demo',
                name: '선택된 세계관 지식',
                kind: 'world_knowledge',
                enabled: true,
                order_editable: true,
                role_hint: 'system',
                placement_zone: 'C. 검색 문맥',
                template_preview: null,
                condition_summary: '관련 키워드가 있을 때',
                source_label: '달빛 서고 세계관',
                provenance_label: 'user-created',
                priority: 90,
                minimum_tokens: null,
                maximum_tokens: 1_024,
                overflow_policy: 'reduce_knowledge_entries',
                cache_boundary_after: false,
            },
            {
                id: 'block-memory-demo',
                name: '관련 장기기억',
                kind: 'retrieved_memory',
                enabled: true,
                order_editable: true,
                role_hint: 'system',
                placement_zone: 'D. 기억',
                template_preview: null,
                condition_summary: '기억 사용이 켜져 있을 때',
                source_label: '현재 대화 기억',
                provenance_label: 'conversation',
                priority: 85,
                minimum_tokens: null,
                maximum_tokens: 768,
                overflow_policy: 'reduce_knowledge_entries',
                cache_boundary_after: false,
            },
            {
                id: 'block-history-demo',
                name: '최근 대화',
                kind: 'history_slice',
                enabled: true,
                order_editable: true,
                role_hint: 'user',
                placement_zone: 'G. 최근 대화',
                template_preview: null,
                condition_summary: '최근 12턴',
                source_label: '현재 분기',
                provenance_label: 'conversation',
                priority: 80,
                minimum_tokens: null,
                maximum_tokens: 2_048,
                overflow_policy: 'keep_latest_items',
                cache_boundary_after: false,
            },
        ],
        creator_controls: [
            {
                id: 'tone',
                label: '말투',
                description: '응답의 기본 분위기',
                kind: 'select',
                value: '따뜻함',
                choices: ['따뜻함', '담담함', '긴장감'],
                minimum: null,
                maximum: null,
                step: null,
            },
            {
                id: 'scene_detail',
                label: '장면 묘사',
                description: '공간과 행동을 묘사하는 정도',
                kind: 'slider',
                value: 70,
                choices: [],
                minimum: 0,
                maximum: 100,
                step: 10,
            },
        ],
        knowledge_book_ids: ['knowledge-book-archive-demo'],
        task_profiles: [
            {
                id: 'task-memory-summary-demo',
                name: '기억 요약',
                task_kind: 'memory_summary',
                model_route_id: 'route-openai-demo',
                generation_preset_id: 'preset-balanced-demo',
                fallback_route_ids: [],
                embedding_dimensions: null,
                timeout_seconds: 30,
                concurrency_limit: 1,
            },
        ],
        memory_records: [
            {
                id: 'memory-logbook-demo',
                conversation_id: conversationId,
                branch_id: branchId,
                kind: 'unresolved_thread',
                title: '위치가 바뀌는 섬',
                summary: '표류자의 수기에 새벽마다 위치가 바뀌는 섬이 기록되어 있다.',
                importance: 9,
                keywords: ['표류자', '섬', '북쪽 항로'],
                pinned: true,
                excluded_from_conversation: false,
                excluded_from_character: false,
                source_navigation: {
                    conversation_id: conversationId,
                    branch_id: branchId,
                    start_message_id: 'message-archive-4',
                    end_message_id: 'message-archive-5',
                },
                invalidated_at: null,
                updated_at: UPDATED_AT,
                revision: 3,
            },
            {
                id: 'memory-user-demo',
                conversation_id: conversationId,
                branch_id: branchId,
                kind: 'user_preference',
                title: '조사 방식',
                summary: '사용자는 단서를 직접 비교하며 천천히 결론을 내리는 방식을 선호한다.',
                importance: 6,
                keywords: ['조사', '단서'],
                pinned: false,
                excluded_from_conversation: false,
                excluded_from_character: false,
                source_navigation: {
                    conversation_id: conversationId,
                    branch_id: branchId,
                    start_message_id: 'message-archive-2',
                    end_message_id: 'message-archive-4',
                },
                invalidated_at: null,
                updated_at: UPDATED_AT,
                revision: 1,
            },
        ],
        selection_evidence: [
            {
                id: 'evidence-silver-strait',
                source_kind: 'knowledge',
                title: '은빛 해협',
                selected: true,
                reason: '북쪽 항로 키워드 일치',
                score: 0.94,
                estimated_tokens: 74,
                placement: 'retrieved_context',
            },
            {
                id: 'evidence-moving-island',
                source_kind: 'memory',
                title: '위치가 바뀌는 섬',
                selected: true,
                reason: '최근 대화의 핵심 미해결 단서',
                score: 0.88,
                estimated_tokens: 46,
                placement: 'retrieved_context',
            },
        ],
        interaction_state: [
            { id: 'trust', label: '신뢰도', value: 62, scope: 'conversation' },
            { id: 'clue_count', label: '발견한 단서', value: 3, scope: 'branch' },
        ],
        content_modules: [
            {
                id: 'module-atmosphere-demo',
                name: '서고 분위기 묘사',
                version: '1.0.0',
                source_label: '로컬 데모',
                license_label: 'Demo only',
                redistribution_status: 'unknown',
                conflicts: [],
                required_capabilities: ['prompt_fragments', 'knowledge', 'transforms'],
                components: [
                    {
                        id: 'module-atmosphere-prompt',
                        kind: 'prompt',
                        name: '공간 묘사 블록',
                        selected: true,
                        enabled: true,
                    },
                ],
                active_revision: 1,
                available_revision: 1,
                revision: 1,
                state_revision: 1,
                merge_review_sha256: null,
                merge_plan_sha256: null,
            },
        ],
    };
}
