<script lang="ts">
    import type { Snippet } from 'svelte';
    import { Check, Copy, GitBranch, Pencil, RotateCcw, Trash2, X } from '@lucide/svelte';
    import { onDestroy } from 'svelte';
    import { t, tr } from '../../lib/i18n';
    import MarkdownText from '../../features/chat/MarkdownText.svelte';
    import IconButton from './IconButton.svelte';
    import ResponseStatus from './ResponseStatus.svelte';
    import { useTextEditor } from './text-editor.svelte';
    import type { SampleCharacter, SampleMessage } from './view-types';
    import type { ChatSession } from './chat-session';
    let {
        message,
        character,
        session,
        renderMessage,
        active,
        onactivate,
        onclose,
        focusReturn,
        onnotice,
    }: {
        message: SampleMessage;
        character: SampleCharacter;
        session: ChatSession;
        renderMessage?: Snippet<[SampleMessage]>;
        active: boolean;
        onactivate: () => void;
        onclose: () => void;
        focusReturn: HTMLElement | null;
        onnotice: (text: string, retry?: () => void) => void;
    } = $props();
    const editor = useTextEditor();
    let removing = $state(false);
    let copyAttempt = 0;
    let mounted = true;
    onDestroy(() => {
        mounted = false;
    });
    const toolsId = $derived('ui-tools-' + message.id);
    $effect(() => {
        if (!active) removing = false;
    });
    async function copy() {
        const attempt = ++copyAttempt;
        try {
            await navigator.clipboard.writeText(message.text);
            if (mounted && attempt === copyAttempt) onnotice(t('uiPreview.copied'));
        } catch {
            if (mounted && attempt === copyAttempt)
                onnotice(t('uiPreview.copyFailed'), () => void copy());
        }
    }
    function edit() {
        editor.open(
            {
                label: t('uiPreview.editMessage'),
                value: message.text,
                hint: t('uiPreview.messageEditorHint'),
                requiredMessage: t('uiPreview.messageRequired'),
                applyOnDone: true,
                onchange: session.edit(message.id),
            },
            focusReturn ?? undefined,
        );
    }
    function mutate(action: () => unknown) {
        // A branch switch/removal can unmount the pressed tool.
        focusReturn?.focus({ preventScroll: true });
        action();
    }
</script>

<div
    class="ui-message-body ui-message"
    role="button"
    tabindex="0"
    aria-label={$tr('uiPreview.messageMenu')}
    aria-describedby={'ui-message-text-' + message.id}
    aria-expanded={active}
    aria-controls={toolsId}
    onclick={(event) => {
        if (event.target instanceof Element && event.target.closest('a, button')) return;
        if (window.getSelection()?.toString()) return;
        onactivate();
    }}
    onkeydown={(event) => {
        if (event.target !== event.currentTarget) return;
        if (event.key === 'Enter' || event.key === ' ') {
            event.preventDefault();
            onactivate();
        }
        if (event.key === 'Escape') onclose();
    }}
>
    <span class="ui-sr">{message.role === 'user' ? $tr('uiPreview.you') : character.name}</span>
    <div id={'ui-message-text-' + message.id}>
        {#if renderMessage}{@render renderMessage(message)}{:else}<MarkdownText
                text={message.text}
            />{/if}
    </div>
    {#if message.status === 'pending'}<span
            class="ui-typing"
            aria-label={$tr('uiPreview.replyPending')}><i></i><i></i><i></i></span
        >{/if}
</div>
{#if message.sample}<small class="ui-message-meta">{$tr('uiPreview.sampleResponse')}</small>{/if}
<ResponseStatus
    {message}
    busy={session.busy}
    onretry={() => mutate(() => session.regenerate(message.id, true))}
/>
<div
    class="ui-message-tools-shell"
    id={toolsId}
    data-open={active}
    inert={!active}
    aria-hidden={!active}
>
    <div class="ui-message-tools-clip">
        <div
            class="ui-message-tools"
            role="group"
            aria-label={$tr('uiPreview.messageTools')}
            data-ui-no-swipe
        >
            {#if removing}
                <span class="ui-remove-prompt">{$tr('uiPreview.removeConfirm')}</span>
                <IconButton
                    label={$tr('uiPreview.confirmRemove')}
                    caption={$tr('uiPreview.deleteTool')}
                    critical
                    disabled={session.busy}
                    onclick={() => {
                        mutate(() => session.removeFrom(message.id));
                        onclose();
                    }}><Check /></IconButton
                >
                <IconButton
                    label={$tr('uiPreview.cancel')}
                    caption={$tr('uiPreview.cancel')}
                    onclick={() => (removing = false)}><X /></IconButton
                >
            {:else}
                <IconButton
                    label={$tr('uiPreview.copyMessage')}
                    caption={$tr('uiPreview.copyTool')}
                    onclick={() => void copy()}><Copy /></IconButton
                >
                <IconButton
                    label={$tr('uiPreview.branchFrom')}
                    caption={$tr('uiPreview.branchTool')}
                    disabled={session.busy}
                    onclick={() => mutate(() => session.fork(message.id))}><GitBranch /></IconButton
                >
                {#if message.role === 'user'}
                    <IconButton
                        label={$tr('uiPreview.editMessage')}
                        caption={$tr('uiPreview.editTool')}
                        disabled={session.busy}
                        onclick={edit}><Pencil /></IconButton
                    >
                {:else if message.role === 'assistant'}
                    <IconButton
                        label={$tr('uiPreview.regenerate')}
                        caption={$tr('uiPreview.regenerate')}
                        disabled={session.busy}
                        onclick={() => mutate(() => session.regenerate(message.id))}
                        ><RotateCcw /></IconButton
                    >
                {/if}
                <IconButton
                    label={$tr('uiPreview.removeFrom')}
                    caption={$tr('uiPreview.deleteTool')}
                    disabled={session.busy}
                    onclick={() => (removing = true)}><Trash2 /></IconButton
                >
            {/if}
        </div>
    </div>
</div>
