<script lang="ts">
    import { Check, Copy, GitBranch, Pencil, RotateCcw, Trash2, X } from '@lucide/svelte';
    import { t, tr } from '../../lib/i18n';
    import MarkdownText from '../../features/chat/MarkdownText.svelte';
    import IconButton from './IconButton.svelte';
    import { useTextEditor } from './text-editor.svelte';
    import type { SampleCharacter, SampleMessage } from './sample-data';
    import type { SampleChatSession } from './sample-chat.svelte';
    let {
        message,
        character,
        session,
        active,
        onactivate,
        onclose,
        focusReturn,
    }: {
        message: SampleMessage;
        character: SampleCharacter;
        session: SampleChatSession;
        active: boolean;
        onactivate: () => void;
        onclose: () => void;
        focusReturn: HTMLElement | null;
    } = $props();
    const editor = useTextEditor();
    let removing = $state(false);
    let notice = $state('');
    const toolsId = $derived('ui-tools-' + message.id);
    $effect(() => {
        if (!active) removing = false;
    });
    async function copy() {
        try {
            await navigator.clipboard.writeText(message.text);
            notice = t('uiPreview.copied');
        } catch {
            notice = t('uiPreview.copyFailed');
        }
    }
    function edit() {
        editor.open(
            {
                label: t('uiPreview.editMessage'),
                value: message.text,
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
    <div id={'ui-message-text-' + message.id}><MarkdownText text={message.text} /></div>
    {#if message.status === 'pending'}<span
            class="ui-typing"
            aria-label={$tr('uiPreview.replyPending')}><i></i><i></i><i></i></span
        >{/if}
</div>
{#if message.sample}<small class="ui-message-meta">{$tr('uiPreview.sampleResponse')}</small>{/if}
{#if message.status === 'failed' || message.status === 'cancelled'}
    <div class="ui-response-state" role="status">
        <span
            >{$tr(
                message.status === 'failed' ? 'uiPreview.replyFailed' : 'uiPreview.replyCancelled',
            )}</span
        >
        <button
            disabled={session.busy}
            onclick={() => mutate(() => session.regenerate(message.id, true))}
            >{$tr('uiPreview.retryReply')}</button
        >
    </div>
{/if}
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
                    disabled={session.busy}
                    onclick={() => {
                        mutate(() => session.removeFrom(message.id));
                        onclose();
                    }}><Check /></IconButton
                >
                <IconButton label={$tr('uiPreview.cancel')} onclick={() => (removing = false)}
                    ><X /></IconButton
                >
            {:else}
                <IconButton label={$tr('uiPreview.copyMessage')} onclick={() => void copy()}
                    ><Copy /></IconButton
                >
                <IconButton
                    label={$tr('uiPreview.branchFrom')}
                    disabled={session.busy}
                    onclick={() => mutate(() => session.fork(message.id))}><GitBranch /></IconButton
                >
                {#if message.role === 'user'}
                    <IconButton
                        label={$tr('uiPreview.editMessage')}
                        disabled={session.busy}
                        onclick={edit}><Pencil /></IconButton
                    >
                {:else}
                    <IconButton
                        label={$tr('uiPreview.regenerate')}
                        disabled={session.busy}
                        onclick={() => mutate(() => session.regenerate(message.id))}
                        ><RotateCcw /></IconButton
                    >
                {/if}
                <IconButton
                    label={$tr('uiPreview.removeFrom')}
                    disabled={session.busy}
                    onclick={() => (removing = true)}><Trash2 /></IconButton
                >
            {/if}
        </div>
    </div>
</div>
<span class="ui-sr" role="status">{notice}</span>
