import { cleanup, fireEvent, render, screen, waitFor } from '@testing-library/svelte';
import { afterEach, beforeAll, expect, it, vi } from 'vitest';
import WorkspaceApp from '../WorkspaceApp.svelte';
import { createPreviewClient } from '../../../preview/mock-client';
import { t } from '../../../lib/i18n';

beforeAll(() => {
    if (typeof Reflect.get(Element.prototype, 'getAnimations') !== 'function')
        Object.defineProperty(Element.prototype, 'getAnimations', {
            configurable: true,
            value: () => [],
        });
});
afterEach(cleanup);
it('keeps a prompt draft editable after Rust rejects the save and does not report success', async () => {
    const client = createPreviewClient();
    vi.spyOn(client, 'listCharacters').mockResolvedValue([]);
    const save = vi
        .spyOn(client, 'upsertPromptPreset')
        .mockRejectedValue(new Error('save rejected'));
    render(WorkspaceApp, { client });
    await screen.findByText(t('workspace.emptyLibrary'));
    await fireEvent.click(screen.getByRole('button', { name: t('navigation.settings') }));
    await fireEvent.click(screen.getByRole('button', { name: t('settings.section.prompt.title') }));
    await fireEvent.click(await screen.findByRole('button', { name: t('settingsUi.addPrompt') }));
    await fireEvent.click(screen.getByRole('button', { name: t('settingsUi.promptName') }));
    await fireEvent.input(
        await screen.findByRole('textbox', { name: t('settingsUi.promptName') }),
        { target: { value: 'Unsaved draft' } },
    );
    await fireEvent.click(screen.getByRole('button', { name: t('uiPreview.editDone') }));
    await waitFor(() => expect(screen.queryByRole('textbox')).toBeNull());
    await fireEvent.click(screen.getByRole('button', { name: t('settingsUi.save') }));
    await waitFor(() => expect(save).toHaveBeenCalledOnce());
    expect(await screen.findByRole('alert')).toBeVisible();
    expect(screen.queryByText(t('settingsLive.saved'))).toBeNull();
    expect(screen.getByRole('button', { name: t('settingsUi.promptName') })).toHaveTextContent(
        'Unsaved draft',
    );
});
