<script lang="ts">
    import { onMount, untrack } from 'svelte';

    import DetailActionBar from '../../components/detail/DetailActionBar.svelte';
    import type {
        LorepiaClient,
        PromptPresetHistoryClientApi,
        PromptPresetRollbackReceiptDto,
    } from '../../lib/ipc/contracts';
    import {
        INITIAL_PROMPT_PRESET_HISTORY_STATE,
        PromptPresetHistoryController,
        type PromptPresetHistoryState,
    } from './prompt-preset-history-controller';

    interface Props {
        client: LorepiaClient & Partial<PromptPresetHistoryClientApi>;
        presetId: string | null;
        currentRevision: number | null;
        disabled?: boolean;
        onApplied?: (receipt: PromptPresetRollbackReceiptDto) => void | Promise<void>;
        detailPage?: string | null;
    }

    const HISTORY_ROUTE = 'history';
    const REVIEW_ROUTE_PREFIX = 'history/review/';

    let {
        client,
        presetId,
        currentRevision,
        disabled = false,
        onApplied = () => undefined,
        detailPage = $bindable(HISTORY_ROUTE),
    }: Props = $props();
    const controller = untrack(() => new PromptPresetHistoryController(client));
    let historyState = $state<PromptPresetHistoryState>(
        structuredClone(INITIAL_PROMPT_PRESET_HISTORY_STATE),
    );
    let contextKey = '';

    const busy = $derived(
        historyState.phase === 'loading' ||
            historyState.phase === 'reviewing' ||
            historyState.phase === 'applying',
    );
    const builtIn = $derived(
        historyState.revisions.length > 0 &&
            historyState.revisions.every((revision) => !revision.rollback_allowed),
    );
    const reviewRouteRevision = $derived(revisionFromReviewRoute(detailPage));
    const showingReview = $derived(reviewRouteRevision !== null);

    $effect(() => {
        const nextKey = `${presetId ?? ''}:${String(currentRevision ?? '')}`;
        if (nextKey === contextKey) return;
        contextKey = nextKey;
        void controller.load(presetId, currentRevision);
    });

    onMount(() => {
        const unsubscribe = controller.state.subscribe((value) => {
            historyState = value;
        });
        return () => {
            unsubscribe();
            controller.destroy();
        };
    });

    function shortHash(value: string): string {
        return `${value.slice(0, 12)}…${value.slice(-8)}`;
    }

    function revisionFromReviewRoute(route: string | null): number | null {
        if (!route?.startsWith(REVIEW_ROUTE_PREFIX)) return null;
        const revision = Number(route.slice(REVIEW_ROUTE_PREFIX.length));
        return Number.isSafeInteger(revision) && revision > 0 ? revision : null;
    }

    function reviewRoute(revision: number): string {
        return `${REVIEW_ROUTE_PREFIX}${String(revision)}`;
    }

    function canReview(revision: PromptPresetHistoryState['revisions'][number]): boolean {
        return (
            revision.revision !== historyState.current_revision &&
            revision.rollback_allowed &&
            (historyState.current_revision === null ||
                revision.revision < historyState.current_revision)
        );
    }

    function reviewDisabled(revision: PromptPresetHistoryState['revisions'][number]): boolean {
        return disabled || busy || !canReview(revision);
    }

    function reviewTitle(
        revision: PromptPresetHistoryState['revisions'][number],
    ): string | undefined {
        if (revision.revision === historyState.current_revision) return '현재 리비전입니다.';
        if (!revision.rollback_allowed) return '앱 내장 프리셋은 롤백할 수 없습니다.';
        if (
            historyState.current_revision !== null &&
            revision.revision > historyState.current_revision
        ) {
            return '현재 상태보다 새로운 리비전입니다. 구성을 다시 불러오세요.';
        }
        return undefined;
    }

    async function openReview(revision: number): Promise<void> {
        const reviewed = await controller.reviewTarget(revision);
        if (reviewed) detailPage = reviewRoute(revision);
    }

    async function applyRollback(): Promise<void> {
        const receipt = await controller.applyReviewedRollback();
        if (receipt !== null) {
            detailPage = HISTORY_ROUTE;
            await onApplied(receipt);
        }
    }
</script>

