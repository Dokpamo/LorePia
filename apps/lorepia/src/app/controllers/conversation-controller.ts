import type {
    CharacterDto,
    CharacterGreetingCatalogDto,
    ConversationDto,
    ConversationMode,
    MessageDto,
} from '../../lib/ipc/contracts';
import { t } from '../../lib/i18n';
import { INITIAL_APP_STATE, type LorepiaAppState } from '../app-state';
import { EpochGuard } from '../operations/epoch-guard';
import type { AppControllerContext } from './controller-context';

interface RemoveMessageResult {
    mutationCommitted: boolean;
    messagesRefreshed: boolean;
    scopeKey: string | null;
}

interface ConversationControllerHooks {
    detachStream(): void;
    invalidateMemoryQueryRetries(): void;
    refreshMemoryQueryRetries(): Promise<void>;
    resumePendingGeneration(messages: MessageDto[]): void;
    selectBranch(branchId: string): Promise<void>;
    activeBranchHead(state: LorepiaAppState): string | null;
}

function firstEnabledGreetingId(catalog: CharacterGreetingCatalogDto): string | null {
    return (
        catalog.greetings.find((greeting) => greeting.enabled && greeting.kind === 'default')?.id ??
        catalog.greetings.find((greeting) => greeting.enabled && greeting.kind === 'alternate')
            ?.id ??
        null
    );
}

export class ConversationController {
    private readonly epoch = new EpochGuard();

    constructor(
        private readonly context: AppControllerContext,
        private readonly hooks: ConversationControllerHooks,
    ) {}

