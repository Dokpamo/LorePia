import { cleanup, fireEvent, render, screen } from '@testing-library/svelte';
import { afterEach, expect, it, vi } from 'vitest';
import { createPreviewClient } from '../../preview/mock-client';
import { DEMO_CONTENT_MODULE_DOCUMENTS } from '../../preview/demo-data';
import { t } from '../../lib/i18n';
import type { CreatorContentModuleDocumentDto, RevisionedDto } from '../../lib/ipc/contracts';
import ProfilePluginsPage from './ProfilePluginsPage.svelte';

afterEach(cleanup);
it('does not restore stale character plugins after navigating to another character', async () => {
    const client = createPreviewClient();
    const [character] = await client.listCharacters();
    const [module] = DEMO_CONTENT_MODULE_DOCUMENTS;
    if (!character || !module) throw new Error('Missing fixtures');
    let finish!: (items: RevisionedDto<CreatorContentModuleDocumentDto>[]) => void;
    client.listContentModules = vi
        .fn()
        .mockImplementationOnce(
            () =>
                new Promise((resolve) => {
                    finish = resolve;
                }),
        )
        .mockResolvedValue([]);
    const view = render(ProfilePluginsPage, {
        client,
        characterId: character.id,
        onclose: vi.fn(),
    });
    await view.rerender({ characterId: 'another-character' });
    expect(await screen.findByText(t('navigation.noProfilePlugins'))).toBeVisible();
    finish(DEMO_CONTENT_MODULE_DOCUMENTS);
    await new Promise((resolve) => setTimeout(resolve, 0));
    expect(screen.queryByText(module.value.name)).toBeNull();
});

it('shows a retry after a failed read without claiming that no plugins exist', async () => {
    const client = createPreviewClient();
    client.listContentModules = vi
        .fn()
        .mockRejectedValueOnce(new Error('failed'))
        .mockResolvedValue([]);
    render(ProfilePluginsPage, { client, characterId: 'character', onclose: vi.fn() });
    expect(await screen.findByRole('alert')).toHaveTextContent(t('navigation.pluginsFailed'));
    expect(screen.queryByText(t('navigation.noProfilePlugins'))).toBeNull();
    await fireEvent.click(screen.getByRole('button', { name: t('workspace.retry') }));
    expect(await screen.findByText(t('navigation.noProfilePlugins'))).toBeVisible();
});
