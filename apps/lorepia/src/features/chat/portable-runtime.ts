import { LuaFactory, LuaMultiReturn, type LuaEngine } from 'wasmoon';
import wasmUrl from 'wasmoon/dist/glue.wasm?url';

import type {
    CharacterRenderProfileDto,
    CharacterRuntimeKnowledgeDto,
    GenerationSelectionInput,
    LorepiaClient,
    MessageDto,
    RuntimePromptMessageInput,
} from '../../lib/ipc/contracts';
import { t } from '../../lib/i18n';
import { renderPortableMacros } from './portable-display';
import LUA_SANDBOX_HARDENING from './portable-runtime-sandbox.lua?raw';
import { runPortableRegex } from './portable-regex';

const TRIGGER_ID = 'character-runtime';
const MAX_PERSISTED_RUNTIME_BYTES = 4 * 1024 * 1024;
const MAX_RUNTIME_NOTICE_CHARS = 4_096;
const MAX_RUNTIME_MODEL_CALLS = 8;
const MAX_RUNTIME_MODEL_PROMPT_CHARS = 64 * 1024;
const MAX_RUNTIME_LUA_SLICE_MS = 100;
const MAX_RUNTIME_LUA_MEMORY_BYTES = 32 * 1024 * 1024;
const MAX_RUNTIME_EVENT_MS = 30_000;
const MAX_RUNTIME_RECORD_KEYS = 256;
const MAX_RUNTIME_STATE_VALUE_BYTES = 64 * 1024;
const MAX_RUNTIME_STATE_VALUE_NODES = 2_048;
const MAX_RUNTIME_MESSAGE_OVERRIDE_CHARS = 262_144;
const MAX_RUNTIME_BACKGROUND_CHARS = 1024 * 1024;
const MAX_RUNTIME_LORE_ENTRIES = 512;
const MAX_RUNTIME_LORE_KEY_TESTS = 1_024;
const MAX_RUNTIME_LORE_REGEX_TESTS = 64;
const MAX_RUNTIME_LORE_SOURCE_CHARS = 262_144;
// Capture the intrinsic before imported Lua runs so prototype substitution cannot replace it.
// eslint-disable-next-line @typescript-eslint/unbound-method
const NATIVE_PROMISE_THEN = Promise.prototype.then;
const IGNORE_PROMISE_SETTLEMENT = () => undefined;

function isNativePromise(value: unknown): boolean {
    if (value === null || (typeof value !== 'object' && typeof value !== 'function')) {
        return false;
    }
    try {
        void Reflect.apply(NATIVE_PROMISE_THEN, value, [
            IGNORE_PROMISE_SETTLEMENT,
            IGNORE_PROMISE_SETTLEMENT,
        ]);
        return true;
    } catch {
        return false;
    }
}

const LUA_ASYNC_BRIDGE = String.raw`
local runtime_promise_create = Promise.create
local runtime_promise_methods = debug.getmetatable(Promise.resolve(nil)).__index
local runtime_promise_await = runtime_promise_methods.await
local runtime_promise_catch = runtime_promise_methods.catch
local runtime_promise_finally = runtime_promise_methods.finally
local runtime_error = error
local runtime_is_promise = __hostIsRuntimePromise
local runtime_schedule_turn = __hostRuntimeYield
local runtime_type = type

runtime_promise_methods.next = nil
runtime_promise_methods.catch = nil
runtime_promise_methods.finally = nil
runtime_promise_methods.await = function(self)
    if not runtime_is_promise(self) then
        runtime_error("await requires a native runtime Promise", 0)
    end
    return runtime_promise_await(self)
end

local function runtime_safe_result(value)
    local kind = runtime_type(value)
    if kind == "nil" or kind == "boolean" or kind == "number" or kind == "string" then
        return value
    end
    return nil
end

function async(callback)
    return function(...)
        local co = coroutine.create(callback)
        local safe, result = coroutine.resume(co, ...)

        return runtime_promise_create(function(resolve, reject)
            local step

            local function reject_continuation(error)
                return reject(error)
            end

            local function after_turn(next_step)
                local scheduled = runtime_schedule_turn()
                local continuation = runtime_promise_finally(scheduled, next_step)
                runtime_promise_catch(continuation, reject_continuation)
            end

            local function continue_after_result()
                after_turn(step)
            end

            step = function()
                if coroutine.status(co) == "dead" then
                    if safe then
                        return resolve(runtime_safe_result(result))
                    end
                    return reject(result)
                end

                safe, result = coroutine.resume(co)
                if safe and runtime_is_promise(result) then
                    local continuation = runtime_promise_finally(result, continue_after_result)
                    runtime_promise_catch(continuation, reject_continuation)
                else
                    after_turn(step)
                end
            end

            if safe and runtime_is_promise(result) then
                local continuation = runtime_promise_finally(result, continue_after_result)
                runtime_promise_catch(continuation, reject_continuation)
            else
                after_turn(step)
            end
        end)
    end
end

`;

const LUA_MODEL_BRIDGE = String.raw`
function LLM(triggerId, messages, useTools, options)
    return __hostMainGeneration(messages):await()
end

function axLLM(triggerId, messages, useTools, options)
    return __hostAuxGeneration(messages):await()
end
`;

