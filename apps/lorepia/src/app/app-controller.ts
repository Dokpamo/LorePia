import { get, writable, type Readable } from 'svelte/store';

import {
    SUPPORTED_CHAT_EVENT_VERSION,
    SUPPORTED_CORE_API_VERSION,
    SUPPORTED_SHELL_API_VERSION,
    type AppSettingsDto,
    type BeginProviderDiscoveryCurlInput,
    type BeginProviderDiscoveryInput,
    type BootstrapDto,
    type CapabilityKeyInput,
    type CharacterDto,
    type CharacterGreetingCatalogDto,
    type ChatEventDto,
    type ChatStreamItemDto,
    type ContinueProviderDiscoveryActionInput,
    type ConversationBranchDto,
    type ConversationDto,
    type ConversationMode,
    type ConversationStateDto,
    type CreateProviderConnectionInput,
    type CredentialTargetDto,
    type GenerationPresetInput,
    type DiscoveryAssistantFailureKindInput,
    type DiscoveryAssistantInterruptionOutcomeInput,
    type GenerationPresetDto,
    type GenerationSelectionInput,
    type GenerationTargetDto,
    type ImportInspectionDto,
    type LoadingPhase,
    type LorepiaClient,
    type MessageActionGenerationDto,
    type MessageDto,
    type MemoryQueryEmbeddingRetryCandidateDto,
    type MemorySupervisorStatusDto,
    type ModelRouteDto,
    type ModelSyncJobDto,
    type NativeCaptureStatusDto,
    type ProviderCatalogRollbackPlanDto,
    type ProviderConnectionDto,
    type ProviderDiscoverySessionDto,
    type ProviderProfileDto,
    type ProviderTemplateDto,
    type ProviderWorkspaceDto,
    type ReviewedPromptSendInput,
    type UpdateProviderConnectionInput,
    type UpsertCapabilityOverrideInput,
    type UpsertModelRouteInput,
} from '../lib/ipc/contracts';
import { LorepiaClientError, normalizeClientError } from '../lib/ipc/errors';
import { ChatStreamVerifier } from '../features/chat/chat-stream';

export interface SectionState {
    phase: LoadingPhase;
    error: string | null;
}

export interface ImportFlowState extends SectionState {
    inspection: ImportInspectionDto | null;
}

export interface ChatState extends SectionState {
    active_generation_id: string | null;
    live_assistant_message_id: string | null;
    streaming_text: string;
    reasoning_text: string;
    reconcile_notice: string | null;
    usage_label: string | null;
}

export interface GreetingCatalogState extends SectionState {
    value: CharacterGreetingCatalogDto | null;
    selected_greeting_id: string | null;
}

export interface MemoryQueryRetryState extends SectionState {
    candidates: MemoryQueryEmbeddingRetryCandidateDto[];
    busy_id: string | null;
    notice: string | null;
}

export interface LorepiaAppState {
    bootstrap: SectionState & { value: BootstrapDto | null };
    memory_supervisor: SectionState & { status: MemorySupervisorStatusDto | null };
    library: SectionState & { characters: CharacterDto[] };
    import_flow: ImportFlowState;
    selected_character: CharacterDto | null;
    conversations: SectionState & { items: ConversationDto[] };
    greeting_catalog: GreetingCatalogState;
    selected_conversation: ConversationDto | null;
    conversation_state: ConversationStateDto | null;
    branches: ConversationBranchDto[];
    messages: SectionState & { items: MessageDto[] };
    memory_query_retries: MemoryQueryRetryState;
    chat: ChatState;
    providers: SectionState & { workspace: ProviderWorkspaceDto };
    announcement: string;
}

const EMPTY_SETTINGS: AppSettingsDto = {
    preserve_partial_generations: true,
    selected_provider_profile_id: null,
    selected_model_route_id: null,
    selected_generation_preset_id: null,
};

const EMPTY_PROVIDER_WORKSPACE: ProviderWorkspaceDto = {
    templates: [],
    connections: [],
    legacy_profiles: [],
    routes: [],
    presets: [],
    settings: EMPTY_SETTINGS,
    credential_statuses: {},
    request_preview: null,
    selected_capability_model_route_id: null,
    capability_observations: [],
    capability_parameter_specs: [],
    effective_capability: null,
    model_sync_jobs: [],
    selected_model_sync_job_id: null,
    model_sync_event: null,
    discoveries: [],
    selected_discovery_id: null,
    discovery_candidates: [],
    discovery_evidence: [],
    discovery_approvals: [],
    discovery_review: null,
    discovery_approval_proposal: null,
    discovery_review_proposal: null,
    discovery_assistant_resume_boundary: null,
    discovery_assistant_host_action: null,
    discovery_event: null,
    discovery_compensation_steps: [],
    discovery_recovery_results: [],
    catalog_status: null,
    catalog_history: null,
    pending_catalog_import: null,
    pending_catalog_rollback: null,
    catalog_diff: null,
};

export const INITIAL_APP_STATE: LorepiaAppState = {
    bootstrap: { phase: 'idle', error: null, value: null },
    memory_supervisor: { phase: 'idle', error: null, status: null },
    library: { phase: 'idle', error: null, characters: [] },
    import_flow: { phase: 'idle', error: null, inspection: null },
    selected_character: null,
    conversations: { phase: 'idle', error: null, items: [] },
    greeting_catalog: {
        phase: 'idle',
        error: null,
        value: null,
        selected_greeting_id: null,
    },
    selected_conversation: null,
    conversation_state: null,
    branches: [],
    messages: { phase: 'idle', error: null, items: [] },
    memory_query_retries: {
        phase: 'idle',
        error: null,
        candidates: [],
        busy_id: null,
        notice: null,
    },
    chat: {
        phase: 'idle',
        error: null,
        active_generation_id: null,
        live_assistant_message_id: null,
        streaming_text: '',
        reasoning_text: '',
        reconcile_notice: null,
        usage_label: null,
    },
    providers: {
        phase: 'idle',
        error: null,
        workspace: EMPTY_PROVIDER_WORKSPACE,
    },
    announcement: '',
};

const GENERATION_REATTACHMENT_UNAVAILABLE_MESSAGE =
    '앱을 다시 연 뒤에는 진행 중이던 응답 스트림에 다시 연결할 수 없습니다. 생성 취소 후 대화를 다시 열어 주세요.';

function errorLabel(error: unknown): string {
    const normalized = normalizeClientError(error);
    const fallback: Record<string, string> = {
        'error.unexpected': '예상하지 못한 오류가 발생했습니다.',
        'error.compatibility': '앱과 Core 버전이 호환되지 않습니다.',
        'error.invalid_input': '입력 내용을 확인해 주세요.',
        'error.core_unavailable': '로컬 Core를 열 수 없습니다.',
        'chat.generation_reattachment_unavailable': GENERATION_REATTACHMENT_UNAVAILABLE_MESSAGE,
        'provider.discovery.assistant_pricing_unavailable':
            '신뢰할 수 있는 가격·토큰 정책이 준비될 때까지 원격 설정 도우미를 사용할 수 없습니다.',
    };
    return fallback[normalized.messageKey] ?? normalized.messageKey;
}

const MAX_MEMORY_QUERY_RETRY_CANDIDATES = 16;
const MAX_PROVIDER_DISCOVERY_EVENT_DRAIN = 100;

function isRetryableMemoryQueryCandidate(
    candidate: MemoryQueryEmbeddingRetryCandidateDto,
    conversationId: string,
    branchId: string,
): boolean {
    const retryableStatus =
        candidate.status === 'interrupted' ||
        candidate.status === 'failed' ||
        candidate.status === 'cancelled';
    return (
        typeof candidate.id === 'string' &&
        candidate.id.length > 0 &&
        retryableStatus &&
        Number.isSafeInteger(candidate.revision) &&
        candidate.revision >= 0 &&
        candidate.revision < Number.MAX_SAFE_INTEGER &&
        candidate.conversation_id === conversationId &&
        candidate.branch_id === branchId &&
        (candidate.error_code === null || typeof candidate.error_code === 'string') &&
        candidate.requires_unknown_outcome_acknowledgement === (candidate.status === 'interrupted')
    );
}

function isQueuedMemoryQueryRetryReceipt(
    receipt: MemoryQueryEmbeddingRetryCandidateDto,
    candidate: MemoryQueryEmbeddingRetryCandidateDto,
): boolean {
    return (
        receipt.id === candidate.id &&
        receipt.status === 'queued' &&
        receipt.revision === candidate.revision + 1 &&
        receipt.conversation_id === candidate.conversation_id &&
        receipt.branch_id === candidate.branch_id &&
        receipt.error_code === null &&
        !receipt.requires_unknown_outcome_acknowledgement
    );
}

function firstEnabledGreetingId(catalog: CharacterGreetingCatalogDto): string | null {
    return (
        catalog.greetings.find((greeting) => greeting.enabled && greeting.kind === 'default')?.id ??
        catalog.greetings.find((greeting) => greeting.enabled && greeting.kind === 'alternate')
            ?.id ??
        null
    );
}

function reattachmentUnavailableChatState(generationId: string): ChatState {
    return {
        phase: 'error',
        error: GENERATION_REATTACHMENT_UNAVAILABLE_MESSAGE,
        active_generation_id: generationId,
        live_assistant_message_id: null,
        streaming_text: '',
        reasoning_text: '',
        reconcile_notice: null,
        usage_label: null,
    };
}

function ensureCompatible(snapshot: BootstrapDto): void {
    if (
        snapshot.shell_api_version !== SUPPORTED_SHELL_API_VERSION ||
        snapshot.core_api_version !== SUPPORTED_CORE_API_VERSION ||
        snapshot.chat_event_version !== SUPPORTED_CHAT_EVENT_VERSION
    ) {
        throw new LorepiaClientError({
            code: 'incompatible_version',
            message_key: 'error.compatibility',
            recoverable: false,
            operation_id: null,
            field_errors: [],
        });
    }
}

function isMemorySupervisorStatus(value: unknown): value is MemorySupervisorStatusDto {
    if (typeof value !== 'object' || value === null) return false;
    const candidate = value as Record<string, unknown>;
    const allowedKeys = new Set([
        'sequence',
        'phase',
        'recovered_interrupted_jobs',
        'completed_jobs',
    ]);
    return (
        Object.keys(candidate).every((key) => allowedKeys.has(key)) &&
        Number.isSafeInteger(candidate.sequence) &&
        Number(candidate.sequence) >= 0 &&
        typeof candidate.phase === 'string' &&
        ['not_started', 'recovered', 'running', 'failed'].includes(candidate.phase) &&
        Number.isSafeInteger(candidate.recovered_interrupted_jobs) &&
        Number(candidate.recovered_interrupted_jobs) >= 0 &&
        Number.isSafeInteger(candidate.completed_jobs) &&
        Number(candidate.completed_jobs) >= 0
    );
}

function credentialKey(target: CredentialTargetDto): string {
    switch (target.kind) {
        case 'connection':
            return `connection:${target.connection_id}`;
        case 'legacy_profile':
            return `legacy_profile:${target.provider_profile_id}`;
        case 'discovery_session':
            return `discovery_session:${target.session_id}`;
    }
}

export function discoveryCredentialTarget(
    session: ProviderDiscoverySessionDto,
): Extract<CredentialTargetDto, { kind: 'discovery_session' }> | null {
    if (!session.credential_binding_requested) return null;
    const eligible =
        session.state === 'awaiting_credential_origin_approval' ||
        session.state === 'awaiting_probe_consent' ||
        session.state === 'awaiting_review' ||
        session.state === 'committing' ||
        (session.state === 'interrupted' &&
            (session.recovery_operation === 'list_models' ||
                session.recovery_operation === 'probe_capabilities'));
    return eligible
        ? {
              kind: 'discovery_session',
              session_id: session.id,
              expected_revision: session.revision,
          }
        : null;
}

function captureAnnouncement(status: NativeCaptureStatusDto, success: string): string {
    switch (status.clipboard_cleanup) {
        case 'cleared':
            return success;
        case 'already_replaced':
            return `${success} 캡처 중 클립보드 내용이 바뀌어 새 내용은 지우지 않았습니다.`;
        case 'clear_failed':
            return `${success} 다만 클립보드를 지우지 못했으니 직접 삭제해 주세요.`;
    }
}

function generationSelectionOperationIdentity(
    selection: GenerationSelectionInput,
): readonly string[] {
    return selection.kind === 'target'
        ? [selection.kind, selection.target.model_route_id, selection.target.generation_preset_id]
        : [selection.kind, selection.provider_profile_id];
}

function generationOperationIdentity(parts: readonly unknown[]): string {
    // Every caller supplies an explicit array with no object-valued members. JSON therefore
    // produces an unambiguous, order-stable identity without retaining user input in a map.
    return JSON.stringify(parts);
}

interface RetainedGenerationOperation {
    identity: string;
    nonce: string;
}

