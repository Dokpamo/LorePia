import { cleanup, render, screen, waitFor } from '@testing-library/svelte';
import { afterEach, expect, it, vi } from 'vitest';
import { LorepiaAppController } from '../../../app/app-controller';
import type { LorepiaClient } from '../../../lib/ipc/contracts';
import type { PersonaClientApi } from '../../personas/persona-contracts';
import ChatPane from '../ChatPane.svelte';
import { chatReadyState } from './chat-pane-state-builder';

afterEach(() => {
    cleanup();
    vi.restoreAllMocks();
});

type Selection = Awaited<ReturnType<PersonaClientApi['getConversationPersonaSelection']>>;
function selection(name: string): Selection {
    return { selected_persona: { value: { name, description: '' } } } as Selection;
}
function profile() {
    return {
        character_id: 'character-1',
        character_content_revision_id: 'revision-1',
        assets: [],
        background_markup: '',
        toggle_schema: '',
        initial_variables: {},
        output_transforms: [],
        display_transforms: [],
        runtime_scripts: [],
        required_runtime_capabilities: [],
        runtime_capabilities_declared: false,
        runtime_knowledge: [],
        runtime_script_count: 0,
    };
}

it('expands the selected persona in stored and streaming text without a runtime worker', async () => {
    const appState = chatReadyState();
    appState.messages.items = [
        {
            id: 'greeting',
            conversation_id: 'conversation-1',
            parent_id: null,
            role: 'assistant',
            content: 'Hello, {{user}}.',
            status: 'complete',
            generation_id: null,
            created_at: '2026-09-10T00:00:00Z',
        },
    ];
    appState.chat.streaming_text = 'Welcome back, {{user}}.';
    const getConversationPersonaSelection = vi.fn().mockResolvedValue(selection('Alex'));
    const client = {
        getConversationPersonaSelection,
        getCharacterRenderProfile: vi.fn().mockResolvedValue(profile()),
    } as unknown as LorepiaClient;
    const controller = new LorepiaAppController(client);
    render(ChatPane, { appState, controller, client });
    expect(await screen.findByText('Hello, Alex.')).toBeInTheDocument();
    expect(await screen.findByText('Welcome back, Alex.')).toBeInTheDocument();
    expect(getConversationPersonaSelection).toHaveBeenCalledExactlyOnceWith({
        conversation_id: 'conversation-1',
    });
    controller.destroy();
});

it('ignores a delayed persona from the conversation that was left', async () => {
    const appState = chatReadyState();
    appState.chat.streaming_text = 'Hello, {{user}}.';
    let finishOld!: (value: Selection) => void;
    const getConversationPersonaSelection = vi
        .fn()
        .mockReturnValueOnce(
            new Promise<Selection>((resolve) => {
                finishOld = resolve;
            }),
        )
        .mockResolvedValueOnce(selection('Blair'));
    const client = {
        getConversationPersonaSelection,
        getCharacterRenderProfile: vi.fn().mockResolvedValue(profile()),
    } as unknown as LorepiaClient;
    const controller = new LorepiaAppController(client);
    const view = render(ChatPane, { appState, controller, client });
    await waitFor(() => expect(getConversationPersonaSelection).toHaveBeenCalledOnce());
    const next = structuredClone(appState);
    if (next.selected_conversation === null || next.conversation_state === null)
        throw new Error('Conversation fixture is incomplete');
    next.selected_conversation.id = 'conversation-2';
    next.conversation_state.conversation_id = 'conversation-2';
    next.conversation_state.active_branch_id = 'branch-2';
    await view.rerender({ appState: next, controller, client });
    expect(await screen.findByText('Hello, Blair.')).toBeInTheDocument();
    finishOld(selection('Alex'));
    await waitFor(() => {
        expect(screen.getByText('Hello, Blair.')).toBeInTheDocument();
        expect(screen.queryByText('Hello, Alex.')).not.toBeInTheDocument();
    });
    controller.destroy();
});
