// @vitest-environment node
import { expect, it, vi } from 'vitest';
import type { LorepiaClient } from '../../lib/ipc/contracts';
import { PortableCharacterRuntime, createPortableRuntimeGrant } from './portable-runtime';
import {
    inProcessWorkerFactory,
    memoryStorage,
    message,
    profile,
} from './tests/portable-runtime-fixtures';

it('preserves absolute chat access, display indices and overrides across partial history eviction', async () => {
    const activeProfile = profile();
    const base = activeProfile.runtime_scripts[0];
    if (base === undefined) throw new Error('missing script fixture');
    activeProfile.required_runtime_capabilities = [
        'runtime:callbacks',
        'chat:read',
        'chat:write',
        'ui:write',
    ];
    activeProfile.runtime_scripts = [
        {
            ...base,
            source: String.raw`
listenEdit("editDisplay", function(triggerId, text, meta)
    return tostring(meta.index) .. ":" .. cbs("{{chat_index}}/{{lastmessageid}}") .. ":" .. text
end)
function onInput(triggerId)
    assert(getChatLength() == 201)
    assert(getChat(triggerId, 200).data == "draft")
    assert(getChat(triggerId, -1).data == "draft")
    assert(cbs("{{chat_index}}/{{lastmessageid}}") == "200/200")
    assert(removeChat(triggerId, 199) == false)
    assert(removeChat(triggerId, 200) == true)
end
function rewrite(triggerId)
    assert(getChatLength() == 200)
    assert(getChat(triggerId, 198).data == "first")
    assert(getChat(triggerId, 199).data == "last")
    assert(getChat(triggerId, -1).data == "last")
    assert(getChat(triggerId, 0) == nil)
    assert(setChat(triggerId, 0, "wrong") == false)
    assert(setChat(triggerId, -1, "retained override") == true)
end
`,
        },
    ];
    const runtime = await PortableCharacterRuntime.create({
        profile: activeProfile,
        grant: await createPortableRuntimeGrant(
            activeProfile,
            activeProfile.required_runtime_capabilities,
        ),
        conversationId: 'conversation',
        branchId: 'branch',
        characterName: 'Character',
        characterDescription: '',
        client: {} as LorepiaClient,
        primarySelection: () => null,
        onChanged: vi.fn(),
        onNotice: vi.fn(),
        storage: memoryStorage(),
        workerFactory: inProcessWorkerFactory,
    });
    try {
        const first = message('first', 'user', 'first');
        const last = message('last', 'assistant', 'last');
        runtime.setMessages([first, last], {
            start_index: 198,
            total_messages: 200,
            head_message_id: last.id,
        });
        await runtime.refreshDisplay();
        expect(runtime.displayText(first)).toBe('198:199/199:first');
        expect(runtime.displayText(last)).toBe('199:199/199:last');
        await runtime.handleAction('rewrite');
        expect(runtime.effectiveText(last)).toBe('retained override');
        await runtime.prepareInput('draft');
        const next = message('next', 'assistant', 'next');
        await runtime.afterOutput([next], {
            start_index: 200,
            total_messages: 201,
            head_message_id: next.id,
        });
        expect(runtime.effectiveText(last)).toBe('retained override');
        expect(runtime.displayText(next)).toBe('200:200/200:next');
        runtime.setMessages([next]);
        expect(runtime.effectiveText(last)).toBe('last');
    } finally {
        runtime.close();
    }
});
