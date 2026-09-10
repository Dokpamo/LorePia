import type {
    ConversationBranchDto,
    ConversationStateDto,
    LorepiaClient,
    MessageDto,
} from '../../lib/ipc/contracts';
import type { LorepiaAppState } from '../app-state';

/** A terminal snapshot can replace only a proven contiguous suffix. */
export function mergeTerminalMessages(
    current: MessageDto[],
    pair: MessageDto[],
    conversationId: string,
    generationId: string,
    head: string | null,
): MessageDto[] | null {
    const [user, assistant] = pair;
    if (
        pair.length !== 2 ||
        !user ||
        !assistant ||
        user.role !== 'user' ||
        user.status !== 'complete' ||
        assistant.role !== 'assistant' ||
        assistant.status === 'pending' ||
        user.conversation_id !== conversationId ||
        assistant.conversation_id !== conversationId ||
        assistant.generation_id !== generationId ||
        assistant.parent_id !== user.id ||
        assistant.id !== head ||
        user.id === assistant.id
    )
        return null;
    let start = current.length;
    if (current[start - 1]?.id === assistant.id) {
        const pending = current[--start];
        if (
            pending?.role !== 'assistant' ||
            pending.generation_id !== generationId ||
            current[start - 1]?.id !== user.id
        )
            return null;
    }
    if (current[start - 1]?.id === user.id) start -= 1;
    if ((current[start - 1]?.id ?? null) !== user.parent_id) return null;
    return [...current.slice(0, start), user, assistant];
}

export async function loadReconciledMessages(
    client: LorepiaClient,
    current: LorepiaAppState,
    next: ConversationStateDto,
    branches: ConversationBranchDto[],
    generationId: string,
    reason: string,
): Promise<MessageDto[]> {
    if (
        reason === 'terminal' &&
        'listGenerationMessages' in client &&
        client.listGenerationMessages &&
        current.messages.phase === 'ready' &&
        current.selected_conversation?.id === next.conversation_id &&
        current.conversation_state?.active_branch_id === next.active_branch_id
    ) {
        const branch = branches.find(
            (value) =>
                value.id === next.active_branch_id &&
                value.conversation_id === next.conversation_id,
        );
        if (branch) {
            try {
                const pair = await client.listGenerationMessages(
                    next.conversation_id,
                    next.active_branch_id,
                    generationId,
                );
                const merged = mergeTerminalMessages(
                    current.messages.items,
                    pair,
                    next.conversation_id,
                    generationId,
                    branch.head_message_id,
                );
                if (merged !== null) return merged;
            } catch {
                // Recovery still reloads and verifies the complete branch on any mismatch.
            }
        }
    }
    return client.listBranchMessages(next.active_branch_id);
}

export function pendingAssistantMessage(
    messages: MessageDto[],
    generationId?: string,
): MessageDto | null {
    for (let index = messages.length - 1; index >= 0; index -= 1) {
        const message = messages[index];
        if (
            message?.role === 'assistant' &&
            message.status === 'pending' &&
            message.generation_id !== null &&
            (generationId === undefined || message.generation_id === generationId)
        )
            return message;
    }
    return null;
}
