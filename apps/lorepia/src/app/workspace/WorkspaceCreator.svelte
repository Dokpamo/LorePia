<script lang="ts">
    import { ArrowLeft } from '@lucide/svelte';
    import type { PortableRuntimeLifecycle } from '../../features/chat/portable-runtime-lifecycle.svelte';
    import PortableRuntimeControls from './runtime/RuntimeControls.svelte';
    import PortableMessage from '../../features/chat/PortableMessage.svelte';
    import type { LorepiaClient } from '../../lib/ipc/contracts';
    import { tr } from '../../lib/i18n';
    import IconButton from '../../ui/workspace/IconButton.svelte';
    let {
        runtime,
        client,
        onback,
    }: { runtime: PortableRuntimeLifecycle; client: LorepiaClient; onback: () => void } = $props();
</script>

<header class="ui-page-header ui-navigation-header">
    <IconButton label={$tr('uiPreview.backChat')} onclick={onback}><ArrowLeft /></IconButton>
    <div class="ui-title-group">
        <strong>{$tr('uiPreview.subpage')}</strong><small>{$tr('uiPreview.creatorSpace')}</small>
    </div>
</header>
<div class="ui-sub-body ui-live-subpage">
    {#if runtime.error}<p role="alert">{runtime.error}</p>{/if}
    <PortableRuntimeControls {runtime} />
    {#if runtime.displayApproved && runtime.background}
        <PortableMessage
            text={runtime.background}
            {client}
            profile={runtime.profile}
            enabled={runtime.displayApproved}
            variables={runtime.variables}
            onAction={(action: string) => void runtime.handleAction(action)}
        />
    {/if}
</div>
