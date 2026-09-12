// @vitest-environment node
import { describe, expect, it, vi } from 'vitest';
import type { LorepiaClient } from '../../lib/ipc/contracts';
import {
    defaultPortableRuntimeState,
    MAX_RUNTIME_RECORD_KEYS,
    updatePortableStringRecord,
} from './portable-runtime-state';
import { removeProvenMessageOverrides, verifiedDeletedMessageIds } from './portable-runtime-window';
import { PortableCharacterRuntime, createPortableRuntimeGrant } from './portable-runtime';
import {
    inProcessWorkerFactory,
    memoryStorage,
    message,
    profile,
} from './tests/portable-runtime-fixtures';

describe('explicit deletion override cleanup', () => {
    it('survives more than 256 deleted replacements while preserving an unseen valid older override', async () => {
        let persisted = {
            ...defaultPortableRuntimeState(''),
            messageOverrides: { older: 'keep' } as Record<string, string>,
        };
        const lookup = vi.fn(() =>
            Promise.resolve({
                messages: [],
                has_older: true,
                has_newer: false,
                head_message_id: 'retained-head',
                total_messages: 1000,
                start_index: 1000,
                retained_message_ids: ['older'],
            }),
        );
        const client = { listBranchMessagesPage: lookup } as unknown as LorepiaClient;
        for (let index = 0; index < 512; index += 1) {
            const id = `deleted-${String(index)}`;
            const next = updatePortableStringRecord(
                persisted.messageOverrides,
                id,
                'override',
                MAX_RUNTIME_RECORD_KEYS,
                262_144,
            );
            expect(next).not.toBeNull();
            if (!next) throw new Error('Override key capacity leaked');
            persisted = { ...persisted, messageOverrides: next };
            const removed = await verifiedDeletedMessageIds(
                client,
                'branch',
                'retained-head',
                Object.keys(next),
            );
            expect(removed).toEqual([id]);
            persisted = removeProvenMessageOverrides(persisted, removed ?? []) ?? persisted;
            expect(persisted.messageOverrides).toEqual({ older: 'keep' });
        }
        expect(lookup).toHaveBeenCalledTimes(512);
        expect(lookup).toHaveBeenLastCalledWith({
            branch_id: 'branch',
            limit: 1,
            check_message_ids: ['older', 'deleted-511'],
        });
    });

    it('preserves overrides for missing membership, failed requests and a changed head', async () => {
        const base = {
            messages: [],
            has_older: false,
            has_newer: false,
            head_message_id: 'head',
            total_messages: 0,
            start_index: 0,
        };
        for (const result of [
            base,
            { ...base, head_message_id: 'different', retained_message_ids: [] },
            { ...base, retained_message_ids: ['unrequested'] },
        ]) {
            const client = {
                listBranchMessagesPage: () => Promise.resolve(result),
            } as unknown as LorepiaClient;
            expect(
                await verifiedDeletedMessageIds(client, 'branch', 'head', ['deleted']),
            ).toBeNull();
        }
        expect(
            await verifiedDeletedMessageIds({} as LorepiaClient, 'branch', 'head', ['deleted']),
        ).toBeNull();
        const failed = {
            listBranchMessagesPage: () => Promise.reject(new Error('offline')),
        } as unknown as LorepiaClient;
        expect(await verifiedDeletedMessageIds(failed, 'branch', 'head', ['deleted'])).toBeNull();
    });

    it('persists the exact proven removal after closing its worker and before reopening the same scope', async () => {
        const active = profile();
        active.required_runtime_capabilities = ['runtime:callbacks', 'chat:read', 'chat:write'];
        const script = active.runtime_scripts[0];
        if (!script) throw new Error('Script missing');
        active.runtime_scripts = [
            {
                ...script,
                source: 'function rewrite(triggerId) assert(setChat(triggerId, -1, "override")) end',
            },
        ];
        const storage = memoryStorage();
        const options = {
            profile: active,
            grant: await createPortableRuntimeGrant(active, active.required_runtime_capabilities),
            conversationId: 'conversation',
            branchId: 'branch',
            characterName: 'Character',
            characterDescription: '',
            client: {} as LorepiaClient,
            primarySelection: () => null,
            onChanged: vi.fn(),
            onNotice: vi.fn(),
            storage,
            workerFactory: inProcessWorkerFactory,
        };
        const original = message('deleted', 'assistant', 'original');
        const runtime = await PortableCharacterRuntime.create(options);
        runtime.setMessages([original], {
            start_index: 1999,
            total_messages: 2000,
            head_message_id: original.id,
        });
        await runtime.handleAction('rewrite');
        expect(runtime.effectiveText(original)).toBe('override');
        runtime.close();
        await runtime.forgetDeletedMessages(['deleted']);
        const reopened = await PortableCharacterRuntime.create(options);
        try {
            expect(reopened.effectiveText(original)).toBe('original');
            expect(reopened.messageOverrideIds).toEqual([]);
        } finally {
            reopened.close();
        }
    });
});
