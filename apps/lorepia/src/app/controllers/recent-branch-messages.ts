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
        const reused = await completeInitialWindow(client, branchId, first);
        if (reused) return reused;
    }
    const page = await client.listBranchMessagesPage({
        branch_id: branchId,
        limit: RUNTIME_HISTORY_MESSAGES,
        include_last_assistant: true,
    });
    return retainMessageWindow(page.messages, page);
}

async function completeInitialWindow(
    client: HistoryPageClient,
    branchId: string,
    first: BranchMessagesPageDto,
): Promise<MessageDto[] | null> {
    const oldest = first.messages[0];
    if (
        !first.snapshot_token ||
        !oldest ||
        !client.listBranchMessagesPage ||
        first.messages.length >= RUNTIME_HISTORY_MESSAGES ||
        first.has_newer
    )
        return null;
    let older: BranchMessagesPageDto;
    try {
        older = await client.listBranchMessagesPage({
            branch_id: branchId,
            before_message_id: oldest.id,
            limit: RUNTIME_HISTORY_MESSAGES - first.messages.length,
            include_last_assistant: !first.messages.some((message) => message.role === 'assistant'),
        });
    } catch (error) {
        // A rewind/removal can invalidate the anchor while the first page is visible.
        if (
            typeof error === 'object' &&
            error !== null &&
            'code' in error &&
            error.code === 'invalid_input'
        )
            return null;
        throw error;
    }
    if (
        older.snapshot_token !== first.snapshot_token ||
        older.head_message_id !== first.head_message_id ||
        older.total_messages !== first.total_messages ||
        older.start_index + older.messages.length !== first.start_index ||
        first.start_index + first.messages.length !== first.total_messages ||
        older.messages.at(-1)?.id !== oldest.parent_id
    )
        return null;
    const items = [...older.messages, ...first.messages];
    if (
        items.length > RUNTIME_HISTORY_MESSAGES ||
        new Set(items.map((message) => message.id)).size !== items.length
    )
        return null;
    const assistant = older.last_assistant_message;
    return retainMessageWindow(items, {
        ...older,
        ...(assistant && items.some((message) => message.id === assistant.id)
            ? { last_assistant_message: null }
            : {}),
    });
}
