<script lang="ts">
    import type {
        ApprovableContentPackageCapabilityDto,
        ContentPackageCapabilityDto,
        ContentPackageTargetReviewDocumentDto,
        LorepiaClient,
    } from '../../../lib/ipc/contracts';
    import DetailActionBar from '../../../components/detail/DetailActionBar.svelte';
    import ContentModuleLifecyclePanel from '../ContentModuleLifecyclePanel.svelte';
    import {
        MAX_COMPLETED_CONTENT_PACKAGE_EXPORTS,
        type ContentPackageController,
        type ContentPackageState,
    } from '../content-package-controller';
    import type { ContentModuleLifecycleClientApi } from '../module-lifecycle-contracts';
    import type { OrchestrationState } from '../orchestration-controller';
    import ContentPackageReview from './ContentPackageReview.svelte';

    interface Props {
        client?: LorepiaClient & Partial<ContentModuleLifecycleClientApi>;
        orchestrationState: OrchestrationState;
        contentPackageState?: ContentPackageState;
        contentPackageController?: ContentPackageController;
        detailPage?: string | null;
    }

    let {
        client,
        orchestrationState,
        contentPackageState,
        contentPackageController,
        detailPage = $bindable(null),
    }: Props = $props();

    const MAX_INLINE_ITEMS = 100;

    function packageCapabilityNeedsApproval(
        capability: ContentPackageCapabilityDto,
    ): capability is ApprovableContentPackageCapabilityDto {
        return capability === 'transforms' || capability === 'declarative_interactions';
    }

    function updateTargetConfirmed(document: ContentPackageTargetReviewDocumentDto): boolean {
        if (contentPackageState === undefined || document.disposition !== 'update') return false;
        return contentPackageState.confirmed_update_targets.some(
            (confirmation) =>
                confirmation.source_component_id === document.source_component_id &&
                confirmation.component_document_ordinal === document.component_document_ordinal &&
                confirmation.target_object_id === document.target_object_id &&
                confirmation.expected_target_revision_id === document.expected_target_revision_id &&
                confirmation.expected_target_state_revision ===
                    document.expected_target_state_revision,
        );
    }

    const packageUpdateTargetsConfirmed = $derived.by(() => {
        const targetReview = contentPackageState?.selection?.target_review;
        if (targetReview === undefined) return false;
        const updates = targetReview.documents.filter(
            (document) => document.disposition === 'update',
        );
        return (
            updates.length === contentPackageState?.confirmed_update_targets.length &&
            updates.every(updateTargetConfirmed)
        );
    });
    const packageBusy = $derived(
        contentPackageState?.phase === 'listing' ||
            contentPackageState?.phase === 'picking' ||
            contentPackageState?.phase === 'resuming' ||
            contentPackageState?.phase === 'selecting' ||
            contentPackageState?.phase === 'approving' ||
            contentPackageState?.phase === 'committing',
    );
    const packageCanReviewSelection = $derived.by(() => {
        const state = contentPackageState;
        const review = state?.inspection;
        return (
            state?.phase === 'ready' &&
            review !== null &&
            review !== undefined &&
            state.selected_component_ids.length > 0 &&
            review.local_import_allowed &&
            !review.issues.some((issue) => issue.severity === 'blocker')
        );
    });
    const packageCanApprove = $derived.by(() => {
        const state = contentPackageState;
        return (
            state?.phase === 'selection_ready' &&
            packageUpdateTargetsConfirmed &&
            !state.required_capabilities
                .filter(packageCapabilityNeedsApproval)
                .some((capability) => !state.approved_capabilities.includes(capability))
        );
    });
</script>

<!-- prettier-ignore-start -->

