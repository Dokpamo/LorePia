import { cleanup, fireEvent, render, screen, waitFor, within } from '@testing-library/svelte';
import { tick } from 'svelte';
import { afterEach, describe, expect, it, vi } from 'vitest';
import { t } from '../../lib/i18n';
import UiPreview from './UiPreview.svelte';
import ResponseStatus from './ResponseStatus.svelte';
import UiNotice from './UiNotice.svelte';

const originalClipboard = Object.getOwnPropertyDescriptor(navigator, 'clipboard');
function clipboard(writeText: (text: string) => Promise<void>) {
    Object.defineProperty(navigator, 'clipboard', {
        configurable: true,
        get: () => ({ writeText }),
    });
}
afterEach(() => {
    cleanup();
    vi.restoreAllMocks();
    vi.useRealTimers();
    if (originalClipboard) Object.defineProperty(navigator, 'clipboard', originalClipboard);
    else Reflect.deleteProperty(navigator, 'clipboard');
});

async function changedSettings() {
    const app = render(UiPreview);
    await fireEvent.click(screen.getByRole('button', { name: t('uiPreview.cardSettings') }));
    await fireEvent.click(screen.getByRole('button', { name: t('uiPreview.cardName') }));
    await fireEvent.input(screen.getByRole('textbox'), { target: { value: '바꾼 이름' } });
    expect(screen.getByText(t('uiPreview.settingsEditorHint'))).toBeVisible();
    await fireEvent.click(screen.getByRole('button', { name: t('uiPreview.editDone') }));
    await waitFor(() => expect(screen.queryByRole('textbox')).toBeNull());
    return app;
}

async function openTools() {
    render(UiPreview);
    await fireEvent.click(screen.getByRole('button', { name: '비 오는 오후 · 오늘' }));
    const message = within(screen.getByRole('log')).getAllByRole('button', {
        name: t('uiPreview.messageMenu'),
    })[0];
    if (!message) throw new Error('Missing message');
    await fireEvent.click(message);
}

