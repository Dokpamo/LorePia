<script lang="ts">
    import { tr } from '../../lib/i18n';
    import type { PersonaController, PersonaState } from './persona-controller';
    import type { PersonaDto } from './persona-contracts';

    interface Props {
        personaState: PersonaState;
        controller: PersonaController;
        conversationTitle?: string | null;
    }

    let { personaState, controller, conversationTitle = null }: Props = $props();
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
    }

    function beginEdit(persona: PersonaDto): void {
        editingPersona = structuredClone(persona);
        name = persona.value.name;
        description = persona.value.description;
        deleteConfirmationId = null;
    }

    async function save(): Promise<void> {
        const trimmedName = name.trim();
        const trimmedDescription = description.trim();
        if (trimmedName === '') return;
        const saved =
            editingPersona === null
                ? await controller.create(trimmedName, trimmedDescription)
                : await controller.updatePersona(editingPersona, trimmedName, trimmedDescription);
        if (saved) beginCreate();
    }
</script>

<section class="settings-section persona-panel" aria-labelledby="persona-settings-title">
    <div class="section-heading">
        <div>
            <p class="eyebrow">Local persona</p>
            <h2 id="persona-settings-title">{$tr('persona.title')}</h2>
            <p>{$tr('persona.hint')}</p>
        </div>
        <button
            type="button"
            disabled={busy}
            onclick={() => void controller.loadContext(personaState.conversation_id)}
        >
            {$tr('common.refresh')}
        </button>
    </div>

    {#if personaState.phase === 'unavailable'}
        <p class="inline-note warning" role="status">{personaState.error}</p>
    {:else}
        <section class="persona-selection" aria-labelledby="persona-selection-title">
            <div class="section-heading compact">
                <div>
                    <h3 id="persona-selection-title">{$tr('persona.selection.title')}</h3>
                    <p>
                        {conversationTitle ??
                            (personaState.conversation_id === null
                                ? $tr('persona.selection.none_conversation')
                                : $tr('persona.selection.current'))}
                    </p>
                </div>
                {#if personaState.selection?.selected_persona}
                    <button
                        type="button"
                        disabled={busy}
                        onclick={() => void controller.clearSelection()}
                    >
                        {$tr('persona.selection.clear')}
                    </button>
                {/if}
            </div>
            {#if personaState.conversation_id === null}
                <p class="inline-note">{$tr('persona.selection.pick_conversation')}</p>
            {:else if personaState.selection?.selected_persona}
                {@const selected = personaState.selection.selected_persona}
                <article class="persona-selected-card">
                    <strong>{selected.value.name}</strong>
                    <p>{selected.value.description || $tr('persona.description.empty')}</p>
                    <small>
                        {$tr('persona.selection.revision', { revision: selected.revision })}
                    </small>
                </article>
            {:else}
                <p class="inline-note">{$tr('persona.selection.empty')}</p>
            {/if}
        </section>

        <form
            class="persona-form"
            aria-labelledby="persona-editor-title"
            onsubmit={(event) => {
                event.preventDefault();
                void save();
            }}
        >
            <div class="section-heading compact">
                <h3 id="persona-editor-title">
                    {editingPersona === null
                        ? $tr('persona.editor.new')
                        : $tr('persona.editor.edit')}
                </h3>
                {#if editingPersona !== null}
                    <button type="button" disabled={busy} onclick={beginCreate}
                        >{$tr('persona.editor.create_button')}</button
                    >
                {/if}
            </div>
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
            <button class="primary" type="submit" disabled={busy || name.trim() === ''}>
                {editingPersona === null
                    ? $tr('persona.editor.submit_create')
                    : $tr('persona.editor.submit_update')}
            </button>
        </form>

        {#if personaState.phase === 'loading'}
            <p class="inline-note" role="status">{$tr('persona.loading')}</p>
        {:else if personaState.phase === 'error'}
            <p class="inline-note warning" role="alert">{personaState.error}</p>
        {/if}

        <div class="persona-list" aria-label={$tr('persona.list.label')}>
            {#if personaState.personas.length === 0 && personaState.phase !== 'loading'}
                <p class="inline-note">{$tr('persona.list.empty')}</p>
            {/if}
            {#each personaState.personas as persona (persona.value.id)}
                {@const isSelected =
                    personaState.selection?.selected_persona?.value.id === persona.value.id}
                <article class="persona-card" class:selected={isSelected}>
                    <header>
                        <div>
                            <h3>{persona.value.name}</h3>
                            <p>{persona.value.description || $tr('persona.description.empty')}</p>
                        </div>
                        <span class="status-pill">r{persona.revision}</span>
                    </header>
                    {#if isSelected}
                        <p class="persona-pin-note">
                            {$tr('persona.list.pinned', {
                                revision: personaState.selection?.selected_persona?.revision ?? 0,
                            })}
                        </p>
                    {/if}
                    <div class="persona-actions">
                        <button
                            class="primary"
                            type="button"
                            disabled={busy || personaState.conversation_id === null || isSelected}
                            onclick={() => void controller.selectPersona(persona)}
                        >
                            {isSelected ? $tr('persona.list.in_use') : $tr('persona.list.select')}
                        </button>
                        <button type="button" disabled={busy} onclick={() => beginEdit(persona)}>
                            {$tr('persona.list.edit')}
                        </button>
                        {#if deleteConfirmationId === persona.value.id}
                            <button
                                class="danger"
                                type="button"
                                disabled={busy}
                                onclick={() => {
                                    deleteConfirmationId = null;
                                    void controller.deletePersona(persona);
                                }}
                            >
                                {$tr('persona.list.confirm_delete')}
                            </button>
                            <button
                                type="button"
                                disabled={busy}
                                onclick={() => (deleteConfirmationId = null)}
                            >
                                {$tr('persona.list.cancel')}
                            </button>
                        {:else}
                            <button
                                type="button"
                                disabled={busy}
                                onclick={() => (deleteConfirmationId = persona.value.id)}
                            >
                                {$tr('persona.list.delete')}
                            </button>
                        {/if}
                    </div>
                </article>
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
    {/if}
</section>

<style>
    .persona-panel {
        display: grid;
        gap: 1rem;
    }

    .persona-selection,
    .persona-form {
        display: grid;
        gap: 0.75rem;
        padding: 1rem;
        border: 1px solid var(--line);
        border-radius: 0.85rem;
        background: var(--surface-raised);
    }

    .section-heading.compact {
        align-items: start;
        margin: 0;
    }

    .section-heading.compact h3,
    .persona-card h3 {
        margin: 0;
    }

    .section-heading.compact p,
    .persona-card p {
        margin: 0.25rem 0 0;
    }

    .persona-selected-card {
        padding: 0.85rem;
        border-left: 0.25rem solid var(--accent);
        border-radius: 0.5rem;
        background: var(--surface-sunken);
    }

    .persona-selected-card p,
    .persona-selected-card small {
        display: block;
        margin: 0.3rem 0 0;
    }

    .persona-form label {
        display: grid;
        gap: 0.35rem;
    }

    .persona-form input,
    .persona-form textarea {
        width: 100%;
        box-sizing: border-box;
    }

    .persona-list {
        display: grid;
        gap: 0.75rem;
    }

    .persona-card {
        display: grid;
        gap: 0.75rem;
        padding: 1rem;
        border: 1px solid var(--line);
        border-radius: 0.85rem;
    }

    .persona-card.selected {
        border-color: var(--accent);
    }

    .persona-card header {
        display: flex;
        justify-content: space-between;
        gap: 1rem;
    }

    .persona-pin-note {
        color: var(--ink-muted);
        font-size: 0.9rem;
    }

    .persona-actions {
        display: flex;
        flex-wrap: wrap;
        gap: 0.5rem;
    }

    .persona-load-more {
        justify-self: center;
    }

    button.danger {
        color: var(--danger);
    }
</style>
