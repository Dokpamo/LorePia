<script lang="ts">
    import './runtime.css';
    import { t, tr } from '../../../lib/i18n';
    import { onMount, untrack } from 'svelte';

    import {
        GenerationAttemptApprovalController,
        INITIAL_GENERATION_ATTEMPT_APPROVAL_STATE,
        type GenerationAttemptApprovalCapableClient,
        type GenerationAttemptApprovalState,
    } from '../../../features/chat/generation-attempt-approval-controller';

    interface Props {
        client: GenerationAttemptApprovalCapableClient;
        controller?: GenerationAttemptApprovalController;
        conversationId: string | null;
        sourceBranchId: string | null;
        headingId?: string;
        refreshEpoch?: number;
        onRetry?: (generationId: string) => void | Promise<void>;
        retryLabel?: string;
        hideWhenInactive?: boolean;
    }

    let {
        client,
        controller: providedController,
        conversationId,
        sourceBranchId,
        headingId = 'generation-attempt-approvals-title',
        refreshEpoch = 0,
        onRetry,
        retryLabel = t('attempt_approval.retry_label'),
        hideWhenInactive = false,
    }: Props = $props();
    const ownsController = untrack(() => providedController === undefined);
    const approvalController = untrack(
        () => providedController ?? new GenerationAttemptApprovalController(client),
    );
    let approvalState = $state<GenerationAttemptApprovalState>(
        structuredClone(INITIAL_GENERATION_ATTEMPT_APPROVAL_STATE),
    );
    let contextKey = '';

    const busy = $derived(
        approvalState.phase === 'loading' || approvalState.busy_proposal_key !== null,
    );
    const hidden = $derived(
        hideWhenInactive &&
            (approvalState.phase === 'idle' ||
                approvalState.phase === 'unavailable' ||
                ((approvalState.phase === 'ready' || approvalState.phase === 'loading') &&
                    approvalState.error === null &&
                    !approvalState.has_more_due &&
                    approvalState.proposals.length === 0 &&
                    approvalState.retry_generation_ids.length === 0)),
    );

    $effect(() => {
        const nextKey =
            conversationId !== null && sourceBranchId !== null
                ? JSON.stringify([conversationId, sourceBranchId, refreshEpoch])
                : '';
        if (nextKey === contextKey) return;
        contextKey = nextKey;
        void approvalController.loadRoom(conversationId, sourceBranchId);
    });

    onMount(() => {
        const unsubscribe = approvalController.state.subscribe((value) => {
            approvalState = value;
        });
        return () => {
            unsubscribe();
            if (ownsController) approvalController.destroy();
        };
    });

    function itemKey(generationId: string, proposalId: string): string {
        return JSON.stringify([generationId, proposalId]);
    }

    function shortId(value: string): string {
        return value.length <= 24 ? value : `${value.slice(0, 12)}…${value.slice(-8)}`;
    }

    function expiryLabel(epochSeconds: number | null): string {
        if (epochSeconds === null) return t('attempt_approval.no_expiry');
        return t('attempt_approval.expires_at', {
            time: new Date(epochSeconds * 1_000).toLocaleString(),
        });
    }
</script>

