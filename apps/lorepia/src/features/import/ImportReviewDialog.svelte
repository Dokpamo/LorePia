<script lang="ts">
    import { X } from '@lucide/svelte';
    import { onMount } from 'svelte';

    import type { LorepiaAppState, LorepiaAppController } from '../../app/app-controller';
    import {
        inspectPortableRegexRules,
        type PortableRegexReviewResult,
    } from '../chat/portable-regex';
    import { t, tr } from '../../lib/i18n';
    import { importedText } from '../../lib/import-display';
    import type { ImportCommitResultDto, ImportInspectionDto } from '../../lib/ipc/contracts';

    interface Props {
        state: LorepiaAppState;
        controller: LorepiaAppController;
        onCommitted?: (result: ImportCommitResultDto) => void;
    }

    let { state: appState, controller, onCommitted }: Props = $props();
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
        const gibibyte = 1_073_741_824;
        const mebibyte = 1_048_576;
        const unit = value >= gibibyte ? 'gigabyte' : value >= mebibyte ? 'megabyte' : 'kilobyte';
        const divisor = value >= gibibyte ? gibibyte : value >= mebibyte ? mebibyte : 1_024;
        return new Intl.NumberFormat('ko-KR', {
            style: 'unit',
            unit,
            maximumFractionDigits: 1,
        }).format(value / divisor);
    }

    function kindLabel(kind: ImportInspectionDto['kind']): string {
        if (kind === 'charx_package') return 'CHARX';
        if (kind === 'character_card_png') return t('import.kind.png');
        if (kind === 'imported_module') return t('import.kind.imported_module');
        if (kind === 'imported_preset') return t('import.kind.imported_preset');
        if (kind === 'imported_memory_preset') return t('import.kind.imported_memory');
        return 'CCv3 JSON';
    }

    function isImportedContent(kind: ImportInspectionDto['kind']): boolean {
        return kind.startsWith('imported_');
    }

    async function commitAndContinue(): Promise<void> {
        const result = await controller.commitImport();
        if (result !== null) onCommitted?.(result);
    }

    function descriptionPreview(value: string): string {
        const trimmed = importedText(value).trim();
        if (!trimmed) return t('import.description.empty');
        const characters = Array.from(trimmed);
        const preview = characters.slice(0, 360).join('').trimEnd();
        return characters.length > 360 ? `${preview}…` : preview;
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
            <div class="modal-body">
                <div class="state-panel" role="status">
                    {appState.import_flow.resource_override_active
                        ? $tr('import.resource.inspecting')
                        : $tr('import.inspecting')}
                </div>
            </div>
        {:else if appState.import_flow.phase === 'error'}
            <div class="modal-body">
                <div class="state-panel error" role="alert">
                    <p>{appState.import_flow.error}</p>
                    {#if appState.import_flow.resource_override_available}
                        <p>{$tr('import.resource.retry_description')}</p>
                        <button
                            class="primary"
                            type="button"
                            onclick={() => void controller.beginImport()}
                            >{$tr('import.resource.retry')}</button
                        >
                    {/if}
                    <button type="button" onclick={() => void controller.discardImport()}>
                        {$tr('import.close')}
                    </button>
                </div>
            </div>
        {:else if appState.import_flow.inspection}
            {@const inspection = appState.import_flow.inspection}
            <div class="modal-body">
                <div class="review-summary">
                    <span class="review-avatar" aria-hidden="true"
                        >{inspection.display_name.slice(0, 1)}</span
                    >
                    <div>
                        <div class="review-title-row">
                            <h3>{importedText(inspection.display_name)}</h3>
                            <span class="review-kind-badge">{kindLabel(inspection.kind)}</span>
                        </div>
                        <p class="review-description">
                            {descriptionPreview(inspection.description)}
                        </p>
                    </div>
                </div>

                <dl class="metadata-grid">
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

                {#if appState.import_flow.resource_override_active}
                    <section class="issue-box warning" aria-labelledby="large-import-title">
                        <h3 id="large-import-title">{$tr('import.resource.title')}</h3>
                        <p>{$tr('import.resource.approved_limits')}</p>
                        <p>{$tr('import.resource.storage_warning')}</p>
                        <p>{$tr('import.resource.security_boundary')}</p>
                    </section>
                {/if}

                {#if isImportedContent(inspection.kind)}
                    <section class="issue-box info" aria-labelledby="compatibility-import-title">
                        <h3 id="compatibility-import-title">{$tr('import.compatibility.title')}</h3>
                        <p>{$tr('import.compatibility.destination')}</p>
                        <p>{$tr('import.compatibility.safety')}</p>
                        <p>{$tr('import.compatibility.update')}</p>
                    </section>
                {/if}

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
                                        count: inspection.dynamic_content
                                            .elevated_runtime_script_count,
                                    })}
                                </li>
                            {/if}
                            {#if inspection.dynamic_content.runtime_capabilities_declared}
                                <li>
                                    {$tr('import.dynamic.capabilities', {
                                        capabilities:
                                            inspection.dynamic_content.required_runtime_capabilities.join(
                                                ', ',
                                            ) || $tr('import.dynamic.capabilities.none'),
                                    })}
                                </li>
                            {:else if inspection.dynamic_content.runtime_script_count > 0}
                                <li>{$tr('import.dynamic.capabilities.legacy')}</li>
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
            </div>

            <footer class="modal-actions">
                <button type="button" onclick={() => void controller.discardImport()}
                    >{$tr('import.cancel')}</button
                >
                <button
                    class="primary"
                    type="button"
                    disabled={!inspection.allowed || regexReviewPhase === 'checking'}
                    onclick={() => void commitAndContinue()}
                >
                    {#if isImportedContent(inspection.kind)}
                        {$tr('import.commit.content')}
                    {:else if inspection.dynamic_content.runtime_script_count > 0 || inspection.dynamic_content.regex_rule_count > 0 || inspection.dynamic_content.custom_markup_present}
                        {$tr('import.commit.safe')}
                    {:else}
                        {$tr('import.commit')}
                    {/if}
                </button>
            </footer>
        {/if}
    </dialog>
</div>
