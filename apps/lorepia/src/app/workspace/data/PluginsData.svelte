<script lang="ts">
    import SettingsRow from '../../../ui/workspace/SettingsRow.svelte';
    import { onMount, untrack } from 'svelte';
    import { tr } from '../../../lib/i18n';
    import type { SettingsServices } from '../../../features/providers/settings/settings-services';
    import {
        ContentModuleLifecycleController,
        contentModuleCandidateKey,
        contentModuleComponentKey,
    } from '../../../features/orchestration/module-lifecycle-controller';
    import RollbackData from './RollbackData.svelte';
    import DataAction from './DataAction.svelte';
    import DataChoice from './DataChoice.svelte';
    import DataNumber from './DataNumber.svelte';
    let { services }: { services: SettingsServices } = $props();
    const controller = untrack(() => new ContentModuleLifecycleController(services.client));
    const controllerStore = controller.state;
    const busy = $derived(
        ['loading', 'reviewing', 'resolving', 'applying'].includes($controllerStore.phase),
    );
    onMount(() => {
        void controller.loadContext(
            services.appState.selected_conversation?.id ?? null,
            services.appState.conversation_state?.active_branch_id ?? null,
        );
        return () => controller.destroy();
    });
    export function isBusy() {
        return busy;
    }
    export function isDirty() {
        return (
            $controllerStore.activation !== null ||
            $controllerStore.rollback !== null ||
            $controllerStore.deactivation !== null
        );
    }
</script>

<section class="ui-settings-group">
    <p>{$tr('settingsLive.pluginsIntro')}</p>
    {#if $controllerStore.activation}
        {@const activation = $controllerStore.activation}
        <h2>{activation.candidate.name}</h2>
        <p>{activation.candidate.version} · {activation.candidate.license}</p>
        <p>{activation.candidate.required_capabilities.join(', ')}</p>
        <DataChoice
            label={$tr('workspaceData.scope')}
            value={activation.request.binding.scope}
            options={$controllerStore.scope_targets.map((item) => ({
                value: item.scope,
                label: item.label,
            }))}
            disabled={busy}
            onSelect={(value: string) => {
                const scope = $controllerStore.scope_targets.find((item) => item.scope === value);
                if (scope) controller.setActivationScope(scope.scope);
            }}
        />
        <DataNumber
            label={$tr('workspaceData.priority')}
            value={activation.request.binding.priority}
            min={-2147483648}
            max={2147483647}
            disabled={busy}
            onchange={(value: number) => controller.setActivationPriority(value)}
        />
        {#if activation.candidate.source_kind === 'imported_package'}<DataChoice
                label={$tr('workspaceData.importApproval')}
                value={activation.request.binding.package_import_approval_id ?? ''}
                options={[
                    { value: '', label: $tr('settingsUi.none') },
                    ...activation.candidate.completed_package_approvals.map((item) => ({
                        value: item.approval_id,
                        label: item.package_id + ' · ' + item.approval_sha256,
                    })),
                ]}
                disabled={busy}
                onSelect={(value: string) =>
                    controller.selectCompletedPackageApproval(value === '' ? null : value)}
            />{/if}
        {#if activation.review}
            {#each activation.review.review.conflicts as conflict (contentModuleComponentKey(conflict.component))}<DataChoice
                    label={conflict.component.id + ' · ' + conflict.reason}
                    value={activation.conflict_choices[
                        contentModuleComponentKey(conflict.component)
                    ] ?? ''}
                    options={[
                        { value: '', label: $tr('settingsUi.none') },
                        ...conflict.candidates.map((item) => ({
                            value: contentModuleCandidateKey(item),
                            label: item.module_id + ' · ' + item.revision_id,
                        })),
                    ]}
                    disabled={busy}
                    onSelect={(value: string) =>
                        controller.chooseActivationConflict(conflict.component, value)}
                />{/each}
            {#if activation.plan}
                {#each activation.plan.components as component (contentModuleComponentKey(component.component))}<p
                    >
                        {component.component.id} · {component.sha256} · {$tr(
                            component.runtime_enabled
                                ? 'settingsUi.enabled'
                                : 'settingsUi.disabled',
                        )}
                    </p>{/each}
                <DataAction disabled={busy} onclick={() => void controller.activateReviewedPlan()}
                    >{$tr('workspaceData.approveActivation')}</DataAction
                >
            {:else}<DataAction disabled={busy} onclick={() => void controller.resolveActivation()}
                    >{$tr('workspaceData.resolve')}</DataAction
                >{/if}
        {:else}<DataAction
                disabled={busy || !activation.candidate.local_use_allowed}
                onclick={() => void controller.reviewActivation()}
                >{$tr('content.module.activation_review')}</DataAction
            >{/if}
        <DataAction disabled={busy} onclick={() => controller.clearReview()}
            >{$tr('settingsLive.cancel')}</DataAction
        >
    {:else}
        {#each $controllerStore.candidates as item (item.module_id)}<SettingsRow
                label={item.name}
                value={item.version}
                disabled={busy || !item.local_use_allowed}
                onclick={() => controller.beginActivation(item.module_id)}
            />{/each}
        {#each $controllerStore.bindings as item (item.binding.binding.id)}<DataAction
                disabled={busy}
                onclick={() => void controller.reviewDeactivation(item.binding.binding.id)}
                >{$tr('content.module.binding_deactivation_review', {
                    name: item.module_name,
                })}</DataAction
            >{/each}
    {/if}
    {#if $controllerStore.deactivation?.review}<p>
            {$controllerStore.deactivation.binding.module_name}
        </p>
        <DataAction disabled={busy} onclick={() => void controller.deactivateReviewedBinding()}
            >{$tr('workspaceData.deactivate')}</DataAction
        ><DataAction disabled={busy} onclick={() => controller.clearReview()}
            >{$tr('settingsLive.cancel')}</DataAction
        >{/if}
    <RollbackData {controller} lifecycle={$controllerStore} {busy} />
    {#if $controllerStore.error}<p role="alert">{$controllerStore.error}</p>{/if}
    {#if $controllerStore.announcement}<p role="status">{$controllerStore.announcement}</p>{/if}
</section>
