<script lang="ts">
    import { tr } from '../../../lib/i18n';
    import type { PortableRuntimeLifecycle } from '../../../features/chat/portable-runtime-lifecycle.svelte';
    import type { PortableRuntimeCapability } from '../../../features/chat/portable-runtime';
    import ChoiceField from '../../../ui/workspace/ChoiceField.svelte';
    import EditField from '../../../ui/workspace/EditField.svelte';
    import { useChoiceSheet } from '../../../ui/workspace/choice-sheet.svelte';
    import './runtime.css';
    let { runtime }: { runtime: PortableRuntimeLifecycle } = $props();
    const choices = useChoiceSheet();
    const capabilityKeys = {
        'runtime:callbacks': 'chat.runtime.capability.callbacks',
        'chat:read': 'chat.runtime.capability.chat_read',
        'chat:write': 'chat.runtime.capability.chat_write',
        'state:readwrite': 'chat.runtime.capability.state_readwrite',
        'profile:read': 'chat.runtime.capability.profile_read',
        'lore:read': 'chat.runtime.capability.lore_read',
        'ui:write': 'chat.runtime.capability.ui_write',
        'model:primary': 'chat.runtime.capability.model_primary',
        'model:auxiliary': 'chat.runtime.capability.model_auxiliary',
        elevated: 'chat.runtime.capability.elevated',
    } as const;
    function toggleCapability(capability: PortableRuntimeCapability, checked: boolean) {
        runtime.selectedCapabilities = checked
            ? [...new Set([...runtime.selectedCapabilities, capability])]
            : runtime.selectedCapabilities.filter((item) => item !== capability);
    }
</script>

<section
    class="ui-settings-group ui-runtime-controls"
    aria-label={$tr('chat.runtime.controls.label')}
>
    <h2 class="ui-settings-heading">{$tr('workspaceRuntime.title')}</h2>
    <p class="ui-runtime-hint" role="status">
        {$tr(
            runtime.phase === 'loading'
                ? 'workspaceRuntime.loading'
                : runtime.phase === 'blocked'
                  ? 'workspaceRuntime.blocked'
                  : runtime.phase === 'busy'
                    ? 'workspaceRuntime.busy'
                    : runtime.phase === 'error'
                      ? 'workspaceRuntime.error'
                      : 'workspaceRuntime.ready',
        )}
    </p>
    {#if runtime.activeGrant === null}
        <p class="ui-runtime-hint">{$tr('workspaceRuntime.grantHint')}</p>
        <p class="ui-runtime-hint">{$tr('chat.runtime.permissions.sensitive')}</p>
        <fieldset>
            <legend class="ui-sr">{$tr('chat.runtime.permissions.requested')}</legend>
            {#each runtime.capabilities as capability (capability)}
                <label class="ui-checkbox-row">
                    <span>{$tr(capabilityKeys[capability])}</span>
                    <input
                        type="checkbox"
                        checked={runtime.selectedCapabilities.includes(capability)}
                        onchange={(event) =>
                            toggleCapability(capability, event.currentTarget.checked)}
                    />
                </label>
            {/each}
        </fieldset>
        <button
            class="ui-submit ui-pressable"
            type="button"
            onclick={() => (runtime.selectedCapabilities = [...runtime.capabilities])}
        >
            <span class="ui-press-visual">{$tr('chat.runtime.permissions.select_all')}</span>
        </button>
        <button class="ui-submit ui-pressable" type="button" onclick={() => void runtime.approve()}>
            <span class="ui-press-visual">{$tr('chat.runtime.permissions.approve_selected')}</span>
        </button>
    {:else}
        <button class="ui-submit ui-pressable" type="button" onclick={() => runtime.revoke()}>
            <span class="ui-press-visual">{$tr('workspaceRuntime.revoke')}</span>
        </button>
        {#if runtime.persistenceStatus?.mode === 'memory-only'}
            <p class="ui-runtime-hint" role="status">
                {$tr('chat.runtime.persistence.memory_only')}
            </p>
        {/if}
        {#if runtime.runtime !== null}
            <fieldset disabled={runtime.phase === 'busy'}>
                {#if runtime.activeGrant.capabilities.includes('model:auxiliary')}
                    <ChoiceField
                        label={$tr('workspaceRuntime.auxiliary')}
                        value={runtime.auxiliaryModelOptions.find(
                            (option) => option.value === runtime.selectedAuxiliaryModel,
                        )?.label ?? ''}
                        onopen={(opener: HTMLButtonElement) =>
                            choices.open(
                                {
                                    label: $tr('workspaceRuntime.auxiliary'),
                                    value: runtime.selectedAuxiliaryModel,
                                    options: runtime.auxiliaryModelOptions,
                                    onselect: (value) => {
                                        if (runtime.phase !== 'busy')
                                            runtime.setAuxiliaryModel(value);
                                    },
                                },
                                opener,
                            )}
                    />
                {/if}
                {#each runtime.runtime.toggles as toggle (toggle.key)}
                    {#if toggle.kind === 'select'}
                        <ChoiceField
                            label={toggle.label}
                            value={toggle.choices[Number(runtime.optionValue(toggle.key))] ?? ''}
                            onopen={(opener: HTMLButtonElement) =>
                                choices.open(
                                    {
                                        label: toggle.label,
                                        value: runtime.optionValue(toggle.key),
                                        options: toggle.choices.map((label, index) => ({
                                            label,
                                            value: String(index),
                                        })),
                                        onselect: (value) => {
                                            if (runtime.phase !== 'busy')
                                                void runtime.setOption(toggle.key, value);
                                        },
                                    },
                                    opener,
                                )}
                        />
                    {:else if toggle.kind === 'toggle'}
                        <label class="ui-checkbox-row"
                            ><span>{toggle.label}</span>
                            <input
                                type="checkbox"
                                checked={runtime.optionValue(toggle.key) === '1'}
                                onchange={(event) =>
                                    void runtime.setOption(
                                        toggle.key,
                                        event.currentTarget.checked ? '1' : '0',
                                    )}
                            />
                        </label>
                    {:else}
                        <EditField
                            label={toggle.label}
                            value={runtime.optionValue(toggle.key)}
                            maxlength={65536}
                            onchange={(value: string) => {
                                if (runtime.phase !== 'busy')
                                    void runtime.setOption(toggle.key, value);
                            }}
                        />
                    {/if}
                {/each}
            </fieldset>
            {#if runtime.modelBudget !== null && (runtime.activeGrant.capabilities.includes('model:primary') || runtime.activeGrant.capabilities.includes('model:auxiliary'))}
                <p class="ui-runtime-hint" aria-live="polite">
                    {$tr('workspaceRuntime.budget', {
                        calls: runtime.modelBudget.callsRemaining,
                        tokens: runtime.modelBudget.tokensRemaining.toLocaleString(),
                    })}
                </p>
                {#if runtime.modelBudget.blockedByUnknownOutcome}<p role="alert">
                        {$tr('workspaceRuntime.unknown')}
                    </p>{/if}
            {/if}
            {#if runtime.modelCall !== null}
                <p role="status">
                    {$tr(
                        runtime.modelCall.target === 'primary'
                            ? 'workspaceRuntime.primaryCall'
                            : 'workspaceRuntime.auxiliaryCall',
                        { name: runtime.modelCall.characterName },
                    )}
                </p>
                <button
                    class="ui-submit ui-pressable"
                    type="button"
                    onclick={() => void runtime.cancelActiveModelCall()}
                >
                    <span class="ui-press-visual">{$tr('workspaceRuntime.stop')}</span>
                </button>
            {/if}
        {/if}
    {/if}
</section>
