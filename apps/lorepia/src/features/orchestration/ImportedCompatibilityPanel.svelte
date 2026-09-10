<script lang="ts">
    import { Database, SlidersHorizontal } from '@lucide/svelte';
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
    import { importedText } from '../../lib/import-display';
    import type { OrchestrationController, OrchestrationState } from './orchestration-controller';
    import {
        buildImportedGenerationPreset,
        buildImportedMemoryProfile,
        buildImportedSummaryTask,
        hintString,
        readImportedCompatibilityHints,
        recommendImportedRoute,
    } from './imported-compatibility';

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
    const hints = $derived(readImportedCompatibilityHints(sourcePreset));
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
            hints === null ? '' : recommendImportedRoute(appState.providers.workspace, modelHint);
        selectedTargetPromptId = targetPrompts[0]?.id ?? '';
        feedback = '';
    });

    onMount(() => {
        void loadCompatibilityCatalog();
    });

    async function loadSourcePreset(promptPresetId: string): Promise<void> {
        const loader = client?.getEditablePromptPreset;
        if (loader === undefined) {
            sourceError = $tr('orchestration.imported_compatibility.error.source_api');
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
            if (readImportedCompatibilityHints(document.value) === null) {
                sourceError = $tr(
                    'orchestration.imported_compatibility.error.source_not_compatible',
                );
            }
        } catch {
            if (epoch !== sourceLoadEpoch) return;
            selectedSourceDocument = null;
            sourceError = $tr('orchestration.imported_compatibility.error.source_load');
        } finally {
            if (epoch === sourceLoadEpoch) sourceLoading = false;
        }
    }

    async function loadCompatibilityCatalog(): Promise<void> {
        const list = client?.listPromptPresets;
        if (list === undefined) {
            sourceError = $tr('orchestration.imported_compatibility.error.source_api');
            return;
        }
        sourceLoading = true;
        sourceError = '';
        try {
            presetCatalog = await list.call(client);
            const contextual = contextualSourceDocument;
            const preferredId =
                contextual !== null && readImportedCompatibilityHints(contextual.value) !== null
                    ? contextual.value.id
                    : (presetCatalog[0]?.value.id ?? '');
            if (preferredId === '') {
                sourceError = $tr('orchestration.imported_compatibility.error.source_empty');
                return;
            }
            await loadSourcePreset(preferredId);
        } catch {
            sourceError = $tr('orchestration.imported_compatibility.error.source_load');
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
        const base = (sourcePreset?.id ?? 'imported').replaceAll(/[^A-Za-z0-9._-]+/g, '-');
        return `${base}-${suffix}`.slice(0, 256);
    }

    function displayPresetName(value: string): string {
        return importedText(value, '\uD638\uD658');
    }

    async function applyGenerationPreset(): Promise<void> {
        if (
            hints?.kind !== 'generation' ||
            sourcePreset === null ||
            appController === undefined ||
            client === undefined ||
            selectedRouteId === ''
        ) {
            feedback = $tr('orchestration.imported_compatibility.error.route_required');
            return;
        }
        busy = true;
        feedback = '';
        try {
            const saveSource = client.upsertPromptPreset;
            if (saveSource === undefined || sourceDocument === null) {
                feedback = $tr('orchestration.imported_compatibility.error.source_api');
                return;
            }
            const generationPresetId = generatedId('provider');
            const candidate = buildImportedGenerationPreset(
                appState.providers.workspace,
                hints,
                selectedRouteId,
                generationPresetId,
                $tr('orchestration.imported_compatibility.generation_name', {
                    name: displayPresetName(sourcePreset.name),
                }),
            );
            if (!(await appController.upsertProviderGenerationPreset(candidate))) {
                feedback = $tr('orchestration.imported_compatibility.error.parameters');
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
                    feedback = $tr('orchestration.imported_compatibility.error.room_binding');
                    return;
                }
                feedback = $tr('orchestration.imported_compatibility.success.generation');
            } else {
                feedback = $tr(
                    'orchestration.imported_compatibility.success.generation_standalone',
                );
            }
        } catch (error: unknown) {
            feedback =
                error instanceof Error
                    ? error.message
                    : $tr('orchestration.imported_compatibility.error.generation');
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
            feedback = $tr('orchestration.imported_compatibility.error.memory_selection');
            return;
        }
        const memoryProfileId = hintString(hints, 'memory_profile_id');
        const task = buildImportedSummaryTask(
            hints,
            selectedRouteId,
            generatedId('summary-provider'),
        );
        const memoryProfile =
            task === null
                ? null
                : buildImportedMemoryProfile(
                      sourcePreset,
                      hints,
                      task.id,
                      $tr('orchestration.imported_compatibility.memory_name', {
                          name: displayPresetName(sourcePreset.name),
                      }),
                  );
        if (memoryProfileId === null || task === null || memoryProfile === null) {
            feedback = $tr('orchestration.imported_compatibility.error.memory_metadata');
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
            feedback = $tr('orchestration.imported_compatibility.error.target_api');
            return;
        }
        busy = true;
        feedback = '';
        try {
            const generationCandidate = buildImportedGenerationPreset(
                appState.providers.workspace,
                hints,
                selectedRouteId,
                task.generation_preset_id,
                $tr('orchestration.imported_compatibility.summary_name', {
                    name: displayPresetName(sourcePreset.name),
                }),
            );
            if (!(await appController.upsertProviderGenerationPreset(generationCandidate))) {
                feedback = $tr('orchestration.imported_compatibility.error.summary_preset');
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
                    feedback = $tr('orchestration.imported_compatibility.error.room_binding');
                    return;
                }
                feedback = $tr('orchestration.imported_compatibility.success.memory');
            } else {
                feedback = $tr('orchestration.imported_compatibility.success.memory_standalone');
            }
        } catch (error: unknown) {
            feedback =
                error instanceof Error
                    ? error.message
                    : $tr('orchestration.imported_compatibility.error.memory');
        } finally {
            busy = false;
        }
    }
