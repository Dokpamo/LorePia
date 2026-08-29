<script lang="ts">
    import { X } from '@lucide/svelte';
    import { onMount } from 'svelte';

    import type { LorepiaAppState, LorepiaAppController } from '../../app/app-controller';
    import {
        inspectPortableRegexRules,
        type PortableRegexReviewResult,
    } from '../chat/portable-regex';
    import { t, tr } from '../../lib/i18n';
    import type { ImportInspectionDto } from '../../lib/ipc/contracts';

    interface Props {
        state: LorepiaAppState;
        controller: LorepiaAppController;
    }

    let { state: appState, controller }: Props = $props();
    let dialog: HTMLDialogElement;
    let regexReviewPhase = $state<'idle' | 'checking' | 'ready'>('idle');
    let regexReviewResults = $state<PortableRegexReviewResult[]>([]);

    $effect(() => {
        const inspection = appState.import_flow.inspection;
        const rules = inspection?.dynamic_content.regex_rules ?? [];
        let cancelled = false;
        regexReviewResults = [];
        if (inspection === null || rules.length === 0) {
            regexReviewPhase = 'idle';
            return;
        }
        regexReviewPhase = 'checking';
        void inspectPortableRegexRules(rules, `import:${inspection.inspection_id}`).then(
            (results) => {
                if (
                    cancelled ||
                    appState.import_flow.inspection?.inspection_id !== inspection.inspection_id
                )
                    return;
                regexReviewResults = results;
                regexReviewPhase = 'ready';
            },
        );
        return () => {
            cancelled = true;
        };
    });

    const invalidRegexCount = $derived(
        regexReviewResults.filter((result) => result.status === 'invalid').length,
    );
    const timedOutRegexCount = $derived(
        regexReviewResults.filter((result) => result.status === 'timed_out').length,
    );
    const unavailableRegexCount = $derived(
        regexReviewResults.filter((result) => result.status === 'unavailable').length,
    );

    onMount(() => {
        const previousFocus =
            document.activeElement instanceof HTMLElement ? document.activeElement : null;
        if (!dialog.open) {
            if (typeof dialog.showModal === 'function') {
                dialog.showModal();
            } else {
                dialog.setAttribute('open', '');
            }
        }
        dialog.focus();
        return () => {
            if (dialog.open && typeof dialog.close === 'function') {
                dialog.close();
            }
            previousFocus?.focus();
        };
    });

    function formatBytes(value: number): string {
        return new Intl.NumberFormat('ko-KR', {
            style: 'unit',
            unit: value >= 1_048_576 ? 'megabyte' : 'kilobyte',
            maximumFractionDigits: 1,
        }).format(value >= 1_048_576 ? value / 1_048_576 : value / 1_024);
    }

    function kindLabel(kind: ImportInspectionDto['kind']): string {
        if (kind === 'charx_package') return 'CHARX';
        if (kind === 'character_card_png') return t('import.kind.png');
        return 'CCv3 JSON';
    }
</script>

