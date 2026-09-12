<script lang="ts">
    import { tr } from '../../../lib/i18n';
    import SettingsTextField from './DataText.svelte';
    import PromptOptions from './PromptOptions.svelte';
    import ChoiceField from './DataChoice.svelte';
    import type { CreatorPromptPresetDocumentDto } from '../../../lib/ipc/contracts';
    import type {
        SettingsDocumentsController,
        SettingsDocumentsState,
    } from '../../../features/providers/settings/settings-documents';
    import type { SettingsServices } from '../../../features/providers/settings/settings-services';
    import {
        newPrompt,
        promptBlock,
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
    let draft = $state<CreatorPromptPresetDocumentDto | null>(null);
    let builtin = $state<string | null>(null);
    let revision = $state<number | null>(null);
    let loading = $state(false);
    let applying = $state(false);
    let deleting = $state(false);
    let savedJson = $state('');
    const dirty = $derived(draft !== null && JSON.stringify(draft) !== savedJson);
    const selected = $derived(
        documentsState.prompts.find((item) => item.value.id === (draft?.id ?? builtin)),
    );
    const busy = $derived(
        documentsState.busy || loading || applying || services.orchestrationState.saving,
    );
    async function edit(promptId: string) {
        if (isBuiltInPrompt(promptId)) {
            controller.cancelSelection();
            draft = null;
            builtin = promptId;
            deleting = false;
            return;
        }
        builtin = null;
        loading = true;
        deleting = false;
        const value = await controller.openPrompt(promptId);
        if (value) {
            draft = structuredClone(value.value);
            revision = value.revision;
            savedJson = JSON.stringify(draft);
        }
        loading = false;
    }
    function create() {
        builtin = null;
        controller.cancelSelection();
        draft = newPrompt();
        revision = null;
        savedJson = '';
        deleting = false;
    }
    async function save() {
        if (!draft) return;
        const saved = await controller.savePrompt($state.snapshot(draft), revision);
        if (saved) {
            revision = saved.revision;
            savedJson = JSON.stringify(draft);
            await refreshRuntime();
        }
    }
    async function refreshRuntime() {
        const conversationId = services.appState.selected_conversation?.id;
        const branchId = services.appState.conversation_state?.active_branch_id;
        if (conversationId && branchId)
            await services.orchestrationController.loadContext(conversationId, branchId);
    }
    async function apply() {
        if (!selected || dirty || services.orchestrationState.phase !== 'ready') return;
        applying = true;
        try {
            services.orchestrationController.stageRoomConfig({
                prompt_preset_id: selected.value.id,
                generation_preset_id: selected.value.default_generation_preset_id,
            });
            await services.orchestrationController.saveRoomConfig();
        } finally {
            applying = false;
        }
    }
    function move(index: number, offset: number) {
        if (!draft) return;
        const blocks = [...draft.blocks];
        const [item] = blocks.splice(index, 1);
        if (!item) return;
        blocks.splice(index + offset, 0, item);
        draft.blocks = blocks;
    }
    import DataAction from './DataAction.svelte';
    export function isDirty() {
        return dirty || services.orchestrationState.dirty_room_config;
    }
    export async function discardChanges() {
        if (services.orchestrationState.dirty_room_config) await refreshRuntime();
    }
    export function isBusy() {
        return busy;
    }
</script>

<section class="ui-settings-group">
    {#if !draft && !builtin}
        <DataAction disabled={busy} onclick={create}>{$tr('settingsUi.addPrompt')}</DataAction>
        {#each documentsState.prompts as item (item.value.id)}<DataAction
                disabled={busy}
                onclick={() => void edit(item.value.id)}>{item.value.name}</DataAction
            >{/each}
    {:else}
        <DataAction
            disabled={busy || dirty || services.orchestrationState.dirty_room_config}
            onclick={() => {
                draft = null;
                builtin = null;
            }}>{$tr('uiPreview.back')}</DataAction
        >
    {/if}
    {#if loading}<p role="status">{$tr('settingsLive.loading')}</p>{/if}
    {#if services.orchestrationState.workspace.room_config.prompt_preset_id === selected?.value.id}
        <PromptOptions {services} />
    {/if}
    {#if draft}
        <SettingsTextField
            label={$tr('settingsUi.promptName')}
            value={draft.name}
            maxlength={120}
            disabled={busy}
            onchange={(value: string) => {
                if (draft) draft.name = value;
            }}
        />
        <ChoiceField
            label={$tr('settingsUi.memory')}
            value={draft.memory_profile_id ?? ''}
            options={[
                { value: '', label: $tr('settingsUi.none') },
                ...documentsState.memories.map((item) => ({
                    value: item.value.id,
                    label: item.value.name,
                })),
            ]}
            disabled={busy}
            onSelect={(value: string) => {
                if (draft) draft.memory_profile_id = value === '' ? null : value;
            }}
        />
        {#each draft.blocks as block, index (block.id)}
            {@const firstPart = block.template?.parts[0]}
            <h2>{block.name}</h2>
            <ChoiceField
                label={$tr('settingsUi.promptToggle', { name: block.name })}
                value={String(block.enabled)}
                options={[
                    { value: 'true', label: $tr('settingsUi.enabled') },
                    { value: 'false', label: $tr('settingsUi.disabled') },
                ]}
                disabled={busy || block.kind === 'latest_user_turn'}
                onSelect={(value: string) => (block.enabled = value === 'true')}
            />
            {#if block.template?.parts.length === 1 && firstPart?.kind === 'text'}<SettingsTextField
                    label={$tr('settingsUi.promptContent')}
                    value={firstPart.value}
                    disabled={busy}
                    onchange={(value: string) => (firstPart.value = value)}
                />{:else}<p>
                    {$tr(
                        block.template
                            ? 'settingsLive.complexTemplate'
                            : 'settingsLive.dynamicSource',
                    )}
                </p>{/if}
            {#if block.kind === 'static_instruction' || block.kind === 'post_history_instruction'}<ChoiceField
                    label={$tr('settingsUi.promptPosition')}
                    value={block.placement_zone}
                    options={[
                        { value: 'preset_instruction', label: $tr('settingsUi.beforeChat') },
                        { value: 'post_history', label: $tr('settingsUi.afterChat') },
                    ]}
                    disabled={busy}
                    onSelect={(value: string) => {
                        if (value === 'preset_instruction' || value === 'post_history')
                            block.placement_zone = value;
                    }}
                />{/if}
            <DataAction disabled={busy || index === 0} onclick={() => move(index, -1)}
                >{$tr('settingsUi.moveUp', { name: block.name })}</DataAction
            >
            <DataAction
                disabled={busy || index === draft.blocks.length - 1}
                onclick={() => move(index, 1)}
                >{$tr('settingsUi.moveDown', { name: block.name })}</DataAction
            >
        {/each}
        <DataAction
            disabled={busy}
            onclick={() =>
                draft?.blocks.push(
                    promptBlock(
                        'static_instruction',
                        $tr('settingsUi.promptName'),
                        { kind: 'template' },
                        'preset_instruction',
                    ),
                )}>{$tr('settingsLive.addBlock')}</DataAction
        >
        <DataAction disabled={busy || !dirty || !draft.name.trim()} onclick={() => void save()}
            >{$tr('settingsUi.save')}</DataAction
        >
    {/if}
    {#if selected}
        <p>{services.appState.selected_conversation?.title ?? $tr('settingsLive.chooseRoom')}</p>
        {#if builtin}<p>{$tr('settingsLive.builtInHint')}</p>{/if}
        <DataAction
            disabled={busy ||
                dirty ||
                services.orchestrationState.phase !== 'ready' ||
                services.orchestrationState.saving}
            onclick={() => void apply()}>{$tr('settingsLive.applyPrompt')}</DataAction
        >
        {#if services.orchestrationState.error}<p role="alert">
                {services.orchestrationState.error}
            </p>{/if}
        {#if !builtin}<DataAction disabled={busy} onclick={() => (deleting = !deleting)}
                >{$tr('settingsUi.deletePrompt')}</DataAction
            >{/if}
        {#if deleting}<p>{$tr('settingsLive.deleteConfirm')}</p>
            <DataAction
                disabled={busy}
                onclick={async () => {
                    if (await controller.deletePrompt(selected)) {
                        draft = null;
                        deleting = false;
                        await refreshRuntime();
                    }
                }}>{$tr('settingsLive.confirmDelete')}</DataAction
            >{/if}
    {/if}
</section>
