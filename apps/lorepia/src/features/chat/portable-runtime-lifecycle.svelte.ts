import type {
    CharacterRenderProfileDto,
    GenerationSelectionInput,
    MessageDto,
    OrchestrationVariableMapDto,
    ProviderWorkspaceDto,
} from '../../lib/ipc/contracts';
import { t } from '../../lib/i18n';
import type { PersonaClientApi } from '../personas/persona-contracts';
import type { InteractionRoomCapableClient } from './interaction-room-controller';
import type { PortableRuntimeModelBudgetSnapshot } from './portable-runtime-model-policy';
import {
    PortableCharacterRuntime,
    createPortableRuntimeGrant,
    defaultPortableRuntimeCapabilities,
    requiredPortableRuntimeCapabilities,
    type PortableRuntimeCapability,
    type PortableRuntimeGrant,
    type PortableRuntimeModelCancellationResult,
    type PortableRuntimeModelCallStatus,
    type PortableRuntimePersistenceStatus,
} from './portable-runtime';

export type PortableRuntimePhase = 'idle' | 'blocked' | 'loading' | 'ready' | 'busy' | 'error';

interface PortableRuntimeLifecycleOptions {
    currentMessages: () => MessageDto[];
    displayMessages: () => MessageDto[];
    providerWorkspace: () => ProviderWorkspaceDto;
    primarySelection: () => GenerationSelectionInput | null;
    onNotice: (message: string) => void;
}

interface PortableRuntimeCreationContext {
    client: InteractionRoomCapableClient | undefined;
    conversationId: string | null;
    branchId: string | null;
    character: { name: string; description: string } | null;
}

interface PortableRuntimeMessageContext {
    messages: MessageDto[];
    activeGenerationId: string | null;
    hasStreamingPresentation: boolean;
}

function runtimeOutputKey(messages: readonly MessageDto[]): string {
    const assistant = [...messages]
        .reverse()
        .find((message) => message.role === 'assistant' && message.status === 'complete');
    return assistant === undefined
        ? ''
        : `${assistant.id}:${assistant.generation_id ?? 'greeting'}`;
}

async function loadPortableRuntimePersona(
    client: InteractionRoomCapableClient,
    conversationId: string,
): Promise<{ name: string; description: string }> {
    const personaClient = client as InteractionRoomCapableClient & Partial<PersonaClientApi>;
    const defaultName = t('chat.runtime.persona.default');
    if (personaClient.getConversationPersonaSelection === undefined) {
        return { name: defaultName, description: '' };
    }
    try {
        const selection = await personaClient.getConversationPersonaSelection({
            conversation_id: conversationId,
        });
        const selectedName = selection.selected_persona?.value.name.trim();
        return {
            name: selectedName === undefined || selectedName === '' ? defaultName : selectedName,
            description: selection.selected_persona?.value.description ?? '',
        };
    } catch {
        return { name: defaultName, description: '' };
    }
}

export class PortableRuntimeLifecycle {
    profile = $state<CharacterRenderProfileDto | null>(null);
    runtime = $state<PortableCharacterRuntime | null>(null);
    selectedCapabilities = $state<PortableRuntimeCapability[]>([]);
    modelCall = $state<PortableRuntimeModelCallStatus | null>(null);
    persistenceStatus = $state<PortableRuntimePersistenceStatus | null>(null);
    phase = $state<PortableRuntimePhase>('idle');
    error = $state<string | null>(null);
    revision = $state(0);

    #grant = $state<PortableRuntimeGrant | null>(null);
    #grantProfile = $state<CharacterRenderProfileDto | null>(null);
    #profileEpoch = 0;
    #creationEpoch = 0;
    #resetEpoch = $state(0);
    #lastOutputKey = '';
    #actionCount = 0;

    constructor(private readonly options: PortableRuntimeLifecycleOptions) {}

    get activeGrant(): PortableRuntimeGrant | null {
        return this.#grantProfile === this.profile ? this.#grant : null;
    }

    capabilities = $derived.by<PortableRuntimeCapability[]>(() =>
        this.profile === null ? [] : requiredPortableRuntimeCapabilities(this.profile),
    );

    variables = $derived.by<Record<string, string>>(() => {
        void this.revision;
        if (this.runtime !== null) return this.runtime.variables;
        if (!(this.activeGrant?.capabilities.includes('profile:read') ?? false)) return {};
        return this.profile?.initial_variables ?? {};
    });

