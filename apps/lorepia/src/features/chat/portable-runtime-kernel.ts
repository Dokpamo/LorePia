import { LuaFactory, LuaMultiReturn, type LuaEngine } from 'wasmoon';
import wasmUrl from 'wasmoon/dist/glue.wasm?url';

import type {
    CharacterRenderProfileDto,
    CharacterRuntimeKnowledgeDto,
} from '../../lib/ipc/contracts';
import { renderPortableMacros } from './portable-display';
import {
    clonePortableRuntimeMessageValue,
    type PortableRuntimeCapability,
    type PortableRuntimeChatMessage,
    type PortableRuntimeHostResultMessage,
    type PortableRuntimeMainMessage,
    type PortableRuntimePersistedState,
    type PortableRuntimeRequestMessage,
    type PortableRuntimeWorkerContext,
    type PortableRuntimeWorkerInitialize,
    type PortableRuntimeWorkerMessage,
    type PortableRuntimeWorkerOperation,
    type PortableRuntimeWorkerResult,
    type PortableRuntimeWorkerSnapshot,
    type PortableRuntimeWorkerValue,
} from './portable-runtime-protocol';
import {
    MAX_RUNTIME_BACKGROUND_CHARS,
    MAX_RUNTIME_MESSAGE_OVERRIDE_CHARS,
    MAX_RUNTIME_NOTICE_CHARS,
    MAX_RUNTIME_RECORD_KEYS,
    normalizePortableRuntimeState,
    safePortableText,
    serializePortableRuntimeState,
    updatePortableStringRecord,
    updatePortableUnknownRecord,
} from './portable-runtime-state';
import LUA_SANDBOX_HARDENING from './portable-runtime-sandbox.lua?raw';

const MAX_RUNTIME_LUA_SLICE_MS = 100;
const MAX_RUNTIME_LUA_MEMORY_BYTES = 32 * 1024 * 1024;
const MAX_RUNTIME_LORE_ENTRIES = 512;
const MAX_RUNTIME_NOTICES_PER_REQUEST = 16;
const MAX_RUNTIME_HOST_CALLS_PER_REQUEST = 8;
const MAX_RUNTIME_HOST_MESSAGE_BYTES = 512 * 1024;
const TRIGGER_ID = 'character-runtime';
// Capture the intrinsic before imported Lua runs so prototype substitution cannot replace it.
// eslint-disable-next-line @typescript-eslint/unbound-method
const NATIVE_PROMISE_THEN = Promise.prototype.then;
const IGNORE_PROMISE_SETTLEMENT = () => undefined;

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

const LUA_PRIMARY_MODEL_BRIDGE = String.raw`
local runtime_host_main_generation = __hostMainGeneration

function LLM(triggerId, messages, useTools, options)
    return runtime_host_main_generation(messages):await()
end
`;

const LUA_AUXILIARY_MODEL_BRIDGE = String.raw`
local runtime_host_aux_generation = __hostAuxGeneration

function axLLM(triggerId, messages, useTools, options)
    return runtime_host_aux_generation(messages):await()
end
`;

type EditCallback = (...values: unknown[]) => unknown;

export interface PortableRuntimeKernelOptions {
    postMessage: (message: PortableRuntimeWorkerMessage) => void;
    luaFactory?: LuaFactory;
}

export class PortableRuntimeKernel {
    private readonly postMessage: (message: PortableRuntimeWorkerMessage) => void;
    private readonly luaFactory: LuaFactory;
    private readonly pendingHostCalls = new Map<
        string,
        { resolve: (value: unknown) => void; reject: (error: Error) => void }
    >();
    private readonly editCallbacks = new Map<string, EditCallback[]>();
    private readonly sleepTimeouts = new Set<ReturnType<typeof globalThis.setTimeout>>();
    private requestQueue: Promise<void> = Promise.resolve();
    private nextHostCallId = 1;
    private stateEventSent = false;
    private stateRevision = 0;
    private postedStateRevision = 0;
    private changedEventSent = false;
    private noticesSent = 0;
    private hostCallsStarted = 0;
    private profile: CharacterRenderProfileDto | null = null;
    private capabilities: ReadonlySet<PortableRuntimeCapability> = new Set();
    private characterName = '';
    private characterDescription = '';
    private personaName = '';
    private personaDescription = '';
    private persisted: PortableRuntimePersistedState | null = null;
    private messages: PortableRuntimeChatMessage[] = [];
    private virtualMessage: PortableRuntimeChatMessage | null = null;
    private activeLoreEntries: CharacterRuntimeKnowledgeDto[] = [];
    private stopped = false;
    private engine: LuaEngine | null = null;
    private closed = false;

