import type {
    CharacterRenderProfileDto,
    CharacterRuntimeKnowledgeDto,
} from '../../lib/ipc/contracts';

export const MAX_PORTABLE_RUNTIME_WORKER_MESSAGE_BYTES = 8 * 1024 * 1024;

export type PortableRuntimeCapability =
    | 'runtime:callbacks'
    | 'chat:read'
    | 'chat:write'
    | 'state:readwrite'
    | 'profile:read'
    | 'lore:read'
    | 'ui:write'
    | 'model:primary'
    | 'model:auxiliary'
    | 'elevated';

export interface PortableRuntimeGrant {
    version: number;
    manifestSha256: string;
    capabilities: PortableRuntimeCapability[];
}

export interface PortableRuntimePersistedState {
    options: Record<string, string>;
    chatVars: Record<string, unknown>;
    state: Record<string, unknown>;
    messageOverrides: Record<string, string>;
    background: string;
    auxiliarySelection: unknown;
}

export interface PortableRuntimeChatMessage {
    id: string;
    role: 'user' | 'char' | 'system';
    data: string;
    time: number;
    virtual: boolean;
}

export interface PortableRuntimeWorkerContext {
    persisted: PortableRuntimePersistedState;
    messages: PortableRuntimeChatMessage[];
    virtualMessage: PortableRuntimeChatMessage | null;
    activeLoreEntries: CharacterRuntimeKnowledgeDto[];
    stopped: boolean;
}

export interface PortableRuntimeWorkerInitialize {
    profile: CharacterRenderProfileDto;
    capabilities: PortableRuntimeCapability[];
    characterName: string;
    characterDescription: string;
    personaName: string;
    personaDescription: string;
    context: PortableRuntimeWorkerContext;
}

export type PortableRuntimeWorkerValue = string | number | boolean | null;

export type PortableRuntimeWorkerOperation =
    | { type: 'initialize'; value: PortableRuntimeWorkerInitialize }
    | { type: 'edit-input'; text: string; context: PortableRuntimeWorkerContext }
    | {
          type: 'invoke';
          name: 'onStart' | 'onOutput' | 'onButtonClick';
          values: unknown[];
          context: PortableRuntimeWorkerContext;
      }
    | { type: 'refresh-display'; context: PortableRuntimeWorkerContext };

export type PortableRuntimeWorkerResult =
    | { type: 'initialized' }
    | { type: 'edited-input'; text: string }
    | { type: 'invoked'; value: PortableRuntimeWorkerValue }
    | { type: 'display'; entries: [string, string][] };

export interface PortableRuntimeWorkerSnapshot {
    persisted: PortableRuntimePersistedState;
    virtualMessage: PortableRuntimeChatMessage | null;
    stopped: boolean;
}

export interface PortableRuntimeRequestMessage {
    channel: 'lorepia-portable-runtime-v1';
    type: 'request';
    requestId: string;
    operation: PortableRuntimeWorkerOperation;
}

export interface PortableRuntimeHostResultMessage {
    channel: 'lorepia-portable-runtime-v1';
    type: 'host-result';
    callId: string;
    ok: boolean;
    value?: unknown;
    error?: string;
}

export type PortableRuntimeMainMessage =
    PortableRuntimeRequestMessage | PortableRuntimeHostResultMessage;

export interface PortableRuntimeResponseMessage {
    channel: 'lorepia-portable-runtime-v1';
    type: 'response';
    requestId: string;
    ok: boolean;
    result?: PortableRuntimeWorkerResult;
    snapshot?: PortableRuntimeWorkerSnapshot;
    error?: {
        code: 'execution-timeout' | 'runtime-error' | 'protocol-error';
        message: string;
    };
}

export interface PortableRuntimeHostCallMessage {
    channel: 'lorepia-portable-runtime-v1';
    type: 'host-call';
    callId: string;
    target: 'primary' | 'auxiliary';
    messages: unknown;
}

export type PortableRuntimeEventMessage =
    | {
          channel: 'lorepia-portable-runtime-v1';
          type: 'state';
          persisted: PortableRuntimePersistedState;
      }
    | { channel: 'lorepia-portable-runtime-v1'; type: 'changed' }
    | {
          channel: 'lorepia-portable-runtime-v1';
          type: 'notice';
          message: string;
          error: boolean;
      };

export type PortableRuntimeWorkerMessage =
    PortableRuntimeResponseMessage | PortableRuntimeHostCallMessage | PortableRuntimeEventMessage;

export function isPortableRuntimeMainMessage(value: unknown): value is PortableRuntimeMainMessage {
    if (!isRecord(value) || value.channel !== 'lorepia-portable-runtime-v1') return false;
    if (value.type === 'host-result') {
        return (
            validIdentifier(value.callId) &&
            typeof value.ok === 'boolean' &&
            (value.ok || typeof value.error === 'string')
        );
    }
    return (
        value.type === 'request' &&
        validIdentifier(value.requestId) &&
        isPortableRuntimeWorkerOperation(value.operation)
    );
}

export function isPortableRuntimeWorkerMessage(
    value: unknown,
): value is PortableRuntimeWorkerMessage {
    if (!isRecord(value) || value.channel !== 'lorepia-portable-runtime-v1') return false;
    if (value.type === 'response') {
        if (!validIdentifier(value.requestId) || typeof value.ok !== 'boolean') return false;
        return value.ok
            ? isPortableRuntimeWorkerResult(value.result) &&
                  isPortableRuntimeWorkerSnapshot(value.snapshot)
            : isPortableRuntimeWorkerError(value.error);
    }
    if (value.type === 'host-call') {
        return (
            validIdentifier(value.callId) &&
            (value.target === 'primary' || value.target === 'auxiliary')
        );
    }
    if (value.type === 'state') return isRecord(value.persisted);
    if (value.type === 'changed') return true;
    return (
        value.type === 'notice' &&
        typeof value.message === 'string' &&
        typeof value.error === 'boolean'
    );
}

