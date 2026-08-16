<script lang="ts">
    import type { LorepiaAppController, LorepiaAppState } from '../../app/app-controller';
    import type { ProviderCatalogDiffDto } from '../../lib/ipc/contracts';

    interface Props {
        appState: LorepiaAppState;
        controller: LorepiaAppController;
    }

    let { appState, controller }: Props = $props();
    let busy = $state(false);

    const workspace = $derived(appState.providers.workspace);
    const status = $derived(workspace.catalog_status);
    const history = $derived(workspace.catalog_history);
    const pendingImport = $derived(workspace.pending_catalog_import);
    const pendingRollback = $derived(workspace.pending_catalog_rollback);

    function changeCount(diff: ProviderCatalogDiffDto): number {
        return diff.manifest_changes.length + diff.model_changes.length;
    }

    async function run(action: () => Promise<void>): Promise<void> {
        busy = true;
        try {
            await action();
        } finally {
            busy = false;
        }
    }
</script>

<section class="workflow-section" aria-labelledby="catalog-title">
    <header class="workflow-heading">
        <div>
            <p class="eyebrow">Signed local catalog</p>
            <h2 id="catalog-title">프로바이더 카탈로그</h2>
            <p>서명·상태 버전·정확한 계획 해시를 검토한 뒤 가져오기나 롤백을 적용합니다.</p>
        </div>
        <button
            class="primary"
            type="button"
            disabled={busy}
            onclick={() => void run(() => controller.pickProviderCatalogImport())}
        >
            서명 카탈로그 가져오기
        </button>
    </header>

    {#if status}
        <dl class="status-grid">
            <div>
                <dt>활성 리비전</dt>
                <dd>{status.active_revision}</dd>
            </div>
            <div>
                <dt>상태 버전</dt>
                <dd>{status.state_version}</dd>
            </div>
            <div>
                <dt>최고 승인 리비전</dt>
                <dd>{status.highest_accepted_revision}</dd>
            </div>
            <div>
                <dt>저장 스냅샷</dt>
                <dd>{status.snapshot_count}개</dd>
            </div>
        </dl>
        <p class="hash-line">활성 스냅샷 <code>{status.active_snapshot_sha256}</code></p>
    {:else}
        <p class="notice">카탈로그 상태를 아직 불러오지 못했습니다.</p>
    {/if}

    {#if pendingImport}
        {@const review = pendingImport.plan.review}
        <article class="review-card" aria-labelledby="catalog-import-review-title">
            <h3 id="catalog-import-review-title">가져오기 계획 검토</h3>
            <p>
                활성 r{review.expected_active_revision} → 후보 r{review.candidate_revision} · 변경
                {changeCount(review.diff)}개
            </p>
            <dl class="review-grid">
                <div>
                    <dt>서명 키</dt>
                    <dd>{review.signing_key_id}</dd>
                </div>
                <div>
                    <dt>서명 카탈로그 리비전</dt>
                    <dd>{review.signed_catalog_revision}</dd>
                </div>
                <div>
                    <dt>Manifest 변경</dt>
                    <dd>{review.diff.manifest_changes.length}개</dd>
                </div>
                <div>
                    <dt>모델 변경</dt>
                    <dd>{review.diff.model_changes.length}개</dd>
                </div>
            </dl>
            <p class="hash-line">Payload <code>{review.payload_sha256}</code></p>
            <p class="hash-line">정확한 계획 <code>{pendingImport.plan.plan_sha256}</code></p>
            <div class="actions">
                <button
                    class="primary"
                    type="button"
                    disabled={busy}
                    onclick={() => void run(() => controller.activateProviderCatalogImport())}
                >
                    검토한 정확한 가져오기 계획 적용
                </button>
                <button
                    class="danger"
                    type="button"
                    disabled={busy}
                    onclick={() => void run(() => controller.discardProviderCatalogImport())}
                >
                    가져오기 계획 폐기
                </button>
            </div>
        </article>
    {/if}

    {#if history && history.revisions.length > 0}
        <section class="history" aria-labelledby="catalog-history-title">
            <header>
                <h3 id="catalog-history-title">로컬 리비전 이력</h3>
                <span>{history.revisions.length}개</span>
            </header>
            <ul>
                {#each history.revisions as revision (revision.revision)}
                    <li>
                        <div>
                            <strong>r{revision.revision}</strong>
                            {#if revision.active}<span class="active-badge">활성</span>{/if}
                            <small>{revision.captured_at}</small>
                            <code>{revision.snapshot_sha256}</code>
                        </div>
                        <div class="actions">
                            <button
                                type="button"
                                disabled={busy || revision.revision === history.active_revision}
                                onclick={() =>
                                    void run(() =>
                                        controller.diffProviderCatalogRevisions(
                                            history.active_revision,
                                            revision.revision,
                                        ),
                                    )}
                            >
                                활성 버전과 비교
                            </button>
                            <button
                                type="button"
                                disabled={busy || revision.revision === history.active_revision}
                                onclick={() =>
                                    void run(() =>
                                        controller.prepareProviderCatalogRollback(
                                            revision.revision,
                                        ),
                                    )}
                            >
                                이 리비전으로 롤백 준비
                            </button>
                        </div>
                    </li>
                {/each}
            </ul>
        </section>
    {/if}

    {#if pendingRollback}
        {@const plan = pendingRollback.catalog_plan}
        <article class="review-card rollback" aria-labelledby="catalog-rollback-review-title">
            <h3 id="catalog-rollback-review-title">롤백 계획 검토</h3>
            <p>
                r{plan.from_revision} → r{plan.to_revision} · 변경 {changeCount(plan.diff)}개
            </p>
            <p class="hash-line">현재 해시 <code>{plan.expected_active_sha256}</code></p>
            <p class="hash-line">대상 해시 <code>{plan.target_sha256}</code></p>
            <p class="hash-line">정확한 계획 <code>{pendingRollback.plan_sha256}</code></p>
            <button
                class="danger"
                type="button"
                disabled={busy}
                onclick={() =>
                    void run(() => controller.activateProviderCatalogRollback(pendingRollback))}
            >
                검토한 정확한 롤백 계획 적용
            </button>
        </article>
    {/if}

    {#if workspace.catalog_diff}
        {@const diff = workspace.catalog_diff}
        <details class="diff-detail" open>
            <summary>
                r{diff.from_revision} → r{diff.to_revision} 변경
                {changeCount(diff)}개
            </summary>
            <ul>
                {#each diff.manifest_changes as change (change.provider_template_id)}
                    <li>
                        manifest · {change.change} · {change.provider_template_id}
                        {#if change.changed_sections.length > 0}
                            <small>{change.changed_sections.join(', ')}</small>
                        {/if}
                    </li>
                {/each}
                {#each diff.model_changes as change (change.model_entry_id)}
                    <li>
                        model · {change.change} · {change.model_entry_id}
                        {#if change.changed_sections.length > 0}
                            <small>{change.changed_sections.join(', ')}</small>
                        {/if}
                    </li>
                {/each}
            </ul>
        </details>
    {/if}
</section>

<style>
    .workflow-section {
        padding: 20px;
        border: 1px solid var(--line);
        border-radius: var(--radius-md);
        background: var(--surface-raised);
    }

    .workflow-heading,
    .history > header,
    .history li {
        display: flex;
        gap: 12px;
        align-items: center;
        justify-content: space-between;
    }

    .workflow-heading h2,
    .review-card h3,
    .history h3 {
        margin: 3px 0;
    }

    .workflow-heading p:last-child,
    .review-card > p,
    .notice {
        margin: 5px 0 0;
        color: var(--ink-muted);
        line-height: 1.45;
    }

    .status-grid,
    .review-grid {
        display: grid;
        grid-template-columns: repeat(4, minmax(0, 1fr));
        gap: 8px;
        margin: 16px 0 0;
    }

    .status-grid > div,
    .review-grid > div {
        padding: 10px;
        border-radius: 10px;
        background: var(--surface-muted);
    }

    dt {
        color: var(--ink-muted);
        font-size: 0.7rem;
    }

    dd {
        margin: 3px 0 0;
        overflow-wrap: anywhere;
        font-weight: 800;
    }

    .hash-line {
        overflow-wrap: anywhere;
    }

    code {
        font-size: 0.72rem;
    }

    .review-card,
    .history,
    .diff-detail {
        margin-top: 16px;
        padding: 15px;
        border: 1px solid var(--line);
        border-radius: 14px;
        background: var(--surface);
    }

    .rollback {
        border-color: color-mix(in srgb, var(--danger), transparent 55%);
    }

    .actions {
        display: flex;
        gap: 8px;
        align-items: center;
        flex-wrap: wrap;
    }

    .history ul,
    .diff-detail ul {
        display: grid;
        gap: 8px;
        margin: 12px 0 0;
        padding: 0;
        list-style: none;
    }

    .history li,
    .diff-detail li {
        padding: 10px;
        border-radius: 10px;
        background: var(--surface-muted);
    }

    .history li > div:first-child,
    .diff-detail li {
        display: grid;
        gap: 3px;
    }

    small {
        color: var(--ink-muted);
    }

    .active-badge {
        width: fit-content;
        padding: 3px 7px;
        border-radius: 999px;
        color: var(--accent);
        background: var(--accent-soft);
        font-size: 0.7rem;
        font-weight: 800;
    }

    @media (max-width: 760px) {
        .workflow-heading,
        .history li {
            align-items: stretch;
            flex-direction: column;
        }

        .status-grid,
        .review-grid {
            grid-template-columns: repeat(2, minmax(0, 1fr));
        }
    }
</style>
