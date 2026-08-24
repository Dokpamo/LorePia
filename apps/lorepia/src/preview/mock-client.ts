/**
 * Stateful, deterministic client for the browser-only LorePia demo.
 *
 * It never calls Tauri IPC, a credential store, a database, or the host file
 * system. Every client begins with a fresh clone of `demo-data.ts`.
 */

import type {
    ChatEventDto,
    ChatStreamItemDto,
    ConversationBranchDto,
    ConversationMode,
    ConversationStateDto,
    CredentialStatus,
    GenerationTargetDto,
    LorepiaClient,
    OrchestrationClientApi,
    OrchestrationDocumentClientApi,
    ProviderOverviewDto,
    RevisionedDto,
    SendMessageInput,
} from '../lib/ipc/contracts';
import type {
    ConversationPersonaSelectionDto,
    PersonaClientApi,
    PersonaDto,
} from '../features/personas/persona-contracts';
import {
    DEMO_BOOTSTRAP,
    DEMO_BRANCHES,
    DEMO_CATALOG_HISTORY,
    DEMO_CATALOG_STATUS,
    DEMO_CHARACTERS,
    DEMO_CONTENT_MODULE_DOCUMENTS,
    DEMO_CONVERSATIONS,
    DEMO_EDITABLE_PROMPT_PRESET,
    DEMO_GENERATION_PRESETS,
    DEMO_GREETINGS,
    DEMO_INTERACTION_RULE_DOCUMENTS,
    DEMO_KNOWLEDGE_BOOK_DOCUMENTS,
    DEMO_MEMORY_PROFILE_DOCUMENTS,
    DEMO_MEMORY_SUPERVISOR,
    DEMO_MESSAGES,
    DEMO_MODEL_ROUTES,
    DEMO_PERSONAS,
    DEMO_PROVIDER_CONNECTIONS,
    DEMO_PROVIDER_TEMPLATES,
    DEMO_SETTINGS,
    DEMO_TASK_PROFILE_DOCUMENTS,
    DEMO_TRANSFORM_SET_DOCUMENTS,
    createDemoOrchestrationWorkspace,
    demoConversationState,
} from './demo-data';

export type PreviewClient = LorepiaClient &
    PersonaClientApi &
    Partial<OrchestrationClientApi & OrchestrationDocumentClientApi>;

const DEMO_CATALOG_REVISION = 'e'.repeat(64);
const DEMO_NOW = '2026-08-24T13:00:00.000Z';

function clone<Value>(value: Value): Value {
    return structuredClone(value);
}

function selectedPersonaSnapshot(persona: PersonaDto | null) {
    return persona === null
        ? null
        : {
              value: clone(persona.value),
              revision: persona.revision,
              revision_id: persona.revision_id,
              snapshot_created_at: persona.updated_at,
          };
}

