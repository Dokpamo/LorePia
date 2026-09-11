import { cleanup, render } from '@testing-library/svelte';
import { afterEach, describe, expect, it } from 'vitest';
import EditField from './EditField.svelte';
import { styleRules } from '../../tests/css-rules';
import editing from './ui-editing.css?raw';
import layout from './ui-layout.css?raw';
import choices from './ui-choices.css?raw';

const rules = styleRules([editing, layout, choices].join('\n'));
const declarations = (selector: string) =>
    rules.find((rule) => rule.selector === `.ui-preview ${selector}`)?.declarations;

afterEach(cleanup);

describe('button visual surfaces', () => {
    it('keeps the edit-field label and arrow inside one scalable visual', () => {
        const { container } = render(EditField, {
            label: 'Title',
            value: 'Current field',
            maxlength: 100,
            onchange: () => undefined,
        });
        const field = container.querySelector('.ui-edit-field');
        expect(field).not.toBeNull();
        expect(field?.children).toHaveLength(1);
        const visual = field?.firstElementChild;
        expect(visual).toHaveClass('ui-press-visual');
        expect(visual?.querySelector('.ui-edit-field-copy')).not.toBeNull();
        expect(visual?.querySelector(':scope > svg')).not.toBeNull();
    });

    it('retains resting field and search geometry on the scalable surface', () => {
        expect(declarations('.ui-edit-field')).toMatchObject({
            width: '100%',
            background: 'transparent',
            padding: '0',
        });
        expect(declarations('.ui-edit-field > .ui-press-visual')).toMatchObject({
            'min-height': 'var(--ui-row)',
            padding: 'var(--ui-space-4)',
            gap: 'var(--ui-space-4)',
            background: 'var(--ui-paper)',
        });
        expect(declarations('.ui-find-chat')).toMatchObject({
            'min-height': 'var(--ui-control)',
            background: 'transparent',
            padding: '0',
        });
        expect(declarations('.ui-find-chat > span')).toMatchObject({
            'min-height': 'var(--ui-control)',
            padding: '0 var(--ui-space-3)',
            background: 'var(--ui-soft)',
        });
    });

    it('scales selected choice fill while retaining the independent history selection', () => {
        expect(declarations(".ui-choice-option[aria-checked='true']")?.background).toBeUndefined();
        expect(
            declarations(".ui-choice-option[aria-checked='true'] > .ui-press-visual"),
        ).toMatchObject({ background: 'var(--ui-selected)' });
        expect(declarations(".ui-history-row[data-current='true']")).toMatchObject({
            background: 'var(--ui-selected)',
        });
    });
});
