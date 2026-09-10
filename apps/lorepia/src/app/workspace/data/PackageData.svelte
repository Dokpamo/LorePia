<script lang="ts">
    import { onMount } from 'svelte';
    import { tr } from '../../../lib/i18n';
    import type { SettingsServices } from '../../../features/providers/settings/settings-services';
    import { canApprovePackage, updateConfirmed, needsApproval } from './package-review';
    import SettingsRow from '../../../ui/workspace/SettingsRow.svelte';
    import DataAction from './DataAction.svelte';
    import DataChoice from './DataChoice.svelte';
    let { services }: { services: SettingsServices } = $props();
    const state = $derived(services.contentPackageState);
    const controller = $derived(services.contentPackageController);
    const busy = $derived(
        ['listing', 'picking', 'resuming', 'selecting', 'approving', 'committing'].includes(
            state.phase,
        ),
    );
    const options = $derived([
        { value: 'true', label: $tr('settingsUi.enabled') },
        { value: 'false', label: $tr('settingsUi.disabled') },
    ]);
    onMount(() => {
        void controller.loadPendingImports();
    });
    export function isBusy() {
        return busy;
    }
</script>

<section class="ui-settings-group">
    {#if !state.inspection || state.result}
        <DataAction
            disabled={busy || state.phase === 'unavailable'}
            onclick={() => void controller.pickAndInspect()}
            >{$tr('workspaceData.pickPackage')}</DataAction
        >
        {#each state.pending_imports as item (item.import_id)}<SettingsRow
                label={item.package_id}
                disabled={busy}
                onclick={() => void controller.resume(item.import_id)}
            />{/each}
    {/if}
    {#if state.inspection && !state.result}
        {@const review = state.inspection}
        <h2>{review.manifest.name} · {review.manifest.version}</h2>
        <p>
            {review.manifest.author ?? $tr('orchestration.package.author.unknown')} · {review
                .manifest.license} · {review.redistribution_status}
        </p>
        <p>{review.review_sha256}</p>
        {#each review.issues as issue, index (index)}<p
                role={issue.severity === 'blocker' ? 'alert' : undefined}
            >
                {issue.severity} · {issue.code}
            </p>{/each}
        {#each review.capability_decisions as decision (decision.capability)}<p>
                {decision.capability} · {decision.support} · {decision.reason}
            </p>{/each}
        {#each review.components as component (component.id)}
            <DataChoice
                label={component.id + ' · ' + component.kind}
                value={String(state.selected_component_ids.includes(component.id))}
                {options}
                disabled={state.phase !== 'ready' || component.disposition !== 'importable'}
                onSelect={(value: string) => {
                    if ((value === 'true') !== state.selected_component_ids.includes(component.id))
                        controller.toggleComponent(component.id);
                }}
            />
            <p>
                {component.disposition} · {component.required_capabilities.join(', ')} · {component.dependency_ids.join(
                    ', ',
                )}
            </p>
        {/each}
        {#if state.selection}
            {#each state.selection.normalization_evidence as item, index (index)}<p>
                    {item.component_id} · {item.field}: {String(item.before)} → {String(item.after)} ·
                    {item.reason}
                </p>{/each}
            {#each state.selection.target_review.documents as document (`${document.source_component_id}:${String(document.component_document_ordinal)}`)}
                <p>
                    {document.document_kind} · {document.target_object_id} · {document.disposition} ·
                    {document.expected_target_revision_id ?? $tr('settingsUi.none')} · {document.expected_target_state_revision ??
                        $tr('settingsUi.none')}
                </p>
                <p>{document.document_sha256}</p>
                {#if document.disposition === 'update'}<DataChoice
                        label={$tr('orchestration.package.update_confirm', {
                            id: document.target_object_id,
                        })}
                        value={String(updateConfirmed(state, document))}
                        {options}
                        disabled={state.phase !== 'selection_ready'}
                        onSelect={(value: string) => {
                            if ((value === 'true') !== updateConfirmed(state, document))
                                controller.toggleUpdateTargetConfirmation(
                                    document.source_component_id,
                                    document.component_document_ordinal,
                                );
                        }}
                    />{/if}
            {/each}
            {#each state.selected_component_ids as componentId (componentId)}<DataChoice
                    label={componentId + ' · ' + $tr('workspaceData.runtimeEnable')}
                    value={String(state.enabled_component_ids.includes(componentId))}
                    {options}
                    disabled={state.phase !== 'selection_ready'}
                    onSelect={(value: string) => {
                        if (
                            (value === 'true') !==
                            state.enabled_component_ids.includes(componentId)
                        )
                            controller.toggleEnabledComponent(componentId);
                    }}
                />{/each}
            {#each state.required_capabilities.filter(needsApproval) as capability (capability)}<DataChoice
                    label={$tr('workspaceData.approveCapability', { capability })}
                    value={String(state.approved_capabilities.includes(capability))}
                    {options}
                    disabled={state.phase !== 'selection_ready'}
                    onSelect={(value: string) => {
                        if ((value === 'true') !== state.approved_capabilities.includes(capability))
                            controller.toggleApprovedCapability(capability);
                    }}
                />{/each}
        {/if}
        {#if state.phase === 'selection_ready'}<DataAction
                disabled={!canApprovePackage(state)}
                onclick={() => void controller.approve()}
                >{$tr('workspaceData.approvePackage')}</DataAction
            >
        {:else if state.phase === 'approved'}<DataAction
                disabled={busy}
                onclick={() => void controller.commit()}
                >{$tr('workspaceData.commitPackage')}</DataAction
            >
        {:else}<DataAction
                disabled={state.phase !== 'ready' ||
                    !state.selected_component_ids.length ||
                    !review.local_import_allowed ||
                    review.issues.some((item) => item.severity === 'blocker')}
                onclick={() => void controller.reviewSelection()}
                >{$tr('workspaceData.reviewPackage')}</DataAction
            >{/if}
        <DataAction disabled={busy} onclick={() => void controller.discard()}
            >{$tr('workspaceData.discardPackage')}</DataAction
        >
    {/if}
    {#if state.error}<p role="alert">{state.error}</p>{/if}
    {#if state.announcement}<p role="status">{state.announcement}</p>{/if}
</section>
