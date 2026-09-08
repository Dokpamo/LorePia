<script lang="ts">
    import { onMount } from 'svelte';
    import { tr } from '../../../lib/i18n';
    import { errorLabel } from '../../../features/orchestration/controllers/orchestration-state';
    import type { StorageOverviewDto } from '../../../lib/ipc/contracts';
    import type { SettingsServices } from '../../../features/providers/settings/settings-services';
    let { services }: { services: SettingsServices } = $props();
    let overview = $state<StorageOverviewDto | null>(null);
    let loading = $state(false);
    let error = $state('');
    let epoch = 0;
    let disposed = false;
    const exports = $derived(services.contentPackageState.completed_package_exports);
    const packageBytes = $derived(exports.reduce((sum, item) => sum + item.size_bytes, 0));
    const size = (bytes: number) =>
        bytes >= 1048576 ? `${(bytes / 1048576).toFixed(1)} MB` : `${(bytes / 1024).toFixed(1)} KB`;
    async function refresh() {
        const current = ++epoch;
        loading = true;
        error = '';
        try {
            if (!services.client.getStorageOverview)
                throw new Error($tr('settingsLive.unsupported'));
            const result = await services.client.getStorageOverview();
            if (disposed || current !== epoch) return;
            if (
                !Object.values(result).every(
                    (value) =>
                        typeof value === 'number' && Number.isSafeInteger(value) && value >= 0,
                )
            )
                throw new Error($tr('settingsLive.invalidStorage'));
            overview = result;
            await services.contentPackageController.loadCompletedPackageExports();
        } catch (cause) {
            if (!disposed && current === epoch) error = errorLabel(cause);
        } finally {
            if (!disposed && current === epoch) loading = false;
        }
    }
    onMount(() => {
        void refresh();
        return () => {
            disposed = true;
            epoch++;
        };
    });
    import DataAction from './DataAction.svelte';
</script>

<section class="ui-settings-group">
    <DataAction disabled={loading} onclick={() => void refresh()}
        >{$tr('settingsLive.refresh')}</DataAction
    >
    {#if loading}<p role="status">{$tr('settingsLive.loading')}</p>{/if}
    {#if error}<p role="alert">{error}</p>{/if}
    {#if overview}<h2>{$tr('settingsLive.storedData')}</h2>
        <p>{$tr('settingsLive.characters')} <strong>{overview.characters}</strong></p>
        <p>{$tr('settingsLive.conversations')} <strong>{overview.conversations}</strong></p>
        <p>{$tr('settingsLive.messages')} <strong>{overview.messages}</strong></p>{/if}
    <h2>{$tr('settingsLive.originals')}</h2>
    <p>{size(packageBytes)}</p>
    <p>{$tr('settingsLive.originalsHint')}</p>
    {#each exports as item (item.source_id)}<DataAction
            disabled={services.contentPackageState.exporting_import_id !== null}
            onclick={() =>
                void services.contentPackageController.exportCompletedPackageFromCatalog(
                    item.source_id,
                )}
            >{item.suggested_file_name} · {size(item.size_bytes)} · {$tr(
                'settingsLive.export',
            )}</DataAction
        >{/each}
    {#if !exports.length && !services.contentPackageState.completed_exports_loading}<p>
            {$tr('settingsLive.emptyOriginals')}
        </p>{/if}
    {#if services.contentPackageState.completed_exports_error}<p role="alert">
            {services.contentPackageState.completed_exports_error}
        </p>{/if}
    {#if services.contentPackageState.export_error}<p role="alert">
            {services.contentPackageState.export_error}
        </p>{/if}
    {#if services.contentPackageState.export_receipt}<p role="status">
            {$tr('settingsLive.exported')}
        </p>{/if}
</section>
