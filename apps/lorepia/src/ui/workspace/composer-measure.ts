interface ComposerMetrics {
    lines: number;
    overflows: boolean;
}

export interface ComposerDockMetrics {
    delta: number;
    follow: boolean;
    editing: boolean;
}

/** The original measureComposer rules, bounded to this local preview's dock. */
export function measureComposer(
    input: HTMLTextAreaElement,
    initial: {
        value: string;
        onmeasure: (metrics: ComposerMetrics) => void;
        ondock?: (metrics: ComposerDockMetrics) => void;
    },
) {
    let options = initial;
    const field = input.closest<HTMLElement>('.ui-compose-field');
    const dock = field?.closest<HTMLElement>('.ui-compose');
    const chat = field?.closest<HTMLElement>('.ui-chat');
    const log = chat?.querySelector<HTMLElement>('.ui-messages');
    let disposed = false;
    let queued = false;
    let width = -1;
    let viewportHeight = -1;
    let pinFrame: number | undefined;
    let resizeFrame: number | undefined;
    let previousOverlay = 0;

    function measure() {
        if (disposed || !field) return;
        // Measure the final writing width without moving the visible text region
        // or disabling its transition. Restore these overrides before paint.
        const previous = {
            height: input.style.height,
            width: input.style.width,
            padding: input.style.padding,
            whiteSpace: input.style.whiteSpace,
        };
        const inset = Number.parseFloat(getComputedStyle(input.parentElement ?? field).right) || 8;
        if (field.clientWidth > 0) input.style.width = `${String(field.clientWidth - inset * 2)}px`;
        input.style.padding =
            getComputedStyle(field).getPropertyValue('--ui-compose-writing-padding') || '8px 8px 0';
        input.style.whiteSpace = 'pre-wrap';
        const style = getComputedStyle(input);
        const line = Number.parseFloat(style.lineHeight) || 25.6;
        const padding =
            (Number.parseFloat(style.paddingTop) || 0) +
            (Number.parseFloat(style.paddingBottom) || 0);
        input.style.height = '0px';
        const scrollHeight = Math.max(line + padding, input.scrollHeight);
        Object.assign(input.style, previous);
        const lines = Math.max(1, Math.ceil((scrollHeight - padding - 1) / line));
        // A normal viewport shows ten lines. Keep navigation/tools accessible
        // when the keyboard or a short window leaves less vertical room.
        const header = chat?.querySelector<HTMLElement>('.ui-page-header')?.offsetHeight ?? 60;
        const actions = field.querySelector<HTMLElement>('.ui-compose-actions');
        const actionsStyle = actions ? getComputedStyle(actions) : null;
        const actionHeight = actions?.offsetHeight ?? 0;
        // Measure writing chrome, not a resting row halfway through expansion.
        const writingGap = Number.parseFloat(
            getComputedStyle(field).getPropertyValue('--ui-compose-tools-gap'),
        );
        const chrome =
            writingGap > 0
                ? (actionHeight > 0 ? actionHeight : 44) + 24
                : (actionHeight > 0 ? actionHeight : 48) +
                  2 * (Number.parseFloat(actionsStyle?.bottom ?? '') || 8) +
                  (Number.parseFloat(getComputedStyle(input.parentElement ?? field).top) || 8);
        const available = chat?.clientHeight ? chat.clientHeight - header - 32 : Infinity;
        const textLimit = Math.max(
            line + padding,
            Math.min(10 * line + padding, available - chrome),
        );
        const overflows = scrollHeight > textLimit + 1;
        field.style.setProperty(
            '--ui-compose-text-size',
            `${String(Math.ceil(Math.min(scrollHeight, textLimit)))}px`,
        );
        field.style.setProperty('--ui-compose-limit', `${String(Math.ceil(textLimit + chrome))}px`);
        options.onmeasure({ lines, overflows });
        syncDock(field.offsetHeight);
        if (pinFrame !== undefined) cancelAnimationFrame(pinFrame);
        if (!overflows) {
            // WebKit can scroll the caret after input during the height transition.
            // Re-pin visible lines for three frames, like the production composer.
            let remaining = 3;
            const pin = () => {
                input.scrollTop = 0;
                remaining -= 1;
                pinFrame = remaining > 0 ? requestAnimationFrame(pin) : undefined;
            };
            input.scrollTop = 0;
            pinFrame = requestAnimationFrame(pin);
        }
    }
    function schedule() {
        if (queued) return;
        queued = true;
        queueMicrotask(() => {
            queued = false;
            measure();
        });
    }
    function scheduleResize() {
        if (resizeFrame !== undefined) return;
        // A ResizeObserver delivery must not resize its own observed subtree.
        resizeFrame = requestAnimationFrame(() => {
            resizeFrame = undefined;
            measure();
        });
    }
    function syncDock(height: number) {
        if (!chat || !dock || height <= 0) return;
        const style = getComputedStyle(dock);
        const bottom = Number.parseFloat(style.paddingBottom) || 0;
        const overlay = height + bottom + (Number.parseFloat(style.paddingTop) || 0);
        if (Math.abs(overlay - previousOverlay) < 0.5) return;
        const follow = !!log && log.scrollHeight - log.scrollTop - log.clientHeight < 100;
        const delta = previousOverlay ? overlay - previousOverlay : 0;
        previousOverlay = overlay;
        chat.style.setProperty('--ui-composer-height', `${String(Math.ceil(height))}px`);
        chat.style.setProperty('--ui-composer-bottom', `${String(bottom)}px`);
        chat.style.setProperty('--ui-composer-overlay', `${String(Math.ceil(overlay))}px`);
        options.ondock?.({ delta, follow, editing: !!field?.contains(document.activeElement) });
    }
    const observer =
        typeof ResizeObserver === 'undefined'
            ? null
            : new ResizeObserver((entries) => {
                  for (const entry of entries) {
                      if (entry.target === field) {
                          syncDock(entry.borderBoxSize[0]?.blockSize ?? entry.contentRect.height);
                      } else if (
                          entry.target === dock &&
                          Math.abs(entry.contentRect.width - width) >= 0.5
                      ) {
                          width = entry.contentRect.width;
                          scheduleResize();
                      } else if (
                          entry.target === chat &&
                          Math.abs(entry.contentRect.height - viewportHeight) >= 0.5
                      ) {
                          viewportHeight = entry.contentRect.height;
                          scheduleResize();
                      }
                  }
              });
    if (dock) observer?.observe(dock);
    if (field) observer?.observe(field);
    if (chat) observer?.observe(chat);
    input.addEventListener('input', measure);
    input.addEventListener('focus', measure);
    window.addEventListener('resize', scheduleResize);
    schedule();
    return {
        update(next: typeof initial) {
            options = next;
            schedule();
        },
        destroy() {
            disposed = true;
            observer?.disconnect();
            input.removeEventListener('input', measure);
            input.removeEventListener('focus', measure);
            window.removeEventListener('resize', scheduleResize);
            if (pinFrame !== undefined) cancelAnimationFrame(pinFrame);
            if (resizeFrame !== undefined) cancelAnimationFrame(resizeFrame);
            chat?.style.removeProperty('--ui-composer-height');
            chat?.style.removeProperty('--ui-composer-bottom');
            chat?.style.removeProperty('--ui-composer-overlay');
        },
    };
}
