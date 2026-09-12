import { t } from '../../lib/i18n';
import type { LorepiaAppState } from '../app-state';
import type { MessageDto } from '../../lib/ipc/contracts';
import type { ChatDisplayMode } from '../../lib/chat-display';
import { formatMessageDay } from '../../features/chat/chat-scroll.svelte';
import type {
    SampleCharacter,
    SampleConversation,
    SampleMessage,
} from '../../ui/workspace/view-types';

export function messageView(message: MessageDto): SampleMessage {
    return {
        id: message.id,
        role: message.role,
        text: message.content,
        status:
            message.status === 'pending' ||
            message.status === 'failed' ||
            message.status === 'cancelled'
                ? message.status
                : 'complete',
        source: message,
    };
}

export function conversationView(
    state: LorepiaAppState,
    displayMode?: ChatDisplayMode,
): SampleConversation | null {
    return new ConversationProjection().project(state, displayMode);
}

/** Per-workspace cache. Stream text never rebuilds saved views or scroll indexes. */
export class ConversationProjection {
    private source: MessageDto[] | null = null;
    private liveId: string | null = null;
    private conversationId: string | null = null;
    private messages: SampleMessage[] = [];
    private scrollMessages: MessageDto[] = [];
    private savedLive: MessageDto | undefined;
    private branchSource: LorepiaAppState['branches'] | null = null;
    private branchViews: NonNullable<SampleConversation['branches']> = [];

    project(state: LorepiaAppState, displayMode?: ChatDisplayMode): SampleConversation | null {
        const selected = state.selected_conversation;
        if (!selected) {
            this.source = null;
            this.messages = [];
            this.scrollMessages = [];
            this.savedLive = undefined;
            this.branchSource = null;
            this.branchViews = [];
            return null;
        }
        const liveId = state.chat.live_assistant_message_id;
        if (
            this.source !== state.messages.items ||
            this.liveId !== liveId ||
            this.conversationId !== selected.id
        ) {
            this.source = state.messages.items;
            this.liveId = liveId;
            this.conversationId = selected.id;
            const saved = state.messages.items.filter((item) => item.id !== liveId);
            this.messages = saved.map(messageView);
            this.savedLive = state.messages.items.find((item) => item.id === liveId);
            this.scrollMessages = saved;
            if (liveId)
                this.scrollMessages = [
                    ...saved,
                    {
                        id: liveId,
                        conversation_id: selected.id,
                        parent_id: this.savedLive?.parent_id ?? null,
                        role: 'assistant',
                        content: '',
                        status: 'pending',
                        generation_id: state.chat.active_generation_id,
                        created_at: this.savedLive?.created_at ?? selected.updated_at,
                    },
                ];
        }
        if (this.branchSource !== state.branches) {
            this.branchSource = state.branches;
            this.branchViews = state.branches.map((item, index) => ({
                id: item.id,
                title: item.title ?? t('workspace.branch', { number: index + 1 }),
                messages: [],
            }));
        }
        const pending = this.scrollMessages.at(-1);
        const messages =
            liveId && pending
                ? [
                      ...this.messages,
                      messageView({
                          ...pending,
                          content: state.chat.streaming_text,
                          generation_id: state.chat.active_generation_id,
                      }),
                  ]
                : this.messages;
        return {
            id: selected.id,
            title: selected.title,
            date: formatMessageDay(selected.created_at),
            messages,
            scrollMessages: this.scrollMessages,
            messageOffset: state.messages.start_index,
            totalMessages: state.messages.total_messages,
            mode: displayMode ?? state.conversation_state?.selected_mode ?? 'chat',
            activeBranchId: state.conversation_state?.active_branch_id,
            branches: this.branchViews,
        };
    }
}

export function characterViews(
    state: Pick<LorepiaAppState, 'library' | 'selected_character' | 'conversations'>,
    subpage = false,
): SampleCharacter[] {
    return state.library.characters.map((item) => ({
        id: item.id,
        name: item.name,
        description: item.description,
        thumbnail: item.name.slice(0, 1),
        avatarAssetId: item.avatar_asset_id,
        createdAt: item.created_at,
        subpage: item.id === state.selected_character?.id && subpage,
        histories:
            item.id === state.selected_character?.id
                ? state.conversations.items.map((conversation) => ({
                      id: conversation.id,
                      title: conversation.title,
                      date: formatMessageDay(conversation.updated_at),
                      messages: [],
                  }))
                : [],
    }));
}