{#if !hidden}
    <section class="ui-settings-group" aria-labelledby={headingId}>
        <header>
            <div>
                <h3 class="ui-settings-heading" id={headingId}>{$tr('attempt_approval.title')}</h3>
                <p>
                    {$tr('attempt_approval.hint')}
                </p>
            </div>
            <button
                class="ui-submit ui-pressable"
                type="button"
                disabled={busy || conversationId === null || sourceBranchId === null}
                onclick={() => void approvalController.reload()}
                ><span class="ui-press-visual">
                    {$tr('attempt_approval.reload')}
                </span></button
            >
        </header>

        <p class="ui-sr" aria-live="polite">{approvalState.announcement}</p>

        {#if conversationId === null || sourceBranchId === null}
            <p role="note">
                {$tr('attempt_approval.pick_room')}
            </p>
        {:else if approvalState.phase === 'loading'}
            <p role="status">{$tr('attempt_approval.loading')}</p>
        {:else if approvalState.phase === 'unavailable'}
            <p role="note">{approvalState.error}</p>
        {:else if approvalState.error !== null}
            <div role="alert">
                <p>{approvalState.error}</p>
                <button
                    class="ui-submit ui-pressable"
                    type="button"
                    disabled={busy}
                    onclick={() => void approvalController.reload()}
                    ><span class="ui-press-visual">
                        {$tr('attempt_approval.reload_latest')}
                    </span></button
                >
            </div>
        {/if}

        {#if approvalState.has_more_due}
            <p role="note">
                {$tr('attempt_approval.too_many')}
            </p>
        {/if}

        {#if approvalState.proposals.length > 0}
            <ol class="ui-runtime-list" aria-label={$tr('attempt_approval.list.label')}>
                {#each approvalState.proposals as item, index (itemKey(item.generation_id, item.proposal.id))}
                    {@const key = itemKey(item.generation_id, item.proposal.id)}
                    {@const summaryId = `${headingId}-proposal-${String(index)}`}
                    <li>
                        <article
                            class="ui-runtime-item"
                            aria-labelledby={`${summaryId}-title`}
                            aria-describedby={`${summaryId}-body ${summaryId}-authority`}
                        >
                            <div>
                                {#if item.proposal.projection_rejection_reason === 'unsafe_native_text'}
                                    <h4 class="ui-settings-heading" id={`${summaryId}-title`}>
                                        {$tr('attempt_approval.unrenderable.title')}
                                    </h4>
                                    <p id={`${summaryId}-body`}>
                                        {$tr('attempt_approval.unrenderable.hint')}
                                    </p>
                                {:else}
                                    <h4 class="ui-settings-heading" id={`${summaryId}-title`}>
                                        {item.proposal.title}
                                    </h4>
                                    <p id={`${summaryId}-body`}>{item.proposal.body}</p>
                                {/if}
                            </div>
                            <dl class="ui-card-details" id={`${summaryId}-authority`}>
                                <div>
                                    <dt>{$tr('attempt_approval.field.attempt')}</dt>
                                    <dd><code>{shortId(item.generation_id)}</code></dd>
                                </div>
                                <div>
                                    <dt>{$tr('attempt_approval.field.branch')}</dt>
                                    <dd><code>{shortId(item.proposed_branch_id)}</code></dd>
                                </div>
                                <div>
                                    <dt>{$tr('attempt_approval.field.pending')}</dt>
                                    <dd>
                                        {$tr('attempt_approval.field.pending_count', {
                                            count: item.pending_proposal_count,
                                        })}
                                    </dd>
                                </div>
                                <div>
                                    <dt>{$tr('attempt_approval.field.expiry')}</dt>
                                    <dd>{expiryLabel(item.proposal.expires_at_epoch_seconds)}</dd>
                                </div>
                            </dl>
                            <div>
                                <button
                                    class="ui-submit ui-pressable"
                                    type="button"
                                    disabled={busy ||
                                        approvalState.has_more_due ||
                                        approvalState.error !== null ||
                                        item.proposal.projection_rejection_reason ===
                                            'unsafe_native_text'}
                                    aria-label={$tr('attempt_approval.approve.label', {
                                        index: index + 1,
                                    })}
                                    aria-describedby={`${summaryId}-body ${summaryId}-authority`}
                                    onclick={() =>
                                        void approvalController.decideProposal(
                                            item.generation_id,
                                            item.proposal.id,
                                            'approve',
                                        )}
                                    ><span class="ui-press-visual">
                                        {approvalState.busy_proposal_key === key
                                            ? $tr('attempt_approval.busy')
                                            : $tr('attempt_approval.approve')}
                                    </span></button
                                >
                                <button
                                    class="ui-submit ui-pressable"
                                    type="button"
                                    disabled={busy ||
                                        approvalState.has_more_due ||
                                        approvalState.error !== null}
                                    aria-label={$tr('attempt_approval.reject.label', {
                                        index: index + 1,
                                    })}
                                    aria-describedby={`${summaryId}-body ${summaryId}-authority`}
                                    onclick={() =>
                                        void approvalController.decideProposal(
                                            item.generation_id,
                                            item.proposal.id,
                                            'reject',
                                        )}
                                    ><span class="ui-press-visual">
                                        {$tr('attempt_approval.reject')}
                                    </span></button
                                >
                            </div>
                        </article>
                    </li>
                {/each}
            </ol>
        {:else if approvalState.phase === 'ready' && approvalState.retry_generation_ids.length === 0}
            <p>{$tr('attempt_approval.empty')}</p>
        {/if}

        {#if approvalState.retry_available && approvalState.retry_generation_ids.length > 0}
            <div>
                <p role="status">{approvalState.announcement}</p>
                {#if onRetry !== undefined}
                    <ol
                        class="ui-runtime-list"
                        aria-label={$tr('attempt_approval.retry_list.label')}
                    >
                        {#each approvalState.retry_generation_ids as generationId (generationId)}
                            <li>
                                <span
                                    >{$tr('attempt_approval.retry_item')}
                                    <code>{generationId}</code></span
                                >
                                <button
                                    class="ui-submit ui-pressable"
                                    type="button"
                                    disabled={busy}
                                    aria-label={$tr('attempt_approval.retry_item.label', {
                                        label: retryLabel,
                                        id: generationId,
                                    })}
                                    onclick={() => void onRetry(generationId)}
                                    ><span class="ui-press-visual">
                                        {retryLabel}
                                    </span></button
                                >
                            </li>
                        {/each}
                    </ol>
                {/if}
            </div>
        {/if}
    </section>
{/if}
