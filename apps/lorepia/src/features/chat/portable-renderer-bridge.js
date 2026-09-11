// Bundled trusted bridge. Imported markup cannot supply scripts or this identity.
// An external script also satisfies the native parent document's script-src 'self'.
(() => {
    'use strict';
    const runtimeId = document.currentScript?.getAttribute('data-runtime-id');
    if (!runtimeId || !/^[0-9a-f-]{36}$/.test(runtimeId)) return;
    const channel = 'lorepia-portable-renderer-v1';
    const actionPattern = /^[A-Za-z0-9][A-Za-z0-9_.:/-]{0,511}$/;
    /** @param {object} message */
    const publish = (message) => parent.postMessage({ channel, runtimeId, ...message }, '*');
    const reportHeight = () => {
        const raw = Math.ceil(
            Math.max(document.documentElement.scrollHeight, document.body.scrollHeight, 32),
        );
        publish({ type: 'portable_resize', height: Math.min(720, Math.max(32, raw)) });
    };
    const playMedia = () => {
        for (const media of document.querySelectorAll('audio[autoplay]')) {
            if (media instanceof HTMLMediaElement) void media.play().catch(() => undefined);
        }
    };
    document.addEventListener(
        'click',
        (event) => {
            if (!event.isTrusted || !(event.target instanceof Element)) return;
            const control = event.target.closest('[data-portable-action]');
            if (!(control instanceof HTMLButtonElement) && !(control instanceof HTMLInputElement))
                return;
            const action = control.getAttribute('data-portable-action')?.trim() ?? '';
            if (!actionPattern.test(action)) return;
            event.preventDefault();
            publish({ type: 'portable_action', action });
        },
        true,
    );
    document.addEventListener('pointerdown', playMedia, { once: true, capture: true });
    if (typeof ResizeObserver === 'function')
        new ResizeObserver(reportHeight).observe(document.documentElement);
    globalThis.addEventListener(
        'load',
        () => {
            playMedia();
            reportHeight();
        },
        { once: true },
    );
    reportHeight();
})();
