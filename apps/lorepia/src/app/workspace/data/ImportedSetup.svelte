<script lang="ts">
    import { onMount, untrack } from 'svelte';
    import { tr } from '../../../lib/i18n';
    import type { SettingsServices } from '../../../features/providers/settings/settings-services';
    import type { SettingsDocumentsState } from '../../../features/providers/settings/settings-documents';
    import { ImportedSetupController } from '../../../features/providers/settings/imported-setup-controller';
    import {
        readImportedCompatibilityHints,
        recommendImportedRoute,
        hintString,
        type ImportedCompatibilityKind,
    } from '../../../features/orchestration/imported-compatibility';
    import DataChoice from './DataChoice.svelte';
    import DataAction from './DataAction.svelte';
    let {
        kind,
        services,
        documentsState,
        onsaved,
    }: {
        kind: ImportedCompatibilityKind;
        services: SettingsServices;
        documentsState: SettingsDocumentsState;
        onsaved: () => void;
    } = $props();
    const controller = untrack(() => new ImportedSetupController(() => services));
    const store = controller.state;
    let sourceId = $state('');
    let routeId = $state('');
    let targetPromptId = $state('');
    let embeddingTaskId = $state('');
    let applyToRoom = $state(false);
    const sources = $derived(
        $store.sources.filter((item) => readImportedCompatibilityHints(item.value)?.kind === kind),
    );
    const targets = $derived(
        $store.sources.filter(
            (item) => readImportedCompatibilityHints(item.value)?.kind !== 'memory',
        ),
    );
    const source = $derived(sources.find((item) => item.value.id === sourceId));
    const routes = $derived(
        services.appState.providers.workspace.routes.filter(
            (item) =>
                !services.appState.providers.workspace.legacy_profiles.some(
                    (legacy) => legacy.id === item.connection_id,
                ),
        ),
    );
    const room = $derived(services.appState.selected_conversation);
    const embeddings = $derived(
        documentsState.tasks.filter(
            (item) => item.value.kind === 'memory_embedding' && item.value.embedding_dimensions,
        ),
    );
    const busy = $derived($store.loading || $store.saving);
    function select(id: string) {
        sourceId = id;
        const value = sources.find((item) => item.value.id === id)?.value;
        const hints = readImportedCompatibilityHints(value ?? null);
        routeId = recommendImportedRoute(
            services.appState.providers.workspace,
            hints
                ? hintString(hints, kind === 'memory' ? 'memory_summarization_model' : 'model_hint')
                : null,
        );
        if (!routes.some((item) => item.id === routeId)) routeId = routes[0]?.id ?? '';
        targetPromptId =
            targets.find(
                (item) =>
                    item.value.id ===
                    services.orchestrationState.workspace.room_config.prompt_preset_id,
            )?.value.id ??
            targets[0]?.value.id ??
            '';
        const memoryId = hints ? hintString(hints, 'memory_profile_id') : null;
        embeddingTaskId =
            documentsState.memories.find((item) => item.value.id === memoryId)?.value
                .embedding_task ?? '';
        applyToRoom = room !== null && (kind === 'generation' || targetPromptId !== '');
    }
    onMount(() => {
        void controller.load(documentsState.prompts).then(() => select(sources[0]?.value.id ?? ''));
        return () => controller.destroy();
    });
    export function isBusy() {
        return busy;
    }
</script>

<section class="ui-settings-group">
    {#if $store.loading}<p role="status">{$tr('settingsLive.loading')}</p>
    {:else if sources.length === 0}<p>{$tr('importSetup.empty')}</p>
    {:else}
        <DataChoice
            label={$tr('importSetup.source')}
            value={sourceId}
            options={sources.map((item) => ({ value: item.value.id, label: item.value.name }))}
            onSelect={select}
            disabled={busy}
        />
        <p>{$tr(kind === 'memory' ? 'importSetup.memoryHint' : 'importSetup.promptHint')}</p>
        <DataChoice
            label={$tr(kind === 'memory' ? 'settingsUi.summaryModelGroup' : 'importSetup.model')}
            value={routeId}
            options={[
                { value: '', label: $tr('settingsLive.chooseModel') },
                ...routes.map((item) => ({
                    value: item.id,
                    label: item.display_name ?? item.model_id,
                })),
            ]}
            onSelect={(value: string) => (routeId = value)}
            disabled={busy}
        />
        {#if !routes.length}<p>
                {$tr('orchestration.imported_compatibility.provider_missing')}
            </p>{/if}
        {#if kind === 'memory'}
            <DataChoice
                label={$tr('settingsUi.embeddingModelGroup')}
                value={embeddingTaskId}
                options={[
                    { value: '', label: $tr('importSetup.withoutEmbedding') },
                    ...embeddings.map((item) => ({
                        value: item.value.id,
                        label:
                            services.appState.providers.workspace.routes.find(
                                (route) => route.id === item.value.route_id,
                            )?.display_name ?? item.value.id,
                    })),
                ]}
                onSelect={(value: string) => (embeddingTaskId = value)}
                disabled={busy}
            />
            {#if !embeddingTaskId}<p>{$tr('importSetup.embeddingHint')}</p>{/if}
            <DataChoice
                label={$tr('importSetup.targetPrompt')}
                value={targetPromptId}
                options={[
                    { value: '', label: $tr('importSetup.later') },
                    ...targets.map((item) => ({ value: item.value.id, label: item.value.name })),
                ]}
                onSelect={(value: string) => {
                    targetPromptId = value;
                    if (!value) applyToRoom = false;
                }}
                disabled={busy}
            />
        {/if}
        {#if room && (kind === 'generation' || targetPromptId)}
            <DataChoice
                label={$tr('importSetup.destination')}
                value={String(applyToRoom)}
                options={[
                    { value: 'true', label: room.title },
                    { value: 'false', label: $tr('importSetup.saveOnly') },
                ]}
                onSelect={(value: string) => (applyToRoom = value === 'true')}
                disabled={busy}
            />
        {/if}
        <DataAction
            disabled={busy || !source || !routeId}
            onclick={async () => {
                if (
                    await controller.save({
                        kind,
                        sourceId,
                        routeId,
                        targetPromptId,
                        embeddingTaskId,
                        applyToRoom,
                    })
                )
                    onsaved();
            }}
            >{$tr(
                $store.saving
                    ? 'orchestration.imported_compatibility.action.busy'
                    : applyToRoom
                      ? 'importSetup.apply'
                      : 'settingsUi.save',
            )}</DataAction
        >
    {/if}
    {#if $store.error}<p role="alert">{$store.error}</p>{/if}
</section>
