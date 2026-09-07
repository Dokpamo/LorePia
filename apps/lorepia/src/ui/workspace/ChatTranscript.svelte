<script lang="ts">
    import type { Snippet } from 'svelte';
    import type { SampleMessage } from './view-types';
    import { onDestroy } from 'svelte';
    import { tr } from '../../lib/i18n';
    import type {
        ChatScrollLifecycle,
        MessageCollectionSnapshot,
        MessageMeasurementInput,
    } from '../../features/chat/chat-scroll.svelte';
    import ChatMessage from './ChatMessage.svelte';
    import { messageExpansion } from './message-expansion';
    import type { SampleCharacter, SampleConversation } from './view-types';
    import type { ChatSession } from './chat-session';
    let {
        character,
        conversation,
        session,
        renderMessage,
        extras,
        scroll,
        collection,
        active,
        onactive,
        onnotice,
        onwrite,
    }: {
        character: SampleCharacter;
        conversation: SampleConversation;
        session: ChatSession;
        renderMessage?: Snippet<[SampleMessage]>;
        extras?: Snippet;
        scroll: ChatScrollLifecycle;
        collection: MessageCollectionSnapshot;
        active: string | null;
        onactive: (id: string | null) => void;
        onnotice: (text: string, retry?: () => void) => void;
        onwrite: () => void;
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
            const tools = row?.querySelector('.ui-message-tools-shell');
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
        data-tools-open={!!active}
        use:messageExpansion={{ active, start: viewport.start, end: viewport.end }}
        style:padding-top={String(22 + viewport.topSpacer) + 'px'}
        style:padding-bottom={'calc(' +
            String(22 + viewport.bottomSpacer) +
            'px + var(--ui-composer-overlay, 84px))'}
    >
        {#if conversation.messages.length === 0}<div class="ui-result-empty">
                <p>{$tr('uiPreview.emptyChat')}</p>
                <button class="ui-result-action ui-pressable" onclick={onwrite}
                    ><span class="ui-press-visual">{$tr('uiPreview.writeMessage')}</span></button
                >
            </div>{/if}
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
                    {renderMessage}
                    active={active === message.id}
                    onactivate={() => activate(active === message.id ? null : message.id)}
                    onclose={() => activate(null)}
                    focusReturn={scroll.scroller}
                    {onnotice}
                />
            </article>
        {/each}
        {#if extras}{@render extras()}{/if}
    </div>
</div>
