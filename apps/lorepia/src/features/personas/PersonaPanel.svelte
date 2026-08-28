<script lang="ts">
    import { UserRoundPlus } from '@lucide/svelte';
    import DetailActionBar from '../../components/detail/DetailActionBar.svelte';
    import DetailPage from '../../components/detail/DetailPage.svelte';
    import { tr } from '../../lib/i18n';
    import type { PersonaController, PersonaState } from './persona-controller';
    import type { PersonaDto } from './persona-contracts';

    interface Props {
        personaState: PersonaState;
        controller: PersonaController;
        editorMode?: 'create' | 'edit' | null;
    }

    let { personaState, controller, editorMode = $bindable(null) }: Props = $props();
    let editingPersona = $state.raw<PersonaDto | null>(null);
    let name = $state('');
    let description = $state('');
    let deleteConfirmationId = $state<string | null>(null);

    const busy = $derived(personaState.phase === 'loading' || personaState.phase === 'saving');

    function beginCreate(): void {
        editingPersona = null;
        name = '';
        description = '';
        deleteConfirmationId = null;
        editorMode = 'create';
    }

    function closeEditor(): void {
        editingPersona = null;
        name = '';
        description = '';
        deleteConfirmationId = null;
        editorMode = null;
    }

    function beginEdit(persona: PersonaDto): void {
        editingPersona = {
            ...persona,
            value: { ...persona.value },
        };
        name = persona.value.name;
        description = persona.value.description;
        deleteConfirmationId = null;
        editorMode = 'edit';
    }

    async function save(): Promise<void> {
        const trimmedName = name.trim();
        const trimmedDescription = description.trim();
        if (trimmedName === '') return;
        const saved =
            editingPersona === null
                ? await controller.create(trimmedName, trimmedDescription)
                : await controller.updatePersona(editingPersona, trimmedName, trimmedDescription);
        if (saved) closeEditor();
    }

    async function deleteEditingPersona(): Promise<void> {
        if (editingPersona === null) return;
        const deleted = await controller.deletePersona(editingPersona);
        if (deleted) closeEditor();
    }

    $effect(() => {
        if (editorMode !== null) return;
        editingPersona = null;
        name = '';
        description = '';
        deleteConfirmationId = null;
    });
</script>

