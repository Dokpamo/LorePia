<script lang="ts">
    import { X } from '@lucide/svelte';
    import { onMount } from 'svelte';

    import type { LorepiaAppState, LorepiaAppController } from '../../app/app-controller';
    import { t, tr } from '../../lib/i18n';
    import type { ImportInspectionDto } from '../../lib/ipc/contracts';

    interface Props {
        state: LorepiaAppState;
        controller: LorepiaAppController;
    }

    let { state, controller }: Props = $props();
    let dialog: HTMLDialogElement;

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

        {#if state.import_flow.phase === 'loading'}
            <div class="state-panel" role="status">{$tr('import.inspecting')}</div>
        {:else if state.import_flow.phase === 'error'}
            <div class="state-panel error" role="alert">
                <p>{state.import_flow.error}</p>
                <button type="button" onclick={() => void controller.discardImport()}
                    >{$tr('import.close')}</button
                >
            </div>
        {:else if state.import_flow.inspection}
            {@const inspection = state.import_flow.inspection}
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
                    disabled={!inspection.allowed}
                    onclick={() => void controller.commitImport()}
                >
                    {$tr('import.commit')}
                </button>
            </footer>
        {/if}
    </dialog>
</div>
