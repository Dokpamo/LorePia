import type { BranchMessagesPageDto } from '../../lib/ipc/contracts/message-history';
import type { LorepiaClient, MessageDto } from '../../lib/ipc/contracts';

export const INITIAL_HISTORY_MESSAGES = 30;
export const RUNTIME_HISTORY_MESSAGES = 128;

export interface MessageWindowMetadata {
    start_index: number;
    total_messages: number;
    head_message_id: string | null;
    last_assistant_message?: MessageDto | null;
}

export type MessageHistoryPage = BranchMessagesPageDto;
export type HistoryPageClient = LorepiaClient;

const metadata = new WeakMap<MessageDto[], MessageWindowMetadata>();

export function messageWindowMetadata(items: MessageDto[]): Partial<MessageWindowMetadata> {
    return metadata.get(items) ?? {};
}

export function retainMessageWindow(
    items: MessageDto[],
    window: MessageWindowMetadata,
): MessageDto[] {
    metadata.set(items, {
        start_index: window.start_index,
        total_messages: window.total_messages,
        head_message_id: window.head_message_id,
        ...(window.last_assistant_message === undefined
            ? {}
            : { last_assistant_message: window.last_assistant_message }),
    });
    return items;
}

/** Latest context is bounded independently of the transcript's movable history window. */
export async function loadRecentBranchMessages(
    client: HistoryPageClient,
    branchId: string,
    onInitial?: (messages: MessageDto[]) => boolean | undefined,
): Promise<MessageDto[]> {
    if (!('listBranchMessagesPage' in client) || !client.listBranchMessagesPage)
        return client.listBranchMessages(branchId);
    if (onInitial) {
        const first = await client.listBranchMessagesPage({
            branch_id: branchId,
            limit: INITIAL_HISTORY_MESSAGES,
        });
        retainMessageWindow(first.messages, first);
        if (onInitial(first.messages) === false) return first.messages;
        if (!first.has_older) return first.messages;
    }
    const page = await client.listBranchMessagesPage({
        branch_id: branchId,
        limit: RUNTIME_HISTORY_MESSAGES,
        include_last_assistant: true,
    });
    return retainMessageWindow(page.messages, page);
}
