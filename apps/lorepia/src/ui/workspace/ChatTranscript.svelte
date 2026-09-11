<script lang="ts">
    import type { Snippet } from 'svelte';
    import type { SampleMessage } from './view-types';
    import { tr } from '../../lib/i18n';
    import type {
        ChatScrollLifecycle,
        MessageCollectionSnapshot,
        MessageMeasurementInput,
    } from '../../features/chat/chat-scroll.svelte';
    import ChatMessage from './ChatMessage.svelte';
    import type { SampleCharacter, SampleConversation } from './view-types';
    import type { ChatSession } from './chat-session';
    import type { LorepiaClient } from '../../lib/ipc/contracts';
    let {
        character,
        client,
        personaName,
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
        client?: Pick<LorepiaClient, 'resolveAssetDelivery'>;
        personaName?: string;
        conversation: SampleConversation;
        session: ChatSession;
        renderMessage?: Snippet<[SampleMessage, number]>;
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
    function measure(node: HTMLElement, input: MessageMeasurementInput) {
        return scroll.measureMessage(node, input);
    }
    function activate(id: string | null) {
        onactive(id);
    }
</script>

<div
    class="ui-messages"
    bind:this={scroll.scroller}
    role="log"
    tabindex="-1"
    aria-label={$tr('uiPreview.messages')}
    onscroll={(event) => scroll.handleScroll(event)}
    onpointerdown={(event) => {
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
                <ChatMessage
                    {client}
                    {personaName}
                    mode={conversation.mode}
                    {message}
                    {character}
                    {session}
                    {renderMessage}
                    messageIndex={viewport.start + index}
                    active={active === message.id}
                    branchId={conversation.activeBranchId}
                    onactivate={() => activate(message.id)}
                    onclose={() => activate(null)}
                    focusReturn={scroll.scroller}
                    {onnotice}
                />
            </article>
        {/each}
        {#if extras}<div class="ui-transcript-extras">{@render extras()}</div>{/if}
    </div>
</div>
