import { get } from 'svelte/store';
import { cleanup, fireEvent, screen } from '@testing-library/svelte';
import { afterEach, describe, expect, it } from 'vitest';

import { openWorkspaceChat } from '../tests/workspace-chat';
import { LorepiaAppController } from '../app/app-controller';
import { OrchestrationController } from '../features/orchestration/orchestration-controller';
import { PersonaController } from '../features/personas/persona-controller';
import {
    DEMO_ARCHIVE_MAIN_MESSAGE_IDS,
    DEMO_INITIAL_CHARACTER_ID,
    DEMO_INITIAL_CONVERSATION_ID,
} from './demo-data';
import { createPreviewClient, createPreviewCharacterPresentations } from './mock-client';

afterEach(() => cleanup());

describe('preview demo client', () => {
    it('keeps profile presentation examples isolated and keyed to real demo greetings', async () => {
        const client = createPreviewClient();
        const first = createPreviewCharacterPresentations();
        const second = createPreviewCharacterPresentations();
        for (const character of await client.listCharacters()) {
            const profile = first[character.id];
            expect(profile?.tags?.length).toBeGreaterThan(0);
            const catalog = await client.getCharacterGreetingCatalog(character.id);
            expect(Object.keys(profile?.introductions ?? {})).toEqual(
                catalog.greetings.map((item) => item.id),
            );
            profile?.tags?.push('mutation');
            expect(second[character.id]?.tags).not.toContain('mutation');
        }
    });
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
        expect(messages).toHaveLength(DEMO_ARCHIVE_MAIN_MESSAGE_IDS.length);
        expect(providers.templates).toHaveLength(2);
        expect(providers.connections).toHaveLength(1);
        expect(routes).toHaveLength(1);
        expect(presets).toHaveLength(2);
        expect(workspace?.prompt_blocks).toHaveLength(4);
        expect(workspace?.memory_records).toHaveLength(2);
        expect(workspace?.room_config.creativity).toBe(65);
        expect(Number.isInteger(workspace?.room_config.creativity)).toBe(true);
        if (!workspace || !client.saveRoomOrchestrationConfig)
            throw new Error('Preview room config is missing');
        await expect(
            client.saveRoomOrchestrationConfig({
                ...workspace.room_config,
                expected_revision: null,
                creativity: 0.65,
            }),
        ).rejects.toThrow('integer from 0 to 100');
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

    it('stores settings documents only inside one demo session', async () => {
        const first = createPreviewClient();
        const second = createPreviewClient();
        if (
            !first.listMemoryProfiles ||
            !second.listMemoryProfiles ||
            !first.upsertMemoryProfile ||
            !first.getStorageOverview
        )
            throw new Error('Settings API missing');
        const [item] = await first.listMemoryProfiles();
        if (!item) throw new Error('Expected fixture');
        const saved = await first.upsertMemoryProfile({
            value: { ...item.value, name: 'Edited memory' },
            expected_revision: item.revision,
        });
        expect(
            (await first.listMemoryProfiles()).find((value) => value.value.id === saved.value.id)
                ?.value.name,
        ).toBe('Edited memory');
        expect(
            (await second.listMemoryProfiles()).find((value) => value.value.id === saved.value.id)
                ?.value.name,
        ).toBe(item.value.name);
        expect(await first.getStorageOverview()).toMatchObject({ characters: 4 });
    });

    it('boots the connected mobile demo and keeps chat input interactive', async () => {
        const { input: textbox } = await openWorkspaceChat();
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
        expect(appState.messages.items).toHaveLength(DEMO_ARCHIVE_MAIN_MESSAGE_IDS.length);
        expect(appState.messages.items.map((message) => message.id)).toEqual(
            DEMO_ARCHIVE_MAIN_MESSAGE_IDS,
        );
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

it('returns independent character-specific plugin bindings from the preview client', async () => {
    const client = createPreviewClient();
    const [character] = await client.listCharacters();
    if (!client.listContentModules || !client.listContentModuleBindings)
        throw new Error('Missing plugin fixture API');
    const [module] = await client.listContentModules();
    if (!character || !module) throw new Error('Missing fixtures');
    const bindings = await client.listContentModuleBindings({
        content_module_id: module.value.id,
    });
    expect(bindings[0]?.value).toMatchObject({
        scope: 'character',
        target_id: character.id,
        enabled: true,
    });
    const [binding] = bindings;
    if (!binding) throw new Error('Missing binding fixture');
    binding.value.enabled = false;
    const again = await client.listContentModuleBindings({ content_module_id: module.value.id });
    expect(again[0]?.value.enabled).toBe(true);
    expect(await client.listContentModuleBindings({ content_module_id: 'unknown' })).toEqual([]);
});