describe('settings, search and action feedback', () => {
    it.each(['button', 'escape'] as const)(
        'protects changed settings on %s and keeps the editable draft',
        async (via) => {
            await changedSettings();
            if (via === 'button')
                await fireEvent.click(screen.getByRole('button', { name: t('uiPreview.back') }));
            else await fireEvent.keyDown(window, { key: 'Escape' });
            const dialog = screen.getByRole('alertdialog');
            const keep = within(dialog).getByRole('button', { name: t('uiPreview.keepEditing') });
            const discard = within(dialog).getByRole('button', {
                name: t('uiPreview.discardChanges'),
            });
            expect(within(dialog).getAllByRole('button')).toEqual([discard, keep]);
            expect(keep).toHaveFocus();
            await fireEvent.keyDown(keep, { key: 'Tab' });
            expect(discard).toHaveFocus();
            await fireEvent.keyDown(discard, { key: 'Tab', shiftKey: true });
            expect(keep).toHaveFocus();
            await fireEvent.keyDown(dialog, { key: 'Escape' });
            await waitFor(() => expect(screen.queryByRole('alertdialog')).toBeNull());
            expect(screen.getByRole('button', { name: t('uiPreview.cardName') })).toHaveTextContent(
                '바꾼 이름',
            );
            await fireEvent.click(screen.getByRole('button', { name: t('uiPreview.save') }));
            expect(screen.getByRole('heading', { name: '바꾼 이름' })).toBeVisible();
        },
    );

    it('discards only after confirmation and leaves unchanged settings without asking', async () => {
        await changedSettings();
        await fireEvent.click(screen.getByRole('button', { name: t('uiPreview.back') }));
        await fireEvent.click(screen.getByRole('button', { name: t('uiPreview.discardChanges') }));
        await waitFor(() => expect(screen.getByRole('heading', { name: '서연' })).toBeVisible());
        await fireEvent.click(screen.getByRole('button', { name: t('uiPreview.cardSettings') }));
        await fireEvent.click(screen.getByRole('button', { name: t('uiPreview.back') }));
        expect(screen.queryByRole('alertdialog')).toBeNull();
        expect(screen.getByRole('heading', { name: '서연' })).toBeVisible();
    });

    it('closes a focused editor when the browser emits focusout during DOM removal', async () => {
        const errors: ErrorEvent[] = [];
        function error(event: ErrorEvent) {
            errors.push(event);
            event.preventDefault();
        }
        window.addEventListener('error', error);
        // jsdom does not emit the native focusout seen when Chromium removes the editor.
        vi.spyOn(Element.prototype, 'remove').mockImplementation(function (this: Element) {
            const focused = document.activeElement;
            if (focused && this.contains(focused))
                focused.dispatchEvent(new FocusEvent('focusout', { bubbles: true }));
            this.parentNode?.removeChild(this);
        });
        try {
            await changedSettings();
            expect(errors).toEqual([]);
            await waitFor(() =>
                expect(screen.getByRole('button', { name: t('uiPreview.cardName') })).toHaveFocus(),
            );
        } finally {
            window.removeEventListener('error', error);
        }
    });

    it('keeps live results and clearing inside the search screen, then opens the selected conversation', async () => {
        render(UiPreview);
        await fireEvent.click(screen.getByRole('button', { name: t('uiPreview.findChat') }));
        const search = screen.getByRole('searchbox');
        expect(search.closest('header')).not.toBeNull();
        expect(screen.getByRole('list', { name: t('uiPreview.searchResults') })).toBeVisible();
        await fireEvent.input(search, { target: { value: '일치하지 않음' } });
        expect(
            within(screen.getByRole('dialog', { name: t('uiPreview.findChat') })).getByText(
                t('uiPreview.noChatMatches'),
            ),
        ).toBeVisible();
        await fireEvent.click(
            within(screen.getByRole('search')).getByRole('button', {
                name: t('uiPreview.clearChatSearch'),
            }),
        );
        expect(search).toHaveFocus();
        expect(screen.getByRole('button', { name: '처음 만난 날 · 어제' })).toBeVisible();
        await fireEvent.input(search, { target: { value: '처음' } });
        await fireEvent.compositionStart(search);
        await fireEvent.keyDown(search, { key: 'Enter', isComposing: true });
        expect(search).toBeVisible();
        await fireEvent.compositionEnd(search);
        await fireEvent.keyDown(search, { key: 'Enter' });
        await waitFor(() => expect(screen.queryByRole('searchbox')).toBeNull());
        await waitFor(() => expect(screen.getByRole('log')).toHaveFocus());
        expect(
            within(screen.getByRole('region', { name: t('uiPreview.chat') })).getByText(
                '처음 만난 날',
            ),
        ).toBeVisible();
    });

    it('shows a visible clipboard failure with retry, then replaces it with success', async () => {
        const writeText = vi
            .fn()
            .mockRejectedValueOnce(new Error('denied'))
            .mockResolvedValue(undefined);
        clipboard(writeText);
        await openTools();
        await fireEvent.click(screen.getByRole('menuitem', { name: t('uiPreview.copyMessage') }));
        const error = await screen.findByText(t('uiPreview.copyFailed'));
        expect(error.closest('.ui-notice')).not.toBeNull();
        expect(error.closest('.ui-sr')).toBeNull();
        const notice = error.closest<HTMLElement>('.ui-notice');
        if (!notice) throw new Error('Missing visible notice');
        await fireEvent.click(
            within(notice).getByRole('button', {
                name: t('uiPreview.retryReply'),
            }),
        );
        await waitFor(() => expect(screen.getByText(t('uiPreview.copied'))).toBeVisible());
        expect(screen.queryByText(t('uiPreview.copyFailed'))).toBeNull();
        expect(writeText).toHaveBeenCalledTimes(2);
    });

    it('ignores a clipboard result after leaving its conversation', async () => {
        let finish: () => void = () => undefined;
        const writeText = vi.fn(
            () =>
                new Promise<void>((resolve) => {
                    finish = resolve;
                }),
        );
        clipboard(writeText);
        await openTools();
        await fireEvent.click(screen.getByRole('menuitem', { name: t('uiPreview.copyMessage') }));
        await fireEvent.click(screen.getByRole('button', { name: t('uiPreview.openManagement') }));
        await fireEvent.click(screen.getByRole('button', { name: '하루 캐릭터 선택' }));
        finish();
        await tick();
        expect(screen.queryByText(t('uiPreview.copied'))).toBeNull();
    });

    it('shows a keyboard tooltip and dismisses it with Escape without navigating away', async () => {
        await openTools();
        await fireEvent.keyDown(screen.getByRole('menu'), { key: 'Escape' });
        await fireEvent.keyDown(window, { key: 'Tab' });
        const button = screen.getByRole('button', { name: t('uiPreview.roomSettings') });
        button.focus();
        await waitFor(() =>
            expect(screen.getByRole('tooltip')).toHaveTextContent(t('uiPreview.roomSettings')),
        );
        expect(button).toHaveAttribute('aria-describedby');
        await fireEvent.keyDown(button, { key: 'Escape' });
        expect(screen.queryByRole('tooltip')).toBeNull();
        expect(button).not.toHaveAttribute('aria-describedby');
        expect(screen.getByRole('log')).toBeVisible();
    });

    it('keeps a success notice while either keyboard focus or the pointer is inside it', async () => {
        vi.useFakeTimers();
        const dismiss = vi.fn();
        render(UiNotice, {
            notice: { id: 1, text: t('uiPreview.copied') },
            ondismiss: dismiss,
        });
        const notice = screen.getByRole('status');
        const close = within(notice).getByRole('button');
        await fireEvent.pointerEnter(notice);
        close.focus();
        await fireEvent.pointerLeave(notice);
        await vi.advanceTimersByTimeAsync(5000);
        expect(dismiss).not.toHaveBeenCalled();
        await fireEvent.pointerEnter(notice);
        close.blur();
        await vi.advanceTimersByTimeAsync(5000);
        expect(dismiss).not.toHaveBeenCalled();
        await fireEvent.pointerLeave(notice);
        await vi.advanceTimersByTimeAsync(4000);
        expect(dismiss).toHaveBeenCalledOnce();
    });

    it('explains a delayed reply and preserves partial text on a terminal failure', async () => {
        vi.useFakeTimers();
        const app = render(ResponseStatus, {
            message: { id: 'reply', role: 'assistant', text: '', status: 'pending' },
            busy: true,
            onretry: vi.fn(),
        });
        expect(screen.getByText(t('uiPreview.replyWaiting'))).toBeVisible();
        await vi.advanceTimersByTimeAsync(10000);
        expect(screen.getByText(t('uiPreview.replyDelayed'))).toBeVisible();
        await app.rerender({
            message: { id: 'reply', role: 'assistant', text: 'partial', status: 'pending' },
            busy: true,
            onretry: vi.fn(),
        });
        expect(screen.getByText(t('uiPreview.replyWriting'))).toBeVisible();
        expect(screen.queryByText(t('uiPreview.replyDelayed'))).toBeNull();
        await vi.advanceTimersByTimeAsync(9999);
        expect(screen.queryByText(t('uiPreview.replyDelayed'))).toBeNull();
        await vi.advanceTimersByTimeAsync(1);
        expect(screen.getByText(t('uiPreview.replyDelayed'))).toBeVisible();
        await app.rerender({
            message: { id: 'reply', role: 'assistant', text: 'partial', status: 'failed' },
            busy: false,
            onretry: vi.fn(),
        });
        expect(screen.getByText(t('uiPreview.partialReplyKept'))).toBeVisible();
        expect(screen.queryByText(t('uiPreview.replyDelayed'))).toBeNull();
        expect(screen.getByRole('button', { name: t('uiPreview.retryReply') })).toBeEnabled();
        app.unmount();
        expect(vi.getTimerCount()).toBe(0);
    });
});