export type PortableRuntimeCapability =
    | 'runtime:callbacks'
    | 'chat:read'
    | 'chat:write'
    | 'state:readwrite'
    | 'profile:read'
    | 'lore:read'
    | 'ui:write'
    | 'model:generate'
    | 'elevated';

export interface PortableRuntimeGrant {
    version: number;
    manifestSha256: string;
    capabilities: PortableRuntimeCapability[];
}

const PORTABLE_RUNTIME_CAPABILITIES: readonly PortableRuntimeCapability[] = [
    'runtime:callbacks',
    'chat:read',
    'chat:write',
    'state:readwrite',
    'profile:read',
    'lore:read',
    'ui:write',
    'model:generate',
];

export function requiredPortableRuntimeCapabilities(
    profile: CharacterRenderProfileDto,
): PortableRuntimeCapability[] {
    const capabilities = [...PORTABLE_RUNTIME_CAPABILITIES];
    if (profile.runtime_scripts.some((script) => script.elevated_access)) {
        capabilities.push('elevated');
    }
    return capabilities;
}

export async function createPortableRuntimeGrant(
    profile: CharacterRenderProfileDto,
    capabilities: readonly PortableRuntimeCapability[] = requiredPortableRuntimeCapabilities(
        profile,
    ),
): Promise<PortableRuntimeGrant> {
    const reviewedCapabilities = canonicalCapabilities(capabilities);
    return {
        version: 1,
        manifestSha256: await portableRuntimeManifestSha256(profile, reviewedCapabilities),
        capabilities: reviewedCapabilities,
    };
}

export async function validatePortableRuntimeGrant(
    profile: CharacterRenderProfileDto,
    grant: PortableRuntimeGrant,
): Promise<boolean> {
    if (grant.version !== 1 || !/^[0-9a-f]{64}$/.test(grant.manifestSha256)) return false;
    const capabilities = canonicalCapabilities(grant.capabilities);
    if (
        capabilities.length !== grant.capabilities.length ||
        capabilities.some((capability, index) => capability !== grant.capabilities[index])
    ) {
        return false;
    }
    return grant.manifestSha256 === (await portableRuntimeManifestSha256(profile, capabilities));
}

async function portableRuntimeManifestSha256(
    profile: CharacterRenderProfileDto,
    capabilities: readonly PortableRuntimeCapability[],
): Promise<string> {
    const subtle = globalThis.crypto.subtle;
    const manifest = JSON.stringify({
        version: 1,
        profile,
        capabilities,
    });
    const digest = await subtle.digest('SHA-256', new TextEncoder().encode(manifest));
    return [...new Uint8Array(digest)].map((byte) => byte.toString(16).padStart(2, '0')).join('');
}

function canonicalCapabilities(
    capabilities: readonly PortableRuntimeCapability[],
): PortableRuntimeCapability[] {
    const allowed = new Set<PortableRuntimeCapability>([
        ...PORTABLE_RUNTIME_CAPABILITIES,
        'elevated',
    ]);
    return [...new Set(capabilities.filter((capability) => allowed.has(capability)))].sort();
}

export interface PortableRuntimeToggle {
    key: string;
    label: string;
    kind: 'select' | 'toggle' | 'text';
    choices: string[];
}

export interface PortableRuntimeOptions {
    profile: CharacterRenderProfileDto;
    grant: PortableRuntimeGrant;
    conversationId: string;
    branchId: string;
    characterName: string;
    characterDescription: string;
    personaName?: string;
    personaDescription?: string;
    client: LorepiaClient;
    primarySelection: () => GenerationSelectionInput | null;
    onChanged: () => void;
    onNotice: (message: string, error: boolean) => void;
    storage?: Storage;
    luaFactory?: LuaFactory;
    /** Host policy override used by bounded tests; imported content cannot set this. */
    eventTimeoutMs?: number;
}

export interface PreparedPortableInput {
    text: string;
    shouldSend: boolean;
}

interface RuntimeChatMessage {
    id: string;
    role: 'user' | 'char' | 'system';
    data: string;
    time: number;
    virtual: boolean;
}

interface PersistedRuntimeState {
    options: Record<string, string>;
    chatVars: Record<string, unknown>;
    state: Record<string, unknown>;
    messageOverrides: Record<string, string>;
    background: string;
    auxiliarySelection: GenerationSelectionInput | null;
}

interface LoreWorkBudget {
    keyTestsRemaining: number;
    regexTestsRemaining: number;
}

type EditCallback = (...values: unknown[]) => unknown;

export class PortableCharacterRuntime {
    readonly toggles: PortableRuntimeToggle[];

    private readonly profile: CharacterRenderProfileDto;
    private readonly client: LorepiaClient;
    private readonly primarySelection: () => GenerationSelectionInput | null;
    private readonly onChanged: () => void;
    private readonly onNotice: (message: string, error: boolean) => void;
    private readonly storage: Storage | undefined;
    private readonly storageKey: string;
    private readonly characterName: string;
    private readonly characterDescription: string;
    private readonly personaName: string;
    private readonly personaDescription: string;
    private readonly luaFactory: LuaFactory;
    private readonly capabilities: ReadonlySet<PortableRuntimeCapability>;
    private readonly eventTimeoutMs: number;
    private readonly editCallbacks = new Map<string, EditCallback[]>();
    private readonly displayCache = new Map<string, string>();
    private readonly sleepTimeouts = new Set<ReturnType<typeof globalThis.setTimeout>>();
    private activeLoreEntries: CharacterRuntimeKnowledgeDto[] = [];

