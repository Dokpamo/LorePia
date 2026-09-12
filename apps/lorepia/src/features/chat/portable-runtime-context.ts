import type { MessageDto } from '../../lib/ipc/contracts';
import {
    portableRuntimeMessageByteLength,
    type PortableRuntimeChatMessage,
    type PortableRuntimeMessageWindow,
} from './portable-runtime-protocol';

export const MAX_RUNTIME_CONTEXT_MESSAGES = 128;
export const MAX_RUNTIME_CONTEXT_BYTES = 512 * 1024;

export interface PortableRuntimeBoundedChatContext {
    messageWindow?: PortableRuntimeMessageWindow;
    messages: PortableRuntimeChatMessage[];
    virtualMessage: PortableRuntimeChatMessage | null;
}

const EMPTY_CONTEXT_BYTES =
    portableRuntimeMessageByteLength({ messages: [], virtualMessage: null }) ??
    MAX_RUNTIME_CONTEXT_BYTES;
const NULL_JSON_BYTES = 4;
const MESSAGE_SEPARATOR_BYTES = 1;

export function boundedPortableRuntimeChatContext(
    messages: readonly MessageDto[],
    virtualMessage: PortableRuntimeChatMessage | null,
    effectiveText: (message: MessageDto) => string,
    window?: PortableRuntimeMessageWindow,
): PortableRuntimeBoundedChatContext {
    // Reserve the largest possible offset representation before selecting a suffix.
    let contextBytes =
        window === undefined
            ? EMPTY_CONTEXT_BYTES
            : (portableRuntimeMessageByteLength({
                  messages: [],
                  virtualMessage: null,
                  messageWindow: { ...window, start_index: window.start_index + messages.length },
              }) ?? Number.POSITIVE_INFINITY);
    if (contextBytes > MAX_RUNTIME_CONTEXT_BYTES) {
        throw new RangeError('portable runtime window metadata exceeds the context byte limit');
    }
    let boundedVirtualMessage: PortableRuntimeChatMessage | null = null;

    if (virtualMessage !== null) {
        const candidate = { ...virtualMessage };
        const bounded = fitMessageSuffix(candidate, (messageBytes) => {
            return contextBytes + messageBytes - NULL_JSON_BYTES <= MAX_RUNTIME_CONTEXT_BYTES;
        });
        if (bounded !== null) {
            boundedVirtualMessage = bounded.message;
            contextBytes += bounded.bytes - NULL_JSON_BYTES;
        }
    }

    const selectedNewestFirst: PortableRuntimeChatMessage[] = [];
    let logicalMessages = boundedVirtualMessage === null ? 0 : 1;
    for (let index = messages.length - 1; index >= 0; index -= 1) {
        if (logicalMessages >= MAX_RUNTIME_CONTEXT_MESSAGES) break;
        const source = messages[index];
        if (source === undefined) continue;
        const candidate: PortableRuntimeChatMessage = {
            id: source.id,
            role: runtimeRole(source.role),
            data: effectiveText(source),
            time: Math.floor(Date.parse(source.created_at) / 1_000) || 0,
            virtual: false,
        };
        const separatorBytes = selectedNewestFirst.length === 0 ? 0 : MESSAGE_SEPARATOR_BYTES;
        const bounded = fitMessageSuffix(candidate, (candidateBytes) => {
            return contextBytes + separatorBytes + candidateBytes <= MAX_RUNTIME_CONTEXT_BYTES;
        });
        if (bounded === null) break;
        selectedNewestFirst.push(bounded.message);
        logicalMessages += 1;
        contextBytes += separatorBytes + bounded.bytes;
        if (bounded.message.data !== candidate.data) break;
    }

    return {
        ...(window === undefined
            ? {}
            : {
                  messageWindow: {
                      ...window,
                      start_index:
                          window.start_index + messages.length - selectedNewestFirst.length,
                  },
              }),
        messages: selectedNewestFirst.reverse(),
        virtualMessage: boundedVirtualMessage,
    };
}

