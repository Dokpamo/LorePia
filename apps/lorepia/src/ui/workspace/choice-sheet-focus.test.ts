import { cleanup, render, screen } from '@testing-library/svelte';
import { afterEach, expect, it, vi } from 'vitest';
import ChoiceSheet from './ChoiceSheet.svelte';
import { focusChoiceOption } from './choice-sheet-focus';

afterEach(() => {
    cleanup();
    document.body.replaceChildren();
    vi.restoreAllMocks();
});

it('reveals choices inside the list without moving the page or sheet during entry', () => {
    const page = document.createElement('div');
    const sheet = document.createElement('div');
    const list = document.createElement('div');
    const option = document.createElement('button');
    list.setAttribute('role', 'radiogroup');
    list.style.position = 'relative';
    list.style.scrollPaddingTop = '4px';
    sheet.style.transform = 'translateY(600px) scale(0.8)';
    list.append(option);
    sheet.append(list);
    page.append(sheet);
    document.body.append(page);
    page.scrollTop = 38;
    Object.defineProperty(list, 'clientHeight', { value: 200 });
    Object.defineProperty(option, 'offsetHeight', { value: 56 });
    let top = 800;
    Object.defineProperty(option, 'offsetTop', { get: () => top });

    focusChoiceOption(option);
    expect(option).toHaveFocus();
    expect(list.scrollTop).toBe(660);
    top = 80;
    focusChoiceOption(option);
    expect(list.scrollTop).toBe(76);
    // A visible row does not reset the user's existing list position.
    top = 120;
    focusChoiceOption(option);
    expect(list.scrollTop).toBe(76);
    expect(page.scrollTop).toBe(38);
    expect(sheet.scrollTop).toBe(0);
});

it('focuses the selected option without letting the browser scroll the entering sheet', () => {
    const focus = vi.spyOn(HTMLElement.prototype, 'focus');
    render(ChoiceSheet, {
        request: {
            label: 'Start scene',
            value: 'second',
            options: [
                { value: 'first', label: 'First scene' },
                { value: 'second', label: 'Second scene' },
            ],
            onselect: vi.fn(),
        },
        onclose: vi.fn(),
    });
    expect(screen.getByRole('radio', { name: 'Second scene' })).toHaveFocus();
    expect(focus).toHaveBeenCalledWith({ preventScroll: true });
});
