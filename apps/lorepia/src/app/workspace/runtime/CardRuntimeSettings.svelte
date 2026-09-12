<script lang="ts">
    import type { PortableRuntimeLifecycle } from '../../../features/chat/portable-runtime-lifecycle.svelte';
    import RuntimeControls from './RuntimeControls.svelte';
    import { tr } from '../../../lib/i18n';
    let { runtime }: { runtime: PortableRuntimeLifecycle } = $props();
</script>

{#if runtime.profileLoading}
    <p role="status">{$tr('chat.runtime.profileLoading')}</p>
{:else if runtime.profileError}
    <p role="alert">{runtime.profileError}</p>
    <button class="ui-submit ui-pressable" onclick={() => runtime.retryProfile()}>
        <span class="ui-press-visual">{$tr('chat.runtime.retryProfile')}</span>
    </button>
{:else if runtime.profile}
    {#if runtime.error}<p role="alert">{runtime.error}</p>{/if}
    <RuntimeControls {runtime} />
{/if}
