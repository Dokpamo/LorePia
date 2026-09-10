<script lang="ts">
    import { House, MessageCircle, CirclePlus, Settings } from '@lucide/svelte';
    import type { RootTab } from './navigation-types';
    let { tab }: { tab: RootTab } = $props();
    const icons = { home: House, chats: MessageCircle, create: CirclePlus, settings: Settings };
    const Icon = $derived(icons[tab]);
</script>

<Icon size={24} aria-hidden="true" />