interface StagedGenerationAttemptRetry {
    identity: string | null;
    generationAttemptId: string;
}

type OrdinaryGenerationOperationContext =
    | { kind: 'new'; authority: RetainedGenerationOperation }
    | {
          kind: 'resume';
          identity: string;
          generationAttemptId: string;
      };

type OrdinaryGenerationOperationInputAuthority =
    { operation_nonce: string } | { generation_attempt_id: string };

export class LorepiaAppController {
    private readonly mutable = writable<LorepiaAppState>(structuredClone(INITIAL_APP_STATE));
    readonly state: Readable<LorepiaAppState> = this.mutable;

    private appEpoch = 0;
    private conversationEpoch = 0;
    private memoryQueryRetryEpoch = 0;
    private streamEpoch = 0;
    private providerEpoch = 0;
    private providerSettingsEpoch = 0;
    private discoveryRequestEpoch = 0;
    private providerSettingsMutationTail: Promise<void> = Promise.resolve();
    private reconcileInFlight: symbol | null = null;
    private reconcileBufferedItems: ChatStreamItemDto[] = [];
    private streamVerifier: ChatStreamVerifier | null = null;
    private activeStreamId: string | null = null;
    private deltaFlushTimer: ReturnType<typeof setTimeout> | null = null;
    private pendingTextDelta = '';
    private pendingReasoningDelta = '';
    private memorySupervisorUnlisten: (() => void) | null = null;
    private retainedGenerationOperation: RetainedGenerationOperation | null = null;
    private stagedGenerationAttemptRetry: StagedGenerationAttemptRetry | null = null;
    private roomGenerationTarget: {
        conversation_id: string;
        branch_id: string;
        /** undefined means the exact room target is still loading. */
        target: GenerationTargetDto | null | undefined;
    } | null = null;

    constructor(private readonly client: LorepiaClient) {}

    beginNewGenerationOperation(): void {
        this.retainedGenerationOperation = null;
        this.stagedGenerationAttemptRetry = null;
    }

    stageGenerationAttemptRetry(generationAttemptId: string): boolean {
        if (
            generationAttemptId.length === 0 ||
            Array.from(generationAttemptId).length > 256 ||
            new TextEncoder().encode(generationAttemptId).byteLength > 512 ||
            /\p{Cc}/u.test(generationAttemptId)
        ) {
            return false;
        }
        this.stagedGenerationAttemptRetry = {
            identity: this.retainedGenerationOperation?.identity ?? null,
            generationAttemptId,
        };
        return true;
    }

    private generationOperationAuthority(identity: string): RetainedGenerationOperation {
        if (this.retainedGenerationOperation?.identity === identity) {
            return this.retainedGenerationOperation;
        }
        const authority = { identity, nonce: globalThis.crypto.randomUUID() };
        this.retainedGenerationOperation = authority;
        return authority;
    }

    private completeGenerationOperation(authority: RetainedGenerationOperation): void {
        const retained = this.retainedGenerationOperation;
        if (retained?.identity === authority.identity && retained.nonce === authority.nonce) {
            this.retainedGenerationOperation = null;
        }
    }

    private generationOperationContext(identity: string): OrdinaryGenerationOperationContext {
        const staged = this.stagedGenerationAttemptRetry;
        if (staged !== null) {
            staged.identity ??= identity;
            if (staged.identity === identity) {
                return {
                    kind: 'resume',
                    identity,
                    generationAttemptId: staged.generationAttemptId,
                };
            }
            this.stagedGenerationAttemptRetry = null;
        }
        return { kind: 'new', authority: this.generationOperationAuthority(identity) };
    }

    private completeGenerationOperationContext(context: OrdinaryGenerationOperationContext): void {
        if (context.kind === 'new') {
            this.completeGenerationOperation(context.authority);
            return;
        }
        const staged = this.stagedGenerationAttemptRetry;
        if (
            staged?.identity === context.identity &&
            staged.generationAttemptId === context.generationAttemptId
        ) {
            this.stagedGenerationAttemptRetry = null;
            if (this.retainedGenerationOperation?.identity === context.identity) {
                this.retainedGenerationOperation = null;
            }
        }
    }

    private generationOperationContextInput(
        context: OrdinaryGenerationOperationContext,
    ): OrdinaryGenerationOperationInputAuthority {
        return context.kind === 'new'
            ? { operation_nonce: context.authority.nonce }
            : { generation_attempt_id: context.generationAttemptId };
    }

    private update(updater: (state: LorepiaAppState) => LorepiaAppState): void {
        this.mutable.update(updater);
    }

    private announce(message: string): void {
        this.update((state) => ({ ...state, announcement: message }));
    }

    async start(): Promise<void> {
        const epoch = ++this.appEpoch;
        this.update((state) => ({
            ...state,
            bootstrap: { ...state.bootstrap, phase: 'loading', error: null },
        }));
        try {
            const snapshot = await this.client.bootstrapSnapshot();
            ensureCompatible(snapshot);
            if (epoch !== this.appEpoch) return;
            this.update((state) => ({
                ...state,
                bootstrap: { phase: 'ready', error: null, value: snapshot },
            }));
            await Promise.all([
                this.loadLibrary(epoch),
                this.loadProviders(),
                this.connectMemorySupervisor(epoch),
            ]);
        } catch (error: unknown) {
            if (epoch !== this.appEpoch) return;
            this.update((state) => ({
                ...state,
                bootstrap: { phase: 'error', error: errorLabel(error), value: null },
            }));
        }
    }

    private async connectMemorySupervisor(parentEpoch: number): Promise<void> {
        this.memorySupervisorUnlisten?.();
        this.memorySupervisorUnlisten = null;
        this.update((state) => ({
            ...state,
            memory_supervisor: {
                ...state.memory_supervisor,
                phase: 'loading',
                error: null,
            },
        }));

        let subscriptionFailed = false;
        try {
            const unlisten = await this.client.subscribeMemorySupervisorStatus((status) => {
                if (parentEpoch !== this.appEpoch || !isMemorySupervisorStatus(status)) return;
                this.applyMemorySupervisorStatus(status);
            });
            if (parentEpoch !== this.appEpoch) {
                unlisten();
                return;
            }
            this.memorySupervisorUnlisten = unlisten;
        } catch {
            subscriptionFailed = true;
        }

        try {
            const status = await this.client.getMemorySupervisorStatus();
            if (parentEpoch !== this.appEpoch) return;
            if (!isMemorySupervisorStatus(status)) {
                throw new Error('invalid memory supervisor status');
            }
            this.applyMemorySupervisorStatus(status);
        } catch {
            if (parentEpoch !== this.appEpoch) return;
            this.update((state) => ({
                ...state,
                memory_supervisor: {
                    ...state.memory_supervisor,
                    phase: state.memory_supervisor.status === null ? 'error' : 'ready',
                    error:
                        state.memory_supervisor.status === null
                            ? '기억 작업 상태를 확인하지 못했습니다.'
                            : null,
                },
            }));
            return;
        }

        if (subscriptionFailed) {
            this.update((state) => ({
                ...state,
                memory_supervisor: {
                    ...state.memory_supervisor,
                    error: '기억 작업 상태의 실시간 갱신을 연결하지 못했습니다.',
                },
            }));
        }
    }

    private applyMemorySupervisorStatus(status: MemorySupervisorStatusDto): void {
        this.update((state) => {
            const current = state.memory_supervisor.status;
            if (current !== null && status.sequence < current.sequence) return state;
            return {
                ...state,
                memory_supervisor: {
                    phase: 'ready',
                    error: null,
                    status,
                },
            };
        });
    }

    async loadLibrary(parentEpoch = this.appEpoch): Promise<void> {
        this.update((state) => ({
            ...state,
            library: { ...state.library, phase: 'loading', error: null },
        }));
        try {
            const characters = await this.client.listCharacters();
            if (parentEpoch !== this.appEpoch) return;
            this.update((state) => ({
                ...state,
                library: { phase: 'ready', error: null, characters },
            }));
        } catch (error: unknown) {
            if (parentEpoch !== this.appEpoch) return;
            this.update((state) => ({
                ...state,
                library: { ...state.library, phase: 'error', error: errorLabel(error) },
            }));
        }
    }

    async beginImport(): Promise<void> {
        this.update((state) => ({
            ...state,
            import_flow: { phase: 'loading', error: null, inspection: null },
        }));
        try {
            const ticket = await this.client.selectImportSource();
            if (ticket === null) {
                this.update((state) => ({
                    ...state,
                    import_flow: { phase: 'idle', error: null, inspection: null },
                }));
                return;
            }
            const inspection = await this.client.inspectImport(ticket.ticket_id);
            this.update((state) => ({
                ...state,
                import_flow: { phase: 'ready', error: null, inspection },
            }));
            this.announce(`${inspection.display_name} 가져오기를 검토해 주세요.`);
        } catch (error: unknown) {
            this.update((state) => ({
                ...state,
                import_flow: { phase: 'error', error: errorLabel(error), inspection: null },
            }));
        }
    }

    async commitImport(): Promise<void> {
        const inspection = get(this.mutable).import_flow.inspection;
        if (inspection?.allowed !== true) return;
        this.update((state) => ({
            ...state,
            import_flow: { ...state.import_flow, phase: 'loading', error: null },
        }));
        try {
            const character = await this.client.commitImport(inspection.inspection_id);
            this.update((state) => ({
                ...state,
                library: {
                    phase: 'ready',
                    error: null,
                    characters: [
                        character,
                        ...state.library.characters.filter((item) => item.id !== character.id),
                    ],
                },
                import_flow: { phase: 'idle', error: null, inspection: null },
            }));
            this.announce(`${character.name}을(를) 서재에 추가했습니다.`);
        } catch (error: unknown) {
            this.update((state) => ({
                ...state,
                import_flow: {
                    ...state.import_flow,
                    phase: 'error',
                    error: errorLabel(error),
                },
            }));
        }
    }

    async discardImport(): Promise<void> {
        const inspection = get(this.mutable).import_flow.inspection;
        this.update((state) => ({
            ...state,
            import_flow: { phase: 'idle', error: null, inspection: null },
        }));
        if (inspection === null) return;
        try {
            await this.client.discardImport(inspection.inspection_id);
        } catch (error: unknown) {
            this.announce(errorLabel(error));
        }
    }

    async selectCharacter(character: CharacterDto): Promise<void> {
        const epoch = ++this.conversationEpoch;
        this.detachStream();
        this.update((state) => ({
            ...state,
            selected_character: character,
            selected_conversation: null,
            conversation_state: null,
            branches: [],
            messages: { phase: 'idle', error: null, items: [] },
            memory_query_retries: {
                phase: 'idle',
                error: null,
                candidates: [],
                busy_id: null,
                notice: null,
            },
            conversations: { phase: 'loading', error: null, items: [] },
            greeting_catalog: {
                phase: 'loading',
                error: null,
                value: null,
                selected_greeting_id: null,
            },
            chat: { ...INITIAL_APP_STATE.chat },
        }));
        const conversationsRequest = this.client
            .listConversations(character.id)
            .then((items) => {
                if (epoch !== this.conversationEpoch) return;
                this.update((state) => ({
                    ...state,
                    conversations: { phase: 'ready', error: null, items },
                }));
            })
            .catch((error: unknown) => {
                if (epoch !== this.conversationEpoch) return;
                this.update((state) => ({
                    ...state,
                    conversations: { phase: 'error', error: errorLabel(error), items: [] },
                }));
            });
        const greetingCatalogRequest = this.client
            .getCharacterGreetingCatalog(character.id)
            .then((catalog) => {
                if (epoch !== this.conversationEpoch) return;
                if (catalog.character_id !== character.id) {
                    this.update((state) => ({
                        ...state,
                        greeting_catalog: {
                            phase: 'error',
                            error: '캐릭터 인사 목록이 선택한 캐릭터와 일치하지 않습니다.',
                            value: null,
                            selected_greeting_id: null,
                        },
                    }));
                    return;
                }
                this.update((state) => ({
                    ...state,
                    greeting_catalog: {
                        phase: 'ready',
                        error: null,
                        value: catalog,
                        selected_greeting_id: firstEnabledGreetingId(catalog),
                    },
                }));
            })
            .catch((error: unknown) => {
                if (epoch !== this.conversationEpoch) return;
                this.update((state) => ({
                    ...state,
                    greeting_catalog: {
                        phase: 'error',
                        error: errorLabel(error),
                        value: null,
                        selected_greeting_id: null,
                    },
                }));
            });
        await Promise.all([conversationsRequest, greetingCatalogRequest]);
    }

    selectGreeting(greetingId: string): boolean {
        const state = get(this.mutable);
        const catalog = state.greeting_catalog.value;
        if (
            state.greeting_catalog.phase !== 'ready' ||
            catalog?.greetings.some(
                (greeting) => greeting.id === greetingId && greeting.enabled,
            ) !== true
        ) {
            return false;
        }
        this.update((current) => ({
            ...current,
            greeting_catalog: {
                ...current.greeting_catalog,
                selected_greeting_id: greetingId,
            },
        }));
        return true;
    }

