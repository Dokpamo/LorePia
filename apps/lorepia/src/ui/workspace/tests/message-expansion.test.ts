import { afterEach, expect, it, vi } from 'vitest';
import { messageExpansion } from '../message-expansion';

afterEach(() => {
    document.body.replaceChildren();
    vi.unstubAllGlobals();
});

it('follows intrinsic tool height and preserves it through virtual removal until remount', () => {
    let notify = () => undefined;
    const observe = vi.fn();
    const disconnect = vi.fn();
    vi.stubGlobal(
        'ResizeObserver',
        class {
            constructor(callback: () => undefined) {
                notify = callback;
            }
            observe = observe;
            disconnect = disconnect;
        },
    );
    const list = document.createElement('div');
    const row = document.createElement('article');
    row.dataset.messageId = 'selected';
    const tools = document.createElement('div');
    tools.className = 'ui-message-tools';
    let height = 52;
    Object.defineProperty(tools, 'offsetHeight', { get: () => height });
    row.append(tools);
    list.append(row);
    document.body.append(list);
    const layout = messageExpansion(list, { active: 'selected', start: 0, end: 80 });
    expect(list.style.getPropertyValue('--ui-tools-height')).toBe('52px');
    height = 72;
    notify();
    expect(list.style.getPropertyValue('--ui-tools-height')).toBe('72px');

    row.remove();
    height = 0;
    notify();
    layout.update({ active: 'selected', start: 80, end: 160 });
    expect(list.style.getPropertyValue('--ui-tools-height')).toBe('72px');
    list.append(row);
    height = 68;
    layout.update({ active: 'selected', start: 0, end: 80 });
    expect(list.style.getPropertyValue('--ui-tools-height')).toBe('68px');
    layout.destroy();
    expect(disconnect).toHaveBeenCalled();
    expect(list.style.getPropertyValue('--ui-tools-height')).toBe('');
});
