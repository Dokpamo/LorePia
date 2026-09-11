import { afterEach, beforeEach, expect, it } from 'vitest';
import { pressFeedback } from './press-feedback';

let root: HTMLDivElement;
let button: HTMLButtonElement;
let image: HTMLImageElement;
let feedback: ReturnType<typeof pressFeedback>;

beforeEach(() => {
    root = document.createElement('div');
    root.innerHTML =
        '<button class="ui-pressable" data-press-feedback="scale"><span class="ui-press-visual"><img alt="Photo"></span></button>';
    document.body.append(root);
    const photoButton = root.querySelector('button');
    const photo = root.querySelector('img');
    if (!photoButton || !photo) throw new Error('Missing photo card');
    button = photoButton;
    image = photo;
    Object.defineProperties(button, {
        offsetWidth: { value: 120 },
        offsetHeight: { value: 150 },
    });
    feedback = pressFeedback(root);
});

afterEach(() => {
    feedback.destroy();
    root.remove();
});

function pointer(target: EventTarget, type: string, x = 100, id = 1) {
    const event = new MouseEvent(type, {
        bubbles: true,
        cancelable: true,
        clientX: x,
        clientY: 100,
    });
    Object.defineProperties(event, {
        pointerId: { value: id },
        isPrimary: { value: true },
    });
    target.dispatchEvent(event);
}

it('presses the whole photo card even when its image gesture prevents the native default', () => {
    image.addEventListener('pointerdown', (event) => {
        event.preventDefault();
        event.stopPropagation();
    });
    pointer(image, 'pointerdown');
    expect(button).toHaveAttribute('data-ui-pressed', 'true');
    expect(Number(button.style.getPropertyValue('--ui-press-scale'))).toBeCloseTo(148 / 150);
    root.dispatchEvent(new FocusEvent('focusout', { bubbles: true }));
    expect(button).toHaveAttribute('data-ui-pressed', 'true');
    pointer(window, 'pointerup');
    expect(button).not.toHaveAttribute('data-ui-pressed');
});

it('releases the visual when a captured drag starts and does not press again mid-gesture', () => {
    image.addEventListener('pointermove', (event) => event.stopPropagation());
    pointer(image, 'pointerdown');
    pointer(image, 'pointermove', 103);
    expect(button).toHaveAttribute('data-ui-pressed', 'true');
    pointer(image, 'pointermove', 106);
    expect(button).not.toHaveAttribute('data-ui-pressed');
    expect(button).toHaveAttribute('data-ui-press-cancelled');
    pointer(image, 'pointermove', 100);
    expect(button).toHaveAttribute('data-ui-press-cancelled');
    pointer(window, 'pointerup');
    expect(button).not.toHaveAttribute('data-ui-press-cancelled');
    pointer(image, 'pointerdown');
    expect(button).toHaveAttribute('data-ui-pressed', 'true');
});

it('ignores other pointers and cleans up interrupted gestures', () => {
    pointer(image, 'pointerdown');
    pointer(window, 'pointermove', 120, 2);
    pointer(window, 'pointerup', 120, 2);
    expect(button).toHaveAttribute('data-ui-pressed', 'true');
    pointer(image, 'lostpointercapture');
    expect(button).not.toHaveAttribute('data-ui-pressed');
    expect(button).toHaveAttribute('data-ui-press-cancelled');
    pointer(window, 'pointercancel');
    expect(button).not.toHaveAttribute('data-ui-press-cancelled');
    pointer(image, 'pointerdown');
    window.dispatchEvent(new Event('blur'));
    expect(button).not.toHaveAttribute('data-ui-pressed');
});

it('preserves keyboard feedback and removes state and listeners on teardown', () => {
    button.dispatchEvent(new KeyboardEvent('keydown', { key: ' ', bubbles: true }));
    expect(button).toHaveAttribute('data-ui-pressed', 'true');
    window.dispatchEvent(new KeyboardEvent('keyup', { key: ' ' }));
    expect(button).not.toHaveAttribute('data-ui-pressed');
    pointer(image, 'pointerdown');
    feedback.destroy();
    expect(button).not.toHaveAttribute('data-ui-pressed');
    pointer(image, 'pointerdown');
    expect(button).not.toHaveAttribute('data-ui-pressed');
});

it('leaves normal pointer buttons to native feedback and ignores disabled cards', () => {
    button.disabled = true;
    pointer(image, 'pointerdown');
    expect(button).not.toHaveAttribute('data-ui-pressed');
    button.disabled = false;
    delete button.dataset.pressFeedback;
    pointer(image, 'pointerdown');
    expect(button).not.toHaveAttribute('data-ui-pressed');
    expect(Number(button.style.getPropertyValue('--ui-press-scale'))).toBeLessThan(1);
});
