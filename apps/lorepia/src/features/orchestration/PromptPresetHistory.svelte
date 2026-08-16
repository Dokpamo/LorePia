<script lang="ts">
    import { onMount, untrack } from 'svelte';

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
    }

    let {
        client,
        presetId,
        currentRevision,
        disabled = false,
        onApplied = () => undefined,
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

    async function applyRollback(): Promise<void> {
        const receipt = await controller.applyReviewedRollback();
        if (receipt !== null) await onApplied(receipt);
    }
</script>

<section class="preset-history" aria-labelledby="prompt-preset-history-title">
    <div class="history-heading">
        <div>
            <h4 id="prompt-preset-history-title">프롬프트 프리셋 리비전</h4>
            <p>과거 문서를 직접 제출하지 않고 Core가 검증한 해시를 승인해 새 리비전을 만듭니다.</p>
        </div>
        <button
            type="button"
            disabled={busy || presetId === null || currentRevision === null}
            onclick={() => void controller.load(presetId, currentRevision)}
        >
            이력 새로고침
        </button>
    </div>

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

    {#if historyState.revisions.length > 0}
        <ol class="revision-list">
            {#each [...historyState.revisions].reverse() as revision (revision.revision_id)}
                <li class:current={revision.revision === historyState.current_revision}>
                    <div>
                        <strong>리비전 {revision.revision}</strong>
                        <span>{revision.name}</span>
                        <small>
                            {new Date(revision.created_at).toLocaleString()} ·
                            {shortHash(revision.sha256)}
                        </small>
                    </div>
                    {#if revision.revision === historyState.current_revision}
                        <span class="current-badge">현재</span>
                    {:else}
                        <button
                            type="button"
                            disabled={disabled ||
                                busy ||
                                !revision.rollback_allowed ||
                                (historyState.current_revision !== null &&
                                    revision.revision > historyState.current_revision)}
                            aria-label={`리비전 ${String(revision.revision)} 롤백 검토`}
                            title={!revision.rollback_allowed
                                ? '앱 내장 프리셋은 롤백할 수 없습니다.'
                                : historyState.current_revision !== null &&
                                    revision.revision > historyState.current_revision
                                  ? '현재 상태보다 새로운 리비전입니다. 구성을 다시 불러오세요.'
                                  : undefined}
                            onclick={() => void controller.reviewTarget(revision.revision)}
                        >
                            변경 내역 검토
                        </button>
                    {/if}
                </li>
            {/each}
        </ol>
    {:else if historyState.phase === 'ready'}
        <p class="history-note">저장된 프롬프트 프리셋 리비전이 없습니다.</p>
    {/if}

    {#if historyState.review !== null && historyState.diff !== null}
        {@const review = historyState.review}
        {@const diff = historyState.diff}
        <section class="rollback-review" aria-labelledby="prompt-rollback-review-title">
            <h5 id="prompt-rollback-review-title">
                리비전 {diff.from_revision} → {diff.to_revision} 롤백 검토
            </h5>
            <p>아래 검토 해시는 현재 프리셋, 대상 문서, 의존성, 바인딩 스냅샷을 함께 고정합니다.</p>
            <dl>
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
            <div class="changed-paths">
                <strong>변경 경로 {diff.changed_paths.length}개</strong>
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
            </div>
            <button
                class="apply-button"
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
        </section>
    {/if}

    {#if historyState.receipt !== null}
        <section class="rollback-receipt" aria-labelledby="prompt-rollback-receipt-title">
            <h5 id="prompt-rollback-receipt-title">롤백 적용 영수증</h5>
            <p>
                대상 리비전 {historyState.receipt.target_revision}의 내용이 새 리비전
                {historyState.receipt.applied_revision}으로 저장되었습니다.
            </p>
            <dl>
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
</section>

<style>
    .preset-history {
        display: grid;
        gap: 0.8rem;
        margin-block: 1rem;
        padding: 1rem;
        border: 1px solid var(--line);
        border-radius: 0.8rem;
        background: color-mix(in srgb, var(--surface-raised) 94%, transparent);
    }

    .history-heading,
    .revision-list li {
        display: flex;
        align-items: center;
        justify-content: space-between;
        gap: 1rem;
    }

    .history-heading h4,
    .rollback-review h5,
    .rollback-receipt h5 {
        margin: 0;
    }

    .history-heading p,
    .rollback-review > p,
    .rollback-receipt > p {
        margin: 0.25rem 0 0;
    }

    .revision-list {
        display: grid;
        gap: 0.5rem;
        margin: 0;
        padding: 0;
        list-style: none;
    }

    .revision-list li {
        padding: 0.7rem;
        border: 1px solid var(--line);
        border-radius: 0.6rem;
    }

    .revision-list li.current {
        border-color: var(--accent);
    }

    .revision-list li > div {
        display: grid;
        gap: 0.15rem;
        min-width: 0;
    }

    .revision-list small,
    .revision-list span {
        overflow-wrap: anywhere;
    }

    .current-badge {
        padding: 0.2rem 0.5rem;
        border-radius: 999px;
        background: var(--accent-soft);
    }

    .history-note,
    .history-error {
        margin: 0;
        padding: 0.65rem;
        border-radius: 0.5rem;
        background: var(--surface-sunken);
    }

    .history-error {
        color: var(--danger);
    }

    .rollback-review,
    .rollback-receipt {
        display: grid;
        gap: 0.75rem;
        padding: 0.8rem;
        border-radius: 0.6rem;
        background: var(--surface-sunken);
    }

    dl {
        display: grid;
        gap: 0.5rem;
        margin: 0;
    }

    dl div {
        display: grid;
        gap: 0.15rem;
    }

    dt {
        font-weight: 700;
    }

    dd {
        margin: 0;
        overflow-wrap: anywhere;
    }

    code {
        overflow-wrap: anywhere;
    }

    .changed-paths ul {
        max-height: 14rem;
        overflow: auto;
        margin: 0.4rem 0;
        padding-inline-start: 1.4rem;
    }

    .apply-button {
        justify-self: start;
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

    @media (max-width: 680px) {
        .history-heading,
        .revision-list li {
            align-items: stretch;
            flex-direction: column;
        }
    }
</style>