    private engine: LuaEngine | null = null;
    private messages: MessageDto[] = [];
    private virtualMessage: RuntimeChatMessage | null = null;
    private stopped = false;
    private closed = false;
    private changedQueued = false;
    private modelCallCount = 0;
    private persisted: PersistedRuntimeState;

    private constructor(options: PortableRuntimeOptions) {
        this.profile = options.profile;
        this.capabilities = new Set(options.grant.capabilities);
        this.client = options.client;
        this.primarySelection = options.primarySelection;
        this.onChanged = options.onChanged;
        this.onNotice = options.onNotice;
        this.storage = options.storage ?? browserStorage();
        this.characterName = options.characterName;
        this.characterDescription = options.characterDescription;
        const personaName = options.personaName?.trim();
        this.personaName =
            personaName === undefined || personaName === ''
                ? t('chat.runtime.persona.default')
                : personaName;
        this.personaDescription = options.personaDescription ?? '';
        this.luaFactory = options.luaFactory ?? new LuaFactory(wasmUrl);
        this.eventTimeoutMs = Math.min(
            MAX_RUNTIME_EVENT_MS,
            Math.max(25, options.eventTimeoutMs ?? MAX_RUNTIME_EVENT_MS),
        );
        this.storageKey = [
            'lorepia.character-runtime.v1',
            options.profile.character_id,
            options.profile.character_content_revision_id ?? 'legacy',
            options.conversationId,
            options.branchId,
        ].join(':');
        this.toggles = parsePortableRuntimeToggles(options.profile.toggle_schema);
        this.persisted = this.loadState();
    }

    static async create(options: PortableRuntimeOptions): Promise<PortableCharacterRuntime> {
        if (!(await validatePortableRuntimeGrant(options.profile, options.grant))) {
            throw new Error(t('chat.runtime.approval_required'));
        }
        const runtime = new PortableCharacterRuntime(options);
        try {
            await runtime.runWithEventDeadline(() => runtime.initialize());
            return runtime;
        } catch (error) {
            runtime.close();
            throw error;
        }
    }

    get backgroundMarkup(): string {
        return this.persisted.background;
    }

    get variables(): Record<string, string> {
        return {
            ...this.generationVariables,
            ...Object.fromEntries(
                Object.entries(this.persisted.chatVars).map(([key, value]) => [
                    key,
                    safeText(value),
                ]),
            ),
        };
    }

    get generationVariables(): Record<string, string> {
        return { ...this.profile.initial_variables, ...this.persisted.options };
    }

    get auxiliarySelection(): GenerationSelectionInput | null {
        return cloneSelection(this.persisted.auxiliarySelection);
    }

    setMessages(messages: MessageDto[]): void {
        this.messages = messages;
        const retained = new Set(messages.map((message) => message.id));
        const messageOverrides = Object.fromEntries(
            Object.entries(this.persisted.messageOverrides).filter(([id]) => retained.has(id)),
        );
        if (
            Object.keys(messageOverrides).length !==
            Object.keys(this.persisted.messageOverrides).length
        ) {
            this.commitPersisted({ ...this.persisted, messageOverrides });
        }
    }

    effectiveText(message: MessageDto): string {
        return this.persisted.messageOverrides[message.id] ?? message.content;
    }

    displayText(message: MessageDto): string {
        return this.displayCache.get(message.id) ?? this.effectiveText(message);
    }

    optionValue(key: string): string {
        return this.lookupVariable(key) ?? '';
    }

    async setOption(key: string, value: string): Promise<void> {
        if (!this.toggles.some((toggle) => toggle.key === key)) return;
        await this.runWithEventDeadline(async () => {
            const options = updateStringRecord(
                this.persisted.options,
                key,
                value,
                MAX_RUNTIME_RECORD_KEYS,
                16_384,
            );
            if (options === null || !this.commitPersisted({ ...this.persisted, options })) return;
            await this.refreshDisplay();
            this.notifyChanged();
        });
    }

    setAuxiliarySelection(selection: GenerationSelectionInput | null): void {
        if (
            this.commitPersisted({
                ...this.persisted,
                auxiliarySelection: cloneSelection(selection),
            })
        ) {
            this.notifyChanged();
        }
    }

    async prepareInput(text: string): Promise<PreparedPortableInput> {
        this.assertOpen();
        return await this.runWithEventDeadline(async () => {
            this.stopped = false;
            let edited: unknown = text;
            for (const callback of this.editCallbacks.get('editInput') ?? []) {
                edited = await Promise.resolve(
                    callback(TRIGGER_ID, typeof edited === 'string' ? edited : text),
                );
            }
            const prepared = typeof edited === 'string' ? edited : text;
            this.virtualMessage = {
                id: '__runtime_pending_user__',
                role: 'user',
                data: prepared,
                time: Math.floor(Date.now() / 1_000),
                virtual: true,
            };
            let result: unknown;
            try {
                await this.refreshActiveLore();
                result = await this.invokeGlobal('onStart', TRIGGER_ID);
            } finally {
                this.virtualMessage = null;
            }
            const startAllowed = typeof result !== 'boolean' || result;
            const shouldSend = !this.chatStopped() && startAllowed && prepared.trim() !== '';
            await this.refreshDisplay();
            this.notifyChanged();
            return { text: prepared, shouldSend };
        });
    }

