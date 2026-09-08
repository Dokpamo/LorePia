<script lang="ts">
    import { ArrowUp, Maximize2, Settings, Square } from '@lucide/svelte';
    import { t, tr } from '../../lib/i18n';
    import { shouldSubmitComposer } from '../../features/chat/composer';
    import IconButton from './IconButton.svelte';
    import { useTextEditor } from './text-editor.svelte';
    import { measureComposer, type ComposerDockMetrics } from './composer-measure';
    let {
        draft,
        ondraft,
        onsend,
        onsettings,
        oninteract,
        busy = false,
        onstop,
        ondock,
    }: {
        draft: string;
        ondraft: (value: string) => void;
        onsend: () => void;
        onsettings: (trigger: HTMLButtonElement) => void;
        oninteract: () => void;
        busy?: boolean;
        onstop?: () => void;
        ondock?: (metrics: ComposerDockMetrics) => void;
    } = $props();
    const editor = useTextEditor();
    let field: HTMLDivElement;
    let input: HTMLTextAreaElement;
    let focused = $state(false);
    let composing = $state(false);
    let lines = $state(1);
    let overflows = $state(false);
    const expanded = $derived(focused || draft.trim().length > 0);
    const canFullscreen = $derived(draft.trim().length > 0 && (lines >= 2 || overflows));
    function send() {
        if (busy || composing || !draft.trim()) return;
        onsend();
        input.focus({ preventScroll: true });
    }
    function open() {
        if (!canFullscreen || composing) return;
        oninteract();
        editor.open(
            {
                label: t('uiPreview.message'),
                value: draft,
                message: true,
                placeholder: t('uiPreview.messagePlaceholder'),
                onchange: ondraft,
                onsend,
                origin: () => ({
                    surface: field,
                    text: input,
                    leading: field.querySelector('.ui-compose-leading'),
                    trailing: field.querySelector('.ui-compose-send'),
                }),
            },
            input,
        );
    }
</script>

<form
    class="ui-compose"
    aria-label={$tr('uiPreview.compose')}
    data-ui-no-swipe
    onsubmit={(event) => {
        event.preventDefault();
        send();
    }}
>
    <div
        bind:this={field}
        class="ui-compose-field"
        class:ui-compose-has-draft={draft.trim().length > 0}
        class:ui-compose-expanded={expanded}
        class:ui-compose-can-fullscreen={canFullscreen}
        class:ui-compose-overflows={overflows}
        class:ui-compose-busy={busy}
        onfocusin={() => {
            focused = true;
            oninteract();
        }}
        onfocusout={(event) => {
            if (
                event.relatedTarget instanceof Node &&
                event.currentTarget.contains(event.relatedTarget)
            )
                return;
            focused = false;
        }}
    >
        <div class="ui-compose-text-region">
            <textarea
                bind:this={input}
                value={draft}
                aria-label={$tr('uiPreview.message')}
                placeholder={$tr('uiPreview.messagePlaceholder')}
                rows="1"
                maxlength="131072"
                use:measureComposer={{
                    value: draft,
                    ondock,
                    onmeasure: (metrics) => {
                        lines = metrics.lines;
                        overflows = metrics.overflows;
                    },
                }}
                oninput={(event) => ondraft(event.currentTarget.value)}
                oncompositionstart={() => (composing = true)}
                oncompositionend={() => (composing = false)}
                onkeydown={(event) => {
                    if (shouldSubmitComposer(event, composing)) {
                        event.preventDefault();
                        send();
                    }
                }}></textarea>
        </div>
        <div class="ui-compose-actions">
            <div class="ui-compose-leading">
                <IconButton
                    label={$tr('uiPreview.roomSettings')}
                    onclick={(event: MouseEvent & { currentTarget: HTMLButtonElement }) =>
                        onsettings(event.currentTarget)}><Settings /></IconButton
                >
            </div>
            <span class="ui-compose-spacer"></span>
            <div class="ui-compose-expand" inert={!canFullscreen} aria-hidden={!canFullscreen}>
                <IconButton
                    label={$tr('uiPreview.expandComposer')}
                    disabled={!canFullscreen || composing}
                    onclick={open}><Maximize2 /></IconButton
                >
            </div>
            <div class="ui-compose-send">
                {#if busy}<IconButton label={$tr('uiPreview.stopReply')} onclick={() => onstop?.()}
                        ><Square /></IconButton
                    >
                {:else}<IconButton
                        label={$tr('uiPreview.send')}
                        disabled={!draft.trim() || composing}
                        onclick={send}><ArrowUp /></IconButton
                    >{/if}
            </div>
        </div>
    </div>
</form>
