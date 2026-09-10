<script lang="ts">
    import SettingsRow from '../../../ui/workspace/SettingsRow.svelte';
    import { tr } from '../../../lib/i18n';
    import ChoiceField from './DataChoice.svelte';
    import type { ProviderWorkspaceDto, TaskProfileDocumentDto } from '../../../lib/ipc/contracts';
    import { taskProfileValidationError } from '../../../features/orchestration/orchestration-controller';
    import { newTask } from '../../../features/providers/settings/settings-defaults';
    import type {
        SettingsDocumentsController,
        SettingsDocumentsState,
    } from '../../../features/providers/settings/settings-documents';
    let {
        controller,
        documentsState,
        workspace,
    }: {
        controller: SettingsDocumentsController;
        documentsState: SettingsDocumentsState;
        workspace: ProviderWorkspaceDto;
    } = $props();
    let draft = $state<TaskProfileDocumentDto | null>(null);
    let revision = $state<number | null>(null);
    let error = $state<string | null>(null);
    const routes = $derived(
        workspace.routes.filter(
            (route) => !workspace.legacy_profiles.some((item) => item.id === route.connection_id),
        ),
    );
    const presets = $derived(
        workspace.presets.filter((item) => item.model_route_id === draft?.route_id),
    );
    function edit(id: string) {
        const task = documentsState.tasks.find((item) => item.value.id === id);
        if (!task) return;
        draft = structuredClone($state.snapshot(task.value));
        revision = task.revision;
        error = null;
    }
    async function save() {
        if (!draft) return;
        error = taskProfileValidationError(draft);
        if (error) return;
        if (!presets.some((item) => item.id === draft?.generation_preset_id)) {
            error = $tr('settingsLive.choosePreset');
            return;
        }
        const result = await controller.saveTask($state.snapshot(draft), revision);
        if (result) draft = null;
    }
    import DataNumber from './DataNumber.svelte';
    export function isDirty() {
        return draft !== null;
    }
</script>

<div class="ui-settings-group">
    <p class="ui-settings-group">{$tr('settingsLive.auxiliaryHint')}</p>
    <div class="ui-settings-group">
        <button
            class="ui-submit ui-pressable"
            onclick={() => {
                draft = newTask('memory_summary');
                revision = null;
                error = null;
            }}
            disabled={documentsState.busy}
            ><span class="ui-press-visual">{$tr('settingsLive.addSummaryModel')}</span></button
        >
        <button
            class="ui-submit ui-pressable"
            onclick={() => {
                draft = newTask('memory_embedding');
                revision = null;
                error = null;
            }}
            disabled={documentsState.busy}
            ><span class="ui-press-visual">{$tr('settingsLive.addEmbeddingModel')}</span></button
        >
    </div>
    {#each documentsState.tasks.filter((item) => item.value.kind === 'memory_summary' || item.value.kind === 'memory_embedding') as item (item.value.id)}<SettingsRow
            label={$tr(
                item.value.kind === 'memory_summary'
                    ? 'settingsUi.summaryModelGroup'
                    : 'settingsUi.embeddingModelGroup',
            )}
            value={workspace.routes.find((route) => route.id === item.value.route_id)
                ?.display_name ?? item.value.route_id}
            disabled={documentsState.busy || draft !== null}
            onclick={() => edit(item.value.id)}
        />{/each}
    {#if draft}
        <section class="ui-settings-group">
            <h3>
                {$tr(
                    draft.kind === 'memory_summary'
                        ? 'settingsUi.summaryModelGroup'
                        : 'settingsUi.embeddingModelGroup',
                )}
            </h3>
            <ChoiceField
                label={$tr('settingsLive.modelRoute')}
                value={draft.route_id}
                options={[
                    { value: '', label: $tr('settingsLive.chooseModel') },
                    ...routes.map((item) => ({
                        value: item.id,
                        label: item.display_name ?? item.model_id,
                    })),
                ]}
                disabled={documentsState.busy}
                onSelect={(value: string) => {
                    if (draft) {
                        draft.route_id = value;
                        draft.generation_preset_id = '';
                    }
                }}
            />
            <ChoiceField
                label={$tr('settingsLive.generationPreset')}
                value={draft.generation_preset_id}
                options={[
                    { value: '', label: $tr('settingsLive.choosePreset') },
                    ...presets.map((item) => ({ value: item.id, label: item.display_name })),
                ]}
                disabled={documentsState.busy}
                onSelect={(value: string) => {
                    if (draft) draft.generation_preset_id = value;
                }}
            />
            {#if draft.kind === 'memory_embedding'}<DataNumber
                    label={$tr('settingsUi.dimensions')}
                    value={draft.embedding_dimensions}
                    min={1}
                    max={32768}
                    disabled={documentsState.busy}
                    onchange={(value: number) => {
                        if (draft) draft.embedding_dimensions = value;
                    }}
                />{/if}
            <DataNumber
                label={$tr('settingsLive.timeout')}
                value={draft.timeout_ms}
                min={1}
                max={600000}
                disabled={documentsState.busy}
                onchange={(value: number) => {
                    if (draft) draft.timeout_ms = value;
                }}
            />
            <DataNumber
                label={$tr('settingsLive.requestCount')}
                value={draft.rate_limit.requests}
                min={1}
                max={10000}
                disabled={documentsState.busy}
                onchange={(value: number) => {
                    if (draft) draft.rate_limit.requests = value;
                }}
            />
            <DataNumber
                label={$tr('settingsLive.requestSeconds')}
                value={draft.rate_limit.per_seconds}
                min={1}
                max={3600}
                disabled={documentsState.busy}
                onchange={(value: number) => {
                    if (draft) draft.rate_limit.per_seconds = value;
                }}
            />
            <DataNumber
                label={$tr('settingsLive.concurrency')}
                value={draft.concurrency_limit}
                min={1}
                max={32}
                disabled={documentsState.busy}
                onchange={(value: number) => {
                    if (draft) draft.concurrency_limit = value;
                }}
            />
            {#if error}<p class="error" role="alert">{error}</p>{/if}
            <div class="ui-settings-group">
                <button
                    class="ui-submit ui-pressable"
                    type="button"
                    onclick={() => void save()}
                    disabled={documentsState.busy}
                    ><span class="ui-press-visual">{$tr('settingsLive.saveModel')}</span></button
                ><button
                    class="ui-submit ui-pressable"
                    type="button"
                    disabled={documentsState.busy}
                    onclick={() => (draft = null)}
                    ><span class="ui-press-visual">{$tr('settingsLive.cancel')}</span></button
                >
            </div>
        </section>
    {/if}
</div>