    async afterOutput(messages: MessageDto[]): Promise<void> {
        this.assertOpen();
        await this.runWithEventDeadline(async () => {
            this.setMessages(messages);
            await this.refreshActiveLore();
            await this.invokeGlobal('onOutput', TRIGGER_ID);
            await this.refreshDisplay();
            this.notifyChanged();
        });
    }

    async handleAction(action: string): Promise<void> {
        if (action.length === 0 || action.length > 512) return;
        this.assertOpen();
        await this.runWithEventDeadline(async () => {
            await this.refreshActiveLore();
            await this.invokeGlobal('onButtonClick', TRIGGER_ID, action);
            await this.refreshDisplay();
            this.notifyChanged();
        });
    }

    async refreshDisplay(): Promise<void> {
        this.assertOpen();
        await this.runWithEventDeadline(async () => {
            const callbacks = this.editCallbacks.get('editDisplay') ?? [];
            this.displayCache.clear();
            if (callbacks.length === 0) return;
            const messages = this.runtimeMessages();
            for (let index = 0; index < messages.length; index += 1) {
                const runtimeMessage = messages[index];
                const sourceMessage = this.messages.find(
                    (message) => message.id === runtimeMessage?.id,
                );
                if (runtimeMessage === undefined || sourceMessage === undefined) continue;
                let display: unknown = runtimeMessage.data;
                for (const callback of callbacks) {
                    display = await Promise.resolve(
                        callback(
                            TRIGGER_ID,
                            typeof display === 'string' ? display : runtimeMessage.data,
                            { index },
                        ),
                    );
                }
                this.displayCache.set(
                    sourceMessage.id,
                    typeof display === 'string' ? display : runtimeMessage.data,
                );
            }
        });
    }

    close(): void {
        if (this.closed) return;
        this.closed = true;
        for (const timeout of this.sleepTimeouts) globalThis.clearTimeout(timeout);
        this.sleepTimeouts.clear();
        const engine = this.engine;
        this.engine = null;
        try {
            engine?.global.close();
        } catch {
            // Closing is best-effort after the runtime has already been made unreachable.
        }
        this.editCallbacks.clear();
        this.displayCache.clear();
    }

    private async initialize(): Promise<void> {
        const engine = await this.luaFactory.createEngine({
            injectObjects: true,
            enableProxy: false,
            traceAllocations: true,
            functionTimeout: MAX_RUNTIME_LUA_SLICE_MS,
        });
        engine.global.setMemoryMax(MAX_RUNTIME_LUA_MEMORY_BYTES);
        this.engine = engine;
        this.installHostFunctions(engine);
        await executeLuaSource(engine, LUA_ASYNC_BRIDGE);
        if (this.capabilities.has('model:generate')) {
            await executeLuaSource(engine, LUA_MODEL_BRIDGE);
        }
        await executeLuaSource(engine, LUA_SANDBOX_HARDENING);
        await this.refreshActiveLore();
        for (const script of this.profile.runtime_scripts) {
            if (script.language.trim().toLowerCase() !== 'lua' || script.source.trim() === '') {
                continue;
            }
            await executeLuaSource(engine, script.source);
        }
        await this.refreshDisplay();
    }