    async openNewConversation(): Promise<boolean> {
        const state = get(this.mutable);
        const character = state.selected_character;
        const catalog = state.greeting_catalog.value;
        if (
            character === null ||
            state.greeting_catalog.phase !== 'ready' ||
            catalog?.character_id !== character.id
        ) {
            this.announce('정확한 캐릭터 인사 리비전을 불러온 뒤 새 대화를 시작해 주세요.');
            return false;
        }
        const greetingId = state.greeting_catalog.selected_greeting_id;
        if (
            greetingId !== null &&
            !catalog.greetings.some((greeting) => greeting.id === greetingId && greeting.enabled)
        ) {
            this.announce('사용 가능한 시작 인사를 다시 선택해 주세요.');
            return false;
        }
        const epoch = ++this.conversationEpoch;
        try {
            const conversation = await this.client.createConversation(
                character.id,
                character.name,
                'chat',
                {
                    character_content_revision_id: catalog.character_content_revision_id,
                    greeting_id: greetingId,
                },
            );
            if (epoch !== this.conversationEpoch) return false;
            this.update((state) => ({
                ...state,
                conversations: {
                    phase: 'ready',
                    error: null,
                    items: [
                        conversation,
                        ...state.conversations.items.filter((item) => item.id !== conversation.id),
                    ],
                },
            }));
            this.prepareConversationLoad(conversation);
            return await this.loadPreparedConversation(conversation, epoch);
        } catch (error: unknown) {
            if (epoch !== this.conversationEpoch) return false;
            this.update((state) => ({
                ...state,
                conversations: {
                    ...state.conversations,
                    phase: 'error',
                    error: errorLabel(error),
                },
            }));
            return false;
        }
    }

    private prepareConversationLoad(conversation: ConversationDto): void {
        ++this.memoryQueryRetryEpoch;
        this.detachStream();
        this.update((state) => ({
            ...state,
            selected_conversation: conversation,
            conversation_state: null,
            branches: [],
            messages: { phase: 'loading', error: null, items: [] },
            memory_query_retries: {
                phase: 'idle',
                error: null,
                candidates: [],
                busy_id: null,
                notice: null,
            },
            chat: { ...INITIAL_APP_STATE.chat },
        }));
    }

    private async loadPreparedConversation(
        conversation: ConversationDto,
        epoch: number,
    ): Promise<boolean> {
        try {
            const [conversationState, branches] = await Promise.all([
                this.client.getConversationState(conversation.id),
                this.client.listBranches(conversation.id),
            ]);
            const messages = await this.client.listBranchMessages(
                conversationState.active_branch_id,
            );
            if (epoch !== this.conversationEpoch) return false;
            this.update((state) => ({
                ...state,
                selected_conversation: conversation,
                conversation_state: conversationState,
                branches,
                messages: { phase: 'ready', error: null, items: messages },
            }));
            void this.refreshMemoryQueryRetries();
            this.resumePendingGeneration(messages);
            return true;
        } catch (error: unknown) {
            if (epoch !== this.conversationEpoch) return false;
            this.update((state) => ({
                ...state,
                messages: { phase: 'error', error: errorLabel(error), items: [] },
            }));
            return false;
        }
    }

    async selectConversation(conversation: ConversationDto): Promise<boolean> {
        const epoch = ++this.conversationEpoch;
        this.prepareConversationLoad(conversation);
        try {
            const opened = await this.client.openExistingConversation(conversation.id);
            if (epoch !== this.conversationEpoch) return false;
            this.update((state) => ({
                ...state,
                selected_conversation: opened,
                conversations: {
                    ...state.conversations,
                    items: state.conversations.items.map((item) =>
                        item.id === opened.id ? opened : item,
                    ),
                },
            }));
            return await this.loadPreparedConversation(opened, epoch);
        } catch (error: unknown) {
            if (epoch !== this.conversationEpoch) return false;
            this.update((state) => ({
                ...state,
                messages: { phase: 'error', error: errorLabel(error), items: [] },
            }));
            return false;
        }
    }

    async selectBranch(branchId: string): Promise<void> {
        const conversation = get(this.mutable).selected_conversation;
        if (conversation === null) return;
        const epoch = ++this.conversationEpoch;
        ++this.memoryQueryRetryEpoch;
        this.detachStream();
        this.update((state) => ({
            ...state,
            messages: { ...state.messages, phase: 'loading', error: null },
            memory_query_retries: {
                phase: 'idle',
                error: null,
                candidates: [],
                busy_id: null,
                notice: null,
            },
            chat: { ...INITIAL_APP_STATE.chat },
        }));
        try {
            const conversationState = await this.client.selectBranch(conversation.id, branchId);
            const messages = await this.client.listBranchMessages(branchId);
            if (epoch !== this.conversationEpoch) return;
            this.update((state) => ({
                ...state,
                conversation_state: conversationState,
                messages: { phase: 'ready', error: null, items: messages },
            }));
            void this.refreshMemoryQueryRetries();
            this.resumePendingGeneration(messages);
        } catch (error: unknown) {
            if (epoch !== this.conversationEpoch) return;
            this.update((state) => ({
                ...state,
                messages: { ...state.messages, phase: 'error', error: errorLabel(error) },
            }));
        }
    }

    async createBranch(fromMessageId: string | null): Promise<void> {
        const conversation = get(this.mutable).selected_conversation;
        if (conversation === null) return;
        try {
            const branch = await this.client.createBranch(conversation.id, fromMessageId, null);
            this.update((state) => ({
                ...state,
                branches: [branch, ...state.branches.filter((item) => item.id !== branch.id)],
            }));
            await this.selectBranch(branch.id);
            this.announce('새 대화 분기를 만들었습니다.');
        } catch (error: unknown) {
            this.announce(errorLabel(error));
        }
    }

    async setConversationMode(mode: ConversationMode): Promise<void> {
        const conversation = get(this.mutable).selected_conversation;
        if (conversation === null) return;
        try {
            const conversationState = await this.client.setConversationMode(conversation.id, mode);
            this.update((state) => ({ ...state, conversation_state: conversationState }));
            this.announce(
                mode === 'chat' ? '채팅 모드로 변경했습니다.' : '스토리 모드로 변경했습니다.',
            );
        } catch (error: unknown) {
            this.announce(errorLabel(error));
        }
    }

    setRoomGenerationTarget(
        conversationId: string | null,
        branchId: string | null,
        target: GenerationTargetDto | null | undefined,
    ): void {
        this.roomGenerationTarget =
            conversationId === null || branchId === null
                ? null
                : {
                      conversation_id: conversationId,
                      branch_id: branchId,
                      target: target === undefined ? undefined : structuredClone(target),
                  };
    }

    private generationSelection(state: LorepiaAppState): GenerationSelectionInput | null {
        const roomTarget = this.roomGenerationTarget;
        if (
            roomTarget !== null &&
            roomTarget.conversation_id === state.selected_conversation?.id &&
            roomTarget.branch_id === state.conversation_state?.active_branch_id
        ) {
            if (roomTarget.target === undefined) return null;
            if (roomTarget.target !== null) {
                return { kind: 'target', target: structuredClone(roomTarget.target) };
            }
        }
        const settings = state.providers.workspace.settings;
        const profileId = settings.selected_provider_profile_id;
        if (profileId !== null) {
            return { kind: 'legacy_profile', provider_profile_id: profileId };
        }
        const routeId = settings.selected_model_route_id;
        const presetId = settings.selected_generation_preset_id;
        if (routeId !== null && presetId !== null) {
            return {
                kind: 'target',
                target: {
                    model_route_id: routeId,
                    generation_preset_id: presetId,
                },
            };
        }
        return null;
    }

    async sendMessage(content: string): Promise<boolean> {
        const state = get(this.mutable);
        if (state.chat.active_generation_id !== null) {
            this.announce('진행 중인 생성을 취소한 뒤 새 메시지를 보내세요.');
            return false;
        }
        const conversation = state.selected_conversation;
        const conversationState = state.conversation_state;
        const selection = this.generationSelection(state);
        if (
            conversation === null ||
            conversationState === null ||
            selection === null ||
            content.trim().length === 0
        ) {
            this.announce('대화와 저장된 기본 모델을 확인한 뒤 메시지를 보내세요.');
            return false;
        }

        const branch = state.branches.find(
            (item) => item.id === conversationState.active_branch_id,
        );
        const expectedHead = branch?.head_message_id ?? null;
        const text = content.trim();
        const operationIdentity = generationOperationIdentity([
            'send',
            conversation.id,
            conversationState.active_branch_id,
            expectedHead,
            text,
            ...generationSelectionOperationIdentity(selection),
        ]);
        const operationContext = this.generationOperationContext(operationIdentity);
        this.clearMemoryQueryRetryNotice();
        const { epoch, streamId } = this.prepareStream(
            conversation.id,
            conversationState.active_branch_id,
        );
        const buffered: ChatStreamItemDto[] = [];
        let ready = false;
        try {
            const started = await this.client.sendMessage(
                {
                    conversation_id: conversation.id,
                    branch_id: conversationState.active_branch_id,
                    expected_head: expectedHead,
                    mode: conversationState.selected_mode,
                    text,
                    selection,
                    ...this.generationOperationContextInput(operationContext),
                },
                streamId,
                (item) => {
                    if (epoch !== this.streamEpoch || this.activeStreamId !== streamId) {
                        void this.disposeStream(streamId);
                        return;
                    }
                    if (ready) this.acceptStreamItem(item, epoch, streamId);
                    else buffered.push(item);
                },
            );
            if (epoch !== this.streamEpoch || this.activeStreamId !== streamId) {
                void this.disposeStream(streamId);
                return false;
            }
            if (!this.streamVerifier?.bindGeneration(started.generation_id)) {
                await this.reconcile(
                    started.generation_id,
                    epoch,
                    streamId,
                    'generation mismatch',
                    this.streamVerifier?.getLastSequence() ?? 0,
                );
                return false;
            }
            this.update((current) => ({
                ...current,
                chat: {
                    ...current.chat,
                    phase: 'ready',
                    active_generation_id: started.generation_id,
                },
            }));
            ready = true;
            for (const item of buffered) {
                if (epoch !== this.streamEpoch || this.activeStreamId !== streamId) break;
                this.acceptStreamItem(item, epoch, streamId);
            }
            this.completeGenerationOperationContext(operationContext);
            return true;
        } catch (error: unknown) {
            this.failStream(epoch, streamId, error);
            return false;
        }
    }

    async sendReviewedPrompt(input: ReviewedPromptSendInput): Promise<boolean> {
        const state = get(this.mutable);
        if (state.chat.active_generation_id !== null) {
            this.announce('진행 중인 생성을 취소한 뒤 검토한 계획을 보내세요.');
            return false;
        }
        const conversation = state.selected_conversation;
        const conversationState = state.conversation_state;
        const branch = state.branches.find(
            (item) => item.id === conversationState?.active_branch_id,
        );
        if (
            conversation === null ||
            conversationState === null ||
            input.conversation_id !== conversation.id ||
            input.branch_id !== conversationState.active_branch_id ||
            input.expected_head !== (branch?.head_message_id ?? null)
        ) {
            this.announce('대화 상태가 미리보기 이후 바뀌었습니다. 최종 계획을 다시 검토하세요.');
            return false;
        }

        this.clearMemoryQueryRetryNotice();
        const { epoch, streamId } = this.prepareStream(input.conversation_id, input.branch_id);
        const buffered: ChatStreamItemDto[] = [];
        let ready = false;
        try {
            const started = await this.client.sendReviewedPrompt(input, streamId, (item) => {
                if (epoch !== this.streamEpoch || this.activeStreamId !== streamId) {
                    void this.disposeStream(streamId);
                    return;
                }
                if (ready) this.acceptStreamItem(item, epoch, streamId);
                else buffered.push(item);
            });
            if (epoch !== this.streamEpoch || this.activeStreamId !== streamId) {
                void this.disposeStream(streamId);
                return false;
            }
            if (!this.streamVerifier?.bindGeneration(started.generation_id)) {
                await this.reconcile(
                    started.generation_id,
                    epoch,
                    streamId,
                    'generation mismatch',
                    this.streamVerifier?.getLastSequence() ?? 0,
                );
                return false;
            }
            this.update((current) => ({
                ...current,
                chat: {
                    ...current.chat,
                    phase: 'ready',
                    active_generation_id: started.generation_id,
                },
            }));
            ready = true;
            for (const item of buffered) {
                if (epoch !== this.streamEpoch || this.activeStreamId !== streamId) break;
                this.acceptStreamItem(item, epoch, streamId);
            }
            return true;
        } catch (error: unknown) {
            this.failStream(epoch, streamId, error);
            return false;
        }
    }

