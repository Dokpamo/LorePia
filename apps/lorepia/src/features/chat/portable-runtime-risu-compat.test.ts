// @vitest-environment node

import { describe, expect, it, vi } from 'vitest';

import type { LorepiaClient } from '../../lib/ipc/contracts';
import { PortableCharacterRuntime, createPortableRuntimeGrant } from './portable-runtime';
import {
    PRIMARY_SELECTION,
    inProcessWorkerFactory,
    memoryStorage,
    message,
    profile,
} from './tests/portable-runtime-fixtures';

describe('portable runtime Risu compatibility', () => {
    it('supports onInput, editRequest, dynamic card buttons, and addChat', async () => {
        const compatibilityProfile = profile();
        const baseScript = compatibilityProfile.runtime_scripts[0];
        if (baseScript === undefined) throw new Error('runtime fixture script is missing');
        compatibilityProfile.required_runtime_capabilities = [
            'runtime:callbacks',
            'chat:read',
            'chat:write',
            'state:readwrite',
        ];
        compatibilityProfile.runtime_scripts = [
            {
                ...baseScript,
                source: String.raw`
listenEdit("editRequest", function(triggerId, data)
    assert(type(data) == "table")
    setState(triggerId, "request-message-count", #data)
    data[#data].content = data[#data].content .. "|request"
    return data
end)

function onStart(triggerId)
    setChatVar(triggerId, "start-ran", "1")
end

function onInput(triggerId)
    setChatVar(triggerId, "input-ran", "1")
end

setmetatable(_G, {
    __index = function(_, key)
        if key == "AOSHEX_6869" then
            return function(triggerId)
                addChat(triggerId, "user", "button submission")
            end
        end
        return nil
    end
})
`,
            },
        ];
        const runtime = await PortableCharacterRuntime.create({
            profile: compatibilityProfile,
            grant: await createPortableRuntimeGrant(
                compatibilityProfile,
                compatibilityProfile.required_runtime_capabilities,
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
        runtime.setMessages([message('user', 'user', 'prior')]);

        await expect(runtime.prepareInput('next')).resolves.toEqual({
            text: 'next|request',
            shouldSend: true,
        });
        expect(runtime.variables['start-ran']).toBe('1');
        expect(runtime.variables['input-ran']).toBe('1');
        await expect(runtime.handleAction('AOSHEX_6869')).resolves.toBe('button submission');

        runtime.close();
    });
});
