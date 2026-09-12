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
    {#if !runtime.displayApproved || !runtime.canReadChat}
        <section class="ui-settings-group">
            <h2 class="ui-settings-heading">{$tr('workspaceRuntime.title')}</h2>
            <p>{$tr('chat.runtime.enableDisplayHint')}</p>
            <button
                class="ui-submit ui-pressable"
                disabled={runtime.phase === 'loading'}
                onclick={() => void runtime.approveDisplay()}
            >
                <span class="ui-press-visual">{$tr('chat.runtime.enableDisplay')}</span>
            </button>
        </section>
    {/if}
    <RuntimeControls {runtime} />
{/if}
