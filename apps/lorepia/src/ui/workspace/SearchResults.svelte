<script lang="ts">
    import { MessageSquare, SearchX } from '@lucide/svelte';
    import { tr } from '../../lib/i18n';
    let {
        items,
        query,
        onselect,
        onclear,
    }: {
        items: { id: string; title: string; date: string }[];
        query: string;
        onselect: (id: string) => void;
        onclear: () => void;
    } = $props();
</script>

<div class="ui-search-results" data-ui-selectable>
    <p class="ui-search-count" role="status">
        {$tr('uiPreview.searchCount', { count: items.length })}
    </p>
    {#if items.length}
        <ul aria-label={$tr('uiPreview.searchResults')}>
            {#each items as item (item.id)}
                <li>
                    <button
                        type="button"
                        class="ui-history-item ui-search-result ui-pressable"
                        aria-label={$tr('uiPreview.historySelect', item)}
                        onclick={() => onselect(item.id)}
                        ><span class="ui-history-content ui-press-visual">
                            <MessageSquare aria-hidden="true" />
                            <span><strong>{item.title}</strong><small>{item.date}</small></span>
                        </span></button
                    >
                </li>
            {/each}
        </ul>
    {:else}
        <div class="ui-result-empty">
            <SearchX aria-hidden="true" />
            <strong>{$tr(query.trim() ? 'uiPreview.noChatMatches' : 'uiPreview.noChats')}</strong>
            <p>{$tr(query.trim() ? 'uiPreview.searchAgain' : 'uiPreview.newChatHint')}</p>
            {#if query}<button class="ui-result-action ui-pressable" onclick={onclear}
                    ><span class="ui-press-visual">{$tr('uiPreview.clearChatSearch')}</span></button
                >{/if}
        </div>
    {/if}
</div>
