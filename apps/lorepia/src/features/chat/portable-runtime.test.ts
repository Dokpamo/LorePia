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
import { PortableCharacterRuntime, parsePortableRuntimeToggles } from './portable-runtime';

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
        const runtime = await PortableCharacterRuntime.create({
            profile: profile(),
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
        expect(notices).not.toHaveBeenCalled();
        expect(changed).toHaveBeenCalled();

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
