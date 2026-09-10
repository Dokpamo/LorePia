<script lang="ts">
    import SettingsRow from '../../../ui/workspace/SettingsRow.svelte';
    import { tr } from '../../../lib/i18n';
    import SettingsTextField from './DataText.svelte';
    import ChoiceField from './DataChoice.svelte';
    import type { CreatorMemoryProfileDocumentDto } from '../../../lib/ipc/contracts';
    import type {
        SettingsDocumentsController,
        SettingsDocumentsState,
    } from '../../../features/providers/settings/settings-documents';
    import type { SettingsServices } from '../../../features/providers/settings/settings-services';
    import AuxiliaryModels from './AuxiliaryModels.svelte';
    import {
        newMemory,
        summaryGuidance,
        summaryTemplate,
        isBuiltInPrompt,
    } from '../../../features/providers/settings/settings-defaults';
    let {
        documentsState,
        controller,
        services,
    }: {
        documentsState: SettingsDocumentsState;
        controller: SettingsDocumentsController;
        services: SettingsServices;
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
    import DataNumber from './DataNumber.svelte';
    let models = $state(false);
    let auxiliary = $state<{ isDirty: () => boolean }>();
    export function isDirty() {
        return dirty || (auxiliary?.isDirty() ?? false);
    }
    export function isBusy() {
        return busy;
    }
</script>

<section class="ui-settings-group">
    <SettingsRow
        label={$tr('settingsLive.auxiliaryModels')}
        disabled={auxiliary?.isDirty() ?? false}
        onclick={() => (models = !models)}
    />
    {#if models}<AuxiliaryModels
            bind:this={auxiliary}
            {documentsState}
            {controller}
            workspace={services.appState.providers.workspace}
        />{/if}
</section>
<div class="ui-settings-group">
    <button
        class="ui-submit ui-pressable"
        disabled={busy || dirty}
        onclick={() => {
            draft = newMemory();
            revision = null;
            selected = '';
            feedback = '';
        }}><span class="ui-press-visual">{$tr('settingsLive.addMemory')}</span></button
    >
</div>
{#each documentsState.memories as item (item.value.id)}<SettingsRow
        label={item.value.name}
        value={modelLabel(item.value.summary_task)}
        disabled={busy || dirty}
        onclick={() => edit(item.value.id)}
    />{/each}
{#if !documentsState.memories.length && !draft}<p class="ui-settings-group">
        {$tr('settingsLive.emptyMemory')}
    </p>{/if}
{#if draft}
    <section class="ui-settings-group">
        <SettingsTextField
            label={$tr('settingsLive.memoryName')}
            value={draft.name}
            maxlength={120}
            disabled={busy}
            onchange={(value: string) => {
                if (draft) draft.name = value;
            }}
        />
        <ChoiceField
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
        <p class="ui-settings-group">{$tr('settingsUi.embeddingHint')}</p>
        <DataNumber
            label={$tr('settingsUi.turns')}
            value={draft.turns_per_summary}
            min={1}
            max={1000}
            disabled={busy}
            onchange={(value: number) => {
                if (draft) draft.turns_per_summary = value;
            }}
        />
        <section class="ui-settings-group">
            <h2>{$tr('settingsLive.memoryBudgets')}</h2>
            <DataNumber
                label={$tr('settingsLive.recentBudget')}
                value={draft.recent_raw_budget.max_tokens}
                min={0}
                max={2000000}
                disabled={busy}
                onchange={(value: number) => {
                    if (draft) draft.recent_raw_budget.max_tokens = value;
                }}
            />
            <DataNumber
                label={$tr('settingsLive.episodicBudget')}
                value={draft.episodic_budget.max_tokens}
                min={0}
                max={2000000}
                disabled={busy}
                onchange={(value: number) => {
                    if (draft) draft.episodic_budget.max_tokens = value;
                }}
            />
            <DataNumber
                label={$tr('settingsLive.semanticBudget')}
                value={draft.semantic_budget.max_tokens}
                min={0}
                max={2000000}
                disabled={busy}
                onchange={(value: number) => {
                    if (draft) draft.semantic_budget.max_tokens = value;
                }}
            />
        </section>
        <DataNumber
            label={$tr('settingsUi.retrievalCount')}
            value={draft.retrieval_count}
            min={1}
            max={1000}
            disabled={busy}
            onchange={(value: number) => {
                if (draft) draft.retrieval_count = value;
            }}
        />
        <section class="ui-settings-group">
            <h2>{$tr('settingsUi.retrieval')}</h2>
            <DataNumber
                label={$tr('settingsLive.recency')}
                integer={false}
                value={draft.recency_weight}
                min={0}
                max={100}
                disabled={busy}
                onchange={(value: number) => {
                    if (draft) draft.recency_weight = value;
                }}
            />
            <DataNumber
                label={$tr('settingsLive.similarity')}
                integer={false}
                value={draft.similarity_weight}
                min={0}
                max={100}
                disabled={busy}
                onchange={(value: number) => {
                    if (draft) draft.similarity_weight = value;
                }}
            />
            <DataNumber
                label={$tr('settingsLive.importance')}
                integer={false}
                value={draft.importance_weight}
                min={0}
                max={100}
                disabled={busy}
                onchange={(value: number) => {
                    if (draft) draft.importance_weight = value;
                }}
            />
        </section>
        <SettingsTextField
            label={$tr('settingsUi.summaryPrompt')}
            value={sourceText ?? ''}
            maxlength={16000}
            disabled={busy || complexTemplate}
            placeholder={$tr(
                complexTemplate ? 'settingsLive.complexTemplate' : 'settingsLive.defaultSummary',
            )}
            onchange={(value: string) => {
                if (draft) draft.summary_template = summaryTemplate(value);
            }}
        />
        {#if complexTemplate}<p class="ui-settings-group">
                {$tr('settingsLive.complexTemplate')}
            </p>{/if}
        <ChoiceField
            label={$tr('settingsLive.preserveMemory')}
            value={String(draft.preserve_invalidated_records)}
            options={[
                { value: 'true', label: $tr('settingsUi.enabled') },
                { value: 'false', label: $tr('settingsUi.disabled') },
            ]}
            disabled={busy}
            onSelect={(value: string) => {
                if (draft) draft.preserve_invalidated_records = value === 'true';
            }}
        />
        <div class="ui-settings-group">
            <button
                class="ui-submit ui-pressable"
                disabled={busy || !draft.summary_task}
                type="button"
                onclick={() => void save()}
                ><span class="ui-press-visual">{$tr('settingsLive.saveMemory')}</span></button
            ><button
                class="ui-submit ui-pressable"
                type="button"
                disabled={busy}
                onclick={() => {
                    draft = null;
                    selected = '';
                    deleting = false;
                }}><span class="ui-press-visual">{$tr('settingsLive.cancel')}</span></button
            >
        </div>
    </section>
{/if}
{#if saved}
    <section class="ui-settings-group" aria-label={$tr('settingsLive.linkMemory')}>
        <h3>{$tr('settingsLive.linkMemory')}</h3>
        <ChoiceField
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
            class="ui-submit ui-pressable"
            disabled={busy || dirty || !targetPrompt}
            onclick={() => void link()}
            ><span class="ui-press-visual">{$tr('settingsLive.linkMemory')}</span></button
        >
        {#if feedback}<p role="status">{feedback}</p>{/if}
        <button
            class="ui-submit ui-pressable"
            disabled={busy}
            onclick={() => (deleting = !deleting)}
            ><span class="ui-press-visual">{$tr('settingsLive.deleteMemory')}</span></button
        >
        {#if deleting}<div role="alert">
                <p>{$tr('settingsLive.deleteConfirm')}</p>
                <button
                    class="ui-submit ui-pressable"
                    disabled={busy}
                    onclick={async () => {
                        if (await controller.deleteMemory(saved)) {
                            draft = null;
                            selected = '';
                            deleting = false;
                        }
                    }}
                    ><span class="ui-press-visual">{$tr('settingsLive.confirmDelete')}</span
                    ></button
                >
            </div>{/if}
    </section>
{/if}
{#if services.orchestrationState.phase === 'ready'}
    <section class="ui-settings-group">
        <h3>{$tr('settingsLive.currentRoom')}</h3>
        <ChoiceField
            label={$tr('settingsUi.memoryEnabled')}
            value={String(services.orchestrationState.workspace.room_config.memory_enabled)}
            options={[
                { value: 'true', label: $tr('settingsUi.enabled') },
                { value: 'false', label: $tr('settingsUi.disabled') },
            ]}
            disabled={busy || services.orchestrationState.saving}
            onSelect={(value: string) => void toggleRoom(value === 'true')}
        />
        <p class="ui-settings-group">{services.appState.selected_conversation?.title}</p>
        {#if services.orchestrationState.error}<p role="alert">
                {services.orchestrationState.error}
            </p>{/if}
    </section>
{/if}
