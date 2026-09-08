import { afterEach, describe, expect, it } from 'vitest';
import { editorMorph, measureEditorOrigin } from './editor-morph';

afterEach(() => document.body.replaceChildren());
function setup(scale = 1) {
    const layer = document.createElement('div');
    const surface = document.createElement('div');
    const text = document.createElement('span');
    surface.append(text);
    document.body.append(surface, layer);
    Object.defineProperty(layer, 'clientWidth', { value: 360 });
    layer.getBoundingClientRect = () => new DOMRect(0, 0, 360 * scale, 760 * scale);
    surface.getBoundingClientRect = () =>
        new DOMRect(12 * scale, 684 * scale, 336 * scale, 64 * scale);
    surface.style.borderTopLeftRadius = '20px';
    return { layer, origin: { surface, text, leading: null, trailing: null } };
}
describe('composer-to-editor geometry', () => {
    it('aligns the first text baseline using padding and the scrolled source position', () => {
        const { layer, origin } = setup();
        const input = document.createElement('textarea');
        layer.append(input);
        origin.text.style.padding = '8px 8px 0';
        origin.text.scrollTop = 26;
        origin.text.getBoundingClientRect = () => new DOMRect(20, 620, 320, 264);
        input.style.padding = '0';
        input.getBoundingClientRect = () => new DOMRect(24, 84, 312, 600);
        editorMorph(layer, () => origin)({ direction: 'in' });
        expect(input.style.getPropertyValue('--ui-origin-x')).toBe('4px');
        expect(input.style.getPropertyValue('--ui-origin-y')).toBe('518px');
        expect(input.style.getPropertyValue('--ui-origin-text-width')).toBe('304px');
    });
    it('maps the actual composer rectangle into the uniformly scaled window', () => {
        for (const scale of [1, 320 / 360]) {
            const { layer, origin } = setup(scale);
            const measured = measureEditorOrigin(layer, origin);
            expect(measured?.top).toBeCloseTo(684);
            expect(measured?.left).toBeCloseTo(12);
            expect(measured?.right).toBeCloseTo(12);
            expect(measured?.bottom).toBeCloseTo(12);
            expect(measured?.radius).toBe(20);
        }
    });
    it('begins at the input bar and finishes full-bleed, remeasuring the return destination', () => {
        const { layer, origin } = setup();
        const transition = editorMorph(layer, () => origin);
        const enter = transition({ direction: 'in' });
        expect(enter.duration).toBe(420);
        expect(enter.css?.(0, 1)).toContain('inset(684px 12px 12px 12px round 20px)');
        expect(enter.css?.(1, 0)).toContain('inset(0px 0px 0px 0px round 0px)');
        origin.surface.getBoundingClientRect = () => new DOMRect(12, 608, 336, 140);
        const leave = transition({ direction: 'out' });
        expect(leave.duration).toBe(360);
        expect(leave.css?.(0, 1)).toContain('inset(608px 12px 12px 12px round 20px)');
    });
    it('does not play a second closing morph after interactive back has completed', () => {
        const { layer, origin } = setup();
        const editor = document.createElement('div');
        editor.dataset.backDismissed = 'true';
        layer.append(editor);
        expect(editorMorph(layer, () => origin)({ direction: 'out' }).duration).toBe(0);
    });
});
