<script lang="ts">
    import { tr } from '../../../lib/i18n';
    import ChoiceField from '../../../components/ChoiceField.svelte';
    import type { ProviderWorkspaceDto, TaskProfileDocumentDto } from '../../../lib/ipc/contracts';
    import { taskProfileValidationError } from '../../orchestration/orchestration-controller';
    import { newTask } from './settings-defaults';
    import type { SettingsDocumentsController, SettingsDocumentsState } from './settings-documents';
    let { controller, documentsState, workspace }: { controller: SettingsDocumentsController; documentsState: SettingsDocumentsState; workspace: ProviderWorkspaceDto } = $props();
    let draft = $state<TaskProfileDocumentDto | null>(null);
    let revision = $state<number | null>(null);
    let error = $state<string | null>(null);
    const routes = $derived(workspace.routes.filter((route) => !workspace.legacy_profiles.some((item) => item.id === route.connection_id)));
    const presets = $derived(workspace.presets.filter((item) => item.model_route_id === draft?.route_id));
    function edit(id: string) {
        const task = documentsState.tasks.find((item) => item.value.id === id);
        if (!task) return;
        draft = structuredClone($state.snapshot(task.value)); revision = task.revision; error = null;
    }
    async function save() {
        if (!draft) return;
        error = taskProfileValidationError(draft);
        if (error) return;
        if (!presets.some((item) => item.id === draft?.generation_preset_id)) { error = $tr('settingsLive.choosePreset'); return; }
        const result = await controller.saveTask($state.snapshot(draft), revision);
        if (result) draft = null;
    }
</script>

<div class="settings-models">
    <p class="settings-lead">{$tr('settingsLive.auxiliaryHint')}</p>
    <div class="settings-inline-actions">
        <button onclick={() => { draft = newTask('memory_summary'); revision = null; error = null; }} disabled={documentsState.busy}>{$tr('settingsLive.addSummaryModel')}</button>
        <button onclick={() => { draft = newTask('memory_embedding'); revision = null; error = null; }} disabled={documentsState.busy}>{$tr('settingsLive.addEmbeddingModel')}</button>
    </div>
    {#each documentsState.tasks.filter((item) => item.value.kind === 'memory_summary' || item.value.kind === 'memory_embedding') as item (item.value.id)}
        <button class="settings-purpose-row" disabled={documentsState.busy} onclick={() => edit(item.value.id)}><span><strong>{$tr(item.value.kind === 'memory_summary' ? 'settingsUi.summaryModelGroup' : 'settingsUi.embeddingModelGroup')}</strong><small>{workspace.routes.find((route) => route.id === item.value.route_id)?.display_name ?? workspace.routes.find((route) => route.id === item.value.route_id)?.model_id ?? item.value.route_id}</small></span><span>{$tr('settingsUi.edit')}</span></button>
    {/each}
    {#if draft}
        <form class="settings-form settings-purpose-card" onsubmit={(event) => { event.preventDefault(); void save(); }}>
            <h3>{$tr(draft.kind === 'memory_summary' ? 'settingsUi.summaryModelGroup' : 'settingsUi.embeddingModelGroup')}</h3>
            <ChoiceField id="settings-task-route" label={$tr('settingsLive.modelRoute')} value={draft.route_id} options={[{ value: '', label: $tr('settingsLive.chooseModel') }, ...routes.map((item) => ({ value: item.id, label: item.display_name ?? item.model_id }))]} disabled={documentsState.busy} onSelect={(value) => { if (draft) { draft.route_id = value; draft.generation_preset_id = ''; } }} />
            <ChoiceField id="settings-task-preset" label={$tr('settingsLive.generationPreset')} value={draft.generation_preset_id} options={[{ value: '', label: $tr('settingsLive.choosePreset') }, ...presets.map((item) => ({ value: item.id, label: item.display_name }))]} disabled={documentsState.busy} onSelect={(value) => { if (draft) draft.generation_preset_id = value; }} />
            {#if draft.kind === 'memory_embedding'}<label>{$tr('settingsUi.dimensions')}<input type="number" bind:value={draft.embedding_dimensions} min="1" max="32768" required disabled={documentsState.busy} /></label>{/if}
            <label>{$tr('settingsLive.timeout')}<input type="number" bind:value={draft.timeout_ms} min="1" max="600000" required disabled={documentsState.busy} /></label>
            <label>{$tr('settingsLive.requestCount')}<input type="number" bind:value={draft.rate_limit.requests} min="1" max="10000" required disabled={documentsState.busy} /></label>
            <label>{$tr('settingsLive.requestSeconds')}<input type="number" bind:value={draft.rate_limit.per_seconds} min="1" max="3600" required disabled={documentsState.busy} /></label>
            <label>{$tr('settingsLive.concurrency')}<input type="number" bind:value={draft.concurrency_limit} min="1" max="32" required disabled={documentsState.busy} /></label>
            {#if error}<p class="error" role="alert">{error}</p>{/if}
            <div class="settings-inline-actions"><button class="primary" type="submit" disabled={documentsState.busy}>{$tr('settingsLive.saveModel')}</button><button type="button" disabled={documentsState.busy} onclick={() => (draft = null)}>{$tr('settingsLive.cancel')}</button></div>
        </form>
    {/if}
</div>
