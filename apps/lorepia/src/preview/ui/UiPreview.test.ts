import { cleanup, fireEvent, render, screen, waitFor, within } from '@testing-library/svelte';
import { afterAll, afterEach, beforeAll, describe, expect, it } from 'vitest';
import { t, type MessageKey } from '../../lib/i18n';
import UiPreview from './UiPreview.svelte';

const originalAnimations = Object.getOwnPropertyDescriptor(Element.prototype, 'getAnimations');
beforeAll(() => {
    if (!originalAnimations)
        Object.defineProperty(Element.prototype, 'getAnimations', {
            configurable: true,
            value: () => [],
        });
});
afterAll(() => {
    if (!originalAnimations) Reflect.deleteProperty(Element.prototype, 'getAnimations');
});

afterEach(cleanup);

async function edit(key: MessageKey) {
    await fireEvent.click(screen.getByRole('button', { name: t(key) }));
    return screen.getByRole(key === 'uiPreview.findChat' ? 'searchbox' : 'textbox', {
        name: t(key),
    });
}
async function closeEditor() {
    const name = screen.getByRole('dialog').getAttribute('aria-label') ?? '';
    await fireEvent.click(screen.getByRole('button', { name: t('uiPreview.closeEditor') }));
    await waitFor(() => expect(screen.queryByRole('dialog', { name })).toBeNull());
}
const messageInput = () => screen.getByRole('textbox', { name: t('uiPreview.message') });

