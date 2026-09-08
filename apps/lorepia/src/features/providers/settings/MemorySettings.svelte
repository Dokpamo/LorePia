<script lang="ts">
    import { BookOpen, ScanSearch, Waypoints } from '@lucide/svelte';
    import { tr } from '../../../lib/i18n';
    import SettingsTextField from './SettingsTextField.svelte';
    import ChoiceField from './SettingsChoiceField.svelte';
    import type { CreatorMemoryProfileDocumentDto } from '../../../lib/ipc/contracts';
    import type { SettingsDocumentsController, SettingsDocumentsState } from './settings-documents';
    import type { SettingsServices } from './settings-services';
    import AuxiliaryModels from './AuxiliaryModels.svelte';
    import {
        newMemory,
        summaryGuidance,
        summaryTemplate,
        isBuiltInPrompt,
    } from './settings-defaults';
    let {
        documentsState,
        controller,
        services,
        onImport,
    }: {
        documentsState: SettingsDocumentsState;
        controller: SettingsDocumentsController;
        services: SettingsServices;
        onImport: () => void;
    } = $props();
    let draft = $state<CreatorMemoryProfileDocumentDto | null>(null);
    let revision = $state<number | null>(null);
    let selected = $state('');
    let targetPrompt = $state('');
    let deleting = $state(false);
    let applying = $state(false);
    let feedback = $state('');
    const summaries = $derived(
        documentsState.tasks.filter((item) => item.value.kind === 'memory_summary'),
    );
    const embeddings = $derived(
        documentsState.tasks.filter((item) => item.value.kind === 'memory_embedding'),
    );
    const saved = $derived(documentsState.memories.find((item) => item.value.id === selected));
    const busy = $derived(documentsState.busy || applying);
    const sourceText = $derived(summaryGuidance(draft?.summary_template));
    const complexTemplate = $derived(sourceText === null);
    const dirty = $derived(!!draft && JSON.stringify(draft) !== JSON.stringify(saved?.value));
    function modelLabel(id: string) {
        const task = documentsState.tasks.find((item) => item.value.id === id)?.value;
        const route = services.appState.providers.workspace.routes.find(
            (item) => item.id === task?.route_id,
        );
        return `${route?.display_name ?? route?.model_id ?? id}${task?.embedding_dimensions ? ` · ${String(task.embedding_dimensions)}` : ''}`;
    }
    function edit(id: string) {
        const document = documentsState.memories.find((item) => item.value.id === id);
        if (!document) return;
        selected = id;
        draft = structuredClone($state.snapshot(document.value));
        revision = document.revision;
        deleting = false;
        feedback = '';
    }
    async function save() {
        if (!draft?.name.trim() || !summaries.some((item) => item.value.id === draft?.summary_task))
            return;
        const result = await controller.saveMemory($state.snapshot(draft), revision);
        if (result) {
            selected = result.value.id;
            revision = result.revision;
            draft = structuredClone(result.value);
        }
    }
    async function link() {
        if (!saved || !targetPrompt || busy) return;
        applying = true;
        feedback = '';
        try {
            const prompt = await controller.openPrompt(targetPrompt);
            if (!prompt) return;
            const result = await controller.savePrompt(
                { ...prompt.value, memory_profile_id: saved.value.id },
                prompt.revision,
            );
            if (result) {
                feedback = $tr('settingsLive.memoryLinked');
                await refreshRuntime();
            }
        } finally {
            applying = false;
        }
    }
    async function refreshRuntime() {
        const conversationId = services.appState.selected_conversation?.id;
        const branchId = services.appState.conversation_state?.active_branch_id;
        if (conversationId && branchId)
            await services.orchestrationController.loadContext(conversationId, branchId);
    }
    async function toggleRoom(enabled: boolean) {
        if (busy || services.orchestrationState.phase !== 'ready') return;
        applying = true;
        try {
            services.orchestrationController.stageRoomConfig({ memory_enabled: enabled });
            await services.orchestrationController.saveRoomConfig();
        } finally {
            applying = false;
        }
    }
</script>

