<script lang="ts">
    import { ArrowLeft } from '@lucide/svelte';
    import type { PortableRuntimeLifecycle } from '../../features/chat/portable-runtime-lifecycle.svelte';
    import PortableRuntimeControls from '../../features/chat/PortableRuntimeControls.svelte';
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
    <IconButton label={$tr('workspace.back')} onclick={onback}><ArrowLeft /></IconButton>
    <h1>{$tr('uiPreview.subpage')}</h1>
</header>
<div class="ui-feature-content ui-creator-content">
    {#if runtime.error}<p role="alert">{runtime.error}</p>{/if}
    <PortableRuntimeControls
        phase={runtime.phase}
        grant={runtime.activeGrant}
        capabilities={runtime.capabilities}
        bind:selectedCapabilities={runtime.selectedCapabilities}
        runtime={runtime.runtime}
        selectedAuxiliaryModel={runtime.selectedAuxiliaryModel}
        auxiliaryModelOptions={runtime.auxiliaryModelOptions}
        modelBudget={runtime.modelBudget}
        modelCall={runtime.modelCall}
        persistenceStatus={runtime.persistenceStatus}
        optionValue={(key) => runtime.optionValue(key)}
        onApprove={() => runtime.approve()}
        onRevoke={() => runtime.revoke()}
        onSelectAuxiliaryModel={(value) => runtime.setAuxiliaryModel(value)}
        onSetOption={(key, value) => runtime.setOption(key, value)}
        onCancelModelCall={async () => {
            await runtime.cancelActiveModelCall();
        }}
    />
    {#if runtime.displayApproved && runtime.background}
        <PortableMessage
            text={runtime.background}
            {client}
            profile={runtime.profile}
            enabled={runtime.displayApproved}
            variables={runtime.variables}
            onAction={(action) => void runtime.handleAction(action)}
        />
    {/if}
</div>
