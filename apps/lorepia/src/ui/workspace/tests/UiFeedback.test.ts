import { cleanup, fireEvent, render, screen, waitFor, within } from '@testing-library/svelte';
import { tick } from 'svelte';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { t } from '../../../lib/i18n';
import { openWorkspaceChat } from '../../../tests/workspace-chat';
import ResponseStatus from '../ResponseStatus.svelte';
import UiNotice from '../UiNotice.svelte';

const originalClipboard = Object.getOwnPropertyDescriptor(navigator, 'clipboard');
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
function clipboard(writeText: (text: string) => Promise<void>) {
    Object.defineProperty(navigator, 'clipboard', {
        configurable: true,
        get: () => ({ writeText }),
    });
}
afterEach(() => {
    cleanup();
    vi.restoreAllMocks();
    vi.unstubAllGlobals();
    vi.useRealTimers();
    if (originalClipboard) Object.defineProperty(navigator, 'clipboard', originalClipboard);
    else Reflect.deleteProperty(navigator, 'clipboard');
});

async function openTools() {
    const view = await openWorkspaceChat();
    const [message] = within(screen.getByRole('log')).getAllByRole('button', {
        name: t('uiPreview.messageMenu'),
    });
    if (!message) throw new Error('Missing message');
    await fireEvent.contextMenu(message);
    return view;
}

describe('current workspace feedback', () => {
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
        const { client, character } = await openTools();
        await fireEvent.click(screen.getByRole('menuitem', { name: t('uiPreview.copyMessage') }));
        await fireEvent.click(screen.getByRole('button', { name: t('uiPreview.openManagement') }));
        await fireEvent.click(screen.getByRole('button', { name: t('navigation.home') }));
        const other = (await client.listCharacters()).find((item) => item.id !== character.id);
        if (!other) throw new Error('Missing second character');
        await fireEvent.click(
            screen.getByRole('button', {
                name: t('uiPreview.cardSelect', { name: other.name }),
            }),
        );
        finish();
        await tick();
        expect(screen.queryByText(t('uiPreview.copied'))).toBeNull();
    });

    it('shows a keyboard tooltip and dismisses it with Escape without navigating away', async () => {
        await openTools();
        await fireEvent.keyDown(screen.getByRole('menu'), { key: 'Escape' });
        const button = screen.getByRole('button', { name: t('uiPreview.roomSettings') });
        await fireEvent.keyDown(button, { key: 'Tab' });
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