export function portableRuntimeChatContextSource(
    context: PortableRuntimeBoundedChatContext,
    maximumCharacters: number,
): string {
    // Preserve String.slice semantics for unusual limits; normal lore budgets are positive integers.
    if (!Number.isSafeInteger(maximumCharacters) || maximumCharacters <= 0) {
        const sourceParts = context.messages.map((message) => message.data);
        if (context.virtualMessage !== null) sourceParts.push(context.virtualMessage.data);
        return sourceParts.join('\n').slice(-maximumCharacters);
    }
    const parts: string[] = [];
    let remaining = maximumCharacters;
    const count = context.messages.length + (context.virtualMessage === null ? 0 : 1);
    for (let index = count - 1; index >= 0 && remaining > 0; index -= 1) {
        const data =
            index === context.messages.length
                ? (context.virtualMessage?.data ?? '')
                : (context.messages[index]?.data ?? '');
        const suffix = data.slice(-remaining);
        parts.push(suffix);
        remaining -= suffix.length;
        if (index > 0 && remaining > 0) {
            parts.push('\n');
            remaining -= 1;
        }
    }
    return parts.reverse().join('');
}

function fitMessageSuffix(
    message: PortableRuntimeChatMessage,
    fits: (messageBytes: number) => boolean,
): { message: PortableRuntimeChatMessage; bytes: number } | null {
    const fullBytes = messageBytes(message);
    if (fits(fullBytes)) return { message, bytes: fullBytes };
    const empty = { ...message, data: '' };
    if (!fits(messageBytes(empty))) return null;

    let low = 0;
    let high = message.data.length;
    while (low < high) {
        const middle = Math.floor((low + high) / 2);
        const candidate = { ...message, data: message.data.slice(middle) };
        if (fits(messageBytes(candidate))) high = middle;
        else low = middle + 1;
    }

    let start = low;
    if (startsInsideSurrogatePair(message.data, start)) start += 1;
    let candidate = { ...message, data: message.data.slice(start) };
    let candidateBytes = messageBytes(candidate);
    while (!fits(candidateBytes) && start < message.data.length) {
        start = nextCodePointBoundary(message.data, start);
        candidate = { ...message, data: message.data.slice(start) };
        candidateBytes = messageBytes(candidate);
    }

    const previous = previousCodePointBoundary(message.data, start);
    if (previous !== null) {
        const longer = { ...message, data: message.data.slice(previous) };
        const longerBytes = messageBytes(longer);
        if (fits(longerBytes)) {
            candidate = longer;
            candidateBytes = longerBytes;
        }
    }
    return { message: candidate, bytes: candidateBytes };
}

function messageBytes(message: PortableRuntimeChatMessage): number {
    return portableRuntimeMessageByteLength(message) ?? Number.POSITIVE_INFINITY;
}

function startsInsideSurrogatePair(value: string, index: number): boolean {
    if (index <= 0 || index >= value.length) return false;
    return isHighSurrogate(value.charCodeAt(index - 1)) && isLowSurrogate(value.charCodeAt(index));
}

function nextCodePointBoundary(value: string, start: number): number {
    if (start >= value.length) return value.length;
    return isHighSurrogate(value.charCodeAt(start)) && isLowSurrogate(value.charCodeAt(start + 1))
        ? start + 2
        : start + 1;
}

function previousCodePointBoundary(value: string, start: number): number | null {
    if (start <= 0) return null;
    const previous = start - 1;
    return isLowSurrogate(value.charCodeAt(previous)) &&
        previous > 0 &&
        isHighSurrogate(value.charCodeAt(previous - 1))
        ? previous - 1
        : previous;
}

function isHighSurrogate(value: number): boolean {
    return value >= 0xd800 && value <= 0xdbff;
}

function isLowSurrogate(value: number): boolean {
    return value >= 0xdc00 && value <= 0xdfff;
}

function runtimeRole(role: MessageDto['role']): PortableRuntimeChatMessage['role'] {
    if (role === 'assistant') return 'char';
    if (role === 'user') return 'user';
    return 'system';
}
