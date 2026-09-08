import { cleanup, fireEvent, render, screen, waitFor } from '@testing-library/svelte';
import { afterEach, describe, expect, it, vi } from 'vitest';
import { t } from '../../lib/i18n';
import TextEditor from './TextEditor.svelte';

afterEach(cleanup);

describe('durable fullscreen edits', () => {
    it('keeps a rejected edit and blocks duplicate saves and back navigation while saving', async () => {
        let finish: (accepted: boolean) => void = () => undefined;
        const onchange = vi.fn(() => new Promise<boolean>((resolve) => (finish = resolve)));
        const onclose = vi.fn();
        render(TextEditor, {
            request: { label: 'Edit', value: 'original', applyOnDone: true, onchange },
            onclose,
            onclosed: () => undefined,
        });
        const input = screen.getByRole('textbox', { name: 'Edit' });
        await fireEvent.input(input, { target: { value: 'edited draft' } });
        const done = screen.getByRole('button', { name: t('uiPreview.editDone') });
        await fireEvent.click(done);
        expect(done).toBeDisabled();
        expect(input).toHaveAttribute('readonly');
        expect(screen.getByRole('dialog')).toHaveAttribute('aria-busy', 'true');
        await fireEvent.keyDown(input, { key: 'Escape' });
        expect(onclose).not.toHaveBeenCalled();
        expect(onchange).toHaveBeenCalledTimes(1);
        finish(false);
        const error = await screen.findByRole('alert');
        expect(error).toHaveTextContent(t('workspace.saveFailed'));
        expect(input).toHaveAttribute('aria-describedby', error.id);
        expect(input).toHaveValue('edited draft');
        expect(onclose).not.toHaveBeenCalled();
        await fireEvent.click(done);
        finish(true);
        await waitFor(() => expect(onclose).toHaveBeenCalledTimes(1));
    });
});
