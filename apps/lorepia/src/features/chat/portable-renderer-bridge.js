// Bundled trusted bridge. Imported markup cannot supply scripts or this identity.
// An external script also satisfies the native parent document's script-src 'self'.
(() => {
    'use strict';
    const runtimeId = document.currentScript?.getAttribute('data-runtime-id');
    const room = document.currentScript?.getAttribute('data-surface') === 'room';
    if (!runtimeId || !/^[0-9a-f-]{36}$/.test(runtimeId)) return;
    const channel = 'lorepia-portable-renderer-v1';
    const actionPattern = /^[A-Za-z0-9][A-Za-z0-9_.:/-]{0,511}$/;
    /** @param {object} message */
    const publish = (message) => parent.postMessage({ channel, runtimeId, ...message }, '*');
    let scheduled = false;
    let lastLayout = '';
    const reportLayout = () => {
        scheduled = false;
        const root = document.querySelector('.portable-message');
        if (!(root instanceof HTMLElement)) return;
        const elements = [...root.querySelectorAll('*')].slice(0, 4096);
        if (!room) {
            // Imported scroll boxes belong to the transcript, not to a bubble.
            for (const element of [root, ...elements]) {
                if (!(element instanceof HTMLElement)) continue;
                const style = getComputedStyle(element);
                if (/auto|scroll/.test(style.overflowY)) {
                    element.style.setProperty('height', 'auto', 'important');
                    element.style.setProperty('max-height', 'none', 'important');
                    element.style.setProperty('overflow-y', 'visible', 'important');
                    element.style.setProperty('overflow-x', 'clip', 'important');
                }
            }
            const height = Math.min(
                65536,
                Math.max(
                    32,
                    Math.ceil(Math.max(root.scrollHeight, root.getBoundingClientRect().height)),
                ),
            );
            if (lastLayout !== String(height)) publish({ type: 'portable_resize', height });
            lastLayout = String(height);
            return;
        }
        const width = Math.min(65536, innerWidth);
        const height = Math.min(65536, innerHeight);
        const regions = [];
        // Only this room report shares read-only styles; message layout writes styles above.
        /** @type {WeakMap<Element, CSSStyleDeclaration>} */
        const styles = new WeakMap();
        /** @param {Element} element */
        const roomStyle = (element) => {
            let style = styles.get(element);
            if (!style) {
                style = getComputedStyle(element);
                styles.set(element, style);
            }
            return style;
        };
        /** @param {DOMRect} bounds */
        const addRegion = (bounds) => {
            const x = Math.max(0, Math.min(width, bounds.left));
            const y = Math.max(0, Math.min(height, bounds.top));
            const right = Math.max(0, Math.min(width, bounds.right));
            const bottom = Math.max(0, Math.min(height, bounds.bottom));
            if (right > x && bottom > y)
                regions.push({ x, y, width: right - x, height: bottom - y });
        };
        for (const element of elements) {
            if (!(element instanceof HTMLElement) || element.tagName === 'STYLE') continue;
            const style = roomStyle(element);
            if (element.parentElement !== root && style.position !== 'fixed') continue;
            if (style.visibility === 'hidden' || style.display === 'none') continue;
            let hidden = false;
            /** @type {HTMLElement | null} */
            let parent = element;
            while (parent && parent !== root) {
                if (Number(roomStyle(parent).opacity) === 0) hidden = true;
                parent = parent.parentElement;
            }
            if (hidden) continue;
            addRegion(element.getBoundingClientRect());
            if (regions.length === 128) break;
        }
        // Root text has no descendant element box; measure its rendered lines.
        let textNodes = 0;
        let textRects = 0;
        for (const node of root.childNodes) {
            if (textNodes++ === 4096 || textRects === 4096 || regions.length === 128) break;
            if (node.nodeType !== Node.TEXT_NODE || !node.textContent?.trim()) continue;
            const style = roomStyle(root);
            if (
                style.visibility === 'hidden' ||
                style.visibility === 'collapse' ||
                style.display === 'none' ||
                Number(style.opacity) === 0
            )
                break;
            const range = document.createRange();
            range.selectNodeContents(node);
            for (const bounds of range.getClientRects()) {
                if (textRects === 4096 || regions.length === 128) break;
                textRects += 1;
                addRegion(bounds);
            }
        }
        const layout = JSON.stringify(regions);
        if (lastLayout !== layout) publish({ type: 'portable_regions', regions });
        lastLayout = layout;
    };
    const scheduleLayout = () => {
        if (scheduled) return;
        scheduled = true;
        requestAnimationFrame(reportLayout);
    };
    let mediaVisible = room;
    /** @type {Set<HTMLMediaElement>} */
    const resumedMedia = new Set();
    const startedAutoplay = new WeakSet();
    const pendingMedia = new WeakSet();
    /** @param {HTMLMediaElement} media */
    const playVisibleMedia = (media) => {
        if (pendingMedia.has(media)) return;
        pendingMedia.add(media);
        void media
            .play()
            .then(() => {
                pendingMedia.delete(media);
                startedAutoplay.add(media);
                if (!mediaVisible) {
                    resumedMedia.add(media);
                    media.pause();
                }
            })
            .catch(() => pendingMedia.delete(media));
    };
    const playMedia = () => {
        if (!mediaVisible) return;
        const selector = room ? 'audio[autoplay]' : 'audio[data-portable-autoplay="true"]';
        for (const media of document.querySelectorAll(selector)) {
            if (!(media instanceof HTMLMediaElement) || (!room && startedAutoplay.has(media)))
                continue;
            playVisibleMedia(media);
        }
    };
    if (!room)
        globalThis.addEventListener('message', (event) => {
            /** @type {unknown} */
            const data = event.data;
            if (
                event.source !== parent ||
                typeof data !== 'object' ||
                data === null ||
                !('channel' in data) ||
                !('runtimeId' in data) ||
                !('type' in data) ||
                !('visible' in data) ||
                data.channel !== channel ||
                data.runtimeId !== runtimeId ||
                data.type !== 'portable_visibility' ||
                typeof data.visible !== 'boolean' ||
                data.visible === mediaVisible
            )
                return;
            mediaVisible = data.visible;
            if (!mediaVisible) {
                for (const media of document.querySelectorAll('audio,video')) {
                    if (!(media instanceof HTMLMediaElement) || media.paused) continue;
                    resumedMedia.add(media);
                    media.pause();
                }
            } else {
                for (const media of resumedMedia) {
                    if (media.isConnected) playVisibleMedia(media);
                }
                resumedMedia.clear();
                playMedia();
            }
        });
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
        new ResizeObserver(scheduleLayout).observe(document.body);
    new MutationObserver(scheduleLayout).observe(document.body, {
        subtree: true,
        childList: true,
        attributes: true,
    });
    globalThis.addEventListener('resize', scheduleLayout);
    document.addEventListener('load', scheduleLayout, true);
    globalThis.addEventListener(
        'load',
        () => {
            playMedia();
            reportLayout();
        },
        { once: true },
    );
    // A clipped cross-origin frame can have animation frames suspended. Its first
    // hit regions must be reported synchronously so the host can reveal it.
    reportLayout();
})();
