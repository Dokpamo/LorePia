<script lang="ts">
    import { onMount, untrack } from 'svelte';
    import type { LorepiaClient } from '../../lib/ipc/contracts';
    import type { PersonaClientApi } from '../../features/personas/persona-contracts';
    import { PersonaController } from '../../features/personas/persona-controller';
    import { t, tr } from '../../lib/i18n';
    import ChoiceField from '../../ui/workspace/ChoiceField.svelte';
    import { useChoiceSheet } from '../../ui/workspace/choice-sheet.svelte';

    let {
        client,
        value = $bindable(''),
        disabled = false,
        onadd,
    }: {
        client: LorepiaClient;
        value?: string;
        disabled?: boolean;
        onadd: (controller: PersonaController) => void;
    } = $props();
    const choices = useChoiceSheet();
    const controller = new PersonaController(untrack(() => client as Partial<PersonaClientApi>));
    const store = controller.state;
    const selected = $derived($store.personas.find((item) => item.value.id === value));
    onMount(() => {
        void controller.loadContext(null);
        return () => controller.destroy();
    });
</script>

<ChoiceField
    label={$tr('persona.title')}
    value={$store.phase === 'loading'
        ? $tr('workspace.loading')
        : (selected?.value.name ?? $tr('workspace.noPersona'))}
    disabled={disabled || $store.phase === 'loading' || $store.phase === 'unavailable'}
    onopen={(opener: HTMLButtonElement) =>
        choices.open(
            {
                label: t('persona.title'),
                value,
                options: [
                    { value: '', label: t('workspace.noPersona') },
                    ...$store.personas.map((item) => ({
                        value: item.value.id,
                        label: item.value.name,
                        description: item.value.description,
                    })),
                ],
                onselect: (next) => {
                    value = next;
                },
                action: { label: t('persona.editor.create_button'), run: () => onadd(controller) },
            },
            opener,
        )}
/>
{#if $store.error}
    <p class="ui-field-error" role="alert">{$store.error}</p>
    {#if $store.phase !== 'unavailable'}
        <button
            type="button"
            class="ui-action-button ui-pressable"
            {disabled}
            onclick={() => controller.loadContext(null)}
        >
            <span class="ui-press-visual">{$tr('workspace.retry')}</span>
        </button>
    {/if}
{:else if $store.next_cursor}
    <button
        type="button"
        class="ui-action-button ui-pressable"
        disabled={disabled || $store.phase === 'loading'}
        onclick={() => controller.loadMore()}
    >
        <span class="ui-press-visual">{$tr('persona.list.load_more')}</span>
    </button>
{/if}
