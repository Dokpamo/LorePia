import { t } from '../../lib/i18n';
import type { SampleBranch, SampleConversation, SampleMessage } from './sample-data';

/** Local preview interactions only. No model, IPC or persisted conversation writes. */
export class SampleChatSession {
    busy = $state(false);
    #sequence = 0;
    #timer: ReturnType<typeof setTimeout> | undefined;
    #run: { owner: SampleConversation; branch: string; message: SampleMessage } | undefined;

    constructor(private readonly current: () => SampleConversation) {}

    #id() {
        return `ui-message-${String(++this.#sequence)}`;
    }

    #replace(owner: SampleConversation, messages: SampleMessage[]) {
        owner.messages = messages;
        const branch = owner.branches?.find((item) => item.id === owner.activeBranchId);
        if (branch) branch.messages = messages;
    }

    branches(): SampleBranch[] {
        const owner = this.current();
        return (
            owner.branches ?? [
                { id: 'main', title: t('uiPreview.originalBranch'), messages: owner.messages },
            ]
        );
    }

    selectBranch(id: string) {
        const owner = this.current();
        const branch = owner.branches?.find((item) => item.id === id);
        if (!branch || this.busy) return;
        owner.activeBranchId = id;
        owner.messages = branch.messages;
    }

    fork(id: string, before = false): string | undefined {
        if (this.busy) return;
        const owner = this.current();
        const index = owner.messages.findIndex((item) => item.id === id);
        if (index < 0) return;
        owner.branches ??= [
            { id: 'main', title: t('uiPreview.originalBranch'), messages: owner.messages },
        ];
        const branch: SampleBranch = {
            id: `ui-branch-${String(++this.#sequence)}`,
            title: t('uiPreview.branchNumber', { number: owner.branches.length }),
            messages: owner.messages.slice(0, index + (before ? 0 : 1)),
        };
        owner.branches = [...owner.branches, branch];
        owner.activeBranchId = branch.id;
        owner.messages = branch.messages;
        return branch.id;
    }

    edit(id: string): (text: string) => void {
        const owner = this.current();
        const sourceBranch = owner.activeBranchId ?? 'main';
        const original = owner.messages.find((item) => item.id === id);
        let branch: string | undefined;
        let editedId: string | undefined;
        return (text) => {
            if (
                !original ||
                !text.trim() ||
                this.current() !== owner ||
                this.busy ||
                (!branch && (owner.activeBranchId ?? 'main') !== sourceBranch) ||
                (!branch && text === original.text)
            )
                return;
            if (!branch) {
                branch = this.fork(id);
                if (!branch) return;
                editedId = this.#id();
            }
            if (owner.activeBranchId !== branch) return;
            this.#replace(owner, [
                ...owner.messages.slice(0, -1),
                { ...original, id: editedId ?? original.id, text, status: 'complete' },
            ]);
        };
    }

    removeFrom(id: string) {
        if (this.busy) return;
        const owner = this.current();
        const index = owner.messages.findIndex((item) => item.id === id);
        if (index >= 0) this.#replace(owner, owner.messages.slice(0, index));
    }

    send(text: string): boolean {
        if (this.busy || !text.trim()) return false;
        const owner = this.current();
        this.#replace(owner, [
            ...owner.messages,
            { id: this.#id(), role: 'user', text: text.trim() },
        ]);
        this.#reply();
        return true;
    }

    regenerate(id: string, retry = false) {
        if (this.fork(id, true)) this.#reply(retry);
    }

    #reply(retry = false) {
        const owner = this.current();
        const message: SampleMessage = {
            id: this.#id(),
            role: 'assistant',
            text: '',
            status: 'pending',
            sample: true,
        };
        this.#replace(owner, [...owner.messages, message]);
        // Read the reactive proxy, so each streamed update reaches the view.
        const reactiveMessage = owner.messages.at(-1);
        if (!reactiveMessage) return;
        const run = { owner, branch: owner.activeBranchId ?? 'main', message: reactiveMessage };
        this.#run = run;
        this.busy = true;
        const reply = Array.from(
            t(owner.mode === 'story' ? 'uiPreview.sampleStory' : 'uiPreview.sampleReply'),
        );
        let cursor = 0;
        const step = () => {
            if (this.#run !== run) return;
            if (this.current() !== owner || (owner.activeBranchId ?? 'main') !== run.branch) {
                this.stop();
                return;
            }
            cursor = Math.min(reply.length, cursor + 3);
            run.message.text = reply.slice(0, cursor).join('');
            const failed = !retry && owner.responsePreview === 'failed' && cursor >= 18;
            if (failed || cursor >= reply.length) {
                run.message.status = failed ? 'failed' : 'complete';
                this.busy = false;
                this.#timer = undefined;
                this.#run = undefined;
            } else this.#timer = setTimeout(step, 35);
        };
        this.#timer = setTimeout(step, !retry && owner.responsePreview === 'slow' ? 12000 : 280);
    }

    stop() {
        if (this.#timer !== undefined) clearTimeout(this.#timer);
        this.#timer = undefined;
        if (this.#run) this.#run.message.status = 'cancelled';
        this.#run = undefined;
        this.busy = false;
    }

    dispose() {
        this.stop();
    }
}