describe('native UI preview', () => {
    it('filters only the selected card history and keeps search separate from conversation drafts', async () => {
        render(UiPreview);
        await fireEvent.input(await edit('uiPreview.findChat'), { target: { value: '오후' } });
        await closeEditor();
        expect(screen.getByRole('button', { name: '비 오는 오후 · 오늘' })).toBeVisible();
        expect(screen.queryByRole('button', { name: '처음 만난 날 · 어제' })).toBeNull();
        await fireEvent.click(screen.getByRole('button', { name: '하루 캐릭터 선택' }));
        expect(screen.getByRole('button', { name: '늦은 오후의 커피 · 오늘' })).toBeVisible();
        expect(screen.getByRole('button', { name: t('uiPreview.findChat') })).toHaveTextContent(
            t('uiPreview.findChat'),
        );
        await fireEvent.click(screen.getByRole('button', { name: '서연 캐릭터 선택' }));
        expect(screen.getByRole('button', { name: t('uiPreview.findChat') })).toHaveTextContent(
            '오후',
        );
        await fireEvent.click(screen.getByRole('button', { name: t('uiPreview.clearChatSearch') }));
        expect(screen.getByRole('button', { name: '처음 만난 날 · 어제' })).toBeVisible();
    });
    it('writes inline, offers fullscreen from two rendered lines, and returns to the same draft', async () => {
        render(UiPreview);
        await fireEvent.click(screen.getByRole('button', { name: '비 오는 오후 · 오늘' }));
        const input = messageInput();
        input.style.lineHeight = '25.6px';
        input.style.padding = '8px 8px 0';
        let height = 34;
        Object.defineProperty(input, 'scrollHeight', { get: () => height });
        await fireEvent.focus(input);
        await fireEvent.input(input, { target: { value: '한 줄' } });
        expect(screen.queryByRole('dialog')).toBeNull();
        expect(screen.queryByRole('button', { name: t('uiPreview.expandComposer') })).toBeNull();
        height = 60;
        await fireEvent.input(input, { target: { value: '첫 번째 줄\n두 번째 줄' } });
        const expand = screen.getByRole('button', { name: t('uiPreview.expandComposer') });
        await fireEvent.click(expand);
        expect(screen.getByRole('dialog')).toBeVisible();
        expect(
            screen
                .getByRole('button', { name: t('uiPreview.collapseComposer') })
                .querySelector('.lucide-minimize-2'),
        ).not.toBeNull();
        expect(screen.queryByRole('button', { name: t('uiPreview.closeEditor') })).toBeNull();
        expect(messageInput()).toHaveValue('첫 번째 줄\n두 번째 줄');
        await fireEvent.input(messageInput(), { target: { value: '두 줄을\n편집했어요' } });
        await fireEvent.click(
            screen.getByRole('button', { name: t('uiPreview.collapseComposer') }),
        );
        await waitFor(() => expect(screen.queryByRole('dialog')).toBeNull());
        expect(messageInput()).toBe(input);
        expect(input).toHaveFocus();
        expect(input).toHaveValue('두 줄을\n편집했어요');
    });
    it('returns from room settings to the chat that opened them', async () => {
        render(UiPreview);
        await fireEvent.click(screen.getByRole('button', { name: '비 오는 오후 · 오늘' }));
        const trigger = screen.getByRole('button', { name: t('uiPreview.roomSettings') });
        await fireEvent.click(trigger);
        await fireEvent.click(screen.getByRole('button', { name: t('uiPreview.back') }));
        expect(messageInput()).toBeVisible();
        expect(trigger).toHaveFocus();
    });
    it('opens fields in a focused full-screen editor and returns keyboard focus without losing edits', async () => {
        render(UiPreview);
        const main = screen.getByRole('main');
        const trigger = screen.getByRole('button', { name: t('uiPreview.cardSettings') });
        await fireEvent.keyDown(window, { key: 'Tab' });
        await fireEvent.click(trigger);
        expect(main).toHaveAttribute('data-keyboard-focus', 'true');
        expect(screen.queryByRole('textbox')).toBeNull();
        const field = screen.getByRole('button', { name: t('uiPreview.cardName') });
        const input = await edit('uiPreview.cardName');
        expect(input).toHaveFocus();
        await fireEvent.pointerDown(input);
        await fireEvent.input(input, { target: { value: '새로운 서연' } });
        expect(main).toHaveAttribute('data-keyboard-focus', 'false');
        await closeEditor();
        expect(field).toHaveFocus();
        expect(field).toHaveTextContent('새로운 서연');
        await fireEvent.click(screen.getByRole('button', { name: t('uiPreview.save') }));
        expect(trigger).toHaveFocus();
        expect(screen.getByRole('heading', { name: '새로운 서연' })).toBeVisible();
    });

    it('keeps drafts attached to conversations across editor dismissal and character switching', async () => {
        render(UiPreview);
        await fireEvent.click(screen.getByRole('button', { name: '비 오는 오후 · 오늘' }));
        await fireEvent.input(messageInput(), {
            target: { value: '서연에게 쓰던 내용' },
        });
        await fireEvent.click(screen.getByRole('button', { name: t('uiPreview.openManagement') }));
        await fireEvent.click(screen.getByRole('button', { name: '하루 캐릭터 선택' }));
        await fireEvent.click(screen.getByRole('button', { name: '늦은 오후의 커피 · 오늘' }));
        expect(messageInput()).toHaveValue('');
        await fireEvent.click(screen.getByRole('button', { name: t('uiPreview.openManagement') }));
        await fireEvent.click(screen.getByRole('button', { name: '서연 캐릭터 선택' }));
        await fireEvent.click(screen.getByRole('button', { name: '비 오는 오후 · 오늘' }));
        expect(messageInput()).toHaveValue('서연에게 쓰던 내용');
    });

    it('creates a conversation, preserves Korean composition and line breaks, and sends once', async () => {
        render(UiPreview);
        await fireEvent.click(screen.getByRole('button', { name: t('uiPreview.newChatAdd') }));
        await fireEvent.input(await edit('uiPreview.chatName'), {
            target: { value: '새로운 오후' },
        });
        await closeEditor();
        await fireEvent.click(screen.getByRole('button', { name: t('uiPreview.startChat') }));
        const input = messageInput();
        await fireEvent.input(input, { target: { value: '같이 책을 읽자.\n두 번째 줄' } });
        await fireEvent.compositionStart(input);
        await fireEvent.keyDown(input, { key: 'Enter', metaKey: true, isComposing: true });
        expect(input).toHaveValue('같이 책을 읽자.\n두 번째 줄');
        expect(screen.getByRole('button', { name: t('uiPreview.send') })).toBeDisabled();
        await fireEvent.compositionEnd(input);
        await fireEvent.keyDown(input, { key: 'Enter', shiftKey: true });
        expect(within(screen.getByRole('log')).queryByText(/같이 책을 읽자/)).toBeNull();
        await fireEvent.keyDown(input, { key: 'Enter' });
        expect(within(screen.getByRole('log')).getAllByText(/같이 책을 읽자/)).toHaveLength(1);
        expect(screen.getByRole('button', { name: t('uiPreview.stopReply') })).toBeEnabled();
        await fireEvent.keyDown(input, { key: 'Enter' });
        expect(within(screen.getByRole('log')).getAllByText(/같이 책을 읽자/)).toHaveLength(1);
        expect(messageInput()).toHaveFocus();
    });

    it('lets a creator omit the right page and restores the settings trigger focus', async () => {
        render(UiPreview);
        const trigger = screen.getByRole('button', { name: t('uiPreview.cardSettings') });
        await fireEvent.click(trigger);
        await fireEvent.click(screen.getByRole('checkbox', { name: t('uiPreview.useSubpage') }));
        await fireEvent.click(screen.getByRole('button', { name: t('uiPreview.save') }));
        expect(trigger).toHaveFocus();
        await fireEvent.click(screen.getByRole('button', { name: '비 오는 오후 · 오늘' }));
        expect(screen.queryByRole('button', { name: t('uiPreview.openSubpage') })).toBeNull();
        expect(
            screen.queryByRole('region', { name: t('uiPreview.subpage'), hidden: true }),
        ).toBeNull();
    });

    it('groups cards with a keyboard alternative, renames the folder in full-screen, and extracts a card', async () => {
        render(UiPreview);
        const first = screen.getByRole('button', { name: '서연 캐릭터 선택' });
        expect(first).not.toHaveTextContent('서연');
        await fireEvent.keyDown(first, { key: 'F10', shiftKey: true });
        await fireEvent.click(screen.getByRole('button', { name: '하루 카드와 묶기' }));
        expect(screen.getByRole('button', { name: '폴더 1 열기' })).toHaveAttribute(
            'aria-expanded',
            'true',
        );
        await fireEvent.click(screen.getByRole('button', { name: t('uiPreview.folderSettings') }));
        await fireEvent.input(await edit('uiPreview.folderName'), { target: { value: '친구들' } });
        await closeEditor();
        await fireEvent.click(screen.getByRole('button', { name: t('uiPreview.back') }));
        expect(screen.getByRole('button', { name: '친구들 열기' })).toBeVisible();
        await fireEvent.click(screen.getByRole('button', { name: '하루 캐릭터 선택' }));
        expect(screen.getByRole('heading', { name: '하루' })).toBeVisible();
        await fireEvent.click(screen.getByRole('button', { name: t('uiPreview.folderSettings') }));
        await fireEvent.click(screen.getByRole('button', { name: '하루 폴더에서 꺼내기' }));
        expect(screen.queryByRole('button', { name: '친구들 열기' })).toBeNull();
        expect(screen.getByRole('button', { name: '하루 캐릭터 선택' })).toHaveFocus();
        for (const name of ['서연', '하루', '도윤'])
            expect(screen.getByRole('button', { name: `${name} 캐릭터 선택` })).toBeVisible();
    });

    it('edits a sent user message through the same full-screen surface', async () => {
        render(UiPreview);
        await fireEvent.click(screen.getByRole('button', { name: '비 오는 오후 · 오늘' }));
        const turns = screen.getAllByRole('article');
        const userTurn = turns[1];
        if (!userTurn) throw new Error('Missing sample user message');
        await fireEvent.click(
            within(userTurn).getByRole('button', { name: t('uiPreview.messageMenu') }),
        );
        await fireEvent.click(screen.getByRole('menuitem', { name: t('uiPreview.editMessage') }));
        await fireEvent.input(
            await screen.findByRole('textbox', { name: t('uiPreview.editMessage') }),
            {
                target: { value: '메시지 고치기' },
            },
        );
        await fireEvent.click(screen.getByRole('button', { name: t('uiPreview.editDone') }));
        await waitFor(() => expect(screen.queryByRole('dialog')).toBeNull());
        expect(within(screen.getByRole('log')).getByText('메시지 고치기')).toBeVisible();
    });

    it('changes only local appearance and starts with fresh samples in a new window session', async () => {
        const first = render(UiPreview);
        await fireEvent.click(screen.getByRole('button', { name: t('uiPreview.appSettings') }));
        await fireEvent.click(screen.getByRole('button', { name: t('uiPreview.theme') }));
        await fireEvent.click(screen.getByRole('radio', { name: t('uiPreview.light') }));
        expect(screen.getByRole('main')).toHaveAttribute('data-appearance', 'light');
        first.unmount();
        render(UiPreview);
        expect(screen.getByRole('main')).toHaveAttribute('data-appearance', 'system');
        expect(screen.getByRole('button', { name: '서연 캐릭터 선택' })).toHaveAttribute(
            'aria-pressed',
            'true',
        );
    });
});
