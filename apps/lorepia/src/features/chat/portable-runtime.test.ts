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
import {
    PortableCharacterRuntime,
    createPortableRuntimeGrant,
    parsePortableRuntimeToggles,
    requiredPortableRuntimeCapabilities,
    validatePortableRuntimeGrant,
} from './portable-runtime';

const PRIMARY_SELECTION: GenerationSelectionInput = {
    kind: 'legacy_profile',
    provider_profile_id: 'primary',
};
const AUXILIARY_SELECTION: GenerationSelectionInput = {
    kind: 'legacy_profile',
    provider_profile_id: 'auxiliary',
};

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
});

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
                    result:
                        input.selection.kind === 'legacy_profile' &&
                        input.selection.provider_profile_id === 'auxiliary'
                            ? 'AUXILIARY_RESULT'
                            : 'PRIMARY_RESULT',
                }),
        );
        const client = { generateRuntimeText } as unknown as LorepiaClient;
        const notices = vi.fn();
        const changed = vi.fn();
        const activeProfile = profile();
        const runtime = await PortableCharacterRuntime.create({
            profile: activeProfile,
            grant: await createPortableRuntimeGrant(activeProfile),
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
            luaFactory: new LuaFactory(
                decodeURIComponent(
                    new URL('../../../node_modules/wasmoon/dist/glue.wasm', import.meta.url)
                        .pathname,
                ),
            ),
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
        expect(generateRuntimeText).toHaveBeenNthCalledWith(1, {
            selection: PRIMARY_SELECTION,
            messages: [{ role: 'user', content: 'main prompt' }],
        });

        await runtime.afterOutput(messages);
        const assistant = messages[1];
        if (assistant === undefined) throw new Error('assistant fixture is missing');
        expect(runtime.displayText(assistant)).toContain('display-1');

        await runtime.handleAction('generate__feature__1');
        expect(runtime.effectiveText(assistant)).toBe(
            'world|AUXILIARY_RESULT|generate__feature__1',
        );
        expect(generateRuntimeText).toHaveBeenNthCalledWith(2, {
            selection: AUXILIARY_SELECTION,
            messages: [{ role: 'user', content: 'aux prompt' }],
        });
        for (let index = 0; index < 10; index += 1) {
            await runtime.handleAction(`budget-${String(index)}`);
        }
        expect(generateRuntimeText).toHaveBeenCalledTimes(8);
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
        const createEngine = vi.fn();
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
            luaFactory: { createEngine } as unknown as LuaFactory,
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
        expect(createEngine).not.toHaveBeenCalled();
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
                    source: 'assert(getFullChat == nil)\nassert(setChat == nil)\nassert(LLM == nil)',
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
            luaFactory: new LuaFactory(
                decodeURIComponent(
                    new URL('../../../node_modules/wasmoon/dist/glue.wasm', import.meta.url)
                        .pathname,
                ),
            ),
        });

        expect(requiredPortableRuntimeCapabilities(restrictedProfile)).toContain('model:generate');
        await expect(
            validatePortableRuntimeGrant(restrictedProfile, {
                ...grant,
                capabilities: [...grant.capabilities, 'chat:read'],
            }),
        ).resolves.toBe(false);
        runtime.close();
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
`,
            },
        ];

        const runtime = await PortableCharacterRuntime.create({
            profile: hardenedProfile,
            grant: await createPortableRuntimeGrant(hardenedProfile),
            conversationId: 'conversation',
            branchId: 'branch',
            characterName: 'Character',
            characterDescription: 'Description',
            client: {} as LorepiaClient,
            primarySelection: () => PRIMARY_SELECTION,
            onChanged: vi.fn(),
            onNotice: vi.fn(),
            storage: memoryStorage(),
            luaFactory: new LuaFactory(
                decodeURIComponent(
                    new URL('../../../node_modules/wasmoon/dist/glue.wasm', import.meta.url)
                        .pathname,
                ),
            ),
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
                luaFactory: new LuaFactory(
                    decodeURIComponent(
                        new URL('../../../node_modules/wasmoon/dist/glue.wasm', import.meta.url)
                            .pathname,
                    ),
                ),
            }),
        ).rejects.toThrow(/timeout/i);
        expect(Date.now() - startedAt).toBeLessThan(2_000);
    });

    it('closes a runtime when yielding callbacks exceed the aggregate event deadline', async () => {
        const yieldingProfile = profile();
        const script = yieldingProfile.runtime_scripts[0];
        if (script === undefined) throw new Error('runtime fixture script is missing');
        yieldingProfile.runtime_scripts = [
            {
                ...script,
                source: String.raw`
onStart = async(function(triggerId)
    for index = 1, 30 do
        sleep(10):await()
    end
    return true
end)
`,
            },
        ];
        const runtime = await PortableCharacterRuntime.create({
            profile: yieldingProfile,
            grant: await createPortableRuntimeGrant(yieldingProfile),
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
            luaFactory: new LuaFactory(
                decodeURIComponent(
                    new URL('../../../node_modules/wasmoon/dist/glue.wasm', import.meta.url)
                        .pathname,
                ),
            ),
        });

        await expect(runtime.prepareInput('hello')).rejects.toThrow(/시간|timeout/i);
        await expect(runtime.prepareInput('again')).rejects.toThrow(/준비|ready/i);
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
            grant: await createPortableRuntimeGrant(boundedProfile),
            conversationId: 'conversation',
            branchId: 'branch',
            characterName: 'Character',
            characterDescription: 'Description',
            client: {} as LorepiaClient,
            primarySelection: () => PRIMARY_SELECTION,
            onChanged: vi.fn(),
            onNotice: vi.fn(),
            storage,
            luaFactory: new LuaFactory(
                decodeURIComponent(
                    new URL('../../../node_modules/wasmoon/dist/glue.wasm', import.meta.url)
                        .pathname,
                ),
            ),
        });
        runtime.setMessages([message('assistant', 'assistant', 'hello')]);

        const outcomes = [];
        for (let index = 0; index < 20; index += 1) {
            outcomes.push(await runtime.prepareInput('next'));
        }
        expect(outcomes.some((outcome) => !outcome.shouldSend)).toBe(true);
        const persisted = storage.getItem(
            'lorepia.character-runtime.v1:character:revision:conversation:branch',
        );
        expect(persisted).not.toBeNull();
        expect(persisted?.length ?? Number.POSITIVE_INFINITY).toBeLessThanOrEqual(4 * 1024 * 1024);
        runtime.close();
    });
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
