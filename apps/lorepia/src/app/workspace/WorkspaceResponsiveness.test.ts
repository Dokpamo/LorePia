import { cleanup, fireEvent, render, screen, waitFor, within } from '@testing-library/svelte';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import WorkspaceApp from './WorkspaceApp.svelte';
import { createPreviewClient } from '../../preview/mock-client';
import { deferred } from '../tests/app-controller-test-support';
import { t } from '../../lib/i18n';
import type { CharacterGreetingCatalogDto, ConversationBranchDto } from '../../lib/ipc/contracts';

beforeEach(() => {
    vi.stubGlobal(
        'ResizeObserver',
        class {
            observe = vi.fn();
            unobserve = vi.fn();
            disconnect = vi.fn();
        },
    );
});
afterEach(() => {
    cleanup();
    vi.restoreAllMocks();
    vi.unstubAllGlobals();
});

describe('workspace responsiveness with slow reads', () => {
    it('opens the selected profile before greeting metadata is available', async () => {
        const client = createPreviewClient();
        const characters = await client.listCharacters();
        const character = characters[1];
        if (!character) throw new Error('Character fixture missing');
        const original = client.getCharacterGreetingCatalog.bind(client);
        const pending = deferred<CharacterGreetingCatalogDto>();
        vi.spyOn(client, 'getCharacterGreetingCatalog').mockImplementation((id) =>
            id === character.id ? pending.promise : original(id),
        );
        render(WorkspaceApp, { client });
        await fireEvent.click(
            await screen.findByRole('button', {
                name: t('uiPreview.cardSelect', { name: character.name }),
            }),
        );
        const profile = await screen.findByRole('dialog', { name: t('navigation.characterInfo') });
        expect(within(profile).getByRole('heading', { name: character.name })).toBeVisible();
        await fireEvent.click(within(profile).getByRole('button', { name: t('uiPreview.back') }));
        pending.resolve(await original(character.id));
        await waitFor(() => expect(screen.queryByRole('dialog')).toBeNull());
    });

    it('prepares the first character without waiting for provider setup', async () => {
        const client = createPreviewClient();
        const [character] = await client.listCharacters();
        if (!character) throw new Error('Character fixture missing');
        const original = client.getProviderOverview.bind(client);
        const pending = deferred<Awaited<ReturnType<typeof original>>>();
        vi.spyOn(client, 'getProviderOverview').mockReturnValue(pending.promise);
        const greetings = vi.spyOn(client, 'getCharacterGreetingCatalog');
        render(WorkspaceApp, { client });
        await screen.findByRole('button', {
            name: t('uiPreview.cardSelect', { name: character.name }),
        });
        await waitFor(() => expect(greetings).toHaveBeenCalledWith(character.id));
        pending.resolve(await original());
    });

    it('shows messages and a truthful loading state before branch metadata, without a fake stop action', async () => {
        const client = createPreviewClient();
        const [conversation] = await client.listConversations(null);
        if (!conversation) throw new Error('Conversation fixture missing');
        const original = client.listBranches.bind(client);
        const branches = deferred<ConversationBranchDto[]>();
        vi.spyOn(client, 'listBranches').mockReturnValue(branches.promise);
        render(WorkspaceApp, { client });
        await fireEvent.click(screen.getByRole('button', { name: t('navigation.chats') }));
        await fireEvent.click(
            await screen.findByRole('button', {
                name: new RegExp('^' + conversation.title + ' ·'),
            }),
        );
        const log = await screen.findByRole('log', { name: t('uiPreview.messages') });
        await waitFor(() => expect(log.querySelector('[data-message-id]')).not.toBeNull());
        expect(log).toHaveAttribute('aria-busy', 'true');
        expect(screen.queryByText(t('uiPreview.emptyChat'))).toBeNull();
        expect(screen.queryByRole('button', { name: t('uiPreview.stopReply') })).toBeNull();
        branches.resolve(await original(conversation.id));
        await waitFor(() => expect(log).toHaveAttribute('aria-busy', 'false'));
    });

    it('loads only the requested settings section', async () => {
        const client = createPreviewClient();
        const [character] = await client.listCharacters();
        if (!character) throw new Error('Character fixture missing');
        const personas = vi.spyOn(client, 'listPersonaPage');
        const orchestration = vi.spyOn(client, 'getOrchestrationWorkspace');
        render(WorkspaceApp, { client });
        await screen.findByRole('button', {
            name: t('uiPreview.cardSelect', { name: character.name }),
        });
        await fireEvent.click(screen.getByRole('button', { name: t('navigation.settings') }));
        const root = await screen.findByRole('region', { name: t('navigation.settings') });
        expect(personas).not.toHaveBeenCalled();
        expect(orchestration).not.toHaveBeenCalled();
        await fireEvent.click(
            within(root).getByRole('button', { name: t('settings.section.persona.title') }),
        );
        await waitFor(() => expect(personas).toHaveBeenCalledTimes(1));
        expect(orchestration).not.toHaveBeenCalled();
    });
});
