import '@testing-library/jest-dom/vitest';

import { performPortableRegexOperation } from '../features/chat/portable-regex-operation';
import { setPortableRegexWorkerFactoryForTests } from '../features/chat/portable-regex';

setPortableRegexWorkerFactoryForTests(() => {
    let messageListener: EventListenerOrEventListenerObject | null = null;
    return {
        addEventListener: (type: string, listener: EventListenerOrEventListenerObject) => {
            if (type === 'message') messageListener = listener;
        },
        removeEventListener: (type: string, listener: EventListenerOrEventListenerObject) => {
            if (type === 'message' && messageListener === listener) messageListener = null;
        },
        postMessage: (value: unknown) => {
            const request = value as {
                id: string;
                request: Parameters<typeof performPortableRegexOperation>[0];
            };
            queueMicrotask(() => {
                const event = {
                    data: {
                        id: request.id,
                        result: performPortableRegexOperation(request.request),
                    },
                } as MessageEvent;
                if (typeof messageListener === 'function') messageListener(event);
                else messageListener?.handleEvent(event);
            });
        },
        terminate: () => undefined,
    } as unknown as Worker;
});

/*
 * jsdom ships no `matchMedia`, and the shell picks its layout from one. Tests
 * get a stub that reports the phone layout and never announces a change, so a
 * component under test renders the same way on every run.
 */
if (typeof window !== 'undefined' && typeof window.matchMedia !== 'function') {
    window.matchMedia = (query: string) => ({
        matches: false,
        media: query,
        onchange: null,
        addEventListener: () => undefined,
        removeEventListener: () => undefined,
        addListener: () => undefined,
        removeListener: () => undefined,
        dispatchEvent: () => false,
    });
}

/*
 * Svelte transitions use the Web Animations API, which jsdom does not ship.
 * Finish each synthetic animation in a microtask so intro/outro lifecycle and
 * focus restoration are still exercised instead of disabling transitions.
 */
if (typeof Element !== 'undefined' && typeof Element.prototype.animate !== 'function') {
    Element.prototype.animate = () => {
        let finishHandler: Animation['onfinish'] = null;
        const animation = {
            currentTime: 0,
            effect: null,
            playState: 'finished',
            cancel: () => undefined,
            get onfinish() {
                return finishHandler;
            },
            set onfinish(handler: Animation['onfinish']) {
                finishHandler = handler;
                if (handler !== null) {
                    queueMicrotask(() => {
                        handler.call(animation, new Event('finish') as AnimationPlaybackEvent);
                    });
                }
            },
        } as unknown as Animation;
        return animation;
    };
}
