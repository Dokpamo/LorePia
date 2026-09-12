import type { LorepiaAppController, LorepiaAppState } from '../app-controller';
import type { ChatSession } from '../../ui/workspace/chat-session';
import type { MessageHistoryController } from '../controllers/message-history-controller';
import { t } from '../../lib/i18n';

/** Delegates mutation, stream order, cancellation and reconciliation to the existing owner. */
export class LiveChatSession implements ChatSession {
    #mutating = $state(false);
    #submittingScope = $state<string | null>(null);
    drafts = $state<Record<string, string>>({});
    constructor(
        private readonly controller: LorepiaAppController,
        private readonly current: () => LorepiaAppState,
        private readonly hooks?: {
            onMutation: () => void;
            onRemoved: (scope: string) => void | Promise<void>;
        },
        private readonly history?: MessageHistoryController,
    ) {}
    get busy() {
        const state = this.current();
        return (
            this.#mutating ||
            state.chat.phase === 'loading' ||
            state.chat.active_generation_id !== null ||
            state.messages.phase !== 'ready'
        );
    }
    get canStop() {
        return this.current().chat.active_generation_id !== null;
    }
    get submitting() {
        return this.#submittingScope === this.scope;
    }
    get scope() {
        const state = this.current();
        return `${state.selected_conversation?.id ?? ''}:${state.conversation_state?.active_branch_id ?? ''}`;
    }
    get draft() {
        return this.drafts[this.scope] ?? '';
    }
    set draft(value: string) {
        this.drafts[this.scope] = value;
    }
    loadHistory(direction: 'older' | 'newer' | 'latest') {
        return this.history?.load(direction) ?? Promise.resolve();
    }
    branches() {
        return this.current().branches.map((item, index) => ({
            id: item.id,
            title: item.title ?? t('workspace.branch', { number: index + 1 }),
            messages: [],
        }));
    }
    async run<T>(operation: () => Promise<T>): Promise<T | undefined> {
        if (this.busy) return;
        this.#mutating = true;
        try {
            return await operation();
        } finally {
            this.#mutating = false;
            this.hooks?.onMutation();
        }
    }
    selectBranch(id: string) {
        return this.run(() => this.controller.selectBranch(id));
    }
    fork(id: string) {
        return this.run(() => this.controller.createBranch(id));
    }
    edit(id: string) {
        return async (value: string) =>
            (await this.run(() => this.controller.editUserMessage(id, value))) ?? false;
    }
    removeFrom(id: string) {
        return this.run(async () => {
            const result = await this.controller.removeMessage(id);
            if (result.mutationCommitted && result.scopeKey === this.scope)
                await this.hooks?.onRemoved(this.scope);
            return result;
        });
    }
    regenerate(id: string) {
        return this.run(() => this.controller.regenerateAssistantMessage(id));
    }
    stop() {
        return this.controller.cancelGeneration();
    }
    async send(dispatch: (text: string) => Promise<boolean | null>) {
        const scope = this.scope;
        const draft = this.draft;
        if (this.busy || !draft.trim()) return false;
        this.#submittingScope = scope;
        try {
            const accepted = await this.run(() => dispatch(draft));
            // A late acknowledgement must not erase another room's draft or newer typing.
            if (accepted && this.drafts[scope] === draft) this.drafts[scope] = '';
            return accepted === true;
        } finally {
            this.#submittingScope = null;
        }
    }
}
