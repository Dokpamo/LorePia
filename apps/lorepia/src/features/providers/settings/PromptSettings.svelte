<script lang="ts">
    import { ArrowDown, ArrowUp, Plus } from '@lucide/svelte';
    import { tr } from '../../../lib/i18n';
    import SettingsTextField from './SettingsTextField.svelte';
    import ChoiceField from './SettingsChoiceField.svelte';
    import type { CreatorPromptPresetDocumentDto } from '../../../lib/ipc/contracts';
    import type { SettingsDocumentsController, SettingsDocumentsState } from './settings-documents';
    import type { SettingsServices } from './settings-services';
    import { newPrompt, promptBlock, isBuiltInPrompt } from './settings-defaults';
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
    const busy = $derived(documentsState.busy || loading || applying);
    const id = $props.id();
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
</script>

<p class="settings-lead">{$tr('settingsLive.promptsIntro')}</p>
<div class="settings-inline-actions">
    <button onclick={create} disabled={busy}
        ><Plus aria-hidden="true" />{$tr('settingsUi.addPrompt')}</button
    ><button onclick={onImport} disabled={busy}>{$tr('settingsUi.importPrompt')}</button>
</div>
<p class="settings-note">{$tr('settingsUi.importHint')}</p>
{#each documentsState.prompts as item (item.value.id)}
    <button
        class="settings-purpose-row"
        disabled={busy}
        aria-pressed={(draft?.id ?? builtin) === item.value.id}
        onclick={() => void edit(item.value.id)}
        ><span
            ><strong>{item.value.name}</strong><small
                >{$tr('settingsLive.blockCount', { count: item.value.block_count })}</small
            ></span
        ><span
            >{$tr(
                isBuiltInPrompt(item.value.id) ? 'settingsLive.builtIn' : 'settingsUi.edit',
            )}</span
        ></button
    >
{/each}
{#if !documentsState.prompts.length && !draft}<p class="settings-empty">
        {$tr('settingsUi.emptyPrompts')}
    </p>{/if}
{#if loading}<p role="status">{$tr('settingsLive.loading')}</p>{/if}
{#if draft}
    <form
        class="settings-form settings-purpose-card"
        onsubmit={(event) => {
            event.preventDefault();
            void save();
        }}
    >
        <SettingsTextField
            label={$tr('settingsUi.promptName')}
            value={draft.name}
            maxlength={120}
            required
            disabled={busy}
            onchange={(value: string) => {
                if (draft) draft.name = value;
            }}
        />
        <ChoiceField
            id={id + '-memory'}
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
            <fieldset class="settings-prompt-block">
                <legend>{block.name}</legend>
                <label class="settings-checkbox"
                    ><input
                        type="checkbox"
                        bind:checked={block.enabled}
                        disabled={busy || block.kind === 'latest_user_turn'}
                    />{$tr('settingsUi.promptToggle', { name: block.name })}</label
                >
                {#if block.template?.parts.length === 1 && firstPart?.kind === 'text'}
                    <SettingsTextField
                        label={$tr('settingsUi.promptContent')}
                        value={firstPart.value}
                        maxlength={32000}
                        rows={5}
                        required={block.enabled}
                        disabled={busy}
                        onchange={(value: string) => {
                            firstPart.value = value;
                        }}
                    />
                {:else}<p class="settings-note">
                        {$tr(
                            block.template
                                ? 'settingsLive.complexTemplate'
                                : 'settingsLive.dynamicSource',
                        )}
                    </p>{/if}
                {#if block.kind === 'static_instruction' || block.kind === 'post_history_instruction'}
                    <ChoiceField
                        id={id + block.id + '-position'}
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
                    />
                {/if}
                <div class="settings-inline-actions">
                    <button
                        type="button"
                        aria-label={$tr('settingsUi.moveUp', { name: block.name })}
                        disabled={busy || index === 0}
                        onclick={() => move(index, -1)}><ArrowUp aria-hidden="true" /></button
                    >
                    <button
                        type="button"
                        aria-label={$tr('settingsUi.moveDown', { name: block.name })}
                        disabled={busy || index === draft.blocks.length - 1}
                        onclick={() => move(index, 1)}><ArrowDown aria-hidden="true" /></button
                    >
                </div>
            </fieldset>
        {/each}
        <button
            type="button"
            disabled={busy}
            onclick={() =>
                draft?.blocks.push(
                    promptBlock(
                        'static_instruction',
                        $tr('settingsUi.promptName'),
                        { kind: 'template' },
                        'preset_instruction',
                    ),
                )}>{$tr('settingsLive.addBlock')}</button
        >
        <p class="settings-note">{$tr('settingsLive.blockOrderHint')}</p>
        <div class="settings-inline-actions">
            <button class="primary" type="submit" disabled={busy || !dirty || !draft.name.trim()}
                >{$tr('settingsUi.save')}</button
            ><button
                type="button"
                disabled={busy}
                onclick={() => {
                    draft = null;
                    deleting = false;
                }}>{$tr('settingsLive.cancel')}</button
            >
        </div>
    </form>
{/if}
{#if selected}
    <section class="settings-purpose-card settings-form">
        <h3>{$tr('settingsLive.currentRoom')}</h3>
        {#if builtin}<p class="settings-note">{$tr('settingsLive.builtInHint')}</p>{/if}
        <p class="settings-note">
            {services.appState.selected_conversation?.title ?? $tr('settingsLive.chooseRoom')}
        </p>
        <button
            class="primary"
            disabled={busy ||
                dirty ||
                services.orchestrationState.phase !== 'ready' ||
                services.orchestrationState.saving}
            onclick={() => void apply()}>{$tr('settingsLive.applyPrompt')}</button
        >
        {#if services.orchestrationState.error}<p role="alert">
                {services.orchestrationState.error}
            </p>{/if}
        {#if !builtin}<button disabled={busy} onclick={() => (deleting = !deleting)}
                >{$tr('settingsUi.deletePrompt')}</button
            >
            {#if deleting}<div role="alert">
                    <p>{$tr('settingsLive.deleteConfirm')}</p>
                    <button
                        disabled={busy}
                        onclick={async () => {
                            if (await controller.deletePrompt(selected)) {
                                draft = null;
                                deleting = false;
                                await refreshRuntime();
                            }
                        }}>{$tr('settingsLive.confirmDelete')}</button
                    >
                </div>{/if}{/if}
    </section>
{/if}