    private installHostFunctions(engine: LuaEngine): void {
        const set = (name: string, value: unknown): void => engine.global.set(name, value);
        set('__hostIsRuntimePromise', isNativePromise);
        set(
            '__hostRuntimeYield',
            () =>
                new Promise<void>((resolve) => {
                    const timeout = globalThis.setTimeout(() => {
                        this.sleepTimeouts.delete(timeout);
                        if (!this.closed) resolve();
                    }, 0);
                    this.sleepTimeouts.add(timeout);
                }),
        );
        if (this.capabilities.has('runtime:callbacks')) {
            set('listenEdit', (kind: unknown, callback: unknown) => {
                if (typeof kind !== 'string' || typeof callback !== 'function') return;
                if (!['editInput', 'editDisplay'].includes(kind)) return;
                const callbacks = this.editCallbacks.get(kind) ?? [];
                if (callbacks.length >= 64) return;
                callbacks.push(callback as EditCallback);
                this.editCallbacks.set(kind, callbacks);
            });
            set('cbs', (first: unknown, second?: unknown) =>
                this.expandMacros(typeof second === 'string' ? second : safeText(first)),
            );
            set('sleep', (first: unknown, second?: unknown) => {
                const milliseconds = Number(second ?? first);
                return new Promise<void>((resolve) => {
                    const timeout = globalThis.setTimeout(
                        () => {
                            this.sleepTimeouts.delete(timeout);
                            if (!this.closed) resolve();
                        },
                        Number.isFinite(milliseconds)
                            ? Math.min(5_000, Math.max(0, milliseconds))
                            : 0,
                    );
                    this.sleepTimeouts.add(timeout);
                });
            });
        }
        if (this.capabilities.has('chat:read')) {
            set('getChatLength', () => this.runtimeMessages().length);
            set('getChat', (_triggerId: unknown, index: unknown) =>
                luaNullable(this.runtimeChatAt(index)),
            );
            set('getFullChat', () => this.runtimeMessages().map(runtimeMessageValue));
        }
        if (this.capabilities.has('chat:write')) {
            set('setChat', (_triggerId: unknown, index: unknown, content: unknown) => {
                const message = this.runtimeMessageAt(index);
                if (message === undefined || typeof content !== 'string') return false;
                if (message.virtual) {
                    if (content.length > MAX_RUNTIME_MESSAGE_OVERRIDE_CHARS) return false;
                    if (this.virtualMessage !== null) this.virtualMessage.data = content;
                } else {
                    const messageOverrides = updateStringRecord(
                        this.persisted.messageOverrides,
                        message.id,
                        content,
                        MAX_RUNTIME_RECORD_KEYS,
                        MAX_RUNTIME_MESSAGE_OVERRIDE_CHARS,
                    );
                    if (
                        messageOverrides === null ||
                        !this.commitPersisted({ ...this.persisted, messageOverrides })
                    ) {
                        return false;
                    }
                }
                this.notifyChanged();
                return true;
            });
            set('removeChat', (_triggerId: unknown, index: unknown) => {
                const messages = this.runtimeMessages();
                const resolved = resolveRuntimeIndex(index, messages.length);
                if (resolved === null) return false;
                if (messages[resolved]?.virtual) {
                    this.virtualMessage = null;
                    this.notifyChanged();
                    return true;
                }
                return false;
            });
            set('reloadChat', () => {
                this.notifyChanged();
                return true;
            });
            set('stopChat', () => {
                this.stopped = true;
                return true;
            });
        }
        if (this.capabilities.has('state:readwrite')) {
            set('getChatVar', (_triggerId: unknown, key: unknown) =>
                luaNullable(typeof key === 'string' ? this.persisted.chatVars[key] : undefined),
            );
            set('setChatVar', (_triggerId: unknown, key: unknown, value: unknown) => {
                if (typeof key !== 'string' || key.length === 0 || key.length > 512) return false;
                const chatVars = updateUnknownRecord(this.persisted.chatVars, key, value);
                if (chatVars === null || !this.commitPersisted({ ...this.persisted, chatVars })) {
                    return false;
                }
                this.notifyChanged();
                return true;
            });
            set('getState', (_triggerId: unknown, key: unknown) =>
                luaNullable(typeof key === 'string' ? this.persisted.state[key] : undefined),
            );
            set('setState', (_triggerId: unknown, key: unknown, value: unknown) => {
                if (typeof key !== 'string' || key.length === 0 || key.length > 512) return false;
                const state = updateUnknownRecord(this.persisted.state, key, value);
                return state !== null && this.commitPersisted({ ...this.persisted, state });
            });
        }
        if (this.capabilities.has('profile:read')) {
            set('getGlobalVar', (_triggerId: unknown, key: unknown) =>
                luaNullable(typeof key === 'string' ? this.lookupVariable(key) : undefined),
            );
            set('getPersonaName', () => this.personaName);
            set('getPersonaDescription', () => this.personaDescription);
            set('getDescription', () => this.characterDescription);
        }
        if (this.capabilities.has('lore:read')) {
            set('getLoreBooks', (_triggerId: unknown, name: unknown) => this.loreBooks(name));
            set('loadLoreBooks', () => this.activeLoreBooks());
        }
        if (this.capabilities.has('ui:write')) {
            set('getBackgroundEmbedding', () => this.persisted.background);
            set('setBackgroundEmbedding', (_triggerId: unknown, value: unknown) => {
                if (typeof value !== 'string' || value.length > MAX_RUNTIME_BACKGROUND_CHARS) {
                    return false;
                }
                if (!this.commitPersisted({ ...this.persisted, background: value })) return false;
                this.notifyChanged();
                return true;
            });
            set('alertNormal', (_triggerId: unknown, message: unknown) => {
                this.emitNotice(message, false);
            });
            set('alertError', (_triggerId: unknown, message: unknown) => {
                this.emitNotice(message, true);
            });
        }
        if (this.capabilities.has('model:generate')) {
            set('__hostMainGeneration', (messages: unknown) =>
                this.generate(this.primarySelection(), messages),
            );
            set('__hostAuxGeneration', (messages: unknown) =>
                this.generate(
                    this.persisted.auxiliarySelection ?? this.primarySelection(),
                    messages,
                ),
            );
        }
        if (this.capabilities.has('elevated')) {
            set('log', () => undefined);
        }
    }

    private async generate(
        selection: GenerationSelectionInput | null,
        rawMessages: unknown,
    ): Promise<{ success: boolean; result: string }> {
        try {
            if (selection === null) throw new Error(t('chat.runtime.generation.model_missing'));
            if (this.client.generateRuntimeText === undefined) {
                throw new Error(t('chat.runtime.generation.unsupported'));
            }
            const messages = runtimePromptMessages(rawMessages);
            const promptChars = messages.reduce(
                (total, message) => total + message.content.length,
                0,
            );
            if (
                this.modelCallCount >= MAX_RUNTIME_MODEL_CALLS ||
                promptChars > MAX_RUNTIME_MODEL_PROMPT_CHARS
            ) {
                throw new Error(t('chat.runtime.generation.budget_exhausted'));
            }
            this.modelCallCount += 1;
            const response = await this.client.generateRuntimeText({ selection, messages });
            return { success: true, result: response.result };
        } catch (error) {
            const result = safeText(error);
            return { success: false, result };
        }
    }

