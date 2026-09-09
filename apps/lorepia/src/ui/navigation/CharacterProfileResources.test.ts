import { cleanup, fireEvent, render, screen, waitFor } from '@testing-library/svelte';
import { afterEach, expect, it, vi } from 'vitest';
import type { CharacterRenderProfileDto } from '../../lib/ipc/contracts';
import { createPreviewClient } from '../../preview/mock-client';
import CharacterProfileResources from './CharacterProfileResources.svelte';
import { t } from '../../lib/i18n';

afterEach(cleanup);

function profileFor(id: string): CharacterRenderProfileDto {
    return {
        character_id: id,
        character_content_revision_id: null,
        assets: [],
        background_markup: '',
        toggle_schema: '',
        initial_variables: {},
        output_transforms: [],
        display_transforms: [],
        runtime_scripts: [],
        required_runtime_capabilities: [],
        runtime_capabilities_declared: true,
        runtime_script_count: 0,
        runtime_knowledge: [
            {
                id: `${id}-entry`,
                name: `${id} lore`,
                content: '<script>inert text</script>',
                enabled: true,
                primary_keys: ['library'],
                secondary_keys: [],
                constant: false,
                selective: false,
                case_sensitive: false,
                whole_word: false,
                use_regex: false,
                probability_basis_points: 10000,
                folder: false,
            },
        ],
    };
}

function character(id: string) {
    return { id, name: id, description: '', thumbnail: id, subpage: false, histories: [] };
}

it('ignores an earlier character response and opens a dedicated page for the current lore', async () => {
    const client = createPreviewClient();
    const pending = new Map<string, (value: CharacterRenderProfileDto) => void>();
    const read = vi.fn(
        (id: string) =>
            new Promise<CharacterRenderProfileDto>((resolve) => pending.set(id, resolve)),
    );
    client.getCharacterRenderProfile = read;
    const onopen = vi.fn();
    const view = render(CharacterProfileResources, {
        client,
        character: character('first'),
        onopen,
    });
    await waitFor(() => expect(pending.has('first')).toBe(true));
    await view.rerender({ client, character: character('second'), onopen });
    await waitFor(() => expect(pending.has('second')).toBe(true));
    pending.get('second')?.(profileFor('second'));
    const disclosure = await screen.findByRole('button', {
        name: new RegExp(
            `${t('navigation.profileLorebook')}\\s*${t('navigation.resourceEntries', { number: 1 })}`,
        ),
    });
    await fireEvent.click(disclosure);
    expect(onopen).toHaveBeenCalledWith(
        {
            title: t('navigation.profileLorebook'),
            collection: { kind: 'lorebook', profile: profileFor('second') },
        },
        disclosure,
    );
    expect(disclosure).not.toHaveAttribute('aria-expanded');
    expect(screen.queryByRole('heading', { name: t('navigation.includedResources') })).toBeNull();
    pending.get('first')?.(profileFor('first'));
    await waitFor(() => expect(screen.queryByText('first lore')).toBeNull());
    await fireEvent.click(disclosure);
    expect(onopen).toHaveBeenLastCalledWith(
        {
            title: t('navigation.profileLorebook'),
            collection: { kind: 'lorebook', profile: profileFor('second') },
        },
        disclosure,
    );
    expect(read.mock.calls).toEqual([['first'], ['second']]);
});

it('retries resource reads without triggering a content action', async () => {
    const client = createPreviewClient();
    const read = vi
        .fn()
        .mockRejectedValueOnce(new Error('unavailable'))
        .mockResolvedValueOnce(profileFor('first'));
    client.getCharacterRenderProfile = read;
    const onopen = vi.fn();
    render(CharacterProfileResources, { client, character: character('first'), onopen });
    expect(await screen.findByRole('alert')).toHaveTextContent(t('navigation.resourcesFailed'));
    await fireEvent.click(screen.getByRole('button', { name: t('workspace.retry') }));
    await screen.findByRole('button', {
        name: new RegExp(
            `${t('navigation.profileLorebook')}\\s*${t('navigation.resourceEntries', { number: 1 })}`,
        ),
    });
    expect(read).toHaveBeenCalledTimes(2);
    expect(onopen).not.toHaveBeenCalled();
});
