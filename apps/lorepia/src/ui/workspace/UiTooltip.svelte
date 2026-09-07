<script lang="ts">
    import { onMount, tick } from 'svelte';
    import { on } from 'svelte/events';
    let { root }: { root: HTMLElement } = $props();
    const id = $props.id();
    let text = $state('');
    let left = $state(0);
    let top = $state(0);
    let ready = $state(false);
    let bubble = $state<HTMLDivElement>();
    let anchor: HTMLElement | null = null;
    let description: string | null = null;
    let timer: ReturnType<typeof setTimeout> | undefined;
    function detach() {
        clearTimeout(timer);
        if (anchor) {
            if (description === null) anchor.removeAttribute('aria-describedby');
            else anchor.setAttribute('aria-describedby', description);
        }
        anchor = null;
    }
    function hide() {
        detach();
        text = '';
        ready = false;
    }
    async function show(node: HTMLElement) {
        hide();
        if (!node.isConnected || node.closest('[inert], [disabled]')) return;
        anchor = node;
        description = node.getAttribute('aria-describedby');
        node.setAttribute('aria-describedby', [description, id].filter(Boolean).join(' '));
        text = node.dataset.uiTooltip ?? '';
        await tick();
        if (anchor !== node || !bubble) return;
        const bounds = root.getBoundingClientRect();
        const target = node.getBoundingClientRect();
        const width = bubble.offsetWidth;
        const height = bubble.offsetHeight;
        left = Math.max(
            8,
            Math.min(
                bounds.width - width - 8,
                target.left - bounds.left + (target.width - width) / 2,
            ),
        );
        top = target.bottom - bounds.top + 6;
        if (top + height > bounds.height - 8)
            top = Math.max(8, target.top - bounds.top - height - 6);
        ready = true;
    }
    onMount(() => {
        const fine = matchMedia('(hover: hover) and (pointer: fine)');
        function target(event: Event) {
            return event.target instanceof Element
                ? event.target.closest<HTMLElement>('[data-ui-tooltip]')
                : null;
        }
        function hover(event: PointerEvent) {
            if (!fine.matches) return;
            const node = target(event);
            if (!node || node === anchor) return;
            hide();
            timer = setTimeout(() => void show(node), 300);
        }
        function focus(event: FocusEvent) {
            const node = target(event);
            if (node && root.dataset.keyboardFocus === 'true') void show(node);
        }
        function escape(event: KeyboardEvent) {
            if (event.key !== 'Escape' || !text) return;
            hide();
            event.preventDefault();
            event.stopImmediatePropagation();
        }
        // Closing a focused surface can dispatch focusout during Svelte's DOM teardown.
        // The event API runs these handlers outside the active render reaction.
        const remove = [
            on(root, 'pointerover', hover),
            on(root, 'pointerout', hide),
            on(root, 'focusin', focus),
            on(root, 'focusout', hide),
            on(root, 'pointerdown', hide, { capture: true }),
            on(root, 'keydown', escape, { capture: true }),
            on(root, 'scroll', hide, { capture: true }),
            on(window, 'resize', hide),
        ];
        return () => {
            remove.forEach((stop) => stop());
            detach();
        };
    });
</script>

{#if text}<div
        {id}
        role="tooltip"
        class="ui-tooltip"
        bind:this={bubble}
        style:left={String(left) + 'px'}
        style:top={String(top) + 'px'}
        style:opacity={ready ? 1 : 0}
    >
        {text}
    </div>{/if}