    private runtimeMessages(): RuntimeChatMessage[] {
        const messages = this.messages.map((message) => ({
            id: message.id,
            role: runtimeRole(message.role),
            data: this.effectiveText(message),
            time: Math.floor(Date.parse(message.created_at) / 1_000) || 0,
            virtual: false,
        }));
        if (this.virtualMessage !== null) messages.push(this.virtualMessage);
        return messages;
    }

    private runtimeMessageAt(index: unknown): RuntimeChatMessage | undefined {
        const messages = this.runtimeMessages();
        const resolved = resolveRuntimeIndex(index, messages.length);
        return resolved === null ? undefined : messages[resolved];
    }

    private runtimeChatAt(index: unknown): ReturnType<typeof runtimeMessageValue> | undefined {
        const message = this.runtimeMessageAt(index);
        return message === undefined ? undefined : runtimeMessageValue(message);
    }

    private lookupVariable(requested: string): string | undefined {
        const variables = this.generationVariables;
        const stripped = requested.startsWith('toggle_') ? requested.slice(7) : requested;
        return (
            variables[requested] ??
            variables[stripped] ??
            variables[`toggle_${requested}`] ??
            variables[`toggle_${stripped}`]
        );
    }

    private loreBooks(name: unknown): { content: string; data: string; name: string }[] {
        if (typeof name !== 'string') return [];
        return this.profile.runtime_knowledge
            .filter((entry) => entry.enabled && entry.name === name)
            .slice(0, MAX_RUNTIME_LORE_ENTRIES)
            .map((entry) => this.loreBookValue(entry));
    }

    private activeLoreBooks(): { content: string; data: string; name: string }[] {
        return this.activeLoreEntries.map((entry) => this.loreBookValue(entry));
    }

    private async refreshActiveLore(): Promise<void> {
        const source = this.runtimeMessages()
            .map((message) => message.data)
            .join('\n')
            .slice(-MAX_RUNTIME_LORE_SOURCE_CHARS);
        const candidates = this.profile.runtime_knowledge
            .filter((entry) => entry.enabled)
            .slice(0, MAX_RUNTIME_LORE_ENTRIES);
        const budget: LoreWorkBudget = {
            keyTestsRemaining: MAX_RUNTIME_LORE_KEY_TESTS,
            regexTestsRemaining: MAX_RUNTIME_LORE_REGEX_TESTS,
        };
        const active: CharacterRuntimeKnowledgeDto[] = [];
        for (const entry of candidates) {
            if (await loreEntryActive(entry, source, budget)) active.push(entry);
        }
        this.activeLoreEntries = active;
    }

    private expandMacros(source: string): string {
        const messages = this.runtimeMessages();
        const lastCharacter = [...messages]
            .reverse()
            .find((message) => message.role === 'char')?.data;
        const expanded = source
            .replaceAll('{{char}}', this.characterName)
            .replaceAll('{{user}}', this.personaName)
            .replaceAll('{{description}}', this.characterDescription)
            .replaceAll('{{lastcharmessage}}', lastCharacter ?? '')
            .replace(
                /\{\{getglobalvar::([^{}]+)}}/gi,
                (_match, key: string) => this.lookupVariable(key.trim()) ?? '',
            )
            .replace(/\{\{getvar::([^{}]+)}}/gi, (_match, key: string) =>
                safeText(this.persisted.chatVars[key.trim()]),
            );
        return renderPortableMacros(
            expanded,
            {
                variables: this.variables,
                chatIndex: Math.max(0, messages.length - 1),
                lastMessageId: Math.max(0, messages.length - 1),
                lastCharacterMessage: lastCharacter ?? '',
                characterName: this.characterName,
                userName: this.personaName,
            },
            expanded,
        );
    }

    private loreBookValue(entry: CharacterRuntimeKnowledgeDto): {
        content: string;
        data: string;
        name: string;
    } {
        const content = this.expandMacros(entry.content);
        return { content, data: content, name: entry.name };
    }

    private async invokeGlobal(name: string, ...values: unknown[]): Promise<unknown> {
        const engine = this.engine;
        if (engine === null) return undefined;
        const callback = engine.global.get(name) as unknown;
        if (typeof callback !== 'function') return undefined;
        return await Promise.resolve((callback as EditCallback)(...values));
    }

    private async runWithEventDeadline<T>(operation: () => Promise<T>): Promise<T> {
        let timeout: ReturnType<typeof globalThis.setTimeout> | undefined;
        const deadline = new Promise<never>((_resolve, reject) => {
            timeout = globalThis.setTimeout(() => {
                this.close();
                reject(new Error(t('chat.runtime.event_timeout')));
            }, this.eventTimeoutMs);
        });
        try {
            return await Promise.race([Promise.resolve().then(operation), deadline]);
        } finally {
            if (timeout !== undefined) globalThis.clearTimeout(timeout);
        }
    }

