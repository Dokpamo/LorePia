import { cleanup, fireEvent, render, screen, within } from '@testing-library/svelte';
import { afterEach, describe, expect, it, vi } from 'vitest';
import MobileNavigation from '../components/mobile/MobileNavigation.svelte';
import { t } from '../lib/i18n';

afterEach(cleanup);

describe('navigation icons and accessible actions', () => {
    it('keeps icon artwork decorative while buttons expose names and destinations', async () => {
        const onHome = vi.fn();
        const onChat = vi.fn();
        const onSettings = vi.fn();
        render(MobileNavigation, { view: 'chat', onHome, onChat, onSettings });
        const navigation = screen.getByRole('navigation', { name: t('app.nav.label') });
        const buttons = within(navigation).getAllByRole('button');
        expect(buttons).toHaveLength(3);
        for (const button of buttons) {
            expect(button).toHaveAccessibleName();
            for (const svg of button.querySelectorAll('svg'))
                expect(svg).toHaveAttribute('aria-hidden', 'true');
        }
        expect(
            within(navigation).getByRole('button', { name: t('mobile.nav.chat') }),
        ).toHaveAttribute('aria-current', 'page');
        await fireEvent.click(within(navigation).getByRole('button', { name: t('app.tab.home') }));
        await fireEvent.click(
            within(navigation).getByRole('button', { name: t('mobile.nav.all') }),
        );
        expect(onHome).toHaveBeenCalledOnce();
        expect(onSettings).toHaveBeenCalledOnce();
        expect(onChat).not.toHaveBeenCalled();
    });
});
