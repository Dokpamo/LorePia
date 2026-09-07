<script lang="ts">
    import type { ComponentProps } from 'svelte';
    import Shared from '../../ui/workspace/CreatorPage.svelte';
    let props: ComponentProps<typeof Shared> = $props();
</script>

<Shared {...props} />
