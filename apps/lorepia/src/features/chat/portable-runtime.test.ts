// @vitest-environment node

import { afterEach, describe, expect, it, vi } from 'vitest';
import { LuaFactory } from 'wasmoon';

import type {
    CharacterRenderProfileDto,
    GenerateRuntimeTextInput,
    GenerationSelectionInput,
    LorepiaClient,
    MessageDto,
    RuntimeTextGenerationDto,
} from '../../lib/ipc/contracts';
import type {
    PortableRuntimeStateRecordDto,
    PortableRuntimeStateScopeInput,
    PutPortableRuntimeStateInput,
} from '../../lib/ipc/portable-runtime-state-contracts';
import { t } from '../../lib/i18n';
import {
    PortableCharacterRuntime,
    createPortableRuntimeGrant,
    defaultPortableRuntimeCapabilities,
    parsePortableRuntimeToggles,
    requiredPortableRuntimeCapabilities,
    type PortableRuntimeCapability,
    type PortableRuntimePersistenceStatus,
    validatePortableRuntimeGrant,
} from './portable-runtime';
import { PortableRuntimeKernel } from './portable-runtime-kernel';
import { resetPortableRuntimeModelBudgetsForTests } from './portable-runtime-model-policy';
import type {
    PortableRuntimeHostCallMessage,
    PortableRuntimeMainMessage,
    PortableRuntimePersistedState,
    PortableRuntimeWorkerMessage,
} from './portable-runtime-protocol';
import type { PortableRuntimeWorkerEndpoint } from './portable-runtime-worker-client';

const PRIMARY_SELECTION: GenerationSelectionInput = {
    kind: 'legacy_profile',
    provider_profile_id: 'primary',
};
const AUXILIARY_SELECTION: GenerationSelectionInput = {
    kind: 'legacy_profile',
    provider_profile_id: 'auxiliary',
};
const TEST_WASM_PATH = decodeURIComponent(
    new URL('../../../node_modules/wasmoon/dist/glue.wasm', import.meta.url).pathname,
);

const LUA_SOURCE = String.raw`
listenEdit("editInput", function(triggerId, text)
    return text .. "!"
end)

listenEdit("editDisplay", function(triggerId, text, meta)
    return text .. "<div>display-" .. tostring(meta.index) .. "</div>"
end)

onStart = async(function(triggerId)
    assert(getChat(triggerId, 999) == nil)
    local fullChat = getFullChat(triggerId)
    assert(type(fullChat) == "table" and #fullChat == 3)
    assert(fullChat[1].role == "user")
    assert(fullChat[2].role == "char")
    assert(fullChat[3].role == "user")
    assert(tonumber(getChatVar(triggerId, "missing")) == nil)
    assert(getState(triggerId, "missing") == nil)
    assert(getGlobalVar(triggerId, "missing") == nil)
    assert(getPersonaName(triggerId) == "Persona")
    assert(getPersonaDescription(triggerId) == "Persona description")
    assert(getDescription(triggerId) == "Description")
    local namedLore = getLoreBooks(triggerId, "Runtime lore")
    assert(type(namedLore) == "table" and namedLore[1].content == "Hello Persona enabled")
    local activeLore = loadLoreBooks(triggerId)
    assert(type(activeLore) == "table" and activeLore[1].data == "Hello Persona enabled")
    local response = LLM(triggerId, {
        { role = "user", content = "main prompt" }
    }, false, { streaming = false })
    setChatVar(triggerId, "main-result", response.result)
    setState(triggerId, "nested", { version = "1", enabled = true })
    setBackgroundEmbedding(triggerId, "<style>.runtime { color: red; }</style>")
    return true
end)

onOutput = async(function(triggerId)
    setChatVar(triggerId, "output-ran", "1")
end)

onButtonClick = async(function(triggerId, code)
    local response = axLLM(triggerId, {
        { role = "user", content = "aux prompt" }
    }, false, { streaming = false })
    local latest = getChat(triggerId, -1)
    setChat(triggerId, -1, latest.data .. "|" .. response.result .. "|" .. code)
end)
`;

function profile(): CharacterRenderProfileDto {
    return {
        character_id: 'character',
        character_content_revision_id: 'revision',
        assets: [],
        background_markup: '',
        toggle_schema: [
            '= Runtime options=divider',
            'mode=Generation mode=select=Off,Auxiliary,Primary',
            'music=Background music=toggle',
            'note=Author note=text',
            '=Explanation=caption',
        ].join('\n'),
        initial_variables: { mode: '0', music: '0', note: '' },
        output_transforms: [],
        display_transforms: [],
        runtime_scripts: [
            {
                id: 'script',
                name: 'Runtime',
                event: 'start',
                language: 'lua',
                source: LUA_SOURCE,
                elevated_access: false,
            },
        ],
        required_runtime_capabilities: [
            'runtime:callbacks',
            'chat:read',
            'chat:write',
            'state:readwrite',
            'profile:read',
            'lore:read',
            'ui:write',
            'model:primary',
            'model:auxiliary',
        ],
        runtime_capabilities_declared: true,
        runtime_knowledge: [
            {
                id: 'lore',
                name: 'Runtime lore',
                content:
                    'Hello {{user}} {{#if::{{equal::{{getglobalvar::mode}}::0}}}}enabled{{/if}}',
                enabled: true,
                primary_keys: [],
                secondary_keys: [],
                constant: true,
                selective: false,
                case_sensitive: false,
                whole_word: false,
                use_regex: false,
                probability_basis_points: 10_000,
                folder: false,
            },
        ],
        runtime_script_count: 1,
    };
}

function message(id: string, role: MessageDto['role'], content: string): MessageDto {
    return {
        id,
        conversation_id: 'conversation',
        parent_id: null,
        role,
        content,
        status: 'complete',
        generation_id: role === 'assistant' ? 'generation' : null,
        created_at: '2026-08-28T00:00:00Z',
    };
}

afterEach(() => {
    vi.restoreAllMocks();
    resetPortableRuntimeModelBudgetsForTests();
});

function inProcessWorkerFactory(): PortableRuntimeWorkerEndpoint {
    type ListenerType = 'message' | 'error' | 'messageerror';
    const listeners: Record<ListenerType, Set<EventListenerOrEventListenerObject>> = {
        message: new Set(),
        error: new Set(),
        messageerror: new Set(),
    };
    let terminated = false;
    const dispatch = (type: ListenerType, event: Event): void => {
        if (terminated) return;
        for (const listener of listeners[type]) {
            if (typeof listener === 'function') listener.call(endpoint, event);
            else listener.handleEvent(event);
        }
    };
    const kernel = new PortableRuntimeKernel({
        luaFactory: new LuaFactory(TEST_WASM_PATH),
        postMessage: (message: PortableRuntimeWorkerMessage) => {
            queueMicrotask(() =>
                dispatch('message', new MessageEvent('message', { data: message })),
            );
        },
    });
    const endpoint: PortableRuntimeWorkerEndpoint = {
        addEventListener: (type: string, listener: EventListenerOrEventListenerObject | null) => {
            if (listener !== null && type in listeners) {
                listeners[type as ListenerType].add(listener);
            }
        },
        removeEventListener: (
            type: string,
            listener: EventListenerOrEventListenerObject | null,
        ) => {
            if (listener !== null && type in listeners) {
                listeners[type as ListenerType].delete(listener);
            }
        },
        postMessage: (message: unknown) => {
            queueMicrotask(() => {
                if (!terminated) kernel.receive(message as PortableRuntimeMainMessage);
            });
        },
        terminate: () => {
            if (terminated) return;
            terminated = true;
            kernel.close();
            for (const values of Object.values(listeners)) values.clear();
        },
    };
    return endpoint;
}

