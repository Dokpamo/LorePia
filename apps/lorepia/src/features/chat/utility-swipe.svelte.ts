interface UtilitySwipeOptions {
    composerFullscreen(): boolean;
    desktop(): boolean;
    open(): boolean;
    openTools(): void;
}

interface UtilityOpenPointer {
    pointerId: number;
    startX: number;
    startY: number;
    lastX: number;
    lastTime: number;
    velocityX: number;
    viewportWidth: number;
}

const UTILITY_SWIPE_AXIS_LOCK_PX = 8;
const UTILITY_SWIPE_COMMIT_MIN_PX = 64;
const UTILITY_SWIPE_COMMIT_MAX_PX = 120;
const UTILITY_SWIPE_COMMIT_RATIO = 0.22;
const UTILITY_SWIPE_FLING_MIN_PX = 32;
const UTILITY_SWIPE_FLING_VELOCITY = 0.55;

export class UtilitySwipeLifecycle {
    gesture = $state<'idle' | 'tracking' | 'dragging'>('idle');

    #pointer: UtilityOpenPointer | null = null;
    #suppressOpenClickUntil = 0;

    constructor(private readonly options: UtilitySwipeOptions) {}

    pointerDown(event: PointerEvent): void {
        if (this.options.desktop() || this.options.open() || this.options.composerFullscreen()) {
            return;
        }
        if (!event.isPrimary || event.button !== 0) return;
        const target = event.currentTarget as HTMLElement;
        const boundsWidth = target.getBoundingClientRect().width;
        const viewportWidth = Math.max(1, boundsWidth || target.clientWidth || window.innerWidth);
        this.#pointer = {
            pointerId: Number.isFinite(event.pointerId) ? event.pointerId : 1,
            startX: event.clientX,
            startY: event.clientY,
            lastX: event.clientX,
            lastTime: event.timeStamp,
            velocityX: 0,
            viewportWidth,
        };
        this.gesture = 'tracking';
    }

    pointerMove(event: PointerEvent): void {
        const pointer = this.#pointer;
        if (event.pointerId !== pointer?.pointerId) return;
        const deltaX = event.clientX - pointer.startX;
        const deltaY = event.clientY - pointer.startY;
        const absoluteX = Math.abs(deltaX);
        const absoluteY = Math.abs(deltaY);
        if (this.gesture === 'tracking') {
            if (absoluteX < UTILITY_SWIPE_AXIS_LOCK_PX && absoluteY < UTILITY_SWIPE_AXIS_LOCK_PX) {
                return;
            }
            if (deltaX >= 0 || absoluteY >= absoluteX) {
                this.#reset();
                return;
            }
            if (absoluteX < absoluteY * 1.2) return;
            this.gesture = 'dragging';
            const target = event.currentTarget as HTMLElement;
            if (typeof target.setPointerCapture === 'function') {
                target.setPointerCapture(pointer.pointerId);
            }
        }
        if (this.gesture !== 'dragging') return;
        event.preventDefault();
        const elapsed = event.timeStamp - pointer.lastTime;
        if (elapsed > 0) {
            pointer.velocityX = Math.min(0, (event.clientX - pointer.lastX) / elapsed);
        }
        pointer.lastX = event.clientX;
        pointer.lastTime = event.timeStamp;
    }

    pointerUp(event: PointerEvent): void {
        const pointer = this.#pointer;
        if (event.pointerId !== pointer?.pointerId) return;
        this.#release(event);
        if (this.gesture !== 'dragging') {
            this.#reset();
            return;
        }
        event.preventDefault();
        const distance = Math.max(0, pointer.startX - event.clientX);
        const commits =
            distance >= this.#commitDistance(pointer.viewportWidth) ||
            (distance >= UTILITY_SWIPE_FLING_MIN_PX &&
                -pointer.velocityX >= UTILITY_SWIPE_FLING_VELOCITY);
        if (commits) {
            this.options.openTools();
            this.#suppressOpenClickUntil = Date.now() + 120;
        }
        this.#reset();
    }

    pointerCancel(event: PointerEvent): void {
        if (event.pointerId !== this.#pointer?.pointerId) return;
        this.#release(event);
        this.#reset();
    }

    clickCapture(event: MouseEvent): void {
        if (Date.now() > this.#suppressOpenClickUntil) return;
        this.#suppressOpenClickUntil = 0;
        event.preventDefault();
        event.stopPropagation();
    }

    #reset(): void {
        this.#pointer = null;
        this.gesture = 'idle';
    }

    #commitDistance(viewportWidth: number): number {
        return Math.min(
            UTILITY_SWIPE_COMMIT_MAX_PX,
            Math.max(UTILITY_SWIPE_COMMIT_MIN_PX, viewportWidth * UTILITY_SWIPE_COMMIT_RATIO),
        );
    }

    #release(event: PointerEvent): void {
        const target = event.currentTarget as HTMLElement;
        if (
            typeof target.hasPointerCapture === 'function' &&
            typeof target.releasePointerCapture === 'function' &&
            target.hasPointerCapture(event.pointerId)
        ) {
            target.releasePointerCapture(event.pointerId);
        }
    }
}
