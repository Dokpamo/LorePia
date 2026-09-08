import { cleanup, fireEvent, render, screen, within } from '@testing-library/svelte';
import { afterEach, describe, expect, it, vi } from 'vitest';
import { t } from '../../lib/i18n';
import { styleRules } from '../../tests/css-rules';
import DiscardChanges from './DiscardChanges.svelte';
import feedback from './ui-feedback.css?raw';
import layout from './ui-layout.css?raw';
import choices from './ui-choices.css?raw';
import cards from './ui-cards.css?raw';

const rules = styleRules([feedback, layout, choices, cards].join('\n'));
const rule = (selector: string) => rules.find((item) => item.selector === selector)?.declarations;
afterEach(cleanup);

describe('unsaved changes actions', () => {
    it('keeps both choices in one action group and preserves keyboard and click behavior', async () => {
        const onkeep = vi.fn();
        const ondiscard = vi.fn();
        render(DiscardChanges, { onkeep, ondiscard });
        const dialog = screen.getByRole('alertdialog');
        const keep = within(dialog).getByRole('button', { name: t('uiPreview.keepEditing') });
        const discard = within(dialog).getByRole('button', { name: t('uiPreview.discardChanges') });
        expect(keep).toHaveFocus();
        expect(keep.parentElement).toBe(discard.parentElement);
        expect(keep.parentElement).toHaveClass('ui-confirm-actions');
        expect(within(dialog).getAllByRole('button')).toEqual([discard, keep]);
        await fireEvent.keyDown(keep, { key: 'Tab' });
        expect(discard).toHaveFocus();
        await fireEvent.keyDown(discard, { key: 'Escape' });
        expect(onkeep).toHaveBeenCalledTimes(1);
        await fireEvent.click(keep);
        expect(onkeep).toHaveBeenCalledTimes(2);
        await fireEvent.click(discard);
        expect(ondiscard).toHaveBeenCalledOnce();
    });

    it('keeps equal horizontal action widths with text wrapping inside each half', () => {
        expect(rule('.ui-preview .ui-confirm-actions')).toMatchObject({
            display: 'grid',
            'grid-template-columns': 'repeat(2, minmax(0, 1fr))',
            gap: '12px',
        });
        expect(
            rule('.ui-preview .ui-confirm-actions > .ui-discard > .ui-press-visual'),
        ).toMatchObject({
            background: 'var(--ui-soft)',
            color: 'var(--ui-secondary)',
        });
        expect(rule('.ui-preview .ui-confirm-actions > button')).toMatchObject({
            margin: '0',
            'min-width': '0',
        });
        expect(rule('.ui-preview .ui-confirm-actions > button > .ui-press-visual')).toMatchObject({
            'min-height': 'var(--ui-control)',
            padding: '8px 16px',
        });
    });

    it('puts horizontal breathing room on the gray press surface of text rows', () => {
        for (const selector of [
            '.ui-preview .ui-card-info-link > span',
            '.ui-preview .ui-choice-field > .ui-press-visual',
            '.ui-preview .ui-organize-row > .ui-press-visual',
        ])
            expect(rule(selector), selector).toMatchObject({
                padding: 'var(--ui-space-2) var(--ui-space-4)',
            });
        expect(rule('.ui-preview .ui-history-content')?.padding).toBe('var(--ui-space-2)');
        expect(rule('.ui-preview .ui-card-info-link')?.margin).toBe('0');
        for (const selector of [
            '.ui-preview .ui-settings-group .ui-choice-field',
            '.ui-preview .ui-settings-group .ui-organize-row',
        ])
            expect(rule(selector)).toMatchObject({
                width: 'calc(100% + 2 * var(--ui-space-4))',
                'margin-inline': 'calc(-1 * var(--ui-space-4))',
            });
        expect(rule('.ui-preview .ui-history-item')?.padding).toBe('0');
        expect(rule('.ui-preview .ui-card-info-link')?.padding).toBe('0');
    });
});