describe('portable runtime', () => {
    it('parses portable controls without exposing structural rows', () => {
        expect(parsePortableRuntimeToggles(profile().toggle_schema)).toEqual([
            {
                key: 'mode',
                label: 'Generation mode',
                kind: 'select',
                choices: ['Off', 'Auxiliary', 'Primary'],
            },
            {
                key: 'music',
                label: 'Background music',
                kind: 'toggle',
                choices: [],
            },
            {
                key: 'note',
                label: 'Author note',
                kind: 'text',
                choices: [],
            },
        ]);
    });

    it('executes callbacks, nullable host values, generation, state, and message edits', async () => {
        const generateRuntimeText = vi.fn(
            (input: GenerateRuntimeTextInput): Promise<RuntimeTextGenerationDto> =>
                Promise.resolve({
                    request_id: input.request_id,
                    result:
                        input.selection.kind === 'legacy_profile' &&
                        input.selection.provider_profile_id === 'auxiliary'
                            ? 'AUXILIARY_RESULT'
                            : 'PRIMARY_RESULT',
                    usage: {
                        input_tokens: 2,
                        cached_read_tokens: null,
                        cached_write_tokens: null,
                        output_tokens: 3,
                        reasoning_tokens: null,
                        tool_tokens: null,
                    },
                }),
        );
        const client = {
            generateRuntimeText,
            cancelRuntimeText: vi.fn().mockResolvedValue(true),
        } as unknown as LorepiaClient;
        const notices = vi.fn();
        const changed = vi.fn();
        const activeProfile = profile();
        const runtime = await PortableCharacterRuntime.create({
            profile: activeProfile,
            grant: await createPortableRuntimeGrant(
                activeProfile,
                requiredPortableRuntimeCapabilities(activeProfile),
            ),
            conversationId: 'conversation',
            branchId: 'branch',
            characterName: 'Character',
            characterDescription: 'Description',
            personaName: 'Persona',
            personaDescription: 'Persona description',
            client,
            primarySelection: () => PRIMARY_SELECTION,
            onChanged: changed,
            onNotice: notices,
            storage: memoryStorage(),
            workerFactory: inProcessWorkerFactory,
        });
        const messages = [
            message('user', 'user', 'hello'),
            message('assistant', 'assistant', 'world'),
        ];
        runtime.setMessages(messages);
        runtime.setAuxiliarySelection(AUXILIARY_SELECTION);

        const prepared = await runtime.prepareInput('next');
        expect(prepared).toEqual({ text: 'next!', shouldSend: true });
        expect(runtime.backgroundMarkup).toContain('.runtime');
        expect(runtime.variables['main-result']).toBe('PRIMARY_RESULT');
        expect(runtime.generationVariables['main-result']).toBeUndefined();
        const primaryInput = generateRuntimeText.mock.calls[0]?.[0];
        expect(primaryInput).toMatchObject({
            audit: {
                character_id: 'character',
                character_content_revision_id: 'revision',
                capability: 'model:primary',
            },
            selection: PRIMARY_SELECTION,
            messages: [{ role: 'user', content: 'main prompt' }],
        });
        expect(primaryInput?.request_id).toMatch(/^[0-9a-f-]{36}$/);
        expect(primaryInput?.audit.grant_sha256).toMatch(/^[0-9a-f]{64}$/);

        await runtime.afterOutput(messages);
        const assistant = messages[1];
        if (assistant === undefined) throw new Error('assistant fixture is missing');
        expect(runtime.displayText(assistant)).toContain('display-1');

        await runtime.handleAction('generate__feature__1');
        expect(runtime.effectiveText(assistant)).toBe(
            'world|AUXILIARY_RESULT|generate__feature__1',
        );
        const auxiliaryInput = generateRuntimeText.mock.calls[1]?.[0];
        expect(auxiliaryInput).toMatchObject({
            audit: {
                character_id: 'character',
                character_content_revision_id: 'revision',
                capability: 'model:auxiliary',
            },
            selection: AUXILIARY_SELECTION,
            messages: [{ role: 'user', content: 'aux prompt' }],
        });
        expect(auxiliaryInput?.request_id).toMatch(/^[0-9a-f-]{36}$/);
        expect(auxiliaryInput?.audit.grant_sha256).toMatch(/^[0-9a-f]{64}$/);
        for (let index = 0; index < 10; index += 1) {
            await runtime.handleAction(`budget-${String(index)}`);
        }
        expect(generateRuntimeText).toHaveBeenCalledTimes(3);
        expect(notices).not.toHaveBeenCalled();
        expect(changed).toHaveBeenCalled();

        runtime.close();
    });

    it('fails closed before Wasmoon starts without an exact script and revision grant', async () => {
        const approvedProfile = profile();
        const grant = await createPortableRuntimeGrant(approvedProfile);
        const changedProfile = {
            ...approvedProfile,
            character_content_revision_id: 'revision-2',
            runtime_scripts: approvedProfile.runtime_scripts.map((script) => ({
                ...script,
                source: `${script.source}\n-- changed`,
            })),
        };
        const workerFactory = vi.fn<() => PortableRuntimeWorkerEndpoint>();
        const options = {
            profile: changedProfile,
            grant,
            conversationId: 'conversation',
            branchId: 'branch',
            characterName: 'Character',
            characterDescription: 'Description',
            client: {} as LorepiaClient,
            primarySelection: () => PRIMARY_SELECTION,
            onChanged: vi.fn(),
            onNotice: vi.fn(),
            storage: memoryStorage(),
            workerFactory,
        };

        await expect(validatePortableRuntimeGrant(changedProfile, grant)).resolves.toBe(false);
        await expect(
            validatePortableRuntimeGrant(
                {
                    ...approvedProfile,
                    runtime_knowledge: approvedProfile.runtime_knowledge.map((entry) => ({
                        ...entry,
                        content: `${entry.content} changed`,
                    })),
                },
                grant,
            ),
        ).resolves.toBe(false);
        await expect(PortableCharacterRuntime.create(options)).rejects.toThrow(/승인/);
        expect(workerFactory).not.toHaveBeenCalled();
    });

    it('limits undeclared legacy profiles to inferred least-privilege capabilities', async () => {
        const legacyProfile: CharacterRenderProfileDto = {
            ...profile(),
            required_runtime_capabilities: [],
            runtime_capabilities_declared: false,
        };
        expect(requiredPortableRuntimeCapabilities(legacyProfile)).toEqual([
            'runtime:callbacks',
            'ui:write',
        ]);
        expect(defaultPortableRuntimeCapabilities(legacyProfile)).toEqual([
            'runtime:callbacks',
            'ui:write',
        ]);

        const legacyPresentationProfile: CharacterRenderProfileDto = {
            ...legacyProfile,
            background_markup: '<section>legacy</section>',
            output_transforms: [{ pattern: 'a', replacement: 'b', flags: 'g' }],
        };
        expect(requiredPortableRuntimeCapabilities(legacyPresentationProfile)).toEqual([
            'chat:read',
            'profile:read',
            'runtime:callbacks',
            'ui:write',
        ]);
        expect(defaultPortableRuntimeCapabilities(legacyPresentationProfile)).toEqual([
            'runtime:callbacks',
            'ui:write',
        ]);

        const legacyMarkupOnlyProfile: CharacterRenderProfileDto = {
            ...legacyPresentationProfile,
            runtime_scripts: [],
            runtime_script_count: 0,
        };
        expect(requiredPortableRuntimeCapabilities(legacyMarkupOnlyProfile)).toEqual([
            'chat:read',
            'profile:read',
            'ui:write',
        ]);
        expect(defaultPortableRuntimeCapabilities(legacyMarkupOnlyProfile)).toEqual(['ui:write']);

        for (const sensitiveCapability of [
            'chat:write',
            'state:readwrite',
            'lore:read',
            'model:primary',
            'model:auxiliary',
            'elevated',
        ] as const) {
            await expect(
                createPortableRuntimeGrant(legacyProfile, [
                    'runtime:callbacks',
                    sensitiveCapability,
                ]),
            ).rejects.toThrow(t('chat.runtime.approval_required'));

            const forgedCapabilities: PortableRuntimeCapability[] = [
                'runtime:callbacks',
                sensitiveCapability,
            ];
            forgedCapabilities.sort();
            const digest = await globalThis.crypto.subtle.digest(
                'SHA-256',
                new TextEncoder().encode(
                    JSON.stringify({
                        version: 1,
                        profile: legacyProfile,
                        capabilities: forgedCapabilities,
                    }),
                ),
            );
            const manifestSha256 = [...new Uint8Array(digest)]
                .map((byte) => byte.toString(16).padStart(2, '0'))
                .join('');
            await expect(
                validatePortableRuntimeGrant(legacyProfile, {
                    version: 1,
                    manifestSha256,
                    capabilities: forgedCapabilities,
                }),
            ).resolves.toBe(false);
        }

        const baseScript = legacyProfile.runtime_scripts[0];
        if (baseScript === undefined) throw new Error('runtime fixture script is missing');
        const legacyElevatedProfile: CharacterRenderProfileDto = {
            ...legacyProfile,
            runtime_scripts: [{ ...baseScript, elevated_access: true }],
        };
        expect(requiredPortableRuntimeCapabilities(legacyElevatedProfile)).toEqual([
            'runtime:callbacks',
            'ui:write',
        ]);
        await expect(
            createPortableRuntimeGrant(legacyElevatedProfile, ['runtime:callbacks', 'elevated']),
        ).rejects.toThrow(t('chat.runtime.approval_required'));
    });

    it('binds grants to the reviewed capability set and omits ungranted host globals', async () => {
        const baseProfile = profile();
        const baseScript = baseProfile.runtime_scripts[0];
        if (baseScript === undefined) throw new Error('runtime fixture script is missing');
        const restrictedProfile = {
            ...baseProfile,
            runtime_scripts: [
                {
                    ...baseScript,
                    source: 'assert(getFullChat == nil)\nassert(setChat == nil)\nassert(LLM == nil)\nassert(axLLM == nil)',
                },
            ],
        };
        const capabilities = ['runtime:callbacks'] as const;
        const grant = await createPortableRuntimeGrant(restrictedProfile, capabilities);
        const runtime = await PortableCharacterRuntime.create({
            profile: restrictedProfile,
            grant,
            conversationId: 'conversation',
            branchId: 'branch',
            characterName: 'Character',
            characterDescription: 'Description',
            client: {} as LorepiaClient,
            primarySelection: () => PRIMARY_SELECTION,
            onChanged: vi.fn(),
            onNotice: vi.fn(),
            storage: memoryStorage(),
            workerFactory: inProcessWorkerFactory,
        });

        expect(requiredPortableRuntimeCapabilities(restrictedProfile)).toContain('model:primary');
        expect(requiredPortableRuntimeCapabilities(restrictedProfile)).toContain('model:auxiliary');
        await expect(createPortableRuntimeGrant(restrictedProfile)).resolves.toMatchObject({
            capabilities: ['runtime:callbacks', 'ui:write'],
        });
        await expect(
            validatePortableRuntimeGrant(restrictedProfile, {
                ...grant,
                capabilities: [...grant.capabilities, 'chat:read'],
            }),
        ).resolves.toBe(false);
        const declaredProfile: CharacterRenderProfileDto = {
            ...restrictedProfile,
            required_runtime_capabilities: ['runtime:callbacks'],
            runtime_capabilities_declared: true,
        };
        expect(requiredPortableRuntimeCapabilities(declaredProfile)).toEqual(['runtime:callbacks']);
        await expect(
            createPortableRuntimeGrant(declaredProfile, ['runtime:callbacks', 'chat:read']),
        ).rejects.toThrow(t('chat.runtime.approval_required'));
        const hostBoundary = runtime as unknown as {
            handleHostCall: (call: PortableRuntimeHostCallMessage) => Promise<unknown>;
        };
        await expect(
            hostBoundary.handleHostCall({
                channel: 'lorepia-portable-runtime-v1',
                type: 'host-call',
                callId: 'forged-call',
                target: 'primary',
                messages: [{ role: 'user', content: 'must not run' }],
            }),
        ).rejects.toThrow(/ungranted host capability/);
        runtime.close();
    });

    it('does not let callback or macro compatibility bypass denied read and write grants', async () => {
        const restrictedProfile = profile();
        const baseScript = restrictedProfile.runtime_scripts[0];
        if (baseScript === undefined) throw new Error('runtime fixture script is missing');
        restrictedProfile.initial_variables = { secret_profile_value: 'PROFILE_SECRET' };
        restrictedProfile.runtime_scripts = [
            {
                ...baseScript,
                source: String.raw`
assert(cbs("{{user}}|{{char}}|{{description}}|{{getglobalvar::secret_profile_value}}|{{getvar::secret_state_value}}|{{lastcharmessage}}|{{chat_index}}|{{lastmessageid}}") == "|||0|0|||")
listenEdit("editInput", function(triggerId, text)
    return "CAPTURED:" .. text
end)
listenEdit("editDisplay", function(triggerId, text)
    return "SPOOFED:" .. text
end)
`,
            },
        ];
        const storage = memoryStorage();
        storage.setItem(
            runtimeStorageKey({
                character_id: 'character',
                character_content_revision_id: 'revision',
                conversation_id: 'conversation',
                branch_id: 'branch',
            }),
            JSON.stringify({
                options: {},
                chatVars: { secret_state_value: 'STATE_SECRET' },
                state: {},
                messageOverrides: {},
                background: '',
                auxiliarySelection: null,
            }),
        );
        const runtime = await PortableCharacterRuntime.create({
            profile: restrictedProfile,
            grant: await createPortableRuntimeGrant(restrictedProfile, ['runtime:callbacks']),
            conversationId: 'conversation',
            branchId: 'branch',
            characterName: 'Character',
            characterDescription: 'Description',
            personaName: 'Persona',
            personaDescription: 'Persona description',
            client: {} as LorepiaClient,
            primarySelection: () => PRIMARY_SELECTION,
            onChanged: vi.fn(),
            onNotice: vi.fn(),
            storage,
            workerFactory: inProcessWorkerFactory,
        });
        const assistant = message('assistant', 'assistant', 'private message');
        runtime.setMessages([assistant]);

        await expect(runtime.prepareInput('private draft')).resolves.toEqual({
            text: 'private draft',
            shouldSend: true,
        });
        expect(runtime.displayText(assistant)).toBe('private message');
        expect(runtime.variables).toEqual({});
        runtime.close();
    });

    it('does not use denied chat text as an active-lore matching oracle', async () => {
        const restrictedProfile = profile();
        const baseScript = restrictedProfile.runtime_scripts[0];
        if (baseScript === undefined) throw new Error('runtime fixture script is missing');
        restrictedProfile.runtime_scripts = [
            {
                ...baseScript,
                source: String.raw`
onOutput = async(function(triggerId)
    local active = loadLoreBooks(triggerId)
    assert(type(active) == "table" and #active == 1)
    assert(active[1].name == "Runtime lore")
end)
`,
            },
        ];
        restrictedProfile.runtime_knowledge = [
            ...restrictedProfile.runtime_knowledge,
            {
                id: 'secret-probe',
                name: 'Secret probe',
                content: 'MATCHED_SECRET_PROBE',
                enabled: true,
                primary_keys: ['SECRET-CHAT-CONTENT'],
                secondary_keys: [],
                constant: false,
                selective: false,
                case_sensitive: true,
                whole_word: false,
                use_regex: false,
                probability_basis_points: 10_000,
                folder: false,
            },
        ];
        const runtime = await PortableCharacterRuntime.create({
            profile: restrictedProfile,
            grant: await createPortableRuntimeGrant(restrictedProfile, [
                'runtime:callbacks',
                'lore:read',
            ]),
            conversationId: 'conversation',
            branchId: 'branch',
            characterName: 'Character',
            characterDescription: 'Description',
            client: {} as LorepiaClient,
            primarySelection: () => PRIMARY_SELECTION,
            onChanged: vi.fn(),
            onNotice: vi.fn(),
            storage: memoryStorage(),
            workerFactory: inProcessWorkerFactory,
        });

        await expect(
            runtime.afterOutput([message('assistant-secret', 'assistant', 'SECRET-CHAT-CONTENT')]),
        ).resolves.toBeUndefined();
        runtime.close();
    });

    it('separates primary and auxiliary model authority at both Lua and host boundaries', async () => {
        const primaryOnlyProfile = profile();
        const baseScript = primaryOnlyProfile.runtime_scripts[0];
        if (baseScript === undefined) throw new Error('runtime fixture script is missing');
        primaryOnlyProfile.runtime_scripts = [
            {
                ...baseScript,
                source: 'assert(type(LLM) == "function")\nassert(axLLM == nil)',
            },
        ];
        const capabilities = ['runtime:callbacks', 'model:primary'] as const;
        const generateRuntimeText = vi.fn();
        const runtime = await PortableCharacterRuntime.create({
            profile: primaryOnlyProfile,
            grant: await createPortableRuntimeGrant(primaryOnlyProfile, capabilities),
            conversationId: 'conversation',
            branchId: 'branch',
            characterName: 'Character',
            characterDescription: 'Description',
            client: { generateRuntimeText } as unknown as LorepiaClient,
            primarySelection: () => PRIMARY_SELECTION,
            onChanged: vi.fn(),
            onNotice: vi.fn(),
            storage: memoryStorage(),
            workerFactory: inProcessWorkerFactory,
        });
        const hostBoundary = runtime as unknown as {
            handleHostCall: (call: PortableRuntimeHostCallMessage) => Promise<unknown>;
        };

        await expect(
            hostBoundary.handleHostCall({
                channel: 'lorepia-portable-runtime-v1',
                type: 'host-call',
                callId: 'forged-auxiliary-call',
                target: 'auxiliary',
                messages: [{ role: 'user', content: 'must not run' }],
            }),
        ).rejects.toThrow(/ungranted host capability/);
        expect(generateRuntimeText).not.toHaveBeenCalled();
        const unsupportedResult = (await hostBoundary.handleHostCall({
            channel: 'lorepia-portable-runtime-v1',
            type: 'host-call',
            callId: 'uncancellable-primary-call',
            target: 'primary',
            messages: [{ role: 'user', content: 'must not run' }],
        })) as { success: boolean; result: string };
        expect(unsupportedResult.success).toBe(false);
        expect(unsupportedResult.result).toMatch(/지원|support/i);
        expect(generateRuntimeText).not.toHaveBeenCalled();
        runtime.close();
    });

    it('does not evaluate elevated scripts unless the exact elevated capability is granted', async () => {
        const elevatedProfile = profile();
        const baseScript = elevatedProfile.runtime_scripts[0];
        if (baseScript === undefined) throw new Error('runtime fixture script is missing');
        elevatedProfile.runtime_scripts = [
            {
                ...baseScript,
                source: 'error("elevated script executed")',
                elevated_access: true,
            },
        ];
        elevatedProfile.required_runtime_capabilities = [
            ...elevatedProfile.required_runtime_capabilities,
            'elevated',
        ];
        const commonOptions = {
            profile: elevatedProfile,
            conversationId: 'conversation',
            branchId: 'branch',
            characterName: 'Character',
            characterDescription: 'Description',
            client: {} as LorepiaClient,
            primarySelection: () => PRIMARY_SELECTION,
            onChanged: vi.fn(),
            onNotice: vi.fn(),
            storage: memoryStorage(),
            workerFactory: inProcessWorkerFactory,
        };

        const safeRuntime = await PortableCharacterRuntime.create({
            ...commonOptions,
            grant: await createPortableRuntimeGrant(elevatedProfile, ['runtime:callbacks']),
        });
        safeRuntime.close();

        await expect(
            PortableCharacterRuntime.create({
                ...commonOptions,
                grant: await createPortableRuntimeGrant(elevatedProfile, [
                    'runtime:callbacks',
                    'elevated',
                ]),
            }),
        ).rejects.toThrow(/elevated script executed/);
    });

    it('does not evaluate ordinary scripts when runtime execution is not granted', async () => {
        const unapprovedProfile = profile();
        const baseScript = unapprovedProfile.runtime_scripts[0];
        if (baseScript === undefined) throw new Error('runtime fixture script is missing');
        unapprovedProfile.runtime_scripts = [
            { ...baseScript, source: 'error("ordinary script executed")' },
        ];

        const runtime = await PortableCharacterRuntime.create({
            profile: unapprovedProfile,
            grant: await createPortableRuntimeGrant(unapprovedProfile, ['ui:write']),
            conversationId: 'conversation',
            branchId: 'branch',
            characterName: 'Character',
            characterDescription: 'Description',
            client: {} as LorepiaClient,
            primarySelection: () => PRIMARY_SELECTION,
            onChanged: vi.fn(),
            onNotice: vi.fn(),
            storage: memoryStorage(),
            workerFactory: inProcessWorkerFactory,
        });

        runtime.close();
    });

    it('exposes one active model request and forwards cancellation by exact request id', async () => {
        const cancellableProfile = profile();
        const baseScript = cancellableProfile.runtime_scripts[0];
        if (baseScript === undefined) throw new Error('runtime fixture script is missing');
        cancellableProfile.runtime_scripts = [
            {
                ...baseScript,
                source: String.raw`
onButtonClick = async(function(triggerId)
    LLM(triggerId, {{ role = "user", content = "cancel me" }}, false, {{}})
end)
`,
            },
        ];
        let rejectGeneration: ((reason: Error) => void) | undefined;
        let startedGeneration: (() => void) | undefined;
        const started = new Promise<void>((resolve) => {
            startedGeneration = resolve;
        });
        const generateRuntimeText = vi.fn(
            (input: GenerateRuntimeTextInput) =>
                new Promise<RuntimeTextGenerationDto>((_resolve, reject) => {
                    void input;
                    rejectGeneration = reject;
                    startedGeneration?.();
                }),
        );
        const cancelRuntimeText = vi.fn().mockResolvedValue(true);
        const onModelCallStatus = vi.fn();
        const runtime = await PortableCharacterRuntime.create({
            profile: cancellableProfile,
            grant: await createPortableRuntimeGrant(cancellableProfile, [
                'runtime:callbacks',
                'model:primary',
            ]),
            conversationId: 'conversation',
            branchId: 'branch',
            characterName: 'Character',
            characterDescription: 'Description',
            client: { generateRuntimeText, cancelRuntimeText } as unknown as LorepiaClient,
            primarySelection: () => PRIMARY_SELECTION,
            onChanged: vi.fn(),
            onNotice: vi.fn(),
            onModelCallStatus,
            storage: memoryStorage(),
            workerFactory: inProcessWorkerFactory,
        });

        const action = runtime.handleAction('cancel');
        await started;
        const request = generateRuntimeText.mock.calls[0]?.[0];
        expect(request?.request_id).toMatch(/^[0-9a-f-]{36}$/);
        expect(onModelCallStatus).toHaveBeenLastCalledWith(
            expect.objectContaining({ requestId: request?.request_id, target: 'primary' }),
        );
        await expect(runtime.cancelActiveModelCall()).resolves.toBe(true);
        expect(cancelRuntimeText).toHaveBeenCalledWith(request?.request_id);

        rejectGeneration?.(new Error('cancelled'));
        await action;
        expect(onModelCallStatus).toHaveBeenLastCalledWith(null);
        runtime.close();
    });

    it('cancels the exact active model request once when an event deadline detaches its worker', async () => {
        const timedProfile = profile();
        const baseScript = timedProfile.runtime_scripts[0];
        if (baseScript === undefined) throw new Error('runtime fixture script is missing');
        timedProfile.runtime_scripts = [
            {
                ...baseScript,
                source: String.raw`
onButtonClick = async(function(triggerId)
    LLM(triggerId, {{ role = "user", content = "wait forever" }}, false, {{}})
end)
`,
            },
        ];
        let startedGeneration: (() => void) | undefined;
        const started = new Promise<void>((resolve) => {
            startedGeneration = resolve;
        });
        const generateRuntimeText = vi.fn(
            (input: GenerateRuntimeTextInput) =>
                new Promise<RuntimeTextGenerationDto>(() => {
                    void input;
                    startedGeneration?.();
                }),
        );
        const cancelRuntimeText = vi.fn().mockResolvedValue(true);
        const runtime = await PortableCharacterRuntime.create({
            profile: timedProfile,
            grant: await createPortableRuntimeGrant(timedProfile, [
                'runtime:callbacks',
                'model:primary',
            ]),
            conversationId: 'conversation',
            branchId: 'branch',
            characterName: 'Character',
            characterDescription: 'Description',
            client: { generateRuntimeText, cancelRuntimeText } as unknown as LorepiaClient,
            primarySelection: () => PRIMARY_SELECTION,
            onChanged: vi.fn(),
            onNotice: vi.fn(),
            storage: memoryStorage(),
            eventTimeoutMs: 50,
            workerFactory: inProcessWorkerFactory,
        });

        const action = runtime.handleAction('timeout');
        await started;
        await expect(action).rejects.toThrow(/시간|timeout/i);
        const requestId = generateRuntimeText.mock.calls[0]?.[0]?.request_id;
        expect(cancelRuntimeText).toHaveBeenCalledTimes(1);
        expect(cancelRuntimeText).toHaveBeenCalledWith(requestId);

        await runtime.cancelActiveModelCall();
        runtime.close();
        expect(cancelRuntimeText).toHaveBeenCalledTimes(1);
    });

    it('removes Lua timeout-bypass and dynamic-loader globals before imported code', async () => {
        const hardenedProfile = profile();
        const script = hardenedProfile.runtime_scripts[0];
        if (script === undefined) throw new Error('runtime fixture script is missing');
        hardenedProfile.runtime_scripts = [
            {
                ...script,
                source: String.raw`
if debug ~= nil then debug.sethook() end
assert(debug == nil)
assert(package == nil)
assert(io == nil)
assert(os == nil)
assert(load == nil)
assert(loadfile == nil)
assert(dofile == nil)
assert(require == nil)
assert(loadstring == nil)
assert(window == nil)
assert(document == nil)
assert(fetch == nil)
assert(XMLHttpRequest == nil)
assert(WebSocket == nil)
assert(Worker == nil)
assert(SharedWorker == nil)
assert(globalThis == nil)
assert(self == nil)
assert(postMessage == nil)
assert(__hostMainGeneration == nil)
assert(__hostAuxGeneration == nil)
assert(__TAURI__ == nil)
assert(__TAURI_INTERNALS__ == nil)
`,
            },
        ];

        const runtime = await PortableCharacterRuntime.create({
            profile: hardenedProfile,
            grant: await createPortableRuntimeGrant(
                hardenedProfile,
                requiredPortableRuntimeCapabilities(hardenedProfile),
            ),
            conversationId: 'conversation',
            branchId: 'branch',
            characterName: 'Character',
            characterDescription: 'Description',
            client: {} as LorepiaClient,
            primarySelection: () => PRIMARY_SELECTION,
            onChanged: vi.fn(),
            onNotice: vi.fn(),
            storage: memoryStorage(),
            workerFactory: inProcessWorkerFactory,
        });

        runtime.close();
    });

    it('interrupts non-yielding startup code within the runtime deadline', async () => {
        const loopingProfile = profile();
        const script = loopingProfile.runtime_scripts[0];
        if (script === undefined) throw new Error('runtime fixture script is missing');
        loopingProfile.runtime_scripts = [{ ...script, source: 'while true do end' }];
        const startedAt = Date.now();

        await expect(
            PortableCharacterRuntime.create({
                profile: loopingProfile,
                grant: await createPortableRuntimeGrant(loopingProfile),
                conversationId: 'conversation',
                branchId: 'branch',
                characterName: 'Character',
                characterDescription: 'Description',
                client: {} as LorepiaClient,
                primarySelection: () => PRIMARY_SELECTION,
                onChanged: vi.fn(),
                onNotice: vi.fn(),
                storage: memoryStorage(),
                workerFactory: inProcessWorkerFactory,
            }),
        ).rejects.toThrow(/timeout/i);
        expect(Date.now() - startedAt).toBeLessThan(2_000);
    });

    it('bounds renderer-facing events emitted by one Lua request', async () => {
        const noisyProfile = profile();
        const script = noisyProfile.runtime_scripts[0];
        if (script === undefined) throw new Error('runtime fixture script is missing');
        noisyProfile.runtime_scripts = [
            {
                ...script,
                source: String.raw`
onStart = async(function(triggerId)
    for index = 1, 1000 do
        alertNormal(triggerId, "notice-" .. tostring(index))
        reloadChat(triggerId)
    end
    return true
end)
`,
            },
        ];
        const notices = vi.fn();
        const runtime = await PortableCharacterRuntime.create({
            profile: noisyProfile,
            grant: await createPortableRuntimeGrant(
                noisyProfile,
                requiredPortableRuntimeCapabilities(noisyProfile),
            ),
            conversationId: 'conversation',
            branchId: 'branch',
            characterName: 'Character',
            characterDescription: 'Description',
            client: {} as LorepiaClient,
            primarySelection: () => PRIMARY_SELECTION,
            onChanged: vi.fn(),
            onNotice: notices,
            storage: memoryStorage(),
            workerFactory: inProcessWorkerFactory,
        });

        await expect(runtime.prepareInput('continue')).resolves.toEqual({
            text: 'continue',
            shouldSend: true,
        });
        expect(notices).toHaveBeenCalledTimes(16);
        runtime.close();
    });

    it('does not let a stale worker snapshot revert host-owned model selection', async () => {
        const delayedProfile = profile();
        const script = delayedProfile.runtime_scripts[0];
        if (script === undefined) throw new Error('runtime fixture script is missing');
        delayedProfile.runtime_scripts = [
            {
                ...script,
                source: String.raw`
listenEdit("editDisplay", async(function(triggerId, text)
    sleep(40):await()
    return text
end))
`,
            },
        ];
        const runtime = await PortableCharacterRuntime.create({
            profile: delayedProfile,
            grant: await createPortableRuntimeGrant(
                delayedProfile,
                requiredPortableRuntimeCapabilities(delayedProfile),
            ),
            conversationId: 'conversation',
            branchId: 'branch',
            characterName: 'Character',
            characterDescription: 'Description',
            client: {} as LorepiaClient,
            primarySelection: () => PRIMARY_SELECTION,
            onChanged: vi.fn(),
            onNotice: vi.fn(),
            storage: memoryStorage(),
            workerFactory: inProcessWorkerFactory,
        });
        runtime.setMessages([message('assistant', 'assistant', 'hello')]);
        runtime.setAuxiliarySelection(PRIMARY_SELECTION);

        const refresh = runtime.refreshDisplay();
        await new Promise((resolve) => globalThis.setTimeout(resolve, 5));
        runtime.setAuxiliarySelection(AUXILIARY_SELECTION);
        await refresh;

        expect(runtime.auxiliarySelection).toEqual(AUXILIARY_SELECTION);
        runtime.close();
    });

    it('terminates and recreates a worker after an aggregate event deadline', async () => {
        const yieldingProfile = profile();
        const script = yieldingProfile.runtime_scripts[0];
        if (script === undefined) throw new Error('runtime fixture script is missing');
        yieldingProfile.runtime_scripts = [
            {
                ...script,
                source: String.raw`
onStart = async(function(triggerId)
    if getState(triggerId, "recovery-marker") == nil then
        setState(triggerId, "recovery-marker", "persisted-before-timeout")
        for index = 1, 30 do
            sleep(10):await()
        end
    end
    return true
end)
`,
            },
        ];
        const runtime = await PortableCharacterRuntime.create({
            profile: yieldingProfile,
            grant: await createPortableRuntimeGrant(
                yieldingProfile,
                requiredPortableRuntimeCapabilities(yieldingProfile),
            ),
            conversationId: 'conversation',
            branchId: 'branch',
            characterName: 'Character',
            characterDescription: 'Description',
            client: {} as LorepiaClient,
            primarySelection: () => PRIMARY_SELECTION,
            onChanged: vi.fn(),
            onNotice: vi.fn(),
            storage: memoryStorage(),
            eventTimeoutMs: 100,
            workerFactory: inProcessWorkerFactory,
        });

        await expect(runtime.prepareInput('hello')).rejects.toThrow(/시간|timeout/i);
        await expect(runtime.prepareInput('again')).resolves.toEqual({
            text: 'again',
            shouldSend: true,
        });
        runtime.close();
    });

    it('serializes rapid operations so worker state cannot be overwritten by stale context', async () => {
        const sequentialProfile = profile();
        const script = sequentialProfile.runtime_scripts[0];
        if (script === undefined) throw new Error('runtime fixture script is missing');
        sequentialProfile.runtime_scripts = [
            {
                ...script,
                source: String.raw`
onButtonClick = async(function(triggerId)
    local count = tonumber(getState(triggerId, "count")) or 0
    sleep(10):await()
    setState(triggerId, "count", count + 1)
end)

onStart = async(function(triggerId)
    setChatVar(triggerId, "observed-count", tostring(getState(triggerId, "count")))
    return true
end)
`,
            },
        ];
        const runtime = await PortableCharacterRuntime.create({
            profile: sequentialProfile,
            grant: await createPortableRuntimeGrant(
                sequentialProfile,
                requiredPortableRuntimeCapabilities(sequentialProfile),
            ),
            conversationId: 'conversation',
            branchId: 'branch',
            characterName: 'Character',
            characterDescription: 'Description',
            client: {} as LorepiaClient,
            primarySelection: () => PRIMARY_SELECTION,
            onChanged: vi.fn(),
            onNotice: vi.fn(),
            storage: memoryStorage(),
            workerFactory: inProcessWorkerFactory,
        });

        await Promise.all([runtime.handleAction('first'), runtime.handleAction('second')]);
        await runtime.prepareInput('continue');
        expect(runtime.variables['observed-count']).toBe('2');
        runtime.close();
    });

    it('persists the final bounded worker state when a Lua request fails', async () => {
        const failingProfile = profile();
        const script = failingProfile.runtime_scripts[0];
        if (script === undefined) throw new Error('runtime fixture script is missing');
        failingProfile.runtime_scripts = [
            {
                ...script,
                source: String.raw`
onButtonClick = async(function(triggerId)
    setState(triggerId, "first", "written")
    setState(triggerId, "last", "preserved")
    error("expected failure")
end)

onStart = async(function(triggerId)
    setChatVar(triggerId, "observed-last", getState(triggerId, "last"))
    return true
end)
`,
            },
        ];
        const runtime = await PortableCharacterRuntime.create({
            profile: failingProfile,
            grant: await createPortableRuntimeGrant(
                failingProfile,
                requiredPortableRuntimeCapabilities(failingProfile),
            ),
            conversationId: 'conversation',
            branchId: 'branch',
            characterName: 'Character',
            characterDescription: 'Description',
            client: {} as LorepiaClient,
            primarySelection: () => PRIMARY_SELECTION,
            onChanged: vi.fn(),
            onNotice: vi.fn(),
            storage: memoryStorage(),
            workerFactory: inProcessWorkerFactory,
        });

        await expect(runtime.handleAction('fail')).rejects.toThrow(/expected failure/);
        await runtime.prepareInput('continue');
        expect(runtime.variables['observed-last']).toBe('preserved');
        runtime.close();
    });

    it('keeps worker state applied in memory while host mutations report browser durability', async () => {
        const storage = memoryStorage();
        const write = storage.setItem.bind(storage);
        let rejectWrites = true;
        storage.setItem = (key, value) => {
            if (rejectWrites) throw new DOMException('quota exceeded', 'QuotaExceededError');
            write(key, value);
        };
        const statuses: PortableRuntimePersistenceStatus[] = [];
        const stateProfile = profile();
        const stateScript = stateProfile.runtime_scripts[0];
        if (stateScript === undefined) throw new Error('runtime fixture script is missing');
        stateProfile.runtime_capabilities_declared = true;
        stateProfile.required_runtime_capabilities = ['runtime:callbacks', 'state:readwrite'];
        stateProfile.runtime_scripts = [
            {
                ...stateScript,
                source: String.raw`
onButtonClick = async(function(triggerId)
    assert(setState(triggerId, "memory-only", "kept") == true)
    assert(getState(triggerId, "memory-only") == "kept")
end)
`,
            },
        ];
        const runtime = await PortableCharacterRuntime.create({
            profile: stateProfile,
            grant: await createPortableRuntimeGrant(stateProfile, [
                'runtime:callbacks',
                'state:readwrite',
            ]),
            conversationId: 'conversation',
            branchId: 'branch',
            characterName: 'Character',
            characterDescription: 'Description',
            client: {} as LorepiaClient,
            primarySelection: () => PRIMARY_SELECTION,
            onChanged: vi.fn(),
            onNotice: vi.fn(),
            onPersistenceStatus: (status) => statuses.push(status),
            storage,
            workerFactory: inProcessWorkerFactory,
        });

        await expect(runtime.handleAction('memory-only')).resolves.toBeUndefined();
        await expect(runtime.setOption('music', '1')).resolves.toEqual({
            applied: true,
            durable: false,
        });
        expect(runtime.setAuxiliarySelection(AUXILIARY_SELECTION)).toEqual({
            applied: true,
            durable: false,
        });
        expect(runtime.optionValue('music')).toBe('1');
        expect(statuses).toContainEqual({ mode: 'memory-only', reason: 'write-failed' });

        rejectWrites = false;
        await expect(runtime.setOption('music', '0')).resolves.toEqual({
            applied: true,
            durable: true,
        });
        expect(runtime.optionValue('music')).toBe('0');
        expect(statuses.at(-1)).toEqual({ mode: 'persistent', backend: 'local-storage' });
        runtime.close();
    });

    it('prefers an existing SQLite record over the exact legacy browser key', async () => {
        const storage = memoryStorage();
        const scope: PortableRuntimeStateScopeInput = {
            character_id: 'character',
            character_content_revision_id: 'revision',
            conversation_id: 'conversation',
            branch_id: 'branch',
        };
        const legacyState = persistedState({ music: 'legacy' });
        const sqliteState = persistedState({ music: 'sqlite' });
        const legacyKey = 'lorepia.character-runtime.v1:character:revision:conversation:branch';
        storage.setItem(legacyKey, JSON.stringify(legacyState));
        const putPortableRuntimeState = vi.fn();
        const client = {
            getPortableRuntimeState: vi
                .fn()
                .mockResolvedValue({ scope_epoch: 0, record: sqliteRecord(scope, sqliteState, 7) }),
            putPortableRuntimeState,
        } as unknown as LorepiaClient;
        const stateProfile = profile();
        const stateScript = stateProfile.runtime_scripts[0];
        if (stateScript === undefined) throw new Error('runtime fixture script is missing');
        stateProfile.runtime_scripts = [{ ...stateScript, source: 'return' }];

        const runtime = await PortableCharacterRuntime.create({
            profile: stateProfile,
            grant: await createPortableRuntimeGrant(stateProfile, ['runtime:callbacks']),
            conversationId: scope.conversation_id,
            branchId: scope.branch_id,
            characterName: 'Character',
            characterDescription: 'Description',
            client,
            primarySelection: () => PRIMARY_SELECTION,
            onChanged: vi.fn(),
            onNotice: vi.fn(),
            storage,
            workerFactory: inProcessWorkerFactory,
        });

        expect(runtime.optionValue('music')).toBe('sqlite');
        expect(putPortableRuntimeState).not.toHaveBeenCalled();
        expect(storage.getItem(legacyKey)).not.toBeNull();
        runtime.close();
    });

    it('refuses ambiguous v1 legacy keys when scope identifiers contain colons', async () => {
        const storage = memoryStorage();
        const scope: PortableRuntimeStateScopeInput = {
            character_id: 'character:one',
            character_content_revision_id: 'revision:two',
            conversation_id: 'conversation:three',
            branch_id: 'branch:four',
        };
        const legacyKey = [
            'lorepia.character-runtime.v1',
            scope.character_id,
            scope.character_content_revision_id,
            scope.conversation_id,
            scope.branch_id,
        ].join(':');
        const adjacentKey = `${legacyKey}:adjacent`;
        const legacyState = persistedState({ music: 'legacy-colon' });
        storage.setItem(legacyKey, JSON.stringify(legacyState));
        storage.setItem(adjacentKey, 'must-remain');
        const getPortableRuntimeState = vi.fn(() =>
            Promise.resolve({ scope_epoch: 0, record: null }),
        );
        const putPortableRuntimeState = vi.fn();
        const client = {
            getPortableRuntimeState,
            putPortableRuntimeState,
        } as unknown as LorepiaClient;
        const stateProfile = profile();
        stateProfile.character_id = scope.character_id;
        stateProfile.character_content_revision_id = scope.character_content_revision_id;
        const stateScript = stateProfile.runtime_scripts[0];
        if (stateScript === undefined) throw new Error('runtime fixture script is missing');
        stateProfile.runtime_scripts = [{ ...stateScript, source: 'return' }];

        const runtime = await PortableCharacterRuntime.create({
            profile: stateProfile,
            grant: await createPortableRuntimeGrant(stateProfile, ['runtime:callbacks']),
            conversationId: scope.conversation_id,
            branchId: scope.branch_id,
            characterName: 'Character',
            characterDescription: 'Description',
            client,
            primarySelection: () => PRIMARY_SELECTION,
            onChanged: vi.fn(),
            onNotice: vi.fn(),
            storage,
            workerFactory: inProcessWorkerFactory,
        });

        expect(runtime.optionValue('music')).toBe('0');
        expect(getPortableRuntimeState).toHaveBeenCalledOnce();
        expect(putPortableRuntimeState).not.toHaveBeenCalled();
        expect(storage.getItem(legacyKey)).not.toBeNull();
        expect(storage.getItem(adjacentKey)).toBe('must-remain');
        runtime.close();
    });

    it('keeps legacy state in memory and never overwrites a conflicting SQLite record', async () => {
        const storage = memoryStorage();
        const scope: PortableRuntimeStateScopeInput = {
            character_id: 'character',
            character_content_revision_id: 'revision',
            conversation_id: 'conversation',
            branch_id: 'branch',
        };
        const legacyKey = 'lorepia.character-runtime.v1:character:revision:conversation:branch';
        const legacyState = persistedState({ music: 'legacy' });
        const currentState = persistedState({ music: 'other-runtime' });
        storage.setItem(legacyKey, JSON.stringify(legacyState));
        const statuses: PortableRuntimePersistenceStatus[] = [];
        const putPortableRuntimeState = vi.fn().mockResolvedValue({
            status: 'revision_conflict',
            current: sqliteRecord(scope, currentState, 3),
        });
        const client = {
            getPortableRuntimeState: vi
                .fn()
                .mockResolvedValueOnce({ scope_epoch: 0, record: null }),
            putPortableRuntimeState,
        } as unknown as LorepiaClient;
        const stateProfile = profile();
        const stateScript = stateProfile.runtime_scripts[0];
        if (stateScript === undefined) throw new Error('runtime fixture script is missing');
        stateProfile.runtime_scripts = [{ ...stateScript, source: 'return' }];

        const runtime = await PortableCharacterRuntime.create({
            profile: stateProfile,
            grant: await createPortableRuntimeGrant(stateProfile, ['runtime:callbacks']),
            conversationId: scope.conversation_id,
            branchId: scope.branch_id,
            characterName: 'Character',
            characterDescription: 'Description',
            client,
            primarySelection: () => PRIMARY_SELECTION,
            onChanged: vi.fn(),
            onNotice: vi.fn(),
            onPersistenceStatus: (status) => statuses.push(status),
            storage,
            workerFactory: inProcessWorkerFactory,
        });

        expect(runtime.optionValue('music')).toBe('legacy');
        await expect(runtime.setOption('music', '0')).resolves.toEqual({
            applied: true,
            durable: false,
        });
        expect(runtime.optionValue('music')).toBe('0');
        expect(putPortableRuntimeState).toHaveBeenCalledTimes(1);
        expect(statuses.at(-1)).toEqual({ mode: 'memory-only', reason: 'conflict' });
        expect(storage.getItem(legacyKey)).not.toBeNull();
        runtime.close();
    });

    it('keeps accepted state when SQLite reads and writes fail and reports both failures', async () => {
        const statuses: PortableRuntimePersistenceStatus[] = [];
        const putPortableRuntimeState = vi.fn().mockRejectedValue(new Error('database busy'));
        const client = {
            getPortableRuntimeState: vi.fn().mockRejectedValue(new Error('database unavailable')),
            putPortableRuntimeState,
        } as unknown as LorepiaClient;
        const stateProfile = profile();
        const stateScript = stateProfile.runtime_scripts[0];
        if (stateScript === undefined) throw new Error('runtime fixture script is missing');
        stateProfile.runtime_scripts = [{ ...stateScript, source: 'return' }];
        const runtime = await PortableCharacterRuntime.create({
            profile: stateProfile,
            grant: await createPortableRuntimeGrant(stateProfile, ['runtime:callbacks']),
            conversationId: 'conversation',
            branchId: 'branch',
            characterName: 'Character',
            characterDescription: 'Description',
            client,
            primarySelection: () => PRIMARY_SELECTION,
            onChanged: vi.fn(),
            onNotice: vi.fn(),
            onPersistenceStatus: (status) => statuses.push(status),
            storage: memoryStorage(),
            workerFactory: inProcessWorkerFactory,
        });

        expect(statuses).toContainEqual({ mode: 'memory-only', reason: 'read-failed' });
        await expect(runtime.setOption('music', '1')).resolves.toEqual({
            applied: true,
            durable: false,
        });
        expect(runtime.optionValue('music')).toBe('1');
        expect(putPortableRuntimeState).toHaveBeenCalledTimes(1);
        expect(statuses.at(-1)).toEqual({ mode: 'memory-only', reason: 'write-failed' });
        runtime.close();
    });

    it('reports a host mutation as durable only after its SQLite write succeeds', async () => {
        const scope: PortableRuntimeStateScopeInput = {
            character_id: 'character',
            character_content_revision_id: 'revision',
            conversation_id: 'conversation',
            branch_id: 'branch',
        };
        let revision = 0;
        const putPortableRuntimeState = vi.fn((input: PutPortableRuntimeStateInput) => {
            revision += 1;
            return Promise.resolve({
                status: 'saved' as const,
                record: sqliteRecord(scope, input.payload.value, revision),
                evicted_rows: 0,
                evicted_bytes: 0,
            });
        });
        const stateProfile = profile();
        const stateScript = stateProfile.runtime_scripts[0];
        if (stateScript === undefined) throw new Error('runtime fixture script is missing');
        stateProfile.runtime_scripts = [{ ...stateScript, source: 'return' }];
        const runtime = await PortableCharacterRuntime.create({
            profile: stateProfile,
            grant: await createPortableRuntimeGrant(stateProfile, ['runtime:callbacks']),
            conversationId: scope.conversation_id,
            branchId: scope.branch_id,
            characterName: 'Character',
            characterDescription: 'Description',
            client: {
                getPortableRuntimeState: vi
                    .fn()
                    .mockResolvedValue({ scope_epoch: 0, record: null }),
                putPortableRuntimeState,
            } as unknown as LorepiaClient,
            primarySelection: () => PRIMARY_SELECTION,
            onChanged: vi.fn(),
            onNotice: vi.fn(),
            storage: memoryStorage(),
            workerFactory: inProcessWorkerFactory,
        });

        await expect(runtime.setOption('music', '1')).resolves.toEqual({
            applied: true,
            durable: true,
        });
        expect(putPortableRuntimeState).toHaveBeenCalledTimes(1);
        runtime.close();
    });

    it('serializes SQLite writes and coalesces pending state to the latest value', async () => {
        const scope: PortableRuntimeStateScopeInput = {
            character_id: 'character',
            character_content_revision_id: 'revision',
            conversation_id: 'conversation',
            branch_id: 'branch',
        };
        let releaseFirstWrite: ((value: unknown) => void) | undefined;
        const firstWrite = new Promise((resolve) => {
            releaseFirstWrite = resolve;
        });
        const putPortableRuntimeState = vi
            .fn()
            .mockImplementationOnce(() => firstWrite)
            .mockImplementationOnce((input: PutPortableRuntimeStateInput) =>
                Promise.resolve({
                    status: 'saved',
                    record: sqliteRecord(scope, input.payload.value, 2),
                    evicted_rows: 0,
                    evicted_bytes: 0,
                }),
            );
        const client = {
            getPortableRuntimeState: vi.fn().mockResolvedValue({ scope_epoch: 0, record: null }),
            putPortableRuntimeState,
        } as unknown as LorepiaClient;
        const stateProfile = profile();
        const stateScript = stateProfile.runtime_scripts[0];
        if (stateScript === undefined) throw new Error('runtime fixture script is missing');
        stateProfile.runtime_scripts = [{ ...stateScript, source: 'return' }];
        const runtime = await PortableCharacterRuntime.create({
            profile: stateProfile,
            grant: await createPortableRuntimeGrant(stateProfile, ['runtime:callbacks']),
            conversationId: scope.conversation_id,
            branchId: scope.branch_id,
            characterName: 'Character',
            characterDescription: 'Description',
            client,
            primarySelection: () => PRIMARY_SELECTION,
            onChanged: vi.fn(),
            onNotice: vi.fn(),
            storage: memoryStorage(),
            workerFactory: inProcessWorkerFactory,
        });

        expect(runtime.setAuxiliarySelection(PRIMARY_SELECTION)).toEqual({
            applied: true,
            durable: false,
        });
        await vi.waitFor(() => expect(putPortableRuntimeState).toHaveBeenCalledTimes(1));
        runtime.setAuxiliarySelection(AUXILIARY_SELECTION);
        runtime.setAuxiliarySelection(null);
        const firstInput = putPortableRuntimeState.mock
            .calls[0]?.[0] as PutPortableRuntimeStateInput;
        releaseFirstWrite?.({
            status: 'saved',
            record: sqliteRecord(scope, firstInput.payload.value, 1),
            evicted_rows: 0,
            evicted_bytes: 0,
        });
        await vi.waitFor(() => expect(putPortableRuntimeState).toHaveBeenCalledTimes(2));

        const secondInput = putPortableRuntimeState.mock
            .calls[1]?.[0] as PutPortableRuntimeStateInput;
        expect(firstInput.expected_revision).toBeNull();
        expect(secondInput.expected_revision).toBe(1);
        expect(secondInput.payload.value.auxiliarySelection).toBeNull();
        runtime.close();
    });

    it('rejects oversized and cumulatively excessive host state atomically', async () => {
        const boundedProfile = profile();
        const script = boundedProfile.runtime_scripts[0];
        if (script === undefined) throw new Error('runtime fixture script is missing');
        boundedProfile.runtime_scripts = [
            {
                ...script,
                source: String.raw`
nextStateIndex = 1
onStart = async(function(triggerId)
    assert(setState(triggerId, "oversized", string.rep("x", 70000)) == false)
    assert(setChat(triggerId, -1, string.rep("x", 300000)) == false)
    local allAccepted = true
    for offset = 1, 5 do
        if not setState(triggerId, "state-" .. tostring(nextStateIndex), string.rep("x", 60000)) then
            allAccepted = false
        end
        nextStateIndex = nextStateIndex + 1
    end
    return allAccepted
end)
`,
            },
        ];
        const storage = memoryStorage();
        const runtime = await PortableCharacterRuntime.create({
            profile: boundedProfile,
            grant: await createPortableRuntimeGrant(
                boundedProfile,
                requiredPortableRuntimeCapabilities(boundedProfile),
            ),
            conversationId: 'conversation',
            branchId: 'branch',
            characterName: 'Character',
            characterDescription: 'Description',
            client: {} as LorepiaClient,
            primarySelection: () => PRIMARY_SELECTION,
            onChanged: vi.fn(),
            onNotice: vi.fn(),
            storage,
            workerFactory: inProcessWorkerFactory,
        });
        runtime.setMessages([message('assistant', 'assistant', 'hello')]);

        const outcomes = [];
        for (let index = 0; index < 16; index += 1) {
            outcomes.push(await runtime.prepareInput('next'));
        }
        expect(outcomes.some((outcome) => !outcome.shouldSend)).toBe(true);
        const persisted = storage.getItem(
            runtimeStorageKey({
                character_id: 'character',
                character_content_revision_id: 'revision',
                conversation_id: 'conversation',
                branch_id: 'branch',
            }),
        );
        expect(persisted).not.toBeNull();
        expect(persisted?.length ?? Number.POSITIVE_INFINITY).toBeLessThanOrEqual(4 * 1024 * 1024);
        runtime.close();
    }, 15_000);
});

