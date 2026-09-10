<script lang="ts">
    import { onMount } from 'svelte';
    import { tr } from '../../../lib/i18n';
    import { errorLabel } from '../../orchestration/controllers/orchestration-state';
    import type { StorageOverviewDto } from '../../../lib/ipc/contracts';
    import type { SettingsServices } from './settings-services';
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
</script>

<div class="settings-inline-actions">
    <button disabled={loading} onclick={() => void refresh()}>{$tr('settingsLive.refresh')}</button>
</div>
{#if loading}<p role="status">{$tr('settingsLive.loading')}</p>{/if}
{#if error}<p role="alert" class="error">{error}</p>{/if}
{#if overview}
    <section
        class="settings-purpose-card settings-storage-counts"
        aria-label={$tr('settingsLive.storedData')}
    >
        <h2>{$tr('settingsLive.storedData')}</h2>
        <dl>
            <div>
                <dt>{$tr('settingsLive.characters')}</dt>
                <dd>{overview.characters}</dd>
            </div>
            <div>
                <dt>{$tr('settingsLive.conversations')}</dt>
                <dd>{overview.conversations}</dd>
            </div>
            <div>
                <dt>{$tr('settingsLive.messages')}</dt>
                <dd>{overview.messages}</dd>
            </div>
        </dl>
    </section>
{/if}
<section class="settings-purpose-card settings-form" aria-label={$tr('settingsLive.originals')}>
    <h2>{$tr('settingsLive.originals')}</h2>
    <p class="settings-storage-size">{size(packageBytes)}</p>
    <p class="settings-note">{$tr('settingsLive.originalsHint')}</p>
    {#if services.contentPackageState.completed_exports_error}<p class="error" role="alert">
            {services.contentPackageState.completed_exports_error}
        </p>{/if}
    {#each exports as item (item.source_id)}
        <div class="settings-export-row">
            <span
                ><strong>{item.suggested_file_name}</strong><small>{size(item.size_bytes)}</small
                ></span
            ><button
                disabled={services.contentPackageState.exporting_import_id !== null}
                onclick={() =>
                    void services.contentPackageController.exportCompletedPackageFromCatalog(
                        item.source_id,
                    )}>{$tr('settingsLive.export')}</button
            >
        </div>
    {/each}
    {#if !exports.length && !services.contentPackageState.completed_exports_loading}<p
            class="settings-empty"
        >
            {$tr('settingsLive.emptyOriginals')}
        </p>{/if}
    {#if services.contentPackageState.export_error}<p class="error" role="alert">
            {services.contentPackageState.export_error}
        </p>{/if}
    {#if services.contentPackageState.export_receipt}<p role="status">
            {$tr('settingsLive.exported')}
        </p>{/if}
</section>
