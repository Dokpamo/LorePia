import { cleanup, fireEvent, render, screen, waitFor, within } from '@testing-library/svelte';
import { afterEach, describe, expect, it } from 'vitest';
import { t } from '../../lib/i18n';
import UiPreview from './UiPreview.svelte';

afterEach(cleanup);

async function editMessage() {
    const app = render(UiPreview);
    await fireEvent.click(screen.getByRole('button', { name: '비 오는 오후 · 오늘' }));
    const log = screen.getByRole('log');
    const message = within(log).getAllByRole('button', { name: t('uiPreview.messageMenu') })[1];
    if (!message) throw new Error('Missing user message');
    await fireEvent.click(message);
    await fireEvent.click(screen.getByRole('button', { name: t('uiPreview.editMessage') }));
    const input = screen.getByRole('textbox', { name: t('uiPreview.editMessage') });
    return { ...app, input, log };
}
async function done() {
    await fireEvent.click(screen.getByRole('button', { name: t('uiPreview.editDone') }));
}

describe('preview editor commit and validation', () => {
    it('keeps the original and following reply until Done, then creates one editable branch', async () => {
        const { input, log, container } = await editMessage();
        await fireEvent.input(input, { target: { value: '수정 중인 첫 문장' } });
        await fireEvent.input(input, { target: { value: '수정이 끝난 문장' } });
        expect(log).toHaveTextContent('오늘은 추천해 주는 책을 읽어볼게.');
        expect(log).toHaveTextContent('그럼, 이 책은 어때요?');
        expect(log).not.toHaveTextContent('수정이 끝난 문장');
        expect(container.querySelector('.ui-branch-select')).toBeNull();
        await fireEvent.compositionStart(input);
        expect(screen.getByRole('button', { name: t('uiPreview.editDone') })).toBeDisabled();
        await fireEvent.compositionEnd(input);
        await done();
        await waitFor(() => expect(screen.queryByRole('dialog')).toBeNull());
        const branches = screen.getByRole('button', { name: t('uiPreview.branch') });
        expect(log).toHaveTextContent('수정이 끝난 문장');
        expect(log).not.toHaveTextContent('그럼, 이 책은 어때요?');
        await fireEvent.click(branches);
        const options = screen.getAllByRole('radio');
        expect(options).toHaveLength(2);
        const original = options[0];
        if (!original) throw new Error('Missing original branch');
        await fireEvent.click(original);
        await waitFor(() => expect(screen.queryByRole('dialog')).toBeNull());
        expect(log).toHaveTextContent('오늘은 추천해 주는 책을 읽어볼게.');
        expect(log).toHaveTextContent('그럼, 이 책은 어때요?');
    });

    it.each(['button', 'escape'] as const)(
        'protects a changed message on %s and discards only its edit',
        async (via) => {
            const { input, log, container } = await editMessage();
            const before = log.textContent;
            await fireEvent.input(input, { target: { value: '저장하지 않을 수정' } });
            const back = async () => {
                if (via === 'button')
                    await fireEvent.click(
                        screen.getByRole('button', { name: t('uiPreview.closeEditor') }),
                    );
                else await fireEvent.keyDown(input, { key: 'Escape' });
            };
            await back();
            const dialog = screen.getByRole('alertdialog');
            const keep = within(dialog).getByRole('button', { name: t('uiPreview.keepEditing') });
            expect(keep).toHaveFocus();
            await fireEvent.keyDown(dialog, { key: 'Escape' });
            await waitFor(() => expect(screen.queryByRole('alertdialog')).toBeNull());
            expect(input).toHaveValue('저장하지 않을 수정');
            expect(input).toHaveFocus();
            await back();
            await fireEvent.click(
                screen.getByRole('button', { name: t('uiPreview.discardChanges') }),
            );
            await waitFor(() => expect(screen.queryByRole('dialog')).toBeNull());
            expect(log.textContent).toBe(before);
            expect(container.querySelector('.ui-branch-select')).toBeNull();
            expect(log).toHaveFocus();
        },
    );

    it('blocks a blank sent message, associates its error, and clears the error after correction', async () => {
        const { input, container } = await editMessage();
        await fireEvent.input(input, { target: { value: '  \n  ' } });
        await done();
        expect(input).toHaveAttribute('aria-invalid', 'true');
        const error = screen.getByRole('alert');
        expect(error).toHaveTextContent(t('uiPreview.messageRequired'));
        expect(input).toHaveAttribute('aria-describedby', error.id);
        expect(input).toHaveFocus();
        expect(container.querySelector('.ui-branch-select')).toBeNull();
        await fireEvent.input(input, { target: { value: '수정한 문장' } });
        expect(input).not.toHaveAttribute('aria-invalid');
        expect(screen.queryByRole('alert')).toBeNull();
        await done();
        await waitFor(() => expect(screen.queryByRole('dialog')).toBeNull());
        expect(within(screen.getByRole('log')).getByText('수정한 문장')).toBeVisible();
    });

    it('closes an unchanged message without confirmation or creating a branch', async () => {
        const { container } = await editMessage();
        await done();
        await waitFor(() => expect(screen.queryByRole('dialog')).toBeNull());
        expect(screen.queryByRole('alertdialog')).toBeNull();
        expect(container.querySelector('.ui-branch-select')).toBeNull();
    });

    it.each([
        ['card', 'uiPreview.cardName', 'uiPreview.cardNameRequired'],
        ['room', 'uiPreview.chatName', 'uiPreview.chatNameRequired'],
    ] as const)(
        'keeps a blank %s name open in both the editor and settings until corrected',
        async (kind, fieldKey, errorKey) => {
            render(UiPreview);
            await fireEvent.click(
                screen.getByRole('button', {
                    name:
                        kind === 'card' ? t('uiPreview.cardSettings') : '비 오는 오후 채팅방 설정',
                }),
            );
            await fireEvent.click(screen.getByRole('button', { name: t(fieldKey) }));
            const input = screen.getByRole('textbox', { name: t(fieldKey) });
            expect(input).toBeRequired();
            await fireEvent.input(input, { target: { value: '   ' } });
            await done();
            expect(screen.getByRole('alert')).toHaveTextContent(t(errorKey));
            expect(input).toHaveAttribute('aria-invalid', 'true');
            await fireEvent.click(screen.getByRole('button', { name: t('uiPreview.closeEditor') }));
            await waitFor(() => expect(screen.queryByRole('textbox')).toBeNull());
            const field = screen.getByRole('button', { name: t(fieldKey) });
            expect(field).toHaveAttribute('data-invalid', 'true');
            await fireEvent.click(screen.getByRole('button', { name: t('uiPreview.save') }));
            expect(screen.getByRole('dialog')).toBeVisible();
            await waitFor(() => expect(field).toHaveFocus());
            const form = field.closest('form');
            if (!form) throw new Error('Missing settings form');
            await fireEvent.submit(form);
            expect(screen.getByRole('dialog')).toBeVisible();
            await fireEvent.click(field);
            await fireEvent.input(screen.getByRole('textbox', { name: t(fieldKey) }), {
                target: { value: '고친 이름' },
            });
            await done();
            await waitFor(() => expect(screen.queryByRole('textbox')).toBeNull());
            expect(screen.queryByRole('alert')).toBeNull();
            await fireEvent.click(screen.getByRole('button', { name: t('uiPreview.save') }));
            await waitFor(() => expect(screen.queryByRole('dialog')).toBeNull());
            expect(
                screen.getByRole(kind === 'card' ? 'heading' : 'button', {
                    name: kind === 'card' ? '고친 이름' : '고친 이름 · 오늘',
                }),
            ).toBeVisible();
        },
    );

    it('still creates a new conversation with its optional default name', async () => {
        render(UiPreview);
        await fireEvent.click(screen.getByRole('button', { name: t('uiPreview.newChatAdd') }));
        await fireEvent.click(screen.getByRole('button', { name: t('uiPreview.startChat') }));
        await waitFor(() => expect(screen.queryByRole('dialog')).toBeNull());
        expect(
            within(screen.getByRole('region', { name: t('uiPreview.chat') })).getByText(
                t('uiPreview.newChat'),
            ),
        ).toBeVisible();
    });
});
