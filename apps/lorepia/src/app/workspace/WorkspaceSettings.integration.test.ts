import { cleanup, fireEvent, render, screen, waitFor } from '@testing-library/svelte';
import { afterEach, beforeAll, describe, expect, it, vi } from 'vitest';
import WorkspaceApp from './WorkspaceApp.svelte';
import { createPreviewClient } from '../../preview/mock-client';
import { t, setLocale } from '../../lib/i18n';
import { setChatTextSize } from '../../lib/display';

beforeAll(() => {
    if (typeof Reflect.get(Element.prototype, 'getAnimations') !== 'function')
        Object.defineProperty(Element.prototype, 'getAnimations', {
            configurable: true,
            value: () => [],
        });
});
afterEach(() => {
    cleanup();
    setLocale('ko');
    setChatTextSize('normal');
});

async function openSettings() {
    const client = createPreviewClient();
    vi.spyOn(client, 'listCharacters').mockResolvedValue([]);
    render(WorkspaceApp, { client });
    await screen.findByText(t('workspace.emptyLibrary'));
    await fireEvent.click(screen.getByRole('button', { name: t('uiPreview.appSettings') }));
    return client;
}
describe('live workspace settings entry', () => {
    it('keeps mockup settings groups and saves a prompt with no selected conversation', async () => {
        const client = await openSettings();
        expect(screen.getByRole('button', { name: t('uiPreview.theme') })).toBeVisible();
        expect(screen.getByRole('button', { name: t('uiPreview.aiConnection') })).toBeVisible();
        const save = vi.spyOn(client, 'upsertPromptPreset');
        await fireEvent.click(
            screen.getByRole('button', { name: t('settings.section.prompt.title') }),
        );
        await fireEvent.click(
            await screen.findByRole('button', { name: t('settingsUi.addPrompt') }),
        );
        for (const [key, value] of [
            ['settingsUi.promptName', 'Saved through settings'],
            ['settingsUi.promptContent', 'Write dialogue with clear character voices.'],
        ] as const) {
            await fireEvent.click(screen.getByRole('button', { name: t(key) }));
            await fireEvent.input(await screen.findByRole('textbox', { name: t(key) }), {
                target: { value },
            });
            await fireEvent.click(screen.getByRole('button', { name: t('uiPreview.editDone') }));
            await waitFor(() => expect(screen.queryByRole('textbox', { name: t(key) })).toBeNull());
        }
        await fireEvent.click(screen.getByRole('button', { name: t('settingsUi.save') }));
        await waitFor(() => expect(save).toHaveBeenCalledOnce());
        await screen.findByText(t('settingsLive.saved'));
        if (!client.listPromptPresets) throw new Error('Settings API missing');
        const saved = (await client.listPromptPresets()).find(
            (item) => item.value.name === 'Saved through settings',
        );
        expect(saved?.revision).toBe(1);
        expect(screen.getByRole('button', { name: t('settingsLive.applyPrompt') })).toBeDisabled();
    });
    it('shows reserved built-in prompts without requesting edit authority', async () => {
        const client = await openSettings();
        if (!client.listPromptPresets) throw new Error('Settings API missing');
        const [original] = await client.listPromptPresets();
        if (!original) throw new Error('Prompt fixture missing');
        vi.spyOn(client, 'listPromptPresets').mockResolvedValue([
            {
                ...original,
                value: {
                    ...original.value,
                    id: 'lorepia.builtin.chat-compatible.v1',
                    name: 'Native default',
                },
            },
        ]);
        const edit = vi.spyOn(client, 'getEditablePromptPreset');
        await fireEvent.click(
            screen.getByRole('button', { name: t('settings.section.prompt.title') }),
        );
        await fireEvent.click(await screen.findByRole('button', { name: /Native default/ }));
        expect(edit).not.toHaveBeenCalled();
        expect(screen.getByText(t('settingsLive.builtInHint'))).toBeVisible();
        expect(screen.queryByRole('button', { name: t('settingsUi.deletePrompt') })).toBeNull();
    });
    it('uses the shared choice sheet and persists the language selection', async () => {
        await openSettings();
        await fireEvent.click(screen.getByRole('button', { name: t('settingsUi.screenLanguage') }));
        expect(screen.getByRole('dialog', { name: t('settingsUi.screenLanguage') })).toHaveClass(
            'ui-choice-sheet',
        );
        await fireEvent.click(screen.getByRole('radio', { name: t('settingsUi.english') }));
        await waitFor(() => expect(localStorage.getItem('lorepia.locale')).toBe('en'));
        expect(document.documentElement.lang).toBe('en');
    });
    it('selects a persona for the open conversation through the saved selection API', async () => {
        const client = createPreviewClient();
        const [character] = await client.listCharacters();
        if (!character) throw new Error('Character fixture missing');
        const [conversation] = await client.listConversations(character.id);
        const [persona] = await client.listPersonas({ limit: 100 });
        if (!conversation || !persona) throw new Error('Conversation fixture missing');
        const select = vi.spyOn(client, 'selectConversationPersona');
        render(WorkspaceApp, { client });
        await fireEvent.click(
            await screen.findByRole('button', {
                name: new RegExp('^' + conversation.title + ' ·'),
            }),
        );
        await fireEvent.click(
            await screen.findByRole('button', { name: t('uiPreview.roomSettings') }),
        );
        await fireEvent.click(screen.getByRole('button', { name: t('uiPreview.back') }));
        await fireEvent.click(screen.getByRole('button', { name: t('uiPreview.openManagement') }));
        await fireEvent.click(screen.getByRole('button', { name: t('uiPreview.appSettings') }));
        await fireEvent.click(
            screen.getByRole('button', { name: t('settings.section.persona.title') }),
        );
        await fireEvent.click(
            await screen.findByRole('button', { name: t('settingsLive.personaForRoom') }),
        );
        await fireEvent.click(screen.getByRole('radio', { name: persona.value.name }));
        await waitFor(() => expect(select).toHaveBeenCalledOnce());
        const saved = await client.getConversationPersonaSelection({
            conversation_id: conversation.id,
        });
        expect(saved.selected_persona?.value.id).toBe(persona.value.id);
    });
    it('loads actual storage counts using the shared client', async () => {
        const client = await openSettings();
        const stats = vi.spyOn(client, 'getStorageOverview');
        await fireEvent.click(screen.getByRole('button', { name: t('uiPreview.data') }));
        await screen.findByRole('heading', { name: t('settingsLive.storedData') });
        expect(stats).toHaveBeenCalledOnce();
        expect(screen.getByText(t('settingsLive.originalsHint'))).toBeVisible();
    });
});
