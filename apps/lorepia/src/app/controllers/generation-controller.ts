import type {
    ChatStreamItemDto,
    GenerationSelectionInput,
    GenerationTargetDto,
    MessageActionGenerationDto,
    OrchestrationVariableMapDto,
    ReviewedPromptSendInput,
} from '../../lib/ipc/contracts';
import { t } from '../../lib/i18n';
import { normalizeClientError } from '../../lib/ipc/errors';
import type { LorepiaAppState } from '../app-state';
import {
    GenerationOperationIdentityAuthority,
    generationOperationIdentity,
    generationSelectionOperationIdentity,
    type GenerationOperationInputAuthority,
} from '../operations/operation-identity';
import type { ChatStreamController } from './chat-stream-controller';
import type { AppControllerContext } from './controller-context';

interface GenerationControllerHooks {
    clearMemoryQueryRetryNotice(): void;
    invalidateMemoryQueryRetries(): void;
    refreshMemoryQueryRetries(): Promise<void>;
    activeBranchHead(state: LorepiaAppState): string | null;
}

export class GenerationController {
    private readonly generationOperations = new GenerationOperationIdentityAuthority();
    private roomGenerationTarget: {
        conversation_id: string;
        branch_id: string;
        /** undefined means the exact room target is still loading. */
        target: GenerationTargetDto | null | undefined;
    } | null = null;

    constructor(
        private readonly context: AppControllerContext,
        private readonly stream: ChatStreamController,
        private readonly hooks: GenerationControllerHooks,
    ) {}

    beginNewGenerationOperation(): void {
        this.generationOperations.beginNewOperation();
    }

    stageGenerationAttemptRetry(generationAttemptId: string): boolean {
        return this.generationOperations.stageAttemptRetry(generationAttemptId);
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

    runtimeGenerationSelection(): GenerationSelectionInput | null {
        const selection = this.generationSelection(this.context.readState());
        return selection === null ? null : structuredClone(selection);
    }

    async sendMessage(
        content: string,
        variableOverrides: OrchestrationVariableMapDto = { values: [] },
    ): Promise<boolean> {
        const state = this.context.readState();
        if (state.chat.active_generation_id !== null) {
            this.context.announce(t('chat.notice.cancel_before_send'));
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
            this.context.announce(t('chat.notice.check_model'));
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
            JSON.stringify(variableOverrides),
        ]);
        const operationContext = this.generationOperations.context(operationIdentity);
        this.hooks.clearMemoryQueryRetryNotice();
        const { epoch, streamId } = this.stream.prepareStream(
            conversation.id,
            conversationState.active_branch_id,
        );
        const buffered: ChatStreamItemDto[] = [];
        let ready = false;
        try {
            const started = await this.context.client.sendMessage(
                {
                    conversation_id: conversation.id,
                    branch_id: conversationState.active_branch_id,
                    expected_head: expectedHead,
                    mode: conversationState.selected_mode,
                    text,
                    selection,
                    ...(variableOverrides.values.length === 0
                        ? {}
                        : { variable_overrides: structuredClone(variableOverrides) }),
                    ...this.generationOperations.input(operationContext),
                },
                streamId,
                (item) => {
                    if (
                        !this.stream.isEpochCurrent(epoch) ||
                        !this.stream.hasActiveStream(streamId)
                    ) {
                        void this.stream.disposeStream(streamId);
                        return;
                    }
                    if (ready) this.stream.acceptStreamItem(item, epoch, streamId);
                    else buffered.push(item);
                },
            );
            if (!this.stream.isEpochCurrent(epoch) || !this.stream.hasActiveStream(streamId)) {
                void this.stream.disposeStream(streamId);
                return false;
            }
            if (!this.stream.bindGeneration(started.generation_id)) {
                await this.stream.reconcile(
                    started.generation_id,
                    epoch,
                    streamId,
                    'generation mismatch',
                    this.stream.lastSequence(),
                );
                return false;
            }
            this.context.update((current) => ({
                ...current,
                chat: {
                    ...current.chat,
                    phase: 'ready',
                    active_generation_id: started.generation_id,
                },
            }));
            ready = true;
            for (const item of buffered) {
                if (!this.stream.isEpochCurrent(epoch) || !this.stream.hasActiveStream(streamId))
                    break;
                this.stream.acceptStreamItem(item, epoch, streamId);
            }
            this.generationOperations.complete(operationContext);
            return true;
        } catch (error: unknown) {
            this.stream.failStream(epoch, streamId, error);
            return false;
        }
    }

