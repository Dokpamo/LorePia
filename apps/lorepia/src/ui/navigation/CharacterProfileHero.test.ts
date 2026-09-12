import { cleanup, fireEvent, render, screen } from '@testing-library/svelte';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import { t } from '../../lib/i18n';
import { createPreviewClient } from '../../preview/mock-client';
import CharacterProfileHero from './CharacterProfileHero.svelte';

beforeEach(() =>
    vi.stubGlobal(
        'ResizeObserver',
        class {
            observe = vi.fn();
            disconnect = vi.fn();
            unobserve = vi.fn();
        },
    ),
);
afterEach(() => {
    cleanup();
    vi.unstubAllGlobals();
});

it('keeps swipes, keyboard and original-image viewing available without a thumbnail strip', async () => {
    const images = Array.from({ length: 29 }, (_, i) => ({
        assetId: `asset-${String(i)}`,
        title: `Image ${String(i + 1)}`,
    }));
    const onview = vi.fn();
    const { container } = render(CharacterProfileHero, {
        images,
        client: createPreviewClient(),
        onview,
        ondetails: vi.fn(),
        character: {
            id: 'character',
            name: 'Example',
            description: '',
            thumbnail: 'A',
            subpage: false,
            histories: [],
        },
    });
    const image = (number: number) =>
        screen.getByRole('button', {
            name: t('navigation.imageSelectItem', { name: `Image ${String(number)}` }),
        });
    expect(screen.queryByRole('toolbar')).toBeNull();
    await fireEvent.keyDown(image(1), { key: 'ArrowRight' });
    expect(image(2)).toHaveFocus();
    await fireEvent.keyDown(image(2), { key: 'ArrowRight' });
    const third = image(3);
    expect(third).toHaveFocus();
    await fireEvent.click(third);
    expect(onview).toHaveBeenCalledExactlyOnceWith('asset-2', third);
    await fireEvent.keyDown(third, { key: 'End' });
    expect(image(29)).toHaveFocus();
    await fireEvent.keyDown(image(29), { key: 'ArrowLeft' });
    expect(image(28)).toHaveFocus();
    await fireEvent.keyDown(image(28), { key: 'Home' });
    expect(image(1)).toHaveFocus();

    const carousel = container.querySelector<HTMLElement>('.seed-profile-carousel');
    if (!carousel) throw new Error('Missing carousel');
    Object.defineProperty(carousel, 'clientWidth', { value: 394 });
    carousel.setPointerCapture = vi.fn();
    carousel.hasPointerCapture = () => true;
    carousel.releasePointerCapture = vi.fn();
    const pointer = async (type: string, x: number, time: number) => {
        const event = new Event(type, { bubbles: true, cancelable: true });
        Object.defineProperties(event, {
            pointerId: { value: 1 },
            isPrimary: { value: true },
            button: { value: 0 },
            clientX: { value: x },
            clientY: { value: 100 },
            timeStamp: { value: time },
        });
        await fireEvent(carousel, event);
    };
    await pointer('pointerdown', 300, 0);
    await pointer('pointermove', 100, 160);
    await pointer('pointerup', 100, 200);
    expect(image(2)).toBeVisible();

    await fireEvent.keyDown(image(2), { key: 'End' });
    const jump = carousel.querySelector('.seed-profile-carousel-jump');
    if (jump) await fireEvent.animationEnd(jump);
    await pointer('pointerdown', 300, 500);
    await pointer('pointermove', 100, 650);
    const track = carousel.querySelector<HTMLElement>('.seed-profile-carousel-track');
    const blur = container.querySelector<HTMLElement>('.seed-profile-blur');
    const offset = Number(blur?.style.transform.match(/translateX\(([-.\d]+)px\)/)?.[1]);
    expect(offset).toBeLessThan(0);
    expect(offset).toBeGreaterThan(-40);
    expect(track?.style.transform).toContain(`${String(offset)}px`);
    expect(
        [...carousel.querySelectorAll('.seed-profile-image')].every((photo) =>
            track?.contains(photo),
        ),
    ).toBe(true);
    await pointer('pointerup', 100, 700);
    expect(blur?.style.transform).toBe('translateX(0px)');
    expect(image(29)).toBeVisible();
});