    constructor(options: PortableRuntimeKernelOptions) {
        this.postMessage = options.postMessage;
        this.luaFactory = options.luaFactory ?? new LuaFactory(wasmUrl);
    }

    receive(message: PortableRuntimeMainMessage): void {
        if (this.closed) return;
        if (message.type === 'host-result') {
            this.resolveHostCall(message);
            return;
        }
        this.requestQueue = this.requestQueue.then(
            () => this.handleRequest(message),
            () => this.handleRequest(message),
        );
    }

    close(): void {
        if (this.closed) return;
        this.closed = true;
        for (const timeout of this.sleepTimeouts) globalThis.clearTimeout(timeout);
        this.sleepTimeouts.clear();
        for (const pending of this.pendingHostCalls.values()) {
            pending.reject(new Error('portable runtime worker was closed'));
        }
        this.pendingHostCalls.clear();
        const engine = this.engine;
        this.engine = null;
        try {
            engine?.global.close();
        } catch {
            // The worker is already isolated and is about to terminate.
        }
        this.editCallbacks.clear();
    }

    private async handleRequest(message: PortableRuntimeRequestMessage): Promise<void> {
        this.stateEventSent = false;
        this.stateRevision = 0;
        this.postedStateRevision = 0;
        this.changedEventSent = false;
        this.noticesSent = 0;
        this.hostCallsStarted = 0;
        try {
            const result = await this.execute(message.operation);
            this.postMessage({
                channel: 'lorepia-portable-runtime-v1',
                type: 'response',
                requestId: message.requestId,
                ok: true,
                result,
                snapshot: this.snapshot(),
            });
        } catch (error) {
            const text = safePortableText(error) || 'portable runtime worker request failed';
            this.postLatestState();
            this.postMessage({
                channel: 'lorepia-portable-runtime-v1',
                type: 'response',
                requestId: message.requestId,
                ok: false,
                error: {
                    code: /timeout/i.test(text)
                        ? 'execution-timeout'
                        : /message.*(?:limit|size)|exceeds.*message/i.test(text)
                          ? 'protocol-error'
                          : 'runtime-error',
                    message: text,
                },
            });
        }
    }

    private async execute(
        operation: PortableRuntimeWorkerOperation,
    ): Promise<PortableRuntimeWorkerResult> {
        if (operation.type === 'initialize') {
            await this.initialize(operation.value);
            return { type: 'initialized' };
        }
        this.assertReady();
        this.applyContext(operation.context);
        if (operation.type === 'edit-input') {
            let edited: unknown = operation.text;
            for (const callback of this.editCallbacks.get('editInput') ?? []) {
                edited = await Promise.resolve(
                    callback(TRIGGER_ID, typeof edited === 'string' ? edited : operation.text),
                );
            }
            return {
                type: 'edited-input',
                text: typeof edited === 'string' ? edited : operation.text,
            };
        }
        if (operation.type === 'invoke') {
            return {
                type: 'invoked',
                value: safeWorkerResult(
                    await this.invokeGlobal(operation.name, ...operation.values),
                ),
            };
        }
        return { type: 'display', entries: await this.renderDisplayEntries() };
    }

    private async initialize(value: PortableRuntimeWorkerInitialize): Promise<void> {
        if (this.engine !== null || this.profile !== null) {
            throw new Error('portable runtime worker was initialized twice');
        }
        this.profile = structuredClone(value.profile);
        this.capabilities = new Set(value.capabilities);
        this.characterName = value.characterName;
        this.characterDescription = value.characterDescription;
        this.personaName = value.personaName;
        this.personaDescription = value.personaDescription;
        this.applyContext(value.context);
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
        if (this.capabilities.has('model:primary'))
            await executeLuaSource(engine, LUA_PRIMARY_MODEL_BRIDGE);
        if (this.capabilities.has('model:auxiliary'))
            await executeLuaSource(engine, LUA_AUXILIARY_MODEL_BRIDGE);
        await executeLuaSource(engine, LUA_SANDBOX_HARDENING);
        if (this.capabilities.has('runtime:callbacks')) {
            for (const script of this.profile.runtime_scripts) {
                if (script.elevated_access && !this.capabilities.has('elevated')) continue;
                if (script.language.trim().toLowerCase() !== 'lua' || script.source.trim() === '') {
                    continue;
                }
                await executeLuaSource(engine, script.source);
            }
        }
    }