    async sendReviewedPrompt(input: ReviewedPromptSendInput): Promise<boolean> {
        const state = this.context.readState();
        if (state.chat.active_generation_id !== null) {
            this.context.announce(t('chat.notice.cancel_before_plan'));
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
            this.context.announce(t('chat.notice.plan_stale'));
            return false;
        }

        this.hooks.clearMemoryQueryRetryNotice();
        const { epoch, streamId } = this.stream.prepareStream(
            input.conversation_id,
            input.branch_id,
        );
        const buffered: ChatStreamItemDto[] = [];
        let ready = false;
        try {
            const started = await this.context.client.sendReviewedPrompt(
                input,
                streamId,
                (item) => {
                    if (
                        !this.stream.isEpochCurrent(epoch) ||
                        !this.stream.hasActiveStream(streamId)
                    ) {
                        void this.stream.disposeStream(streamId);
                        return;
                    }
                    if (ready) this.stream.acceptStreamItem(item, epoch, streamId);
                    else buffered.push(item);
                },
            );
            if (!this.stream.isEpochCurrent(epoch) || !this.stream.hasActiveStream(streamId)) {
                void this.stream.disposeStream(streamId);
                return false;
            }
            if (!this.stream.bindGeneration(started.generation_id)) {
                await this.stream.reconcile(
                    started.generation_id,
                    epoch,
                    streamId,
                    'generation mismatch',
                    this.stream.lastSequence(),
                );
                return false;
            }
            this.context.update((current) => ({
                ...current,
                chat: {
                    ...current.chat,
                    phase: 'ready',
                    active_generation_id: started.generation_id,
                },
            }));
            ready = true;
            for (const item of buffered) {
                if (!this.stream.isEpochCurrent(epoch) || !this.stream.hasActiveStream(streamId))
                    break;
                this.stream.acceptStreamItem(item, epoch, streamId);
            }
            return true;
        } catch (error: unknown) {
            this.stream.failStream(epoch, streamId, error);
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
                return this.context.client.editUserMessage(
                    {
                        conversation_id: conversationId,
                        branch_id: branchId,
                        expected_head: this.hooks.activeBranchHead(state),
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
                return this.context.client.regenerateAssistantMessage(
                    {
                        conversation_id: conversationId,
                        branch_id: branchId,
                        expected_head: this.hooks.activeBranchHead(state),
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
            operationAuthority: GenerationOperationInputAuthority,
            streamId: string,
            onItem: (item: ChatStreamItemDto) => void,
        ) => Promise<MessageActionGenerationDto> | null,
    ): Promise<boolean> {
        const state = this.context.readState();
        if (state.chat.active_generation_id !== null) {
            this.context.announce(t('chat.notice.cancel_before_edit'));
            return false;
        }
        const conversation = state.selected_conversation;
        const selection = this.generationSelection(state);
        if (conversation === null || state.conversation_state === null || selection === null) {
            this.context.announce(t('chat.notice.check_model_first'));
            return false;
        }

        const operationIdentity = generationOperationIdentity([
            action,
            conversation.id,
            state.conversation_state.active_branch_id,
            this.hooks.activeBranchHead(state),
            messageId,
            replacementText,
            ...generationSelectionOperationIdentity(selection),
        ]);
        const operationContext = this.generationOperations.context(operationIdentity);

        this.hooks.clearMemoryQueryRetryNotice();
        const { epoch, streamId } = this.stream.beginStreamReceiver();
        const buffered: ChatStreamItemDto[] = [];
        let ready = false;
        this.stream.setChatLoading();
        try {
            const started = await start(
                state,
                selection,
                this.generationOperations.input(operationContext),
                streamId,
                (item) => {
                    if (
                        !this.stream.isEpochCurrent(epoch) ||
                        !this.stream.hasActiveStream(streamId)
                    ) {
                        void this.stream.disposeStream(streamId);
                        return;
                    }
                    if (ready) this.stream.acceptStreamItem(item, epoch, streamId);
                    else buffered.push(item);
                },
            );
            if (
                started === null ||
                !this.stream.isEpochCurrent(epoch) ||
                !this.stream.hasActiveStream(streamId)
            ) {
                void this.stream.disposeStream(streamId);
                return false;
            }

            const conversationState = await this.context.client.selectBranch(
                conversation.id,
                started.branch.id,
            );
            const messages = await this.context.client.listBranchMessages(started.branch.id);
            if (!this.stream.isEpochCurrent(epoch) || !this.stream.hasActiveStream(streamId)) {
                void this.stream.disposeStream(streamId);
                return false;
            }
            const pendingAssistant = this.stream.pendingAssistantMessage(
                messages,
                started.generation_id,
            );
            this.hooks.invalidateMemoryQueryRetries();
            this.stream.installVerifier({
                conversationId: conversation.id,
                branchId: started.branch.id,
                generationId: started.generation_id,
                assistantMessageId: pendingAssistant?.id,
            });
            this.context.update((current) => ({
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
                    interrupted_jobs: [],
                    busy_id: null,
                    notice: null,
                },
                chat: {
                    ...current.chat,
                    phase: 'ready',
                    active_generation_id: started.generation_id,
                },
            }));
            void this.hooks.refreshMemoryQueryRetries();
            ready = true;
            for (const item of buffered) this.stream.acceptStreamItem(item, epoch, streamId);
            this.generationOperations.complete(operationContext);
            return true;
        } catch (error: unknown) {
            this.stream.failStream(epoch, streamId, error);
            return false;
        }
    }

    async cancelGeneration(): Promise<void> {
        const generationId = this.context.readState().chat.active_generation_id;
        if (generationId === null) return;
        try {
            await this.context.client.cancelGeneration(generationId);
            this.context.announce(t('chat.notice.cancel_requested'));
        } catch (error: unknown) {
            const normalized = normalizeClientError(error);
            if (normalized.code !== 'not_found' && normalized.code !== 'cancelled') {
                this.context.announce(this.context.errorLabel(normalized));
            }
        }
    }
}