function memoryStorage(): Storage {
    const values = new Map<string, string>();
    return {
        get length() {
            return values.size;
        },
        clear: () => values.clear(),
        getItem: (key) => values.get(key) ?? null,
        key: (index) => [...values.keys()][index] ?? null,
        removeItem: (key) => values.delete(key),
        setItem: (key, value) => values.set(key, value),
    };
}

function runtimeStorageKey(scope: PortableRuntimeStateScopeInput): string {
    return `lorepia.character-runtime.v2:${encodeURIComponent(
        JSON.stringify([
            scope.character_id,
            scope.character_content_revision_id,
            scope.conversation_id,
            scope.branch_id,
        ]),
    )}`;
}

function persistedState(options: Record<string, string>): PortableRuntimePersistedState {
    return {
        options,
        chatVars: {},
        state: {},
        messageOverrides: {},
        background: '',
        auxiliarySelection: null,
    };
}

function sqliteRecord(
    scope: PortableRuntimeStateScopeInput,
    value: PortableRuntimePersistedState,
    revision: number,
): PortableRuntimeStateRecordDto {
    return {
        scope,
        scope_epoch: 0,
        revision,
        payload: { schema_version: 1, value },
        created_at: '2026-08-29T00:00:00Z',
        updated_at: '2026-08-29T00:00:00Z',
    };
}
