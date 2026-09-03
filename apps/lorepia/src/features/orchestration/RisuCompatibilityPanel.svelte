<script lang="ts">
    import { onMount } from 'svelte';

    import type { LorepiaAppController, LorepiaAppState } from '../../app/app-controller';
    import ChoiceField from '../../components/ChoiceField.svelte';
    import type {
        CreatorPromptPresetDocumentDto,
        LorepiaClient,
        OrchestrationDocumentClientApi,
        PromptPresetSummaryDto,
        RevisionedDto,
    } from '../../lib/ipc/contracts';
    import { tr } from '../../lib/i18n';
    import type { OrchestrationController, OrchestrationState } from './orchestration-controller';
    import {
        buildRisuGenerationPreset,
        buildRisuMemoryProfile,
        buildRisuSummaryTask,
        hintString,
        readRisuCompatibilityHints,
        recommendRisuRoute,
    } from './risu-compatibility';

    interface Props {
        client?: LorepiaClient & Partial<OrchestrationDocumentClientApi>;
        appController?: LorepiaAppController;
        appState: LorepiaAppState;
        orchestrationState: OrchestrationState;
        controller: OrchestrationController;
    }

    let { client, appController, appState, orchestrationState, controller }: Props = $props();
    let selectedRouteId = $state('');
    let selectedTargetPromptId = $state('');
    let selectedSourcePresetId = $state('');
    let selectedSourceDocument = $state<RevisionedDto<CreatorPromptPresetDocumentDto> | null>(null);
    let presetCatalog = $state<RevisionedDto<PromptPresetSummaryDto>[]>([]);
    let sourceLoading = $state(false);
    let sourceError = $state('');
    let sourceLoadEpoch = 0;
    let busy = $state(false);
    let feedback = $state('');
    let sourcePresetId = '';

    const contextualSourceDocument = $derived(orchestrationState.editable_prompt_preset);
    const sourceDocument = $derived(selectedSourceDocument ?? contextualSourceDocument);
    const sourcePreset = $derived(sourceDocument?.value ?? null);
    const hints = $derived(readRisuCompatibilityHints(sourcePreset));
    const availablePromptPresets = $derived(
        presetCatalog.length > 0
            ? presetCatalog.map((preset) => preset.value)
            : orchestrationState.workspace.prompt_presets,
    );
    const targetPrompts = $derived(
        availablePromptPresets.filter((preset) => preset.id !== sourcePreset?.id),
    );
    const modelHint = $derived(
        hints === null
            ? null
            : hintString(
                  hints,
                  hints.kind === 'memory' ? 'memory_summarization_model' : 'model_hint',
              ),
    );

    $effect(() => {
        const nextSourceId = sourcePreset?.id ?? '';
        if (sourcePresetId === nextSourceId) return;
        sourcePresetId = nextSourceId;
        selectedRouteId =
            hints === null ? '' : recommendRisuRoute(appState.providers.workspace, modelHint);
        selectedTargetPromptId = targetPrompts[0]?.id ?? '';
        feedback = '';
    });

    onMount(() => {
        void loadCompatibilityCatalog();
    });

    async function loadSourcePreset(promptPresetId: string): Promise<void> {
        const loader = client?.getEditablePromptPreset;
        if (loader === undefined) {
            sourceError = $tr('orchestration.risu.error.source_api');
            return;
        }
        const epoch = ++sourceLoadEpoch;
        selectedSourcePresetId = promptPresetId;
        sourceLoading = true;
        sourceError = '';
        try {
            const document = await loader.call(client, { prompt_preset_id: promptPresetId });
            if (epoch !== sourceLoadEpoch) return;
            selectedSourceDocument = document;
            if (readRisuCompatibilityHints(document.value) === null) {
                sourceError = $tr('orchestration.risu.error.source_not_compatible');
            }
        } catch {
            if (epoch !== sourceLoadEpoch) return;
            selectedSourceDocument = null;
            sourceError = $tr('orchestration.risu.error.source_load');
        } finally {
            if (epoch === sourceLoadEpoch) sourceLoading = false;
        }
    }

    async function loadCompatibilityCatalog(): Promise<void> {
        const list = client?.listPromptPresets;
        if (list === undefined) {
            sourceError = $tr('orchestration.risu.error.source_api');
            return;
        }
        sourceLoading = true;
        sourceError = '';
        try {
            presetCatalog = await list.call(client);
            const contextual = contextualSourceDocument;
            const preferredId =
                contextual !== null && readRisuCompatibilityHints(contextual.value) !== null
                    ? contextual.value.id
                    : (presetCatalog[0]?.value.id ?? '');
            if (preferredId === '') {
                sourceError = $tr('orchestration.risu.error.source_empty');
                return;
            }
            await loadSourcePreset(preferredId);
        } catch {
            sourceError = $tr('orchestration.risu.error.source_load');
        } finally {
            sourceLoading = false;
        }
    }

    function hasRoomContext(): boolean {
        const room = orchestrationState.workspace.room_config;
        return (
            orchestrationState.phase === 'ready' &&
            room.conversation_id !== '' &&
            room.branch_id !== ''
        );
    }

    function updateSelectedSourceRevision(
        value: CreatorPromptPresetDocumentDto,
        saved: RevisionedDto<PromptPresetSummaryDto>,
    ): void {
        if (sourceDocument === null) return;
        selectedSourceDocument = {
            ...sourceDocument,
            value,
            revision: saved.revision,
            updated_at: saved.updated_at,
        };
        presetCatalog = presetCatalog.map((preset) =>
            preset.value.id === saved.value.id ? saved : preset,
        );
    }

    function generatedId(suffix: string): string {
        const base = (sourcePreset?.id ?? 'risu').replaceAll(/[^A-Za-z0-9._-]+/g, '-');
        return `${base}-${suffix}`.slice(0, 256);
    }

    async function applyGenerationPreset(): Promise<void> {
        if (
            hints?.kind !== 'generation' ||
            sourcePreset === null ||
            appController === undefined ||
            client === undefined ||
            selectedRouteId === ''
        ) {
            feedback = $tr('orchestration.risu.error.route_required');
            return;
        }
        busy = true;
        feedback = '';
        try {
            const saveSource = client.upsertPromptPreset;
            if (saveSource === undefined || sourceDocument === null) {
                feedback = $tr('orchestration.risu.error.source_api');
                return;
            }
            const generationPresetId = generatedId('provider');
            const candidate = buildRisuGenerationPreset(
                appState.providers.workspace,
                hints,
                selectedRouteId,
                generationPresetId,
                $tr('orchestration.risu.generation_name', { name: sourcePreset.name }),
            );
            if (!(await appController.upsertProviderGenerationPreset(candidate))) {
                feedback = $tr('orchestration.risu.error.parameters');
                return;
            }
            const updatedSource = {
                ...sourcePreset,
                default_generation_preset_id: generationPresetId,
            };
            const saved = await saveSource.call(client, {
                value: updatedSource,
                expected_revision: sourceDocument.revision,
            });
            updateSelectedSourceRevision(updatedSource, saved);
            if (hasRoomContext()) {
                controller.stageRoomConfig({
                    prompt_preset_id: sourcePreset.id,
                    generation_preset_id: generationPresetId,
                });
                if (!(await controller.saveRoomConfig())) {
                    feedback = $tr('orchestration.risu.error.room_binding');
                    return;
                }
                feedback = $tr('orchestration.risu.success.generation');
            } else {
                feedback = $tr('orchestration.risu.success.generation_standalone');
            }
        } catch (error: unknown) {
            feedback =
                error instanceof Error ? error.message : $tr('orchestration.risu.error.generation');
        } finally {
            busy = false;
        }
    }

    async function applyMemoryPreset(): Promise<void> {
        if (
            hints?.kind !== 'memory' ||
            sourcePreset === null ||
            appController === undefined ||
            client === undefined ||
            selectedRouteId === '' ||
            selectedTargetPromptId === ''
        ) {
            feedback = $tr('orchestration.risu.error.memory_selection');
            return;
        }
        const memoryProfileId = hintString(hints, 'memory_profile_id');
        const task = buildRisuSummaryTask(hints, selectedRouteId, generatedId('summary-provider'));
        const memoryProfile =
            task === null
                ? null
                : buildRisuMemoryProfile(
                      sourcePreset,
                      hints,
                      task.id,
                      $tr('orchestration.risu.memory_name', { name: sourcePreset.name }),
                  );
        if (memoryProfileId === null || task === null || memoryProfile === null) {
            feedback = $tr('orchestration.risu.error.memory_metadata');
            return;
        }
        const listMemory = client.listMemoryProfiles;
        const saveMemory = client.upsertMemoryProfile;
        const listTasks = client.listTaskProfiles;
        const saveTask = client.upsertTaskProfile;
        const getTarget = client.getEditablePromptPreset;
        const saveTarget = client.upsertPromptPreset;
        if (
            listMemory === undefined ||
            saveMemory === undefined ||
            listTasks === undefined ||
            saveTask === undefined ||
            getTarget === undefined ||
            saveTarget === undefined
        ) {
            feedback = $tr('orchestration.risu.error.target_api');
            return;
        }
        busy = true;
        feedback = '';
        try {
            const generationCandidate = buildRisuGenerationPreset(
                appState.providers.workspace,
                hints,
                selectedRouteId,
                task.generation_preset_id,
                $tr('orchestration.risu.summary_name', { name: sourcePreset.name }),
            );
            if (!(await appController.upsertProviderGenerationPreset(generationCandidate))) {
                feedback = $tr('orchestration.risu.error.summary_preset');
                return;
            }
            const existingTask = (await listTasks.call(client)).find(
                (candidate) => candidate.value.id === task.id,
            );
            await saveTask.call(client, {
                value: task,
                expected_revision: existingTask?.revision ?? null,
            });
            const existingMemory = (await listMemory.call(client)).find(
                (candidate) => candidate.value.id === memoryProfile.id,
            );
            await saveMemory.call(client, {
                value: memoryProfile,
                expected_revision: existingMemory?.revision ?? null,
            });
            const target = await getTarget.call(client, {
                prompt_preset_id: selectedTargetPromptId,
            });
            await saveTarget.call(client, {
                value: { ...target.value, memory_profile_id: memoryProfileId },
                expected_revision: target.revision,
            });
            const targetSummary = targetPrompts.find(
                (preset) => preset.id === selectedTargetPromptId,
            );
            if (hasRoomContext()) {
                controller.stageRoomConfig({
                    prompt_preset_id: selectedTargetPromptId,
                    generation_preset_id:
                        targetSummary?.default_generation_preset_id ??
                        orchestrationState.workspace.room_config.generation_preset_id,
                    memory_enabled: true,
                });
                if (!(await controller.saveRoomConfig())) {
                    feedback = $tr('orchestration.risu.error.room_binding');
                    return;
                }
                feedback = $tr('orchestration.risu.success.memory');
            } else {
                feedback = $tr('orchestration.risu.success.memory_standalone');
            }
        } catch (error: unknown) {
            feedback =
                error instanceof Error ? error.message : $tr('orchestration.risu.error.memory');
        } finally {
            busy = false;
        }
    }