    async editUserMessage(messageId: string, replacementText: string): Promise<boolean> {
        const trimmed = replacementText.trim();
        if (trimmed.length === 0) return false;
        return this.startBranchGeneration(
            'edit',
            messageId,
            trimmed,
            (state, selection, operationAuthority, streamId, onItem) => {
                const branchId = state.conversation_state?.active_branch_id;
                const conversationId = state.selected_conversation?.id;
                if (branchId === undefined || conversationId === undefined) return null;
                return this.client.editUserMessage(
                    {
                        conversation_id: conversationId,
                        branch_id: branchId,
                        expected_head: this.activeBranchHead(state),
                        message_id: messageId,
                        replacement_text: trimmed,
                        selection,
                        ...operationAuthority,
                    },
                    streamId,
                    onItem,
                );
            },
        );
    }

    async regenerateAssistantMessage(messageId: string): Promise<boolean> {
        return this.startBranchGeneration(
            'regenerate',
            messageId,
            null,
            (state, selection, operationAuthority, streamId, onItem) => {
                const branchId = state.conversation_state?.active_branch_id;
                const conversationId = state.selected_conversation?.id;
                if (branchId === undefined || conversationId === undefined) return null;
                return this.client.regenerateAssistantMessage(
                    {
                        conversation_id: conversationId,
                        branch_id: branchId,
                        expected_head: this.activeBranchHead(state),
                        message_id: messageId,
                        selection,
                        ...operationAuthority,
                    },
                    streamId,
                    onItem,
                );
            },
        );
    }

    private async startBranchGeneration(
        action: 'edit' | 'regenerate',
        messageId: string,
        replacementText: string | null,
        start: (
            state: LorepiaAppState,
            selection: GenerationSelectionInput,
            operationAuthority: OrdinaryGenerationOperationInputAuthority,
            streamId: string,
            onItem: (item: ChatStreamItemDto) => void,
        ) => Promise<MessageActionGenerationDto> | null,
    ): Promise<boolean> {
        const state = get(this.mutable);
        if (state.chat.active_generation_id !== null) {
            this.announce('진행 중인 생성을 취소한 뒤 메시지를 변경하세요.');
            return false;
        }
        const conversation = state.selected_conversation;
        const selection = this.generationSelection(state);
        if (conversation === null || state.conversation_state === null || selection === null) {
            this.announce('대화와 저장된 기본 모델을 먼저 확인해 주세요.');
            return false;
        }

        const operationIdentity = generationOperationIdentity([
            action,
            conversation.id,
            state.conversation_state.active_branch_id,
            this.activeBranchHead(state),
            messageId,
            replacementText,
            ...generationSelectionOperationIdentity(selection),
        ]);
        const operationContext = this.generationOperationContext(operationIdentity);

        this.clearMemoryQueryRetryNotice();
        const { epoch, streamId } = this.beginStreamReceiver();
        const buffered: ChatStreamItemDto[] = [];
        let ready = false;
        this.setChatLoading();
        try {
            const started = await start(
                state,
                selection,
                this.generationOperationContextInput(operationContext),
                streamId,
                (item) => {
                    if (epoch !== this.streamEpoch || this.activeStreamId !== streamId) {
                        void this.disposeStream(streamId);
                        return;
                    }
                    if (ready) this.acceptStreamItem(item, epoch, streamId);
                    else buffered.push(item);
                },
            );
            if (
                started === null ||
                epoch !== this.streamEpoch ||
                this.activeStreamId !== streamId
            ) {
                void this.disposeStream(streamId);
                return false;
            }

            const conversationState = await this.client.selectBranch(
                conversation.id,
                started.branch.id,
            );
            const messages = await this.client.listBranchMessages(started.branch.id);
            if (epoch !== this.streamEpoch || this.activeStreamId !== streamId) {
                void this.disposeStream(streamId);
                return false;
            }
            const pendingAssistant = this.pendingAssistantMessage(messages, started.generation_id);
            ++this.memoryQueryRetryEpoch;
            this.streamVerifier = new ChatStreamVerifier({
                conversationId: conversation.id,
                branchId: started.branch.id,
                generationId: started.generation_id,
                assistantMessageId: pendingAssistant?.id,
            });
            this.update((current) => ({
                ...current,
                conversation_state: conversationState,
                branches: [
                    started.branch,
                    ...current.branches.filter((item) => item.id !== started.branch.id),
                ],
                messages: { phase: 'ready', error: null, items: messages },
                memory_query_retries: {
                    phase: 'idle',
                    error: null,
                    candidates: [],
                    busy_id: null,
                    notice: null,
                },
                chat: {
                    ...current.chat,
                    phase: 'ready',
                    active_generation_id: started.generation_id,
                },
            }));
            void this.refreshMemoryQueryRetries();
            ready = true;
            for (const item of buffered) this.acceptStreamItem(item, epoch, streamId);
            this.completeGenerationOperationContext(operationContext);
            return true;
        } catch (error: unknown) {
            this.failStream(epoch, streamId, error);
            return false;
        }
    }

    async removeMessage(messageId: string): Promise<void> {
        const state = get(this.mutable);
        const conversation = state.selected_conversation;
        const branchId = state.conversation_state?.active_branch_id;
        if (conversation === null || branchId === undefined) return;
        const conversationId = conversation.id;
        const expectedHead = this.activeBranchHead(state);
        const epoch = this.conversationEpoch;
        const isCurrentBranchSnapshot = (current: LorepiaAppState): boolean =>
            epoch === this.conversationEpoch &&
            current.selected_conversation?.id === conversationId &&
            current.conversation_state?.active_branch_id === branchId &&
            this.activeBranchHead(current) === expectedHead;
        try {
            const branch = await this.client.removeMessageFromBranch({
                conversation_id: conversationId,
                branch_id: branchId,
                expected_head: expectedHead,
                message_id: messageId,
            });
            if (!isCurrentBranchSnapshot(get(this.mutable))) return;
            if (branch.id !== branchId || branch.conversation_id !== conversationId) {
                this.announce('메시지 제거 결과가 요청한 대화 분기와 일치하지 않습니다.');
                return;
            }
            const messages = await this.client.listBranchMessages(branchId);
            if (!isCurrentBranchSnapshot(get(this.mutable))) return;
            this.update((current) => ({
                ...current,
                branches: current.branches.map((item) => (item.id === branchId ? branch : item)),
                messages: { phase: 'ready', error: null, items: messages },
            }));
            this.announce('이 메시지부터 분기에서 제거했습니다.');
        } catch (error: unknown) {
            if (!isCurrentBranchSnapshot(get(this.mutable))) return;
            this.announce(errorLabel(error));
        }
    }

    async refreshMemoryQueryRetries(): Promise<void> {
        const state = get(this.mutable);
        const conversationId = state.selected_conversation?.id;
        const branchId = state.conversation_state?.active_branch_id;
        if (conversationId === undefined || branchId === undefined) {
            ++this.memoryQueryRetryEpoch;
            this.update((current) => ({
                ...current,
                memory_query_retries: {
                    phase: 'idle',
                    error: null,
                    candidates: [],
                    busy_id: null,
                    notice: null,
                },
            }));
            return;
        }
        if (state.memory_query_retries.busy_id !== null) return;
        const requestEpoch = ++this.memoryQueryRetryEpoch;
        this.update((current) => ({
            ...current,
            memory_query_retries: {
                ...current.memory_query_retries,
                phase: 'loading',
                error: null,
            },
        }));
        try {
            const candidates = await this.client.listRetryableMemoryQueryEmbeddings({
                conversation_id: conversationId,
                branch_id: branchId,
                limit: MAX_MEMORY_QUERY_RETRY_CANDIDATES,
            });
            const current = get(this.mutable);
            if (
                requestEpoch !== this.memoryQueryRetryEpoch ||
                current.selected_conversation?.id !== conversationId ||
                current.conversation_state?.active_branch_id !== branchId
            ) {
                return;
            }
            const uniqueIds = new Set(candidates.map((candidate) => candidate.id));
            if (
                candidates.length > MAX_MEMORY_QUERY_RETRY_CANDIDATES ||
                uniqueIds.size !== candidates.length ||
                !candidates.every((candidate) =>
                    isRetryableMemoryQueryCandidate(candidate, conversationId, branchId),
                )
            ) {
                this.update((value) => ({
                    ...value,
                    memory_query_retries: {
                        ...value.memory_query_retries,
                        phase: 'error',
                        error: '기억 검색 재시도 목록을 검증하지 못했습니다.',
                        busy_id: null,
                    },
                }));
                return;
            }
            this.update((value) => ({
                ...value,
                memory_query_retries: {
                    phase: 'ready',
                    error: null,
                    candidates,
                    busy_id: null,
                    notice: value.memory_query_retries.notice,
                },
            }));
        } catch (error: unknown) {
            const current = get(this.mutable);
            if (
                requestEpoch !== this.memoryQueryRetryEpoch ||
                current.selected_conversation?.id !== conversationId ||
                current.conversation_state?.active_branch_id !== branchId
            ) {
                return;
            }
            this.update((value) => ({
                ...value,
                memory_query_retries: {
                    ...value.memory_query_retries,
                    phase: 'error',
                    error: errorLabel(error),
                    busy_id: null,
                },
            }));
        }
    }

    clearMemoryQueryRetryNotice(): void {
        this.update((state) =>
            state.memory_query_retries.notice === null
                ? state
                : {
                      ...state,
                      memory_query_retries: {
                          ...state.memory_query_retries,
                          notice: null,
                      },
                  },
        );
    }

    async retryMemoryQueryEmbedding(
        candidate: MemoryQueryEmbeddingRetryCandidateDto,
        acknowledgeUnknownOutcome: boolean,
    ): Promise<boolean> {
        const state = get(this.mutable);
        const listedCandidate = state.memory_query_retries.candidates.find(
            (value) => value.id === candidate.id,
        );
        if (listedCandidate?.revision !== candidate.revision) {
            this.announce('현재 대화 분기의 재시도 항목을 다시 불러와 주세요.');
            return false;
        }
        if (
            listedCandidate.status !== candidate.status ||
            listedCandidate.requires_unknown_outcome_acknowledgement !==
                candidate.requires_unknown_outcome_acknowledgement ||
            state.selected_conversation?.id !== listedCandidate.conversation_id ||
            state.conversation_state?.active_branch_id !== listedCandidate.branch_id ||
            !isRetryableMemoryQueryCandidate(
                listedCandidate,
                listedCandidate.conversation_id,
                listedCandidate.branch_id,
            )
        ) {
            this.announce('현재 대화 분기의 재시도 항목을 다시 불러와 주세요.');
            return false;
        }
        if (state.memory_query_retries.busy_id !== null) {
            this.announce('진행 중인 기억 검색 재시도가 끝난 뒤 다시 시도해 주세요.');
            return false;
        }
        if (listedCandidate.status === 'interrupted' && !acknowledgeUnknownOutcome) {
            this.announce('결과를 알 수 없는 외부 요청임을 확인한 뒤 다시 시도해 주세요.');
            return false;
        }
        ++this.memoryQueryRetryEpoch;
        this.update((current) => ({
            ...current,
            memory_query_retries: {
                ...current.memory_query_retries,
                phase: 'loading',
                error: null,
                busy_id: listedCandidate.id,
                notice: null,
            },
        }));
        try {
            const receipt = await this.client.retryMemoryQueryEmbedding({
                id: listedCandidate.id,
                expected_revision: listedCandidate.revision,
                acknowledge_unknown_outcome:
                    listedCandidate.status === 'interrupted' && acknowledgeUnknownOutcome,
            });
            if (!isQueuedMemoryQueryRetryReceipt(receipt, listedCandidate)) {
                const current = get(this.mutable);
                if (
                    current.selected_conversation?.id === listedCandidate.conversation_id &&
                    current.conversation_state?.active_branch_id === listedCandidate.branch_id
                ) {
                    this.update((value) => ({
                        ...value,
                        memory_query_retries: {
                            ...value.memory_query_retries,
                            phase: 'error',
                            error: '재시도 결과를 검증하지 못했습니다. 목록을 새로고침해 상태를 확인하세요.',
                            busy_id: null,
                        },
                    }));
                }
                return false;
            }
            const current = get(this.mutable);
            if (
                current.selected_conversation?.id !== listedCandidate.conversation_id ||
                current.conversation_state?.active_branch_id !== listedCandidate.branch_id
            ) {
                return true;
            }
            const notice =
                '임베딩 준비만 다시 대기열에 넣었습니다. 미리보기나 메시지 결과는 만들지 않았습니다. 원래 계획 미리보기 또는 메시지 전송·편집·재생성을 다시 실행하세요.';
            this.update((value) => ({
                ...value,
                memory_query_retries: {
                    phase: 'ready',
                    error: null,
                    candidates: value.memory_query_retries.candidates.filter(
                        (listed) =>
                            listed.id !== listedCandidate.id ||
                            listed.revision !== listedCandidate.revision,
                    ),
                    busy_id: null,
                    notice,
                },
            }));
            this.announce(notice);
            return true;
        } catch (error: unknown) {
            const current = get(this.mutable);
            if (
                current.selected_conversation?.id === listedCandidate.conversation_id &&
                current.conversation_state?.active_branch_id === listedCandidate.branch_id
            ) {
                this.update((value) => ({
                    ...value,
                    memory_query_retries: {
                        ...value.memory_query_retries,
                        phase: 'error',
                        error: errorLabel(error),
                        busy_id: null,
                    },
                }));
            }
            return false;
        }
    }

