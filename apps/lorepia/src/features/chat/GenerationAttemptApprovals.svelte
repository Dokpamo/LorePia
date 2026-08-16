<script lang="ts">
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
        retryLabel = '원래 작업으로 돌아가기',
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
        if (epochSeconds === null) return '자동 만료 없음';
        return `${new Date(epochSeconds * 1_000).toLocaleString()} 만료`;
    }
</script>

<section class="attempt-approvals" aria-labelledby={headingId}>
    <header>
        <div>
            <h3 id={headingId}>생성 시도 승인</h3>
            <p>
                이 생성 시도에 고정된 제안만 승인하거나 거절합니다. 결정 뒤 생성은 자동으로 다시
                제출되지 않습니다.
            </p>
        </div>
        <button
            type="button"
            disabled={busy || conversationId === null || sourceBranchId === null}
            onclick={() => void approvalController.reload()}
        >
            승인 목록 다시 불러오기
        </button>
    </header>

    <p class="sr-only" aria-live="polite">{approvalState.announcement}</p>

    {#if conversationId === null || sourceBranchId === null}
        <p class="attempt-note" role="note">
            대화와 소스 브랜치를 선택하면 중단된 생성 시도의 승인 제안을 확인할 수 있습니다.
        </p>
    {:else if approvalState.phase === 'loading'}
        <p role="status">만료 제안을 정리한 뒤 승인 목록을 불러오는 중입니다.</p>
    {:else if approvalState.phase === 'unavailable'}
        <p class="attempt-note" role="note">{approvalState.error}</p>
    {:else if approvalState.error !== null}
        <div class="attempt-error" role="alert">
            <p>{approvalState.error}</p>
            <button type="button" disabled={busy} onclick={() => void approvalController.reload()}>
                최신 승인 목록 다시 불러오기
            </button>
        </div>
    {/if}

    {#if approvalState.has_more_due}
        <p class="attempt-note" role="note">
            한 번에 정리할 수 있는 100개보다 많은 만료 제안이 있습니다. 결정을 계속하기 전에 목록을
            다시 불러오세요.
        </p>
    {/if}

    {#if approvalState.proposals.length > 0}
        <ol class="proposal-list" aria-label="대기 중인 생성 시도 승인 제안">
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
                                <h4 id={`${summaryId}-title`}>저장 제안 내용을 표시할 수 없음</h4>
                                <p id={`${summaryId}-body`}>
                                    안전한 표시 범위를 벗어난 원문은 숨겼습니다. 이 제안은 거절만 할
                                    수 있습니다.
                                </p>
                            {:else}
                                <h4 id={`${summaryId}-title`}>{item.proposal.title}</h4>
                                <p id={`${summaryId}-body`}>{item.proposal.body}</p>
                            {/if}
                        </div>
                        <dl id={`${summaryId}-authority`}>
                            <div>
                                <dt>생성 시도</dt>
                                <dd><code>{shortId(item.generation_id)}</code></dd>
                            </div>
                            <div>
                                <dt>제안 브랜치</dt>
                                <dd><code>{shortId(item.proposed_branch_id)}</code></dd>
                            </div>
                            <div>
                                <dt>남은 승인</dt>
                                <dd>{item.pending_proposal_count}개</dd>
                            </div>
                            <div>
                                <dt>만료</dt>
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
                                aria-label={`제안 ${String(index + 1)} 승인`}
                                aria-describedby={`${summaryId}-body ${summaryId}-authority`}
                                onclick={() =>
                                    void approvalController.decideProposal(
                                        item.generation_id,
                                        item.proposal.id,
                                        'approve',
                                    )}
                            >
                                {approvalState.busy_proposal_key === key ? '반영 중…' : '승인'}
                            </button>
                            <button
                                class="reject"
                                type="button"
                                disabled={busy ||
                                    approvalState.has_more_due ||
                                    approvalState.error !== null}
                                aria-label={`제안 ${String(index + 1)} 거절`}
                                aria-describedby={`${summaryId}-body ${summaryId}-authority`}
                                onclick={() =>
                                    void approvalController.decideProposal(
                                        item.generation_id,
                                        item.proposal.id,
                                        'reject',
                                    )}
                            >
                                거절
                            </button>
                        </div>
                    </article>
                </li>
            {/each}
        </ol>
    {:else if approvalState.phase === 'ready' && approvalState.retry_generation_ids.length === 0}
        <p class="attempt-note">대기 중인 생성 시도 승인 제안이 없습니다.</p>
    {/if}

    {#if approvalState.retry_available && approvalState.retry_generation_ids.length > 0}
        <div class="retry-generation">
            <p role="status">{approvalState.announcement}</p>
            {#if onRetry !== undefined}
                <ol class="retry-list" aria-label="다시 시도할 재개 가능한 생성 시도">
                    {#each approvalState.retry_generation_ids as generationId (generationId)}
                        <li class="retry-item">
                            <span>생성 시도 <code>{generationId}</code></span>
                            <button
                                type="button"
                                disabled={busy}
                                aria-label={`${retryLabel}: 생성 시도 ${generationId}`}
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
        background: var(--surface-muted);
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
        color: var(--text-muted);
    }

    dd,
    code {
        overflow-wrap: anywhere;
    }

    .proposal-actions {
        justify-content: flex-end;
    }

    .approve {
        color: var(--accent-contrast);
        background: var(--accent);
    }

    .reject,
    .attempt-error {
        color: var(--danger);
    }

    .attempt-note {
        color: var(--text-muted);
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

    @media (max-width: 760px) {
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