{#if detailPage === 'packages' && contentPackageState && contentPackageController}
    <section class="studio-card package-detail" data-studio-owned-lists="" data-studio-owned-code="" aria-labelledby="package-import-title">
        <div class="section-heading">
            <div>
                <h3 id="package-import-title">LorePia 패키지 선택 가져오기</h3>
                <p>
                    경로나 원본 바이트 없이 Core가 검사한 manifest, 라이선스, 충돌,
                    격리 결과만 검토합니다.
                </p>
            </div>
        </div>

        {#if contentPackageState.phase === 'unavailable'}
            <p class="bounded-note" role="note">{contentPackageState.error}</p>
        {:else if contentPackageState.phase === 'error'}
            <p class="drawer-status error" role="alert">
                {contentPackageState.error}
            </p>
        {:else if contentPackageState.phase === 'listing'}
            <p role="status">중단된 패키지 검토를 확인하고 있습니다.</p>
        {:else if contentPackageState.phase === 'picking'}
            <p role="status">
                패키지를 선택하고 Core에서 안전하게 검사하는 중입니다.
            </p>
        {:else if contentPackageState.phase === 'resuming'}
            <p role="status">중단된 패키지 검토를 다시 여는 중입니다.</p>
        {:else if contentPackageState.phase === 'selecting'}
            <p role="status">
                정규화 근거, 대상 쓰기 검토, 가져오기 계획을 계산하는 중입니다.
            </p>
        {:else if contentPackageState.phase === 'approving'}
            <p role="status">
                표시된 대상 쓰기 근거와 명시적 승인을 고정하는 중입니다.
            </p>
        {:else if contentPackageState.phase === 'committing'}
            <p role="status">승인된 패키지를 원자적으로 가져오는 중입니다.</p>
        {/if}

        <section aria-labelledby="completed-package-exports-title">
            <div class="section-heading">
                <div>
                    <h4 id="completed-package-exports-title">
                        완료된 패키지 내보내기
                    </h4>
                    <p>
                        재시작 후에도 Core가 다시 검증한 완료 패키지만 표시합니다.
                    </p>
                </div>
                <button
                    type="button"
                    disabled={contentPackageState.completed_exports_loading}
                    onclick={() => void contentPackageController.loadCompletedPackageExports()}
                >
                    목록 새로고침
                </button>
            </div>
            <div aria-live="polite" aria-atomic="true">
                {#if contentPackageState.completed_exports_loading}
                    <p role="status">완료된 패키지를 다시 검증하고 있습니다.</p>
                {/if}
                {#if contentPackageState.completed_exports_error}
                    <p class="drawer-status error" role="alert">
                        {contentPackageState.completed_exports_error}
                    </p>
                {/if}
            </div>
            {#if contentPackageState.completed_package_exports.length === 0}
                <p class="bounded-note">내보낼 수 있는 완료 패키지가 없습니다.</p>
            {:else}
                <ul class="compact-list" aria-label="완료된 패키지 내보내기 목록">
                    {#each contentPackageState.completed_package_exports.slice(0, MAX_COMPLETED_CONTENT_PACKAGE_EXPORTS) as descriptor (descriptor.source_id)}
                        <li>
                            <strong>{descriptor.suggested_file_name}</strong>
                            <span>크기 {descriptor.size_bytes}바이트</span>
                            <span>
                                SHA-256 <code>{descriptor.sha256}</code>
                            </span>
                            <button
                                type="button"
                                aria-label={`${descriptor.suggested_file_name} 완료 패키지 내보내기`}
                                disabled={contentPackageState.exporting_import_id !== null}
                                onclick={() =>
                                    void contentPackageController.exportCompletedPackageFromCatalog(
                                        descriptor.source_id,
                                    )}
                            >
                                {contentPackageState.exporting_import_id === descriptor.source_id
                                    ? '내보내는 중…'
                                    : '내보내기'}
                            </button>
                        </li>
                    {/each}
                </ul>
            {/if}
            {#if contentPackageState.completed_package_exports.length > MAX_COMPLETED_CONTENT_PACKAGE_EXPORTS}
                <p class="bounded-note" role="note">
                    처음 {MAX_COMPLETED_CONTENT_PACKAGE_EXPORTS}개 완료 패키지만
                    표시합니다.
                </p>
            {/if}
        </section>

        {#if contentPackageState.pending_imports.length > 0}
            <section aria-labelledby="pending-package-imports-title">
                <h4 id="pending-package-imports-title">중단된 검토</h4>
                <ul class="compact-list">
                    {#each contentPackageState.pending_imports.slice(0, MAX_INLINE_ITEMS) as pendingImport (pendingImport.import_id)}
                        <li>
                            <span>
                                {pendingImport.package_id} · {pendingImport.status} · revision
                                {pendingImport.revision}
                            </span>
                            <button
                                type="button"
                                disabled={contentPackageState.phase === 'picking' ||
                                    contentPackageState.phase === 'resuming' ||
                                    contentPackageState.phase === 'selecting' ||
                                    contentPackageState.phase === 'approving' ||
                                    contentPackageState.phase === 'committing'}
                                onclick={() =>
                                    void contentPackageController.resume(pendingImport.import_id)}
                            >
                                검토 재개
                            </button>
                        </li>
                    {/each}
                </ul>
            </section>
        {/if}

        {#if contentPackageState.result}
            <article class="revision-diff" aria-labelledby="package-result-title">
                <h4 id="package-result-title">가져오기 완료</h4>
                <p>
                    {contentPackageState.result.package_id} ·
                    {contentPackageState.result.status} · revision
                    {contentPackageState.result.revision}
                </p>
                <p>
                    문서 {contentPackageState.result.committed_document_ids
                        .length}개 · 자산 {contentPackageState.result.asset_ids
                        .length}개
                </p>
                <button
                    type="button"
                    disabled={contentPackageState.exporting_import_id !== null}
                    onclick={() => void contentPackageController.exportCompletedPackage()}
                >
                    {contentPackageState.exporting_import_id === null
                        ? '완료된 패키지 내보내기'
                        : '내보내는 중…'}
                </button>
            </article>
        {/if}

        <div aria-live="polite" aria-atomic="true">
            {#if contentPackageState.exporting_import_id !== null}
                <p role="status">운영체제 저장 위치를 선택하고 있습니다.</p>
            {/if}
            {#if contentPackageState.export_error}
                <p class="drawer-status error" role="alert">
                    {contentPackageState.export_error}
                </p>
            {/if}
            {#if contentPackageState.export_receipt}
                <article class="revision-diff" aria-labelledby="package-export-title">
                    <h4 id="package-export-title">최근 패키지 내보내기</h4>
                    <p>파일명 {contentPackageState.export_receipt.file_name}</p>
                    <p>
                        크기 {contentPackageState.export_receipt.size_bytes}바이트
                    </p>
                    <p>
                        SHA-256
                        <code>{contentPackageState.export_receipt.sha256}</code>
                    </p>
                </article>
            {/if}
        </div>

        <ContentPackageReview
            {contentPackageState}
            {contentPackageController}
            {updateTargetConfirmed}
        />

        <DetailActionBar fixed ariaLabel="LorePia 패키지 작업">
            {#if contentPackageState.inspection === null}
                <button
                    class="detail-action primary"
                    type="button"
                    aria-label="새 LorePia 패키지 선택"
                    disabled={packageBusy || contentPackageState.phase === 'unavailable'}
                    onclick={() => void contentPackageController.pickAndInspect()}
                >
                    패키지 선택
                </button>
            {:else}
                <button
                    class="detail-action detail-action--destructive danger"
                    type="button"
                    disabled={contentPackageState.phase === 'committing' ||
                        contentPackageState.phase === 'picking'}
                    onclick={() => void contentPackageController.discard()}
                >
                    검토 폐기
                </button>
                {#if contentPackageState.phase === 'selection_ready'}
                    <button
                        class="detail-action detail-action--grow primary"
                        type="button"
                        aria-label="표시된 근거와 기능 명시적 승인"
                        disabled={!packageCanApprove}
                        onclick={() => void contentPackageController.approve()}
                    >
                        명시적 승인
                    </button>
                {:else if contentPackageState.phase === 'approved'}
                    <button
                        class="detail-action detail-action--grow primary"
                        type="button"
                        aria-label="승인된 패키지 가져오기 확정"
                        onclick={() => void contentPackageController.commit()}
                    >
                        가져오기 확정
                    </button>
                {:else}
                    <button
                        class="detail-action detail-action--grow primary"
                        type="button"
                        aria-label="선택 및 정규화 검토"
                        disabled={!packageCanReviewSelection}
                        onclick={() => void contentPackageController.reviewSelection()}
                    >
                        선택 검토
                    </button>
                {/if}
            {/if}
        </DetailActionBar>
    </section>
{/if}

{#if detailPage === 'modules' || detailPage?.startsWith('modules:')}
    <ContentModuleLifecyclePanel
        {client}
        conversationId={orchestrationState.workspace.room_config.conversation_id || null}
        branchId={orchestrationState.workspace.room_config.branch_id || null}
        bind:detailPage
    />
{/if}

<!-- prettier-ignore-end -->
