<script lang="ts">
    import type { Snippet } from 'svelte';
    import { onDestroy } from 'svelte';
    import { t, tr } from '../../lib/i18n';
    import MarkdownText from '../../features/chat/MarkdownText.svelte';
    import MessageMenu from './MessageMenu.svelte';
    import { messagePress } from './message-press';
    import ResponseStatus from './ResponseStatus.svelte';
    import { useTextEditor } from './text-editor.svelte';
    import type { SampleCharacter, SampleMessage } from './view-types';
    import type { ChatSession } from './chat-session';
    let {
        message,
        character,
        session,
        renderMessage,
        messageIndex = 0,
        branchId,
        active,
        onactivate,
        onclose,
        focusReturn,
        onnotice,
    }: {
        message: SampleMessage;
        character: SampleCharacter;
        session: ChatSession;
        messageIndex?: number;
        branchId?: string;
        renderMessage?: Snippet<[SampleMessage, number]>;
        active: boolean;
        onactivate: () => void;
        onclose: () => void;
        focusReturn: HTMLElement | null;
        onnotice: (text: string, retry?: () => void) => void;
    } = $props();
    const editor = useTextEditor();
    let body = $state<HTMLDivElement>();
    let copyAttempt = 0;
    let mounted = true;
    onDestroy(() => {
        mounted = false;
    });
    function closeMenu(restoreFocus: boolean) {
        if (restoreFocus) body?.focus({ preventScroll: true });
        onclose();
    }
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
            body,
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
    bind:this={body}
    data-ui-selectable
    use:messagePress={onactivate}
    role="button"
    tabindex="0"
    aria-label={$tr('uiPreview.messageMenu')}
    aria-describedby={'ui-message-text-' + message.id}
    aria-expanded={active}
    aria-haspopup="menu"
    onclick={(event) => {
        if (event.target instanceof Element && event.target.closest('a, button')) return;
        // Keyboard/assistive activation has no pointer click count.
        if (event.detail === 0) onactivate();
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
        {#if renderMessage}{@render renderMessage(message, messageIndex)}{:else}<MarkdownText
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
{#if active && body}
    <MessageMenu
        anchor={body}
        {message}
        busy={session.busy}
        branches={session.branches()}
        {branchId}
        onclose={closeMenu}
        oncopy={() => void copy()}
        onedit={edit}
        onfork={() => mutate(() => session.fork(message.id))}
        onbranch={(id: string) => mutate(() => session.selectBranch(id))}
        onremove={() => mutate(() => session.removeFrom(message.id))}
        onregenerate={() => mutate(() => session.regenerate(message.id))}
    />
{/if}
