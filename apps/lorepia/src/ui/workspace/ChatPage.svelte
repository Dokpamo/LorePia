<script lang="ts">
    import type { Snippet } from 'svelte';
    import type { SampleMessage } from './view-types';
    import { onMount, untrack } from 'svelte';
    import { ArrowDown, ArrowLeft, PanelRight } from '@lucide/svelte';
    import { tr } from '../../lib/i18n';
    import type { MessageDto } from '../../lib/ipc/contracts';
    import {
        ChatScrollLifecycle,
        type MessageCollectionSnapshot,
    } from '../../features/chat/chat-scroll.svelte';
    import IconButton from './IconButton.svelte';
    import ChatTranscript from './ChatTranscript.svelte';
    import MessageComposer from './MessageComposer.svelte';
    import UiNotice from './UiNotice.svelte';
    import type { UiNoticeValue } from './notice';
    import type { ComposerDockMetrics } from './composer-measure';
    import type { ChatSession } from './chat-session';
    import type { Page, SampleCharacter, SampleConversation } from './view-types';
    let {
        character,
        conversation,
        session,
        renderMessage,
        extras,
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
        session: ChatSession;
        renderMessage?: Snippet<[SampleMessage]>;
        extras?: Snippet;
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
    let notice = $state<UiNoticeValue | null>(null);
    let noticeId = 0;
    function showNotice(text: string, retry?: () => void) {
        notice = { id: ++noticeId, text, retry };
    }
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
                created_at: message.source?.created_at ?? conversation.date,
            })),
        );
    });
    const scroll: ChatScrollLifecycle = new ChatScrollLifecycle({
        currentCollection: () => collection,
        messageDayKey: (value) => value.slice(0, 10),
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
        untrack(() => {
            notice = null;
            scroll.syncBranch(key, () => activate(null));
        });
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
    {renderMessage}
    {extras}
    {scroll}
    {collection}
    {active}
    onactive={activate}
    onnotice={showNotice}
    onwrite={() =>
        scroll.scroller
            ?.closest('.ui-chat')
            ?.querySelector<HTMLTextAreaElement>('.ui-compose textarea')
            ?.focus()}
/>
<div class="ui-chat-floaters">
    {#if notice}{#key notice.id}<UiNotice {notice} ondismiss={() => (notice = null)} />{/key}{/if}
    <div
        class="ui-jump-latest"
        data-visible={!scroll.nearBottom}
        inert={scroll.nearBottom}
        aria-hidden={scroll.nearBottom}
    >
        <IconButton
            label={$tr('uiPreview.latestMessage')}
            onclick={() => {
                const log = scroll.scroller;
                if (!log) return;
                log.focus({ preventScroll: true });
                log.scrollTo({
                    top: log.scrollHeight,
                    behavior: matchMedia('(prefers-reduced-motion: reduce)').matches
                        ? 'instant'
                        : 'smooth',
                });
            }}><ArrowDown /></IconButton
        >
    </div>
</div>
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
