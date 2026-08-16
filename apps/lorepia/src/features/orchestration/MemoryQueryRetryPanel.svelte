<script lang="ts">
    import type { LorepiaAppController, MemoryQueryRetryState } from '../../app/app-controller';
    import type { MemoryQueryEmbeddingRetryCandidateDto } from '../../lib/ipc/contracts';

    interface Props {
        state: MemoryQueryRetryState;
        controller: LorepiaAppController;
        headingId: string;
    }

    let { state: retryState, controller, headingId }: Props = $props();
    let acknowledgedCandidateKey = $state<string | null>(null);

    const visible = $derived(
        retryState.phase === 'loading' ||
            retryState.error != null ||
            retryState.notice != null ||
            retryState.candidates.length > 0,
    );

    function candidateKey(candidate: MemoryQueryEmbeddingRetryCandidateDto): string {
        return `${candidate.id}:${String(candidate.revision)}`;
    }

    function statusLabel(candidate: MemoryQueryEmbeddingRetryCandidateDto): string {
        if (candidate.status === 'interrupted') return '결과를 알 수 없는 요청';
        if (candidate.status === 'failed') return '실패한 요청';
        if (candidate.status === 'cancelled') return '취소된 요청';
        return '이미 대기열에 들어간 요청';
    }

    async function retry(
        candidate: MemoryQueryEmbeddingRetryCandidateDto,
        acknowledgeUnknownOutcome: boolean,
    ): Promise<void> {
        const accepted = await controller.retryMemoryQueryEmbedding(
            candidate,
            acknowledgeUnknownOutcome,
        );
        if (accepted) acknowledgedCandidateKey = null;
    }

    $effect(() => {
        const acknowledged = acknowledgedCandidateKey;
        if (acknowledged === null) return;
        const stillPresent = retryState.candidates.some(
            (candidate) =>
                candidate.status === 'interrupted' && candidateKey(candidate) === acknowledged,
        );
        if (!stillPresent) acknowledgedCandidateKey = null;
    });
</script>

{#if visible}
    <section class="memory-query-retry" aria-labelledby={headingId}>
        <header>
            <div>
                <h3 id={headingId}>기억 검색 준비가 중단되었습니다</h3>
                <p>
                    준비 작업을 재시도한 뒤 원래 계획 미리보기 또는 메시지 전송·편집·재생성을 다시
                    실행하세요.
                </p>
            </div>
            <button
                type="button"
                disabled={retryState.phase === 'loading' || retryState.busy_id !== null}
                onclick={() => void controller.refreshMemoryQueryRetries()}
            >
                새로고침
            </button>
        </header>

        {#if retryState.notice != null}
            <p class="retry-notice" role="status">{retryState.notice}</p>
        {/if}
        {#if retryState.error != null}
            <p class="retry-error" role="alert">{retryState.error}</p>
        {:else if retryState.phase === 'loading' && retryState.candidates.length === 0}
            <p class="retry-loading" role="status">재시도할 준비 작업을 확인하는 중입니다.</p>
        {/if}

        {#if retryState.candidates.length > 0}
            <ul>
                {#each retryState.candidates as candidate, index (candidate.id)}
                    {@const key = candidateKey(candidate)}
                    {@const warningId = `${headingId}-unknown-${String(index)}`}
                    <li>
                        <div class="retry-summary">
                            <strong>{statusLabel(candidate)}</strong>
                            {#if candidate.error_code !== null}
                                <span>오류 코드: {candidate.error_code.slice(0, 256)}</span>
                            {/if}
                        </div>

                        {#if candidate.status === 'interrupted'}
                            {#if acknowledgedCandidateKey === key}
                                <p class="retry-warning" id={warningId}>
                                    이전 외부 제공자 요청의 결과를 확인할 수 없습니다. 같은 임베딩
                                    요청이 중복 처리될 수 있음을 확인하세요.
                                </p>
                                <button
                                    class="danger"
                                    type="button"
                                    aria-describedby={warningId}
                                    disabled={retryState.busy_id !== null}
                                    onclick={() => void retry(candidate, true)}
                                >
                                    위험을 확인하고 재시도
                                </button>
                            {:else}
                                <button
                                    class="danger"
                                    type="button"
                                    disabled={retryState.busy_id !== null}
                                    onclick={() => {
                                        acknowledgedCandidateKey = key;
                                    }}
                                >
                                    재시도 검토
                                </button>
                            {/if}
                        {:else}
                            <button
                                class="primary"
                                type="button"
                                disabled={retryState.busy_id !== null}
                                onclick={() => void retry(candidate, false)}
                            >
                                준비 작업 재시도
                            </button>
                        {/if}
                    </li>
                {/each}
            </ul>
        {/if}
    </section>
{/if}

<style>
    .memory-query-retry {
        display: grid;
        gap: 10px;
        padding: 12px;
        border: 1px solid color-mix(in srgb, var(--danger) 35%, var(--line));
        border-radius: 14px;
        background: var(--surface-muted);
    }

    header,
    li {
        display: flex;
        gap: 10px;
        align-items: center;
        justify-content: space-between;
    }

    h3,
    p {
        margin: 0;
    }

    header p,
    .retry-summary span,
    .retry-loading {
        color: var(--ink-muted);
        font-size: 0.75rem;
    }

    ul {
        display: grid;
        gap: 8px;
        margin: 0;
        padding: 0;
        list-style: none;
    }

    li {
        flex-wrap: wrap;
        padding: 10px;
        border: 1px solid var(--line);
        border-radius: 10px;
        background: var(--surface);
    }

    .retry-summary {
        display: grid;
        gap: 2px;
    }

    .retry-warning,
    .retry-error {
        width: 100%;
        color: var(--danger);
        font-size: 0.78rem;
    }

    .retry-notice {
        color: var(--success, var(--ink));
        font-size: 0.78rem;
    }

    @media (max-width: 720px) {
        header,
        li {
            align-items: stretch;
            flex-direction: column;
        }

        header > button,
        li > button {
            width: 100%;
        }
    }
</style>
