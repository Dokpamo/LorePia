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
import { renderPortableDisplay } from './portable-display';

const TRIGGER_ID = 'character-runtime';
const MAX_PERSISTED_RUNTIME_BYTES = 4 * 1024 * 1024;
const MAX_RUNTIME_NOTICE_CHARS = 4_096;

const LUA_ASYNC_BRIDGE = String.raw`
function async(callback)
    return function(...)
        local co = coroutine.create(callback)
        local safe, result = coroutine.resume(co, ...)

        return Promise.create(function(resolve, reject)
            local function step()
                if coroutine.status(co) == "dead" then
                    local send = safe and resolve or reject
                    return send(result)
                end

                safe, result = coroutine.resume(co)
                if safe and result == Promise.resolve(result) then
                    result:finally(step)
                else
                    step()
                end
            end

            if safe and result == Promise.resolve(result) then
                result:finally(step)
            else
                step()
            end
        end)
    end
end

function LLM(triggerId, messages, useTools, options)
    return __hostMainGeneration(messages):await()
end

function axLLM(triggerId, messages, useTools, options)
    return __hostAuxGeneration(messages):await()
end
`;

export interface PortableRuntimeToggle {
    key: string;
    label: string;
    kind: 'select' | 'toggle' | 'text';
    choices: string[];
}

export interface PortableRuntimeOptions {
    profile: CharacterRenderProfileDto;
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
    private readonly editCallbacks = new Map<string, EditCallback[]>();
    private readonly displayCache = new Map<string, string>();

    private engine: LuaEngine | null = null;
    private messages: MessageDto[] = [];
    private virtualMessage: RuntimeChatMessage | null = null;
    private stopped = false;
    private closed = false;
    private changedQueued = false;
    private persisted: PersistedRuntimeState;

    private constructor(options: PortableRuntimeOptions) {
        this.profile = options.profile;
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
        const runtime = new PortableCharacterRuntime(options);
        await runtime.initialize();
        return runtime;
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
        let changed = false;
        for (const id of Object.keys(this.persisted.messageOverrides)) {
            if (!retained.has(id)) {
                Reflect.deleteProperty(this.persisted.messageOverrides, id);
                changed = true;
            }
        }
        if (changed) this.persist();
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
        this.persisted.options[key] = value.slice(0, 16_384);
        this.persist();
        await this.refreshDisplay();
        this.notifyChanged();
    }

    setAuxiliarySelection(selection: GenerationSelectionInput | null): void {
        this.persisted.auxiliarySelection = cloneSelection(selection);
        this.persist();
        this.notifyChanged();
    }

