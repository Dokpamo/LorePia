import { LuaFactory } from 'wasmoon';

import type {
    CharacterRenderProfileDto,
    GenerationSelectionInput,
    MessageDto,
} from '../../../lib/ipc/contracts';
import { PortableRuntimeKernel } from '../portable-runtime-kernel';
import type {
    PortableRuntimeMainMessage,
    PortableRuntimeWorkerMessage,
} from '../portable-runtime-protocol';
import type { PortableRuntimeWorkerEndpoint } from '../portable-runtime-worker-client';

export const PRIMARY_SELECTION: GenerationSelectionInput = {
    kind: 'legacy_profile',
    provider_profile_id: 'primary',
};

const TEST_WASM_PATH = decodeURIComponent(
    new URL('../../../../node_modules/wasmoon/dist/glue.wasm', import.meta.url).pathname,
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

export function profile(): CharacterRenderProfileDto {
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

export function message(id: string, role: MessageDto['role'], content: string): MessageDto {
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

export function inProcessWorkerFactory(): PortableRuntimeWorkerEndpoint {
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
        postMessage: (workerMessage: PortableRuntimeWorkerMessage) => {
            queueMicrotask(() =>
                dispatch('message', new MessageEvent('message', { data: workerMessage })),
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
        postMessage: (workerMessage: unknown) => {
            queueMicrotask(() => {
                if (!terminated) kernel.receive(workerMessage as PortableRuntimeMainMessage);
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

export function memoryStorage(): Storage {
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
