<script lang="ts">
    import { onDestroy } from 'svelte';
    import { ArrowDown } from '@lucide/svelte';
    import { tr } from '../../lib/i18n';
    import type {
        ChatScrollLifecycle,
        MessageCollectionSnapshot,
        MessageMeasurementInput,
    } from '../../features/chat/chat-scroll.svelte';
    import ChatMessage from './ChatMessage.svelte';
    import IconButton from './IconButton.svelte';
    import type { SampleCharacter, SampleConversation } from './sample-data';
    import type { SampleChatSession } from './sample-chat.svelte';
    let {
        character,
        conversation,
        session,
        scroll,
        collection,
        active,
        onactive,
    }: {
        character: SampleCharacter;
        conversation: SampleConversation;
        session: SampleChatSession;
        scroll: ChatScrollLifecycle;
        collection: MessageCollectionSnapshot;
        active: string | null;
        onactive: (id: string | null) => void;
    } = $props();
    const viewport = $derived(scroll.virtualWindow());
    const visible = $derived(conversation.messages.slice(viewport.start, viewport.end));
    let revealFrame: number | undefined;
    function measure(node: HTMLElement, input: MessageMeasurementInput) {
        return scroll.measureMessage(node, input);
    }
    function interruptReveal() {
        if (revealFrame !== undefined) cancelAnimationFrame(revealFrame);
        revealFrame = undefined;
    }
    function activate(id: string | null) {
        interruptReveal();
        onactive(id);
        if (!id) return;
        scroll.stabilizeMessageActionLayout(id);
        // Follow the expanding inline row, stopping as soon as the user scrolls.
        const until = performance.now() + 400;
        const reveal = () => {
            const log = scroll.scroller;
            if (!log || active !== id) return;
            const row = [...log.querySelectorAll<HTMLElement>('[data-message-id]')].find(
                (item) => item.dataset.messageId === id,
            );
            const tools = row?.querySelector('.ui-message-tools');
            const dock = log.parentElement?.querySelector('.ui-compose-field');
            if (tools && dock) {
                const overlap =
                    tools.getBoundingClientRect().bottom - dock.getBoundingClientRect().top + 12;
                const scale = log.getBoundingClientRect().width / log.offsetWidth || 1;
                if (overlap > 0)
                    scroll.applyProgrammaticScrollPosition(log, log.scrollTop + overlap / scale);
            }
            revealFrame = performance.now() < until ? requestAnimationFrame(reveal) : undefined;
        };
        revealFrame = requestAnimationFrame(reveal);
    }
    onDestroy(interruptReveal);
</script>

<div
    class="ui-messages"
    bind:this={scroll.scroller}
    role="log"
    tabindex="-1"
    aria-label={$tr('uiPreview.messages')}
    onscroll={(event) => scroll.handleScroll(event)}
    onwheel={interruptReveal}
    onpointerdown={(event) => {
        interruptReveal();
        if (!(event.target instanceof Element) || !event.target.closest('[data-message-id]'))
            activate(null);
    }}
>
    <div
        class="ui-transcript-list"
        style:padding-top={String(22 + viewport.topSpacer) + 'px'}
        style:padding-bottom={'calc(' +
            String(22 + viewport.bottomSpacer) +
            'px + var(--ui-composer-overlay, 84px))'}
    >
        {#if conversation.messages.length === 0}<p class="ui-chat-empty">
                {$tr('uiPreview.emptyChat')}
            </p>{/if}
        {#each visible as message, index (message.id)}
            <article
                class="ui-message-turn"
                class:ui-user-turn={message.role === 'user'}
                data-message-id={message.id}
                data-active={active === message.id}
                tabindex="-1"
                aria-posinset={viewport.start + index + 1}
                aria-setsize={collection.items.length}
                use:measure={{
                    messageId: message.id,
                    epoch: scroll.measurementEpoch,
                    includesDayDivider: false,
                }}
            >
                {#if viewport.start + index === 0}<div class="ui-date">
                        <span>{conversation.date}</span>
                    </div>{/if}
                <ChatMessage
                    {message}
                    {character}
                    {session}
                    active={active === message.id}
                    onactivate={() => activate(active === message.id ? null : message.id)}
                    onclose={() => activate(null)}
                    focusReturn={scroll.scroller}
                />
            </article>
        {/each}
    </div>
</div>
{#if !scroll.nearBottom}<div class="ui-jump-latest">
        <IconButton
            label={$tr('uiPreview.latestMessage')}
            onclick={() => {
                const log = scroll.scroller;
                if (log)
                    log.scrollTo({
                        top: log.scrollHeight,
                        behavior: matchMedia('(prefers-reduced-motion: reduce)').matches
                            ? 'instant'
                            : 'smooth',
                    });
            }}><ArrowDown /></IconButton
        >
    </div>{/if}
