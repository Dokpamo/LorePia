<script lang="ts">
    import { locale, tr } from '../../lib/i18n';
    import { chatActivityTime } from './chat-activity';
    let { value, caption = false }: { value?: string; caption?: boolean } = $props();
    const time = $derived(value ? chatActivityTime(value, $locale) : null);
</script>

{#if time}
    <time
        class="seed-last-chat"
        class:seed-last-chat-caption={caption}
        datetime={value}
        title={time.full}
        aria-label={$tr('navigation.lastChatAt', { time: time.full })}
        >{caption ? $tr('navigation.lastChatAt', { time: time.short }) : time.short}</time
    >
{/if}

<style>
    .seed-last-chat {
        flex-shrink: 0;
        align-self: flex-start;
        padding-top: 3px;
        color: var(--ui-muted);
        font-size: var(--ui-caption);
        line-height: 1.5;
        font-variant-numeric: tabular-nums;
        white-space: nowrap;
    }
    .seed-last-chat-caption {
        display: block;
        padding-top: 4px;
        font-size: calc(12px * var(--ui-text-scale));
        white-space: normal;
    }
</style>