<ol class="settings-memory-flow" aria-label={$tr('settingsUi.memory')}>
    <li>
        <BookOpen aria-hidden="true" /><span
            >{$tr('settingsUi.summaryStep')}<small>{$tr('settingsUi.summaryStepHint')}</small></span
        >
    </li>
    <li>
        <Waypoints aria-hidden="true" /><span
            >{$tr('settingsUi.embeddingStep')}<small>{$tr('settingsUi.embeddingStepHint')}</small
            ></span
        >
    </li>
    <li>
        <ScanSearch aria-hidden="true" /><span
            >{$tr('settingsUi.retrievalStep')}<small>{$tr('settingsUi.retrievalStepHint')}</small
            ></span
        >
    </li>
</ol>
<details class="settings-purpose-card settings-disclosure">
    <summary>{$tr('settingsLive.auxiliaryModels')}</summary><AuxiliaryModels
        {documentsState}
        {controller}
        workspace={services.appState.providers.workspace}
    />
</details>
<div class="settings-inline-actions">
    <button
        disabled={busy}
        onclick={() => {
            draft = newMemory();
            revision = null;
            selected = '';
            feedback = '';
        }}>{$tr('settingsLive.addMemory')}</button
    ><button disabled={busy} onclick={onImport}>{$tr('settingsLive.importMemory')}</button>
