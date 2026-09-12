<script lang="ts">
    import { tr } from '../../../lib/i18n';
    import type { CreatorControlDto, CreatorControlValue } from '../../../lib/ipc/contracts';
    import type { SettingsServices } from '../../../features/providers/settings/settings-services';
    import SettingsRow from '../../../ui/workspace/SettingsRow.svelte';
    import DataAction from './DataAction.svelte';
    import DataChoice from './DataChoice.svelte';
    import DataNumber from './DataNumber.svelte';
    import DataText from './DataText.svelte';
    let { services }: { services: SettingsServices } = $props();
    let expanded = $state(false);
    const view = $derived(services.orchestrationState);
    const controls = $derived(view.workspace.creator_controls);
    const busy = $derived(view.saving || view.phase !== 'ready');
    function value(control: CreatorControlDto): CreatorControlValue {
        return view.workspace.room_config.creator_values[control.id] ?? control.value;
    }
    function update(control: CreatorControlDto, next: CreatorControlValue) {
        services.orchestrationController.stageCreatorControl(control.id, next);
    }
</script>

{#if controls.length}
    <section class="ui-settings-group">
        <SettingsRow
            label={$tr('importSetup.options')}
            value={String(controls.length)}
            disabled={busy}
            onclick={() => (expanded = !expanded)}
        />
        {#if expanded}
            {#each controls as control (control.id)}
                {@const selectedValue = value(control)}
                {#if control.kind === 'toggle'}
                    <label class="ui-checkbox-row">
                        <span>{control.label}</span>
                        <input
                            type="checkbox"
                            checked={value(control) === true}
                            disabled={busy}
                            onchange={(event) => update(control, event.currentTarget.checked)}
                        />
                    </label>
                {:else if control.kind === 'select'}
                    <DataChoice
                        label={control.label}
                        value={String(value(control))}
                        options={control.choices.map((choice, index) => ({
                            value: choice,
                            label: control.choice_labels?.[index] ?? choice,
                        }))}
                        disabled={busy}
                        onSelect={(next: string) => update(control, next)}
                    />
                {:else if control.kind === 'multi_select'}
                    <fieldset disabled={busy}>
                        <legend>{control.label}</legend>
                        {#each control.choices as choice, index (choice)}
                            <label class="ui-checkbox-row">
                                <span>{control.choice_labels?.[index] ?? choice}</span>
                                <input
                                    type="checkbox"
                                    checked={Array.isArray(selectedValue) &&
                                        selectedValue.includes(choice)}
                                    onchange={(event) => {
                                        const current = value(control);
                                        const selected = Array.isArray(current) ? current : [];
                                        update(
                                            control,
                                            event.currentTarget.checked
                                                ? [...selected, choice]
                                                : selected.filter((item) => item !== choice),
                                        );
                                    }}
                                />
                            </label>
                        {/each}
                    </fieldset>
                {:else if control.kind === 'number' || control.kind === 'slider'}
                    <DataNumber
                        label={control.label}
                        value={Number(value(control))}
                        min={control.minimum ?? undefined}
                        max={control.maximum ?? undefined}
                        integer={Number.isInteger(control.step ?? 1)}
                        disabled={busy}
                        onchange={(next: number) => update(control, next)}
                    />
                {:else}
                    <DataText
                        label={control.label}
                        value={String(value(control))}
                        disabled={busy}
                        onchange={(next: string) => update(control, next)}
                    />
                {/if}
                {#if control.description}<p>{control.description}</p>{/if}
            {/each}
            <DataAction
                disabled={busy || !view.dirty_room_config}
                onclick={() => void services.orchestrationController.saveRoomConfig()}
                >{$tr('settingsUi.save')}</DataAction
            >
        {/if}
    </section>
{/if}
