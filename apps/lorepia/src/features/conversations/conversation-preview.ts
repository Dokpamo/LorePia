import type { LorepiaAppState } from '../../app/app-controller';
import type { ConversationDto } from '../../lib/ipc/contracts';

export function safeConversationPreview(
    conversation: ConversationDto,
    state: LorepiaAppState,
    fallback: string,
): string {
    const message =
        state.selected_conversation?.id === conversation.id && state.messages.phase === 'ready'
            ? state.messages.items.at(-1)?.content
            : undefined;
    const preview = message?.trim().replace(/\s+/gu, ' ');
    return preview === undefined ||
        preview === '' ||
        preview.includes('{{') ||
        preview.includes('}}')
        ? fallback
        : preview;
}
