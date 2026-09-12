<script lang="ts">
    import { onMount, untrack } from 'svelte';
    import { tr } from '../../../lib/i18n';
    import { ModuleActivationController } from '../../../features/orchestration/module-activation-controller';
    import {
        contentModuleCandidateKey,
        contentModuleComponentKey,
        type ContentModuleActivationState,
    } from '../../../features/orchestration/module-lifecycle-controller';
    import type { ContentModulePagedClientApi } from '../../../features/orchestration/module-activation-pages';
    import DataAction from './DataAction.svelte';
    import { ChevronDown } from '@lucide/svelte';
    import DataChoice from './DataChoice.svelte';
    let {
        client,
        activation,
        onComplete,
        onBusy,
    }: {
        client: ContentModulePagedClientApi;
        activation: ContentModuleActivationState;
        onComplete: () => void;
        onBusy: (busy: boolean) => void;
    } = $props();
    const controller = untrack(() => new ModuleActivationController(client, activation));
    const moduleStore = controller.state;
    let showComponents = $state(false);
    const missingChoice = $derived(
        $moduleStore.page?.conflicts.some(
            (c) => !$moduleStore.choices[contentModuleComponentKey(c.component)],
        ) ?? false,
    );
    $effect(() => onBusy($moduleStore.busy));
    onMount(() => () => {
        controller.destroy();
        onBusy(false);
    });
    async function apply() {
        if (await controller.activate()) onComplete();
    }
</script>

{#if $moduleStore.page}
    <p>{$tr('importSetup.moduleComponentCount', { count: $moduleStore.page.component_count })}</p>
    {#each $moduleStore.page.conflicts as conflict (contentModuleComponentKey(conflict.component))}
        <DataChoice
            label={conflict.component.id}
            value={$moduleStore.choices[contentModuleComponentKey(conflict.component)] ?? ''}
            options={[
                { value: '', label: $tr('settingsUi.none') },
                ...conflict.candidates.map((c) => ({
                    value: contentModuleCandidateKey(c),
                    label: c.module_id + ' · ' + c.revision_id,
                })),
                { value: 'omit', label: $tr('importSetup.moduleOmit') },
            ]}
            disabled={$moduleStore.busy}
            onSelect={(value: string) => controller.choose(conflict, value)}
        />
    {/each}
    <button
        type="button"
        class="ui-choice-field ui-pressable"
        aria-expanded={showComponents}
        onclick={() => (showComponents = !showComponents)}
    >
        <span class="ui-press-visual"
            ><span>{$tr('importSetup.moduleShowItems')}</span><span class="ui-choice-value"
            ></span><ChevronDown aria-hidden="true" /></span
        >
    </button>
    {#if showComponents}
        <ul class="module-components">
            {#each $moduleStore.page.components as item (contentModuleComponentKey(item.component))}
                <li>{item.component.id}</li>
            {/each}
        </ul>
        <div class="module-pages">
            <button
                type="button"
                class="module-page-button"
                disabled={$moduleStore.busy || $moduleStore.page.offset === 0}
                onclick={() =>
                    void controller.review(Math.max(0, ($moduleStore.page?.offset ?? 0) - 64))}
                >{$tr('importSetup.previousItems')}</button
            >
            <button
                type="button"
                class="module-page-button"
                disabled={$moduleStore.busy || $moduleStore.page.next_offset === null}
                onclick={() => {
                    if ($moduleStore.page?.next_offset != null)
                        void controller.review($moduleStore.page.next_offset);
                }}>{$tr('importSetup.nextItems')}</button
            >
        </div>
    {/if}
    {#if $moduleStore.plan}
        <p>
            {$tr('importSetup.modulePlanCount', {
                count: $moduleStore.plan.component_count,
                omitted: $moduleStore.plan.omitted_component_count,
            })}
        </p>
        <DataAction disabled={$moduleStore.busy} onclick={() => void apply()}
            >{$tr('workspaceData.approveActivation')}</DataAction
        >
    {:else}
        <DataAction
            disabled={$moduleStore.busy || missingChoice}
            onclick={() => void controller.resolve()}>{$tr('workspaceData.resolve')}</DataAction
        >
    {/if}
{:else}
    <DataAction
        disabled={$moduleStore.busy ||
            !activation.candidate.local_use_allowed ||
            (activation.candidate.source_kind === 'imported_package' &&
                !activation.request.binding.package_import_approval_id)}
        onclick={() => void controller.review()}
        >{$tr('content.module.activation_review')}</DataAction
    >
{/if}
{#if $moduleStore.busy}<p role="status">{$tr('importSetup.moduleChecking')}</p>{/if}
{#if $moduleStore.error}
    <p role="alert">{$moduleStore.error}</p>
    <DataAction disabled={$moduleStore.busy} onclick={() => void controller.review()}
        >{$tr('content.module.activation_review')}</DataAction
    >
{/if}

<style>
    .module-components {
        margin: 0;
        padding: 0 16px;
        list-style: none;
    }
    .module-components li {
        padding: 8px 0;
        overflow-wrap: anywhere;
        font-size: 0.875rem;
    }
    .module-page-button {
        min-height: 44px;
        padding: 8px 16px;
        border: 0;
        background: transparent;
        color: var(--ui-text);
        font: inherit;
    }
    .module-page-button:disabled {
        opacity: 0.4;
    }
    .module-pages {
        display: flex;
        justify-content: space-between;
        gap: 8px;
    }
</style>
