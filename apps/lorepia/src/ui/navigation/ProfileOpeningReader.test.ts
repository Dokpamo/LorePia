import { cleanup, fireEvent, render, screen, waitFor, within } from '@testing-library/svelte';
import { afterEach, expect, it, vi } from 'vitest';
import { createPreviewClient } from '../../preview/mock-client';
import { t, locale } from '../../lib/i18n';
import { openingLanguage } from '../../lib/i18n/opening-language';
import type {
    CharacterGreetingDetailDto,
    CharacterGreetingDetailInput,
} from '../../lib/ipc/contracts/character';
import ProfileOpeningReader from './ProfileOpeningReader.svelte';

afterEach(() => {
    cleanup();
    locale.set('ko');
});

it('reads the chosen language, drops stale replies, and starts only the currently displayed variant', async () => {
    const client = createPreviewClient();
    const pending = new Map<string, (value: CharacterGreetingDetailDto) => void>();
    client.getCharacterGreetingDetail = vi.fn(
        (input: CharacterGreetingDetailInput) =>
            new Promise<CharacterGreetingDetailDto>((resolve) =>
                pending.set(input.greeting_id, resolve),
            ),
    );
    const onstart = vi.fn();
    const props = {
        client,
        characterId: 'character',
        revisionId: 'r1',
        activeRevisionId: 'r1',
        selectedId: 'en',
        recommendedLanguage: 'en',
        profile: null,
        group: {
            id: 'scene',
            kind: 'alternate' as const,
            number: 1,
            variants: [
                { id: 'ko', language: 'ko' },
                { id: 'en', language: 'en' },
            ],
        },
        onstart,
        onclose: vi.fn(),
    };
    const view = render(ProfileOpeningReader, props);
    const start = screen.getByRole('button', { name: t('navigation.startChat') });
    expect(start).toBeDisabled();
    await waitFor(() => expect(pending.has('ko')).toBe(true));
    const reader = screen.getByRole('dialog');
    const language = screen.getByRole('button', { name: t('navigation.openingLanguage') });
    expect(screen.queryByRole('button', { name: openingLanguage('en') })).toBeNull();
    await fireEvent.click(language);
    const menu = screen.getByRole('dialog', { name: t('navigation.openingLanguage') });
    expect(reader).toHaveProperty('inert', true);
    expect(start).toBeDisabled();
    expect(within(menu).getByRole('radio', { name: openingLanguage('ko') })).toBeChecked();
    await fireEvent.click(within(menu).getByRole('radio', { name: openingLanguage('en') }));
    await waitFor(() => expect(language).toHaveFocus());
    expect(language).toHaveAccessibleDescription(openingLanguage('en'));
    await waitFor(() => expect(pending.has('en')).toBe(true));
    const reply = (id: string, text: string) => ({
        character_id: 'character',
        character_content_revision_id: 'r1',
        greeting_id: id,
        text,
    });
    pending.get('en')?.(
        reply('en', 'Full English scene.\n\nFinal paragraph. <script>inert</script>'),
    );
    expect(await screen.findByText('Final paragraph. <script>inert</script>')).toBeVisible();
    pending.get('ko')?.(reply('ko', 'Stale scene'));
    await waitFor(() => expect(screen.queryByText('Stale scene')).toBeNull());
    expect(onstart).not.toHaveBeenCalled();
    expect(screen.getByRole('dialog').querySelector('script')).toBeNull();
    await fireEvent.click(start);
    expect(onstart).toHaveBeenCalledExactlyOnceWith('en', start);
    await view.rerender({ covered: true });
    await view.rerender({ covered: false });
    expect(language).toHaveAccessibleDescription(openingLanguage('en'));
    await fireEvent.click(language);
    await fireEvent.keyDown(screen.getByRole('dialog', { name: t('navigation.openingLanguage') }), {
        key: 'Escape',
    });
    await waitFor(() => expect(language).toHaveFocus());
    expect(language).toHaveAccessibleDescription(openingLanguage('en'));
    await view.rerender({ activeRevisionId: 'r2' });
    expect(start).toBeDisabled();
    expect(within(screen.getByRole('dialog')).getByRole('status')).toHaveTextContent(
        t('navigation.openingUnavailable'),
    );
});

it('omits language selection for a single-language opening', () => {
    render(ProfileOpeningReader, {
        client: createPreviewClient(),
        characterId: 'character',
        revisionId: null,
        activeRevisionId: null,
        selectedId: null,
        profile: null,
        group: {
            id: 'scene',
            kind: 'default',
            number: 0,
            variants: [{ id: 'one', language: 'ko' }],
        },
        onclose: vi.fn(),
        onstart: vi.fn(),
    });
    expect(screen.queryByRole('button', { name: t('navigation.openingLanguage') })).toBeNull();
});
