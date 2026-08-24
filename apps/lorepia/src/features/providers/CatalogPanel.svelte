<script lang="ts">
    import { tick } from 'svelte';
    import { tr } from '../../lib/i18n';
    import DetailActionBar from '../../components/detail/DetailActionBar.svelte';
    import DetailPage from '../../components/detail/DetailPage.svelte';
    import type { LorepiaAppController, LorepiaAppState } from '../../app/app-controller';
    import type { ProviderCatalogDiffDto } from '../../lib/ipc/contracts';

    interface Props {
        appState: LorepiaAppState;
        controller: LorepiaAppController;
        detailPage?: string | null;
    }

    let { appState, controller, detailPage = $bindable(null) }: Props = $props();
    let busy = $state(false);

    const workspace = $derived(appState.providers.workspace);
    const status = $derived(workspace.catalog_status);
    const history = $derived(workspace.catalog_history);
    const pendingImport = $derived(workspace.pending_catalog_import);
    const pendingRollback = $derived(workspace.pending_catalog_rollback);
    const selectedRevision = $derived(
        history?.revisions.find(
            (revision) => detailPage === `revision:${String(revision.revision)}`,
        ) ?? null,
    );

    function changeCount(diff: ProviderCatalogDiffDto): number {
        return diff.manifest_changes.length + diff.model_changes.length;
    }

    function securityChanges(
        diff: ProviderCatalogDiffDto,
    ): ProviderCatalogDiffDto['manifest_changes'] {
        return diff.manifest_changes.filter((change) => change.security_review != null);
    }

    function reviewJson(value: unknown): string {
        return value === undefined ? '—' : JSON.stringify(value);
    }

    async function run(action: () => Promise<void>): Promise<void> {
        if (busy) return;
        busy = true;
        try {
            await action();
        } finally {
            busy = false;
        }
    }

    async function pickImportAndOpenReview(): Promise<void> {
        const previousTicket = appState.providers.workspace.pending_catalog_import;
        await controller.pickProviderCatalogImport();
        await tick();
        const nextTicket = appState.providers.workspace.pending_catalog_import;
        if (nextTicket !== null && nextTicket !== previousTicket) {
            detailPage = 'import-review';
        }
    }

    async function prepareRollbackAndOpenReview(targetRevision: number): Promise<void> {
        const previousPlan = appState.providers.workspace.pending_catalog_rollback;
        await controller.prepareProviderCatalogRollback(targetRevision);
        await tick();
        const nextPlan = appState.providers.workspace.pending_catalog_rollback;
        if (nextPlan !== null && nextPlan !== previousPlan) {
            detailPage = 'rollback-review';
        }
    }

    async function compareRevisionsAndOpenDiff(
        fromRevision: number,
        toRevision: number,
    ): Promise<void> {
        const previousDiff = appState.providers.workspace.catalog_diff;
        await controller.diffProviderCatalogRevisions(fromRevision, toRevision);
        await tick();
        const nextDiff = appState.providers.workspace.catalog_diff;
        if (nextDiff !== null && nextDiff !== previousDiff) {
            detailPage = 'diff';
        }
    }

    async function activateImportAndOpenDiff(): Promise<void> {
        await controller.activateProviderCatalogImport();
        await tick();
        if (workspace.pending_catalog_import !== null) return;
        detailPage = workspace.catalog_diff === null ? null : 'diff';
    }

    async function discardImportAndReturn(): Promise<void> {
        await controller.discardProviderCatalogImport();
        await tick();
        if (workspace.pending_catalog_import === null) detailPage = null;
    }

    async function activateRollbackAndOpenDiff(
        plan: NonNullable<typeof pendingRollback>,
    ): Promise<void> {
        await controller.activateProviderCatalogRollback(plan);
        await tick();
        if (workspace.pending_catalog_rollback !== null) return;
        detailPage = workspace.catalog_diff === null ? null : 'diff';
    }

    $effect(() => {
        if (detailPage === 'import-review' && pendingImport === null) detailPage = null;
        if (detailPage === 'rollback-review' && pendingRollback === null) detailPage = null;
        if (detailPage === 'diff' && workspace.catalog_diff === null) detailPage = null;
        if (detailPage?.startsWith('revision:') && selectedRevision === null) detailPage = null;
    });
</script>

