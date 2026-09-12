import { tick } from 'svelte';

interface UnderfillOptions {
    scope: string;
    start: number;
    count: number;
    first: string | undefined;
    last: string | undefined;
    busy: boolean;
    check: (() => void) | undefined;
    onunderfill?: (value: boolean) => void;
}

/** View-local fallback for history edges that cannot produce a scroll event. */
export function observeTranscriptUnderfill(node: HTMLElement, initial: UnderfillOptions) {
    let options = initial;
    let disposed = false;
    let queued = false;
    let reported: boolean | undefined;
    let attempted: {
        scope: string;
        start: number;
        first: string | undefined;
        last: string | undefined;
        count: number;
        height: number;
    } | null = null;
    function schedule() {
        if (queued || disposed) return;
        queued = true;
        void tick().then(() => {
            queued = false;
            if (disposed) return;
            const height = node.scrollHeight;
            const underfilled =
                node.clientHeight > 0 && height <= node.clientHeight + 1 && options.count > 0;
            if (reported !== underfilled) {
                reported = underfilled;
                options.onunderfill?.(underfilled);
            }
            if (!underfilled || options.busy || !options.check || options.start <= 0) return;
            if (attempted?.scope === options.scope) {
                if (
                    attempted.start === options.start &&
                    attempted.first === options.first &&
                    attempted.last === options.last
                )
                    return;
                // A capped/repeated page that cannot grow the content cannot fill this viewport.
                if (options.count <= attempted.count && height <= attempted.height) return;
            }
            attempted = {
                scope: options.scope,
                start: options.start,
                first: options.first,
                last: options.last,
                count: options.count,
                height,
            };
            options.check();
        });
    }
    const observer = typeof ResizeObserver === 'undefined' ? null : new ResizeObserver(schedule);
    observer?.observe(node);
    const content = node.querySelector('.ui-transcript-list');
    if (content) observer?.observe(content, { box: 'border-box' });
    schedule();
    return {
        update(next: UnderfillOptions) {
            if (next.scope !== options.scope) {
                attempted = null;
                reported = undefined;
            }
            options = next;
            schedule();
        },
        destroy() {
            disposed = true;
            observer?.disconnect();
        },
    };
}