    private activeBranchHead(state: LorepiaAppState): string | null {
        const activeBranchId = state.conversation_state?.active_branch_id;
        return state.branches.find((item) => item.id === activeBranchId)?.head_message_id ?? null;
    }

    private beginStreamReceiver(): { epoch: number; streamId: string } {
        this.detachStream();
        const streamId = this.activateStreamReceiver();
        return { epoch: this.streamEpoch, streamId };
    }

    private activateStreamReceiver(): string {
        const streamId = globalThis.crypto.randomUUID();
        this.activeStreamId = streamId;
        return streamId;
    }

    private prepareStream(
        conversationId: string,
        branchId: string,
        generationId?: string,
        assistantMessageId?: string,
        sequenceBaseline = 0,
    ): { epoch: number; streamId: string } {
        const active = this.beginStreamReceiver();
        this.streamVerifier = new ChatStreamVerifier({
            conversationId,
            branchId,
            generationId,
            assistantMessageId,
            sequenceBaseline,
        });
        this.setChatLoading(generationId ?? null);
        return active;
    }

    private setChatLoading(generationId: string | null = null): void {
        this.update((state) => ({
            ...state,
            chat: {
                phase: 'loading',
                error: null,
                active_generation_id: generationId,
                live_assistant_message_id: null,
                streaming_text: '',
                reasoning_text: '',
                reconcile_notice: null,
                usage_label: null,
            },
        }));
    }

    private failStream(epoch: number, streamId: string, error: unknown): void {
        void this.disposeStream(streamId);
        if (epoch !== this.streamEpoch) return;
        this.cancelPendingDeltas();
        this.update((state) => ({
            ...state,
            chat: {
                ...state.chat,
                phase: 'error',
                error: errorLabel(error),
                active_generation_id: null,
                live_assistant_message_id: null,
            },
        }));
        void this.refreshMemoryQueryRetries();
    }

    private resumePendingGeneration(messages: MessageDto[]): void {
        const pending = this.pendingAssistantMessage(messages);
        if (pending?.generation_id === null || pending?.generation_id === undefined) return;
        const state = get(this.mutable);
        const conversationId = state.selected_conversation?.id;
        const branchId = state.conversation_state?.active_branch_id;
        if (conversationId === undefined || branchId === undefined) return;
        const generationId = pending.generation_id;
        const { epoch, streamId } = this.prepareStream(
            conversationId,
            branchId,
            generationId,
            pending.id,
        );
        void this.subscribePendingGeneration(
            generationId,
            conversationId,
            branchId,
            pending.id,
            0,
            epoch,
            streamId,
        );
    }

    private async subscribePendingGeneration(
        generationId: string,
        conversationId: string,
        branchId: string,
        assistantMessageId: string,
        sequenceBaseline: number,
        epoch: number,
        streamId: string,
    ): Promise<boolean> {
        const buffered: ChatStreamItemDto[] = [];
        let ready = false;
        try {
            await this.client.subscribeGeneration(
                generationId,
                conversationId,
                branchId,
                sequenceBaseline,
                streamId,
                (item) => {
                    if (epoch !== this.streamEpoch || this.activeStreamId !== streamId) {
                        void this.disposeStream(streamId);
                        return;
                    }
                    if (ready) this.acceptStreamItem(item, epoch, streamId);
                    else buffered.push(item);
                },
            );
            if (epoch !== this.streamEpoch || this.activeStreamId !== streamId) {
                void this.disposeStream(streamId);
                return false;
            }
            this.streamVerifier = new ChatStreamVerifier({
                conversationId,
                branchId,
                generationId,
                assistantMessageId,
                sequenceBaseline,
                requireLiveSnapshot: true,
            });
            this.update((state) => ({
                ...state,
                chat: {
                    ...state.chat,
                    phase: 'ready',
                    error: null,
                    active_generation_id: generationId,
                    reconcile_notice: null,
                },
            }));
            ready = true;
            for (const item of buffered) {
                if (epoch !== this.streamEpoch || this.activeStreamId !== streamId) break;
                this.acceptStreamItem(item, epoch, streamId);
            }
            return true;
        } catch (error: unknown) {
            void this.disposeStream(streamId);
            if (epoch !== this.streamEpoch) return false;
            const normalized = normalizeClientError(error);
            if (
                normalized.code === 'generation_reattachment_unavailable' &&
                (await this.settleGenerationAfterUnavailableSubscription(
                    generationId,
                    conversationId,
                    epoch,
                ))
            ) {
                return false;
            }
            if (epoch !== this.streamEpoch) return false;
            this.streamVerifier = null;
            this.cancelPendingDeltas();
            this.update((state) => ({
                ...state,
                chat: {
                    ...reattachmentUnavailableChatState(generationId),
                    error: errorLabel(normalized),
                },
            }));
            return false;
        }
    }

    private async settleGenerationAfterUnavailableSubscription(
        generationId: string,
        conversationId: string,
        epoch: number,
    ): Promise<boolean> {
        if (get(this.mutable).selected_conversation?.id !== conversationId) return false;
        try {
            const conversationState = await this.client.getConversationState(conversationId);
            const [branches, messages] = await Promise.all([
                this.client.listBranches(conversationId),
                this.client.listBranchMessages(conversationState.active_branch_id),
            ]);
            if (
                epoch !== this.streamEpoch ||
                get(this.mutable).selected_conversation?.id !== conversationId
            ) {
                return false;
            }
            if (this.pendingAssistantMessage(messages, generationId) !== null) return false;
            this.streamVerifier = null;
            this.reconcileBufferedItems = [];
            this.cancelPendingDeltas();
            this.update((state) => ({
                ...state,
                conversation_state: conversationState,
                branches,
                messages: { phase: 'ready', error: null, items: messages },
                chat: {
                    ...state.chat,
                    phase: 'idle',
                    error: null,
                    active_generation_id: null,
                    live_assistant_message_id: null,
                    streaming_text: '',
                    reasoning_text: '',
                    reconcile_notice: null,
                },
            }));
            this.announce('대화가 저장된 상태와 동기화됐습니다.');
            return true;
        } catch {
            return false;
        }
    }

    private pendingAssistantMessage(
        messages: MessageDto[],
        generationId?: string,
    ): MessageDto | null {
        return (
            [...messages]
                .reverse()
                .find(
                    (message) =>
                        message.role === 'assistant' &&
                        message.status === 'pending' &&
                        message.generation_id !== null &&
                        (generationId === undefined || message.generation_id === generationId),
                ) ?? null
        );
    }

    private acceptStreamItem(item: ChatStreamItemDto, epoch: number, streamId: string): void {
        if (this.reconcileInFlight !== null) {
            if (epoch === this.streamEpoch && this.activeStreamId === streamId) {
                this.reconcileBufferedItems.push(item);
            }
            return;
        }
        if (this.streamVerifier === null) return;
        const decision = this.streamVerifier.accept(item);
        if (decision.type === 'ignore') return;
        if (decision.type === 'live_snapshot') {
            this.cancelPendingDeltas();
            const liveAssistant = this.pendingAssistantMessage(
                get(this.mutable).messages.items,
                decision.generationId,
            );
            if (liveAssistant === null) {
                void this.reconcile(
                    decision.generationId,
                    epoch,
                    streamId,
                    'live snapshot route mismatch',
                    decision.sequenceBaseline,
                );
                return;
            }
            this.update((state) => ({
                ...state,
                chat: {
                    ...state.chat,
                    phase: 'ready',
                    error: null,
                    active_generation_id: decision.generationId,
                    live_assistant_message_id: liveAssistant.id,
                    streaming_text: decision.displayPrefix,
                    reasoning_text: decision.reasoningPrefix,
                    reconcile_notice: null,
                },
            }));
            return;
        }
        if (decision.type === 'reconcile') {
            if (decision.reason === 'terminal') {
                this.flushPendingDeltas(epoch);
            } else {
                this.cancelPendingDeltas();
            }
            const generationId =
                decision.event?.generation_id ?? this.streamVerifier.getGenerationId();
            if (generationId !== null) {
                void this.reconcile(
                    generationId,
                    epoch,
                    streamId,
                    decision.reason,
                    decision.sequenceBaseline,
                );
            }
            return;
        }
        this.applyChatEvent(decision.event, epoch);
    }

    private applyChatEvent(event: ChatEventDto, epoch: number): void {
        if (event.kind.type === 'text_delta') {
            this.pendingTextDelta += event.kind.payload;
            this.scheduleDeltaFlush(epoch);
            return;
        }
        if (event.kind.type === 'reasoning_delta') {
            this.pendingReasoningDelta += event.kind.payload;
            this.scheduleDeltaFlush(epoch);
            return;
        }
        this.update((state) => {
            const chat = { ...state.chat, active_generation_id: event.generation_id };
            switch (event.kind.type) {
                case 'generation_started':
                    chat.phase = 'ready';
                    break;
                case 'usage_updated': {
                    const output = event.kind.payload.output_tokens;
                    chat.usage_label =
                        output === null ? null : `출력 ${output.toLocaleString()} 토큰`;
                    break;
                }
                case 'message_committed':
                    chat.reconcile_notice = '저장된 메시지를 확인하는 중입니다.';
                    break;
                case 'tool_call_started':
                    chat.reconcile_notice =
                        '모델이 도구 사용을 제안했습니다. 자동 실행하지 않습니다.';
                    break;
                case 'tool_call_arguments_delta':
                case 'tool_call_completed':
                case 'generation_cancelled':
                case 'generation_failed':
                case 'generation_finished':
                    break;
                case 'text_delta':
                case 'reasoning_delta':
                    break;
            }
            return { ...state, chat };
        });
    }

    private scheduleDeltaFlush(epoch: number): void {
        if (this.deltaFlushTimer !== null) return;
        this.deltaFlushTimer = setTimeout(() => this.flushPendingDeltas(epoch), 16);
    }

    private flushPendingDeltas(epoch: number): void {
        if (this.deltaFlushTimer !== null) {
            clearTimeout(this.deltaFlushTimer);
            this.deltaFlushTimer = null;
        }
        if (epoch !== this.streamEpoch) {
            this.pendingTextDelta = '';
            this.pendingReasoningDelta = '';
            return;
        }
        const text = this.pendingTextDelta;
        const reasoning = this.pendingReasoningDelta;
        this.pendingTextDelta = '';
        this.pendingReasoningDelta = '';
        if (text === '' && reasoning === '') return;
        this.update((state) => ({
            ...state,
            chat: {
                ...state.chat,
                streaming_text: state.chat.streaming_text + text,
                reasoning_text: state.chat.reasoning_text + reasoning,
            },
        }));
    }

    private cancelPendingDeltas(): void {
        if (this.deltaFlushTimer !== null) {
            clearTimeout(this.deltaFlushTimer);
            this.deltaFlushTimer = null;
        }
        this.pendingTextDelta = '';
        this.pendingReasoningDelta = '';
    }