    displayApproved = $derived(this.activeGrant?.capabilities.includes('ui:write') ?? false);

    canReadChat = $derived(this.activeGrant?.capabilities.includes('chat:read') ?? false);

    background = $derived.by(() => {
        void this.revision;
        if (!this.displayApproved) return '';
        return this.runtime?.backgroundMarkup ?? this.profile?.background_markup ?? '';
    });

    lastCharacterMessage = $derived.by(() => {
        void this.revision;
        if (!this.canReadChat) return '';
        const message = [...this.options.displayMessages()]
            .reverse()
            .find((candidate) => candidate.role === 'assistant');
        return message === undefined ? '' : this.effectiveText(message);
    });

    auxiliaryModelOptions = $derived.by(() => {
        const workspace = this.options.providerWorkspace();
        const options: {
            value: string;
            label: string;
            selection: GenerationSelectionInput | null;
        }[] = [{ value: '', label: '현재 기본 생성 모델', selection: null }];
        for (const preset of workspace.presets) {
            const route = workspace.routes.find(
                (candidate) => candidate.id === preset.model_route_id,
            );
            options.push({
                value: `target:${preset.id}`,
                label: `${route?.display_name ?? route?.model_id ?? preset.model_route_id} · ${preset.display_name}`,
                selection: {
                    kind: 'target',
                    target: {
                        model_route_id: preset.model_route_id,
                        generation_preset_id: preset.id,
                    },
                },
            });
        }
        for (const profile of workspace.legacy_profiles) {
            options.push({
                value: `legacy:${profile.id}`,
                label: `${profile.display_name} · ${profile.model}`,
                selection: { kind: 'legacy_profile', provider_profile_id: profile.id },
            });
        }
        return options;
    });

    selectedAuxiliaryModel = $derived.by(() => {
        void this.revision;
        const selection = this.runtime?.auxiliarySelection;
        if (selection === null || selection === undefined) return '';
        return selection.kind === 'target'
            ? `target:${selection.target.generation_preset_id}`
            : `legacy:${selection.provider_profile_id}`;
    });

    modelBudget = $derived.by<PortableRuntimeModelBudgetSnapshot | null>(() => {
        void this.revision;
        return this.runtime?.modelBudget ?? null;
    });

    get requiresLuaRuntime(): boolean {
        return (
            this.profile?.runtime_scripts.some(
                (script) => script.language.trim().toLowerCase() === 'lua',
            ) ?? false
        );
    }

    loadProfile(
        client: InteractionRoomCapableClient | undefined,
        characterId: string | null,
    ): () => void {
        const profileEpoch = ++this.#profileEpoch;
        const getCharacterRenderProfile = client?.getCharacterRenderProfile?.bind(client);
        let cancelled = false;
        this.profile = null;
        this.#grant = null;
        this.#grantProfile = null;
        this.selectedCapabilities = [];
        this.modelCall = null;
        this.persistenceStatus = null;
        if (characterId !== null && getCharacterRenderProfile !== undefined) {
            void getCharacterRenderProfile(characterId)
                .then((profile) => {
                    if (
                        !cancelled &&
                        profileEpoch === this.#profileEpoch &&
                        profile.character_id === characterId
                    ) {
                        this.profile = profile;
                        this.selectedCapabilities = defaultPortableRuntimeCapabilities(profile);
                    }
                })
                .catch(() => {
                    // Legacy characters have no companion render profile and
                    // continue through the ordinary Markdown renderer.
                });
        }
        return () => {
            cancelled = true;
        };
    }

