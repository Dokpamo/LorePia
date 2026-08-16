<script lang="ts">
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
            <h2 id="persona-settings-title">내 Persona</h2>
            <p>Persona는 로컬 사용자 소유이며, 선택은 대화마다 따로 저장됩니다.</p>
        </div>
        <button
            type="button"
            disabled={busy}
            onclick={() => void controller.loadContext(personaState.conversation_id)}
        >
            새로고침
        </button>
    </div>

    {#if personaState.phase === 'unavailable'}
        <p class="inline-note warning" role="status">{personaState.error}</p>
    {:else}
        <section class="persona-selection" aria-labelledby="persona-selection-title">
            <div class="section-heading compact">
                <div>
                    <h3 id="persona-selection-title">현재 대화 Persona</h3>
                    <p>
                        {conversationTitle ??
                            (personaState.conversation_id === null
                                ? '선택된 대화 없음'
                                : '현재 대화')}
                    </p>
                </div>
                {#if personaState.selection?.selected_persona}
                    <button
                        type="button"
                        disabled={busy}
                        onclick={() => void controller.clearSelection()}
                    >
                        선택 해제
                    </button>
                {/if}
            </div>
            {#if personaState.conversation_id === null}
                <p class="inline-note">대화를 선택하면 그 대화의 Persona를 지정할 수 있습니다.</p>
            {:else if personaState.selection?.selected_persona}
                {@const selected = personaState.selection.selected_persona}
                <article class="persona-selected-card">
                    <strong>{selected.value.name}</strong>
                    <p>{selected.value.description || '설명 없음'}</p>
                    <small>
                        선택 리비전 {selected.revision} · 이후 Persona 편집과 분리된 불변 스냅샷
                    </small>
                </article>
            {:else}
                <p class="inline-note">이 대화에는 선택된 Persona가 없습니다.</p>
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
                    {editingPersona === null ? '새 Persona' : 'Persona 편집'}
                </h3>
                {#if editingPersona !== null}
                    <button type="button" disabled={busy} onclick={beginCreate}>새로 만들기</button>
                {/if}
            </div>
            <label>
                <span>이름</span>
                <input
                    bind:value={name}
                    required
                    maxlength="120"
                    autocomplete="off"
                    disabled={busy}
                />
            </label>
            <label>
                <span>설명</span>
                <textarea bind:value={description} rows="3" maxlength="4000" disabled={busy}
                ></textarea>
            </label>
            <button class="primary" type="submit" disabled={busy || name.trim() === ''}>
                {editingPersona === null ? 'Persona 만들기' : '변경 저장'}
            </button>
        </form>

        {#if personaState.phase === 'loading'}
            <p class="inline-note" role="status">Persona를 불러오는 중입니다.</p>
        {:else if personaState.phase === 'error'}
            <p class="inline-note warning" role="alert">{personaState.error}</p>
        {/if}

        <div class="persona-list" aria-label="저장된 Persona">
            {#if personaState.personas.length === 0 && personaState.phase !== 'loading'}
                <p class="inline-note">저장된 Persona가 없습니다.</p>
            {/if}
            {#each personaState.personas as persona (persona.value.id)}
                {@const isSelected =
                    personaState.selection?.selected_persona?.value.id === persona.value.id}
                <article class="persona-card" class:selected={isSelected}>
                    <header>
                        <div>
                            <h3>{persona.value.name}</h3>
                            <p>{persona.value.description || '설명 없음'}</p>
                        </div>
                        <span class="status-pill">r{persona.revision}</span>
                    </header>
                    {#if isSelected}
                        <p class="persona-pin-note">
                            이 대화는 현재 Persona의 r{personaState.selection?.selected_persona
                                ?.revision}
                            스냅샷을 사용합니다.
                        </p>
                    {/if}
                    <div class="persona-actions">
                        <button
                            class="primary"
                            type="button"
                            disabled={busy || personaState.conversation_id === null || isSelected}
                            onclick={() => void controller.selectPersona(persona)}
                        >
                            {isSelected ? '이 대화에서 사용 중' : '이 대화에 선택'}
                        </button>
                        <button type="button" disabled={busy} onclick={() => beginEdit(persona)}>
                            편집
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
                                삭제 확인
                            </button>
                            <button
                                type="button"
                                disabled={busy}
                                onclick={() => (deleteConfirmationId = null)}
                            >
                                취소
                            </button>
                        {:else}
                            <button
                                type="button"
                                disabled={busy}
                                onclick={() => (deleteConfirmationId = persona.value.id)}
                            >
                                삭제
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
                    더 불러오기
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
        border: 1px solid var(--line, #d8d6d0);
        border-radius: 0.85rem;
        background: var(--surface-raised, #fff);
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
        border-left: 0.25rem solid var(--accent, #6750a4);
        border-radius: 0.5rem;
        background: var(--surface-muted, #f6f3fb);
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
        border: 1px solid var(--line, #d8d6d0);
        border-radius: 0.85rem;
    }

    .persona-card.selected {
        border-color: var(--accent, #6750a4);
    }

    .persona-card header {
        display: flex;
        justify-content: space-between;
        gap: 1rem;
    }

    .persona-pin-note {
        color: var(--text-muted, #625f68);
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
        color: var(--danger, #9f1d20);
    }
</style>