</script>

{#if presetCatalog.length > 0}
    <section
        class="compatibility-card compatibility-source"
        aria-label={$tr('orchestration.risu.source.label')}
    >
        <ChoiceField
            id="risu-compatibility-source"
            label={$tr('orchestration.risu.source.label')}
            value={selectedSourcePresetId}
            options={presetCatalog.map((preset) => ({
                value: preset.value.id,
                label: preset.value.name,
            }))}
            disabled={busy || sourceLoading}
            required
            onSelect={(value: string) => void loadSourcePreset(value)}
        />
        {#if sourceLoading}
            <p class="compatibility-warning" role="status">
                {$tr('orchestration.risu.source.loading')}
            </p>
        {:else if sourceError !== ''}
            <p class="compatibility-warning" role="status">{sourceError}</p>
        {/if}
    </section>
{:else if sourceLoading && hints === null}
    <section class="compatibility-card" aria-label={$tr('orchestration.risu.label')}>
        <p class="compatibility-warning" role="status">
            {$tr('orchestration.risu.source.loading')}
        </p>
    </section>
{:else if sourceError !== '' && hints === null}
    <section class="compatibility-card" aria-label={$tr('orchestration.risu.label')}>
        <p class="compatibility-warning" role="status">{sourceError}</p>
    </section>
{/if}

{#if hints !== null}
    <section class="compatibility-card" aria-label={$tr('orchestration.risu.label')}>
        <div class="compatibility-copy">
            <span class="eyebrow">{$tr('orchestration.risu.eyebrow')}</span>
            <h3>
                {$tr(
                    hints.kind === 'memory'
                        ? 'orchestration.risu.title.memory'
                        : 'orchestration.risu.title.generation',
                )}
            </h3>
            <p>
                {$tr(
                    hints.kind === 'memory'
                        ? 'orchestration.risu.description.memory'
                        : 'orchestration.risu.description.generation',
                )}
            </p>
        </div>

        {#if appState.providers.workspace.routes.length === 0}
            <p class="compatibility-warning" role="status">
                {$tr('orchestration.risu.provider_missing')}
            </p>
        {:else}
            <ChoiceField
                id="risu-compatibility-route"
                label={$tr(
                    hints.kind === 'memory'
                        ? 'orchestration.risu.route.memory'
                        : 'orchestration.risu.route.generation',
                )}
                value={selectedRouteId}
                options={appState.providers.workspace.routes.map((route) => ({
                    value: route.id,
                    label: `${route.display_name ?? route.model_id} · ${route.api_family}`,
                }))}
                disabled={busy}
                required
                hint={modelHint === null
                    ? undefined
                    : $tr('orchestration.risu.model_hint', { model: modelHint })}
                onSelect={(value: string) => (selectedRouteId = value)}
            />
        {/if}

        {#if hints.kind === 'memory'}
            <ChoiceField
                id="risu-compatibility-target-prompt"
                label={$tr('orchestration.risu.target_prompt')}
                value={selectedTargetPromptId}
                options={[
                    { value: '', label: $tr('orchestration.risu.select') },
                    ...targetPrompts.map((preset) => ({
                        value: preset.id,
                        label: preset.name,
                    })),
                ]}
                disabled={busy}
                required
                onSelect={(value: string) => (selectedTargetPromptId = value)}
            />
        {/if}

        <button
            class="primary compatibility-action"
            type="button"
            disabled={busy ||
                selectedRouteId === '' ||
                (hints.kind === 'memory' && selectedTargetPromptId === '')}
            onclick={() =>
                void (hints.kind === 'memory' ? applyMemoryPreset() : applyGenerationPreset())}
        >
            {busy
                ? $tr('orchestration.risu.action.busy')
                : $tr(
                      hints.kind === 'memory'
                          ? 'orchestration.risu.action.memory'
                          : 'orchestration.risu.action.generation',
                  )}
        </button>
        {#if feedback !== ''}
            <p class="compatibility-feedback" role="status">{feedback}</p>
        {/if}
    </section>
{/if}

<style>
    .compatibility-card {
        display: grid;
        gap: 14px;
        padding: 18px;
        border: 1px solid var(--line);
        border-radius: var(--radius-lg);
        background: var(--surface-raised);
        color: var(--ink);
    }

    .compatibility-copy {
        display: grid;
        gap: 5px;
    }

    .compatibility-source {
        margin-bottom: 12px;
    }

    .eyebrow {
        color: var(--accent);
        font-size: 0.7rem;
        font-weight: 800;
        letter-spacing: 0.12em;
    }

    h3,
    p {
        margin: 0;
    }

    .compatibility-copy p {
        color: var(--ink-muted);
        line-height: 1.5;
    }

    .compatibility-action {
        min-height: 44px;
    }

    .compatibility-warning,
    .compatibility-feedback {
        color: var(--ink-muted);
        line-height: 1.5;
    }
</style>
