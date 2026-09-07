<script lang="ts">
    import { onMount, untrack } from 'svelte';
    import { ArrowLeft, PanelRight } from '@lucide/svelte';
    import { tr } from '../../lib/i18n';
    import type { MessageDto } from '../../lib/ipc/contracts';
    import {
        ChatScrollLifecycle,
        type MessageCollectionSnapshot,
    } from '../../features/chat/chat-scroll.svelte';
    import IconButton from './IconButton.svelte';
    import ChatTranscript from './ChatTranscript.svelte';
    import MessageComposer from './MessageComposer.svelte';
    import type { ComposerDockMetrics } from './composer-measure';
    import type { SampleChatSession } from './sample-chat.svelte';
    import type { Page, SampleCharacter, SampleConversation } from './sample-data';
    let {
        character,
        conversation,
        session,
        draft,
        managementVisible = false,
        creatorVisible = false,
        onnavigate,
        ondraft,
        onsend,
        onsettings,
        oninteract,
    }: {
        character: SampleCharacter;
        conversation: SampleConversation;
        session: SampleChatSession;
        draft: string;
        managementVisible?: boolean;
        creatorVisible?: boolean;
        onnavigate: (page: Page) => void;
        ondraft: (value: string) => void;
        onsend: () => void;
        onsettings: (trigger: HTMLButtonElement) => void;
        oninteract: () => void;
    } = $props();
    let active = $state<string | null>(null);
    // The original scroll owner needs identity/date, not a fresh index per stream token.
    const projected = $derived.by(() => {
        const messages = conversation.messages;
        const id = conversation.id;
        return untrack(() =>
            messages.map((message): MessageDto => ({
                id: message.id,
                conversation_id: id,
                parent_id: null,
                role: message.role,
                content: '',
                status: 'complete',
                generation_id: null,
                created_at: '2026-09-07T00:00:00Z',
            })),
        );
    });
    const scroll: ChatScrollLifecycle = new ChatScrollLifecycle({
        currentCollection: () => collection,
        messageDayKey: () => 'sample-day',
        onMemorySourceMissing: () => undefined,
        onMemorySourceFocused: () => undefined,
    });
    const collection: MessageCollectionSnapshot = $derived(
        scroll.snapshotMessageCollection(projected),
    );
    const branches = $derived(session.branches());
    function activate(id: string | null) {
        active = id;
        if (id) scroll.stabilizeMessageActionLayout(id);
        else scroll.clearStableMessageActionLayout();
    }
    $effect(() => {
        const key =
            conversation.id +
            ':' +
            (conversation.activeBranchId ?? 'main') +
            ':' +
            (conversation.mode ?? 'chat');
        untrack(() => scroll.syncBranch(key, () => activate(null)));
    });
    $effect(() => {
        const snapshot = collection;
        untrack(() => {
            scroll.syncCollection(snapshot);
            if (active && !snapshot.retainedIds.has(active)) activate(null);
        });
    });
    $effect(() => {
        const count = conversation.messages.length;
        const last = conversation.messages.at(-1);
        const length = last?.status === 'pending' ? last.text.length : 0;
        untrack(() => scroll.syncMessageGrowth(count, length));
    });
    onMount(() => scroll.observeScroller());
    function syncDock(metrics: ComposerDockMetrics) {
        const log = scroll.scroller;
        if (!log) return;
        if (metrics.follow) scroll.applyProgrammaticScrollPosition(log, log.scrollHeight);
        else if (active && metrics.editing)
            scroll.applyProgrammaticScrollPosition(log, log.scrollTop + metrics.delta);
    }
    function sendToTail() {
        if (session.busy || !draft.trim()) return;
        activate(null);
        const log = scroll.scroller;
        if (log) scroll.applyProgrammaticScrollPosition(log, log.scrollHeight);
        onsend();
    }
</script>

<header class="ui-page-header" class:ui-navigation-header={!managementVisible}>
    {#if !managementVisible}<IconButton
            label={$tr('uiPreview.openManagement')}
            onclick={() => onnavigate(0)}><ArrowLeft /></IconButton
        >{/if}
    <div class="ui-title-group">
        <strong>{character.name}</strong><small>{conversation.title}</small>
    </div>
    {#if branches.length > 1}
        <select
            class="ui-branch-select"
            aria-label={$tr('uiPreview.branch')}
            value={conversation.activeBranchId ?? 'main'}
            disabled={session.busy}
            onchange={(event) => session.selectBranch(event.currentTarget.value)}
        >
            {#each branches as branch (branch.id)}<option value={branch.id}>{branch.title}</option
                >{/each}
        </select>
    {/if}
    {#if character.subpage && !creatorVisible}<IconButton
            label={$tr('uiPreview.openSubpage')}
            onclick={() => onnavigate(2)}><PanelRight /></IconButton
        >{/if}
</header>
<ChatTranscript
    {character}
    {conversation}
    {session}
    {scroll}
    {collection}
    {active}
    onactive={activate}
/>
<MessageComposer
    {draft}
    {ondraft}
    onsend={sendToTail}
    {onsettings}
    {oninteract}
    busy={session.busy}
    onstop={() => session.stop()}
    ondock={syncDock}
/>