    private async reconcile(
        generationId: string,
        epoch: number,
        streamId: string,
        reason: string,
        sequenceBaseline: number,
    ): Promise<void> {
        if (this.reconcileInFlight !== null) return;
        const conversation = get(this.mutable).selected_conversation;
        if (conversation === null) {
            void this.disposeStream(streamId);
            return;
        }
        const reconciliation = Symbol('generation-reconciliation');
        this.reconcileInFlight = reconciliation;
        this.reconcileBufferedItems = [];
        this.update((state) => ({
            ...state,
            chat: {
                ...state.chat,
                reconcile_notice: `스트림 상태를 복구하는 중입니다. (${reason})`,
            },
        }));
        try {
            await this.disposeStream(streamId);
            if (epoch !== this.streamEpoch) return;
            const conversationState = await this.client.getConversationState(conversation.id);
            const [branches, messages] = await Promise.all([
                this.client.listBranches(conversation.id),
                this.client.listBranchMessages(conversationState.active_branch_id),
            ]);
            if (epoch !== this.streamEpoch) return;
            const pendingAssistant = this.pendingAssistantMessage(messages, generationId);
            this.streamVerifier = null;
            this.update((state) => ({
                ...state,
                conversation_state: conversationState,
                branches,
                messages: { phase: 'ready', error: null, items: messages },
                chat:
                    pendingAssistant === null
                        ? {
                              ...state.chat,
                              phase: 'idle',
                              error: null,
                              active_generation_id: null,
                              live_assistant_message_id: null,
                              streaming_text: '',
                              reasoning_text: '',
                              reconcile_notice: null,
                          }
                        : {
                              ...state.chat,
                              phase: 'loading',
                              error: null,
                              active_generation_id: generationId,
                              live_assistant_message_id: null,
                              streaming_text: '',
                              reasoning_text: '',
                              reconcile_notice:
                                  '저장된 상태에서 생성 스트림을 다시 연결하는 중입니다.',
                          },
            }));
            if (pendingAssistant === null) {
                this.reconcileBufferedItems = [];
                this.announce('대화가 저장된 상태와 동기화됐습니다.');
                return;
            }
            this.reconcileBufferedItems = [];
            const nextStreamId = this.activateStreamReceiver();
            void this.subscribePendingGeneration(
                generationId,
                conversation.id,
                conversationState.active_branch_id,
                pendingAssistant.id,
                sequenceBaseline,
                epoch,
                nextStreamId,
            );
        } catch (error: unknown) {
            void this.disposeStream(streamId);
            if (epoch !== this.streamEpoch) return;
            this.update((state) => ({
                ...state,
                chat: {
                    ...state.chat,
                    phase: 'error',
                    error: errorLabel(error),
                    live_assistant_message_id: null,
                    streaming_text: '',
                    reasoning_text: '',
                    reconcile_notice: '대화 새로고침이 필요합니다.',
                },
            }));
        } finally {
            if (this.reconcileInFlight === reconciliation) this.reconcileInFlight = null;
        }
    }

    async cancelGeneration(): Promise<void> {
        const generationId = get(this.mutable).chat.active_generation_id;
        if (generationId === null) return;
        try {
            await this.client.cancelGeneration(generationId);
            this.announce('생성 취소를 요청했습니다.');
        } catch (error: unknown) {
            const normalized = normalizeClientError(error);
            if (normalized.code !== 'not_found' && normalized.code !== 'cancelled') {
                this.announce(errorLabel(normalized));
            }
        }
    }

    async loadProviders(): Promise<void> {
        const epoch = ++this.providerEpoch;
        const settingsEpoch = this.providerSettingsEpoch;
        this.update((state) => ({
            ...state,
            providers: { ...state.providers, phase: 'loading', error: null },
        }));
        try {
            const [overview, discoveries, catalogStatus, catalogHistory] = await Promise.all([
                this.client.getProviderOverview(),
                this.client.listProviderDiscoveries(50),
                this.client.providerCatalogStatus(),
                this.client.providerCatalogHistory(50, null, null),
            ]);
            const routeGroups = await Promise.all(
                overview.connections.map((connection) =>
                    this.client.listModelRoutes(connection.id),
                ),
            );
            const routes = routeGroups.flat();
            const presetGroups = await Promise.all(
                routes.map((route) => this.client.listGenerationPresets(route.id)),
            );
            const retainedLegacyProfileIds = new Set(
                overview.legacy_profiles.map((profile) => profile.id),
            );
            const credentialTargets: CredentialTargetDto[] = [
                ...overview.connections
                    .filter(
                        (connection) =>
                            connection.credential_binding_required &&
                            !retainedLegacyProfileIds.has(connection.id),
                    )
                    .map((connection): CredentialTargetDto => ({
                        kind: 'connection',
                        connection_id: connection.id,
                    })),
                ...overview.legacy_profiles.map((profile): CredentialTargetDto => ({
                    kind: 'legacy_profile',
                    provider_profile_id: profile.id,
                })),
                ...discoveries.flatMap((session): CredentialTargetDto[] => {
                    const target = discoveryCredentialTarget(session);
                    return target === null ? [] : [target];
                }),
            ];
            const credentialStates = await Promise.all(
                credentialTargets.map(async (target) => ({
                    target,
                    status: (await this.client.credentialStatus(target)).status,
                })),
            );
            const modelSyncGroups = await Promise.all(
                overview.connections.map((connection) =>
                    this.client.listProviderModelSyncs(connection.id, 20),
                ),
            );
            if (epoch !== this.providerEpoch) return;
            this.update((state) => ({
                ...state,
                providers: {
                    phase: 'ready',
                    error: null,
                    workspace: {
                        templates: overview.templates,
                        connections: overview.connections,
                        legacy_profiles: overview.legacy_profiles,
                        routes,
                        presets: presetGroups.flat(),
                        settings:
                            settingsEpoch === this.providerSettingsEpoch
                                ? overview.settings
                                : state.providers.workspace.settings,
                        credential_statuses: Object.fromEntries(
                            credentialStates.map(({ target, status }) => [
                                credentialKey(target),
                                status,
                            ]),
                        ),
                        request_preview: state.providers.workspace.request_preview,
                        selected_capability_model_route_id:
                            state.providers.workspace.selected_capability_model_route_id,
                        capability_observations: state.providers.workspace.capability_observations,
                        capability_parameter_specs:
                            state.providers.workspace.capability_parameter_specs,
                        effective_capability: state.providers.workspace.effective_capability,
                        model_sync_jobs: modelSyncGroups
                            .flat()
                            .sort((left, right) => right.updated_at.localeCompare(left.updated_at)),
                        selected_model_sync_job_id:
                            state.providers.workspace.selected_model_sync_job_id,
                        model_sync_event: state.providers.workspace.model_sync_event,
                        discoveries,
                        selected_discovery_id: state.providers.workspace.selected_discovery_id,
                        discovery_candidates: state.providers.workspace.discovery_candidates,
                        discovery_evidence: state.providers.workspace.discovery_evidence,
                        discovery_approvals: state.providers.workspace.discovery_approvals,
                        discovery_review: state.providers.workspace.discovery_review,
                        discovery_approval_proposal:
                            state.providers.workspace.discovery_approval_proposal,
                        discovery_review_proposal:
                            state.providers.workspace.discovery_review_proposal,
                        discovery_assistant_resume_boundary:
                            state.providers.workspace.discovery_assistant_resume_boundary,
                        discovery_assistant_host_action:
                            state.providers.workspace.discovery_assistant_host_action,
                        discovery_event: state.providers.workspace.discovery_event,
                        discovery_compensation_steps:
                            state.providers.workspace.discovery_compensation_steps,
                        discovery_recovery_results:
                            state.providers.workspace.discovery_recovery_results,
                        catalog_status: catalogStatus,
                        catalog_history: catalogHistory,
                        pending_catalog_import: state.providers.workspace.pending_catalog_import,
                        pending_catalog_rollback:
                            state.providers.workspace.pending_catalog_rollback,
                        catalog_diff: state.providers.workspace.catalog_diff,
                    },
                },
            }));
        } catch (error: unknown) {
            if (epoch !== this.providerEpoch) return;
            this.update((state) => ({
                ...state,
                providers: {
                    ...state.providers,
                    phase: 'error',
                    error: errorLabel(error),
                },
            }));
        }
    }

    async captureProviderCredential(target: CredentialTargetDto): Promise<boolean> {
        if (this.isRetainedLegacyConnectionCredentialTarget(target)) return false;
        try {
            const capture = await this.client.captureCredential(target);
            const status = await this.client.credentialStatus(target);
            this.update((state) => ({
                ...state,
                providers: {
                    ...state.providers,
                    workspace: {
                        ...state.providers.workspace,
                        credential_statuses: {
                            ...state.providers.workspace.credential_statuses,
                            [credentialKey(target)]: status.status,
                        },
                    },
                },
            }));
            this.announce(captureAnnouncement(capture, '운영체제 자격증명 저장소에 저장했습니다.'));
            return true;
        } catch (error: unknown) {
            this.announce(errorLabel(error));
            return false;
        }
    }

    async deleteProviderCredential(target: CredentialTargetDto): Promise<void> {
        if (this.isRetainedLegacyConnectionCredentialTarget(target)) return;
        try {
            await this.client.deleteCredential(target);
            this.update((state) => ({
                ...state,
                providers: {
                    ...state.providers,
                    workspace: {
                        ...state.providers.workspace,
                        credential_statuses: {
                            ...state.providers.workspace.credential_statuses,
                            [credentialKey(target)]: 'missing',
                        },
                    },
                },
            }));
            this.announce('저장된 자격증명을 삭제했습니다.');
        } catch (error: unknown) {
            this.announce(errorLabel(error));
        }
    }

    private isRetainedLegacyConnectionCredentialTarget(target: CredentialTargetDto): boolean {
        return (
            target.kind === 'connection' &&
            get(this.mutable).providers.workspace.legacy_profiles.some(
                (profile) => profile.id === target.connection_id,
            )
        );
    }

    async createProviderConnection(input: CreateProviderConnectionInput): Promise<boolean> {
        try {
            await this.client.createProviderConnection(input);
            await this.loadProviders();
            this.announce('프로바이더 연결을 만들었습니다.');
            return true;
        } catch (error: unknown) {
            this.announce(errorLabel(error));
            return false;
        }
    }

    async updateProviderConnection(input: UpdateProviderConnectionInput): Promise<boolean> {
        try {
            await this.client.upsertProviderConnection(input);
            await this.loadProviders();
            this.announce('프로바이더 연결을 수정했습니다.');
            return true;
        } catch (error: unknown) {
            this.announce(errorLabel(error));
            return false;
        }
    }

    async deleteProviderConnection(connectionId: string): Promise<boolean> {
        try {
            await this.client.deleteProviderConnection(connectionId);
            await this.loadProviders();
            this.announce('프로바이더 연결과 연결된 자격증명을 삭제했습니다.');
            return true;
        } catch (error: unknown) {
            this.announce(errorLabel(error));
            return false;
        }
    }

    async upsertProviderModelRoute(input: UpsertModelRouteInput): Promise<boolean> {
        try {
            await this.client.upsertModelRoute(input);
            await this.loadProviders();
            this.announce('모델 라우트를 저장했습니다.');
            return true;
        } catch (error: unknown) {
            this.announce(errorLabel(error));
            return false;
        }
    }

    async deleteProviderModelRoute(modelRouteId: string): Promise<boolean> {
        try {
            await this.client.deleteModelRoute(modelRouteId);
            await this.loadProviders();
            this.announce('모델 라우트를 삭제했습니다.');
            return true;
        } catch (error: unknown) {
            this.announce(errorLabel(error));
            return false;
        }
    }

    async upsertProviderGenerationPreset(input: GenerationPresetInput): Promise<boolean> {
        try {
            await this.client.upsertGenerationPreset(input);
            await this.loadProviders();
            this.announce('생성 프리셋을 저장했습니다.');
            return true;
        } catch (error: unknown) {
            this.announce(errorLabel(error));
            return false;
        }
    }

    async deleteProviderGenerationPreset(generationPresetId: string): Promise<boolean> {
        try {
            await this.client.deleteGenerationPreset(generationPresetId);
            await this.loadProviders();
            this.announce('생성 프리셋을 삭제했습니다.');
            return true;
        } catch (error: unknown) {
            this.announce(errorLabel(error));
            return false;
        }
    }

    async validateProviderGenerationPresetCandidate(
        input: GenerationPresetInput,
    ): Promise<boolean> {
        try {
            await this.client.validateGenerationPresetCandidate(input);
            this.announce('프리셋 후보가 유효합니다.');
            return true;
        } catch (error: unknown) {
            this.announce(errorLabel(error));
            return false;
        }
    }

    async previewProviderRequestCandidate(input: GenerationPresetInput): Promise<void> {
        try {
            const preview = await this.client.previewProviderRequestCandidate(input);
            this.updateProviderWorkspace((workspace) => ({
                ...workspace,
                request_preview: preview,
            }));
        } catch (error: unknown) {
            this.announce(errorLabel(error));
        }
    }

    async previewSelectedProviderRequest(): Promise<void> {
        const settings = get(this.mutable).providers.workspace.settings;
        if (
            settings.selected_model_route_id === null ||
            settings.selected_generation_preset_id === null
        ) {
            this.announce('저장된 기본 모델 라우트가 없습니다.');
            return;
        }
        try {
            const preview = await this.client.previewProviderRequest({
                model_route_id: settings.selected_model_route_id,
                generation_preset_id: settings.selected_generation_preset_id,
            });
            this.update((state) => ({
                ...state,
                providers: {
                    ...state.providers,
                    workspace: { ...state.providers.workspace, request_preview: preview },
                },
            }));
        } catch (error: unknown) {
            this.announce(errorLabel(error));
        }
    }

    private updateProviderWorkspace(
        updater: (workspace: ProviderWorkspaceDto) => ProviderWorkspaceDto,
    ): void {
        this.update((state) => ({
            ...state,
            providers: {
                ...state.providers,
                workspace: updater(state.providers.workspace),
            },
        }));
    }

    private storeProviderSettings(settings: AppSettingsDto): void {
        ++this.providerSettingsEpoch;
        this.updateProviderWorkspace((workspace) => ({ ...workspace, settings }));
    }

