import type {
    CharacterDto,
    CharacterGreetingCatalogDto,
    ChatEventDto,
    ChatStreamItemDto,
    ConversationBranchDto,
    ConversationDto,
    ConversationStateDto,
    LorepiaClient,
} from '../../lib/ipc/contracts';

export function createAppControllerFixture() {
    const character: CharacterDto = {
        id: 'character-1',
        name: '라온',
        description: '',
        source_hash: 'synthetic',
        avatar_asset_id: null,
        created_at: '2026-08-02T00:00:00Z',
    };

    const conversation: ConversationDto = {
        id: 'conversation-1',
        character_id: character.id,
        title: '첫 대화',
        created_at: '2026-08-02T00:00:00Z',
        updated_at: '2026-08-02T00:00:00Z',
    };

    const greetingCatalog: CharacterGreetingCatalogDto = {
        character_id: character.id,
        character_content_revision_id: 'character-revision-7',
        greetings: [
            { id: 'alternate-first', kind: 'alternate', enabled: true },
            { id: 'default-disabled', kind: 'default', enabled: false },
            { id: 'default-enabled', kind: 'default', enabled: true },
            { id: 'alternate-second', kind: 'alternate', enabled: true },
        ],
    };

    const conversationState: ConversationStateDto = {
        conversation_id: conversation.id,
        active_branch_id: 'branch-1',
        selected_mode: 'chat',
        updated_at: '2026-08-02T00:00:00Z',
    };

    const branch: ConversationBranchDto = {
        id: conversationState.active_branch_id,
        conversation_id: conversation.id,
        title: null,
        fork_message_id: null,
        head_message_id: null,
        created_at: '2026-08-02T00:00:00Z',
        updated_at: '2026-08-02T00:00:00Z',
    };

    function mockClient(overrides: Partial<LorepiaClient>): LorepiaClient {
        const defaults: Partial<LorepiaClient> = {
            bootstrapSnapshot: () =>
                Promise.resolve({
                    shell_api_version: 3,
                    core_api_version: 10,
                    chat_event_version: 4,
                    health: {
                        core_version: '0.1.0',
                        database_open: true,
                        schema_version: 1,
                        data_root_writable: true,
                        staging_writable: true,
                        recovery_pending: false,
                        active_jobs: 0,
                    },
                }),
            getMemorySupervisorStatus: () =>
                Promise.resolve({
                    sequence: 1,
                    phase: 'running',
                    recovered_interrupted_jobs: 0,
                    completed_jobs: 0,
                }),
            subscribeMemorySupervisorStatus: () => Promise.resolve(() => undefined),
            listCharacters: () => Promise.resolve([character]),
            getProviderOverview: () =>
                Promise.resolve({
                    templates: [],
                    connections: [],
                    legacy_profiles: [],
                    settings: {
                        preserve_partial_generations: true,
                        selected_provider_profile_id: null,
                        selected_model_route_id: 'route-1',
                        selected_generation_preset_id: 'preset-1',
                    },
                }),
            listProviderDiscoveries: () => Promise.resolve([]),
            providerCatalogStatus: () =>
                Promise.resolve({
                    status_schema_version: 1,
                    state_version: 1,
                    active_revision: 1,
                    active_snapshot_sha256: 'synthetic-active',
                    bundled_baseline_sha256: 'synthetic-baseline',
                    snapshot_count: 1,
                    signed_update_count: 0,
                    highest_accepted_revision: 1,
                    latest_issued_at: null,
                    active_signed_revisions: [],
                }),
            providerCatalogHistory: () =>
                Promise.resolve({
                    history_schema_version: 1,
                    active_revision: 1,
                    revisions: [],
                    activations: [],
                    next_before_revision: null,
                    next_before_state_version: null,
                }),
            listConversations: () => Promise.resolve([conversation]),
            getCharacterGreetingCatalog: () => Promise.resolve(greetingCatalog),
            openExistingConversation: () => Promise.resolve(conversation),
            getConversationState: () => Promise.resolve(conversationState),
            listBranches: () => Promise.resolve([branch]),
            listBranchMessages: () => Promise.resolve([]),
            listRetryableMemoryQueryEmbeddings: () => Promise.resolve([]),
            listInterruptedMemoryJobs: () => Promise.resolve([]),
            disposeChatStream: () => Promise.resolve(false),
        };
        return new Proxy({ ...defaults, ...overrides } as LorepiaClient, {
            get(target, property, receiver) {
                const value = Reflect.get(target, property, receiver) as unknown;
                if (typeof value === 'function') return value;
                throw new Error(`Unexpected client method: ${String(property)}`);
            },
        });
    }

    function textEvent(sequence: number, payload = '늦게 도착한 조각'): ChatStreamItemDto {
        const event: ChatEventDto = {
            event_version: 4,
            generation_id: 'generation-1',
            conversation_id: conversation.id,
            branch_id: branch.id,
            assistant_message_id: 'message-1',
            sequence,
            emitted_at: '2026-08-02T00:00:00Z',
            kind: { type: 'text_delta', payload },
        };
        return { type: 'event', payload: event };
    }

    function reasoningEvent(sequence: number, payload = '마지막 추론 조각'): ChatStreamItemDto {
        return {
            type: 'event',
            payload: {
                event_version: 4,
                generation_id: 'generation-1',
                conversation_id: conversation.id,
                branch_id: branch.id,
                assistant_message_id: 'message-1',
                sequence,
                emitted_at: '2026-08-02T00:00:00Z',
                kind: { type: 'reasoning_delta', payload },
            },
        };
    }

    function terminalEvent(sequence: number): ChatStreamItemDto {
        return {
            type: 'event',
            payload: {
                event_version: 4,
                generation_id: 'generation-1',
                conversation_id: conversation.id,
                branch_id: branch.id,
                assistant_message_id: 'message-1',
                sequence,
                emitted_at: '2026-08-02T00:00:00Z',
                kind: { type: 'generation_finished' },
            },
        };
    }

    return {
        character,
        conversation,
        greetingCatalog,
        conversationState,
        branch,
        mockClient,
        textEvent,
        reasoningEvent,
        terminalEvent,
    };
}

export function deferred<T>(): {
    promise: Promise<T>;
    resolve: (value: T) => void;
    reject: (reason: unknown) => void;
} {
    let resolve!: (value: T) => void;
    let reject!: (reason: unknown) => void;
    const promise = new Promise<T>((nextResolve, nextReject) => {
        resolve = nextResolve;
        reject = nextReject;
    });
    return { promise, resolve, reject };
}
