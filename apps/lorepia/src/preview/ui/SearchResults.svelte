<script lang="ts">
    import type { ComponentProps } from 'svelte';
    import Shared from '../../ui/workspace/SearchResults.svelte';
    let props: ComponentProps<typeof Shared> = $props();
</script>

<Shared {...props} />
