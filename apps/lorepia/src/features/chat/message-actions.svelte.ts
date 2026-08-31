import type { MessageDto } from '../../lib/ipc/contracts';
import type { ChatScrollLifecycle, MessageCollectionSnapshot } from './chat-scroll.svelte';

export class MessageActionsState {
    editingMessageId = $state<string | null>(null);
    editDraft = $state('');
    pendingRemoveId = $state<string | null>(null);
    activeMessageActionId = $state<string | null>(null);
    hoveredMessageActionId = $state<string | null>(null);

    constructor(private readonly chatScroll: ChatScrollLifecycle) {}

    syncCollection(messageCollection: MessageCollectionSnapshot): void {
        const activeMessageId = this.activeMessageActionId;
        if (
            activeMessageId !== null &&
            messageCollection.indexesById[activeMessageId] === undefined
        ) {
            this.activeMessageActionId = null;
            this.chatScroll.clearStableMessageActionLayout();
        }
    }

    resetTransientActions(): void {
        this.editingMessageId = null;
        this.pendingRemoveId = null;
    }

    handleMessageScrollPointerDown(event: PointerEvent): void {
        const target = event.target;
        if (target instanceof Element && target.closest('[data-message-id]') !== null) return;
        this.activeMessageActionId = null;
    }

    activate(messageId: string): void {
        this.chatScroll.stabilizeMessageActionLayout(messageId);
        this.activeMessageActionId = messageId;
    }

    hover(messageId: string, desktop: boolean): void {
        if (!desktop) return;
        this.hoveredMessageActionId = messageId;
    }

    unhover(messageId: string): void {
        if (this.hoveredMessageActionId === messageId) this.hoveredMessageActionId = null;
    }

    beginEdit(message: MessageDto): void {
        this.editingMessageId = message.id;
        this.editDraft = message.content;
        this.pendingRemoveId = null;
    }

    cancelEdit(): void {
        this.editingMessageId = null;
        this.editDraft = '';
    }

    finishEdit(): void {
        this.editingMessageId = null;
        this.editDraft = '';
    }

    requestRemove(messageId: string): void {
        this.pendingRemoveId = messageId;
    }

    cancelRemove(): void {
        this.pendingRemoveId = null;
    }

    confirmRemove(): void {
        this.pendingRemoveId = null;
    }
}