    private applyContext(context: PortableRuntimeWorkerContext): void {
        const profile = this.profile;
        if (profile === null) throw new Error('portable runtime worker profile is missing');
        const persisted = normalizePortableRuntimeState(
            context.persisted,
            profile.background_markup,
        );
        if (persisted === null) throw new Error('portable runtime state is invalid');
        this.persisted = persisted;
        this.messages = context.messages.map((message) => ({ ...message }));
        this.virtualMessage =
            context.virtualMessage === null ? null : { ...context.virtualMessage };
        this.activeLoreEntries = context.activeLoreEntries.map((entry) => structuredClone(entry));
        this.stopped = context.stopped;
    }

    private installHostFunctions(engine: LuaEngine): void {
        const set = (name: string, value: unknown): void => engine.global.set(name, value);
        set('__hostIsRuntimePromise', isNativePromise);
        set('__hostRuntimeYield', () => this.sleep(0));
        if (this.capabilities.has('runtime:callbacks')) {
            set('listenEdit', (kind: unknown, callback: unknown) => {
                if (typeof kind !== 'string' || typeof callback !== 'function') return;
                if (!['editInput', 'editDisplay'].includes(kind)) return;
                if (
                    kind === 'editInput' &&
                    (!this.capabilities.has('chat:read') || !this.capabilities.has('chat:write'))
                ) {
                    return;
                }
                if (
                    kind === 'editDisplay' &&
                    (!this.capabilities.has('chat:read') || !this.capabilities.has('ui:write'))
                ) {
                    return;
                }
                const callbacks = this.editCallbacks.get(kind) ?? [];
                if (callbacks.length >= 64) return;
                callbacks.push(callback as EditCallback);
                this.editCallbacks.set(kind, callbacks);
            });
            set('cbs', (first: unknown, second?: unknown) =>
                this.expandMacros(typeof second === 'string' ? second : safePortableText(first)),
            );
            set('sleep', (first: unknown, second?: unknown) => this.sleep(Number(second ?? first)));
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
                    const persisted = this.requirePersisted();
                    const messageOverrides = updatePortableStringRecord(
                        persisted.messageOverrides,
                        message.id,
                        content,
                        MAX_RUNTIME_RECORD_KEYS,
                        MAX_RUNTIME_MESSAGE_OVERRIDE_CHARS,
                    );
                    if (
                        messageOverrides === null ||
                        !this.commitPersisted({ ...persisted, messageOverrides })
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
                luaNullable(
                    typeof key === 'string' ? this.requirePersisted().chatVars[key] : undefined,
                ),
            );
            set('setChatVar', (_triggerId: unknown, key: unknown, value: unknown) => {
                if (typeof key !== 'string' || key.length === 0 || key.length > 512) return false;
                const persisted = this.requirePersisted();
                const chatVars = updatePortableUnknownRecord(persisted.chatVars, key, value);
                if (chatVars === null || !this.commitPersisted({ ...persisted, chatVars })) {
                    return false;
                }
                this.notifyChanged();
                return true;
            });
            set('getState', (_triggerId: unknown, key: unknown) =>
                luaNullable(
                    typeof key === 'string' ? this.requirePersisted().state[key] : undefined,
                ),
            );
            set('setState', (_triggerId: unknown, key: unknown, value: unknown) => {
                if (typeof key !== 'string' || key.length === 0 || key.length > 512) return false;
                const persisted = this.requirePersisted();
                const state = updatePortableUnknownRecord(persisted.state, key, value);
                return state !== null && this.commitPersisted({ ...persisted, state });
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
            set('getBackgroundEmbedding', () => this.requirePersisted().background);
            set('setBackgroundEmbedding', (_triggerId: unknown, value: unknown) => {
                if (typeof value !== 'string' || value.length > MAX_RUNTIME_BACKGROUND_CHARS) {
                    return false;
                }
                const persisted = this.requirePersisted();
                if (!this.commitPersisted({ ...persisted, background: value })) return false;
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
        if (this.capabilities.has('model:primary')) {
            set('__hostMainGeneration', (messages: unknown) => this.callHost('primary', messages));
        }
        if (this.capabilities.has('model:auxiliary')) {
            set('__hostAuxGeneration', (messages: unknown) => this.callHost('auxiliary', messages));
        }
        if (this.capabilities.has('elevated')) set('log', () => undefined);
    }

    private async renderDisplayEntries(): Promise<[string, string][]> {
        const callbacks = this.editCallbacks.get('editDisplay') ?? [];
        if (callbacks.length === 0) return [];
        const entries: [string, string][] = [];
        const messages = this.runtimeMessages();
        for (let index = 0; index < messages.length; index += 1) {
            const runtimeMessage = messages[index];
            if (runtimeMessage === undefined || runtimeMessage.virtual) continue;
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
            entries.push([
                runtimeMessage.id,
                typeof display === 'string' ? display : runtimeMessage.data,
            ]);
        }
        return entries;
    }

    private async invokeGlobal(name: string, ...values: unknown[]): Promise<unknown> {
        const engine = this.engine;
        if (engine === null) return undefined;
        const callback = engine.global.get(name) as unknown;
        if (typeof callback !== 'function') return undefined;
        return await Promise.resolve((callback as EditCallback)(...values));
    }

    private callHost(target: 'primary' | 'auxiliary', messages: unknown): Promise<unknown> {
        if (this.hostCallsStarted >= MAX_RUNTIME_HOST_CALLS_PER_REQUEST) {
            return Promise.reject(new Error('portable runtime model call budget exhausted'));
        }
        const clonedMessages = clonePortableRuntimeMessageValue(
            messages,
            MAX_RUNTIME_HOST_MESSAGE_BYTES,
        );
        if (!clonedMessages.ok) {
            return Promise.reject(new Error('portable runtime model prompt exceeds its limit'));
        }
        const callId = `host-${String(this.nextHostCallId)}`;
        this.nextHostCallId += 1;
        this.hostCallsStarted += 1;
        return new Promise((resolve, reject) => {
            this.pendingHostCalls.set(callId, { resolve, reject });
            this.postMessage({
                channel: 'lorepia-portable-runtime-v1',
                type: 'host-call',
                callId,
                target,
                messages: clonedMessages.value,
            });
        });
    }

    private resolveHostCall(message: PortableRuntimeHostResultMessage): void {
        const pending = this.pendingHostCalls.get(message.callId);
        if (pending === undefined) return;
        this.pendingHostCalls.delete(message.callId);
        if (message.ok) {
            pending.resolve(message.value);
        } else {
            pending.reject(new Error(message.error ?? 'portable runtime host call failed'));
        }
    }

    private commitPersisted(candidate: PortableRuntimePersistedState): boolean {
        if (serializePortableRuntimeState(candidate) === null) return false;
        this.persisted = candidate;
        this.stateRevision += 1;
        if (this.stateEventSent) return true;
        this.stateEventSent = true;
        this.postMessage({
            channel: 'lorepia-portable-runtime-v1',
            type: 'state',
            persisted: candidate,
        });
        this.postedStateRevision = this.stateRevision;
        return true;
    }

    private postLatestState(): void {
        if (this.persisted === null || this.stateRevision === this.postedStateRevision) return;
        this.postMessage({
            channel: 'lorepia-portable-runtime-v1',
            type: 'state',
            persisted: this.persisted,
        });
        this.postedStateRevision = this.stateRevision;
    }

    private notifyChanged(): void {
        if (this.changedEventSent) return;
        this.changedEventSent = true;
        this.postMessage({ channel: 'lorepia-portable-runtime-v1', type: 'changed' });
    }

    private emitNotice(value: unknown, error: boolean): void {
        const message = safePortableText(value).slice(0, MAX_RUNTIME_NOTICE_CHARS);
        if (message === '' || this.noticesSent >= MAX_RUNTIME_NOTICES_PER_REQUEST) return;
        this.noticesSent += 1;
        this.postMessage({
            channel: 'lorepia-portable-runtime-v1',
            type: 'notice',
            message,
            error,
        });
    }

    private sleep(milliseconds: number): Promise<void> {
        return new Promise((resolve) => {
            const timeout = globalThis.setTimeout(
                () => {
                    this.sleepTimeouts.delete(timeout);
                    if (!this.closed) resolve();
                },
                Number.isFinite(milliseconds) ? Math.min(5_000, Math.max(0, milliseconds)) : 0,
            );
            this.sleepTimeouts.add(timeout);
        });
    }

    private runtimeMessages(): PortableRuntimeChatMessage[] {
        return this.virtualMessage === null
            ? this.messages
            : [...this.messages, this.virtualMessage];
    }

    private runtimeMessageAt(index: unknown): PortableRuntimeChatMessage | undefined {
        const messages = this.runtimeMessages();
        const resolved = resolveRuntimeIndex(index, messages.length);
        return resolved === null ? undefined : messages[resolved];
    }

    private runtimeChatAt(index: unknown): ReturnType<typeof runtimeMessageValue> | undefined {
        const message = this.runtimeMessageAt(index);
        return message === undefined ? undefined : runtimeMessageValue(message);
    }

    private lookupVariable(requested: string): string | undefined {
        const profile = this.requireProfile();
        const persisted = this.requirePersisted();
        const variables = { ...profile.initial_variables, ...persisted.options };
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
        return this.requireProfile()
            .runtime_knowledge.filter((entry) => entry.enabled && entry.name === name)
            .slice(0, MAX_RUNTIME_LORE_ENTRIES)
            .map((entry) => this.loreBookValue(entry));
    }

    private activeLoreBooks(): { content: string; data: string; name: string }[] {
        return this.activeLoreEntries.map((entry) => this.loreBookValue(entry));
    }

    private expandMacros(source: string): string {
        const persisted = this.requirePersisted();
        const messages = this.runtimeMessages();
        const mayReadChat = this.capabilities.has('chat:read');
        const mayReadProfile = this.capabilities.has('profile:read');
        const mayReadState = this.capabilities.has('state:readwrite');
        const lastCharacter = mayReadChat
            ? [...messages].reverse().find((message) => message.role === 'char')?.data
            : undefined;
        const globalVariables = mayReadProfile
            ? { ...this.requireProfile().initial_variables, ...persisted.options }
            : {};
        const localVariables = mayReadState
            ? Object.fromEntries(
                  Object.entries(persisted.chatVars).map(([key, value]) => [
                      key,
                      safePortableText(value),
                  ]),
              )
            : {};
        const expanded = source.replaceAll(
            '{{description}}',
            mayReadProfile ? this.characterDescription : '',
        );
        return renderPortableMacros(
            expanded,
            {
                variables: {},
                globalVariables,
                localVariables,
                chatIndex: mayReadChat ? Math.max(0, messages.length - 1) : undefined,
                lastMessageId: mayReadChat ? Math.max(0, messages.length - 1) : undefined,
                lastCharacterMessage: lastCharacter ?? '',
                characterName: mayReadProfile ? this.characterName : '',
                userName: mayReadProfile ? this.personaName : '',
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

    private snapshot(): PortableRuntimeWorkerSnapshot {
        return {
            persisted: this.requirePersisted(),
            virtualMessage: this.virtualMessage,
            stopped: this.stopped,
        };
    }

    private requireProfile(): CharacterRenderProfileDto {
        if (this.profile === null) throw new Error('portable runtime worker profile is missing');
        return this.profile;
    }

    private requirePersisted(): PortableRuntimePersistedState {
        if (this.persisted === null) throw new Error('portable runtime worker state is missing');
        return this.persisted;
    }

    private assertReady(): void {
        if (this.closed || this.engine === null) {
            throw new Error('portable runtime worker is not ready');
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

function luaNullable(value: unknown): unknown {
    return value === undefined ? LuaMultiReturn.of(undefined) : value;
}

function runtimeMessageValue(message: PortableRuntimeChatMessage): {
    role: PortableRuntimeChatMessage['role'];
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

function safeWorkerResult(value: unknown): PortableRuntimeWorkerValue {
    return value === undefined || value === null
        ? null
        : typeof value === 'string' || typeof value === 'number' || typeof value === 'boolean'
          ? value
          : null;
}
