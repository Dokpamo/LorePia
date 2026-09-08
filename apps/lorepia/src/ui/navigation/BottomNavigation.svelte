<script lang="ts">
    import TabIcon from './TabIcon.svelte';
    import { tr } from '../../lib/i18n';
    import type { RootTab } from './navigation-types';
    let { selected, onchange }: { selected: RootTab; onchange: (tab: RootTab) => void } = $props();
    const items = [
        { id: 'home', label: 'navigation.home' },
        { id: 'chats', label: 'navigation.chats' },
        { id: 'create', label: 'navigation.create' },
        { id: 'settings', label: 'navigation.settings' },
    ] as const;
</script>

<nav class="seed-bottom-navigation" aria-label={$tr('navigation.label')}>
    <div class="seed-bottom-items">
        {#each items as item (item.id)}
            <button
                class="seed-tab ui-pressable"
                aria-current={selected === item.id ? 'page' : undefined}
                onclick={() => onchange(item.id)}
            >
                <span class="ui-press-visual"
                    ><TabIcon tab={item.id} /><span>{$tr(item.label)}</span></span
                >
            </button>
        {/each}
    </div>
</nav>