    private enqueueProviderSettingsMutation<T>(mutation: () => Promise<T>): Promise<T> {
        const pending = this.providerSettingsMutationTail.then(mutation);
        this.providerSettingsMutationTail = pending.then(
            () => undefined,
            () => undefined,
        );
        return pending;
    }

    private storeModelSyncJob(job: ModelSyncJobDto): void {
        this.updateProviderWorkspace((workspace) => ({
            ...workspace,
            model_sync_jobs: [
                job,
                ...workspace.model_sync_jobs.filter((candidate) => candidate.id !== job.id),
            ],
            selected_model_sync_job_id: job.id,
        }));
    }

    private storeDiscoverySession(session: ProviderDiscoverySessionDto): void {
        this.updateProviderWorkspace((workspace) => ({
            ...workspace,
            discoveries: [
                session,
                ...workspace.discoveries.filter((candidate) => candidate.id !== session.id),
            ],
            selected_discovery_id: session.id,
        }));
    }

    async loadProviderCapabilities(modelRouteId: string): Promise<void> {
        if (modelRouteId === '') return;
        try {
            const [observations, parameterSpecs] = await Promise.all([
                this.client.listCapabilityObservations(modelRouteId),
                this.client.effectiveParameterSpecs(modelRouteId),
            ]);
            this.updateProviderWorkspace((workspace) => ({
                ...workspace,
                selected_capability_model_route_id: modelRouteId,
                capability_observations: observations,
                capability_parameter_specs: parameterSpecs,
                effective_capability: null,
            }));
        } catch (error: unknown) {
            this.announce(errorLabel(error));
        }
    }

    async inspectEffectiveProviderCapability(key: CapabilityKeyInput): Promise<void> {
        const routeId = get(this.mutable).providers.workspace.selected_capability_model_route_id;
        if (routeId === null) return;
        try {
            const capability = await this.client.effectiveCapability(routeId, key);
            this.updateProviderWorkspace((workspace) => ({
                ...workspace,
                effective_capability: capability,
            }));
        } catch (error: unknown) {
            this.announce(errorLabel(error));
        }
    }

    async upsertProviderCapabilityOverride(input: UpsertCapabilityOverrideInput): Promise<boolean> {
        try {
            await this.client.upsertUserCapabilityOverride(input);
            await this.loadProviderCapabilities(input.model_route_id);
            this.announce('사용자 capability override를 저장했습니다.');
            return true;
        } catch (error: unknown) {
            this.announce(errorLabel(error));
            return false;
        }
    }

    async deleteProviderCapabilityOverride(observationId: string): Promise<void> {
        const routeId = get(this.mutable).providers.workspace.selected_capability_model_route_id;
        if (routeId === null) return;
        try {
            await this.client.deleteUserCapabilityOverride(routeId, observationId);
            await this.loadProviderCapabilities(routeId);
            this.announce('사용자 capability override를 삭제했습니다.');
        } catch (error: unknown) {
            this.announce(errorLabel(error));
        }
    }

    async selectProviderGenerationTarget(
        modelRouteId: string | null,
        generationPresetId: string | null,
    ): Promise<boolean> {
        if ((modelRouteId === null) !== (generationPresetId === null)) return false;
        return this.enqueueProviderSettingsMutation(async () => {
            try {
                const settings = await this.client.selectGenerationTarget(
                    modelRouteId === null || generationPresetId === null
                        ? null
                        : {
                              model_route_id: modelRouteId,
                              generation_preset_id: generationPresetId,
                          },
                );
                this.storeProviderSettings(settings);
                this.announce(
                    modelRouteId === null
                        ? '기본 생성 대상을 해제했습니다.'
                        : '기본 생성 대상을 저장했습니다.',
                );
                return true;
            } catch (error: unknown) {
                this.announce(errorLabel(error));
                return false;
            }
        });
    }

    async selectLegacyProviderProfile(profileId: string): Promise<boolean> {
        return this.enqueueProviderSettingsMutation(async () => {
            const workspace = get(this.mutable).providers.workspace;
            if (!workspace.legacy_profiles.some((profile) => profile.id === profileId))
                return false;
            try {
                const settings = await this.client.updateSettings({
                    ...workspace.settings,
                    selected_provider_profile_id: profileId,
                    selected_model_route_id: null,
                    selected_generation_preset_id: null,
                });
                this.storeProviderSettings(settings);
                this.announce('기존 프로바이더를 기본 대상으로 저장했습니다.');
                return true;
            } catch (error: unknown) {
                this.announce(errorLabel(error));
                return false;
            }
        });
    }

    async setPreservePartialGenerations(preserve: boolean): Promise<boolean> {
        return this.enqueueProviderSettingsMutation(async () => {
            const current = get(this.mutable).providers.workspace.settings;
            try {
                const settings = await this.client.updateSettings({
                    ...current,
                    preserve_partial_generations: preserve,
                });
                this.storeProviderSettings(settings);
                this.announce('부분 생성 보존 설정을 저장했습니다.');
                return true;
            } catch (error: unknown) {
                this.announce(errorLabel(error));
                return false;
            }
        });
    }

    async startProviderModelSync(connectionId: string): Promise<void> {
        try {
            const started = await this.client.startProviderModelSync(connectionId);
            await this.refreshProviderModelSync(started.job_id);
            this.announce('모델 동기화를 시작했습니다. 자동 승인하지 않습니다.');
        } catch (error: unknown) {
            this.announce(errorLabel(error));
        }
    }

    async refreshProviderModelSync(jobId: string): Promise<void> {
        try {
            const [job, events] = await Promise.all([
                this.client.getProviderModelSync(jobId),
                this.client.pollProviderModelSyncEvents(jobId, 100),
            ]);
            const latestEvent = events.at(-1) ?? null;
            for (const event of events) {
                await this.client.ackProviderModelSyncEvent(jobId, event.sequence);
            }
            this.storeModelSyncJob(job);
            this.updateProviderWorkspace((workspace) => ({
                ...workspace,
                model_sync_event: latestEvent ?? workspace.model_sync_event,
            }));
        } catch (error: unknown) {
            this.announce(errorLabel(error));
        }
    }

    async approveProviderModelSync(jobId: string): Promise<void> {
        const job = get(this.mutable).providers.workspace.model_sync_jobs.find(
            (candidate) => candidate.id === jobId,
        );
        if (job?.review === null || job?.review === undefined) return;
        try {
            this.storeModelSyncJob(
                await this.client.approveProviderModelSync(jobId, job.review.sha256),
            );
            await this.loadProviders();
            this.announce('검토한 정확한 모델 동기화 변경을 적용했습니다.');
        } catch (error: unknown) {
            this.announce(errorLabel(error));
        }
    }

    async cancelProviderModelSync(jobId: string): Promise<void> {
        try {
            this.storeModelSyncJob(await this.client.cancelProviderModelSync(jobId));
            this.announce('모델 동기화를 취소했습니다.');
        } catch (error: unknown) {
            this.announce(errorLabel(error));
        }
    }

    async beginProviderDiscovery(
        request:
            | { kind: 'site'; input: BeginProviderDiscoveryInput }
            | { kind: 'curl'; input: BeginProviderDiscoveryCurlInput },
    ): Promise<boolean> {
        try {
            let session: ProviderDiscoverySessionDto;
            let capture: NativeCaptureStatusDto | null = null;
            if (request.kind === 'site') {
                session = await this.client.beginProviderDiscovery(request.input);
            } else {
                const captured = await this.client.beginProviderDiscoveryCurl(request.input);
                session = captured.session;
                capture = captured.capture;
            }
            this.storeDiscoverySession(session);
            await this.refreshProviderDiscovery(session.id);
            await this.pollSelectedProviderDiscoveryEvents();
            this.announce(
                capture === null
                    ? '프로바이더 탐색을 시작했습니다.'
                    : captureAnnouncement(capture, '캡처한 cURL로 프로바이더 탐색을 시작했습니다.'),
            );
            return true;
        } catch (error: unknown) {
            this.announce(errorLabel(error));
            return false;
        }
    }

    async refreshProviderDiscovery(sessionId: string): Promise<void> {
        const requestEpoch = ++this.discoveryRequestEpoch;
        this.updateProviderWorkspace((workspace) => ({
            ...workspace,
            selected_discovery_id: sessionId,
        }));
        await this.refreshProviderDiscoveryAtEpoch(sessionId, requestEpoch);
    }

    private isCurrentDiscoveryRequest(sessionId: string, requestEpoch: number): boolean {
        return (
            requestEpoch === this.discoveryRequestEpoch &&
            get(this.mutable).providers.workspace.selected_discovery_id === sessionId
        );
    }

    private async refreshProviderDiscoveryAtEpoch(
        sessionId: string,
        requestEpoch: number,
    ): Promise<void> {
        try {
            const [
                session,
                candidates,
                evidence,
                approvals,
                review,
                approvalProposal,
                reviewProposal,
                assistantResumeBoundary,
            ] = await Promise.all([
                this.client.getProviderDiscovery(sessionId),
                this.client.listProviderDiscoveryCandidates(sessionId),
                this.client.listProviderDiscoveryEvidence(sessionId),
                this.client.listProviderDiscoveryApprovals(sessionId),
                this.client.getProviderDiscoveryReview(sessionId),
                this.client.getProviderDiscoveryApprovalProposal(sessionId),
                this.client.getProviderDiscoveryReviewProposal(sessionId),
                this.client.getProviderDiscoveryAssistantResumeBoundary(sessionId),
            ]);
            if (
                session.id !== sessionId ||
                !this.isCurrentDiscoveryRequest(sessionId, requestEpoch)
            ) {
                return;
            }
            const compensationSteps =
                session.commit_attempt_id === null
                    ? []
                    : await this.client.listProviderDiscoveryCompensationSteps(
                          session.commit_attempt_id,
                      );
            const credentialTarget = discoveryCredentialTarget(session);
            const credentialStatus =
                credentialTarget === null
                    ? null
                    : (await this.client.credentialStatus(credentialTarget)).status;
            if (!this.isCurrentDiscoveryRequest(sessionId, requestEpoch)) return;
            this.updateProviderWorkspace((workspace) => {
                const sessionCredentialKey = `discovery_session:${session.id}`;
                const credentialStatuses = Object.fromEntries(
                    Object.entries(workspace.credential_statuses).filter(
                        ([key]) => key !== sessionCredentialKey,
                    ),
                );
                if (credentialTarget !== null && credentialStatus !== null) {
                    credentialStatuses[credentialKey(credentialTarget)] = credentialStatus;
                }
                return {
                    ...workspace,
                    credential_statuses: credentialStatuses,
                    discoveries: [
                        session,
                        ...workspace.discoveries.filter((candidate) => candidate.id !== session.id),
                    ],
                    selected_discovery_id: session.id,
                    discovery_candidates: candidates,
                    discovery_evidence: evidence,
                    discovery_approvals: approvals,
                    discovery_review: review,
                    discovery_approval_proposal: approvalProposal,
                    discovery_review_proposal: reviewProposal,
                    discovery_assistant_resume_boundary: assistantResumeBoundary,
                    discovery_compensation_steps: compensationSteps,
                };
            });
        } catch (error: unknown) {
            if (!this.isCurrentDiscoveryRequest(sessionId, requestEpoch)) return;
            this.announce(errorLabel(error));
        }
    }

    private selectedProviderDiscoveryId(): string | null {
        return get(this.mutable).providers.workspace.selected_discovery_id;
    }

    async runProviderDiscoveryAssistant(): Promise<void> {
        const sessionId = this.selectedProviderDiscoveryId();
        if (sessionId === null) return;
        try {
            const hostAction = await this.client.runProviderDiscoveryAssistantTurn(sessionId);
            this.updateProviderWorkspace((workspace) => ({
                ...workspace,
                discovery_assistant_host_action: hostAction,
            }));
            await this.refreshProviderDiscovery(sessionId);
            this.announce('설정 도우미 결과가 검토 대기 상태로 도착했습니다.');
        } catch (error: unknown) {
            this.announce(errorLabel(error));
        }
    }

    async resumeProviderDiscoveryAssistantCoreHostAction(): Promise<void> {
        await this.mutateSelectedDiscoveryAssistant((sessionId) =>
            this.client.resumeProviderDiscoveryAssistantCoreHostAction(sessionId),
        );
    }

    async approveProviderDiscoveryAssistantRetry(): Promise<void> {
        await this.mutateSelectedDiscoveryAssistant((sessionId) =>
            this.client.approveProviderDiscoveryAssistantRetry(sessionId),
        );
    }

    async requestProviderDiscoveryAssistantRevision(): Promise<void> {
        await this.mutateSelectedDiscoveryAssistant((sessionId) =>
            this.client.requestProviderDiscoveryAssistantRevision(sessionId),
        );
    }

