import { cleanup, fireEvent, render, screen } from '@testing-library/svelte';
import { afterEach, describe, expect, it } from 'vitest';
import UiPreview from '../../preview/ui/UiPreview.svelte';
import { createSampleCharacters } from '../../preview/ui/sample-data';
import { t } from '../../lib/i18n';
afterEach(cleanup);
async function setup() {
    render(UiPreview);
    const chat = createSampleCharacters()[0]?.histories[0];
    if (!chat) throw new Error('Missing sample conversation');
    await fireEvent.click(screen.getByRole('button', { name: `${chat.title} · ${chat.date}` }));
    const input = screen.getByRole('textbox', { name: t('uiPreview.message') });
    const field = input.closest('.ui-compose-field');
    input.focus();
    await fireEvent.focusIn(input);
    return { input, field };
}
describe('composer focus and draft', () => {
    it('collapses an empty field even when the back gesture prevents normal blur', async () => {
        const { input, field } = await setup();
        expect(field).toHaveClass('ui-compose-expanded');
        const log = screen.getByRole('log');
        log.addEventListener('pointerdown', (event) => event.preventDefault());
        await fireEvent.pointerDown(log);
        expect(field).not.toHaveClass('ui-compose-expanded');
        expect(input).not.toHaveFocus();
    });
    it('retains a nonempty draft and expansion on an outside press, then collapses after clearing', async () => {
        const { input, field } = await setup();
        await fireEvent.input(input, { target: { value: 'keep this draft' } });
        await fireEvent.pointerDown(screen.getByRole('log'));
        expect(field).toHaveClass('ui-compose-expanded');
        expect(input).toHaveValue('keep this draft');
        await fireEvent.input(input, { target: { value: '' } });
        await fireEvent.pointerDown(screen.getByRole('log'));
        expect(field).not.toHaveClass('ui-compose-expanded');
    });
});
