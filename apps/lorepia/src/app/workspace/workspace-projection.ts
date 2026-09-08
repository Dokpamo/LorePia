import { t } from '../../lib/i18n';
import type { LorepiaAppState } from '../app-state';
import type { MessageDto } from '../../lib/ipc/contracts';
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

export function conversationView(state: LorepiaAppState): SampleConversation | null {
    const selected = state.selected_conversation;
    if (!selected) return null;
    const liveId = state.chat.live_assistant_message_id;
    const messages = state.messages.items.filter((item) => item.id !== liveId).map(messageView);
    if (liveId) {
        const saved = state.messages.items.find((item) => item.id === liveId);
        messages.push(
            messageView({
                id: liveId,
                conversation_id: selected.id,
                parent_id: saved?.parent_id ?? null,
                role: 'assistant',
                content: state.chat.streaming_text,
                status: 'pending',
                generation_id: state.chat.active_generation_id,
                created_at: saved?.created_at ?? selected.updated_at,
            }),
        );
    }
    return {
        id: selected.id,
        title: selected.title,
        date: formatMessageDay(selected.created_at),
        messages,
        mode: state.conversation_state?.selected_mode ?? 'chat',
        activeBranchId: state.conversation_state?.active_branch_id,
        branches: state.branches.map((item, index) => ({
            id: item.id,
            title: item.title ?? t('workspace.branch', { number: index + 1 }),
            messages: [],
        })),
    };
}

export function characterViews(state: LorepiaAppState, subpage = false): SampleCharacter[] {
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