export function createPreviewClient(): PreviewClient {
    const characters = clone(DEMO_CHARACTERS);
    let conversations = clone(DEMO_CONVERSATIONS);
    let branches = clone(DEMO_BRANCHES);
    const messages = clone(DEMO_MESSAGES);
    let personas = clone(DEMO_PERSONAS);
    let settings = clone(DEMO_SETTINGS);
    const connections = clone(DEMO_PROVIDER_CONNECTIONS);
    const routes = clone(DEMO_MODEL_ROUTES);
    const presets = clone(DEMO_GENERATION_PRESETS);
    const selectedBranchIds = new Map<string, string>();
    const conversationModes = new Map<string, ConversationMode>();
    const personaSelections = new Map<string, { personaId: string | null; revision: number }>();
    const credentialStatuses = new Map<string, CredentialStatus>(
        connections.map((connection) => [connection.id, 'available']),
    );
    const branchMessageIds = new Map<string, string[]>([
        [
            'branch-archive-main',
            [
                'message-archive-1',
                'message-archive-2',
                'message-archive-3',
                'message-archive-4',
                'message-archive-5',
                'message-archive-6',
            ],
        ],
        [
            'branch-archive-map',
            [
                'message-archive-1',
                'message-archive-2',
                'message-archive-3',
                'message-map-1',
                'message-map-2',
            ],
        ],
        ['branch-rain-main', ['message-rain-1', 'message-rain-2']],
        ['branch-workshop-main', ['message-workshop-1', 'message-workshop-2']],
        ['branch-stars-main', ['message-stars-1', 'message-stars-2']],
    ]);
    let nextConversation = 1;
    let nextBranch = 1;
    let nextMessage = 1;
    let nextPersona = 1;
    let nextGeneration = 1;

    for (const conversation of conversations) {
        selectedBranchIds.set(
            conversation.id,
            demoConversationState(conversation.id).active_branch_id,
        );
        conversationModes.set(conversation.id, 'chat');
    }

    function characterById(characterId: string) {
        const character = characters.find((candidate) => candidate.id === characterId);
        if (!character) throw new Error(`Unknown demo character: ${characterId}`);
        return character;
    }

    function conversationById(conversationId: string) {
        const conversation = conversations.find((candidate) => candidate.id === conversationId);
        if (!conversation) throw new Error(`Unknown demo conversation: ${conversationId}`);
        return conversation;
    }

    function branchById(branchId: string) {
        const branch = branches.find((candidate) => candidate.id === branchId);
        if (!branch) throw new Error(`Unknown demo branch: ${branchId}`);
        return branch;
    }

    function conversationState(conversationId: string): ConversationStateDto {
        const branchId = selectedBranchIds.get(conversationId);
        if (!branchId) throw new Error(`Demo conversation has no branch: ${conversationId}`);
        return {
            conversation_id: conversationId,
            active_branch_id: branchId,
            selected_mode: conversationModes.get(conversationId) ?? 'chat',
            updated_at: branchById(branchId).updated_at,
        };
    }

    function messagesForBranch(branchId: string) {
        const ids = new Set(branchMessageIds.get(branchId) ?? []);
        return messages.filter((message) => ids.has(message.id));
    }

    function personaSelection(conversationId: string): ConversationPersonaSelectionDto {
        const current = personaSelections.get(conversationId);
        const selected =
            current?.personaId === null || current?.personaId === undefined
                ? null
                : (personas.find((persona) => persona.value.id === current.personaId) ?? null);
        return {
            conversation_id: conversationId,
            state_revision: current?.revision ?? null,
            selected_persona: selectedPersonaSnapshot(selected),
            updated_at: current === undefined ? null : DEMO_NOW,
            cleared_at: current !== undefined && selected === null ? DEMO_NOW : null,
        };
    }

    function providerOverview(): ProviderOverviewDto {
        return {
            settings: clone(settings),
            templates: clone(DEMO_PROVIDER_TEMPLATES),
            connections: clone(connections),
            legacy_profiles: [],
        };
    }

    function revisioned<Value>(value: Value, revision: number): RevisionedDto<Value> {
        return {
            value: clone(value),
            revision,
            created_at: DEMO_NOW,
            updated_at: DEMO_NOW,
            deleted_at: null,
        };
    }

    function createConversation(characterId: string, title: string, mode: ConversationMode) {
        characterById(characterId);
        const suffix = String(nextConversation++);
        const conversation = {
            id: `conversation-demo-${suffix}`,
            character_id: characterId,
            title,
            created_at: DEMO_NOW,
            updated_at: DEMO_NOW,
        };
        const branch: ConversationBranchDto = {
            id: `branch-demo-${suffix}`,
            conversation_id: conversation.id,
            title: null,
            fork_message_id: null,
            head_message_id: null,
            created_at: DEMO_NOW,
            updated_at: DEMO_NOW,
        };
        conversations = [conversation, ...conversations];
        branches = [branch, ...branches];
        selectedBranchIds.set(conversation.id, branch.id);
        conversationModes.set(conversation.id, mode);
        branchMessageIds.set(branch.id, []);
        return conversation;
    }

    function emitDemoGeneration(
        input: SendMessageInput,
        onItem: (item: ChatStreamItemDto) => void,
    ): { generation_id: string } {
        const branch = branchById(input.branch_id);
        const generationId = `generation-demo-${String(nextGeneration++)}`;
        const userMessageId = `message-demo-user-${String(nextMessage++)}`;
        const assistantMessageId = `message-demo-assistant-${String(nextMessage++)}`;
        const reply =
            '좋아요. 이 대화는 데모 모드에서 로컬로만 추가됐어요. 새로고침하면 원래 테스트 데이터로 돌아갑니다.';

        messages.push(
            {
                id: userMessageId,
                conversation_id: input.conversation_id,
                parent_id: input.expected_head,
                role: 'user',
                content: input.text,
                status: 'complete',
                generation_id: null,
                created_at: DEMO_NOW,
            },
            {
                id: assistantMessageId,
                conversation_id: input.conversation_id,
                parent_id: userMessageId,
                role: 'assistant',
                content: reply,
                status: 'complete',
                generation_id: generationId,
                created_at: DEMO_NOW,
            },
        );
        branchMessageIds.set(branch.id, [
            ...(branchMessageIds.get(branch.id) ?? []),
            userMessageId,
            assistantMessageId,
        ]);
        branch.head_message_id = assistantMessageId;
        branch.updated_at = DEMO_NOW;
        conversationById(input.conversation_id).updated_at = DEMO_NOW;

        const event = (sequence: number, kind: ChatEventDto['kind']): ChatStreamItemDto => ({
            type: 'event',
            payload: {
                event_version: 4,
                generation_id: generationId,
                conversation_id: input.conversation_id,
                branch_id: input.branch_id,
                assistant_message_id: assistantMessageId,
                sequence,
                emitted_at: DEMO_NOW,
                kind,
            },
        });
        onItem(event(1, { type: 'generation_started' }));
        onItem(event(2, { type: 'text_delta', payload: reply }));
        onItem(
            event(3, {
                type: 'message_committed',
                payload: { message_id: assistantMessageId, status: 'complete' },
            }),
        );
        onItem(event(4, { type: 'generation_finished' }));
        return { generation_id: generationId };
    }

    const client: Partial<PreviewClient> = {
        bootstrapSnapshot: () => Promise.resolve(clone(DEMO_BOOTSTRAP)),
        getMemorySupervisorStatus: () => Promise.resolve(clone(DEMO_MEMORY_SUPERVISOR)),
        subscribeMemorySupervisorStatus: (onStatus) => {
            queueMicrotask(() => onStatus(clone(DEMO_MEMORY_SUPERVISOR)));
            return Promise.resolve(() => undefined);
        },

        listCharacters: () => Promise.resolve(clone(characters)),
        getCharacter: (characterId) => Promise.resolve(clone(characterById(characterId))),
        getCharacterGreetingCatalog: (characterId) => {
            const catalog = DEMO_GREETINGS[characterId];
            if (!catalog) throw new Error(`Unknown demo greeting catalog: ${characterId}`);
            return Promise.resolve(clone(catalog));
        },
        selectImportSource: () => Promise.resolve(null),
        discardImport: () => Promise.resolve(),

        listConversations: (characterId) =>
            Promise.resolve(
                clone(
                    conversations
                        .filter(
                            (conversation) =>
                                characterId === null || conversation.character_id === characterId,
                        )
                        .sort((left, right) => right.updated_at.localeCompare(left.updated_at)),
                ),
            ),
        createConversation: (characterId, title, mode) =>
            Promise.resolve(clone(createConversation(characterId, title, mode))),
        openConversation: (characterId) => {
            const existing = conversations.find(
                (conversation) => conversation.character_id === characterId,
            );
            return Promise.resolve(
                clone(
                    existing ??
                        createConversation(characterId, characterById(characterId).name, 'chat'),
                ),
            );
        },
        openExistingConversation: (conversationId) =>
            Promise.resolve(clone(conversationById(conversationId))),
        getConversation: (conversationId) =>
            Promise.resolve(clone(conversationById(conversationId))),
        getConversationState: (conversationId) =>
            Promise.resolve(clone(conversationState(conversationId))),
        listBranches: (conversationId) =>
            Promise.resolve(
                clone(branches.filter((branch) => branch.conversation_id === conversationId)),
            ),
        createBranch: (conversationId, fromMessageId, title) => {
            conversationById(conversationId);
            const activeBranchId = selectedBranchIds.get(conversationId);
            const sourceIds = activeBranchId ? (branchMessageIds.get(activeBranchId) ?? []) : [];
            const stopAt =
                fromMessageId === null ? sourceIds.length : sourceIds.indexOf(fromMessageId) + 1;
            const inheritedIds = stopAt <= 0 ? [] : sourceIds.slice(0, stopAt);
            const branch: ConversationBranchDto = {
                id: `branch-user-${String(nextBranch++)}`,
                conversation_id: conversationId,
                title,
                fork_message_id: fromMessageId,
                head_message_id: inheritedIds.at(-1) ?? null,
                created_at: DEMO_NOW,
                updated_at: DEMO_NOW,
            };
            branches = [branch, ...branches];
            branchMessageIds.set(branch.id, inheritedIds);
            return Promise.resolve(clone(branch));
        },
        selectBranch: (conversationId, branchId) => {
            const branch = branchById(branchId);
            if (branch.conversation_id !== conversationId) throw new Error('Demo branch mismatch.');
            selectedBranchIds.set(conversationId, branchId);
            return Promise.resolve(clone(conversationState(conversationId)));
        },
        setConversationMode: (conversationId, mode) => {
            conversationModes.set(conversationId, mode);
            return Promise.resolve(clone(conversationState(conversationId)));
        },
        listBranchMessages: (branchId) => Promise.resolve(clone(messagesForBranch(branchId))),
        listMessages: (conversationId) =>
            Promise.resolve(
                clone(messages.filter((message) => message.conversation_id === conversationId)),
            ),
        listInterruptedMemoryJobs: () => Promise.resolve([]),
        listRetryableMemoryQueryEmbeddings: () => Promise.resolve([]),

        sendMessage: (input, _streamId, onItem) =>
            Promise.resolve(emitDemoGeneration(input, onItem)),
        subscribeGeneration: () => Promise.resolve(),
        disposeChatStream: () => Promise.resolve(true),
        cancelGeneration: () => Promise.resolve(),

        getProviderOverview: () => Promise.resolve(clone(providerOverview())),
        getSettings: () => Promise.resolve(clone(settings)),
        updateSettings: (nextSettings) => {
            settings = clone(nextSettings);
            return Promise.resolve(clone(settings));
        },
        selectGenerationTarget: (target: GenerationTargetDto | null) => {
            settings = {
                ...settings,
                selected_provider_profile_id: null,
                selected_model_route_id: target?.model_route_id ?? null,
                selected_generation_preset_id: target?.generation_preset_id ?? null,
            };
            return Promise.resolve(clone(settings));
        },
        listProviderTemplates: () => Promise.resolve(clone(DEMO_PROVIDER_TEMPLATES)),
        listProviderConnections: () => Promise.resolve(clone(connections)),
        listModelRoutes: (connectionId) =>
            Promise.resolve(clone(routes.filter((route) => route.connection_id === connectionId))),
        listGenerationPresets: (routeId) =>
            Promise.resolve(clone(presets.filter((preset) => preset.model_route_id === routeId))),
        listProviderDiscoveries: () => Promise.resolve([]),
        listProviderModelSyncs: () => Promise.resolve([]),
        credentialStatus: (target) => {
            const status =
                target.kind === 'connection'
                    ? (credentialStatuses.get(target.connection_id) ?? 'missing')
                    : 'missing';
            return Promise.resolve({ status });
        },
        captureCredential: (target) => {
            if (target.kind === 'connection') {
                credentialStatuses.set(target.connection_id, 'available');
            }
            return Promise.resolve({ clipboard_cleanup: 'cleared' });
        },
        deleteCredential: (target) => {
            if (target.kind === 'connection') {
                credentialStatuses.set(target.connection_id, 'missing');
            }
            return Promise.resolve();
        },
        providerCatalogStatus: () => Promise.resolve(clone(DEMO_CATALOG_STATUS)),
        providerCatalogHistory: () => Promise.resolve(clone(DEMO_CATALOG_HISTORY)),

        getOrchestrationWorkspace: (conversationId, branchId) =>
            Promise.resolve(clone(createDemoOrchestrationWorkspace(conversationId, branchId))),
        saveRoomOrchestrationConfig: (input) => {
            const workspace = createDemoOrchestrationWorkspace(
                input.conversation_id,
                input.branch_id,
            );
            return Promise.resolve({
                room_config: {
                    ...workspace.room_config,
                    ...clone(input),
                },
                revision: (input.expected_revision ?? 0) + 1,
                generation_target: workspace.generation_target,
            });
        },
        getEditablePromptPreset: () => Promise.resolve(clone(DEMO_EDITABLE_PROMPT_PRESET)),
        listTaskProfiles: () => Promise.resolve(clone(DEMO_TASK_PROFILE_DOCUMENTS)),
        listMemoryProfiles: () => Promise.resolve(clone(DEMO_MEMORY_PROFILE_DOCUMENTS)),
        listKnowledgeBooks: () => Promise.resolve(clone(DEMO_KNOWLEDGE_BOOK_DOCUMENTS)),
        listTransformSets: () => Promise.resolve(clone(DEMO_TRANSFORM_SET_DOCUMENTS)),
        listInteractionRuleSets: () => Promise.resolve(clone(DEMO_INTERACTION_RULE_DOCUMENTS)),
        listContentModules: () => Promise.resolve(clone(DEMO_CONTENT_MODULE_DOCUMENTS)),
        upsertPromptPreset: (input) =>
            Promise.resolve(
                revisioned(
                    {
                        id: input.value.id,
                        name: input.value.name,
                        schema_version: input.value.schema_version,
                        block_count: input.value.blocks.length,
                        default_generation_preset_id: input.value.default_generation_preset_id,
                    },
                    (input.expected_revision ?? 0) + 1,
                ),
            ),
        upsertTaskProfile: (input) =>
            Promise.resolve(revisioned(input.value, (input.expected_revision ?? 0) + 1)),
        upsertMemoryProfile: (input) =>
            Promise.resolve(revisioned(input.value, (input.expected_revision ?? 0) + 1)),
        upsertKnowledgeBook: (input) =>
            Promise.resolve(revisioned(input.value, (input.expected_revision ?? 0) + 1)),
        upsertTransformSet: (input) =>
            Promise.resolve(revisioned(input.value, (input.expected_revision ?? 0) + 1)),
        upsertInteractionRuleSet: (input) =>
            Promise.resolve(revisioned(input.value, (input.expected_revision ?? 0) + 1)),
        upsertContentModule: (input) =>
            Promise.resolve(revisioned(input.value, (input.expected_revision ?? 0) + 1)),

        createPersona: (input) => {
            const suffix = String(nextPersona++);
            const persona: PersonaDto = {
                value: {
                    id: `persona-demo-${suffix}`,
                    name: input.name,
                    description: input.description,
                },
                revision: 1,
                revision_id: `persona-demo-${suffix}-revision-1`,
                created_at: DEMO_NOW,
                updated_at: DEMO_NOW,
            };
            personas = [persona, ...personas];
            return Promise.resolve(clone(persona));
        },
        updatePersona: (input) => {
            const index = personas.findIndex((persona) => persona.value.id === input.persona_id);
            const current = personas[index];
            if (!current) throw new Error(`Unknown demo persona: ${input.persona_id}`);
            const revision = current.revision + 1;
            const updated: PersonaDto = {
                ...current,
                value: {
                    id: current.value.id,
                    name: input.name,
                    description: input.description,
                },
                revision,
                revision_id: `${current.value.id}-revision-${String(revision)}`,
                updated_at: DEMO_NOW,
            };
            personas[index] = updated;
            return Promise.resolve(clone(updated));
        },
        getPersona: (input) => {
            const persona = personas.find((candidate) => candidate.value.id === input.persona_id);
            if (!persona) throw new Error(`Unknown demo persona: ${input.persona_id}`);
            return Promise.resolve(clone(persona));
        },
        listPersonas: () => Promise.resolve(clone(personas)),
        listPersonaPage: () =>
            Promise.resolve({
                kind: 'page' as const,
                catalog_revision: DEMO_CATALOG_REVISION,
                items: clone(personas),
                next_cursor: null,
            }),
        deletePersona: (input) => {
            const index = personas.findIndex((persona) => persona.value.id === input.persona_id);
            if (index < 0) throw new Error(`Unknown demo persona: ${input.persona_id}`);
            personas.splice(index, 1);
            for (const [conversationId, selection] of personaSelections) {
                if (selection.personaId === input.persona_id) {
                    personaSelections.set(conversationId, {
                        personaId: null,
                        revision: selection.revision + 1,
                    });
                }
            }
            return Promise.resolve({
                persona_id: input.persona_id,
                revision: input.expected_revision + 1,
                deleted_at: DEMO_NOW,
            });
        },
        getConversationPersonaSelection: (input) =>
            Promise.resolve(clone(personaSelection(input.conversation_id))),
        selectConversationPersona: (input) => {
            if (!personas.some((persona) => persona.value.id === input.persona_id)) {
                throw new Error(`Unknown demo persona: ${input.persona_id}`);
            }
            personaSelections.set(input.conversation_id, {
                personaId: input.persona_id,
                revision: (input.expected_state_revision ?? 0) + 1,
            });
            return Promise.resolve(clone(personaSelection(input.conversation_id)));
        },
        clearConversationPersona: (input) => {
            personaSelections.set(input.conversation_id, {
                personaId: null,
                revision: input.expected_state_revision + 1,
            });
            return Promise.resolve(clone(personaSelection(input.conversation_id)));
        },
    };

    return client as PreviewClient;
}