{#snippet detailContent()}
    {#if personaState.phase === 'unavailable'}
        <div class="persona-feedback warning" role="status">
            <p>{personaState.error}</p>
        </div>
    {:else if editorMode !== null}
        <form
            id="persona-editor-form"
            class="persona-form persona-editor-page"
            aria-label={editorMode === 'create'
                ? $tr('persona.editor.new')
                : $tr('persona.editor.edit')}
            onsubmit={(event) => {
                event.preventDefault();
                void save();
            }}
        >
            <label>
                <span>{$tr('persona.editor.name')}</span>
                <input
                    bind:value={name}
                    required
                    maxlength="120"
                    autocomplete="off"
                    disabled={busy}
                />
            </label>
            <label>
                <span>{$tr('persona.editor.description')}</span>
                <textarea bind:value={description} rows="3" maxlength="4000" disabled={busy}
                ></textarea>
            </label>
        </form>
    {:else}
        {#if personaState.phase === 'loading'}
            <div class="persona-feedback" role="status">
                <p>{$tr('persona.loading')}</p>
            </div>
        {:else if personaState.phase === 'error'}
            <div class="persona-feedback error" role="alert">
                <p>{personaState.error}</p>
                <button
                    type="button"
                    onclick={() => void controller.loadContext(personaState.conversation_id)}
                >
                    {$tr('persona.error.reload')}
                </button>
            </div>
        {/if}

        <div class="persona-catalog">
            <div class="setting-list persona-list">
                {#if personaState.personas.length === 0 && personaState.phase !== 'loading'}
                    <p class="persona-empty-state">{$tr('persona.list.empty')}</p>
                {/if}
                {#each personaState.personas as persona (persona.value.id)}
                    <button
                        class="setting-row persona-row"
                        type="button"
                        disabled={busy}
                        onclick={() => beginEdit(persona)}
                    >
                        <span class="setting-content">
                            <span class="setting-copy persona-row-copy">
                                <strong class="persona-row-name">{persona.value.name}</strong>
                                <small class="persona-row-description">
                                    {persona.value.description || $tr('persona.description.empty')}
                                </small>
                            </span>
                        </span>
                    </button>
                {/each}
                {#if personaState.next_cursor !== null}
                    <button
                        class="persona-load-more"
                        type="button"
                        disabled={busy}
                        onclick={() => void controller.loadMore()}
                    >
                        {$tr('persona.list.load_more')}
                    </button>
                {/if}
            </div>
        </div>
    {/if}
{/snippet}

{#snippet detailActions()}
    {#if personaState.phase !== 'unavailable'}
        <DetailActionBar className="persona-action-bar" ariaLabel={$tr('persona.actions.label')}>
            {#if editorMode === null}
                <button
                    class="primary detail-action detail-action--wide persona-bar-action persona-bar-action-wide"
                    type="button"
                    disabled={busy}
                    onclick={beginCreate}
                >
                    <UserRoundPlus class="persona-add-icon" aria-hidden="true" />
                    {$tr('persona.editor.create_button')}
                </button>
            {:else if editingPersona !== null && deleteConfirmationId === editingPersona.value.id}
                <button
                    class="danger detail-action detail-action--destructive persona-bar-action persona-delete-confirm"
                    type="button"
                    disabled={busy}
                    onclick={() => void deleteEditingPersona()}
                >
                    {$tr('persona.list.confirm_delete')}
                </button>
                <button
                    class="detail-action detail-action--grow persona-bar-action persona-cancel-action"
                    type="button"
                    disabled={busy}
                    onclick={() => (deleteConfirmationId = null)}
                >
                    {$tr('persona.list.cancel')}
                </button>
            {:else}
                {#if editingPersona !== null}
                    <button
                        class="detail-action detail-action--destructive detail-action--borderless persona-bar-action persona-delete-button"
                        type="button"
                        disabled={busy}
                        onclick={() => (deleteConfirmationId = editingPersona?.value.id ?? null)}
                    >
                        {$tr('persona.list.delete')}
                    </button>
                {/if}
                <button
                    class="primary detail-action detail-action--grow persona-bar-action persona-save-action"
                    type="submit"
                    form="persona-editor-form"
                    disabled={busy || name.trim() === ''}
                >
                    {editingPersona === null
                        ? $tr('persona.editor.submit_create')
                        : $tr('persona.editor.submit_update')}
                </button>
            {/if}
        </DetailActionBar>
    {/if}
{/snippet}

<DetailPage
    className="persona-panel"
    scrollClassName="provider-scroll settings-detail-scroll persona-scroll"
    ariaLabel={$tr('persona.title')}
    resetKey={editorMode ?? 'list'}
    content={detailContent}
    actions={detailActions}
/>

<style>
    .persona-empty-state,
    .persona-feedback {
        padding: 12px;
        border-radius: 12px;
        margin: 0;
        background: var(--surface-sunken);
        color: var(--ink-muted);
        line-height: 1.5;
    }

    .persona-feedback {
        display: flex;
        align-items: center;
        justify-content: space-between;
        gap: 12px;
    }

    .persona-feedback p {
        margin: 0;
    }

    .persona-feedback.error {
        border: 1px solid var(--status-error-border);
        color: var(--status-error-fg);
        background: var(--status-error-bg);
    }

    .persona-feedback.warning {
        border: 1px solid var(--status-warning-border);
        color: var(--status-warning-fg);
        background: var(--status-warning-bg);
    }

    .persona-editor-page {
        display: grid;
        gap: 14px;
    }

    .persona-form label {
        display: grid;
        gap: 7px;
        color: var(--ink-muted);
        font-size: var(--detail-support-type);
        font-weight: 700;
    }

    .persona-form input,
    .persona-form textarea {
        width: 100%;
        min-width: 0;
        box-sizing: border-box;
        padding: clamp(12px, 3.432vw, 15px);
        border: 1.5px solid var(--line);
        border-radius: var(--radius-md);
        -webkit-appearance: none;
        appearance: none;
        background: color-mix(in srgb, var(--surface-sunken) 26%, var(--surface-raised));
        box-shadow: var(--control-inset-shadow);
        caret-color: var(--accent);
        color: var(--ink);
        font-size: var(--detail-support-type);
        line-height: 1.5;
        transition:
            background-color 140ms ease,
            box-shadow 140ms ease;
    }

    .persona-form input {
        min-height: clamp(48px, 13.73vw, 60px);
    }

    .persona-form textarea {
        min-height: clamp(112px, 32.037vw, 140px);
        resize: none;
    }

    .persona-form :is(input, textarea):hover:not(:focus, :disabled) {
        border-color: var(--line);
    }

    .persona-form :is(input, textarea):focus {
        border-color: var(--accent);
        outline: none;
    }

    .persona-form :is(input, textarea):disabled {
        cursor: not-allowed;
        opacity: var(--disabled-opacity);
    }

    .persona-bar-action :global(.persona-add-icon) {
        width: 20px;
        height: 20px;
        flex: none;
        fill: none;
        stroke: currentcolor;
        stroke-linecap: round;
        stroke-linejoin: round;
        stroke-width: 1.8;
    }

    .persona-list {
        width: 100%;
        margin: 0;
    }

    .persona-row-copy {
        display: grid;
        min-width: 0;
        gap: 5px;
    }

    .persona-row-name,
    .persona-row-description {
        overflow: hidden;
        color: var(--ink);
        font-size: var(--detail-support-type);
        font-weight: 550;
        line-height: 1.35;
        text-overflow: ellipsis;
    }

    .persona-row-name {
        white-space: nowrap;
    }

    .persona-row-description {
        display: -webkit-box;
        color: var(--ink-muted);
        overflow-wrap: anywhere;
        white-space: normal;
        line-clamp: 3;
        -webkit-box-orient: vertical;
        -webkit-line-clamp: 3;
    }

    .persona-load-more {
        align-self: center;
        margin: 8px;
    }
</style>
