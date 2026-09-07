import { cleanup, fireEvent, render, screen, waitFor, within } from '@testing-library/svelte';
import { afterEach, describe, expect, it } from 'vitest';
import { t } from '../../lib/i18n';
import UiPreview from './UiPreview.svelte';

afterEach(cleanup);
function messageAt(log: HTMLElement, index: number) {
    const message = within(log).getAllByRole('button', { name: t('uiPreview.messageMenu') })[index];
    if (!message) throw new Error('Missing message');
    return message;
}
async function openChat() {
    render(UiPreview);
    await fireEvent.click(screen.getByRole('button', { name: '비 오는 오후 · 오늘' }));
    return screen.getByRole('log');
}
describe('chat presentation and inline message tools', () => {
    it('follows a newly sent message after reading older history, while typing preserves the reading position', async () => {
        const log = await openChat();
        Object.defineProperties(log, {
            clientHeight: { value: 600 },
            scrollHeight: { value: 2000 },
        });
        log.scrollTop = 100;
        await fireEvent.scroll(log);
        expect(screen.getByRole('button', { name: t('uiPreview.latestMessage') })).toBeVisible();
        const input = screen.getByRole('textbox', { name: t('uiPreview.message') });
        await fireEvent.input(input, { target: { value: 'new message' } });
        expect(log.scrollTop).toBe(100);
        await fireEvent.click(screen.getByRole('button', { name: t('uiPreview.send') }));
        expect(screen.queryByRole('button', { name: t('uiPreview.latestMessage') })).toBeNull();
        expect(log.scrollTop).toBe(2000);
    });
    it('opens tools on the message itself and closes them on another message or empty space', async () => {
        const log = await openChat();
        const messages = within(log).getAllByRole('button', { name: t('uiPreview.messageMenu') });
        await fireEvent.click(messageAt(log, 0));
        expect(messages[0]).toHaveAttribute('aria-expanded', 'true');
        expect(
            within(log)
                .getByRole('group', { name: t('uiPreview.messageTools') })
                .closest('article'),
        ).toBe(messages[0]?.closest('article'));
        await fireEvent.keyDown(messageAt(log, 1), { key: 'Enter' });
        expect(messages[0]).toHaveAttribute('aria-expanded', 'false');
        expect(messages[1]).toHaveAttribute('aria-expanded', 'true');
        expect(screen.getByRole('button', { name: t('uiPreview.editMessage') })).toBeVisible();
        await fireEvent.pointerDown(log);
        expect(screen.queryByRole('group', { name: t('uiPreview.messageTools') })).toBeNull();
    });
    it('switches chat/story from room settings without replacing the conversation', async () => {
        const log = await openChat();
        const before = log.textContent;
        const chat = screen.getByRole('region', { name: t('uiPreview.chat') });
        expect(chat).toHaveAttribute('data-conversation-mode', 'chat');
        await fireEvent.click(screen.getByRole('button', { name: t('uiPreview.roomSettings') }));
        await fireEvent.click(screen.getByRole('radio', { name: /스토리/ }));
        await fireEvent.click(screen.getByRole('button', { name: t('uiPreview.save') }));
        expect(chat).toHaveAttribute('data-conversation-mode', 'story');
        expect(log.textContent).toBe(before);
    });
    it('requires a second action before removing a message and its continuation', async () => {
        const log = await openChat();
        await fireEvent.click(messageAt(log, 1));
        await fireEvent.click(screen.getByRole('button', { name: t('uiPreview.removeFrom') }));
        expect(within(log).getAllByRole('article')).toHaveLength(3);
        await fireEvent.click(screen.getByRole('button', { name: t('uiPreview.cancel') }));
        expect(within(log).getAllByRole('article')).toHaveLength(3);
        await fireEvent.click(screen.getByRole('button', { name: t('uiPreview.removeFrom') }));
        await fireEvent.click(screen.getByRole('button', { name: t('uiPreview.confirmRemove') }));
        expect(within(log).getAllByRole('article')).toHaveLength(1);
        expect(log).toHaveFocus();
    });
    it('returns to the original continuation using the branch picker', async () => {
        const log = await openChat();
        await fireEvent.click(messageAt(log, 0));
        await fireEvent.click(screen.getByRole('button', { name: t('uiPreview.branchFrom') }));
        expect(within(log).getAllByRole('article')).toHaveLength(1);
        expect(log).toHaveFocus();
        await fireEvent.change(screen.getByRole('combobox', { name: t('uiPreview.branch') }), {
            target: { value: 'main' },
        });
        await waitFor(() => expect(within(log).getAllByRole('article')).toHaveLength(3));
    });
});
