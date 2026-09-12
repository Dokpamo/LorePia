import { cleanup, fireEvent, render, screen, within } from '@testing-library/svelte';
import { afterEach, describe, expect, it, vi } from 'vitest';
import BottomNavigation from '../ui/navigation/BottomNavigation.svelte';
import { t } from '../lib/i18n';

afterEach(cleanup);

describe('navigation icons and accessible actions', () => {
    it('keeps icon artwork decorative while buttons expose names and destinations', async () => {
        const onchange = vi.fn();
        render(BottomNavigation, { selected: 'chats', onchange });
        const navigation = screen.getByRole('navigation', { name: t('navigation.label') });
        const buttons = within(navigation).getAllByRole('button');
        expect(buttons).toHaveLength(4);
        for (const button of buttons) {
            expect(button).toHaveAccessibleName();
            for (const svg of button.querySelectorAll('svg'))
                expect(svg).toHaveAttribute('aria-hidden', 'true');
        }
        expect(
            within(navigation).getByRole('button', { name: t('navigation.chats') }),
        ).toHaveAttribute('aria-current', 'page');
        await fireEvent.click(
            within(navigation).getByRole('button', { name: t('navigation.home') }),
        );
        await fireEvent.click(
            within(navigation).getByRole('button', { name: t('navigation.settings') }),
        );
        expect(onchange.mock.calls).toEqual([['home'], ['settings']]);
    });
});
