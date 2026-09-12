import { describe, expect, it, vi } from 'vitest';

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

describe('portable runtime context work bounds', () => {
    it('serializes retained messages only once while preserving the final byte budget', () => {
        const messages = Array.from({ length: 127 }, (_, index) => message(index));
        const encode = vi.spyOn(TextEncoder.prototype, 'encode');
        let context: ReturnType<typeof bounded>;
        try {
            context = bounded(messages, virtualMessage());
            expect(encode).toHaveBeenCalledTimes(128);
        } finally {
            encode.mockRestore();
        }
        expect(contextBytes(context)).toBeLessThanOrEqual(MAX_RUNTIME_CONTEXT_BYTES);
        expect(context.messages).toHaveLength(127);
    });

    it('matches full join/slice for empty messages, Unicode, separators and unusual limits', () => {
        const values = ['', 'a', '\ud83d\ude42', '\ud800', '\udc00', 'a\nb', '"\\\n'];
        const limits = [-10, -1, -0.5, 0, 0.5, 1, 2, 3, 7, 30, NaN, Infinity, -Infinity];
        for (const first of values) {
            for (const last of values) {
                for (const virtual of [null, virtualMessage(''), virtualMessage(last)]) {
                    const context = {
                        messages: [virtualMessage(first), virtualMessage(''), virtualMessage(last)],
                        virtualMessage: virtual,
                    };
                    const all = [
                        ...context.messages.map((item) => item.data),
                        ...(virtual ? [virtual.data] : []),
                    ].join('\n');
                    for (const limit of limits)
                        expect(portableRuntimeChatContextSource(context, limit)).toBe(
                            all.slice(-limit),
                        );
                }
            }
        }
        expect(portableRuntimeChatContextSource({ messages: [], virtualMessage: null }, 10)).toBe(
            '',
        );
    });

    it('does not visit older text when the newest message covers the lore suffix', () => {
        const older = virtualMessage('old');
        Object.defineProperty(older, 'data', { get: () => 'old', configurable: true });
        const read = vi.spyOn(older, 'data', 'get');
        const context = {
            messages: [older, virtualMessage('x'.repeat(262_144))],
            virtualMessage: null,
        };
        expect(portableRuntimeChatContextSource(context, 1024)).toBe('x'.repeat(1024));
        expect(read).not.toHaveBeenCalled();
        read.mockRestore();
    });
});

it('keeps global suffix indices and budgets optional paging metadata', () => {
    const messages = Array.from({ length: 200 }, (_, index) => message(index));
    const context = boundedPortableRuntimeChatContext(messages, null, (item) => item.content, {
        start_index: 800,
        total_messages: 1000,
        head_message_id: 'message-199',
    });
    expect(context.messages).toHaveLength(128);
    expect(context.messageWindow).toEqual({
        start_index: 872,
        total_messages: 1000,
        head_message_id: 'message-199',
    });
    const large = boundedPortableRuntimeChatContext(
        [message(999, 'x'.repeat(MAX_RUNTIME_CONTEXT_BYTES))],
        null,
        (item) => item.content,
        {
            start_index: 999,
            total_messages: 1000,
            head_message_id: 'message-999',
        },
    );
    expect(portableRuntimeMessageByteLength(large)).toBeLessThanOrEqual(MAX_RUNTIME_CONTEXT_BYTES);
    expect(large.messageWindow?.start_index).toBe(999);
});

it('rejects metadata that alone exceeds the context budget before returning a window', () => {
    expect(() =>
        boundedPortableRuntimeChatContext([], null, (item) => item.content, {
            start_index: 0,
            total_messages: 0,
            head_message_id: 'x'.repeat(MAX_RUNTIME_CONTEXT_BYTES),
        }),
    ).toThrow('portable runtime window metadata exceeds the context byte limit');
});
