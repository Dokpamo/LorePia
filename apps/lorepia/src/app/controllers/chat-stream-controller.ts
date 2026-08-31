import type { ChatEventDto, ChatStreamItemDto, MessageDto } from '../../lib/ipc/contracts';
import { t } from '../../lib/i18n';
import { normalizeClientError } from '../../lib/ipc/errors';
import { ChatStreamVerifier, type ChatStreamExpectation } from '../../features/chat/chat-stream';
import type { ChatState } from '../app-state';
import { EpochGuard } from '../operations/epoch-guard';
import type { AppControllerContext } from './controller-context';

interface ChatStreamControllerHooks {
    refreshMemoryQueryRetries(): Promise<void>;
}

const GENERATION_REATTACHMENT_UNAVAILABLE_MESSAGE = t('app.error.stream_lost');

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

export class ChatStreamController {
    private readonly streamEpoch = new EpochGuard();
    private reconcileInFlight: symbol | null = null;
    private reconcileBufferedItems: ChatStreamItemDto[] = [];
    private streamVerifier: ChatStreamVerifier | null = null;
    private activeStreamId: string | null = null;
    private deltaFlushTimer: ReturnType<typeof setTimeout> | null = null;
    private pendingTextDelta = '';
    private pendingReasoningDelta = '';

    constructor(
        private readonly context: AppControllerContext,
        private readonly hooks: ChatStreamControllerHooks,
    ) {}

    isEpochCurrent(epoch: number): boolean {
        return this.streamEpoch.isCurrent(epoch);
    }

    hasActiveStream(streamId: string): boolean {
        return this.activeStreamId === streamId;
    }

    bindGeneration(generationId: string): boolean {
        return this.streamVerifier?.bindGeneration(generationId) ?? false;
    }

    lastSequence(): number {
        return this.streamVerifier?.getLastSequence() ?? 0;
    }

    installVerifier(expectation: ChatStreamExpectation): void {
        this.streamVerifier = new ChatStreamVerifier(expectation);
    }

    beginStreamReceiver(): { epoch: number; streamId: string } {
        this.detachStream();
        const streamId = this.activateStreamReceiver();
        return { epoch: this.streamEpoch.current(), streamId };
    }

    private activateStreamReceiver(): string {
        const streamId = globalThis.crypto.randomUUID();
        this.activeStreamId = streamId;
        return streamId;
    }