    private loadState(): PersistedRuntimeState {
        const fallback: PersistedRuntimeState = {
            options: {},
            chatVars: {},
            state: {},
            messageOverrides: {},
            background: this.profile.background_markup,
            auxiliarySelection: null,
        };
        try {
            const serialized = this.storage?.getItem(this.storageKey);
            if (serialized === null || serialized === undefined) return fallback;
            if (new TextEncoder().encode(serialized).byteLength > MAX_PERSISTED_RUNTIME_BYTES) {
                return fallback;
            }
            const parsed = JSON.parse(serialized) as Partial<PersistedRuntimeState>;
            const candidate: PersistedRuntimeState = {
                options: boundedStringRecord(parsed.options, 16_384) ?? {},
                chatVars: boundedUnknownRecord(parsed.chatVars) ?? {},
                state: boundedUnknownRecord(parsed.state) ?? {},
                messageOverrides:
                    boundedStringRecord(
                        parsed.messageOverrides,
                        MAX_RUNTIME_MESSAGE_OVERRIDE_CHARS,
                    ) ?? {},
                background:
                    typeof parsed.background === 'string'
                        ? parsed.background.slice(0, MAX_RUNTIME_BACKGROUND_CHARS)
                        : this.profile.background_markup.slice(0, MAX_RUNTIME_BACKGROUND_CHARS),
                auxiliarySelection: validSelection(parsed.auxiliarySelection)
                    ? cloneSelection(parsed.auxiliarySelection)
                    : null,
            };
            return serializePersisted(candidate) === null ? fallback : candidate;
        } catch {
            return fallback;
        }
    }

    private commitPersisted(candidate: PersistedRuntimeState): boolean {
        const serialized = serializePersisted(candidate);
        if (serialized === null) return false;
        this.persisted = candidate;
        try {
            this.storage?.setItem(this.storageKey, serialized);
        } catch {
            // A bounded in-memory state remains usable if browser storage is unavailable.
        }
        return true;
    }

    private notifyChanged(): void {
        if (this.changedQueued || this.closed) return;
        this.changedQueued = true;
        queueMicrotask(() => {
            this.changedQueued = false;
            if (!this.closed) this.onChanged();
        });
    }

    private emitNotice(value: unknown, error: boolean): void {
        const message = safeText(value).slice(0, MAX_RUNTIME_NOTICE_CHARS);
        if (message !== '') this.onNotice(message, error);
    }

    private chatStopped(): boolean {
        return this.stopped;
    }

    private assertOpen(): void {
        if (this.closed || this.engine === null) {
            throw new Error(t('chat.runtime.not_ready'));
        }
    }
}

async function executeLuaSource(engine: LuaEngine, source: string): Promise<void> {
    const thread = engine.global.newThread();
    const threadIndex = engine.global.getTop();
    try {
        thread.loadString(source);
        await thread.run(0, { timeout: MAX_RUNTIME_LUA_SLICE_MS });
    } finally {
        engine.global.remove(threadIndex);
    }
}

export function parsePortableRuntimeToggles(schema: string): PortableRuntimeToggle[] {
    const toggles: PortableRuntimeToggle[] = [];
    for (const sourceLine of schema.split(/\r?\n/)) {
        const line = sourceLine.trim();
        if (line === '' || line.startsWith('=')) continue;
        const [key = '', label = '', rawKind = '', rawChoices = ''] = line.split('=');
        const kind = rawKind.trim().toLowerCase();
        if (
            key.trim() === '' ||
            label.trim() === '' ||
            !['select', 'toggle', 'checkbox', 'text', 'textarea'].includes(kind)
        ) {
            continue;
        }
        toggles.push({
            key: key.trim(),
            label: label.trim(),
            kind:
                kind === 'select'
                    ? 'select'
                    : kind === 'toggle' || kind === 'checkbox'
                      ? 'toggle'
                      : 'text',
            choices:
                kind === 'select'
                    ? rawChoices
                          .split(',')
                          .map((choice) => choice.trim())
                          .filter(Boolean)
                          .slice(0, 128)
                    : [],
        });
        if (toggles.length >= MAX_RUNTIME_RECORD_KEYS) break;
    }
    return toggles;
}

function runtimePromptMessages(value: unknown): RuntimePromptMessageInput[] {
    if (!Array.isArray(value)) throw new Error(t('chat.runtime.prompt.invalid'));
    const messages: RuntimePromptMessageInput[] = [];
    for (const item of value) {
        if (!isRecord(item) || typeof item.content !== 'string' || item.content.trim() === '') {
            continue;
        }
        const role = item.role;
        messages.push({
            role:
                role === 'char' || role === 'assistant'
                    ? 'assistant'
                    : role === 'system'
                      ? 'system'
                      : 'user',
            content: item.content,
        });
    }
    if (messages.length === 0) throw new Error(t('chat.runtime.prompt.empty'));
    return messages;
}

function runtimeRole(role: MessageDto['role']): RuntimeChatMessage['role'] {
    if (role === 'assistant') return 'char';
    if (role === 'user') return 'user';
    return 'system';
}

function runtimeMessageValue(message: RuntimeChatMessage): {
    role: RuntimeChatMessage['role'];
    data: string;
    time: number;
} {
    return { role: message.role, data: message.data, time: message.time };
}

function resolveRuntimeIndex(value: unknown, length: number): number | null {
    const numeric = Number(value);
    if (!Number.isInteger(numeric)) return null;
    const resolved = numeric < 0 ? length + numeric : numeric;
    return resolved >= 0 && resolved < length ? resolved : null;
}

async function loreEntryActive(
    entry: CharacterRuntimeKnowledgeDto,
    source: string,
    budget: LoreWorkBudget,
): Promise<boolean> {
    if (entry.constant) return true;
    const primary = entry.primary_keys;
    if (primary.length === 0) return false;
    const primaryMatch = await anyLoreKeyMatches(entry, primary, source, budget);
    if (!primaryMatch) return false;
    return (
        !entry.selective || (await anyLoreKeyMatches(entry, entry.secondary_keys, source, budget))
    );
}