    async prepareInput(text: string): Promise<PreparedPortableInput> {
        this.assertOpen();
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
            result = await this.invokeGlobal('onStart', TRIGGER_ID);
        } finally {
            this.virtualMessage = null;
        }
        const startAllowed = typeof result !== 'boolean' || result;
        const shouldSend = !this.chatStopped() && startAllowed && prepared.trim() !== '';
        await this.refreshDisplay();
        this.notifyChanged();
        return { text: prepared, shouldSend };
    }

    async afterOutput(messages: MessageDto[]): Promise<void> {
        this.setMessages(messages);
        await this.invokeGlobal('onOutput', TRIGGER_ID);
        await this.refreshDisplay();
        this.notifyChanged();
    }

    async handleAction(action: string): Promise<void> {
        if (action.length === 0 || action.length > 512) return;
        await this.invokeGlobal('onButtonClick', TRIGGER_ID, action);
        await this.refreshDisplay();
        this.notifyChanged();
    }

    async refreshDisplay(): Promise<void> {
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
    }

    close(): void {
        if (this.closed) return;
        this.closed = true;
        this.engine?.global.close();
        this.engine = null;
        this.editCallbacks.clear();
        this.displayCache.clear();
    }

    private async initialize(): Promise<void> {
        const engine = await this.luaFactory.createEngine({
            injectObjects: true,
            enableProxy: false,
            functionTimeout: 10_000,
        });
        this.engine = engine;
        this.installHostFunctions(engine);
        await engine.doString(LUA_ASYNC_BRIDGE);
        for (const script of this.profile.runtime_scripts) {
            if (script.language.trim().toLowerCase() !== 'lua' || script.source.trim() === '') {
                continue;
            }
            await engine.doString(script.source);
        }
        await this.refreshDisplay();
    }

    private installHostFunctions(engine: LuaEngine): void {
        const set = (name: string, value: unknown): void => engine.global.set(name, value);
        set('listenEdit', (kind: unknown, callback: unknown) => {
            if (typeof kind !== 'string' || typeof callback !== 'function') return;
            const callbacks = this.editCallbacks.get(kind) ?? [];
            callbacks.push(callback as EditCallback);
            this.editCallbacks.set(kind, callbacks);
        });
        set('getChatLength', () => this.runtimeMessages().length);
        set('getChat', (_triggerId: unknown, index: unknown) =>
            luaNullable(this.runtimeChatAt(index)),
        );
        set('getFullChat', () => this.runtimeMessages().map(runtimeMessageValue));
        set('setChat', (_triggerId: unknown, index: unknown, content: unknown) => {
            const message = this.runtimeMessageAt(index);
            if (message === undefined || typeof content !== 'string') return false;
            if (message.virtual) {
                if (this.virtualMessage !== null) this.virtualMessage.data = content;
            } else {
                this.persisted.messageOverrides[message.id] = content;
                this.persist();
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
        set('getChatVar', (_triggerId: unknown, key: unknown) =>
            luaNullable(typeof key === 'string' ? this.persisted.chatVars[key] : undefined),
        );
        set('setChatVar', (_triggerId: unknown, key: unknown, value: unknown) => {
            if (typeof key !== 'string' || key.length === 0 || key.length > 512) return false;
            if (value === undefined || value === null) {
                Reflect.deleteProperty(this.persisted.chatVars, key);
            } else this.persisted.chatVars[key] = jsonValue(value);
            this.persist();
            this.notifyChanged();
            return true;
        });
        set('getState', (_triggerId: unknown, key: unknown) =>
            luaNullable(typeof key === 'string' ? this.persisted.state[key] : undefined),
        );
        set('setState', (_triggerId: unknown, key: unknown, value: unknown) => {
            if (typeof key !== 'string' || key.length === 0 || key.length > 512) return false;
            if (value === undefined || value === null) {
                Reflect.deleteProperty(this.persisted.state, key);
            } else this.persisted.state[key] = jsonValue(value);
            this.persist();
            return true;
        });
        set('getGlobalVar', (_triggerId: unknown, key: unknown) =>
            luaNullable(typeof key === 'string' ? this.lookupVariable(key) : undefined),
        );
        set('getBackgroundEmbedding', () => this.persisted.background);
        set('setBackgroundEmbedding', (_triggerId: unknown, value: unknown) => {
            if (typeof value !== 'string') return false;
            this.persisted.background = value.slice(0, 1024 * 1024);
            this.persist();
            this.notifyChanged();
            return true;
        });
        set('getPersonaName', () => this.personaName);
        set('getPersonaDescription', () => this.personaDescription);
        set('getDescription', () => this.characterDescription);
        set('getLoreBooks', (_triggerId: unknown, name: unknown) => this.loreBooks(name));
        set('loadLoreBooks', () => this.activeLoreBooks());
        set('cbs', (first: unknown, second?: unknown) =>
            this.expandMacros(typeof second === 'string' ? second : safeText(first)),
        );
        set('sleep', (first: unknown, second?: unknown) => {
            const milliseconds = Number(second ?? first);
            return new Promise<void>((resolve) => {
                globalThis.setTimeout(
                    resolve,
                    Number.isFinite(milliseconds) ? Math.max(0, milliseconds) : 0,
                );
            });
        });
        set('alertNormal', (_triggerId: unknown, message: unknown) => {
            this.emitNotice(message, false);
        });
        set('alertError', (_triggerId: unknown, message: unknown) => {
            this.emitNotice(message, true);
        });
        set('log', (...values: unknown[]) => {
            console.info('[character-runtime]', ...values);
        });
        set('__hostMainGeneration', (messages: unknown) =>
            this.generate(this.primarySelection(), messages),
        );
        set('__hostAuxGeneration', (messages: unknown) =>
            this.generate(this.persisted.auxiliarySelection ?? this.primarySelection(), messages),
        );
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
            .map((entry) => this.loreBookValue(entry));
    }

    private activeLoreBooks(): { content: string; data: string; name: string }[] {
        const haystack = this.runtimeMessages()
            .map((message) => message.data)
            .join('\n');
        return this.profile.runtime_knowledge
            .filter((entry) => entry.enabled && loreEntryActive(entry, haystack))
            .map((entry) => this.loreBookValue(entry));
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
        return renderPortableDisplay(expanded, [], {
            variables: this.variables,
            chatIndex: Math.max(0, messages.length - 1),
            lastMessageId: Math.max(0, messages.length - 1),
            lastCharacterMessage: lastCharacter ?? '',
            characterName: this.characterName,
            userName: this.personaName,
        });
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
            const parsed = JSON.parse(serialized) as Partial<PersistedRuntimeState>;
            return {
                options: recordOfStrings(parsed.options),
                chatVars: recordOfUnknown(parsed.chatVars),
                state: recordOfUnknown(parsed.state),
                messageOverrides: recordOfStrings(parsed.messageOverrides),
                background:
                    typeof parsed.background === 'string'
                        ? parsed.background
                        : this.profile.background_markup,
                auxiliarySelection: validSelection(parsed.auxiliarySelection)
                    ? cloneSelection(parsed.auxiliarySelection)
                    : null,
            };
        } catch {
            return fallback;
        }
    }

    private persist(): void {
        try {
            const serialized = JSON.stringify(this.persisted);
            if (serialized.length <= MAX_PERSISTED_RUNTIME_BYTES) {
                this.storage?.setItem(this.storageKey, serialized);
            }
        } catch {
            // Runtime state remains usable for the current session if storage
            // is unavailable or the host quota is exhausted.
        }
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
                    : [],
        });
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

function loreEntryActive(entry: CharacterRuntimeKnowledgeDto, source: string): boolean {
    if (entry.constant) return true;
    const primary = entry.primary_keys;
    if (primary.length === 0) return false;
    const primaryMatch = primary.some((key) => loreKeyMatches(entry, key, source));
    if (!primaryMatch) return false;
    return (
        !entry.selective || entry.secondary_keys.some((key) => loreKeyMatches(entry, key, source))
    );
}

function loreKeyMatches(entry: CharacterRuntimeKnowledgeDto, key: string, source: string): boolean {
    if (key === '') return false;
    if (entry.use_regex) {
        try {
            return new RegExp(key, entry.case_sensitive ? '' : 'i').test(source);
        } catch {
            return false;
        }
    }
    const haystack = entry.case_sensitive ? source : source.toLocaleLowerCase();
    const needle = entry.case_sensitive ? key : key.toLocaleLowerCase();
    if (!entry.whole_word) return haystack.includes(needle);
    const escaped = needle.replace(/[.*+?^${}()|[\]\\]/g, '\\$&');
    return new RegExp(`(^|[^\\p{L}\\p{N}_])${escaped}([^\\p{L}\\p{N}_]|$)`, 'u').test(haystack);
}

function jsonValue(value: unknown): unknown {
    try {
        return JSON.parse(JSON.stringify(value)) as unknown;
    } catch {
        return safeText(value);
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

function recordOfStrings(value: unknown): Record<string, string> {
    if (!isRecord(value)) return {};
    return Object.fromEntries(
        Object.entries(value).filter(
            (entry): entry is [string, string] => typeof entry[1] === 'string',
        ),
    );
}

function recordOfUnknown(value: unknown): Record<string, unknown> {
    return isRecord(value) ? value : {};
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
