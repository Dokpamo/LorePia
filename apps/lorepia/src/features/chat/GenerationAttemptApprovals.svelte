<script lang="ts">
    import { t, tr } from '../../lib/i18n';
    import { onMount, untrack } from 'svelte';

    import {
        GenerationAttemptApprovalController,
        INITIAL_GENERATION_ATTEMPT_APPROVAL_STATE,
        type GenerationAttemptApprovalCapableClient,
        type GenerationAttemptApprovalState,
    } from './generation-attempt-approval-controller';

    interface Props {
        client: GenerationAttemptApprovalCapableClient;
        controller?: GenerationAttemptApprovalController;
        conversationId: string | null;
        sourceBranchId: string | null;
        headingId?: string;
        refreshEpoch?: number;
        onRetry?: (generationId: string) => void | Promise<void>;
        retryLabel?: string;
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

<section class="attempt-approvals" aria-labelledby={headingId}>
    <header>
        <div>
            <h3 id={headingId}>{$tr('attempt_approval.title')}</h3>
            <p>
                {$tr('attempt_approval.hint')}
            </p>
        </div>
        <button
            type="button"
            disabled={busy || conversationId === null || sourceBranchId === null}
            onclick={() => void approvalController.reload()}
        >
            {$tr('attempt_approval.reload')}
        </button>
    </header>

    <p class="sr-only" aria-live="polite">{approvalState.announcement}</p>

    {#if conversationId === null || sourceBranchId === null}
        <p class="attempt-note" role="note">
            {$tr('attempt_approval.pick_room')}
        </p>
    {:else if approvalState.phase === 'loading'}
        <p role="status">{$tr('attempt_approval.loading')}</p>
    {:else if approvalState.phase === 'unavailable'}
        <p class="attempt-note" role="note">{approvalState.error}</p>
    {:else if approvalState.error !== null}
        <div class="attempt-error" role="alert">
            <p>{approvalState.error}</p>
            <button type="button" disabled={busy} onclick={() => void approvalController.reload()}>
                {$tr('attempt_approval.reload_latest')}
            </button>
        </div>
    {/if}

    {#if approvalState.has_more_due}
        <p class="attempt-note" role="note">
            {$tr('attempt_approval.too_many')}
        </p>
    {/if}

    {#if approvalState.proposals.length > 0}
        <ol class="proposal-list" aria-label={$tr('attempt_approval.list.label')}>
            {#each approvalState.proposals as item, index (itemKey(item.generation_id, item.proposal.id))}
                {@const key = itemKey(item.generation_id, item.proposal.id)}
                {@const summaryId = `${headingId}-proposal-${String(index)}`}
                <li>
                    <article
                        aria-labelledby={`${summaryId}-title`}
                        aria-describedby={`${summaryId}-body ${summaryId}-authority`}
                    >
                        <div class="proposal-copy">
                            {#if item.proposal.projection_rejection_reason === 'unsafe_native_text'}
                                <h4 id={`${summaryId}-title`}>
                                    {$tr('attempt_approval.unrenderable.title')}
                                </h4>
                                <p id={`${summaryId}-body`}>
                                    {$tr('attempt_approval.unrenderable.hint')}
                                </p>
                            {:else}
                                <h4 id={`${summaryId}-title`}>{item.proposal.title}</h4>
                                <p id={`${summaryId}-body`}>{item.proposal.body}</p>
                            {/if}
                        </div>
                        <dl id={`${summaryId}-authority`}>
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
                        <div class="proposal-actions">
                            <button
                                class="approve"
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
                            >
                                {approvalState.busy_proposal_key === key
                                    ? $tr('attempt_approval.busy')
                                    : $tr('attempt_approval.approve')}
                            </button>
                            <button
                                class="reject"
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
                            >
                                {$tr('attempt_approval.reject')}
                            </button>
                        </div>
                    </article>
                </li>
            {/each}
        </ol>
    {:else if approvalState.phase === 'ready' && approvalState.retry_generation_ids.length === 0}
        <p class="attempt-note">{$tr('attempt_approval.empty')}</p>
    {/if}

    {#if approvalState.retry_available && approvalState.retry_generation_ids.length > 0}
        <div class="retry-generation">
            <p role="status">{approvalState.announcement}</p>
            {#if onRetry !== undefined}
                <ol class="retry-list" aria-label={$tr('attempt_approval.retry_list.label')}>
                    {#each approvalState.retry_generation_ids as generationId (generationId)}
                        <li class="retry-item">
                            <span
                                >{$tr('attempt_approval.retry_item')}
                                <code>{generationId}</code></span
                            >
                            <button
                                type="button"
                                disabled={busy}
                                aria-label={$tr('attempt_approval.retry_item.label', {
                                    label: retryLabel,
                                    id: generationId,
                                })}
                                onclick={() => void onRetry(generationId)}
                            >
                                {retryLabel}
                            </button>
                        </li>
                    {/each}
                </ol>
            {/if}
        </div>
    {/if}
</section>

<style>
    .attempt-approvals {
        display: grid;
        gap: 12px;
        padding: 14px;
        border: 1px solid var(--line);
        border-radius: 16px;
        background: var(--surface-sunken);
    }

    header,
    article,
    .attempt-error,
    .retry-generation,
    .proposal-actions {
        display: flex;
        gap: 12px;
        align-items: center;
        justify-content: space-between;
    }

    header > div,
    .proposal-copy {
        display: grid;
        gap: 4px;
    }

    h3,
    h4,
    p,
    dl,
    dd {
        margin: 0;
    }

    .proposal-list {
        display: grid;
        gap: 10px;
        padding: 0;
        margin: 0;
        list-style: none;
    }

    article {
        align-items: stretch;
        padding: 12px;
        border: 1px solid var(--line);
        border-radius: 12px;
        background: var(--surface);
    }

    .proposal-copy {
        flex: 1 1 260px;
    }

    dl {
        display: grid;
        grid-template-columns: repeat(2, minmax(0, 1fr));
        gap: 6px 12px;
        flex: 1 1 300px;
        font-size: 0.85rem;
    }

    dl div {
        min-width: 0;
    }

    dt {
        color: var(--ink-muted);
    }

    dd,
    code {
        overflow-wrap: anywhere;
    }

    .proposal-actions {
        justify-content: flex-end;
    }

    .approve {
        color: var(--primary-ink);
        background: var(--primary-bg);
    }

    .reject,
    .attempt-error {
        color: var(--danger);
    }

    .attempt-note {
        color: var(--ink-muted);
    }

    .retry-generation {
        display: grid;
        gap: 10px;
        padding-top: 10px;
        border-top: 1px solid var(--line);
    }

    .retry-list {
        display: grid;
        gap: 8px;
        padding: 0;
        margin: 0;
        list-style: none;
    }

    .retry-item {
        display: flex;
        gap: 12px;
        align-items: center;
        justify-content: space-between;
    }

    .sr-only {
        position: absolute;
        width: 1px;
        height: 1px;
        padding: 0;
        margin: -1px;
        overflow: hidden;
        clip: rect(0, 0, 0, 0);
        white-space: nowrap;
        border: 0;
    }

    @container view (max-width: 760px) {
        header,
        article,
        .attempt-error,
        .retry-item {
            align-items: stretch;
            flex-direction: column;
        }

        dl {
            grid-template-columns: 1fr;
        }
    }
</style>
