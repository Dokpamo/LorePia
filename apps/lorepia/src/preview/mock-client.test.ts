import { get } from 'svelte/store';
import { cleanup, fireEvent, render, screen, waitFor } from '@testing-library/svelte';
import { afterEach, describe, expect, it } from 'vitest';

import App from '../app/App.svelte';
import { LorepiaAppController } from '../app/app-controller';
import { OrchestrationController } from '../features/orchestration/orchestration-controller';
import { PersonaController } from '../features/personas/persona-controller';
import { DEMO_INITIAL_CHARACTER_ID, DEMO_INITIAL_CONVERSATION_ID } from './demo-data';
import { createPreviewClient } from './mock-client';

afterEach(() => cleanup());

describe('preview demo client', () => {
    it('connects home, chat, settings, studio, and persona fixtures', async () => {
        const client = createPreviewClient();
        const characters = await client.listCharacters();
        const conversations = await client.listConversations(DEMO_INITIAL_CHARACTER_ID);
        const state = await client.getConversationState(DEMO_INITIAL_CONVERSATION_ID);
        const branches = await client.listBranches(DEMO_INITIAL_CONVERSATION_ID);
        const messages = await client.listBranchMessages(state.active_branch_id);
        const providers = await client.getProviderOverview();
        const routes = await client.listModelRoutes(providers.connections[0]?.id ?? '');
        const presets = await client.listGenerationPresets(routes[0]?.id ?? '');
        const workspace = await client.getOrchestrationWorkspace?.(
            DEMO_INITIAL_CONVERSATION_ID,
            state.active_branch_id,
        );
        const personaPage = await client.listPersonaPage({ limit: 100, after: null });

        expect(characters).toHaveLength(4);
        expect(conversations).toHaveLength(2);
        expect(branches).toHaveLength(2);
        expect(messages).toHaveLength(6);
        expect(providers.templates).toHaveLength(2);
        expect(providers.connections).toHaveLength(1);
        expect(routes).toHaveLength(1);
        expect(presets).toHaveLength(2);
        expect(workspace?.prompt_blocks).toHaveLength(4);
        expect(workspace?.memory_records).toHaveLength(2);
        expect(personaPage.kind).toBe('page');
        if (personaPage.kind === 'page') expect(personaPage.items).toHaveLength(3);
    });

    it('keeps each demo session isolated and resets it on client recreation', async () => {
        const first = createPreviewClient();
        const second = createPreviewClient();

        const firstRead = await first.listCharacters();
        const firstCharacter = firstRead[0];
        if (firstCharacter === undefined) throw new Error('Demo character fixture is missing.');
        firstCharacter.name = '변경된 복사본';
        expect((await first.listCharacters())[0]?.name).toBe('아리아');

        await first.createPersona({ name: '테스트 페르소나', description: '세션 전용' });
        expect(await first.listPersonas({ limit: 100 })).toHaveLength(4);
        expect(await second.listPersonas({ limit: 100 })).toHaveLength(3);
    });

    it('boots the connected mobile demo and keeps chat input interactive', async () => {
        render(App, {
            client: createPreviewClient(),
            initialSelection: {
                characterId: DEMO_INITIAL_CHARACTER_ID,
                conversationId: DEMO_INITIAL_CONVERSATION_ID,
            },
        });

        const character = await screen.findByRole('button', {
            name: /아리아 오래된 항해 기록/,
        });
        await waitFor(() => expect(character).toHaveAttribute('aria-pressed', 'true'));

        await fireEvent.click(screen.getByRole('button', { name: '채팅' }));
        const conversation = await screen.findByRole('button', {
            name: /잊혀진 서고.*아리아과의 대화/,
        });
        await fireEvent.click(conversation);

        const textbox = await screen.findByRole('textbox', { name: '메시지' });
        await fireEvent.input(textbox, { target: { value: '데모 입력 확인' } });
        expect(screen.getByRole('button', { name: '메시지 보내기' })).toBeEnabled();
    });

    it('loads the same fixtures through the real screen controllers', async () => {
        const client = createPreviewClient();
        const app = new LorepiaAppController(client);
        const studio = new OrchestrationController(client);
        const personas = new PersonaController(client);

        await app.start();
        let appState = get(app.state);
        expect(appState.bootstrap.phase).toBe('ready');
        expect(appState.library.characters).toHaveLength(4);
        expect(appState.providers.phase).toBe('ready');

        const character = appState.library.characters.find(
            (candidate) => candidate.id === DEMO_INITIAL_CHARACTER_ID,
        );
        expect(character).toBeDefined();
        if (character === undefined) throw new Error('Initial demo character was not loaded.');
        await app.selectCharacter(character);
        appState = get(app.state);
        const conversation = appState.conversations.items.find(
            (candidate) => candidate.id === DEMO_INITIAL_CONVERSATION_ID,
        );
        expect(conversation).toBeDefined();
        if (conversation === undefined)
            throw new Error('Initial demo conversation was not loaded.');
        expect(await app.selectConversation(conversation)).toBe(true);

        appState = get(app.state);
        expect(appState.messages.phase).toBe('ready');
        expect(appState.messages.items).toHaveLength(6);
        expect(appState.branches).toHaveLength(2);

        const conversationState = appState.conversation_state;
        if (conversationState === null) throw new Error('Demo conversation state was not loaded.');
        await studio.loadContext(DEMO_INITIAL_CONVERSATION_ID, conversationState.active_branch_id);
        const studioState = get(studio.state);
        expect(studioState.phase).toBe('ready');
        expect(studioState.workspace.prompt_blocks).toHaveLength(4);
        expect(studioState.workspace.content_modules).toHaveLength(1);
        expect(studioState.editable_memory_profiles).toHaveLength(1);

        expect(await personas.loadContext(DEMO_INITIAL_CONVERSATION_ID)).toBe(true);
        expect(get(personas.state).personas).toHaveLength(3);

        app.destroy();
        studio.destroy();
        personas.destroy();
    });
});