    recreate(context: PortableRuntimeCreationContext): () => void {
        const profile = this.profile;
        const grant = this.activeGrant;
        const { client, conversationId, branchId, character } = context;
        const resetEpoch = this.#resetEpoch;
        const hasLuaRuntime = this.requiresLuaRuntime;
        const hasDynamicProfile =
            hasLuaRuntime ||
            (profile?.output_transforms.length ?? 0) > 0 ||
            (profile?.display_transforms.length ?? 0) > 0 ||
            (profile?.background_markup.trim().length ?? 0) > 0;
        const creationEpoch = ++this.#creationEpoch;
        const runtimeIsCurrent = () =>
            creationEpoch === this.#creationEpoch && resetEpoch === this.#resetEpoch;
        let createdRuntime: PortableCharacterRuntime | null = null;
        this.runtime = null;
        this.error = null;
        this.#lastOutputKey = '';
        this.#actionCount = 0;
        this.modelCall = null;
        this.persistenceStatus = null;
        this.phase = !hasDynamicProfile
            ? 'idle'
            : grant === null
              ? 'blocked'
              : hasLuaRuntime
                ? 'loading'
                : 'ready';
        if (
            !hasLuaRuntime ||
            grant === null ||
            profile === null ||
            client === undefined ||
            conversationId === null ||
            branchId === null ||
            character === null
        ) {
            return () => undefined;
        }
        void loadPortableRuntimePersona(client, conversationId)
            .then((persona) =>
                PortableCharacterRuntime.create({
                    profile,
                    grant,
                    conversationId,
                    branchId,
                    characterName: character.name,
                    characterDescription: character.description,
                    personaName: persona.name,
                    personaDescription: persona.description,
                    client,
                    primarySelection: this.options.primarySelection,
                    onChanged: () => {
                        this.revision += 1;
                    },
                    onNotice: (message, error) => {
                        this.options.onNotice(message);
                        if (error) this.error = message;
                    },
                    onModelCallStatus: (status) => {
                        if (runtimeIsCurrent()) {
                            this.modelCall = status;
                            this.revision += 1;
                        }
                    },
                    onPersistenceStatus: (status) => {
                        if (runtimeIsCurrent()) this.persistenceStatus = status;
                    },
                }),
            )
            .then(async (runtime) => {
                if (!runtimeIsCurrent()) {
                    runtime.close();
                    return;
                }
                createdRuntime = runtime;
                const messages = this.options.currentMessages();
                runtime.setMessages(messages);
                await runtime.refreshDisplay();
                this.#lastOutputKey = runtimeOutputKey(messages);
                this.runtime = runtime;
                this.phase = 'ready';
                this.revision += 1;
            })
            .catch((error: unknown) => {
                if (!runtimeIsCurrent()) return;
                this.phase = 'error';
                this.error =
                    error instanceof Error ? error.message : t('chat.runtime.start_failed');
            });
        return () => {
            createdRuntime?.close();
            if (creationEpoch === this.#creationEpoch) this.#creationEpoch += 1;
        };
    }

    syncMessages(context: PortableRuntimeMessageContext): void {
        const runtime = this.runtime;
        if (runtime === null) return;
        runtime.setMessages(context.messages);
        const outputKey = runtimeOutputKey(context.messages);
        if (
            outputKey !== '' &&
            outputKey !== this.#lastOutputKey &&
            context.activeGenerationId === null &&
            !context.hasStreamingPresentation
        ) {
            this.#lastOutputKey = outputKey;
            this.phase = 'busy';
            this.error = null;
            void runtime
                .afterOutput(context.messages)
                .then(() => {
                    if (runtime !== this.runtime) return;
                    this.phase = 'ready';
                    this.revision += 1;
                })
                .catch((error: unknown) => {
                    if (runtime !== this.runtime) return;
                    this.phase = 'error';
                    this.error =
                        error instanceof Error
                            ? error.message
                            : t('chat.runtime.after_output_failed');
                });
            return;
        }
        void runtime.refreshDisplay().then(() => {
            if (runtime === this.runtime) this.revision += 1;
        });
    }

    async approve(): Promise<void> {
        const profile = this.profile;
        if (profile === null) return;
        const approvalEpoch = this.#profileEpoch;
        this.phase = 'loading';
        this.error = null;
        try {
            const grant = await createPortableRuntimeGrant(profile, this.selectedCapabilities);
            if (approvalEpoch !== this.#profileEpoch || this.profile !== profile) return;
            this.#grantProfile = profile;
            this.#grant = grant;
        } catch (error) {
            this.phase = 'error';
            this.error =
                error instanceof Error ? error.message : t('chat.runtime.approval_create_failed');
        }
    }

    revoke(): void {
        this.#grant = null;
        this.#grantProfile = null;
        this.runtime?.close();
        this.runtime = null;
        this.modelCall = null;
        this.phase = 'blocked';
        this.error = null;
        this.revision += 1;
    }

    resetScope(): void {
        this.#resetEpoch += 1;
    }

    async cancelActiveModelCall(): Promise<PortableRuntimeModelCancellationResult> {
        return (await this.runtime?.cancelActiveModelCall()) ?? 'not_found';
    }

    displayText(message: MessageDto): string {
        void this.revision;
        return this.runtime?.displayText(message) ?? message.content;
    }

    effectiveText(message: MessageDto): string {
        return this.runtime?.effectiveText(message) ?? message.content;
    }

    optionValue(key: string): string {
        void this.revision;
        return this.runtime?.optionValue(key) ?? '';
    }

    async dispatchInput(
        content: string,
        sendMessage: (
            content: string,
            variableOverrides?: OrchestrationVariableMapDto,
        ) => Promise<boolean>,
    ): Promise<boolean | null> {
        let runtime = this.runtime;
        if (this.requiresLuaRuntime && this.activeGrant !== null && runtime === null) {
            const portableRuntimePreparationFallbackNotice =
                '캐릭터 기능을 준비하는 중입니다. 잠시 뒤 다시 보내세요.';
            const copyNotice = this.error ?? portableRuntimePreparationFallbackNotice;
            this.options.onNotice(copyNotice);
            return null;
        }
        const prepared = await this.prepareInput(content);
        let preparedContent = content;
        let handledByRuntime = false;
        if (prepared !== null) {
            runtime = prepared.runtime;
            preparedContent = prepared.text;
            handledByRuntime = prepared.handledByRuntime;
        }
        if (handledByRuntime) return true;
        return runtime === null
            ? sendMessage(preparedContent)
            : sendMessage(preparedContent, this.generationVariableOverrides(runtime));
    }

    async prepareInput(content: string): Promise<{
        runtime: PortableCharacterRuntime;
        text: string;
        handledByRuntime: boolean;
    } | null> {
        const runtime = this.runtime;
        if (runtime === null) return null;
        this.phase = 'busy';
        this.error = null;
        const prepared = await runtime.prepareInput(content);
        this.phase = 'ready';
        return {
            runtime,
            text: prepared.text,
            handledByRuntime: !prepared.shouldSend,
        };
    }

    async setOption(key: string, value: string): Promise<void> {
        const runtime = this.runtime;
        if (runtime === null) return;
        this.phase = 'busy';
        this.error = null;
        try {
            await runtime.setOption(key, value);
            this.phase = 'ready';
            this.revision += 1;
        } catch (error) {
            this.phase = 'error';
            this.error = error instanceof Error ? error.message : t('chat.runtime.option_failed');
        }
    }

    setAuxiliarySelection(selection: GenerationSelectionInput | null): void {
        if (this.runtime === null) return;
        this.runtime.setAuxiliarySelection(selection);
        this.revision += 1;
    }

    setAuxiliaryModel(value: string): void {
        const option = this.auxiliaryModelOptions.find((candidate) => candidate.value === value);
        this.setAuxiliarySelection(option?.selection ?? null);
    }

    async handleAction(action: string): Promise<void> {
        const runtime = this.runtime;
        if (runtime === null) return;
        if (this.#actionCount === 0) this.error = null;
        this.#actionCount += 1;
        this.phase = 'busy';
        try {
            await runtime.handleAction(action);
            this.revision += 1;
        } catch (error) {
            this.error = error instanceof Error ? error.message : t('chat.runtime.action_failed');
        } finally {
            this.#actionCount = Math.max(0, this.#actionCount - 1);
            if (runtime === this.runtime && this.#actionCount === 0) {
                this.phase = this.error === null ? 'ready' : 'error';
            }
        }
    }

    dismissError(): string | null {
        const dismissed = this.error;
        this.error = null;
        return dismissed;
    }

    dismissErrorNotice(notice: string): string {
        return notice === this.dismissError() ? '' : notice;
    }

    fail(error: unknown, fallback: string): string {
        const message = error instanceof Error ? error.message : fallback;
        this.phase = 'error';
        this.error = message;
        return message;
    }

    generationVariableOverrides(runtime: PortableCharacterRuntime): OrchestrationVariableMapDto {
        const additions = Object.entries(runtime.generationVariables)
            .sort(([left], [right]) => left.localeCompare(right))
            .map(([id, value]) => ({
                variable: { scope: 'character' as const, namespace: null, id },
                value: { type: 'text' as const, value },
            }));
        return { values: additions };
    }
}
