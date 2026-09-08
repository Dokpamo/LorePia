<script lang="ts">
    import { tr } from '../../../lib/i18n';
    import type {
        ContentModuleLifecycleState,
        ContentModuleLifecycleController,
    } from '../../../features/orchestration/module-lifecycle-controller';
    import {
        contentModuleCandidateKey,
        contentModuleComponentKey,
    } from '../../../features/orchestration/module-lifecycle-controller';
    import DataAction from './DataAction.svelte';
    import DataChoice from './DataChoice.svelte';
    let {
        controller,
        lifecycle,
        busy,
    }: {
        controller: ContentModuleLifecycleController;
        lifecycle: ContentModuleLifecycleState;
        busy: boolean;
    } = $props();
    let targets = $state<Record<string, string>>({});
    let approvals = $state<Record<string, string>>({});
</script>

<section class="ui-settings-group">
    <h2>{$tr('workspaceData.rollback')}</h2>
    {#if lifecycle.rollback}
        {@const rollback = lifecycle.rollback}
        {#if rollback.review}
            <p>
                {rollback.review.target_revision.name} · {rollback.review.target_revision.version}
            </p>
            <p>
                {rollback.review.review.rollback.current_revision_id} → {rollback.review.review
                    .rollback.target_revision_id}
            </p>
            <p>{rollback.review.review.rollback.target_source_sha256}</p>
            {#each rollback.review.review.rollback.blockers as blocker, index (index)}<p
                    role="alert"
                >
                    {blocker.kind}
                </p>{/each}
            {#each rollback.review.review.activation.conflicts as conflict (contentModuleComponentKey(conflict.component))}<DataChoice
                    label={conflict.component.id + ' · ' + conflict.reason}
                    value={rollback.conflict_choices[
                        contentModuleComponentKey(conflict.component)
                    ] ?? ''}
                    options={[
                        { value: '', label: $tr('settingsUi.none') },
                        ...conflict.candidates.map((item) => ({
                            value: contentModuleCandidateKey(item),
                            label:
                                item.module_id +
                                ' · ' +
                                item.revision_id +
                                ' · ' +
                                item.component_hash,
                        })),
                    ]}
                    disabled={busy}
                    onSelect={(value: string) =>
                        controller.chooseRollbackConflict(conflict.component, value)}
                />{/each}
            {#if rollback.plan}
                {#each rollback.plan.activation.components as component (contentModuleComponentKey(component.component))}<p
                    >
                        {component.component.id} · {component.sha256} · {$tr(
                            component.runtime_enabled
                                ? 'settingsUi.enabled'
                                : 'settingsUi.disabled',
                        )}
                    </p>{/each}
                <p>{rollback.plan.rollback.plan_sha256}</p>
                <DataAction
                    disabled={busy || !rollback.review.review.rollback.eligible}
                    onclick={() => void controller.applyReviewedRollback()}
                    >{$tr('workspaceData.approveRollback')}</DataAction
                >
            {:else}<DataAction
                    disabled={busy ||
                        !rollback.review.review.rollback.eligible ||
                        rollback.review.review.activation.conflicts.some(
                            (item) =>
                                !rollback.conflict_choices[
                                    contentModuleComponentKey(item.component)
                                ],
                        )}
                    onclick={() => void controller.resolveRollback()}
                    >{$tr('workspaceData.resolve')}</DataAction
                >{/if}
        {/if}
        <DataAction disabled={busy} onclick={() => controller.clearReview()}
            >{$tr('settingsLive.cancel')}</DataAction
        >
    {:else}
        {#each lifecycle.bindings as item (item.binding.binding.id)}
            {@const id = item.binding.binding.id}
            {@const revision = item.revisions.find(
                (revision) => revision.revision_id === targets[id],
            )}
            <DataChoice
                label={item.module_name}
                value={targets[id] ?? ''}
                options={[
                    { value: '', label: $tr('settingsUi.none') },
                    ...item.revisions
                        .filter((revision) => revision.rollback_allowed)
                        .map((revision) => ({
                            value: revision.revision_id,
                            label:
                                revision.name +
                                ' · ' +
                                revision.version +
                                ' · ' +
                                revision.revision_id,
                        })),
                ]}
                disabled={busy}
                onSelect={(value: string) => {
                    targets[id] = value;
                    approvals[id] = '';
                }}
            />
            {#if revision?.source_kind === 'imported_package'}<DataChoice
                    label={$tr('workspaceData.importApproval')}
                    value={approvals[id] ?? ''}
                    options={[
                        { value: '', label: $tr('settingsUi.none') },
                        ...revision.completed_package_approvals.map((approval) => ({
                            value: approval.approval_id,
                            label: approval.package_id + ' · ' + approval.approval_sha256,
                        })),
                    ]}
                    disabled={busy}
                    onSelect={(value: string) => (approvals[id] = value)}
                />{/if}
            <DataAction
                disabled={busy ||
                    !revision ||
                    !revision.rollback_allowed ||
                    (revision.source_kind === 'imported_package' &&
                        !revision.completed_package_approvals.some(
                            (approval) => approval.approval_id === approvals[id],
                        ))}
                onclick={() => {
                    if (revision)
                        void controller.reviewRollback(
                            id,
                            revision.revision_id,
                            revision.source_kind === 'imported_package'
                                ? (approvals[id] ?? null)
                                : null,
                        );
                }}
                >{$tr('content.module.rollback_review', {
                    name: item.module_name,
                    revision: revision?.revision_id ?? '',
                })}</DataAction
            >
        {/each}
    {/if}
</section>
