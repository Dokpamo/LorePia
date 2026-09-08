<script lang="ts">
    import { onMount } from 'svelte';
    import type { LorepiaAppController } from '../../app/app-controller';
    import type { ProviderDiagnosticsKind } from '../../app/controllers/provider-workspace-loader';
    import { tr } from '../../lib/i18n';

    let {
        controller,
        kind,
        label,
    }: {
        controller: LorepiaAppController;
        kind: ProviderDiagnosticsKind;
        label: string;
    } = $props();
    let busy = $state(false);
    let error = $state<string | null>(null);
    let epoch = 0;

    async function refresh(): Promise<void> {
        const current = ++epoch;
        busy = true;
        const result = await controller.loadProviderDiagnostics(kind);
        if (current !== epoch) return;
        error = result;
        busy = false;
    }

    onMount(() => {
        void refresh();
        return () => {
            epoch += 1;
        };
    });
</script>

{#if error !== null}
    <div aria-busy={busy}>
        <p role="alert">{error}</p>
        <button
            type="button"
            disabled={busy}
            aria-label={`${label}: ${$tr('common.refresh')}`}
            onclick={() => void refresh()}>{$tr('app.bootstrap.retry')}</button
        >
    </div>
{/if}
