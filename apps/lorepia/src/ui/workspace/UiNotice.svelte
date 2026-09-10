<script lang="ts">
    import { onMount } from 'svelte';
    import { fade } from 'svelte/transition';
    import { X } from '@lucide/svelte';
    import { tr } from '../../lib/i18n';
    import IconButton from './IconButton.svelte';
    import type { UiNoticeValue } from './notice';

    let { notice, ondismiss }: { notice: UiNoticeValue; ondismiss: () => void } = $props();
    let panel: HTMLDivElement;
    let timer: ReturnType<typeof setTimeout> | undefined;
    let hovering = false;
    let focusing = false;
    function pause() {
        clearTimeout(timer);
    }
    function resume() {
        pause();
        // Errors stay actionable; success notices can disappear after reading.
        if (!notice.retry && !hovering && !focusing) timer = setTimeout(ondismiss, 4000);
    }
    function pointerPresence(present: boolean) {
        hovering = present;
        resume();
    }
    function focusPresence(present: boolean) {
        focusing = present;
        resume();
    }
    function dismiss() {
        const focus = panel.contains(document.activeElement);
        const log = panel.closest('.ui-chat')?.querySelector<HTMLElement>('.ui-messages');
        ondismiss();
        if (focus) log?.focus({ preventScroll: true });
    }
    function retry() {
        if (panel.contains(document.activeElement))
            panel
                .closest('.ui-chat')
                ?.querySelector<HTMLElement>('.ui-messages')
                ?.focus({ preventScroll: true });
        notice.retry?.();
    }
    onMount(() => {
        resume();
        return pause;
    });
    const duration = matchMedia('(prefers-reduced-motion: reduce)').matches ? 0 : 120;
</script>

<div
    class="ui-notice"
    bind:this={panel}
    role="status"
    aria-atomic="true"
    data-ui-no-swipe
    transition:fade={{ duration }}
    onpointerenter={() => pointerPresence(true)}
    onpointerleave={() => pointerPresence(false)}
    onfocusin={() => focusPresence(true)}
    onfocusout={(event) =>
        focusPresence(event.relatedTarget instanceof Node && panel.contains(event.relatedTarget))}
>
    <span>{notice.text}</span>
    {#if notice.retry}<button class="ui-notice-retry ui-pressable" onclick={retry}
            ><span class="ui-press-visual">{$tr('uiPreview.retryReply')}</span></button
        >{/if}
    <IconButton label={$tr('uiPreview.dismissNotice')} onclick={dismiss}><X /></IconButton>
</div>