</div>
{#each documentsState.memories as item (item.value.id)}
    <button
        class="settings-purpose-row"
        aria-pressed={selected === item.value.id}
        disabled={busy}
        onclick={() => edit(item.value.id)}
        ><span
            ><strong>{item.value.name}</strong><small
                >{modelLabel(item.value.summary_task)} · {$tr(
                    item.value.embedding_task
                        ? 'settingsUi.vectorMemory'
                        : 'settingsUi.summaryOnly',
                )}</small
            ></span
        ><span>{$tr('settingsUi.edit')}</span></button
    >
{/each}
{#if !documentsState.memories.length && !draft}<p class="settings-empty">
        {$tr('settingsLive.emptyMemory')}
    </p>{/if}
{#if draft}
    <form
        class="settings-form settings-purpose-card"
        onsubmit={(event) => {
            event.preventDefault();
            void save();
        }}
    >
        <SettingsTextField
            label={$tr('settingsLive.memoryName')}
            value={draft.name}
            maxlength={120}
            required
            disabled={busy}
            onchange={(value: string) => {
                if (draft) draft.name = value;
            }}
        />
        <ChoiceField
            id="memory-summary-task"
            label={$tr('settingsUi.summaryModelGroup')}
            value={draft.summary_task}
            options={[
                { value: '', label: $tr('settingsLive.chooseModel') },
                ...summaries.map((item) => ({
                    value: item.value.id,
                    label: modelLabel(item.value.id),
                })),
            ]}
            disabled={busy}
            onSelect={(value: string) => {
                if (draft) draft.summary_task = value;
            }}
        />
        <ChoiceField
            id="memory-embedding-task"
            label={$tr('settingsUi.embeddingModelGroup')}
            value={draft.embedding_task ?? ''}
            options={[
                { value: '', label: $tr('settingsUi.summaryOnly') },
                ...embeddings.map((item) => ({
                    value: item.value.id,
                    label: modelLabel(item.value.id),
                })),
            ]}
            disabled={busy}
            onSelect={(value: string) => {
                if (draft) draft.embedding_task = value === '' ? null : value;
            }}
        />
        <p class="settings-note">{$tr('settingsUi.embeddingHint')}</p>
        <label
            >{$tr('settingsUi.turns')}<input
                type="number"
                bind:value={draft.turns_per_summary}
                min="1"
                max="1000"
                required
                disabled={busy}
            /></label
        >
        <fieldset class="settings-field-grid">
            <legend>{$tr('settingsLive.memoryBudgets')}</legend>
            <label
                >{$tr('settingsLive.recentBudget')}<input
                    type="number"
                    bind:value={draft.recent_raw_budget.max_tokens}
                    min="0"
                    max="2000000"
                    required
                    disabled={busy}
                /></label
            >
            <label
                >{$tr('settingsLive.episodicBudget')}<input
                    type="number"
                    bind:value={draft.episodic_budget.max_tokens}
                    min="0"
                    max="2000000"
                    required
                    disabled={busy}
                /></label
            >
            <label
                >{$tr('settingsLive.semanticBudget')}<input
                    type="number"
                    bind:value={draft.semantic_budget.max_tokens}
                    min="0"
                    max="2000000"
                    required
                    disabled={busy}
                /></label
            >
        </fieldset>
        <label
            >{$tr('settingsUi.retrievalCount')}<input
                type="number"
                bind:value={draft.retrieval_count}
                min="1"
                max="1000"
                required
                disabled={busy}
            /></label
        >
        <fieldset class="settings-field-grid">
            <legend>{$tr('settingsUi.retrieval')}</legend>
            <label
                >{$tr('settingsLive.recency')}<input
                    type="number"
                    bind:value={draft.recency_weight}
                    min="0"
                    max="100"
                    step="0.1"
                    required
                    disabled={busy}
                /></label
            >
            <label
                >{$tr('settingsLive.similarity')}<input
                    type="number"
                    bind:value={draft.similarity_weight}
                    min="0"
                    max="100"
                    step="0.1"
                    required
                    disabled={busy}
                /></label
            >
            <label
                >{$tr('settingsLive.importance')}<input
                    type="number"
                    bind:value={draft.importance_weight}
                    min="0"
                    max="100"
                    step="0.1"
                    required
                    disabled={busy}
                /></label
            >
        </fieldset>
        <SettingsTextField
            label={$tr('settingsUi.summaryPrompt')}
            value={sourceText ?? ''}
            maxlength={16000}
            rows={5}
            disabled={busy || complexTemplate}
            placeholder={$tr(
                complexTemplate ? 'settingsLive.complexTemplate' : 'settingsLive.defaultSummary',
            )}
            onchange={(value: string) => {
                if (draft) draft.summary_template = summaryTemplate(value);
            }}
        />
        {#if complexTemplate}<p class="settings-note">{$tr('settingsLive.complexTemplate')}</p>{/if}
        <label class="settings-checkbox"
            ><input
                type="checkbox"
                bind:checked={draft.preserve_invalidated_records}
                disabled={busy}
            />{$tr('settingsLive.preserveMemory')}</label
        >
        <div class="settings-inline-actions">
            <button class="primary" disabled={busy || !draft.summary_task} type="submit"
                >{$tr('settingsLive.saveMemory')}</button
            ><button
                type="button"
                disabled={busy}
                onclick={() => {
                    draft = null;
                    selected = '';
                    deleting = false;
                }}>{$tr('settingsLive.cancel')}</button
            >
        </div>
    </form>
{/if}
{#if saved}
    <section
        class="settings-purpose-card settings-form"
        aria-label={$tr('settingsLive.linkMemory')}
    >
        <h3>{$tr('settingsLive.linkMemory')}</h3>
        <ChoiceField
            id="memory-target-prompt"
            label={$tr('settingsUi.prompts')}
            value={targetPrompt}
            options={[
                { value: '', label: $tr('settingsLive.choosePrompt') },
                ...documentsState.prompts
                    .filter((item) => !isBuiltInPrompt(item.value.id))
                    .map((item) => ({
                        value: item.value.id,
                        label: item.value.name,
                    })),
            ]}
            disabled={busy}
            onSelect={(value: string) => (targetPrompt = value)}
        />
        <button
            class="primary"
            disabled={busy || dirty || !targetPrompt}
            onclick={() => void link()}>{$tr('settingsLive.linkMemory')}</button
        >
        {#if feedback}<p role="status">{feedback}</p>{/if}
        <button disabled={busy} onclick={() => (deleting = !deleting)}
            >{$tr('settingsLive.deleteMemory')}</button
        >
        {#if deleting}<div role="alert">
                <p>{$tr('settingsLive.deleteConfirm')}</p>
                <button
                    disabled={busy}
                    onclick={async () => {
                        if (await controller.deleteMemory(saved)) {
                            draft = null;
                            selected = '';
                            deleting = false;
                        }
                    }}>{$tr('settingsLive.confirmDelete')}</button
                >
            </div>{/if}
    </section>
{/if}
{#if services.orchestrationState.phase === 'ready'}
    <section class="settings-purpose-card settings-form">
        <h3>{$tr('settingsLive.currentRoom')}</h3>
        <label class="settings-checkbox"
            ><input
                type="checkbox"
                checked={services.orchestrationState.workspace.room_config.memory_enabled}
                disabled={busy || services.orchestrationState.saving}
                onchange={(event) => void toggleRoom(event.currentTarget.checked)}
            />{$tr('settingsUi.memoryEnabled')}</label
        >
        <p class="settings-note">{services.appState.selected_conversation?.title}</p>
        {#if services.orchestrationState.error}<p role="alert">
                {services.orchestrationState.error}
            </p>{/if}
    </section>
{/if}
