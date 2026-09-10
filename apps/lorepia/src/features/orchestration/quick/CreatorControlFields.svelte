<script lang="ts">
    import ChoicePopover from '../../../components/ChoicePopover.svelte';
    import ToggleSwitch from '../../../components/ToggleSwitch.svelte';
    import { tr } from '../../../lib/i18n';
    import type {
        CreatorControlDto,
        CreatorControlValue,
        RoomOrchestrationConfigDto,
    } from '../../../lib/ipc/contracts';
    import { creatorControlOptions } from '../creator-control-options';
    import type { OrchestrationController } from '../orchestration-controller';

    interface Props {
        controls: CreatorControlDto[];
        roomConfig: RoomOrchestrationConfigDto;
        controller: OrchestrationController;
    }
    let { controls, roomConfig, controller }: Props = $props();

    function controlValue(control: CreatorControlDto): CreatorControlValue {
        return roomConfig.creator_values[control.id] ?? control.value;
    }

    function selectedValues(control: CreatorControlDto): string[] {
        const value = controlValue(control);
        return Array.isArray(value) ? value : [];
    }

    function toggleMultiChoice(control: CreatorControlDto, choice: string, checked: boolean): void {
        const values = selectedValues(control);
        const nextValues = checked
            ? values.includes(choice)
                ? values
                : [...values, choice]
            : values.filter((value) => value !== choice);
        controller.stageCreatorControl(control.id, nextValues);
    }
</script>

<fieldset>
    <legend>{$tr('quick.creator_controls')}</legend>
    <div class="creator-controls">
        {#each controls.slice(0, 256) as control (control.id)}
            {#if control.kind === 'toggle'}
                <ToggleSwitch
                    label={control.label}
                    checked={Boolean(controlValue(control))}
                    showLabel
                    onChange={(checked: boolean) =>
                        controller.stageCreatorControl(control.id, checked)}
                />
            {:else if control.kind === 'select'}
                <div class="creator-choice-row">
                    <ChoicePopover
                        id={`creator-control-${control.id}`}
                        label={control.label}
                        value={String(controlValue(control))}
                        options={creatorControlOptions(control)}
                        onSelect={(value: string) =>
                            controller.stageCreatorControl(control.id, value)}
                    />
                </div>
            {:else if control.kind === 'multi_select'}
                <fieldset class="nested-fieldset">
                    <legend>{control.label}</legend>
                    {#each control.choices.slice(0, 40) as choice (choice)}
                        <ToggleSwitch
                            label={choice}
                            checked={selectedValues(control).includes(choice)}
                            showLabel
                            onChange={(checked: boolean) =>
                                toggleMultiChoice(control, choice, checked)}
                        />
                    {/each}
                </fieldset>
            {:else if control.kind === 'number' || control.kind === 'slider'}
                <label>
                    <span>{control.label}</span>
                    <input
                        type={control.kind === 'slider' ? 'range' : 'number'}
                        min={control.minimum ?? undefined}
                        max={control.maximum ?? undefined}
                        step={control.step ?? 1}
                        value={Number(controlValue(control))}
                        oninput={(event) =>
                            controller.stageCreatorControl(
                                control.id,
                                Number(event.currentTarget.value),
                            )}
                    />
                </label>
            {:else}
                <label>
                    <span>{control.label}</span>
                    <input
                        type="text"
                        maxlength="4096"
                        value={String(controlValue(control))}
                        oninput={(event) =>
                            controller.stageCreatorControl(control.id, event.currentTarget.value)}
                    />
                </label>
            {/if}
            {#if control.description}
                <small>{control.description}</small>
            {/if}
        {/each}
    </div>
</fieldset>

<style>
    fieldset {
        display: grid;
        gap: 9px;
        min-width: 0;
        margin: 0;
        padding: 12px;
        border: 0;
        border-radius: var(--radius-md);
    }
    .nested-fieldset {
        background: var(--surface-sunken);
    }
    .creator-controls {
        display: grid;
        gap: 10px;
    }
    .creator-choice-row {
        min-width: 0;
        border-radius: var(--radius-md);
        background: var(--surface-sunken);
    }
    .creator-controls > label {
        display: grid;
        gap: 6px;
    }
    .creator-controls small {
        color: var(--ink-muted);
    }
    input[type='text'],
    input[type='number'] {
        height: 38px;
        min-height: 38px;
        padding: 0 34px 0 12px;
        border: 1px solid var(--line);
        border-radius: var(--radius-md);
        background: var(--bg);
        color: var(--ink);
    }
</style>