<div class="modal-backdrop">
    <dialog
        class="modal-card"
        aria-modal="true"
        aria-labelledby="import-review-title"
        tabindex="-1"
        bind:this={dialog}
        oncancel={(event) => {
            event.preventDefault();
            void controller.discardImport();
        }}
    >
        <header class="modal-header">
            <div>
                <p class="eyebrow">Import review</p>
                <h2 id="import-review-title">{$tr('import.title')}</h2>
            </div>
            <button
                class="icon-button"
                type="button"
                aria-label={$tr('import.dialog.close')}
                onclick={() => void controller.discardImport()}
            >
                <X class="import-close-icon" size={20} aria-hidden="true" />
            </button>
        </header>

        {#if appState.import_flow.phase === 'loading'}
            <div class="state-panel" role="status">{$tr('import.inspecting')}</div>
        {:else if appState.import_flow.phase === 'error'}
            <div class="state-panel error" role="alert">
                <p>{appState.import_flow.error}</p>
                <button type="button" onclick={() => void controller.discardImport()}
                    >{$tr('import.close')}</button
                >
            </div>
        {:else if appState.import_flow.inspection}
            {@const inspection = appState.import_flow.inspection}
            <div class="review-summary">
                <span class="review-avatar" aria-hidden="true"
                    >{inspection.display_name.slice(0, 1)}</span
                >
                <div>
                    <h3>{inspection.display_name}</h3>
                    <p>{inspection.description || $tr('import.description.empty')}</p>
                </div>
            </div>

            <dl class="metadata-grid">
                <div>
                    <dt>{$tr('import.kind')}</dt>
                    <dd>{kindLabel(inspection.kind)}</dd>
                </div>
                <div>
                    <dt>{$tr('import.source_size')}</dt>
                    <dd>{formatBytes(inspection.source_size)}</dd>
                </div>
                <div>
                    <dt>{$tr('import.estimated_size')}</dt>
                    <dd>{formatBytes(inspection.estimated_stored_size)}</dd>
                </div>
                <div>
                    <dt>{$tr('import.assets')}</dt>
                    <dd>
                        {$tr('import.assets.count', {
                            count: inspection.asset_count.toLocaleString(),
                        })}
                    </dd>
                </div>
            </dl>

            {#if inspection.dynamic_content.runtime_script_count > 0 || inspection.dynamic_content.regex_rule_count > 0 || inspection.dynamic_content.custom_markup_present}
                <section class="issue-box warning" aria-labelledby="dynamic-content-title">
                    <h3 id="dynamic-content-title">{$tr('import.dynamic.title')}</h3>
                    <ul>
                        {#if inspection.dynamic_content.runtime_script_count > 0}
                            <li>
                                {$tr('import.dynamic.lua', {
                                    count: inspection.dynamic_content.runtime_script_count,
                                })}
                            </li>
                        {/if}
                        {#if inspection.dynamic_content.elevated_runtime_script_count > 0}
                            <li>
                                {$tr('import.dynamic.elevated', {
                                    count: inspection.dynamic_content.elevated_runtime_script_count,
                                })}
                            </li>
                        {/if}
                        {#if inspection.dynamic_content.model_calls_possible}
                            <li>{$tr('import.dynamic.model')}</li>
                        {/if}
                        {#if inspection.dynamic_content.custom_markup_present}
                            <li>{$tr('import.dynamic.markup')}</li>
                        {/if}
                        {#if inspection.dynamic_content.regex_rule_count > 0}
                            <li>
                                {$tr('import.dynamic.regex', {
                                    count: inspection.dynamic_content.regex_rule_count,
                                })}
                            </li>
                        {/if}
                    </ul>
                    {#if regexReviewPhase === 'checking'}
                        <p role="status">{$tr('import.regex.checking')}</p>
                    {:else if regexReviewPhase === 'ready'}
                        {#if invalidRegexCount + timedOutRegexCount > 0}
                            <p role="alert">
                                {$tr('import.regex.disabled', {
                                    count: invalidRegexCount + timedOutRegexCount,
                                })}
                            </p>
                        {:else}
                            <p>{$tr('import.regex.valid')}</p>
                        {/if}
                        {#if unavailableRegexCount > 0}
                            <p role="alert">
                                {$tr('import.regex.unavailable', {
                                    count: unavailableRegexCount,
                                })}
                            </p>
                        {/if}
                    {/if}
                    <p>{$tr('import.dynamic.network')}</p>
                    <p>{$tr('import.dynamic.safe_mode')}</p>
                </section>
            {/if}

            {#if inspection.blocked_reasons.length > 0}
                <section class="issue-box blocked" aria-labelledby="blocked-title">
                    <h3 id="blocked-title">{$tr('import.blocked')}</h3>
                    <ul>
                        {#each inspection.blocked_reasons as reason (reason)}
                            <li>{reason}</li>
                        {/each}
                    </ul>
                </section>
            {/if}

            {#if inspection.warnings.length > 0}
                <section class="issue-box warning" aria-labelledby="warning-title">
                    <h3 id="warning-title">{$tr('import.warnings')}</h3>
                    <ul>
                        {#each inspection.warnings as warning (warning.code)}
                            <li>{warning.message}</li>
                        {/each}
                    </ul>
                </section>
            {/if}

            {#if inspection.unsupported_optional_fields.length > 0}
                <details>
                    <summary>{$tr('import.unsupported_fields')}</summary>
                    <p>{inspection.unsupported_optional_fields.join(', ')}</p>
                </details>
            {/if}

            <footer class="modal-actions">
                <button type="button" onclick={() => void controller.discardImport()}
                    >{$tr('import.cancel')}</button
                >
                <button
                    class="primary"
                    type="button"
                    disabled={!inspection.allowed || regexReviewPhase === 'checking'}
                    onclick={() => void controller.commitImport()}
                >
                    {#if inspection.dynamic_content.runtime_script_count > 0 || inspection.dynamic_content.regex_rule_count > 0 || inspection.dynamic_content.custom_markup_present}
                        {$tr('import.commit.safe')}
                    {:else}
                        {$tr('import.commit')}
                    {/if}
                </button>
            </footer>
        {/if}
    </dialog>
</div>
