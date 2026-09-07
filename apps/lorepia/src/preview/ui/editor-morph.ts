import { cubicOut, cubicInOut, quintOut } from 'svelte/easing';
import { fly, type TransitionConfig } from 'svelte/transition';

export interface EditorOrigin {
    surface: HTMLElement;
    text: HTMLElement;
    leading: HTMLElement | null;
    trailing: HTMLElement | null;
}

/** All measurements use the same logical space, including sub-360px scaling. */
export function measureEditorOrigin(layer: HTMLElement, origin: EditorOrigin) {
    const bounds = layer.getBoundingClientRect();
    const source = origin.surface.getBoundingClientRect();
    const scale = bounds.width / (layer.clientWidth || bounds.width);
    if (!bounds.width || !source.width || !scale) return null;
    return {
        top: Math.max(0, source.top - bounds.top) / scale,
        right: Math.max(0, bounds.right - source.right) / scale,
        bottom: Math.max(0, bounds.bottom - source.bottom) / scale,
        left: Math.max(0, source.left - bounds.left) / scale,
        radius: Number.parseFloat(getComputedStyle(origin.surface).borderTopLeftRadius) || 0,
        scale,
    };
}

function align(
    source: HTMLElement | null,
    target: HTMLElement | null,
    scale: number,
    center: boolean,
) {
    if (!source || !target) return;
    const previous = target.style.transform;
    target.style.transform = 'none';
    const a = source.getBoundingClientRect();
    const b = target.getBoundingClientRect();
    const sourceStyle = getComputedStyle(source);
    const targetStyle = getComputedStyle(target);
    const paddingX = center
        ? 0
        : (Number.parseFloat(sourceStyle.paddingLeft) || 0) -
          (Number.parseFloat(targetStyle.paddingLeft) || 0);
    const paddingY = center
        ? 0
        : (Number.parseFloat(sourceStyle.paddingTop) || 0) -
          (Number.parseFloat(targetStyle.paddingTop) || 0) -
          source.scrollTop +
          target.scrollTop;
    target.style.setProperty(
        '--ui-origin-x',
        `${String((a.left - b.left + (center ? (a.width - b.width) / 2 : 0)) / scale + paddingX)}px`,
    );
    target.style.setProperty(
        '--ui-origin-y',
        `${String((a.top - b.top + (center ? (a.height - b.height) / 2 : 0)) / scale + paddingY)}px`,
    );
    target.style.transform = previous;
}

/** Retains the original composer's field/text/control correspondence, without its theme. */
export function editorMorph(layer: HTMLElement, source?: () => EditorOrigin) {
    return ({ direction }: { direction: 'in' | 'out' } = { direction: 'in' }): TransitionConfig => {
        const reduced = window.matchMedia('(prefers-reduced-motion: reduce)').matches;
        if (direction === 'out' && layer.querySelector('[data-back-dismissed="true"]'))
            return { duration: 0 };
        if (typeof source !== 'function')
            return fly(layer, { y: 20, duration: reduced ? 0 : 200, easing: cubicOut });
        const origin = source();
        const geometry = measureEditorOrigin(layer, origin);
        if (!geometry) return { duration: 0 };
        origin.surface.dataset.uiEditorOrigin = 'true';
        align(origin.leading, layer.querySelector('[data-editor-back]'), geometry.scale, true);
        align(origin.trailing, layer.querySelector('[data-editor-send]'), geometry.scale, true);
        const input = layer.querySelector('textarea');
        align(origin.text, input, geometry.scale, false);
        if (input) {
            const style = getComputedStyle(origin.text);
            input.style.setProperty('--ui-origin-font', style.fontSize);
            input.style.setProperty('--ui-origin-line', style.lineHeight);
        }
        return {
            duration: reduced ? 0 : direction === 'in' ? 420 : 360,
            easing: direction === 'in' ? quintOut : cubicInOut,
            css: (progress) => {
                const p = 1 - progress;
                return `--ui-editor-progress:${String(progress)}; clip-path: inset(${String(geometry.top * p)}px ${String(geometry.right * p)}px ${String(geometry.bottom * p)}px ${String(geometry.left * p)}px round ${String(geometry.radius * p)}px);`;
            },
        };
    };
}