    async acceptProviderDiscoveryAssistantDraft(): Promise<void> {
        await this.mutateSelectedDiscoveryAssistant((sessionId) =>
            this.client.acceptProviderDiscoveryAssistantDraft(sessionId),
        );
    }

    async recordProviderDiscoveryAssistantFailure(
        kind: DiscoveryAssistantFailureKindInput,
        retryable: boolean,
    ): Promise<void> {
        await this.mutateSelectedDiscoveryAssistant((sessionId) =>
            this.client.recordProviderDiscoveryAssistantFailure(sessionId, kind, retryable),
        );
    }

    async interruptProviderDiscoveryAssistant(
        outcome: DiscoveryAssistantInterruptionOutcomeInput,
    ): Promise<void> {
        await this.mutateSelectedDiscoveryAssistant((sessionId) =>
            this.client.interruptProviderDiscoveryAssistant(sessionId, outcome),
        );
    }

    async restartProviderDiscoveryAssistantAfterInterruption(): Promise<void> {
        await this.mutateSelectedDiscoveryAssistant((sessionId) =>
            this.client.restartProviderDiscoveryAssistantAfterInterruption(sessionId),
        );
    }

    private async mutateSelectedDiscoveryAssistant(
        action: (sessionId: string) => Promise<ProviderDiscoverySessionDto>,
    ): Promise<void> {
        const sessionId = this.selectedProviderDiscoveryId();
        if (sessionId === null) return;
        try {
            this.storeDiscoverySession(await action(sessionId));
            await this.refreshProviderDiscovery(sessionId);
        } catch (error: unknown) {
            this.announce(errorLabel(error));
        }
    }

    async pollSelectedProviderDiscoveryEvents(): Promise<void> {
        const selectedId = get(this.mutable).providers.workspace.selected_discovery_id;
        if (selectedId === null) return;
        const requestEpoch = ++this.discoveryRequestEpoch;
        try {
            let latest = null as ProviderWorkspaceDto['discovery_event'];
            let acknowledgedCount = 0;
            let drained = false;
            while (acknowledgedCount < MAX_PROVIDER_DISCOVERY_EVENT_DRAIN) {
                if (!this.isCurrentDiscoveryRequest(selectedId, requestEpoch)) return;
                const remaining = MAX_PROVIDER_DISCOVERY_EVENT_DRAIN - acknowledgedCount;
                const events = await this.client.pollProviderDiscoveryEventsForSession(
                    selectedId,
                    remaining,
                );
                if (!this.isCurrentDiscoveryRequest(selectedId, requestEpoch)) return;
                if (events.some((item) => item.event.session_id !== selectedId)) {
                    throw new Error('session-filtered discovery poll returned a foreign event');
                }
                if (events.length > remaining) {
                    throw new Error('session-filtered discovery poll exceeded its requested limit');
                }
                if (events.length === 0) {
                    drained = true;
                    break;
                }
                for (const item of events) {
                    if (!this.isCurrentDiscoveryRequest(selectedId, requestEpoch)) return;
                    const acknowledged = await this.client.ackProviderDiscoveryEvent(item.event.id);
                    if (!this.isCurrentDiscoveryRequest(selectedId, requestEpoch)) return;
                    if (!acknowledged) {
                        throw new Error('provider discovery event acknowledgement was rejected');
                    }
                    latest = item.event;
                    acknowledgedCount += 1;
                }
            }
            if (latest !== null) {
                this.updateProviderWorkspace((workspace) => ({
                    ...workspace,
                    discovery_event: latest,
                }));
            }
            if (!this.isCurrentDiscoveryRequest(selectedId, requestEpoch)) return;
            await this.refreshProviderDiscoveryAtEpoch(selectedId, requestEpoch);
            if (!drained && this.isCurrentDiscoveryRequest(selectedId, requestEpoch)) {
                this.announce(
                    '탐색 이벤트가 너무 많이 쌓여 일부만 확인했습니다. 다시 확인해 주세요.',
                );
            }
        } catch (error: unknown) {
            if (!this.isCurrentDiscoveryRequest(selectedId, requestEpoch)) return;
            this.announce(errorLabel(error));
        }
    }

    async continueProviderDiscovery(
        action: ContinueProviderDiscoveryActionInput,
    ): Promise<boolean> {
        const workspace = get(this.mutable).providers.workspace;
        const session = workspace.discoveries.find(
            (candidate) => candidate.id === workspace.selected_discovery_id,
        );
        if (!session?.action_required) {
            this.announce('현재 탐색 상태에서 진행할 작업이 없습니다.');
            return false;
        }
        const actionId = globalThis.crypto.randomUUID();
        try {
            const next = await this.client.continueProviderDiscovery({
                session_id: session.id,
                action_id: actionId,
                expected_revision: session.revision,
                action,
            });
            this.storeDiscoverySession(next);
            await this.refreshProviderDiscovery(next.id);
            await this.pollSelectedProviderDiscoveryEvents();
            return true;
        } catch (error: unknown) {
            this.announce(errorLabel(error));
            return false;
        }
    }

    async supplyProviderDiscoveryDocumentEvidence(documentUrl: string): Promise<boolean> {
        const workspace = get(this.mutable).providers.workspace;
        const session = workspace.discoveries.find(
            (candidate) => candidate.id === workspace.selected_discovery_id,
        );
        if (session === undefined || documentUrl.trim() === '') return false;
        try {
            this.storeDiscoverySession(
                await this.client.supplyProviderDiscoveryDocumentEvidence(
                    session.id,
                    session.revision,
                    documentUrl.trim(),
                ),
            );
            await this.refreshProviderDiscovery(session.id);
            await this.pollSelectedProviderDiscoveryEvents();
            return true;
        } catch (error: unknown) {
            this.announce(errorLabel(error));
            return false;
        }
    }

    async supplyProviderDiscoveryCurlEvidence(): Promise<boolean> {
        const workspace = get(this.mutable).providers.workspace;
        const session = workspace.discoveries.find(
            (candidate) => candidate.id === workspace.selected_discovery_id,
        );
        if (session === undefined) return false;
        try {
            const captured = await this.client.supplyProviderDiscoveryCurlEvidence(
                session.id,
                session.revision,
            );
            this.storeDiscoverySession(captured.session);
            await this.refreshProviderDiscovery(session.id);
            await this.pollSelectedProviderDiscoveryEvents();
            this.announce(
                captureAnnouncement(captured.capture, '캡처한 cURL 근거를 추가했습니다.'),
            );
            return true;
        } catch (error: unknown) {
            this.announce(errorLabel(error));
            return false;
        }
    }

    async cancelProviderDiscovery(): Promise<void> {
        const workspace = get(this.mutable).providers.workspace;
        const session = workspace.discoveries.find(
            (candidate) => candidate.id === workspace.selected_discovery_id,
        );
        if (session === undefined) return;
        try {
            this.storeDiscoverySession(
                await this.client.cancelProviderDiscovery(session.id, session.revision),
            );
            await this.refreshProviderDiscovery(session.id);
            this.announce('프로바이더 탐색을 취소했습니다.');
        } catch (error: unknown) {
            this.announce(errorLabel(error));
        }
    }

    async commitProviderDiscovery(): Promise<boolean> {
        const sessionId = get(this.mutable).providers.workspace.selected_discovery_id;
        if (sessionId === null) return false;
        try {
            await this.client.commitProviderDiscovery(sessionId);
            await this.loadProviders();
            this.announce('검토·승인된 프로바이더 연결을 저장했습니다.');
            return true;
        } catch (error: unknown) {
            this.announce(errorLabel(error));
            return false;
        }
    }

    async recoverProviderDiscoveries(): Promise<void> {
        try {
            const results = await this.client.recoverProviderDiscovery();
            this.updateProviderWorkspace((workspace) => ({
                ...workspace,
                discovery_recovery_results: results,
            }));
            await this.loadProviders();
            this.announce(
                results.length === 0
                    ? '복구가 필요한 탐색 작업이 없습니다.'
                    : `${String(results.length)}개 탐색 작업을 복구했습니다.`,
            );
        } catch (error: unknown) {
            this.announce(errorLabel(error));
        }
    }

    async continueProviderDiscoveryCompensation(resume: boolean): Promise<void> {
        const sessionId = get(this.mutable).providers.workspace.selected_discovery_id;
        if (sessionId === null) return;
        try {
            const session = resume
                ? await this.client.resumeProviderDiscoveryCompensation(sessionId)
                : await this.client.continueProviderDiscoveryCompensation(sessionId);
            this.storeDiscoverySession(session);
            await this.refreshProviderDiscovery(sessionId);
        } catch (error: unknown) {
            this.announce(errorLabel(error));
        }
    }

    async pickProviderCatalogImport(): Promise<void> {
        try {
            const ticket = await this.client.pickProviderCatalogImport();
            if (ticket === null) return;
            this.updateProviderWorkspace((workspace) => ({
                ...workspace,
                pending_catalog_import: ticket,
            }));
            this.announce('서명된 카탈로그 변경 계획을 검토해 주세요.');
        } catch (error: unknown) {
            this.announce(errorLabel(error));
        }
    }

    async activateProviderCatalogImport(): Promise<void> {
        const ticket = get(this.mutable).providers.workspace.pending_catalog_import;
        if (ticket === null) return;
        try {
            const result = await this.client.activateProviderCatalogImport(ticket.ticket_id);
            this.updateProviderWorkspace((workspace) => ({
                ...workspace,
                catalog_status: result.status,
                pending_catalog_import: null,
                catalog_diff: result.diff,
            }));
            await this.loadProviders();
            this.announce('검토한 카탈로그 변경을 적용했습니다.');
        } catch (error: unknown) {
            this.announce(errorLabel(error));
        }
    }

    async discardProviderCatalogImport(): Promise<void> {
        const ticket = get(this.mutable).providers.workspace.pending_catalog_import;
        if (ticket === null) return;
        try {
            await this.client.discardProviderCatalogImport(ticket.ticket_id);
            this.updateProviderWorkspace((workspace) => ({
                ...workspace,
                pending_catalog_import: null,
            }));
            this.announce('카탈로그 가져오기 계획을 폐기했습니다.');
        } catch (error: unknown) {
            this.announce(errorLabel(error));
        }
    }

    async diffProviderCatalogRevisions(fromRevision: number, toRevision: number): Promise<void> {
        try {
            const catalogDiff = await this.client.diffProviderCatalogRevisions(
                fromRevision,
                toRevision,
            );
            this.updateProviderWorkspace((workspace) => ({
                ...workspace,
                catalog_diff: catalogDiff,
            }));
        } catch (error: unknown) {
            this.announce(errorLabel(error));
        }
    }

    async prepareProviderCatalogRollback(targetRevision: number): Promise<void> {
        try {
            const plan = await this.client.prepareProviderCatalogRollback(targetRevision);
            this.updateProviderWorkspace((workspace) => ({
                ...workspace,
                pending_catalog_rollback: plan,
                catalog_diff: plan.catalog_plan.diff,
            }));
            this.announce('정확한 롤백 계획을 검토해 주세요.');
        } catch (error: unknown) {
            this.announce(errorLabel(error));
        }
    }

    async activateProviderCatalogRollback(plan?: ProviderCatalogRollbackPlanDto): Promise<void> {
        const exactPlan = plan ?? get(this.mutable).providers.workspace.pending_catalog_rollback;
        if (exactPlan === null) return;
        try {
            const result = await this.client.activateProviderCatalogRollback(exactPlan);
            this.updateProviderWorkspace((workspace) => ({
                ...workspace,
                catalog_status: result.status,
                pending_catalog_rollback: null,
                catalog_diff: exactPlan.catalog_plan.diff,
            }));
            await this.loadProviders();
            this.announce('검토한 카탈로그 리비전으로 롤백했습니다.');
        } catch (error: unknown) {
            this.announce(errorLabel(error));
        }
    }

    private async disposeStream(streamId: string): Promise<void> {
        if (this.activeStreamId === streamId) this.activeStreamId = null;
        try {
            await this.client.disposeChatStream(streamId);
        } catch {
            // Receiver disposal is idempotent and must not mask the product action.
        }
    }

    private detachStream(): void {
        const streamId = this.activeStreamId;
        this.activeStreamId = null;
        ++this.streamEpoch;
        this.streamVerifier = null;
        this.reconcileInFlight = null;
        this.reconcileBufferedItems = [];
        this.cancelPendingDeltas();
        if (streamId !== null) void this.disposeStream(streamId);
    }

    destroy(): void {
        ++this.appEpoch;
        ++this.conversationEpoch;
        ++this.memoryQueryRetryEpoch;
        ++this.providerEpoch;
        ++this.discoveryRequestEpoch;
        this.memorySupervisorUnlisten?.();
        this.memorySupervisorUnlisten = null;
        this.detachStream();
    }
}

export type {
    GenerationPresetDto,
    ModelRouteDto,
    ProviderConnectionDto,
    ProviderProfileDto,
    ProviderTemplateDto,
};
