<script lang="ts">
    import { House, MessageCircle, SquarePen, Settings } from '@lucide/svelte';
    import type { RootTab } from './navigation-types';
    let { tab }: { tab: RootTab } = $props();
    const icons = { home: House, chats: MessageCircle, create: SquarePen, settings: Settings };
    const Icon = $derived(icons[tab]);
</script>

<Icon size={24} aria-hidden="true" />
