import type {
    CharacterRenderProfileDto,
    CharacterRuntimeKnowledgeDto,
    GenerationUsageDto,
    GenerationSelectionInput,
    LorepiaClient,
    MessageDto,
    RuntimePromptMessageInput,
} from '../../lib/ipc/contracts';
import { normalizeClientError } from '../../lib/ipc/errors';
import { t } from '../../lib/i18n';
import {
    type PortableRuntimeCapability,
    type PortableRuntimeChatMessage,
    type PortableRuntimeGrant,
    type PortableRuntimeHostCallMessage,
    type PortableRuntimePersistedState,
    type PortableRuntimeWorkerContext,
    type PortableRuntimeWorkerOperation,
    type PortableRuntimeWorkerResult,
    type PortableRuntimeWorkerSnapshot,
    type PortableRuntimeWorkerValue,
} from './portable-runtime-protocol';
import {
    MAX_PERSISTED_RUNTIME_BYTES,
    MAX_RUNTIME_RECORD_KEYS,
    cloneSelection,
    defaultPortableRuntimeState,
    normalizePortableRuntimeState,
    safePortableText,
    serializePortableRuntimeState,
    updatePortableStringRecord,
    validSelection,
} from './portable-runtime-state';
import {
    PortableRuntimeWorkerClient,
    PortableRuntimeWorkerError,
    type PortableRuntimeWorkerFactory,
} from './portable-runtime-worker-client';
import { portableRegexRuleKey, runPortableRegex } from './portable-regex';
import {
    beginPortableRuntimeModelCall,
    portableRuntimeModelBudgetSnapshot,
    type PortableRuntimeModelCallLease,
    type PortableRuntimeModelBudgetSnapshot,
} from './portable-runtime-model-policy';

export type { PortableRuntimeCapability, PortableRuntimeGrant } from './portable-runtime-protocol';

const MAX_RUNTIME_MODEL_PROMPT_CHARS = 64 * 1024;
const MAX_RUNTIME_EVENT_MS = 30_000;
const MAX_RUNTIME_LORE_ENTRIES = 512;
const MAX_RUNTIME_LORE_KEY_TESTS = 1_024;
const MAX_RUNTIME_LORE_REGEX_TESTS = 64;
const MAX_RUNTIME_LORE_SOURCE_CHARS = 262_144;

const PORTABLE_RUNTIME_CAPABILITIES: readonly PortableRuntimeCapability[] = [
    'runtime:callbacks',
    'chat:read',
    'chat:write',
    'state:readwrite',
    'profile:read',
    'lore:read',
    'ui:write',
    'model:primary',
    'model:auxiliary',
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
    const manifest = JSON.stringify({ version: 1, profile, capabilities });
    const digest = await globalThis.crypto.subtle.digest(
        'SHA-256',
        new TextEncoder().encode(manifest),
    );
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
    onModelCallStatus?: (status: PortableRuntimeModelCallStatus | null) => void;
    storage?: Storage;
    /** Worker constructor override used by bounded tests; imported content cannot set this. */
    workerFactory?: PortableRuntimeWorkerFactory;
    /** Host policy override used by bounded tests; imported content cannot set this. */
    eventTimeoutMs?: number;
}

export interface PortableRuntimeModelCallStatus {
    requestId: string;
    target: 'primary' | 'auxiliary';
    characterName: string;
    startedAt: number;
}

export interface PreparedPortableInput {
    text: string;
    shouldSend: boolean;
}

interface LoreWorkBudget {
    keyTestsRemaining: number;
    regexTestsRemaining: number;
}

export class PortableCharacterRuntime {
    readonly toggles: PortableRuntimeToggle[];

    private readonly profile: CharacterRenderProfileDto;
    private readonly client: LorepiaClient;
    private readonly primarySelection: () => GenerationSelectionInput | null;
    private readonly onChanged: () => void;
    private readonly onNotice: (message: string, error: boolean) => void;
    private readonly onModelCallStatus: (status: PortableRuntimeModelCallStatus | null) => void;
    private readonly storage: Storage | undefined;
    private readonly storageKey: string;
    private readonly characterName: string;
    private readonly characterDescription: string;
    private readonly personaName: string;
    private readonly personaDescription: string;
    private readonly workerFactory: PortableRuntimeWorkerFactory | undefined;
    private readonly capabilities: PortableRuntimeCapability[];
    private readonly grantSha256: string;
    private readonly eventTimeoutMs: number;
    private readonly regexRuleScope: string;
    private readonly modelBudgetScope: string;
    private activeLoreEntries: CharacterRuntimeKnowledgeDto[] = [];
    private messages: MessageDto[] = [];
    private virtualMessage: PortableRuntimeChatMessage | null = null;
    private displayCache = new Map<string, string>();
    private worker: PortableRuntimeWorkerClient | null = null;
    private workerVersion = 0;
    private workerStopped = false;
    private recoveryPromise: Promise<void> | null = null;
    private operationQueue: Promise<void> = Promise.resolve();
    private closed = false;
    private changedQueued = false;
    private activeModelRequestId: string | null = null;
    private persisted: PortableRuntimePersistedState;

