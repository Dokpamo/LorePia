<script lang="ts">
    import { tick } from 'svelte';
    import type { PersonaController } from '../../features/personas/persona-controller';
    import type { PersonaDto } from '../../features/personas/persona-contracts';
    import { tr } from '../../lib/i18n';
    import SettingsPanel from '../../ui/workspace/SettingsPanel.svelte';
    import EditField from '../../ui/workspace/EditField.svelte';
    import DiscardChanges from '../../ui/workspace/DiscardChanges.svelte';
    import type { BackDecision } from '../../ui/workspace/edge-back';

    let {
        controller,
        onclose,
        oncreated,
        covered = false,
    }: {
        controller: PersonaController;
        onclose: () => void;
        oncreated: (persona: PersonaDto) => void;
        covered?: boolean;
    } = $props();
    const formId = $props.id();
    const store = $derived(controller.state);
    let name = $state('');
    let description = $state('');
    let saving = $state(false);
    let confirming = $state(false);
    let resumeBack: (() => Promise<void>) | undefined;
    function beforeback(): BackDecision {
        if (saving) return false;
        if (name.trim() || description.trim())
            return {
                confirm: (resume) => {
                    resumeBack = resume;
                    confirming = true;
                },
            };
        return true;
    }
    async function keepEditing() {
        confirming = false;
        await tick();
        await resumeBack?.();
        resumeBack = undefined;
    }
    async function save() {
        if (saving || !name.trim()) return;
        saving = true;
        try {
            const created = await controller.createWithResult(name.trim(), description.trim());
            if (created) oncreated(created);
        } finally {
            saving = false;
        }
    }
</script>

<SettingsPanel
    title={$tr('persona.editor.new')}
    kind="persona-create"
    inlineTitle
    {onclose}
    {beforeback}
    disabled={saving}
    covered={covered || confirming}
>
    <form
        id={formId}
        class="ui-edit-form persona-create-fields"
        aria-label={$tr('persona.editor.new')}
        onsubmit={(event) => {
            event.preventDefault();
            void save();
        }}
    >
        <fieldset disabled={saving}>
            <EditField
                label={$tr('persona.editor.name')}
                value={name}
                maxlength={120}
                onchange={(value: string) => {
                    name = value.replaceAll('\n', ' ');
                }}
            />
            <EditField
                label={$tr('persona.editor.description')}
                value={description}
                maxlength={4000}
                onchange={(value: string) => {
                    description = value;
                }}
            />
        </fieldset>
        {#if $store.error}<p class="ui-field-error" role="alert">{$store.error}</p>{/if}
    </form>
    {#snippet footer()}
        <button
            type="submit"
            form={formId}
            class="ui-submit ui-pressable"
            disabled={saving || !name.trim()}
        >
            <span class="ui-press-visual"
                >{$tr(saving ? 'workspace.loading' : 'persona.editor.submit_create')}</span
            >
        </button>
    {/snippet}
</SettingsPanel>
{#if confirming}<DiscardChanges onkeep={keepEditing} ondiscard={onclose} />{/if}

<style>
    .persona-create-fields {
        padding-inline: var(--ui-inset);
    }
    fieldset {
        border: 0;
        margin: 0;
        padding: 0;
        min-width: 0;
    }
</style>
