<script lang="ts">
    import { House, MessageCircleMore, Menu } from '@lucide/svelte';
    import { tr } from '../../lib/i18n';

    interface Props {
        view: string;
        onHome: () => void;
        onChat: () => void;
        onSettings: () => void;
    }

    let { view, onHome, onChat, onSettings }: Props = $props();
    const items = $derived([
        { id: 'home', label: $tr('app.tab.home'), icon: House, select: onHome },
        { id: 'chat', label: $tr('mobile.nav.chat'), icon: MessageCircleMore, select: onChat },
        { id: 'settings', label: $tr('mobile.nav.all'), icon: Menu, select: onSettings },
    ]);
</script>

<nav class="mobile-navigation" data-mobile-navigation aria-label={$tr('app.nav.label')}>
    {#each items as item (item.id)}
        <button
            type="button"
            aria-current={(view === 'create' ? 'settings' : view) === item.id ? 'page' : undefined}
            onclick={item.select}
        >
            <item.icon aria-hidden="true" />
            <span>{item.label}</span>
        </button>
    {/each}
</nav>
