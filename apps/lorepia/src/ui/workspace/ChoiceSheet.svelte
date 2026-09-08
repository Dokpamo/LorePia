<script lang="ts">
    import { Check, X } from '@lucide/svelte';
    import { onDestroy, onMount, untrack } from 'svelte';
    import { fade } from 'svelte/transition';
    import { tr } from '../../lib/i18n';
    import IconButton from './IconButton.svelte';
    import { trapFocus } from './focus-trap';
    import { choiceSheetDrag, choiceSheetTransition } from './choice-sheet-motion';
    import type { ChoiceRequest } from './settings-choice';

    let {
        request,
        onclose,
        onclosed,
    }: {
        request: ChoiceRequest;
        onclose: () => void;
        onclosed?: () => void;
    } = $props();
    const id = $props.id();
    let focused = $state(
        untrack(() =>
            Math.max(
                0,
                request.options.findIndex((item) => item.value === request.value),
            ),
        ),
    );
    let panel: HTMLElement;
    let closing = $state(false);
    const reduced = matchMedia('(prefers-reduced-motion: reduce)').matches;
    onDestroy(() => onclosed?.());
    onMount(() =>
        panel
            .querySelector<HTMLElement>('[role="radio"][tabindex="0"]')
            ?.focus({ preventScroll: true }),
    );
    function close() {
        if (closing) return;
        closing = true;
        panel.dataset.choiceClosing = 'true';
        onclose();
    }
    function move(event: KeyboardEvent) {
        const offset = ['ArrowDown', 'ArrowRight'].includes(event.key)
            ? 1
            : ['ArrowUp', 'ArrowLeft'].includes(event.key)
              ? -1
              : 0;
        if (!offset && event.key !== 'Home' && event.key !== 'End') return;
        event.preventDefault();
        focused =
            event.key === 'Home'
                ? 0
                : event.key === 'End'
                  ? request.options.length - 1
                  : (focused + offset + request.options.length) % request.options.length;
        panel
            .querySelectorAll<HTMLElement>('[role="radio"]')
            [focused]?.focus({ preventScroll: true });
    }
</script>

<div class="ui-choice-layer" inert={closing} aria-hidden={closing} data-ui-no-swipe>
    <button
        class="ui-choice-backdrop"
        aria-hidden="true"
        tabindex="-1"
        transition:fade={{ duration: reduced ? 0 : 300 }}
        onclick={close}
    ></button>
    <div
        class="ui-choice-sheet"
        bind:this={panel}
        role="dialog"
        aria-modal="true"
        aria-labelledby={id}
        tabindex="-1"
        transition:choiceSheetTransition={reduced}
        onkeydown={(event) => {
            if (event.key === 'Escape') {
                event.preventDefault();
                event.stopPropagation();
                close();
            }
            trapFocus(event);
        }}
    >
        <button
            type="button"
            class="ui-choice-handle"
            aria-label={$tr('uiPreview.dragChoicesToClose')}
            tabindex="-1"
            use:choiceSheetDrag={{ panel: () => panel, onclose: close }}
            onclick={close}><span aria-hidden="true"></span></button
        >
        <header>
            <h2 {id}>{request.label}</h2>
            <IconButton label={$tr('uiPreview.closeChoices')} onclick={close}><X /></IconButton>
        </header>
        <div role="radiogroup" aria-labelledby={id} tabindex="-1" onkeydown={move}>
            {#each request.options as item, index (item.value)}
                <button
                    type="button"
                    role="radio"
                    aria-checked={request.value === item.value}
                    tabindex={focused === index ? 0 : -1}
                    class="ui-choice-option ui-pressable"
                    onfocus={() => (focused = index)}
                    onclick={() => {
                        if (closing) return;
                        request.onselect(item.value);
                        close();
                    }}
                >
                    <span class="ui-press-visual"
                        ><span>{item.label}</span>{#if request.value === item.value}<Check
                                aria-hidden="true"
                            />{/if}</span
                    >
                </button>
            {/each}
        </div>
    </div>
</div>