{#snippet securityReview(diff: ProviderCatalogDiffDto, ariaLabel: string)}
    {#each securityChanges(diff) as change (change.provider_template_id)}
        {@const authority = change.security_review}
        {#if authority}
            <section class="security-review detail-section" aria-label={ariaLabel}>
                <h3>{change.provider_template_id} · security authority</h3>
                <div class="security-surfaces">
                    {#each [{ label: 'Before', surface: authority.before }, { label: 'After', surface: authority.after }] as { label, surface } (label)}
                        <section aria-label={label}>
                            <strong>{label}</strong>
                            {#if surface}
                                <dl class="security-list">
                                    <div>
                                        <dt>Origin</dt>
                                        <dd><code>{surface.origin ?? '—'}</code></dd>
                                    </div>
                                    <div>
                                        <dt>Auth</dt>
                                        <dd><code>{reviewJson(surface.authentication)}</code></dd>
                                    </div>
                                    <div>
                                        <dt>Endpoints</dt>
                                        <dd><code>{reviewJson(surface.endpoints)}</code></dd>
                                    </div>
                                    <div>
                                        <dt>Decoders</dt>
                                        <dd><code>{reviewJson(surface.decoders)}</code></dd>
                                    </div>
                                    <div>
                                        <dt>Mappings</dt>
                                        <dd>
                                            <code>{reviewJson(surface.parameter_mappings)}</code>
                                        </dd>
                                    </div>
                                </dl>
                            {:else}
                                <p class="empty-value">없음</p>
                            {/if}
                        </section>
                    {/each}
                </div>
            </section>
        {/if}
    {/each}
{/snippet}

{#snippet detailContent()}
    {#if detailPage === null}
        <ul class="setting-list catalog-index" aria-label={$tr('catalog.title')}>
            <li>
                <button
                    class="setting-row catalog-row"
                    type="button"
                    onclick={() => (detailPage = 'status')}
                >
                    <span class="setting-content">
                        <span class="setting-copy catalog-row-copy">
                            <strong>활성 카탈로그</strong>
                            <small>
                                {status
                                    ? `r${String(status.active_revision)} · ${$tr(
                                          'catalog.snapshots',
                                      )} ${$tr('catalog.count', {
                                          count: status.snapshot_count,
                                      })}`
                                    : $tr('catalog.status_unavailable')}
                            </small>
                        </span>
                    </span>
                </button>
            </li>

            {#if pendingImport}
                {@const review = pendingImport.plan.review}
                <li>
                    <button
                        class="setting-row catalog-row"
                        type="button"
                        onclick={() => (detailPage = 'import-review')}
                    >
                        <span class="setting-content">
                            <span class="setting-copy catalog-row-copy">
                                <strong>{$tr('catalog.review.title')}</strong>
                                <small>
                                    {$tr('catalog.review.transition', {
                                        from: review.expected_active_revision,
                                        to: review.candidate_revision,
                                        count: changeCount(review.diff),
                                    })}
                                </small>
                            </span>
                        </span>
                    </button>
                </li>
            {/if}

            {#if pendingRollback}
                {@const plan = pendingRollback.catalog_plan}
                <li>
                    <button
                        class="setting-row catalog-row"
                        type="button"
                        onclick={() => (detailPage = 'rollback-review')}
                    >
                        <span class="setting-content">
                            <span class="setting-copy catalog-row-copy">
                                <strong>{$tr('catalog.rollback.title')}</strong>
                                <small>
                                    {$tr('catalog.rollback.transition', {
                                        from: plan.from_revision,
                                        to: plan.to_revision,
                                        count: changeCount(plan.diff),
                                    })}
                                </small>
                            </span>
                        </span>
                    </button>
                </li>
            {/if}

            {#if workspace.catalog_diff}
                {@const diff = workspace.catalog_diff}
                <li>
                    <button
                        class="setting-row catalog-row"
                        type="button"
                        onclick={() => (detailPage = 'diff')}
                    >
                        <span class="setting-content">
                            <span class="setting-copy catalog-row-copy">
                                <strong>리비전 비교</strong>
                                <small>
                                    {$tr('catalog.diff.transition', {
                                        from: diff.from_revision,
                                        to: diff.to_revision,
                                        count: changeCount(diff),
                                    })}
                                </small>
                            </span>
                        </span>
                    </button>
                </li>
            {/if}

            {#if history}
                {#each history.revisions as revision (revision.revision)}
                    <li>
                        <button
                            class="setting-row catalog-row"
                            type="button"
                            onclick={() => (detailPage = `revision:${String(revision.revision)}`)}
                        >
                            <span class="setting-content">
                                <span class="setting-copy catalog-row-copy">
                                    <strong>리비전 r{revision.revision}</strong>
                                    <small>{revision.captured_at}</small>
                                </span>
                                {#if revision.active}
                                    <span class="active-badge">{$tr('catalog.history.active')}</span
                                    >
                                {/if}
                            </span>
                        </button>
                    </li>
                {/each}
            {/if}
        </ul>
    {:else if detailPage === 'status'}
        {#if status}
            <dl class="detail-fields" aria-label="활성 카탈로그 상태">
                <div>
                    <dt>{$tr('catalog.active_revision')}</dt>
                    <dd>{status.active_revision}</dd>
                </div>
                <div>
                    <dt>{$tr('catalog.state_version')}</dt>
                    <dd>{status.state_version}</dd>
                </div>
                <div>
                    <dt>{$tr('catalog.highest_revision')}</dt>
                    <dd>{status.highest_accepted_revision}</dd>
                </div>
                <div>
                    <dt>{$tr('catalog.snapshots')}</dt>
                    <dd>{$tr('catalog.count', { count: status.snapshot_count })}</dd>
                </div>
                <div class="digest-row">
                    <dt>{$tr('catalog.active_snapshot')}</dt>
                    <dd><code>{status.active_snapshot_sha256}</code></dd>
                </div>
            </dl>
        {:else}
            <p class="notice">{$tr('catalog.status_unavailable')}</p>
        {/if}
    {:else if detailPage === 'import-review' && pendingImport}
        {@const review = pendingImport.plan.review}
        <div class="detail-page">
            <p class="detail-lead">
                {$tr('catalog.review.transition', {
                    from: review.expected_active_revision,
                    to: review.candidate_revision,
                    count: changeCount(review.diff),
                })}
            </p>
            <dl class="detail-fields">
                <div>
                    <dt>{$tr('catalog.review.signing_key')}</dt>
                    <dd>{review.signing_key_id}</dd>
                </div>
                <div>
                    <dt>{$tr('catalog.review.signed_revision')}</dt>
                    <dd>{review.signed_catalog_revision}</dd>
                </div>
                <div>
                    <dt>{$tr('catalog.review.manifest_changes')}</dt>
                    <dd>
                        {$tr('catalog.count', { count: review.diff.manifest_changes.length })}
                    </dd>
                </div>
                <div>
                    <dt>{$tr('catalog.review.model_changes')}</dt>
                    <dd>{$tr('catalog.count', { count: review.diff.model_changes.length })}</dd>
                </div>
            </dl>
            <section class="digest-list detail-section" aria-label="가져오기 무결성 정보">
                <p><span>Payload</span><code>{review.payload_sha256}</code></p>
                <p>
                    <span>{$tr('catalog.review.exact_plan')}</span>
                    <code>{pendingImport.plan.plan_sha256}</code>
                </p>
            </section>
            {@render securityReview(review.diff, 'Catalog security authority change')}
        </div>
    {:else if detailPage === 'rollback-review' && pendingRollback}
        {@const plan = pendingRollback.catalog_plan}
        <div class="detail-page">
            <p class="detail-lead">
                {$tr('catalog.rollback.transition', {
                    from: plan.from_revision,
                    to: plan.to_revision,
                    count: changeCount(plan.diff),
                })}
            </p>
            <section class="digest-list detail-section" aria-label="롤백 무결성 정보">
                <p>
                    <span>{$tr('catalog.rollback.current_hash')}</span>
                    <code>{plan.expected_active_sha256}</code>
                </p>
                <p>
                    <span>{$tr('catalog.rollback.target_hash')}</span>
                    <code>{plan.target_sha256}</code>
                </p>
                <p>
                    <span>{$tr('catalog.review.exact_plan')}</span>
                    <code>{pendingRollback.plan_sha256}</code>
                </p>
            </section>
            {@render securityReview(plan.diff, 'Catalog rollback security authority change')}
        </div>
    {:else if detailPage === 'diff' && workspace.catalog_diff}
        {@const diff = workspace.catalog_diff}
        <div class="detail-page">
            <p class="detail-lead">
                {$tr('catalog.diff.transition', {
                    from: diff.from_revision,
                    to: diff.to_revision,
                    count: changeCount(diff),
                })}
            </p>
            <ul class="change-list" aria-label="카탈로그 변경 내역">
                {#each diff.manifest_changes as change (change.provider_template_id)}
                    <li>
                        <strong>manifest · {change.change}</strong>
                        <span>{change.provider_template_id}</span>
                        {#if change.changed_sections.length > 0}
                            <small>{change.changed_sections.join(', ')}</small>
                        {/if}
                    </li>
                {/each}
                {#each diff.model_changes as change (change.model_entry_id)}
                    <li>
                        <strong>model · {change.change}</strong>
                        <span>{change.model_entry_id}</span>
                        {#if change.changed_sections.length > 0}
                            <small>{change.changed_sections.join(', ')}</small>
                        {/if}
                    </li>
                {/each}
            </ul>
        </div>
    {:else if selectedRevision && history}
        <div class="detail-page">
            <dl
                class="detail-fields"
                aria-label={`${$tr('catalog.active_revision')} r${String(selectedRevision.revision)}`}
            >
                <div>
                    <dt>리비전</dt>
                    <dd>r{selectedRevision.revision}</dd>
                </div>
                <div>
                    <dt>상태</dt>
                    <dd>
                        {selectedRevision.active ? $tr('catalog.history.active') : '—'}
                    </dd>
                </div>
                <div>
                    <dt>저장 시각</dt>
                    <dd>{selectedRevision.captured_at}</dd>
                </div>
                <div class="digest-row">
                    <dt>스냅샷 SHA-256</dt>
                    <dd><code>{selectedRevision.snapshot_sha256}</code></dd>
                </div>
            </dl>
        </div>
    {/if}
{/snippet}

{#snippet detailActions()}
    {#if detailPage === null}
        <DetailActionBar className="catalog-action-bar" ariaLabel="카탈로그 작업">
            <button
                class="primary detail-action detail-action--wide"
                type="button"
                disabled={busy}
                onclick={() => void run(pickImportAndOpenReview)}
            >
                {$tr('catalog.import')}
            </button>
        </DetailActionBar>
    {:else if detailPage === 'import-review' && pendingImport}
        <DetailActionBar className="catalog-action-bar" ariaLabel={$tr('catalog.review.title')}>
            <button
                class="danger detail-action detail-action--destructive"
                type="button"
                disabled={busy}
                onclick={() => void run(discardImportAndReturn)}
            >
                {$tr('catalog.review.discard')}
            </button>
            <button
                class="primary detail-action detail-action--grow"
                type="button"
                disabled={busy}
                onclick={() => void run(activateImportAndOpenDiff)}
            >
                {$tr('catalog.review.apply')}
            </button>
        </DetailActionBar>
    {:else if detailPage === 'rollback-review' && pendingRollback}
        <DetailActionBar className="catalog-action-bar" ariaLabel={$tr('catalog.rollback.title')}>
            <button
                class="danger detail-action detail-action--destructive"
                type="button"
                disabled={busy}
                onclick={() => void run(() => activateRollbackAndOpenDiff(pendingRollback))}
            >
                {$tr('catalog.rollback.apply')}
            </button>
            <span class="detail-action--grow catalog-action-spacer" aria-hidden="true"></span>
        </DetailActionBar>
    {:else if selectedRevision && history}
        <DetailActionBar className="catalog-action-bar" ariaLabel="리비전 작업">
            <button
                class="danger detail-action detail-action--destructive"
                type="button"
                disabled={busy || selectedRevision.revision === history.active_revision}
                onclick={() =>
                    void run(() => prepareRollbackAndOpenReview(selectedRevision.revision))}
            >
                {$tr('catalog.history.prepare_rollback')}
            </button>
            <button
                class="primary detail-action detail-action--grow"
                type="button"
                disabled={busy || selectedRevision.revision === history.active_revision}
                onclick={() =>
                    void run(() =>
                        compareRevisionsAndOpenDiff(
                            history.active_revision,
                            selectedRevision.revision,
                        ),
                    )}
            >
                {$tr('catalog.history.compare')}
            </button>
        </DetailActionBar>
    {/if}
{/snippet}

<DetailPage
    className="catalog-panel"
    scrollClassName="provider-scroll settings-detail-scroll catalog-scroll"
    ariaLabel={$tr('catalog.title')}
    resetKey={detailPage ?? 'index'}
    hasActions={detailPage === null ||
        (detailPage === 'import-review' && pendingImport !== null) ||
        (detailPage === 'rollback-review' && pendingRollback !== null) ||
        (selectedRevision !== null && history !== null)}
    content={detailContent}
    actions={detailActions}
/>

<style>
    .catalog-index {
        width: auto;
        margin: 0;
    }

    .catalog-row-copy {
        display: grid;
        min-width: 0;
        gap: 5px;
    }

    .catalog-row-copy > :is(strong, small) {
        overflow: hidden;
        font-size: var(--detail-support-type);
        line-height: 1.35;
        text-overflow: ellipsis;
    }

    .catalog-row-copy > strong {
        color: var(--ink);
        font-weight: 550;
        white-space: nowrap;
    }

    .catalog-row-copy > small {
        display: -webkit-box;
        color: var(--ink-muted);
        font-weight: 550;
        overflow-wrap: anywhere;
        white-space: normal;
        line-clamp: 3;
        -webkit-box-orient: vertical;
        -webkit-line-clamp: 3;
    }

    .active-badge {
        width: fit-content;
        flex: none;
        padding: 3px 8px;
        border-radius: var(--radius-pill);
        color: var(--warning);
        background: color-mix(in srgb, var(--brand-sun) 14%, transparent);
        font-size: 0.72rem;
        font-weight: 800;
    }

    .detail-page {
        display: grid;
        min-width: 0;
        gap: 18px;
    }

    .detail-lead,
    .notice,
    .empty-value {
        margin: 0;
        color: var(--ink-muted);
        line-height: 1.5;
    }

    .notice {
        padding: 12px;
        border-radius: 12px;
        background: var(--surface-sunken);
    }

    .detail-fields,
    .security-list {
        display: grid;
        margin: 0;
    }

    .detail-fields > div,
    .security-list > div {
        display: grid;
        grid-template-columns: minmax(112px, 0.65fr) minmax(0, 1.35fr);
        align-items: start;
        padding: 13px 2px;
        border-bottom: 1px solid var(--line);
        gap: 12px;
    }

    .detail-fields > div:first-child,
    .security-list > div:first-child {
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

    .detail-section {
        padding-top: 18px;
        border-top: 1px solid var(--line);
    }

    .digest-list {
        display: grid;
        gap: 12px;
    }

    .digest-list p {
        display: grid;
        min-width: 0;
        margin: 0;
        gap: 5px;
    }

    .digest-list span,
    .security-review h3 {
        color: var(--ink-muted);
        font-size: var(--detail-support-type);
        font-weight: 700;
    }

    .security-review {
        display: grid;
        min-width: 0;
        gap: 14px;
    }

    .security-review h3 {
        margin: 0;
        color: var(--ink);
    }

    .security-surfaces {
        display: grid;
        grid-template-columns: repeat(2, minmax(0, 1fr));
        gap: 18px;
    }

    .security-surfaces > section {
        display: grid;
        min-width: 0;
        align-content: start;
        gap: 10px;
    }

    .security-surfaces > section + section {
        padding-left: 18px;
        border-left: 1px solid var(--line);
    }

    .security-surfaces > section > strong {
        color: var(--ink);
        font-size: var(--detail-support-type);
    }

    .security-list > div {
        grid-template-columns: minmax(72px, 0.45fr) minmax(0, 1.55fr);
        padding-block: 9px;
    }

    .change-list {
        display: grid;
        padding: 0;
        margin: 0;
        list-style: none;
    }

    .change-list li {
        display: grid;
        padding: 13px 2px;
        border-bottom: 1px solid var(--line);
        gap: 4px;
    }

    .change-list li:first-child {
        padding-top: 0;
    }

    .change-list :is(span, small) {
        color: var(--ink-muted);
        overflow-wrap: anywhere;
    }

    .catalog-action-spacer {
        min-width: 0;
    }

    @container view (max-width: 760px) {
        .security-surfaces {
            grid-template-columns: 1fr;
        }

        .security-surfaces > section + section {
            padding-top: 14px;
            padding-left: 0;
            border-top: 1px solid var(--line);
            border-left: 0;
        }

        .detail-fields > div,
        .security-list > div {
            grid-template-columns: 1fr;
            gap: 5px;
        }
    }
</style>