</script>

{#if presetCatalog.length > 0}
    <section
        class="compatibility-card compatibility-source"
        aria-label={$tr('orchestration.imported_compatibility.source.label')}
    >
        <ChoiceField
            id="imported-compatibility-source"
            label={$tr('orchestration.imported_compatibility.source.label')}
            value={selectedSourcePresetId}
            options={presetCatalog.map((preset) => ({
                value: preset.value.id,
                label: displayPresetName(preset.value.name),
            }))}
            disabled={busy || sourceLoading}
            required
            onSelect={(value: string) => void loadSourcePreset(value)}
        />
        {#if sourceLoading}
            <p class="compatibility-warning" role="status">
                {$tr('orchestration.imported_compatibility.source.loading')}
            </p>
        {:else if sourceError !== ''}
            <p class="compatibility-warning" role="status">{sourceError}</p>
        {/if}
    </section>
{:else if sourceLoading && hints === null}
    <section
        class="compatibility-card"
        aria-label={$tr('orchestration.imported_compatibility.label')}
    >
        <p class="compatibility-warning" role="status">
            {$tr('orchestration.imported_compatibility.source.loading')}
        </p>
    </section>
{:else if sourceError !== '' && hints === null}
    <section
        class="compatibility-card"
        aria-label={$tr('orchestration.imported_compatibility.label')}
    >
        <p class="compatibility-warning" role="status">{sourceError}</p>
    </section>
{/if}

{#if hints !== null}
    <section
        class="compatibility-card"
        aria-label={$tr('orchestration.imported_compatibility.label')}
    >
        <div class="compatibility-heading">
            <span class="compatibility-mark" aria-hidden="true">
                {#if hints.kind === 'memory'}
                    <Database size={20} />
                {:else}
                    <SlidersHorizontal size={20} />
                {/if}
            </span>
            <div class="compatibility-copy">
                <span class="eyebrow">{$tr('orchestration.imported_compatibility.eyebrow')}</span>
                <h3>
                    {$tr(
                        hints.kind === 'memory'
                            ? 'orchestration.imported_compatibility.title.memory'
                            : 'orchestration.imported_compatibility.title.generation',
                    )}
                </h3>
                <p>
                    {$tr(
                        hints.kind === 'memory'
                            ? 'orchestration.imported_compatibility.description.memory'
                            : 'orchestration.imported_compatibility.description.generation',
                    )}
                </p>
            </div>
        </div>

        <div class="compatibility-fields">
            {#if appState.providers.workspace.routes.length === 0}
                <p class="compatibility-warning" role="status">
                    {$tr('orchestration.imported_compatibility.provider_missing')}
                </p>
            {:else}
                <ChoiceField
                    id="imported-compatibility-route"
                    label={$tr(
                        hints.kind === 'memory'
                            ? 'orchestration.imported_compatibility.route.memory'
                            : 'orchestration.imported_compatibility.route.generation',
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
                        : $tr('orchestration.imported_compatibility.model_hint', {
                              model: modelHint,
                          })}
                    onSelect={(value: string) => (selectedRouteId = value)}
                />
            {/if}

            {#if hints.kind === 'memory'}
                <ChoiceField
                    id="imported-compatibility-target-prompt"
                    label={$tr('orchestration.imported_compatibility.target_prompt')}
                    value={selectedTargetPromptId}
                    options={[
                        { value: '', label: $tr('orchestration.imported_compatibility.select') },
                        ...targetPrompts.map((preset) => ({
                            value: preset.id,
                            label: displayPresetName(preset.name),
                        })),
                    ]}
                    disabled={busy}
                    required
                    onSelect={(value: string) => (selectedTargetPromptId = value)}
                />
            {/if}
        </div>

        <footer class="compatibility-footer">
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
                    ? $tr('orchestration.imported_compatibility.action.busy')
                    : $tr(
                          hints.kind === 'memory'
                              ? 'orchestration.imported_compatibility.action.memory'
                              : 'orchestration.imported_compatibility.action.generation',
                      )}
            </button>
            {#if feedback !== ''}
                <p class="compatibility-feedback" role="status">{feedback}</p>
            {/if}
        </footer>
    </section>
{/if}

<style>
    .compatibility-card {
        display: grid;
        gap: 18px;
        padding: clamp(16px, 4.5vw, 20px);
        border: 1px solid var(--line);
        border-radius: var(--radius-lg);
        background: var(--surface-raised);
        box-shadow: var(--shadow-1);
        color: var(--ink);
    }

    .compatibility-heading {
        display: grid;
        grid-template-columns: auto minmax(0, 1fr);
        align-items: start;
        gap: 12px;
    }

    .compatibility-mark {
        display: grid;
        width: 38px;
        height: 38px;
        border: 1px solid var(--line);
        border-radius: var(--radius-md);
        background: var(--surface-sunken);
        color: var(--ink);
        place-items: center;
    }

    .compatibility-copy {
        display: grid;
        gap: 5px;
    }

    .compatibility-source {
        margin-bottom: 12px;
        background: color-mix(in srgb, var(--surface-sunken) 58%, var(--surface-raised));
        box-shadow: none;
    }

    .eyebrow {
        color: var(--ink-subtle);
        font-size: 0.7rem;
        font-weight: 700;
        letter-spacing: 0.08em;
    }

    h3,
    p {
        margin: 0;
    }

    .compatibility-copy p {
        color: var(--ink-muted);
        line-height: 1.5;
    }

    .compatibility-fields,
    .compatibility-footer {
        display: grid;
        gap: 14px;
    }

    .compatibility-fields {
        padding-top: 16px;
        border-top: 1px solid var(--line);
    }

    .compatibility-action {
        min-height: 48px;
        border-radius: var(--radius-pill);
    }

    .compatibility-warning,
    .compatibility-feedback {
        padding: 10px 12px;
        border: 1px solid var(--status-warning-border);
        border-radius: var(--radius-md);
        margin: 0;
        background: var(--status-warning-bg);
        color: var(--status-warning-fg);
        line-height: 1.5;
    }

    .compatibility-feedback {
        border-color: var(--status-info-border);
        background: var(--status-info-bg);
        color: var(--status-info-fg);
    }

    @container view (min-width: 720px) {
        .compatibility-fields {
            grid-template-columns: repeat(2, minmax(0, 1fr));
        }
    }
</style>
