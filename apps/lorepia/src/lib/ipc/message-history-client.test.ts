import { describe, expect, it, vi } from 'vitest';
import { LiveLorepiaClient, LOREPIA_COMMANDS, type LorepiaTransport } from './client';
import type {
    BranchMessagesPageDto,
    ListBranchMessagesPageInput,
} from './contracts/message-history';

describe('bounded message history IPC', () => {
    it('sends strict page requests through the shared transport and preserves snapshot metadata', async () => {
        const page: BranchMessagesPageDto = {
            messages: [],
            has_older: true,
            has_newer: false,
            head_message_id: 'message-2000',
            total_messages: 2000,
            start_index: 1970,
        };
        const invoke = vi.fn().mockResolvedValue(page);
        const createChatChannel = vi.fn();
        const listen = vi.fn();
        const transport: LorepiaTransport = {
            invoke,
            createChatChannel,
            listen,
        };
        const client = new LiveLorepiaClient(transport);
        const requests: ListBranchMessagesPageInput[] = [
            { branch_id: 'branch', limit: 30 },
            { branch_id: 'branch', before_message_id: 'message-1970', limit: 30 },
            { branch_id: 'branch', after_message_id: 'message-1940', limit: 30 },
        ];
        for (const request of requests) {
            expect(await client.listBranchMessagesPage(request)).toBe(page);
            expect(invoke).toHaveBeenLastCalledWith(LOREPIA_COMMANDS.listBranchMessagesPage, {
                request,
            });
        }
        expect(createChatChannel).not.toHaveBeenCalled();
        expect(listen).not.toHaveBeenCalled();
    });
});
