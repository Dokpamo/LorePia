<script lang="ts">
    import type {
        ApprovableContentPackageCapabilityDto,
        ContentPackageCapabilityDto,
        ContentPackageTargetReviewDocumentDto,
    } from '../../../lib/ipc/contracts';
    import {
        MAX_VISIBLE_CONTENT_PACKAGE_TARGET_DOCUMENTS,
        type ContentPackageController,
        type ContentPackageState,
    } from '../content-package-controller';

    interface Props {
        contentPackageState: ContentPackageState;
        contentPackageController: ContentPackageController;
        updateTargetConfirmed: (document: ContentPackageTargetReviewDocumentDto) => boolean;
    }

    let { contentPackageState, contentPackageController, updateTargetConfirmed }: Props = $props();

    const MAX_INLINE_ITEMS = 100;
    const MAX_MODULE_COMPONENTS = 200;

    function packageCapabilityNeedsApproval(
        capability: ContentPackageCapabilityDto,
    ): capability is ApprovableContentPackageCapabilityDto {
        return capability === 'transforms' || capability === 'declarative_interactions';
    }
</script>

<!-- prettier-ignore-start -->

{#if contentPackageState.inspection}
    {@const packageReview = contentPackageState.inspection}
    <article class="package-review" data-studio-owned-lists="" data-studio-owned-code="">
        <header>
            <div>
                <h4>
                    {packageReview.manifest.name} v{packageReview.manifest.version}
                </h4>
                <p>
                    {packageReview.manifest.package_id} ·
                    {packageReview.manifest.author ?? '작성자 정보 없음'}
                </p>
            </div>
            <span class="license-badge">
                {packageReview.manifest.license} ·
                {packageReview.redistribution_status}
            </span>
        </header>
        <dl class="plan-summary" data-studio-owned-definition="">
            <div>
                <dt>로컬 가져오기</dt>
                <dd>
                    {packageReview.local_import_allowed ? '허용' : '차단'}
                </dd>
            </div>
            <div>
                <dt>원본 크기</dt>
                <dd>{packageReview.source_size_bytes} bytes</dd>
            </div>
            <div>
                <dt>압축 해제 크기</dt>
                <dd>{packageReview.total_uncompressed_size_bytes} bytes</dd>
            </div>
            <div>
                <dt>자산</dt>
                <dd>{packageReview.asset_count}개</dd>
            </div>
            <div>
                <dt>검토 해시</dt>
                <dd><code>{packageReview.review_sha256}</code></dd>
            </div>
            <div>
                <dt>기능 검토 해시</dt>
                <dd>
                    <code>{packageReview.capability_review_sha256}</code>
                </dd>
            </div>
        </dl>

        <p>
            재배포 manifest:
            {packageReview.manifest.redistribution_allowed
                ? '허용'
                : '허용 안 됨'}
        </p>
        {#if packageReview.manifest.required_app_version}
            <p>
                요구 앱 버전: {packageReview.manifest.required_app_version}
            </p>
        {/if}
        {#if packageReview.manifest.required_capabilities.length > 0}
            <p>
                manifest 요구 기능:
                {packageReview.manifest.required_capabilities.slice(0, MAX_INLINE_ITEMS).join(', ')}
            </p>
        {/if}

        {#if packageReview.capability_decisions.length > 0}
            <h4>기능 지원 검토</h4>
            <ul class="conflict-list">
                {#each packageReview.capability_decisions.slice(0, MAX_INLINE_ITEMS) as decision (decision.capability)}
                    <li>
                        {decision.capability} · {decision.support} ·
                        {decision.approved ? '검토 통과' : '미승인'} ·
                        {decision.reason.slice(0, 4096)}
                    </li>
                {/each}
            </ul>
        {/if}

        <fieldset>
            <legend>가져올 구성요소</legend>
            {#each packageReview.components.slice(0, MAX_MODULE_COMPONENTS) as component (component.id)}
                <label class="component-choice">
                    <input
                        type="checkbox"
                        checked={contentPackageState.selected_component_ids.includes(component.id)}
                        disabled={contentPackageState.phase !== 'ready' ||
                            component.disposition !== 'importable'}
                        onchange={() => contentPackageController.toggleComponent(component.id)}
                    />
                    <span>
                        {component.id} · {component.kind}
                        <small>
                            {component.disposition} · 자산 {component.asset_count}개
                        </small>
                    </span>
                </label>
                {#if component.required_capabilities.length > 0}
                    <p>
                        요구 기능:
                        {component.required_capabilities.slice(0, MAX_INLINE_ITEMS).join(', ')}
                    </p>
                {/if}
                {#if component.dependency_ids.length > 0}
                    <p>
                        의존:
                        {component.dependency_ids.slice(0, MAX_INLINE_ITEMS).join(', ')}
                    </p>
                {/if}
                {#if component.conflict_ids.length > 0}
                    <p>
                        충돌:
                        {component.conflict_ids.slice(0, MAX_INLINE_ITEMS).join(', ')}
                    </p>
                {/if}
            {/each}
        </fieldset>
        {#if packageReview.components.length > MAX_MODULE_COMPONENTS}
            <p class="bounded-note">처음 200개 구성요소만 표시합니다.</p>
        {/if}

        {#if packageReview.issues.length > 0}
            <h4>검사 결과</h4>
            <ul class="conflict-list">
                {#each packageReview.issues.slice(0, MAX_INLINE_ITEMS) as issue, index (`${issue.severity}:${issue.code}:${String(index)}`)}
                    <li>
                        {issue.severity} · {issue.code} ·
                        <span>{issue.message.slice(0, 4096)}</span>
                    </li>
                {/each}
            </ul>
        {/if}

        {#if contentPackageState.selection}
            {@const packageSelection = contentPackageState.selection}
            <article class="revision-diff" aria-labelledby="package-normalization-title">
                <h4 id="package-normalization-title">
                    승인 전 정규화 근거
                </h4>
                <p>
                    아래 변경과 해시를 확인해야만 승인할 수 있습니다. 정규화
                    근거 해시
                    <code>{packageSelection.normalization_evidence_sha256}</code>
                </p>
                <p>
                    선택 계획
                    <code>{packageSelection.content_selection_plan_hash}</code>
                    · 가져오기 계획
                    <code>{packageSelection.import_plan_sha256}</code>
                </p>
                {#if packageSelection.normalization_evidence.length > 0}
                    <ul class="compact-list">
                        {#each packageSelection.normalization_evidence.slice(0, MAX_INLINE_ITEMS) as evidence (`${evidence.component_id}:${evidence.object_id}:${evidence.field}`)}
                            <li>
                                {evidence.component_id} / {evidence.object_id}
                                ·
                                {evidence.field}:
                                {evidence.before ? '켜짐' : '꺼짐'} →
                                {evidence.after ? '켜짐' : '꺼짐'} ·
                                {evidence.reason.slice(0, 4096)}
                            </li>
                        {/each}
                    </ul>
                {:else}
                    <p>활성 상태를 자동으로 낮춘 항목이 없습니다.</p>
                {/if}
            </article>

            <article class="revision-diff" aria-labelledby="package-target-review-title">
                <h4 id="package-target-review-title">대상 쓰기 검토</h4>
                <p>
                    대상 검토 SHA-256
                    <code>{packageSelection.target_review.target_review_sha256}</code>
                </p>
                <p class="bounded-note">
                    기존 대상을 갱신하는 문서는 대상 리비전과 상태 CAS를
                    각각 확인해야 합니다. 새 대상 생성은 별도 확인이
                    필요하지 않습니다.
                </p>
                {#if packageSelection.target_review.documents.length === 0}
                    <p>가져오기 계획에 쓸 문서 대상이 없습니다.</p>
                {:else}
                    <ol
                        class="compact-list"
                        aria-label="패키지 문서 대상 검토"
                    >
                        {#each packageSelection.target_review.documents.slice(0, MAX_VISIBLE_CONTENT_PACKAGE_TARGET_DOCUMENTS) as document (`${document.source_component_id}:${String(document.component_document_ordinal)}`)}
                            <li>
                                <strong>
                                    {document.source_component_id} · 전체 문서
                                    인덱스
                                    {document.document_index} · 구성요소 문서
                                    순서
                                    {document.component_document_ordinal}
                                </strong>
                                <span>
                                    소스 구성요소 SHA-256
                                    <code>{document.source_component_sha256}</code>
                                </span>
                                <span>
                                    종류 <code
                                        >{document.document_kind}</code
                                    >
                                    · 대상
                                    <code>{document.target_object_id}</code>
                                    · 처리
                                    {document.disposition}
                                </span>
                                <span>
                                    기대 불변 리비전
                                    <code
                                        >{document.expected_target_revision_id ??
                                            '없음'}</code
                                    >
                                    · 기대 상태 CAS
                                    {document.expected_target_state_revision ??
                                        '없음'}
                                </span>
                                <span>
                                    문서 SHA-256
                                    <code>{document.document_sha256}</code>
                                </span>
                                {#if document.disposition === 'update'}
                                    <label class="component-choice">
                                        <input
                                            type="checkbox"
                                            aria-label={`${document.target_object_id} 기존 대상 업데이트 확인`}
                                            checked={updateTargetConfirmed(document)}
                                            disabled={contentPackageState.phase !==
                                                'selection_ready'}
                                            onchange={() =>
                                                contentPackageController.toggleUpdateTargetConfirmation(
                                                    document.source_component_id,
                                                    document.component_document_ordinal,
                                                )}
                                        />
                                        <span>
                                            이 불변 리비전과 상태 CAS의 기존
                                            대상 업데이트 확인
                                        </span>
                                    </label>
                                {:else}
                                    <span class="bounded-note">
                                        새 대상 생성 — 별도 업데이트 확인
                                        불필요
                                    </span>
                                {/if}
                            </li>
                        {/each}
                    </ol>
                {/if}
                {#if packageSelection.target_review.documents.length > MAX_VISIBLE_CONTENT_PACKAGE_TARGET_DOCUMENTS}
                    <p class="bounded-note" role="note">
                        처음 {MAX_VISIBLE_CONTENT_PACKAGE_TARGET_DOCUMENTS}개
                        대상 문서만 표시합니다. 숨겨진 업데이트 대상이
                        있으면 승인할 수 없습니다.
                    </p>
                {/if}
            </article>

            <p>
                선택에서 요구된 기능:
                {contentPackageState.required_capabilities.length > 0
                    ? contentPackageState.required_capabilities.join(', ')
                    : '없음'}
            </p>

            <fieldset>
                <legend>가져온 뒤 활성화할 구성요소</legend>
                <p class="bounded-note">
                    선택하지 않은 구성요소도 가져오지만 비활성 상태를
                    유지합니다.
                </p>
                {#each contentPackageState.selected_component_ids as componentId (componentId)}
                    <label class="component-choice">
                        <input
                            type="checkbox"
                            checked={contentPackageState.enabled_component_ids.includes(
                                componentId,
                            )}
                            disabled={contentPackageState.phase !== 'selection_ready'}
                            onchange={() =>
                                contentPackageController.toggleEnabledComponent(componentId)}
                        />
                        <span>{componentId} 활성화</span>
                    </label>
                {/each}
            </fieldset>

            {@const approvalCapabilities = contentPackageState.required_capabilities.filter(
                packageCapabilityNeedsApproval,
            )}
            {#if approvalCapabilities.length > 0}
                <fieldset>
                    <legend>명시적으로 승인할 기능</legend>
                    {#each approvalCapabilities as capability (capability)}
                        <label class="component-choice">
                            <input
                                type="checkbox"
                                checked={contentPackageState.approved_capabilities.includes(
                                    capability,
                                )}
                                disabled={contentPackageState.phase !== 'selection_ready'}
                                onchange={() =>
                                    contentPackageController.toggleApprovedCapability(capability)}
                            />
                            <span>{capability} 기능 승인</span>
                        </label>
                    {/each}
                </fieldset>
            {:else}
                <p class="bounded-note">
                    추가 승인이 필요한 기능은 없습니다.
                </p>
            {/if}
        {/if}

        {#if contentPackageState.approval}
            <article class="revision-diff" aria-labelledby="package-approval-title">
                <h4 id="package-approval-title">고정된 명시적 승인</h4>
                <p>
                    승인 해시
                    <code>{contentPackageState.approval.approval_sha256}</code>
                </p>
                <p>
                    활성 구성요소:
                    {contentPackageState.approval.enabled_component_ids.length > 0
                        ? contentPackageState.approval.enabled_component_ids.join(', ')
                        : '없음'}
                </p>
                <p>
                    승인 기능:
                    {contentPackageState.approval.approved_capabilities.length > 0
                        ? contentPackageState.approval.approved_capabilities.join(', ')
                        : '없음'}
                </p>
            </article>
        {/if}
    </article>
{/if}

<!-- prettier-ignore-end -->
