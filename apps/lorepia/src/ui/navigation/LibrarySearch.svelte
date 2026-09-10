<script lang="ts">
    import { Search, X } from '@lucide/svelte';
    import { tr } from '../../lib/i18n';

    let { query = $bindable(''), label }: { query?: string; label: string } = $props();
    let input: HTMLInputElement;
    export function focusInput() {
        input.focus({ preventScroll: true });
    }
</script>

<div class="seed-search seed-library-search">
    <Search aria-hidden="true" />
    <input
        bind:this={input}
        type="search"
        bind:value={query}
        aria-label={label}
        placeholder={label}
    />
    {#if query}
        <button
            class="seed-search-clear ui-pressable"
            aria-label={$tr('navigation.clearSearch')}
            onclick={() => {
                query = '';
                input.focus();
            }}><span class="ui-press-visual"><X aria-hidden="true" /></span></button
        >
    {/if}
</div>
