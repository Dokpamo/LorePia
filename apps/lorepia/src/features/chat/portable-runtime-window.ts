import type { PortableRuntimeStateScopeInput } from '../../lib/ipc/portable-runtime-state-contracts';
import type { LorepiaClient } from '../../lib/ipc/contracts';
import type { MessageDto } from '../../lib/ipc/contracts';
import type {
    PortableRuntimeMessageWindow,
    PortableRuntimePersistedState,
} from './portable-runtime-protocol';

/** Only a complete history is authority to forget an older message override. */
export function pruneMessageOverrides(
    persisted: PortableRuntimePersistedState,
    messages: readonly MessageDto[],
    window?: PortableRuntimeMessageWindow,
): PortableRuntimePersistedState | null {
    if (window !== undefined && (window.start_index > 0 || messages.length < window.total_messages))
        return null;
    const retained = new Set(messages.map((message) => message.id));
    const messageOverrides = Object.fromEntries(
        Object.entries(persisted.messageOverrides).filter(([id]) => retained.has(id)),
    );
    return Object.keys(messageOverrides).length === Object.keys(persisted.messageOverrides).length
        ? null
        : { ...persisted, messageOverrides };
}

export function runtimeStateScopeEquals(
    left: PortableRuntimeStateScopeInput,
    right: PortableRuntimeStateScopeInput,
): boolean {
    return (
        left.character_id === right.character_id &&
        left.character_content_revision_id === right.character_content_revision_id &&
        left.conversation_id === right.conversation_id &&
        left.branch_id === right.branch_id
    );
}

/** IDs are supplied only after an explicit deletion's authoritative membership check. */
export function removeProvenMessageOverrides(
    persisted: PortableRuntimePersistedState,
    removedIds: readonly string[],
): PortableRuntimePersistedState | null {
    const removed = new Set(removedIds);
    const messageOverrides = Object.fromEntries(
        Object.entries(persisted.messageOverrides).filter(([id]) => !removed.has(id)),
    );
    return Object.keys(messageOverrides).length === Object.keys(persisted.messageOverrides).length
        ? null
        : { ...persisted, messageOverrides };
}

export async function verifiedDeletedMessageIds(
    client: LorepiaClient,
    branchId: string,
    expectedHead: string | null,
    candidates: string[],
): Promise<string[] | null> {
    if (candidates.length === 0) return [];
    if (
        candidates.length > 256 ||
        !('listBranchMessagesPage' in client) ||
        !client.listBranchMessagesPage
    )
        return null;
    try {
        const page = await client.listBranchMessagesPage({
            branch_id: branchId,
            limit: 1,
            check_message_ids: candidates,
        });
        const retained = page.retained_message_ids;
        if (
            page.head_message_id !== expectedHead ||
            !Array.isArray(retained) ||
            retained.some((id) => !candidates.includes(id))
        )
            return null;
        const ids = new Set(retained);
        return candidates.filter((id) => !ids.has(id));
    } catch {
        return null;
    }
}
