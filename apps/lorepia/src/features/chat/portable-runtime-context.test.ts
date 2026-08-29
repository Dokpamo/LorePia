import { describe, expect, it } from 'vitest';

import type { MessageDto } from '../../lib/ipc/contracts';
import {
    MAX_RUNTIME_CONTEXT_BYTES,
    MAX_RUNTIME_CONTEXT_MESSAGES,
    boundedPortableRuntimeChatContext,
    portableRuntimeChatContextSource,
} from './portable-runtime-context';
import {
    portableRuntimeMessageByteLength,
    type PortableRuntimeChatMessage,
} from './portable-runtime-protocol';

function message(index: number, data = `message-${String(index)}`): MessageDto {
    return {
        id: `message-${String(index)}`,
        conversation_id: 'conversation',
        parent_id: index === 0 ? null : `message-${String(index - 1)}`,
        role: index % 2 === 0 ? 'user' : 'assistant',
        content: data,
        status: 'complete',
        generation_id: index % 2 === 0 ? null : `generation-${String(index)}`,
        created_at: `2026-08-29T00:${String(index % 60).padStart(2, '0')}:00Z`,
    };
}

function virtualMessage(data = 'pending'): PortableRuntimeChatMessage {
    return {
        id: '__runtime_pending_user__',
        role: 'user',
        data,
        time: 1_788_048_000,
        virtual: true,
    };
}

function bounded(
    messages: readonly MessageDto[],
    virtual: PortableRuntimeChatMessage | null = null,
) {
    return boundedPortableRuntimeChatContext(messages, virtual, (item) => item.content);
}

function contextBytes(context: ReturnType<typeof bounded>): number {
    return portableRuntimeMessageByteLength(context) ?? Number.POSITIVE_INFINITY;
}

describe('portable runtime chat context', () => {
    it('keeps the newest 128 persisted messages in chronological order', () => {
        const messages = Array.from({ length: 140 }, (_, index) => message(index));

        const context = bounded(messages);

        expect(context.messages).toHaveLength(MAX_RUNTIME_CONTEXT_MESSAGES);
        expect(context.messages.map((item) => item.id)).toEqual(
            messages.slice(-MAX_RUNTIME_CONTEXT_MESSAGES).map((item) => item.id),
        );
        expect(context.virtualMessage).toBeNull();
        expect(contextBytes(context)).toBeLessThanOrEqual(MAX_RUNTIME_CONTEXT_BYTES);
    });

    it('counts the virtual newest message as one slot before persisted history', () => {
        const messages = Array.from({ length: MAX_RUNTIME_CONTEXT_MESSAGES }, (_, index) =>
            message(index),
        );
        const virtual = virtualMessage();

        const context = bounded(messages, virtual);

        expect(context.messages).toHaveLength(MAX_RUNTIME_CONTEXT_MESSAGES - 1);
        expect(context.messages[0]?.id).toBe('message-1');
        expect(context.messages.at(-1)?.id).toBe(
            `message-${String(MAX_RUNTIME_CONTEXT_MESSAGES - 1)}`,
        );
        expect(context.virtualMessage).toEqual(virtual);
    });

    it('budgets JSON escaping and keeps a contiguous newest suffix', () => {
        const escaped = `OLD-BOUNDARY:${'"\\\n'.repeat(100_000)}:BOUNDARY-END`;
        expect(new TextEncoder().encode(escaped).byteLength).toBeLessThan(
            MAX_RUNTIME_CONTEXT_BYTES,
        );
        const messages = [
            message(0, 'must-not-skip-around-boundary'),
            message(1, escaped),
            message(2, 'newest'),
        ];

        const context = bounded(messages);

        expect(contextBytes(context)).toBeLessThanOrEqual(MAX_RUNTIME_CONTEXT_BYTES);
        expect(context.messages.map((item) => item.id)).toEqual(['message-1', 'message-2']);
        expect(context.messages[0]?.data).not.toBe(escaped);
        expect(context.messages[0]?.data.endsWith(':BOUNDARY-END')).toBe(true);
    });

    it('retains an oversized newest message identity with a UTF-8-safe emoji suffix', () => {
        const oversized = `HEAD:${'🙂'.repeat(200_000)}:TAIL`;
        const source = message(7, oversized);

        const context = bounded([source]);
        const retained = context.messages[0];
        if (retained === undefined) throw new Error('oversized newest message was not retained');

        expect(retained).toMatchObject({
            id: source.id,
            role: 'char',
            time: Math.floor(Date.parse(source.created_at) / 1_000),
            virtual: false,
        });
        expect(retained.data).not.toBe(oversized);
        expect(retained.data.endsWith(':TAIL')).toBe(true);
        const firstCodeUnit = retained.data.charCodeAt(0);
        const lastCodeUnit = retained.data.charCodeAt(retained.data.length - 1);
        expect(firstCodeUnit >= 0xdc00 && firstCodeUnit <= 0xdfff).toBe(false);
        expect(lastCodeUnit >= 0xd800 && lastCodeUnit <= 0xdbff).toBe(false);
        expect(contextBytes(context)).toBeLessThanOrEqual(MAX_RUNTIME_CONTEXT_BYTES);
    });

    it('builds lore matching text from the exact bounded chat and virtual view', () => {
        const messages = Array.from({ length: 130 }, (_, index) => message(index));
        const virtual = virtualMessage('virtual-newest');
        const context = bounded(messages, virtual);

        const source = portableRuntimeChatContextSource(context, 1_000_000);

        expect(context.messages[0]?.id).toBe('message-3');
        expect(source).toBe(
            [...context.messages.map((item) => item.data), virtual.data].join('\n'),
        );
        expect(source).not.toContain('message-2\n');
        expect(source.endsWith('virtual-newest')).toBe(true);
    });
});
