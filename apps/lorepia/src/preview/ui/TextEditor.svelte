<script lang="ts">
    import { ArrowLeft, ArrowUp, Check } from '@lucide/svelte';
    import { onMount, untrack } from 'svelte';
    import { editorMorph } from './editor-morph';
    import { tr } from '../../lib/i18n';
    import { shouldSubmitComposer } from '../../features/chat/composer';
    import IconButton from './IconButton.svelte';
    import { edgeBack, requestBack } from './edge-back';
    import type { TextEditRequest } from './text-editor.svelte';

    let {
        request,
        onclose,
        onclosed,
    }: { request: TextEditRequest; onclose: () => void; onclosed: () => void } = $props();
    let value = $state(untrack(() => request.value));
    const origin = untrack(() => request.origin);
    let composing = $state(false);
    let input: HTMLTextAreaElement;
    let panel: HTMLDivElement;
    onMount(() => {
        input.focus({ preventScroll: true });
        input.setSelectionRange(value.length, value.length);
    });
    function send() {
        if (!value.trim() || composing) return;
        request.onsend?.();
        onclose();
    }
</script>

<div
    class="ui-editor-layer"
    class:ui-editor-morph={!!origin}
    transition:editorMorph={origin}
    onoutroend={() => {
        origin?.().surface.removeAttribute('data-ui-editor-origin');
        onclosed();
    }}
>
    <div
        class="ui-text-editor"
        bind:this={panel}
        role="dialog"
        tabindex="-1"
        aria-modal="true"
        aria-label={request.label}
        use:edgeBack={{ onback: onclose }}
        data-ui-no-swipe
        onkeydown={(event) => {
            if (event.key === 'Escape') {
                event.preventDefault();
                event.stopPropagation();
                requestBack(panel);
            }
            if (event.key === 'Tab') {
                const controls = Array.from(
                    event.currentTarget.querySelectorAll<HTMLElement>(
                        'button:not(:disabled), textarea',
                    ),
                );
                const first = controls[0];
                const last = controls.at(-1);
                if (event.shiftKey && document.activeElement === first) {
                    event.preventDefault();
                    last?.focus();
                } else if (!event.shiftKey && document.activeElement === last) {
                    event.preventDefault();
                    first?.focus();
                }
            }
        }}
    >
        <header class="ui-page-header ui-navigation-header">
            <div data-editor-back>
                <IconButton label={$tr('uiPreview.closeEditor')} onclick={() => requestBack(panel)}
                    ><ArrowLeft /></IconButton
                >
            </div>
            <div class="ui-title-group"><strong>{request.label}</strong></div>
            {#if request.message}
                <div data-editor-send>
                    <IconButton
                        label={$tr('uiPreview.send')}
                        disabled={!value.trim() || composing}
                        onclick={send}><ArrowUp /></IconButton
                    >
                </div>
            {:else}
                <IconButton label={$tr('uiPreview.editDone')} onclick={onclose}
                    ><Check /></IconButton
                >
            {/if}
        </header>
        <div class="ui-editor-text-region">
            <textarea
                bind:this={input}
                aria-label={request.label}
                {value}
                placeholder={request.placeholder}
                maxlength={request.maxlength ?? 131072}
                spellcheck="false"
                oninput={(event) => {
                    value = event.currentTarget.value;
                    request.onchange(value);
                }}
                oncompositionstart={() => (composing = true)}
                oncompositionend={() => (composing = false)}
                onkeydown={(event) => {
                    if (request.message && shouldSubmitComposer(event, composing)) {
                        event.preventDefault();
                        send();
                    }
                }}></textarea>
        </div>
        <footer class="ui-editor-footer">
            {#if request.message}<span class="ui-editor-key-hint"
                    >{$tr('uiPreview.editorKeys')}</span
                >
            {:else if request.maxlength}<span>{value.length} / {request.maxlength}</span>{/if}
        </footer>
    </div>
</div>