<section class="preset-history has-fixed-actions" aria-label="프롬프트 프리셋 리비전">
    <p class="sr-only" aria-live="polite">{historyState.announcement}</p>

    {#if disabled}
        <p class="history-note" role="note">
            저장하지 않은 프롬프트 블록 변경이 있어 롤백을 잠갔습니다. 먼저 저장하거나 다시
            불러오세요.
        </p>
    {/if}
    {#if builtIn}
        <p class="history-note" role="note">
            앱 내장 프롬프트 프리셋은 정책 보호를 위해 모든 롤백 동작이 비활성화됩니다.
        </p>
    {/if}
    {#if historyState.phase === 'loading'}
        <p role="status">프롬프트 프리셋 리비전을 불러오는 중입니다.</p>
    {:else if historyState.phase === 'unavailable'}
        <p class="history-note" role="note">{historyState.error}</p>
    {:else if historyState.error !== null}
        <p class="history-error" role="alert">{historyState.error}</p>
    {/if}

    {#if historyState.truncated}
        <p class="history-note" role="note">최신 100개 리비전만 표시합니다.</p>
    {/if}

    {#if !showingReview}
        {#if historyState.receipt !== null}
            <section class="rollback-receipt" aria-label="롤백 적용 영수증">
                <p class="receipt-summary">
                    대상 리비전 {historyState.receipt.target_revision}의 내용이 새 리비전
                    {historyState.receipt.applied_revision}으로 저장되었습니다.
                </p>
                <dl class="detail-fields">
                    <div>
                        <dt>적용 리비전</dt>
                        <dd><code>{historyState.receipt.applied_revision_id}</code></dd>
                    </div>
                    <div>
                        <dt>적용 SHA-256</dt>
                        <dd><code>{historyState.receipt.applied_sha256}</code></dd>
                    </div>
                    <div>
                        <dt>승인 ID</dt>
                        <dd><code>{historyState.receipt.approval_id}</code></dd>
                    </div>
                    <div>
                        <dt>승인 SHA-256</dt>
                        <dd><code>{historyState.receipt.approval_sha256}</code></dd>
                    </div>
                </dl>
            </section>
        {/if}

        {#if historyState.revisions.length > 0}
            <ol class="setting-list revision-list" aria-label="프롬프트 프리셋 리비전">
                {#each [...historyState.revisions].reverse() as revision (revision.revision_id)}
                    <li>
                        <button
                            class="setting-row revision-row"
                            class:current={revision.revision === historyState.current_revision}
                            type="button"
                            disabled={reviewDisabled(revision)}
                            aria-current={revision.revision === historyState.current_revision
                                ? 'true'
                                : undefined}
                            aria-label={revision.revision === historyState.current_revision
                                ? `리비전 ${String(revision.revision)} 현재`
                                : `리비전 ${String(revision.revision)} 롤백 검토`}
                            title={reviewTitle(revision)}
                            onclick={() => void openReview(revision.revision)}
                        >
                            <span class="setting-content">
                                <span class="setting-copy revision-copy">
                                    <strong>리비전 {revision.revision} · {revision.name}</strong>
                                    <small>
                                        {new Date(revision.created_at).toLocaleString()} ·
                                        {shortHash(revision.sha256)}
                                    </small>
                                </span>
                            </span>
                            {#if revision.revision === historyState.current_revision}
                                <span class="revision-state">현재</span>
                            {/if}
                        </button>
                    </li>
                {/each}
            </ol>
        {:else if historyState.phase === 'ready'}
            <p class="history-note">저장된 프롬프트 프리셋 리비전이 없습니다.</p>
        {/if}
    {:else if historyState.review !== null && historyState.diff !== null}
        {@const review = historyState.review}
        {@const diff = historyState.diff}
        <section
            class="rollback-review"
            aria-label={`리비전 ${String(diff.to_revision)} 롤백 검토`}
        >
            <dl class="detail-fields">
                <div>
                    <dt>검토 해시</dt>
                    <dd><code>{review.review_sha256}</code></dd>
                </div>
                <div>
                    <dt>현재 프리셋</dt>
                    <dd>
                        <code>{review.expected_current_revision_id}</code> ·
                        <code>{shortHash(review.expected_current_sha256)}</code>
                    </dd>
                </div>
                <div>
                    <dt>대상 문서</dt>
                    <dd>
                        <code>{review.target_revision_id}</code> ·
                        <code>{shortHash(review.target_document_sha256)}</code>
                    </dd>
                </div>
                <div>
                    <dt>의존성 해시</dt>
                    <dd><code>{review.target_dependency_sha256}</code></dd>
                </div>
                <div>
                    <dt>바인딩 스냅샷</dt>
                    <dd><code>{review.binding_snapshot_sha256}</code></dd>
                </div>
                <div>
                    <dt>Diff 해시</dt>
                    <dd><code>{diff.diff_sha256}</code></dd>
                </div>
            </dl>
            <section class="changed-paths" aria-labelledby="prompt-rollback-changed-paths">
                <h3 id="prompt-rollback-changed-paths">
                    변경 경로 {diff.changed_paths.length}개
                </h3>
                {#if diff.changed_paths.length === 0}
                    <p>문서 필드 변경이 없습니다.</p>
                {:else}
                    <ul>
                        {#each diff.changed_paths as path (path)}
                            <li><code>{path}</code></li>
                        {/each}
                    </ul>
                {/if}
                {#if diff.truncated}
                    <p class="history-note">표시 한도를 넘은 변경 경로가 더 있습니다.</p>
                {/if}
            </section>
        </section>
    {:else if historyState.phase !== 'reviewing'}
        <p class="history-note">검토 내용을 다시 불러오세요.</p>
    {/if}

    {#if !showingReview}
        <DetailActionBar fixed className="history-action-bar" ariaLabel="리비전 이력 작업">
            <button
                class="primary detail-action detail-action--wide"
                type="button"
                disabled={busy || presetId === null || currentRevision === null}
                onclick={() => void controller.load(presetId, currentRevision)}
            >
                이력 새로고침
            </button>
        </DetailActionBar>
    {:else if historyState.review !== null && historyState.diff !== null}
        <DetailActionBar fixed className="history-action-bar" ariaLabel="롤백 검토 작업">
            <button
                class="primary detail-action detail-action--wide"
                type="button"
                disabled={disabled || historyState.phase === 'applying'}
                onclick={() => void applyRollback()}
            >
                {historyState.phase === 'applying'
                    ? '승인 적용 중…'
                    : historyState.approval_id === null
                      ? '이 검토 해시로 롤백 승인'
                      : '동일한 승인 ID로 다시 확인'}
            </button>
        </DetailActionBar>
    {/if}
</section>

<style>
    .preset-history {
        display: grid;
        min-width: 0;
        padding: 0;
        border: 0;
        margin: 0;
        background: transparent;
        gap: 18px;
    }

    .revision-list {
        width: auto;
    }

    .revision-row {
        min-width: 0;
    }

    .revision-row:disabled {
        cursor: default;
        opacity: 1;
    }

    .revision-copy {
        display: grid;
        min-width: 0;
        gap: 5px;
    }

    .revision-copy strong,
    .revision-copy small {
        overflow-wrap: anywhere;
        line-height: 1.35;
    }

    .revision-copy strong {
        color: var(--ink);
        font-size: var(--detail-support-type);
        font-weight: 550;
    }

    .revision-copy small {
        color: var(--ink-muted);
        font-size: var(--detail-support-type);
        font-weight: 550;
    }

    .revision-state {
        flex: none;
        color: var(--warning);
        font-size: var(--detail-support-type);
        font-weight: 750;
    }

    .history-note,
    .history-error {
        margin: 0;
        padding: 12px;
        border-radius: 12px;
        background: var(--surface-sunken);
        color: var(--ink-muted);
        line-height: 1.5;
    }

    .history-error {
        border: 1px solid var(--status-error-border);
        color: var(--status-error-fg);
        background: var(--status-error-bg);
    }

    .rollback-review,
    .rollback-receipt {
        display: grid;
        min-width: 0;
        padding: 0;
        border: 0;
        background: transparent;
        gap: 18px;
    }

    .detail-fields {
        display: grid;
        margin: 0;
    }

    .detail-fields > div {
        display: grid;
        grid-template-columns: minmax(112px, 0.65fr) minmax(0, 1.35fr);
        align-items: start;
        padding: 13px 2px;
        border-bottom: 1px solid var(--line);
        gap: 12px;
    }

    .detail-fields > div:first-child {
        padding-top: 0;
    }

    dt {
        color: var(--ink-muted);
        font-size: var(--detail-support-type);
        font-weight: 700;
    }

    dd {
        min-width: 0;
        margin: 0;
        color: var(--ink);
        font-size: var(--detail-support-type);
        font-weight: 650;
        overflow-wrap: anywhere;
    }

    code {
        font-size: 0.76rem;
        white-space: pre-wrap;
        overflow-wrap: anywhere;
    }

    .receipt-summary,
    .changed-paths p {
        margin: 0;
        color: var(--ink-muted);
        line-height: 1.5;
    }

    .changed-paths {
        display: grid;
        padding-top: 18px;
        border-top: 1px solid var(--line);
        gap: 10px;
    }

    .changed-paths h3 {
        margin: 0;
        color: var(--ink);
        font-size: var(--detail-support-type);
    }

    .changed-paths ul {
        margin: 0;
        padding-inline-start: 1.4rem;
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

    @container view (max-width: 680px) {
        .detail-fields > div {
            grid-template-columns: 1fr;
            gap: 5px;
        }
    }
</style>