    async selectCharacter(character: CharacterDto): Promise<void> {
        const epoch = this.epoch.advance();
        this.hooks.detachStream();
        this.context.update((state) => ({
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
                interrupted_jobs: [],
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
        const conversationsRequest = this.context.client
            .listConversations(character.id)
            .then((items) => {
                if (!this.epoch.isCurrent(epoch)) return;
                this.context.update((state) => ({
                    ...state,
                    conversations: { phase: 'ready', error: null, items },
                }));
            })
            .catch((error: unknown) => {
                if (!this.epoch.isCurrent(epoch)) return;
                this.context.update((state) => ({
                    ...state,
                    conversations: {
                        phase: 'error',
                        error: this.context.errorLabel(error),
                        items: [],
                    },
                }));
            });
        const greetingCatalogRequest = this.context.client
            .getCharacterGreetingCatalog(character.id)
            .then((catalog) => {
                if (!this.epoch.isCurrent(epoch)) return;
                if (catalog.character_id !== character.id) {
                    this.context.update((state) => ({
                        ...state,
                        greeting_catalog: {
                            phase: 'error',
                            error: t('chat.error.greeting_mismatch'),
                            value: null,
                            selected_greeting_id: null,
                        },
                    }));
                    return;
                }
                this.context.update((state) => ({
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
                if (!this.epoch.isCurrent(epoch)) return;
                this.context.update((state) => ({
                    ...state,
                    greeting_catalog: {
                        phase: 'error',
                        error: this.context.errorLabel(error),
                        value: null,
                        selected_greeting_id: null,
                    },
                }));
            });
        await Promise.all([conversationsRequest, greetingCatalogRequest]);
    }

    selectGreeting(greetingId: string): boolean {
        const state = this.context.readState();
        const catalog = state.greeting_catalog.value;
        if (
            state.greeting_catalog.phase !== 'ready' ||
            catalog?.greetings.some(
                (greeting) => greeting.id === greetingId && greeting.enabled,
            ) !== true
        ) {
            return false;
        }
        this.context.update((current) => ({
            ...current,
            greeting_catalog: {
                ...current.greeting_catalog,
                selected_greeting_id: greetingId,
            },
        }));
        return true;
    }

    async openNewConversation(): Promise<boolean> {
        const state = this.context.readState();
        const character = state.selected_character;
        const catalog = state.greeting_catalog.value;
        if (
            character === null ||
            state.greeting_catalog.phase !== 'ready' ||
            catalog?.character_id !== character.id
        ) {
            this.context.announce(t('chat.notice.greeting_revision'));
            return false;
        }
        const greetingId = state.greeting_catalog.selected_greeting_id;
        if (
            greetingId !== null &&
            !catalog.greetings.some((greeting) => greeting.id === greetingId && greeting.enabled)
        ) {
            this.context.announce(t('chat.notice.greeting_reselect'));
            return false;
        }
        const epoch = this.epoch.advance();
        try {
            const conversation = await this.context.client.createConversation(
                character.id,
                character.name,
                'chat',
                {
                    character_content_revision_id: catalog.character_content_revision_id,
                    greeting_id: greetingId,
                },
            );
            if (!this.epoch.isCurrent(epoch)) return false;
            this.context.update((current) => ({
                ...current,
                conversations: {
                    phase: 'ready',
                    error: null,
                    items: [
                        conversation,
                        ...current.conversations.items.filter(
                            (item) => item.id !== conversation.id,
                        ),
                    ],
                },
            }));
            this.prepareConversationLoad(conversation);
            return await this.loadPreparedConversation(conversation, epoch);
        } catch (error: unknown) {
            if (!this.epoch.isCurrent(epoch)) return false;
            this.context.update((current) => ({
                ...current,
                conversations: {
                    ...current.conversations,
                    phase: 'error',
                    error: this.context.errorLabel(error),
                },
            }));
            return false;
        }
    }

    async selectConversation(conversation: ConversationDto): Promise<boolean> {
        const epoch = this.epoch.advance();
        this.prepareConversationLoad(conversation);
        try {
            const opened = await this.context.client.openExistingConversation(conversation.id);
            if (!this.epoch.isCurrent(epoch)) return false;
            this.context.update((state) => ({
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
            if (!this.epoch.isCurrent(epoch)) return false;
            this.context.update((state) => ({
                ...state,
                messages: {
                    phase: 'error',
                    error: this.context.errorLabel(error),
                    items: [],
                },
            }));
            return false;
        }
    }

    async selectBranch(branchId: string): Promise<void> {
        const conversation = this.context.readState().selected_conversation;
        if (conversation === null) return;
        const epoch = this.epoch.advance();
        this.hooks.invalidateMemoryQueryRetries();
        this.hooks.detachStream();
        this.context.update((state) => ({
            ...state,
            messages: { ...state.messages, phase: 'loading', error: null },
            memory_query_retries: {
                phase: 'idle',
                error: null,
                candidates: [],
                interrupted_jobs: [],
                busy_id: null,
                notice: null,
            },
            chat: { ...INITIAL_APP_STATE.chat },
        }));
        try {
            const conversationState = await this.context.client.selectBranch(
                conversation.id,
                branchId,
            );
            const messages = await this.context.client.listBranchMessages(branchId);
            if (!this.epoch.isCurrent(epoch)) return;
            this.context.update((state) => ({
                ...state,
                conversation_state: conversationState,
                messages: { phase: 'ready', error: null, items: messages },
            }));
            void this.hooks.refreshMemoryQueryRetries();
            this.hooks.resumePendingGeneration(messages);
        } catch (error: unknown) {
            if (!this.epoch.isCurrent(epoch)) return;
            this.context.update((state) => ({
                ...state,
                messages: {
                    ...state.messages,
                    phase: 'error',
                    error: this.context.errorLabel(error),
                },
            }));
        }
    }

    async createBranch(fromMessageId: string | null): Promise<void> {
        const conversation = this.context.readState().selected_conversation;
        if (conversation === null) return;
        try {
            const branch = await this.context.client.createBranch(
                conversation.id,
                fromMessageId,
                null,
            );
            this.context.update((state) => ({
                ...state,
                branches: [branch, ...state.branches.filter((item) => item.id !== branch.id)],
            }));
            await this.hooks.selectBranch(branch.id);
            this.context.announce(t('chat.notice.branch_created'));
        } catch (error: unknown) {
            this.context.announce(this.context.errorLabel(error));
        }
    }

    async setConversationMode(mode: ConversationMode): Promise<void> {
        const conversation = this.context.readState().selected_conversation;
        if (conversation === null) return;
        try {
            const conversationState = await this.context.client.setConversationMode(
                conversation.id,
                mode,
            );
            this.context.update((state) => ({ ...state, conversation_state: conversationState }));
            this.context.announce(
                mode === 'chat' ? t('chat.notice.mode_chat') : t('chat.notice.mode_story'),
            );
        } catch (error: unknown) {
            this.context.announce(this.context.errorLabel(error));
        }
    }

    async removeMessage(messageId: string): Promise<RemoveMessageResult> {
        const state = this.context.readState();
        const conversation = state.selected_conversation;
        const branchId = state.conversation_state?.active_branch_id;
        if (conversation === null || branchId === undefined) {
            return { mutationCommitted: false, messagesRefreshed: false, scopeKey: null };
        }
        const conversationId = conversation.id;
        const scopeKey = `${conversationId}:${branchId}`;
        const expectedHead = this.hooks.activeBranchHead(state);
        const epoch = this.epoch.current();
        const isCurrentBranchSnapshot = (current: LorepiaAppState): boolean =>
            this.epoch.isCurrent(epoch) &&
            current.selected_conversation?.id === conversationId &&
            current.conversation_state?.active_branch_id === branchId &&
            this.hooks.activeBranchHead(current) === expectedHead;
        try {
            const branch = await this.context.client.removeMessageFromBranch({
                conversation_id: conversationId,
                branch_id: branchId,
                expected_head: expectedHead,
                message_id: messageId,
            });
            if (branch.id !== branchId || branch.conversation_id !== conversationId) {
                if (isCurrentBranchSnapshot(this.context.readState())) {
                    this.context.announce(t('chat.notice.remove_mismatch'));
                }
                return { mutationCommitted: false, messagesRefreshed: false, scopeKey };
            }
            if (!isCurrentBranchSnapshot(this.context.readState())) {
                return { mutationCommitted: true, messagesRefreshed: false, scopeKey };
            }
            try {
                const messages = await this.context.client.listBranchMessages(branchId);
                if (!isCurrentBranchSnapshot(this.context.readState())) {
                    return { mutationCommitted: true, messagesRefreshed: false, scopeKey };
                }
                this.context.update((current) => ({
                    ...current,
                    branches: current.branches.map((item) =>
                        item.id === branchId ? branch : item,
                    ),
                    messages: { phase: 'ready', error: null, items: messages },
                }));
                this.context.announce(t('chat.notice.removed'));
                return { mutationCommitted: true, messagesRefreshed: true, scopeKey };
            } catch (error: unknown) {
                if (isCurrentBranchSnapshot(this.context.readState())) {
                    const message = this.context.errorLabel(error);
                    this.context.update((current) => ({
                        ...current,
                        branches: current.branches.map((item) =>
                            item.id === branchId ? branch : item,
                        ),
                        messages: { ...current.messages, phase: 'error', error: message },
                    }));
                    this.context.announce(message);
                }
                return { mutationCommitted: true, messagesRefreshed: false, scopeKey };
            }
        } catch (error: unknown) {
            if (isCurrentBranchSnapshot(this.context.readState())) {
                this.context.announce(this.context.errorLabel(error));
            }
            return { mutationCommitted: false, messagesRefreshed: false, scopeKey };
        }
    }

    destroy(): void {
        this.epoch.advance();
    }

    private prepareConversationLoad(conversation: ConversationDto): void {
        this.hooks.invalidateMemoryQueryRetries();
        this.hooks.detachStream();
        this.context.update((state) => ({
            ...state,
            selected_conversation: conversation,
            conversation_state: null,
            branches: [],
            messages: { phase: 'loading', error: null, items: [] },
            memory_query_retries: {
                phase: 'idle',
                error: null,
                candidates: [],
                interrupted_jobs: [],
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
                this.context.client.getConversationState(conversation.id),
                this.context.client.listBranches(conversation.id),
            ]);
            const messages = await this.context.client.listBranchMessages(
                conversationState.active_branch_id,
            );
            if (!this.epoch.isCurrent(epoch)) return false;
            this.context.update((state) => ({
                ...state,
                selected_conversation: conversation,
                conversation_state: conversationState,
                branches,
                messages: { phase: 'ready', error: null, items: messages },
            }));
            void this.hooks.refreshMemoryQueryRetries();
            this.hooks.resumePendingGeneration(messages);
            return true;
        } catch (error: unknown) {
            if (!this.epoch.isCurrent(epoch)) return false;
            this.context.update((state) => ({
                ...state,
                messages: {
                    phase: 'error',
                    error: this.context.errorLabel(error),
                    items: [],
                },
            }));
            return false;
        }
    }
}