    private constructor(options: PortableRuntimeOptions) {
        this.profile = options.profile;
        this.capabilities = canonicalCapabilities(options.grant.capabilities);
        this.grantSha256 = options.grant.manifestSha256;
        this.client = options.client;
        this.primarySelection = options.primarySelection;
        this.onChanged = options.onChanged;
        this.onNotice = options.onNotice;
        this.onModelCallStatus = options.onModelCallStatus ?? (() => undefined);
        this.storage = options.storage ?? browserStorage();
        this.characterName = options.characterName;
        this.characterDescription = options.characterDescription;
        const personaName = options.personaName?.trim();
        this.personaName =
            personaName === undefined || personaName === ''
                ? t('chat.runtime.persona.default')
                : personaName;
        this.personaDescription = options.personaDescription ?? '';
        this.workerFactory = options.workerFactory;
        this.eventTimeoutMs = Math.min(
            MAX_RUNTIME_EVENT_MS,
            Math.max(25, options.eventTimeoutMs ?? MAX_RUNTIME_EVENT_MS),
        );
        this.regexRuleScope = `${options.profile.character_id}:${options.profile.character_content_revision_id ?? 'legacy'}`;
        this.modelBudgetScope = this.regexRuleScope;
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
            await runtime.initializeWithDeadline();
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
            ...(this.capabilities.includes('profile:read') ? this.generationVariables : {}),
            ...(this.capabilities.includes('state:readwrite')
                ? Object.fromEntries(
                      Object.entries(this.persisted.chatVars).map(([key, value]) => [
                          key,
                          safePortableText(value),
                      ]),
                  )
                : {}),
        };
    }

    get generationVariables(): Record<string, string> {
        return { ...this.profile.initial_variables, ...this.persisted.options };
    }

    get auxiliarySelection(): GenerationSelectionInput | null {
        return validSelection(this.persisted.auxiliarySelection)
            ? cloneSelection(this.persisted.auxiliarySelection)
            : null;
    }

    get modelBudget(): PortableRuntimeModelBudgetSnapshot {
        return portableRuntimeModelBudgetSnapshot(this.modelBudgetScope);
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
        await this.runWithEventDeadline(async (workerVersion) => {
            const options = updatePortableStringRecord(
                this.persisted.options,
                key,
                value,
                MAX_RUNTIME_RECORD_KEYS,
                16_384,
            );
            if (options === null || !this.commitPersisted({ ...this.persisted, options })) return;
            await this.refreshDisplayInWorker(workerVersion);
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
        return await this.runWithEventDeadline(async (workerVersion) => {
            this.workerStopped = false;
            const edited = await this.requestWorker(
                { type: 'edit-input', text, context: this.workerContext() },
                workerVersion,
            );
            if (edited.type !== 'edited-input') throw protocolResultError();
            const prepared = edited.text;
            this.virtualMessage = {
                id: '__runtime_pending_user__',
                role: 'user',
                data: prepared,
                time: Math.floor(Date.now() / 1_000),
                virtual: true,
            };
            let result: PortableRuntimeWorkerValue;
            try {
                await this.refreshActiveLore();
                const invoked = await this.requestWorker(
                    {
                        type: 'invoke',
                        name: 'onStart',
                        values: ['character-runtime'],
                        context: this.workerContext(),
                    },
                    workerVersion,
                );
                if (invoked.type !== 'invoked') throw protocolResultError();
                result = invoked.value;
            } finally {
                this.virtualMessage = null;
            }
            const startAllowed = result !== false;
            const shouldSend = this.canSendPreparedInput(startAllowed, prepared);
            await this.refreshDisplayInWorker(workerVersion);
            this.notifyChanged();
            return { text: prepared, shouldSend };
        });
    }

    async afterOutput(messages: MessageDto[]): Promise<void> {
        this.assertOpen();
        await this.runWithEventDeadline(async (workerVersion) => {
            this.setMessages(messages);
            await this.refreshActiveLore();
            const result = await this.requestWorker(
                {
                    type: 'invoke',
                    name: 'onOutput',
                    values: ['character-runtime'],
                    context: this.workerContext(),
                },
                workerVersion,
            );
            if (result.type !== 'invoked') throw protocolResultError();
            await this.refreshDisplayInWorker(workerVersion);
            this.notifyChanged();
        });
    }

    async handleAction(action: string): Promise<void> {
        if (action.length === 0 || action.length > 512) return;
        this.assertOpen();
        await this.runWithEventDeadline(async (workerVersion) => {
            await this.refreshActiveLore();
            const result = await this.requestWorker(
                {
                    type: 'invoke',
                    name: 'onButtonClick',
                    values: ['character-runtime', action],
                    context: this.workerContext(),
                },
                workerVersion,
            );
            if (result.type !== 'invoked') throw protocolResultError();
            await this.refreshDisplayInWorker(workerVersion);
            this.notifyChanged();
        });
    }

    async refreshDisplay(): Promise<void> {
        this.assertOpen();
        await this.runWithEventDeadline((workerVersion) =>
            this.refreshDisplayInWorker(workerVersion),
        );
    }

    close(): void {
        if (this.closed) return;
        this.closed = true;
        void this.cancelActiveModelCall();
        this.workerVersion += 1;
        const worker = this.worker;
        this.worker = null;
        worker?.close();
        this.displayCache.clear();
    }

    async cancelActiveModelCall(): Promise<boolean> {
        const requestId = this.activeModelRequestId;
        if (requestId === null || this.client.cancelRuntimeText === undefined) return false;
        return await this.client.cancelRuntimeText(requestId).catch(() => false);
    }

    private async initializeWithDeadline(): Promise<void> {
        let timeout: ReturnType<typeof globalThis.setTimeout> | undefined;
        const deadlineError = new PortableRuntimeWorkerError(
            'execution-timeout',
            t('chat.runtime.event_timeout'),
        );
        const deadline = new Promise<never>((_resolve, reject) => {
            timeout = globalThis.setTimeout(() => {
                this.detachWorker(deadlineError);
                reject(deadlineError);
            }, this.eventTimeoutMs);
        });
        try {
            await Promise.race([this.initializeWorker(), deadline]);
        } finally {
            if (timeout !== undefined) globalThis.clearTimeout(timeout);
        }
    }

    private async initializeWorker(): Promise<void> {
        this.assertNotClosed();
        await this.refreshActiveLore();
        this.assertNotClosed();
        const workerVersion = this.workerVersion + 1;
        this.workerVersion = workerVersion;
        const worker = new PortableRuntimeWorkerClient(this.workerFactory, {
            onHostCall: async (call) => {
                if (this.worker !== worker || this.workerVersion !== workerVersion) {
                    throw new Error(t('chat.runtime.not_ready'));
                }
                return await this.handleHostCall(call);
            },
            onState: (persisted) => {
                if (this.worker !== worker || this.workerVersion !== workerVersion) return;
                if (!this.acceptWorkerState(persisted)) {
                    this.detachWorker(
                        new PortableRuntimeWorkerError(
                            'protocol-error',
                            'portable runtime worker returned invalid state',
                        ),
                        worker,
                    );
                }
            },
            onChanged: () => {
                if (this.worker === worker && this.workerVersion === workerVersion) {
                    this.notifyChanged();
                }
            },
            onNotice: (message, error) => {
                if (this.worker === worker && this.workerVersion === workerVersion) {
                    this.onNotice(message, error);
                }
            },
        });
        this.worker = worker;
        try {
            const response = await worker.request({
                type: 'initialize',
                value: {
                    profile: this.profile,
                    capabilities: this.capabilities,
                    characterName: this.characterName,
                    characterDescription: this.characterDescription,
                    personaName: this.personaName,
                    personaDescription: this.personaDescription,
                    context: this.workerContext(),
                },
            });
            if (this.worker !== worker || this.workerVersion !== workerVersion) {
                throw new PortableRuntimeWorkerError(
                    'worker-terminated',
                    t('chat.runtime.not_ready'),
                );
            }
            this.applySnapshot(response.snapshot);
            if (response.result.type !== 'initialized') throw protocolResultError();
            await this.refreshDisplayInWorker(workerVersion);
        } catch (error) {
            this.detachWorker(
                error instanceof Error ? error : new Error(t('chat.runtime.not_ready')),
                worker,
            );
            throw error;
        }
    }

    private async refreshDisplayInWorker(workerVersion: number): Promise<void> {
        const result = await this.requestWorker(
            { type: 'refresh-display', context: this.workerContext() },
            workerVersion,
        );
        if (result.type !== 'display') throw protocolResultError();
        const displayCache = new Map<string, string>();
        const retained = new Set(this.messages.map((message) => message.id));
        for (const [id, text] of result.entries) {
            if (retained.has(id)) displayCache.set(id, text);
        }
        this.displayCache = displayCache;
    }

    private async requestWorker(
        operation: PortableRuntimeWorkerOperation,
        workerVersion: number,
    ): Promise<PortableRuntimeWorkerResult> {
        const worker = this.worker;
        if (worker === null || this.workerVersion !== workerVersion) {
            throw new PortableRuntimeWorkerError('worker-terminated', t('chat.runtime.not_ready'));
        }
        const response = await worker.request(operation);
        if (this.worker !== worker || this.workerVersion !== workerVersion) {
            throw new PortableRuntimeWorkerError('worker-terminated', t('chat.runtime.not_ready'));
        }
        this.applySnapshot(response.snapshot);
        return response.result;
    }

    private async handleHostCall(call: PortableRuntimeHostCallMessage): Promise<unknown> {
        const requiredCapability: PortableRuntimeCapability =
            call.target === 'primary' ? 'model:primary' : 'model:auxiliary';
        if (!this.capabilities.includes(requiredCapability)) {
            throw new PortableRuntimeWorkerError(
                'protocol-error',
                'portable runtime worker requested an ungranted host capability',
            );
        }
        const selection =
            call.target === 'primary' ? this.primarySelection() : this.auxiliarySelection;
        return await this.generate(call.target, selection, call.messages);
    }

    private async generate(
        target: 'primary' | 'auxiliary',
        selection: GenerationSelectionInput | null,
        rawMessages: unknown,
    ): Promise<{ success: boolean; result: string }> {
        let lease: PortableRuntimeModelCallLease | undefined;
        let usage: GenerationUsageDto | null = null;
        let callOutcome: 'completed' | 'known_failure' | 'unknown_outcome' = 'known_failure';
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
            if (promptChars > MAX_RUNTIME_MODEL_PROMPT_CHARS) {
                throw new Error(t('chat.runtime.generation.budget_exhausted'));
            }
            const promptByteLength = messages.reduce(
                (total, message) => total + new TextEncoder().encode(message.content).byteLength,
                0,
            );
            const admission = beginPortableRuntimeModelCall(
                this.modelBudgetScope,
                promptByteLength,
            );
            if (!admission.ok) {
                throw new Error(t('chat.runtime.generation.budget_exhausted'));
            }
            lease = admission.lease;
            this.activeModelRequestId = lease.requestId;
            const generation = this.client.generateRuntimeText({
                request_id: lease.requestId,
                audit: {
                    character_id: this.profile.character_id,
                    character_content_revision_id:
                        this.profile.character_content_revision_id ?? null,
                    capability: target === 'primary' ? 'model:primary' : 'model:auxiliary',
                    grant_sha256: this.grantSha256,
                },
                selection,
                messages,
            });
            this.onModelCallStatus({
                requestId: lease.requestId,
                target,
                characterName: this.characterName,
                startedAt: Date.now(),
            });
            const response = await generation;
            if (response.request_id !== lease.requestId) {
                throw new PortableRuntimeWorkerError(
                    'protocol-error',
                    'runtime model response identity did not match the request',
                );
            }
            usage = response.usage;
            callOutcome = 'completed';
            return { success: true, result: response.result };
        } catch (error) {
            if (lease !== undefined) {
                const clientError = normalizeClientError(error);
                callOutcome =
                    clientError.code === 'internal' || clientError.code === 'unexpected'
                        ? 'unknown_outcome'
                        : 'known_failure';
            }
            return { success: false, result: safePortableText(error) };
        } finally {
            if (lease !== undefined) {
                lease.finish(usage, callOutcome);
                if (this.activeModelRequestId === lease.requestId) {
                    this.activeModelRequestId = null;
                    this.onModelCallStatus(null);
                }
            }
        }
    }

    private workerContext(): PortableRuntimeWorkerContext {
        return {
            persisted: this.persisted,
            messages: this.messages.map((message) => ({
                id: message.id,
                role: runtimeRole(message.role),
                data: this.effectiveText(message),
                time: Math.floor(Date.parse(message.created_at) / 1_000) || 0,
                virtual: false,
            })),
            virtualMessage: this.virtualMessage,
            activeLoreEntries: this.activeLoreEntries,
            stopped: this.workerStopped,
        };
    }

    private canSendPreparedInput(startAllowed: boolean, text: string): boolean {
        return !this.workerStopped && startAllowed && text.trim() !== '';
    }

    private applySnapshot(snapshot: PortableRuntimeWorkerSnapshot): void {
        if (!this.acceptWorkerState(snapshot.persisted)) {
            throw new PortableRuntimeWorkerError(
                'protocol-error',
                'portable runtime worker returned invalid state',
            );
        }
        this.virtualMessage = snapshot.virtualMessage;
        this.workerStopped = snapshot.stopped;
    }

    private acceptWorkerState(value: unknown): boolean {
        const normalized = normalizePortableRuntimeState(value, this.profile.background_markup);
        return (
            normalized !== null &&
            this.commitPersisted({
                ...normalized,
                auxiliarySelection: this.auxiliarySelection,
            })
        );
    }

    private runWithEventDeadline<T>(operation: (workerVersion: number) => Promise<T>): Promise<T> {
        const run = () => this.executeWithEventDeadline(operation);
        const result = this.operationQueue.then(run, run);
        this.operationQueue = result.then(
            () => undefined,
            () => undefined,
        );
        return result;
    }

    private async executeWithEventDeadline<T>(
        operation: (workerVersion: number) => Promise<T>,
    ): Promise<T> {
        if (this.closed) throw new Error(t('chat.runtime.not_ready'));
        const worker = this.worker;
        const workerVersion = this.workerVersion;
        if (worker === null) throw new Error(t('chat.runtime.not_ready'));
        let timeout: ReturnType<typeof globalThis.setTimeout> | undefined;
        const deadlineError = new PortableRuntimeWorkerError(
            'execution-timeout',
            t('chat.runtime.event_timeout'),
        );
        const deadline = new Promise<never>((_resolve, reject) => {
            timeout = globalThis.setTimeout(() => {
                this.detachWorker(deadlineError, worker);
                reject(deadlineError);
            }, this.eventTimeoutMs);
        });
        try {
            return await Promise.race([operation(workerVersion), deadline]);
        } catch (error) {
            const workerFailure =
                error instanceof PortableRuntimeWorkerError &&
                (error.code === 'execution-timeout' ||
                    error.code === 'protocol-error' ||
                    error.code === 'worker-terminated');
            if (workerFailure) this.detachWorker(error, worker);
            if (workerFailure) {
                try {
                    await this.recoverWorker();
                } catch {
                    this.close();
                }
            }
            if (error instanceof PortableRuntimeWorkerError && error.code === 'execution-timeout') {
                throw new Error(t('chat.runtime.event_timeout'), { cause: error });
            }
            throw error;
        } finally {
            if (timeout !== undefined) globalThis.clearTimeout(timeout);
        }
    }

    private async recoverWorker(): Promise<void> {
        if (this.closed) throw new Error(t('chat.runtime.not_ready'));
        if (this.recoveryPromise !== null) return await this.recoveryPromise;
        this.recoveryPromise = this.initializeWithDeadline();
        try {
            await this.recoveryPromise;
        } finally {
            this.recoveryPromise = null;
        }
    }

    private detachWorker(error: Error, expected?: PortableRuntimeWorkerClient): void {
        const worker = this.worker;
        if (worker === null || (expected !== undefined && worker !== expected)) return;
        this.worker = null;
        this.workerVersion += 1;
        worker.close(error);
    }

    private async refreshActiveLore(): Promise<void> {
        if (!this.capabilities.includes('lore:read')) {
            this.activeLoreEntries = [];
            return;
        }
        const mayReadChat = this.capabilities.includes('chat:read');
        const sourceParts = mayReadChat
            ? this.messages.map((message) => this.effectiveText(message))
            : [];
        if (mayReadChat && this.virtualMessage !== null) sourceParts.push(this.virtualMessage.data);
        const source = sourceParts.join('\n').slice(-MAX_RUNTIME_LORE_SOURCE_CHARS);
        const candidates = this.profile.runtime_knowledge
            .filter((entry) => entry.enabled && !entry.folder && (mayReadChat || entry.constant))
            .slice(0, MAX_RUNTIME_LORE_ENTRIES);
        const budget: LoreWorkBudget = {
            keyTestsRemaining: MAX_RUNTIME_LORE_KEY_TESTS,
            regexTestsRemaining: MAX_RUNTIME_LORE_REGEX_TESTS,
        };
        const active: CharacterRuntimeKnowledgeDto[] = [];
        for (const entry of candidates) {
            if (await loreEntryActive(entry, source, budget, this.regexRuleScope))
                active.push(entry);
        }
        this.activeLoreEntries = active;
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

    private loadState(): PortableRuntimePersistedState {
        const fallback = defaultPortableRuntimeState(this.profile.background_markup);
        try {
            const serialized = this.storage?.getItem(this.storageKey);
            if (serialized === null || serialized === undefined) return fallback;
            if (new TextEncoder().encode(serialized).byteLength > MAX_PERSISTED_RUNTIME_BYTES) {
                return fallback;
            }
            return (
                normalizePortableRuntimeState(
                    JSON.parse(serialized),
                    this.profile.background_markup,
                ) ?? fallback
            );
        } catch {
            return fallback;
        }
    }

    private commitPersisted(candidate: PortableRuntimePersistedState): boolean {
        const serialized = serializePortableRuntimeState(candidate);
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

    private assertOpen(): void {
        if (this.closed || this.worker === null) throw new Error(t('chat.runtime.not_ready'));
    }

    private assertNotClosed(): void {
        if (this.closed) throw new Error(t('chat.runtime.not_ready'));
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

function runtimeRole(role: MessageDto['role']): PortableRuntimeChatMessage['role'] {
    if (role === 'assistant') return 'char';
    if (role === 'user') return 'user';
    return 'system';
}

async function loreEntryActive(
    entry: CharacterRuntimeKnowledgeDto,
    source: string,
    budget: LoreWorkBudget,
    ruleScope: string,
): Promise<boolean> {
    if (entry.constant) return true;
    if (entry.primary_keys.length === 0) return false;
    const primaryMatch = await anyLoreKeyMatches(
        entry,
        entry.primary_keys,
        'primary',
        source,
        budget,
        ruleScope,
    );
    if (!primaryMatch) return false;
    return (
        !entry.selective ||
        (await anyLoreKeyMatches(
            entry,
            entry.secondary_keys,
            'secondary',
            source,
            budget,
            ruleScope,
        ))
    );
}

async function anyLoreKeyMatches(
    entry: CharacterRuntimeKnowledgeDto,
    keys: readonly string[],
    keyKind: 'primary' | 'secondary',
    source: string,
    budget: LoreWorkBudget,
    ruleScope: string,
): Promise<boolean> {
    for (const [keyIndex, key] of keys.entries()) {
        if (budget.keyTestsRemaining <= 0) return false;
        budget.keyTestsRemaining -= 1;
        if (await loreKeyMatches(entry, key, keyKind, keyIndex, source, budget, ruleScope))
            return true;
    }
    return false;
}

async function loreKeyMatches(
    entry: CharacterRuntimeKnowledgeDto,
    key: string,
    keyKind: 'primary' | 'secondary',
    keyIndex: number,
    source: string,
    budget: LoreWorkBudget,
    ruleScope: string,
): Promise<boolean> {
    if (key === '') return false;
    if (entry.use_regex) {
        if (budget.regexTestsRemaining <= 0) return false;
        budget.regexTestsRemaining -= 1;
        const result = await runPortableRegex(
            {
                operation: 'test',
                source,
                pattern: key,
                flags: entry.case_sensitive ? '' : 'i',
            },
            {
                ruleKey: portableRegexRuleKey(
                    ruleScope,
                    'lore',
                    `${entry.id}:${keyKind}:${String(keyIndex)}`,
                    keyIndex,
                ),
            },
        );
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

function browserStorage(): Storage | undefined {
    try {
        return typeof window === 'undefined' ? undefined : window.localStorage;
    } catch {
        return undefined;
    }
}

function protocolResultError(): PortableRuntimeWorkerError {
    return new PortableRuntimeWorkerError(
        'protocol-error',
        'portable runtime worker returned an unexpected result',
    );
}

function isRecord(value: unknown): value is Record<string, unknown> {
    return typeof value === 'object' && value !== null && !Array.isArray(value);
}
