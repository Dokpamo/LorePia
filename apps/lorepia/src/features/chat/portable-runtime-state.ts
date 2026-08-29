import type { GenerationSelectionInput } from '../../lib/ipc/contracts';
import type { PortableRuntimePersistedState } from './portable-runtime-protocol';

export const MAX_PERSISTED_RUNTIME_BYTES = 4 * 1024 * 1024;
export const MAX_RUNTIME_NOTICE_CHARS = 4_096;
export const MAX_RUNTIME_RECORD_KEYS = 256;
export const MAX_RUNTIME_STATE_VALUE_BYTES = 64 * 1024;
export const MAX_RUNTIME_STATE_VALUE_NODES = 2_048;
export const MAX_RUNTIME_MESSAGE_OVERRIDE_CHARS = 262_144;
export const MAX_RUNTIME_BACKGROUND_CHARS = 1024 * 1024;

export function defaultPortableRuntimeState(
    backgroundMarkup: string,
): PortableRuntimePersistedState {
    return {
        options: {},
        chatVars: {},
        state: {},
        messageOverrides: {},
        background: backgroundMarkup.slice(0, MAX_RUNTIME_BACKGROUND_CHARS),
        auxiliarySelection: null,
    };
}

export function normalizePortableRuntimeState(
    value: unknown,
    backgroundMarkup: string,
): PortableRuntimePersistedState | null {
    if (!isRecord(value)) return null;
    const options = boundedStringRecord(value.options, 16_384);
    const chatVars = boundedUnknownRecord(value.chatVars);
    const state = boundedUnknownRecord(value.state);
    const messageOverrides = boundedStringRecord(
        value.messageOverrides,
        MAX_RUNTIME_MESSAGE_OVERRIDE_CHARS,
    );
    if (options === null || chatVars === null || state === null || messageOverrides === null) {
        return null;
    }
    const background =
        typeof value.background === 'string'
            ? value.background.slice(0, MAX_RUNTIME_BACKGROUND_CHARS)
            : backgroundMarkup.slice(0, MAX_RUNTIME_BACKGROUND_CHARS);
    const auxiliarySelection = validSelection(value.auxiliarySelection)
        ? cloneSelection(value.auxiliarySelection)
        : null;
    const candidate: PortableRuntimePersistedState = {
        options,
        chatVars,
        state,
        messageOverrides,
        background,
        auxiliarySelection,
    };
    return serializePortableRuntimeState(candidate) === null ? null : candidate;
}

export function serializePortableRuntimeState(value: PortableRuntimePersistedState): string | null {
    try {
        const serialized = JSON.stringify(value);
        return new TextEncoder().encode(serialized).byteLength <= MAX_PERSISTED_RUNTIME_BYTES
            ? serialized
            : null;
    } catch {
        return null;
    }
}

export function updatePortableStringRecord(
    record: Record<string, string>,
    key: string,
    value: string,
    maxKeys: number,
    maxValueChars: number,
): Record<string, string> | null {
    if (
        !validPortableRuntimeKey(key) ||
        value.length > maxValueChars ||
        (!(key in record) && Object.keys(record).length >= maxKeys)
    ) {
        return null;
    }
    return Object.fromEntries([
        ...Object.entries(record).filter(([name]) => name !== key),
        [key, value],
    ]);
}

export function updatePortableUnknownRecord(
    record: Record<string, unknown>,
    key: string,
    value: unknown,
): Record<string, unknown> | null {
    if (!validPortableRuntimeKey(key)) return null;
    if (value === undefined || value === null) {
        return Object.fromEntries(Object.entries(record).filter(([name]) => name !== key));
    }
    if (!(key in record) && Object.keys(record).length >= MAX_RUNTIME_RECORD_KEYS) return null;
    const bounded = boundedPortableJsonValue(value);
    if (!bounded.ok) return null;
    return Object.fromEntries([
        ...Object.entries(record).filter(([name]) => name !== key),
        [key, bounded.value],
    ]);
}

export function safePortableText(value: unknown): string {
    if (value === undefined || value === null) return '';
    if (typeof value === 'string') return value;
    if (typeof value === 'number' || typeof value === 'boolean' || typeof value === 'bigint') {
        return String(value);
    }
    if (value instanceof Error) return value.message;
    try {
        const encoded: unknown = JSON.stringify(value);
        return typeof encoded === 'string' ? encoded : '';
    } catch {
        return '';
    }
}

export function boundedPortableJsonValue(
    value: unknown,
): { ok: true; value: unknown } | { ok: false } {
    let nodes = 0;
    try {
        const serialized: unknown = JSON.stringify(value, (_key, item: unknown) => {
            nodes += 1;
            if (nodes > MAX_RUNTIME_STATE_VALUE_NODES) throw new Error('node budget exceeded');
            return item;
        });
        if (
            typeof serialized !== 'string' ||
            new TextEncoder().encode(serialized).byteLength > MAX_RUNTIME_STATE_VALUE_BYTES
        ) {
            return { ok: false };
        }
        return { ok: true, value: JSON.parse(serialized) as unknown };
    } catch {
        return { ok: false };
    }
}

export function validPortableRuntimeKey(value: string): boolean {
    return (
        value.length > 0 &&
        value.length <= 512 &&
        !['__proto__', 'constructor', 'prototype'].includes(value)
    );
}

export function validSelection(value: unknown): value is GenerationSelectionInput {
    if (!isRecord(value)) return false;
    if (value.kind === 'legacy_profile') return typeof value.provider_profile_id === 'string';
    return (
        value.kind === 'target' &&
        isRecord(value.target) &&
        typeof value.target.model_route_id === 'string' &&
        typeof value.target.generation_preset_id === 'string'
    );
}

export function cloneSelection(
    selection: GenerationSelectionInput | null,
): GenerationSelectionInput | null {
    return selection === null ? null : structuredClone(selection);
}

function boundedStringRecord(value: unknown, maxValueChars: number): Record<string, string> | null {
    if (!isRecord(value)) return {};
    const entries = Object.entries(value);
    if (entries.length > MAX_RUNTIME_RECORD_KEYS) return null;
    if (
        entries.some(
            ([key, item]) =>
                !validPortableRuntimeKey(key) ||
                typeof item !== 'string' ||
                item.length > maxValueChars,
        )
    ) {
        return null;
    }
    return Object.fromEntries(entries as [string, string][]);
}

function boundedUnknownRecord(value: unknown): Record<string, unknown> | null {
    if (!isRecord(value)) return {};
    const entries = Object.entries(value);
    if (entries.length > MAX_RUNTIME_RECORD_KEYS) return null;
    const result: [string, unknown][] = [];
    for (const [key, item] of entries) {
        if (!validPortableRuntimeKey(key)) return null;
        const bounded = boundedPortableJsonValue(item);
        if (!bounded.ok) return null;
        result.push([key, bounded.value]);
    }
    return Object.fromEntries(result);
}

function isRecord(value: unknown): value is Record<string, unknown> {
    return typeof value === 'object' && value !== null && !Array.isArray(value);
}