export function portableRuntimeMessageWithinLimit(value: unknown): boolean {
    const byteLength = portableRuntimeMessageByteLength(value);
    return byteLength !== null && byteLength <= MAX_PORTABLE_RUNTIME_WORKER_MESSAGE_BYTES;
}

export function portableRuntimeMessageByteLength(value: unknown): number | null {
    try {
        const serialized = JSON.stringify(value);
        return typeof serialized === 'string'
            ? new TextEncoder().encode(serialized).byteLength
            : null;
    } catch {
        return null;
    }
}

export function clonePortableRuntimeMessageValue(
    value: unknown,
    maxBytes = MAX_PORTABLE_RUNTIME_WORKER_MESSAGE_BYTES,
): { ok: true; value: unknown } | { ok: false } {
    try {
        const serialized = JSON.stringify(value);
        if (
            typeof serialized !== 'string' ||
            new TextEncoder().encode(serialized).byteLength >
                Math.min(maxBytes, MAX_PORTABLE_RUNTIME_WORKER_MESSAGE_BYTES)
        ) {
            return { ok: false };
        }
        return { ok: true, value: JSON.parse(serialized) as unknown };
    } catch {
        return { ok: false };
    }
}

function validIdentifier(value: unknown): value is string {
    return typeof value === 'string' && value.length > 0 && value.length <= 128;
}

function isPortableRuntimeWorkerOperation(value: unknown): value is PortableRuntimeWorkerOperation {
    if (!isRecord(value)) return false;
    if (value.type === 'initialize') {
        const initialize = value.value;
        return (
            isRecord(initialize) &&
            isRecord(initialize.profile) &&
            Array.isArray(initialize.capabilities) &&
            initialize.capabilities.every(isPortableRuntimeCapability) &&
            typeof initialize.characterName === 'string' &&
            typeof initialize.characterDescription === 'string' &&
            typeof initialize.personaName === 'string' &&
            typeof initialize.personaDescription === 'string' &&
            isPortableRuntimeWorkerContext(initialize.context)
        );
    }
    if (value.type === 'edit-input') {
        return typeof value.text === 'string' && isPortableRuntimeWorkerContext(value.context);
    }
    if (value.type === 'invoke') {
        return (
            ['onStart', 'onOutput', 'onButtonClick'].includes(String(value.name)) &&
            Array.isArray(value.values) &&
            isPortableRuntimeWorkerContext(value.context)
        );
    }
    return value.type === 'refresh-display' && isPortableRuntimeWorkerContext(value.context);
}

function isPortableRuntimeWorkerContext(value: unknown): value is PortableRuntimeWorkerContext {
    return (
        isRecord(value) &&
        isRecord(value.persisted) &&
        Array.isArray(value.messages) &&
        value.messages.every(isPortableRuntimeChatMessage) &&
        (value.virtualMessage === null || isPortableRuntimeChatMessage(value.virtualMessage)) &&
        Array.isArray(value.activeLoreEntries) &&
        value.activeLoreEntries.every(isRecord) &&
        typeof value.stopped === 'boolean'
    );
}

function isPortableRuntimeChatMessage(value: unknown): value is PortableRuntimeChatMessage {
    return (
        isRecord(value) &&
        typeof value.id === 'string' &&
        ['user', 'char', 'system'].includes(String(value.role)) &&
        typeof value.data === 'string' &&
        typeof value.time === 'number' &&
        Number.isFinite(value.time) &&
        typeof value.virtual === 'boolean'
    );
}

function isPortableRuntimeWorkerResult(value: unknown): value is PortableRuntimeWorkerResult {
    if (!isRecord(value)) return false;
    if (value.type === 'initialized') return true;
    if (value.type === 'edited-input') return typeof value.text === 'string';
    if (value.type === 'invoked') return isPortableRuntimeWorkerValue(value.value);
    return (
        value.type === 'display' &&
        Array.isArray(value.entries) &&
        value.entries.every(
            (entry: unknown) =>
                Array.isArray(entry) &&
                entry.length === 2 &&
                typeof entry[0] === 'string' &&
                typeof entry[1] === 'string',
        )
    );
}

function isPortableRuntimeWorkerSnapshot(value: unknown): value is PortableRuntimeWorkerSnapshot {
    return (
        isRecord(value) &&
        isRecord(value.persisted) &&
        (value.virtualMessage === null || isPortableRuntimeChatMessage(value.virtualMessage)) &&
        typeof value.stopped === 'boolean'
    );
}

function isPortableRuntimeWorkerError(
    value: unknown,
): value is NonNullable<PortableRuntimeResponseMessage['error']> {
    return (
        isRecord(value) &&
        (value.code === 'execution-timeout' ||
            value.code === 'runtime-error' ||
            value.code === 'protocol-error') &&
        typeof value.message === 'string'
    );
}

function isPortableRuntimeWorkerValue(value: unknown): value is PortableRuntimeWorkerValue {
    return (
        value === null ||
        typeof value === 'string' ||
        typeof value === 'number' ||
        typeof value === 'boolean'
    );
}

function isPortableRuntimeCapability(value: unknown): value is PortableRuntimeCapability {
    return (
        typeof value === 'string' &&
        [
            'runtime:callbacks',
            'chat:read',
            'chat:write',
            'state:readwrite',
            'profile:read',
            'lore:read',
            'ui:write',
            'model:primary',
            'model:auxiliary',
            'elevated',
        ].includes(value)
    );
}

function isRecord(value: unknown): value is Record<string, unknown> {
    return typeof value === 'object' && value !== null && !Array.isArray(value);
}
