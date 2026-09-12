import { tick } from 'svelte';
import { afterEach, expect, it, vi } from 'vitest';
import { observeTranscriptUnderfill } from './transcript-underfill';

afterEach(() => vi.unstubAllGlobals());
function fixture() {
    let resize: (target: Element) => void = vi.fn();
    const observed = new Set<Element>();
    const disconnect = vi.fn(() => observed.clear());
    const observe = vi.fn((target: Element) => observed.add(target));
    vi.stubGlobal(
        'ResizeObserver',
        class {
            constructor(callback: ResizeObserverCallback) {
                resize = (target) => {
                    if (observed.has(target)) callback([], this as unknown as ResizeObserver);
                };
            }
            observe = observe;
            disconnect = disconnect;
        },
    );
    const node = document.createElement('div');
    const content = document.createElement('div');
    content.className = 'ui-transcript-list';
    node.append(content);
    let height = 600;
    let viewport = 900;
    Object.defineProperties(node, {
        scrollHeight: { get: () => height },
        clientHeight: { get: () => viewport },
    });
    const check = vi.fn();
    const options = {
        scope: 'conversation:branch',
        start: 1970,
        count: 30,
        first: 'm-1970',
        last: 'm-1999',
        busy: false,
        check,
    };
    const action = observeTranscriptUnderfill(node, options);
    return {
        node,
        content,
        action,
        options,
        check,
        disconnect,
        observe,
        resize: (target: Element = node) => resize(target),
        height: (value: number) => {
            height = value;
        },
        viewport: (value: number) => {
            viewport = value;
        },
    };
}
it('loads only growing pages until scrollable and resumes when resizing exposes empty space', async () => {
    const f = fixture();
    await tick();
    expect(f.check).toHaveBeenCalledOnce();
    f.height(1200);
    f.action.update({ ...f.options, start: 1940, first: 'm-1940', count: 45 });
    await tick();
    expect(f.check).toHaveBeenCalledOnce();
    f.viewport(1500);
    f.resize();
    f.resize();
    await tick();
    expect(f.check).toHaveBeenCalledTimes(2);
    f.height(1800);
    f.action.update({ ...f.options, start: 1910, first: 'm-1910', count: 90 });
    await tick();
    expect(f.check).toHaveBeenCalledTimes(2);
    f.action.destroy();
    expect(f.observe).toHaveBeenCalledWith(f.node);
    expect(f.disconnect).toHaveBeenCalledOnce();
});
it('does not retry unchanged failures or chase capped pages without layout progress', async () => {
    const f = fixture();
    await tick();
    f.action.update({ ...f.options, busy: true });
    await tick();
    f.action.update(f.options);
    f.resize();
    await tick();
    expect(f.check).toHaveBeenCalledOnce();
    f.height(700);
    f.action.update({ ...f.options, start: 1910, first: 'm-1910', count: 45 });
    await tick();
    expect(f.check).toHaveBeenCalledTimes(2);
    f.action.update({ ...f.options, start: 1880, first: 'm-1880', last: 'm-1969', count: 45 });
    f.resize();
    await tick();
    expect(f.check).toHaveBeenCalledTimes(2);
    f.action.destroy();
});
it('skips loading, hidden and oldest windows, resets branch attempts and cancels queued work on unmount', async () => {
    const f = fixture();
    f.action.update({ ...f.options, busy: true });
    await tick();
    expect(f.check).not.toHaveBeenCalled();
    f.viewport(0);
    f.action.update(f.options);
    await tick();
    expect(f.check).not.toHaveBeenCalled();
    f.viewport(900);
    f.action.update({ ...f.options, start: 0 });
    await tick();
    expect(f.check).not.toHaveBeenCalled();
    f.action.update(f.options);
    await tick();
    expect(f.check).toHaveBeenCalledOnce();
    f.action.update({ ...f.options, scope: 'conversation:next' });
    await tick();
    expect(f.check).toHaveBeenCalledTimes(2);
    f.action.update({ ...f.options, scope: 'conversation:third' });
    f.action.destroy();
    f.resize();
    await tick();
    expect(f.check).toHaveBeenCalledTimes(2);
});

it('rechecks an initially oversized virtual estimate when only the inner list shrinks', async () => {
    const f = fixture();
    expect(f.observe).toHaveBeenCalledWith(f.content, { box: 'border-box' });
    f.height(2000);
    await tick();
    expect(f.check).not.toHaveBeenCalled();
    f.height(600);
    f.resize(f.content);
    await tick();
    expect(f.check).toHaveBeenCalledOnce();
    f.action.destroy();
    expect(f.disconnect).toHaveBeenCalledOnce();
    f.resize(f.content);
    await tick();
    expect(f.check).toHaveBeenCalledOnce();
});

it('stops before another page would exceed the DOM or retained cap, even while measured height increases', async () => {
    const f = fixture();
    f.viewport(6000);
    await tick();
    f.height(1200);
    f.action.update({ ...f.options, start: 1940, first: 'm-1940', count: 60 });
    await tick();
    expect(f.check).toHaveBeenCalledOnce();
    f.height(1800);
    const capped = { ...f.options, start: 1910, first: 'm-1910', count: 90 };
    f.action.update(capped);
    await tick();
    for (const height of [2000, 2400, 3000]) {
        f.height(height);
        f.resize(f.content);
        await tick();
    }
    expect(f.check).toHaveBeenCalledOnce();
    f.action.update({ ...capped, start: 1880, first: 'm-1880', last: 'm-1969' });
    await tick();
    expect(f.check).toHaveBeenCalledOnce();
    f.action.destroy();
});