    prepareStream(
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

    setChatLoading(generationId: string | null = null): void {
        this.context.update((state) => ({
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

    failStream(epoch: number, streamId: string, error: unknown): void {
        void this.disposeStream(streamId);
        if (!this.streamEpoch.isCurrent(epoch)) return;
        this.cancelPendingDeltas();
        this.context.update((state) => ({
            ...state,
            chat: {
                ...state.chat,
                phase: 'error',
                error: this.context.errorLabel(error),
                active_generation_id: null,
                live_assistant_message_id: null,
            },
        }));
        void this.hooks.refreshMemoryQueryRetries();
    }

    resumePendingGeneration(messages: MessageDto[]): void {
        const pending = this.pendingAssistantMessage(messages);
        if (pending?.generation_id === null || pending?.generation_id === undefined) return;
        const state = this.context.readState();
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
            await this.context.client.subscribeGeneration(
                generationId,
                conversationId,
                branchId,
                sequenceBaseline,
                streamId,
                (item) => {
                    if (!this.streamEpoch.isCurrent(epoch) || this.activeStreamId !== streamId) {
                        void this.disposeStream(streamId);
                        return;
                    }
                    if (ready) this.acceptStreamItem(item, epoch, streamId);
                    else buffered.push(item);
                },
            );
            if (!this.streamEpoch.isCurrent(epoch) || this.activeStreamId !== streamId) {
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
            this.context.update((state) => ({
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
                if (!this.streamEpoch.isCurrent(epoch) || this.activeStreamId !== streamId) break;
                this.acceptStreamItem(item, epoch, streamId);
            }
            return true;
        } catch (error: unknown) {
            void this.disposeStream(streamId);
            if (!this.streamEpoch.isCurrent(epoch)) return false;
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
            if (!this.streamEpoch.isCurrent(epoch)) return false;
            this.streamVerifier = null;
            this.cancelPendingDeltas();
            this.context.update((state) => ({
                ...state,
                chat: {
                    ...reattachmentUnavailableChatState(generationId),
                    error: this.context.errorLabel(normalized),
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
        if (this.context.readState().selected_conversation?.id !== conversationId) return false;
        try {
            const conversationState =
                await this.context.client.getConversationState(conversationId);
            const [branches, messages] = await Promise.all([
                this.context.client.listBranches(conversationId),
                this.context.client.listBranchMessages(conversationState.active_branch_id),
            ]);
            if (
                !this.streamEpoch.isCurrent(epoch) ||
                this.context.readState().selected_conversation?.id !== conversationId
            ) {
                return false;
            }
            if (this.pendingAssistantMessage(messages, generationId) !== null) return false;
            this.streamVerifier = null;
            this.reconcileBufferedItems = [];
            this.cancelPendingDeltas();
            this.context.update((state) => ({
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
            this.context.announce(t('chat.notice.synced'));
            return true;
        } catch {
            return false;
        }
    }

    pendingAssistantMessage(messages: MessageDto[], generationId?: string): MessageDto | null {
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

    acceptStreamItem(item: ChatStreamItemDto, epoch: number, streamId: string): void {
        if (this.reconcileInFlight !== null) {
            if (this.streamEpoch.isCurrent(epoch) && this.activeStreamId === streamId) {
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
                this.context.readState().messages.items,
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
            this.context.update((state) => ({
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
        this.context.update((state) => {
            const chat = { ...state.chat, active_generation_id: event.generation_id };
            switch (event.kind.type) {
                case 'generation_started':
                    chat.phase = 'ready';
                    break;
                case 'usage_updated': {
                    const output = event.kind.payload.output_tokens;
                    chat.usage_label =
                        output === null
                            ? null
                            : t('chat.usage.output_tokens', { count: output.toLocaleString() });
                    break;
                }
                case 'message_committed':
                    chat.reconcile_notice = t('chat.notice.reconciling');
                    break;
                case 'tool_call_started':
                    chat.reconcile_notice = t('chat.notice.tool_suggested');
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
        if (!this.streamEpoch.isCurrent(epoch)) {
            this.pendingTextDelta = '';
            this.pendingReasoningDelta = '';
            return;
        }
        const text = this.pendingTextDelta;
        const reasoning = this.pendingReasoningDelta;
        this.pendingTextDelta = '';
        this.pendingReasoningDelta = '';
        if (text === '' && reasoning === '') return;
        this.context.update((state) => ({
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

    async reconcile(
        generationId: string,
        epoch: number,
        streamId: string,
        reason: string,
        sequenceBaseline: number,
    ): Promise<void> {
        if (this.reconcileInFlight !== null) return;
        const conversation = this.context.readState().selected_conversation;
        if (conversation === null) {
            void this.disposeStream(streamId);
            return;
        }
        const reconciliation = Symbol('generation-reconciliation');
        this.reconcileInFlight = reconciliation;
        this.reconcileBufferedItems = [];
        this.context.update((state) => ({
            ...state,
            chat: {
                ...state.chat,
                reconcile_notice: t('chat.notice.stream_recovering', { reason }),
            },
        }));
        try {
            await this.disposeStream(streamId);
            if (!this.streamEpoch.isCurrent(epoch)) return;
            const conversationState = await this.context.client.getConversationState(
                conversation.id,
            );
            const [branches, messages] = await Promise.all([
                this.context.client.listBranches(conversation.id),
                this.context.client.listBranchMessages(conversationState.active_branch_id),
            ]);
            if (!this.streamEpoch.isCurrent(epoch)) return;
            const pendingAssistant = this.pendingAssistantMessage(messages, generationId);
            this.streamVerifier = null;
            this.context.update((state) => ({
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
                              reconcile_notice: t('chat.notice.stream_reconnecting'),
                          },
            }));
            if (pendingAssistant === null) {
                this.reconcileBufferedItems = [];
                this.context.announce(t('chat.notice.synced'));
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
            if (!this.streamEpoch.isCurrent(epoch)) return;
            this.context.update((state) => ({
                ...state,
                chat: {
                    ...state.chat,
                    phase: 'error',
                    error: this.context.errorLabel(error),
                    live_assistant_message_id: null,
                    streaming_text: '',
                    reasoning_text: '',
                    reconcile_notice: t('chat.notice.refresh_needed'),
                },
            }));
        } finally {
            if (this.reconcileInFlight === reconciliation) this.reconcileInFlight = null;
        }
    }

    async disposeStream(streamId: string): Promise<void> {
        if (this.activeStreamId === streamId) this.activeStreamId = null;
        try {
            await this.context.client.disposeChatStream(streamId);
        } catch {
            // Receiver disposal is idempotent and must not mask the product action.
        }
    }

    detachStream(): void {
        const streamId = this.activeStreamId;
        this.activeStreamId = null;
        this.streamEpoch.advance();
        this.streamVerifier = null;
        this.reconcileInFlight = null;
        this.reconcileBufferedItems = [];
        this.cancelPendingDeltas();
        if (streamId !== null) void this.disposeStream(streamId);
    }
}
