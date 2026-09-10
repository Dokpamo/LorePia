interface ExpansionState {
    active: string | null;
    start: number;
    end: number;
}

/** Keep the transcript's lift on the same intrinsic height as its expanding tools. */
export function messageExpansion(list: HTMLElement, initial: ExpansionState) {
    let state = initial;
    let tools: HTMLElement | null = null;
    let observer: ResizeObserver | undefined;
    function measure() {
        // A virtualized row can leave the DOM while its tools remain selected.
        // Retain the last height until it is rendered again or explicitly closed.
        if (!tools?.isConnected || !list.contains(tools)) return;
        const height = tools.offsetHeight;
        if (height > 0) list.style.setProperty('--ui-tools-height', `${String(height)}px`);
    }
    function connect() {
        observer?.disconnect();
        tools = null;
        if (!state.active) return;
        const row = [...list.querySelectorAll<HTMLElement>('[data-message-id]')].find(
            (item) => item.dataset.messageId === state.active,
        );
        tools = row?.querySelector<HTMLElement>('.ui-message-tools') ?? null;
        measure();
        if (tools && typeof ResizeObserver !== 'undefined') {
            observer = new ResizeObserver(measure);
            observer.observe(tools);
        }
    }
    connect();
    return {
        update(next: ExpansionState) {
            state = next;
            connect();
        },
        destroy() {
            observer?.disconnect();
            list.style.removeProperty('--ui-tools-height');
        },
    };
}
