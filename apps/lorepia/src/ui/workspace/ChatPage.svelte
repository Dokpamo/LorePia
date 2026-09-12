<script lang="ts">
    import type { Snippet } from 'svelte';
    import type { SampleMessage } from './view-types';
    import { onMount, untrack, tick } from 'svelte';
    import { ArrowDown, ArrowLeft, PanelRight } from '@lucide/svelte';
    import { tr } from '../../lib/i18n';
    import type { LorepiaClient, MessageDto } from '../../lib/ipc/contracts';
    import {
        ChatScrollLifecycle,
        type MessageCollectionSnapshot,
    } from '../../features/chat/chat-scroll.svelte';
    import IconButton from './IconButton.svelte';
    import type { MessageHistoryState } from '../../app/controllers/message-history-controller';
    import ChatTranscript from './ChatTranscript.svelte';
    import MessageComposer from './MessageComposer.svelte';
    import UiNotice from './UiNotice.svelte';
    import type { UiNoticeValue } from './notice';
    import type { ComposerDockMetrics } from './composer-measure';
    import type { ChatSession } from './chat-session';
    import type { Page, SampleCharacter, SampleConversation } from './view-types';
    let {
        character,
        client,
        personaName,
        conversation,
        session,
        history,
        renderMessage,
        extras,
        cardSurface,
        externalNotice = '',
        draft,
        managementVisible = false,
        creatorVisible = false,
        onnavigate,
        ondraft,
        onsend,
        onsettings,
        oninteract,
        loading = false,
        loadError = null,
        onretry,
    }: {
        character: SampleCharacter;
        client?: Pick<LorepiaClient, 'resolveAssetDelivery'>;
        personaName?: string;
        conversation: SampleConversation;
        session: ChatSession;
        history?: MessageHistoryState;
        renderMessage?: Snippet<[SampleMessage, number]>;
        extras?: Snippet;
        cardSurface?: Snippet;
        externalNotice?: string;
        draft: string;
        managementVisible?: boolean;
        creatorVisible?: boolean;
        onnavigate: (page: Page) => void;
        ondraft: (value: string) => void;
        onsend: () => void;
        onsettings: (trigger: HTMLButtonElement) => void;
        oninteract: () => void;
        loading?: boolean;
        loadError?: string | null;
        onretry?: () => void;
    } = $props();
    let active = $state<string | null>(null);
    let underfilled = $state(false);
    let notice = $state<UiNoticeValue | null>(null);
    let noticeId = 0;
    let previousNotice = untrack(() => externalNotice);
    $effect(() => {
        const value = externalNotice;
        untrack(() => {
            if (value && value !== previousNotice) showNotice(value);
            previousNotice = value;
        });
    });
    $effect(() => {
        if (history?.error) {
            const direction = history.failed_direction ?? 'latest';
            untrack(() =>
                showNotice($tr('ux.loading.failed'), () => void session.loadHistory?.(direction)),
            );
        }
    });
    function showNotice(text: string, retry?: () => void) {
        notice = { id: ++noticeId, text, retry };
    }
    // The original scroll owner needs identity/date, not a fresh index per stream token.
    const projected = $derived.by(() => {
        if (conversation.scrollMessages) return conversation.scrollMessages;
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
    function activate(id: string | null) {
        active = id;
        scroll.clearStableMessageActionLayout();
    }
    const branchKey = $derived(
        conversation.id +
            ':' +
            (conversation.activeBranchId ?? 'main') +
            ':' +
            (conversation.mode ?? 'chat'),
    );
    let noticeScope = untrack(
        () => conversation.id + ':' + (conversation.activeBranchId ?? 'main'),
    );
    $effect(() => {
        const key = branchKey;
        const scope = conversation.id + ':' + (conversation.activeBranchId ?? 'main');
        untrack(() => {
            if (noticeScope !== scope) notice = null;
            noticeScope = scope;
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
    async function latest() {
        if (history?.has_newer) {
            await session.loadHistory?.('latest');
            await tick();
        }
    }
    function prefetchHistory() {
        const log = scroll.scroller;
        if (!log || !history || history.loading || history.error) return;
        if (log.scrollTop < 640 && history.has_older) void session.loadHistory?.('older');
        else if (log.scrollHeight - log.clientHeight - log.scrollTop < 640 && history.has_newer)
            void session.loadHistory?.('newer');
    }
    async function sendToTail() {
        if (session.busy || !draft.trim()) return;
        activate(null);
        await latest();
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
    {#if character.subpage && !creatorVisible}<IconButton
            label={$tr('chat.runtime.openRoom')}
            onclick={() => onnavigate(2)}><PanelRight /></IconButton
        >{/if}
</header>
{#if cardSurface}{@render cardSurface()}{/if}
<div class="ui-chat-notices">
    {#if notice}{#key notice.id}<UiNotice {notice} ondismiss={() => (notice = null)} />{/key}{/if}
</div>
<ChatTranscript
    {client}
    {personaName}
    {character}
    {conversation}
    {session}
    {renderMessage}
    {extras}
    {scroll}
    {collection}
    {active}
    {loading}
    {loadError}
    {onretry}
    historyLoading={history?.loading ?? false}
    onhistoryedge={prefetchHistory}
    onunderfill={(value: boolean) => {
        underfilled = value;
    }}
    onactive={activate}
    onnotice={showNotice}
    onwrite={() =>
        scroll.scroller
            ?.closest('.ui-chat')
            ?.querySelector<HTMLTextAreaElement>('.ui-compose textarea')
            ?.focus()}
/>
<div class="ui-chat-floaters">
    {#if underfilled && history?.has_older && !history.loading && !loading && !loadError && typeof session.loadHistory === 'function'}
        <button
            class="ui-result-action ui-pressable"
            style:pointer-events="auto"
            style:align-self="center"
            onclick={() => void session.loadHistory?.('older')}
            ><span class="ui-press-visual">{$tr('pagination.older_messages')}</span></button
        >
    {/if}
    <div
        class="ui-jump-latest"
        data-visible={!scroll.nearBottom || history?.has_newer}
        inert={scroll.nearBottom && !history?.has_newer}
        aria-hidden={scroll.nearBottom && !history?.has_newer}
    >
        <IconButton
            label={$tr('uiPreview.latestMessage')}
            onclick={async () => {
                await latest();
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
    stoppable={session.canStop ?? session.busy}
    inputReady={!loading || conversation.activeBranchId !== undefined}
    submitting={session.submitting ?? false}
    onstop={() => session.stop()}
    ondock={syncDock}
/>
