<script lang="ts">
    import { onMount } from 'svelte';

    import type { LorepiaAppState, LorepiaAppController } from '../../app/app-controller';

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
                <h2 id="import-review-title">가져오기 검토</h2>
            </div>
            <button
                class="icon-button"
                type="button"
                aria-label="가져오기 검토 닫기"
                onclick={() => void controller.discardImport()}
            >
                ×
            </button>
        </header>

        {#if state.import_flow.phase === 'loading'}
            <div class="state-panel" role="status">로컬 파일을 안전하게 검사하는 중입니다.</div>
        {:else if state.import_flow.phase === 'error'}
            <div class="state-panel error" role="alert">
                <p>{state.import_flow.error}</p>
                <button type="button" onclick={() => void controller.discardImport()}>닫기</button>
            </div>
        {:else if state.import_flow.inspection}
            {@const inspection = state.import_flow.inspection}
            <div class="review-summary">
                <span class="review-avatar" aria-hidden="true"
                    >{inspection.display_name.slice(0, 1)}</span
                >
                <div>
                    <h3>{inspection.display_name}</h3>
                    <p>{inspection.description || '설명이 없습니다.'}</p>
                </div>
            </div>

            <dl class="metadata-grid">
                <div>
                    <dt>형식</dt>
                    <dd>{inspection.kind === 'charx_package' ? 'CHARX' : 'CCv3 JSON'}</dd>
                </div>
                <div>
                    <dt>원본 크기</dt>
                    <dd>{formatBytes(inspection.source_size)}</dd>
                </div>
                <div>
                    <dt>예상 저장 크기</dt>
                    <dd>{formatBytes(inspection.estimated_stored_size)}</dd>
                </div>
                <div>
                    <dt>에셋</dt>
                    <dd>{inspection.asset_count.toLocaleString()}개</dd>
                </div>
            </dl>

            {#if inspection.blocked_reasons.length > 0}
                <section class="issue-box blocked" aria-labelledby="blocked-title">
                    <h3 id="blocked-title">가져올 수 없음</h3>
                    <ul>
                        {#each inspection.blocked_reasons as reason (reason)}
                            <li>{reason}</li>
                        {/each}
                    </ul>
                </section>
            {/if}

            {#if inspection.warnings.length > 0}
                <section class="issue-box warning" aria-labelledby="warning-title">
                    <h3 id="warning-title">확인할 내용</h3>
                    <ul>
                        {#each inspection.warnings as warning (warning.code)}
                            <li>{warning.message}</li>
                        {/each}
                    </ul>
                </section>
            {/if}

            {#if inspection.unsupported_optional_fields.length > 0}
                <details>
                    <summary>아직 지원하지 않는 선택 필드</summary>
                    <p>{inspection.unsupported_optional_fields.join(', ')}</p>
                </details>
            {/if}

            <footer class="modal-actions">
                <button type="button" onclick={() => void controller.discardImport()}>취소</button>
                <button
                    class="primary"
                    type="button"
                    disabled={!inspection.allowed}
                    onclick={() => void controller.commitImport()}
                >
                    서재에 추가
                </button>
            </footer>
        {/if}
    </dialog>
</div>
