import type { LorepiaAppController, LorepiaAppState } from '../app-controller';
import type { ChatSession } from '../../ui/workspace/chat-session';
import { conversationView } from './workspace-projection';

/** Delegates mutation, stream order, cancellation and reconciliation to the existing owner. */
export class LiveChatSession implements ChatSession {
    #mutating = $state(false);
    drafts = $state<Record<string, string>>({});
    constructor(
        private readonly controller: LorepiaAppController,
        private readonly current: () => LorepiaAppState,
        private readonly hooks?: { onMutation: () => void; onRemoved: (scope: string) => void },
    ) {}
    get busy() {
        const state = this.current();
        return (
            this.#mutating ||
            state.chat.phase === 'loading' ||
            state.chat.active_generation_id !== null ||
            state.messages.phase === 'loading'
        );
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
    branches() {
        return conversationView(this.current())?.branches ?? [];
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
                this.hooks?.onRemoved(this.scope);
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
        if (!draft.trim()) return false;
        const accepted = await this.run(() => dispatch(draft));
        // A late acknowledgement must not erase another room's draft or newer typing.
        if (accepted && this.drafts[scope] === draft) this.drafts[scope] = '';
        return accepted === true;
    }
}
