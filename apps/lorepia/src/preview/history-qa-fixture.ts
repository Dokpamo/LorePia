/** Fixed long-history fixture for manual browser QA; never touches native storage. */
import type { MessageDto } from '../lib/ipc/contracts';
import type { PreviewClient } from './mock-client';

export function installLongHistoryFixture(client: PreviewClient): void {
    const originalMessages = client.listBranchMessages.bind(client);
    const originalBranches = client.listBranches.bind(client);
    const histories = new Map<string, MessageDto[]>();
    client.listBranchMessages = async (branchId) => {
        const cached = histories.get(branchId);
        if (cached) return cached;
        const original = await originalMessages(branchId);
        const conversationId = original[0]?.conversation_id;
        if (!conversationId) return original;
        const messages = Array.from({ length: 2000 }, (_, index): MessageDto => ({
            id: `${branchId}-history-${String(index)}`,
            conversation_id: conversationId,
            parent_id: index ? `${branchId}-history-${String(index - 1)}` : null,
            role: index % 2 === 0 ? 'user' : 'assistant',
            content: `History ${String(index + 1)} / 2000 — Synthetic conversation text for scroll and paging verification.`,
            status: 'complete',
            generation_id: null,
            created_at: '2026-09-12T00:00:00.000Z',
        }));
        histories.set(branchId, messages);
        return messages;
    };
    client.listBranches = async (conversationId) =>
        Promise.all(
            (await originalBranches(conversationId)).map(async (branch) => ({
                ...branch,
                head_message_id: (await client.listBranchMessages(branch.id)).at(-1)?.id ?? null,
            })),
        );
}