async function anyLoreKeyMatches(
    entry: CharacterRuntimeKnowledgeDto,
    keys: readonly string[],
    source: string,
    budget: LoreWorkBudget,
): Promise<boolean> {
    for (const key of keys) {
        if (budget.keyTestsRemaining <= 0) return false;
        budget.keyTestsRemaining -= 1;
        if (await loreKeyMatches(entry, key, source, budget)) return true;
    }
    return false;
}

async function loreKeyMatches(
    entry: CharacterRuntimeKnowledgeDto,
    key: string,
    source: string,
    budget: LoreWorkBudget,
): Promise<boolean> {
    if (key === '') return false;
    if (entry.use_regex) {
        if (budget.regexTestsRemaining <= 0) return false;
        budget.regexTestsRemaining -= 1;
        const result = await runPortableRegex({
            operation: 'test',
            source,
            pattern: key,
            flags: entry.case_sensitive ? '' : 'i',
        });
        return result.ok && result.value === true;
    }
    const haystack = entry.case_sensitive ? source : source.toLocaleLowerCase();
    const needle = entry.case_sensitive ? key : key.toLocaleLowerCase();
    if (!entry.whole_word) return haystack.includes(needle);
    let offset = haystack.indexOf(needle);
    while (offset >= 0) {
        const before = codePointBefore(haystack, offset);
        const after = codePointAt(haystack, offset + needle.length);
        if (!isWordCharacter(before) && !isWordCharacter(after)) return true;
        offset = haystack.indexOf(needle, offset + Math.max(1, needle.length));
    }
    return false;
}

function isWordCharacter(value: string): boolean {
    return value !== '' && /[\p{L}\p{N}_]/u.test(value);
}

function codePointBefore(value: string, index: number): string {
    if (index <= 0) return '';
    const trailing = value.charCodeAt(index - 1);
    const start = trailing >= 0xdc00 && trailing <= 0xdfff && index >= 2 ? index - 2 : index - 1;
    return value.slice(start, index);
}

function codePointAt(value: string, index: number): string {
    const point = value.codePointAt(index);
    return point === undefined ? '' : String.fromCodePoint(point);
}

function boundedJsonValue(value: unknown): { ok: true; value: unknown } | { ok: false } {
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

function safeText(value: unknown): string {
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

function luaNullable(value: unknown): unknown {
    return value === undefined ? LuaMultiReturn.of(undefined) : value;
}

function browserStorage(): Storage | undefined {
    try {
        return typeof window === 'undefined' ? undefined : window.localStorage;
    } catch {
        return undefined;
    }
}

function serializePersisted(value: PersistedRuntimeState): string | null {
    try {
        const serialized = JSON.stringify(value);
        return new TextEncoder().encode(serialized).byteLength <= MAX_PERSISTED_RUNTIME_BYTES
            ? serialized
            : null;
    } catch {
        return null;
    }
}

function updateStringRecord(
    record: Record<string, string>,
    key: string,
    value: string,
    maxKeys: number,
    maxValueChars: number,
): Record<string, string> | null {
    if (
        !validRuntimeKey(key) ||
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

function updateUnknownRecord(
    record: Record<string, unknown>,
    key: string,
    value: unknown,
): Record<string, unknown> | null {
    if (!validRuntimeKey(key)) return null;
    if (value === undefined || value === null) {
        return Object.fromEntries(Object.entries(record).filter(([name]) => name !== key));
    }
    if (!(key in record) && Object.keys(record).length >= MAX_RUNTIME_RECORD_KEYS) return null;
    const bounded = boundedJsonValue(value);
    if (!bounded.ok) return null;
    return Object.fromEntries([
        ...Object.entries(record).filter(([name]) => name !== key),
        [key, bounded.value],
    ]);
}

function boundedStringRecord(value: unknown, maxValueChars: number): Record<string, string> | null {
    if (!isRecord(value)) return {};
    const entries = Object.entries(value);
    if (entries.length > MAX_RUNTIME_RECORD_KEYS) return null;
    if (
        entries.some(
            ([key, item]) =>
                !validRuntimeKey(key) || typeof item !== 'string' || item.length > maxValueChars,
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
        if (!validRuntimeKey(key)) return null;
        const bounded = boundedJsonValue(item);
        if (!bounded.ok) return null;
        result.push([key, bounded.value]);
    }
    return Object.fromEntries(result);
}

function validRuntimeKey(value: string): boolean {
    return (
        value.length > 0 &&
        value.length <= 512 &&
        !['__proto__', 'constructor', 'prototype'].includes(value)
    );
}

function isRecord(value: unknown): value is Record<string, unknown> {
    return typeof value === 'object' && value !== null && !Array.isArray(value);
}

function validSelection(value: unknown): value is GenerationSelectionInput {
    if (!isRecord(value)) return false;
    if (value.kind === 'legacy_profile') return typeof value.provider_profile_id === 'string';
    return (
        value.kind === 'target' &&
        isRecord(value.target) &&
        typeof value.target.model_route_id === 'string' &&
        typeof value.target.generation_preset_id === 'string'
    );
}

function cloneSelection(
    selection: GenerationSelectionInput | null,
): GenerationSelectionInput | null {
    return selection === null ? null : structuredClone(selection);
}
