<script lang="ts">
    import type { LorepiaAppState } from '../../../app/app-controller';
    import type { OrchestrationState } from '../orchestration-controller';

    interface Props {
        appState: LorepiaAppState;
        orchestrationState: OrchestrationState;
        detailPage: string | null;
    }

    interface DisplayTransformDiagnosticItem {
        messageId: string;
        generationId: string;
        createdAt: string;
        canonicalContentSha256: string;
        displayContentSha256: string;
        diagnosticsSha256: string;
        diagnostics: NonNullable<
            LorepiaAppState['messages']['items'][number]['display_projection']
        >['diagnostics'];
    }

    const MAX_DISPLAY_PROJECTION_MESSAGES = 64;
    const MAX_DISPLAY_TRANSFORM_DIAGNOSTICS = 512;

    let { appState, orchestrationState, detailPage }: Props = $props();

    const displayTransformDiagnostics = $derived.by(() => {
        const items: DisplayTransformDiagnosticItem[] = [];
        let diagnosticCount = 0;
        let truncated = false;
        for (const message of [...appState.messages.items].reverse()) {
            const projection = message.display_projection;
            if (projection === undefined || message.generation_id === null) continue;
            if (
                items.length >= MAX_DISPLAY_PROJECTION_MESSAGES ||
                diagnosticCount + projection.diagnostics.length > MAX_DISPLAY_TRANSFORM_DIAGNOSTICS
            ) {
                truncated = true;
                continue;
            }
            items.push({
                messageId: message.id,
                generationId: message.generation_id,
                createdAt: message.created_at,
                canonicalContentSha256: projection.canonical_content_sha256,
                displayContentSha256: projection.display_content_sha256,
                diagnosticsSha256: projection.diagnostics_sha256,
                diagnostics: projection.diagnostics,
            });
            diagnosticCount += projection.diagnostics.length;
        }
        items.reverse();
        return { items, diagnosticCount, truncated };
    });
</script>

<!-- prettier-ignore-start -->

{#if detailPage === 'display'}
    <section
        class="studio-card diagnostic-flat" data-studio-owned-lists="" data-studio-owned-code=""
        aria-labelledby="display-transform-diagnostics-title"
    >
        <div class="section-heading">
            <div>
                <h3 id="display-transform-diagnostics-title">메시지 표시 변환 진단</h3>
                <p>
                    Core가 재오픈 시 해시를 검증한 DisplayOnly sidecar와 규칙 결과만
                    표시합니다. 정규 메시지 본문, 패턴, 치환문, 오류 원문은 이 진단에
                    포함하지 않습니다.
                </p>
            </div>
        </div>
        {#if displayTransformDiagnostics.items.length === 0}
            <p class="empty-note">현재 분기에 저장된 표시 변환 진단이 없습니다.</p>
        {:else}
            <p class="bounded-note" role="note">
                메시지 {displayTransformDiagnostics.items.length}개 · 진단
                {displayTransformDiagnostics.diagnosticCount}개
            </p>
            <ol class="message-preview-list">
                {#each displayTransformDiagnostics.items as item (item.messageId)}
                    <li>
                        <header>
                            <strong
                                >메시지 <code>{item.messageId.slice(0, 256)}</code
                                ></strong
                            >
                            <span>{item.createdAt}</span>
                        </header>
                        <small>
                            생성 <code>{item.generationId.slice(0, 256)}</code>
                        </small>
                        <dl class="state-list" data-studio-owned-definition="">
                            <div>
                                <dt>정규 내용 SHA-256</dt>
                                <dd><code>{item.canonicalContentSha256.slice(0, 64)}</code></dd>
                            </div>
                            <div>
                                <dt>표시 내용 SHA-256</dt>
                                <dd><code>{item.displayContentSha256.slice(0, 64)}</code></dd>
                            </div>
                            <div>
                                <dt>진단 SHA-256</dt>
                                <dd><code>{item.diagnosticsSha256.slice(0, 64)}</code></dd>
                            </div>
                        </dl>
                        {#if item.diagnostics.length === 0}
                            <p class="empty-note">
                                적용 규칙 또는 파이프라인 거부가 없습니다.
                            </p>
                        {:else}
                            <ol class="compact-list">
                                {#each item.diagnostics as diagnostic, index (`${item.messageId}:${String(index)}:${diagnostic.stage}:${diagnostic.rule_id ?? 'pipeline'}`)}
                                    <li>
                                        <strong
                                            >{diagnostic.stage} · {diagnostic.disposition}</strong
                                        >
                                        <span>
                                            set revision
                                            <code
                                                >{diagnostic.set_revision_id?.slice(0, 256) ??
                                                    'pipeline'}</code
                                            >
                                            · rule
                                            <code
                                                >{diagnostic.rule_id?.slice(0, 256) ??
                                                    'pipeline'}</code
                                            >
                                            · code
                                            <code>{diagnostic.code?.slice(0, 256) ?? 'none'}</code>
                                        </span>
                                        <small>
                                            before
                                            <code>{diagnostic.before_sha256.slice(0, 64)}</code> ·
                                            after
                                            <code
                                                >{diagnostic.after_sha256?.slice(0, 64) ??
                                                    'none'}</code
                                            >
                                            · {diagnostic.recorded_at}
                                        </small>
                                    </li>
                                {/each}
                            </ol>
                        {/if}
                    </li>
                {/each}
            </ol>
            {#if displayTransformDiagnostics.truncated}
                <p class="bounded-note">
                    최신 64개 메시지와 최대 512개 진단까지만 표시합니다.
                </p>
            {/if}
        {/if}
    </section>
{/if}

{#if detailPage === 'selection'}
    <section class="studio-card diagnostic-flat" aria-labelledby="selection-evidence-title">
        <div class="section-heading">
            <div>
                <h3 id="selection-evidence-title">현재 방의 지식·기억 선택 근거</h3>
                <p>
                    현재 분기 스냅샷에서 Core가 선택하거나 제외한 지식과 기억의 이유,
                    점수, 토큰, 삽입 위치를 표시합니다.
                </p>
            </div>
        </div>
        {#if orchestrationState.workspace.selection_evidence.length === 0}
            <p class="empty-note">현재 스냅샷에 선택 근거가 없습니다.</p>
        {:else}
            <ul class="evidence-list">
                {#each orchestrationState.workspace.selection_evidence as evidence (evidence.id)}
                    <li class:selected={evidence.selected}>
                        <strong>{evidence.title.slice(0, 512)}</strong>
                        <span>{evidence.reason.slice(0, 4096)}</span>
                        <small>
                            {evidence.source_kind} · {evidence.selected
                                ? '선택'
                                : '제외'} ·
                            {evidence.estimated_tokens} tokens · score
                            {evidence.score ?? '없음'} · 배치 {evidence.placement ??
                                '없음'}
                        </small>
                    </li>
                {/each}
            </ul>
        {/if}
        {#if orchestrationState.list_truncation.selection_evidence}
            <p class="bounded-note" role="note">
                안전한 UI 한도에 따라 처음 300개 선택 근거만 표시합니다. 전체 후보
                목록으로 해석하지 마세요.
            </p>
        {/if}
    </section>
{/if}

<!-- prettier-ignore-end -->
