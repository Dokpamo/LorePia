<script lang="ts">
    import type {
        PersonaController,
        PersonaState,
    } from '../../../features/personas/persona-controller';
    import type { PersonaDto } from '../../../features/personas/persona-contracts';
    import { tr } from '../../../lib/i18n';
    import DataAction from './DataAction.svelte';
    import DataText from './DataText.svelte';
    import DataChoice from './DataChoice.svelte';
    let {
        controller,
        personaState,
    }: { controller: PersonaController; personaState: PersonaState } = $props();
    let editing = $state(false),
        name = $state(''),
        description = $state(''),
        deleting = $state(false);
    let selected = $state<PersonaDto | null>(null);
    const busy = $derived(personaState.phase === 'saving' || personaState.phase === 'loading');
    function edit(item: PersonaDto | null) {
        selected = item;
        name = item?.value.name ?? '';
        description = item?.value.description ?? '';
        editing = true;
        deleting = false;
    }
    async function save() {
        if (!name.trim() || busy) return;
        const ok = selected
            ? await controller.updatePersona(selected, name, description)
            : await controller.create(name, description);
        if (ok) editing = false;
    }
    export function isDirty() {
        return (
            editing &&
            (name !== (selected?.value.name ?? '') ||
                description !== (selected?.value.description ?? ''))
        );
    }
    export function isBusy() {
        return busy;
    }
</script>

<section class="ui-settings-group">
    {#if editing}
        <DataText
            label={$tr('persona.editor.name')}
            value={name}
            maxlength={120}
            disabled={busy}
            onchange={(value: string) => (name = value)}
        />
        <DataText
            label={$tr('persona.editor.description')}
            value={description}
            maxlength={4000}
            disabled={busy}
            onchange={(value: string) => (description = value)}
        />
        <DataAction disabled={busy || !name.trim()} onclick={() => void save()}
            >{$tr('settingsUi.save')}</DataAction
        >
        <DataAction disabled={busy} onclick={() => (editing = false)}
            >{$tr('settingsLive.cancel')}</DataAction
        >
        {#if selected}<DataAction disabled={busy} onclick={() => (deleting = !deleting)}
                >{$tr('settingsLive.confirmDelete')}</DataAction
            >{/if}
        {#if deleting && selected}<p>{$tr('settingsLive.deleteConfirm')}</p>
            <DataAction
                disabled={busy}
                onclick={async () => {
                    if (selected && (await controller.deletePersona(selected))) editing = false;
                }}>{$tr('settingsLive.confirmDelete')}</DataAction
            >{/if}
    {:else}
        <DataAction disabled={busy} onclick={() => edit(null)}
            >{$tr('persona.editor.new')}</DataAction
        >
        {#each personaState.personas as item (item.value.id)}<DataAction
                disabled={busy}
                onclick={() => edit(item)}>{item.value.name}</DataAction
            >{/each}
        {#if personaState.next_cursor}<DataAction
                disabled={busy}
                onclick={() => void controller.loadMore()}
                >{$tr('persona.list.load_more')}</DataAction
            >{/if}
        <DataChoice
            label={$tr('settingsLive.personaForRoom')}
            value={personaState.selection?.selected_persona?.value.id ?? ''}
            options={[
                { value: '', label: $tr('settingsUi.none') },
                ...personaState.personas.map((item) => ({
                    value: item.value.id,
                    label: item.value.name,
                })),
            ]}
            disabled={busy || !personaState.conversation_id}
            onSelect={(value: string) => {
                const item = personaState.personas.find((item) => item.value.id === value);
                if (item) void controller.selectPersona(item);
                else void controller.clearSelection();
            }}
        />
    {/if}
    {#if personaState.error}<p role="alert">{personaState.error}</p>{/if}
    {#if personaState.announcement}<p role="status">{personaState.announcement}</p>{/if}
</section>
