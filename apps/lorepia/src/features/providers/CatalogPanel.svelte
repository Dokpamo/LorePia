<script lang="ts">
    import { tr } from '../../lib/i18n';
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

    function securityChanges(
        diff: ProviderCatalogDiffDto,
    ): ProviderCatalogDiffDto['manifest_changes'] {
        return diff.manifest_changes.filter((change) => change.security_review != null);
    }

    function reviewJson(value: unknown): string {
        return value === undefined ? '—' : JSON.stringify(value);
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
            <h2 id="catalog-title">{$tr('catalog.title')}</h2>
            <p>{$tr('catalog.hint')}</p>
        </div>
        <button
            class="primary"
            type="button"
            disabled={busy}
            onclick={() => void run(() => controller.pickProviderCatalogImport())}
        >
            {$tr('catalog.import')}
        </button>
    </header>

    {#if status}
        <dl class="status-grid">
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
        </dl>
        <p class="hash-line">
            {$tr('catalog.active_snapshot')} <code>{status.active_snapshot_sha256}</code>
        </p>
    {:else}
        <p class="notice">{$tr('catalog.status_unavailable')}</p>
    {/if}

    {#if pendingImport}
        {@const review = pendingImport.plan.review}
        <article class="review-card" aria-labelledby="catalog-import-review-title">
            <h3 id="catalog-import-review-title">{$tr('catalog.review.title')}</h3>
            <p>
                {$tr('catalog.review.transition', {
                    from: review.expected_active_revision,
                    to: review.candidate_revision,
                    count: changeCount(review.diff),
                })}
            </p>
            <dl class="review-grid">
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
                    <dd>{$tr('catalog.count', { count: review.diff.manifest_changes.length })}</dd>
                </div>
                <div>
                    <dt>{$tr('catalog.review.model_changes')}</dt>
                    <dd>{$tr('catalog.count', { count: review.diff.model_changes.length })}</dd>
                </div>
            </dl>
            <p class="hash-line">Payload <code>{review.payload_sha256}</code></p>
            <p class="hash-line">
                {$tr('catalog.review.exact_plan')} <code>{pendingImport.plan.plan_sha256}</code>
            </p>
            {#each securityChanges(review.diff) as change (change.provider_template_id)}
                {@const authority = change.security_review}
                {#if authority}
                    <section class="security-review" aria-label="Catalog security authority change">
                        <h4>{change.provider_template_id} · security authority</h4>
                        <div class="security-surfaces">
                            {#each [{ label: 'Before', surface: authority.before }, { label: 'After', surface: authority.after }] as { label, surface } (label)}
                                <div>
                                    <strong>{label}</strong>
                                    {#if surface}
                                        <dl>
                                            <div>
                                                <dt>Origin</dt>
                                                <dd><code>{surface.origin ?? '—'}</code></dd>
                                            </div>
                                            <div>
                                                <dt>Auth</dt>
                                                <dd>
                                                    <code>{reviewJson(surface.authentication)}</code
                                                    >
                                                </dd>
                                            </div>
                                            <div>
                                                <dt>Endpoints</dt>
                                                <dd>
                                                    <code>{reviewJson(surface.endpoints)}</code>
                                                </dd>
                                            </div>
                                            <div>
                                                <dt>Decoders</dt>
                                                <dd><code>{reviewJson(surface.decoders)}</code></dd>
                                            </div>
                                            <div>
                                                <dt>Mappings</dt>
                                                <dd>
                                                    <code
                                                        >{reviewJson(
                                                            surface.parameter_mappings,
                                                        )}</code
                                                    >
                                                </dd>
                                            </div>
                                        </dl>
                                    {:else}
                                        <p>없음</p>
                                    {/if}
                                </div>
                            {/each}
                        </div>
                    </section>
                {/if}
            {/each}
            <div class="actions">
                <button
                    class="primary"
                    type="button"
                    disabled={busy}
                    onclick={() => void run(() => controller.activateProviderCatalogImport())}
                >
                    {$tr('catalog.review.apply')}
                </button>
                <button
                    class="danger"
                    type="button"
                    disabled={busy}
                    onclick={() => void run(() => controller.discardProviderCatalogImport())}
                >
                    {$tr('catalog.review.discard')}
                </button>
            </div>
        </article>
    {/if}

    {#if history && history.revisions.length > 0}
        <section class="history" aria-labelledby="catalog-history-title">
            <header>
                <h3 id="catalog-history-title">{$tr('catalog.history.title')}</h3>
                <span>{$tr('catalog.count', { count: history.revisions.length })}</span>
            </header>
            <ul>
                {#each history.revisions as revision (revision.revision)}
                    <li>
                        <div>
                            <strong>r{revision.revision}</strong>
                            {#if revision.active}<span class="active-badge"
                                    >{$tr('catalog.history.active')}</span
                                >{/if}
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
                                {$tr('catalog.history.compare')}
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
                                {$tr('catalog.history.prepare_rollback')}
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
            <h3 id="catalog-rollback-review-title">{$tr('catalog.rollback.title')}</h3>
            <p>
                {$tr('catalog.rollback.transition', {
                    from: plan.from_revision,
                    to: plan.to_revision,
                    count: changeCount(plan.diff),
                })}
            </p>
            <p class="hash-line">
                {$tr('catalog.rollback.current_hash')} <code>{plan.expected_active_sha256}</code>
            </p>
            <p class="hash-line">
                {$tr('catalog.rollback.target_hash')} <code>{plan.target_sha256}</code>
            </p>
            <p class="hash-line">
                {$tr('catalog.review.exact_plan')} <code>{pendingRollback.plan_sha256}</code>
            </p>
            {#each securityChanges(plan.diff) as change (change.provider_template_id)}
                {@const authority = change.security_review}
                {#if authority}
                    <section
                        class="security-review"
                        aria-label="Catalog rollback security authority change"
                    >
                        <h4>{change.provider_template_id} · security authority</h4>
                        <div class="security-surfaces">
                            {#each [{ label: 'Before', surface: authority.before }, { label: 'After', surface: authority.after }] as { label, surface } (label)}
                                <div>
                                    <strong>{label}</strong>
                                    {#if surface}
                                        <dl>
                                            <div>
                                                <dt>Origin</dt>
                                                <dd><code>{surface.origin ?? '—'}</code></dd>
                                            </div>
                                            <div>
                                                <dt>Auth</dt>
                                                <dd>
                                                    <code>{reviewJson(surface.authentication)}</code
                                                    >
                                                </dd>
                                            </div>
                                            <div>
                                                <dt>Endpoints</dt>
                                                <dd>
                                                    <code>{reviewJson(surface.endpoints)}</code>
                                                </dd>
                                            </div>
                                            <div>
                                                <dt>Decoders</dt>
                                                <dd><code>{reviewJson(surface.decoders)}</code></dd>
                                            </div>
                                            <div>
                                                <dt>Mappings</dt>
                                                <dd>
                                                    <code
                                                        >{reviewJson(
                                                            surface.parameter_mappings,
                                                        )}</code
                                                    >
                                                </dd>
                                            </div>
                                        </dl>
                                    {:else}
                                        <p>없음</p>
                                    {/if}
                                </div>
                            {/each}
                        </div>
                    </section>
                {/if}
            {/each}
            <button
                class="danger"
                type="button"
                disabled={busy}
                onclick={() =>
                    void run(() => controller.activateProviderCatalogRollback(pendingRollback))}
            >
                {$tr('catalog.rollback.apply')}
            </button>
        </article>
    {/if}

    {#if workspace.catalog_diff}
        {@const diff = workspace.catalog_diff}
        <details class="diff-detail" open>
            <summary>
                {$tr('catalog.diff.transition', {
                    from: diff.from_revision,
                    to: diff.to_revision,
                    count: changeCount(diff),
                })}
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
        background: var(--surface-sunken);
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

    .security-review {
        margin: 12px 0;
        padding: 12px;
        border: 1px solid var(--line);
        border-radius: 10px;
        background: var(--surface-sunken);
    }

    .security-review h4 {
        margin: 0 0 8px;
    }

    .security-surfaces {
        display: grid;
        grid-template-columns: repeat(2, minmax(0, 1fr));
        gap: 8px;
    }

    .security-surfaces > div {
        min-width: 0;
        padding: 8px;
        border-radius: 8px;
        background: var(--surface);
    }

    .security-surfaces dl {
        display: grid;
        gap: 6px;
        margin: 8px 0 0;
    }

    .security-surfaces dd,
    .security-surfaces code {
        white-space: pre-wrap;
        overflow-wrap: anywhere;
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
        background: var(--surface-sunken);
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

    @container view (max-width: 760px) {
        .workflow-heading,
        .history li {
            align-items: stretch;
            flex-direction: column;
        }

        .status-grid,
        .review-grid {
            grid-template-columns: repeat(2, minmax(0, 1fr));
        }

        .security-surfaces {
            grid-template-columns: 1fr;
        }
    }
</style>
