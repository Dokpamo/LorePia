import { cleanup, fireEvent, render, screen, waitFor, within } from '@testing-library/svelte';
import { afterEach, describe, expect, it } from 'vitest';
import { t } from '../../lib/i18n';
import UiPreview from './UiPreview.svelte';

afterEach(cleanup);

describe('preview settings choices', () => {
    it('expands, collapses, and dismisses from the keyboard handle without changing the setting', async () => {
        render(UiPreview);
        await fireEvent.click(screen.getByRole('button', { name: t('uiPreview.appSettings') }));
        const opener = screen.getByRole('button', { name: t('uiPreview.theme') });
        await fireEvent.click(opener);
        const handle = screen.getByRole('button', { name: t('uiPreview.expandSheet') });
        await fireEvent.click(handle);
        expect(handle).toHaveAttribute('aria-expanded', 'true');
        expect(screen.getByRole('dialog', { name: t('uiPreview.theme') })).toHaveAttribute(
            'data-sheet-expanded',
            'true',
        );
        await fireEvent.keyDown(handle, { key: 'ArrowDown' });
        expect(handle).toHaveAttribute('aria-expanded', 'false');
        await fireEvent.keyDown(handle, { key: 'ArrowDown' });
        await waitFor(() =>
            expect(screen.queryByRole('dialog', { name: t('uiPreview.theme') })).toBeNull(),
        );
        expect(opener).toHaveFocus();
        expect(screen.getByRole('main')).toHaveAttribute('data-appearance', 'system');
    });

    it('opens with the current theme selected, contains keyboard focus and cancels without changing it', async () => {
        render(UiPreview);
        await fireEvent.click(screen.getByRole('button', { name: t('uiPreview.appSettings') }));
        const opener = screen.getByRole('button', { name: t('uiPreview.theme') });
        await fireEvent.click(opener);
        const sheet = screen.getByRole('dialog', { name: t('uiPreview.theme') });
        const selected = within(sheet).getByRole('radio', { name: t('uiPreview.system') });
        const close = within(sheet).getByRole('button', { name: t('uiPreview.closeChoices') });
        const handle = within(sheet).getByRole('button', { name: t('uiPreview.expandSheet') });
        expect(selected).toHaveAttribute('aria-checked', 'true');
        expect(selected).toHaveFocus();
        await fireEvent.keyDown(selected, { key: 'ArrowDown' });
        const next = within(sheet).getByRole('radio', { name: t('uiPreview.light') });
        expect(next).toHaveFocus();
        expect(screen.getByRole('main')).toHaveAttribute('data-appearance', 'system');
        await fireEvent.keyDown(next, { key: 'Tab' });
        expect(handle).toHaveFocus();
        await fireEvent.keyDown(handle, { key: 'Tab', shiftKey: true });
        expect(next).toHaveFocus();
        await fireEvent.click(close);
        await waitFor(() =>
            expect(screen.queryByRole('dialog', { name: t('uiPreview.theme') })).toBeNull(),
        );
        expect(opener).toHaveFocus();
        expect(screen.getByRole('main')).toHaveAttribute('data-appearance', 'system');
    });

    it('applies theme and text size through the same choice UI and restores the trigger', async () => {
        render(UiPreview);
        await fireEvent.click(screen.getByRole('button', { name: t('uiPreview.appSettings') }));
        const theme = screen.getByRole('button', { name: t('uiPreview.theme') });
        await fireEvent.click(theme);
        await fireEvent.click(screen.getByRole('radio', { name: t('uiPreview.dark') }));
        await waitFor(() => expect(theme).toHaveFocus());
        expect(theme).toHaveTextContent(t('uiPreview.dark'));
        expect(screen.getByRole('main')).toHaveAttribute('data-appearance', 'dark');
        const size = screen.getByRole('button', { name: t('uiPreview.textSize') });
        await fireEvent.click(size);
        await fireEvent.click(screen.getByRole('radio', { name: t('uiPreview.textLarge') }));
        await waitFor(() => expect(size).toHaveFocus());
        expect(size).toHaveTextContent(t('uiPreview.textLarge'));
        expect(screen.getByRole('main').style.getPropertyValue('--ui-text-scale')).toBe('1.1');
    });

    it('keeps room choices inside the settings draft and discards them through the existing guard', async () => {
        render(UiPreview);
        const roomSettings = () => screen.getByRole('button', { name: '비 오는 오후 채팅방 설정' });
        await fireEvent.click(roomSettings());
        await fireEvent.click(screen.getByRole('button', { name: t('uiPreview.responsePreview') }));
        await fireEvent.click(screen.getByRole('radio', { name: t('uiPreview.previewSlow') }));
        await waitFor(() =>
            expect(
                screen.queryByRole('dialog', { name: t('uiPreview.responsePreview') }),
            ).toBeNull(),
        );
        expect(
            screen.getByRole('button', { name: t('uiPreview.responsePreview') }),
        ).toHaveTextContent(t('uiPreview.previewSlow'));
        await fireEvent.click(screen.getByRole('button', { name: t('uiPreview.back') }));
        expect(screen.getByRole('alertdialog')).toBeVisible();
        await fireEvent.click(screen.getByRole('button', { name: t('uiPreview.discardChanges') }));
        await waitFor(() => expect(screen.queryByRole('dialog')).toBeNull());
        await fireEvent.click(roomSettings());
        expect(
            screen.getByRole('button', { name: t('uiPreview.responsePreview') }),
        ).toHaveTextContent(t('uiPreview.previewComplete'));
    });
});
