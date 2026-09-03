import { cleanup, fireEvent, render, screen, waitFor } from '@testing-library/svelte';
import { afterEach, describe, expect, it, vi } from 'vitest';

import ChoicePopover from './ChoicePopover.svelte';
import SegmentedControl from './SegmentedControl.svelte';
import ToggleSwitch from './ToggleSwitch.svelte';

afterEach(() => cleanup());

describe('ChoicePopover', () => {
    it('can close an open menu after the owning action disables the trigger', async () => {
        const onSelect = vi.fn();
        const options = [
            { value: 'off', label: 'Off' },
            { value: 'light', label: 'Light' },
        ];
        const rendered = render(ChoicePopover, {
            id: 'runtime-mode',
            label: 'Runtime mode',
            value: 'off',
            options,
            onSelect,
        });

        await fireEvent.click(screen.getByRole('combobox', { name: 'Runtime mode: Off' }));
        await rendered.rerender({
            id: 'runtime-mode',
            label: 'Runtime mode',
            value: 'off',
            options,
            onSelect,
            disabled: true,
        });
        await fireEvent.click(screen.getByRole('option', { name: 'Light' }));

        expect(onSelect).toHaveBeenCalledWith('light');
        await waitFor(() => expect(screen.queryByRole('listbox')).not.toBeInTheDocument());
    });
});

describe('SegmentedControl', () => {
    const options = [
        { value: 'chat', label: 'Chat' },
        { value: 'story', label: 'Story' },
    ];

    it('moves one shared thumb by changing only the selected index', async () => {
        const onSelect = vi.fn();
        const rendered = render(SegmentedControl, {
            id: 'conversation-mode',
            label: 'Conversation mode',
            value: 'chat',
            options,
            onSelect,
        });
        const group = screen.getByRole('radiogroup', { name: 'Conversation mode' });

        expect(group.querySelectorAll('.segmented-control-thumb')).toHaveLength(1);
        expect(group.style.getPropertyValue('--segment-index')).toBe('0');
        expect(screen.getByRole('radio', { name: 'Chat' })).toHaveAttribute('aria-checked', 'true');

        await fireEvent.click(screen.getByRole('radio', { name: 'Story' }));
        expect(onSelect).toHaveBeenCalledWith('story');

        await rendered.rerender({
            id: 'conversation-mode',
            label: 'Conversation mode',
            value: 'story',
            options,
            onSelect,
        });
        expect(group.style.getPropertyValue('--segment-index')).toBe('1');
        expect(screen.getByRole('radio', { name: 'Story' })).toHaveAttribute(
            'aria-checked',
            'true',
        );
    });

    it('supports arrow-key selection without changing the control width', async () => {
        const onSelect = vi.fn();
        render(SegmentedControl, {
            id: 'response-length',
            label: 'Response length',
            value: 'chat',
            options,
            onSelect,
        });
        const chat = screen.getByRole('radio', { name: 'Chat' });
        chat.focus();

        await fireEvent.keyDown(chat, { key: 'ArrowRight' });

        expect(onSelect).toHaveBeenCalledWith('story');
        expect(screen.getByRole('radio', { name: 'Story' })).toHaveFocus();
    });
});

describe('ToggleSwitch', () => {
    it('keeps one track and translates one thumb between boolean states', async () => {
        const onChange = vi.fn();
        const rendered = render(ToggleSwitch, {
            label: 'Preserve partial response',
            checked: false,
            onChange,
        });
        const control = screen.getByRole('switch', { name: 'Preserve partial response' });

        expect(control.querySelectorAll('.toggle-switch-track')).toHaveLength(1);
        expect(control.querySelectorAll('.toggle-switch-thumb')).toHaveLength(1);
        expect(control).toHaveAttribute('aria-checked', 'false');

        await fireEvent.click(control);
        expect(onChange).toHaveBeenCalledWith(true);

        await rendered.rerender({
            label: 'Preserve partial response',
            checked: true,
            onChange,
        });
        expect(control).toHaveAttribute('aria-checked', 'true');
    });
});
