import { cleanup, fireEvent, screen, waitFor } from '@testing-library/svelte';
import { afterEach, describe, expect, it } from 'vitest';
import { t } from '../../../lib/i18n';
import { openWorkspaceChat } from '../../../tests/workspace-chat';

afterEach(cleanup);
const messageInput = () => screen.getByRole('textbox', { name: t('uiPreview.message') });
describe('current composer fullscreen behavior', () => {
    it('writes inline, offers fullscreen from two rendered lines, and returns to the same draft', async () => {
        await openWorkspaceChat();
        const input = messageInput();
        input.style.lineHeight = '25.6px';
        input.style.padding = '8px 8px 0';
        let height = 34;
        Object.defineProperty(input, 'scrollHeight', { get: () => height });
        await fireEvent.focus(input);
        await fireEvent.input(input, { target: { value: 'One line' } });
        expect(screen.queryByRole('dialog')).toBeNull();
        expect(screen.queryByRole('button', { name: t('uiPreview.expandComposer') })).toBeNull();
        height = 60;
        await fireEvent.input(input, { target: { value: 'First line\nSecond line' } });
        const expand = screen.getByRole('button', { name: t('uiPreview.expandComposer') });
        await fireEvent.click(expand);
        expect(screen.getByRole('dialog')).toBeVisible();
        expect(
            screen
                .getByRole('button', { name: t('uiPreview.collapseComposer') })
                .querySelector('.lucide-minimize-2'),
        ).not.toBeNull();
        expect(screen.queryByRole('button', { name: t('uiPreview.closeEditor') })).toBeNull();
        expect(messageInput()).toHaveValue('First line\nSecond line');
        await fireEvent.input(messageInput(), {
            target: { value: 'Edited first line\nEdited second line' },
        });
        await fireEvent.click(
            screen.getByRole('button', { name: t('uiPreview.collapseComposer') }),
        );
        await waitFor(() => expect(screen.queryByRole('dialog')).toBeNull());
        expect(messageInput()).toBe(input);
        await waitFor(() => expect(input).toHaveFocus());
        expect(input).toHaveValue('Edited first line\nEdited second line');
    });
});
