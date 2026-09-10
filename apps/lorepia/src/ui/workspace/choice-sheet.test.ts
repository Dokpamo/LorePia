import { tick } from 'svelte';
import { afterEach, expect, it, vi } from 'vitest';
import { ChoiceSheetState } from './choice-sheet.svelte';

afterEach(() => {
    document.body.replaceChildren();
    vi.unstubAllGlobals();
});

it('restores the selector after the covered page resumes, without stealing focus from a new sheet', async () => {
    const frames: FrameRequestCallback[] = [];
    vi.stubGlobal('requestAnimationFrame', (callback: FrameRequestCallback) =>
        frames.push(callback),
    );
    const opener = document.createElement('button');
    const back = document.createElement('button');
    const current = document.createElement('button');
    document.body.append(opener, back, current);
    const choices = new ChoiceSheetState();
    const request = {
        label: 'Language',
        value: 'ko',
        options: [{ value: 'ko', label: 'Korean' }],
        onselect: vi.fn(),
    };
    choices.open(request, opener);
    choices.close();
    choices.finish();
    await tick();
    back.focus();
    expect(frames).toHaveLength(1);
    frames.shift()?.(0);
    expect(opener).toHaveFocus();

    choices.open(request, opener);
    choices.close();
    choices.finish();
    await tick();
    choices.open(request, current);
    current.focus();
    frames.shift()?.(0);
    expect(current).toHaveFocus();
});

it('opens a follow-up only after the sheet exits and does not restore focus over it', async () => {
    const opener = document.createElement('button');
    const next = document.createElement('button');
    document.body.append(opener, next);
    const choices = new ChoiceSheetState();
    choices.open({ label: 'Personas', value: '', options: [], onselect: vi.fn() }, opener);
    const followup = vi.fn(() => next.focus());
    choices.close(followup);
    expect(followup).not.toHaveBeenCalled();
    expect(choices.present).toBe(true);
    choices.finish();
    await tick();
    expect(followup).toHaveBeenCalledOnce();
    expect(next).toHaveFocus();
});
