<script lang="ts">
    import { onMount, untrack } from 'svelte';

    import DetailActionBar from '../../components/detail/DetailActionBar.svelte';
    import ChoiceField from '../../components/ChoiceField.svelte';

    import type {
        OrchestrationModuleScope,
        OrchestrationVariableRefDto,
        OrchestrationVariableValueDto,
    } from '../../lib/ipc/contracts';
    import {
        ContentModuleLifecycleController,
        INITIAL_CONTENT_MODULE_LIFECYCLE_STATE,
        contentModuleCandidateKey,
        contentModuleComponentKey,
        type ContentModuleLifecycleState,
    } from './module-lifecycle-controller';
    import type {
        ContentModuleCandidateSourceDto,
        ContentModuleComponentRefDto,
        ContentModuleLifecycleClientApi,
        ContentModuleRollbackBlockerDto,
    } from './module-lifecycle-contracts';

    const MAX_VISIBLE_HASH_APPROVAL_ITEMS = 100;
    const MAX_VISIBLE_VARIABLE_VALUE_CHARACTERS = 160;
    const MAX_VISIBLE_STRING_LIST_VALUES = 12;

    export type ModuleLifecycleSubpage =
        | 'modules'
        | 'modules:candidates'
        | 'modules:bindings'
        | 'modules:activation'
        | 'modules:deactivation'
        | 'modules:rollback';

    interface Props {
        client?: Partial<ContentModuleLifecycleClientApi>;
        conversationId: string | null;
        branchId: string | null;
        detailPage?: string | null;
    }

    let {
        client = {},
        conversationId,
        branchId,
        detailPage = $bindable<string | null | undefined>(),
    }: Props = $props();
    const controller = untrack(() => new ContentModuleLifecycleController(client));
    let lifecycleState = $state<ContentModuleLifecycleState>(
        structuredClone(INITIAL_CONTENT_MODULE_LIFECYCLE_STATE),
    );
    let contextKey = '';
    let rollbackPackageApprovalChoices = $state<Record<string, string>>({});
    let reviewReturnPage = $state<ModuleLifecycleSubpage>('modules:candidates');
    let previousLifecyclePage = $state<ModuleLifecycleSubpage>('modules');

    const busy = $derived(
        lifecycleState.phase === 'loading' ||
            lifecycleState.phase === 'reviewing' ||
            lifecycleState.phase === 'resolving' ||
            lifecycleState.phase === 'applying',
    );
    const activation = $derived(lifecycleState.activation);
    const rollback = $derived(lifecycleState.rollback);
    const deactivation = $derived(lifecycleState.deactivation);
    const activationConflictsResolved = $derived.by(
        () =>
            activation?.review?.review.conflicts.every(
                (conflict) =>
                    activation.conflict_choices[contentModuleComponentKey(conflict.component)] !==
                    undefined,
            ) ?? false,
    );
    const rollbackConflictsResolved = $derived.by(
        () =>
            rollback?.review?.review.activation.conflicts.every(
                (conflict) =>
                    rollback.conflict_choices[contentModuleComponentKey(conflict.component)] !==
                    undefined,
            ) ?? false,
    );
    const activationCanReview = $derived.by(() => {
        if (!activation?.candidate.local_use_allowed) return false;
        return (
            activation.candidate.source_kind !== 'imported_package' ||
            activation.request.binding.package_import_approval_id !== null
        );
    });
    const lifecyclePage = $derived.by<ModuleLifecycleSubpage>(() => {
        if (detailPage?.startsWith('modules:activation')) return 'modules:activation';
        return detailPage?.startsWith('modules:')
            ? (detailPage as ModuleLifecycleSubpage)
            : 'modules';
    });
    const legacyFlatMode = $derived(detailPage === undefined);

    $effect(() => {
        const nextKey =
            conversationId !== null && branchId !== null ? `${conversationId}:${branchId}` : '';
        if (nextKey === contextKey) return;
        contextKey = nextKey;
        rollbackPackageApprovalChoices = {};
        void controller.loadContext(conversationId, branchId);
    });

    $effect(() => {
        const nextPage = lifecyclePage;
        const leftReview =
            (previousLifecyclePage === 'modules:activation' ||
                previousLifecyclePage === 'modules:deactivation' ||
                previousLifecyclePage === 'modules:rollback') &&
            nextPage !== previousLifecyclePage;
        previousLifecyclePage = nextPage;
        if (leftReview) controller.clearReview();
    });

    onMount(() => {
        const unsubscribe = controller.state.subscribe((value) => {
            lifecycleState = value;
        });
        return () => {
            unsubscribe();
            controller.destroy();
        };
    });

    function shortHash(value: string): string {
        return value.length <= 24 ? value : `${value.slice(0, 12)}…${value.slice(-8)}`;
    }

    function componentLabel(component: ContentModuleComponentRefDto): string {
        const labels: Record<ContentModuleComponentRefDto['kind'], string> = {
            prompt_block: '프롬프트 블록',
            control: '컨트롤',
            knowledge_book: '지식 책',
            transform_set: '변환 세트',
            interaction_rule_set: '상호작용 규칙',
            asset: '에셋',
        };
        return `${labels[component.kind]} · ${component.id}`;
    }

    function boundedText(value: string, maximum = MAX_VISIBLE_VARIABLE_VALUE_CHARACTERS): string {
        const characters = Array.from(value);
        return characters.length <= maximum ? value : `${characters.slice(0, maximum).join('')}…`;
    }

    function sourceLabel(source: ContentModuleCandidateSourceDto): string {
        return `${source.module_id} / ${source.revision_id} · 소스 ${shortHash(source.revision_source_sha256)} · 바인딩 ${source.binding_id} · ${scopeLabel(source.scope)} · 우선순위 ${String(source.priority)} · 순서 ${String(source.module_ordinal)} · 런타임 의도 ${source.runtime_enabled_intent ? '켬' : '끔'}`;
    }

    function variableRefLabel(variable: OrchestrationVariableRefDto): string {
        return [variable.scope, variable.namespace, variable.id]
            .filter((part): part is string => part !== null)
            .join(':');
    }

    function variableValueLabel(value: OrchestrationVariableValueDto): string {
        switch (value.type) {
            case 'bool':
            case 'integer':
            case 'decimal':
                return String(value.value);
            case 'text':
            case 'enum':
                return boundedText(value.value);
            case 'string_list': {
                const visible = value.value
                    .slice(0, MAX_VISIBLE_STRING_LIST_VALUES)
                    .map((item) => boundedText(item, 48));
                const remainder = value.value.length - visible.length;
                return `[${visible.join(', ')}]${remainder > 0 ? ` 외 ${String(remainder)}개` : ''}`;
            }
        }
    }

    function scopeLabel(scope: OrchestrationModuleScope): string {
        const labels: Record<OrchestrationModuleScope, string> = {
            app: '앱',
            user: '로컬 사용자',
            persona: 'Persona',
            character: '캐릭터',
            conversation: '대화',
            branch: '브랜치',
        };
        return labels[scope];
    }

    function dispositionLabel(
        disposition: ContentModuleLifecycleState['bindings'][number]['disposition'],
    ): string {
        const labels = {
            applied: '적용됨',
            needs_reapproval: '새 리비전 재승인 필요',
            disabled: '비활성',
            awaiting_approval: '승인 대기',
        } as const;
        return labels[disposition];
    }

    function rollbackApprovalKey(bindingId: string, revisionId: string): string {
        return `${bindingId}:${revisionId}`;
    }

    function blockerLabel(blocker: ContentModuleRollbackBlockerDto): string {
        switch (blocker.kind) {
            case 'binding_disabled':
                return '바인딩이 비활성 상태입니다.';
            case 'binding_awaiting_approval':
                return '바인딩 승인이 아직 완료되지 않았습니다.';
            case 'stale_binding':
                return '바인딩 상태가 검토 이후 변경되었습니다.';
            case 'different_module':
                return '대상 리비전이 다른 모듈에 속합니다.';
            case 'target_already_active':
                return '대상 리비전이 이미 활성 상태입니다.';
            case 'target_not_ancestor':
                return '대상 리비전이 현재 리비전의 정확한 조상이 아닙니다.';
            case 'corrupt_revision_lineage':
                return '리비전 계보를 검증할 수 없습니다.';
            case 'corrupt_snapshot':
                return '불변 리비전 스냅샷을 검증할 수 없습니다.';
            case 'unsupported_schema_version':
                return `지원하지 않는 스키마 버전 ${String(blocker.schema_version)}입니다.`;
            case 'scope_target_missing':
                return '바인딩 범위 대상이 현재 컨텍스트에 없습니다.';
            case 'missing_asset':
                return `필요한 에셋이 없습니다: ${blocker.asset_id}`;
            case 'unsupported_capability':
                return `지원하지 않는 권한입니다: ${blocker.capability}`;
            case 'quarantined_target':
                return '격리된 리비전은 롤백 대상으로 사용할 수 없습니다.';
            case 'unresolved_conflict':
                return `해결되지 않은 충돌: ${componentLabel(blocker.component)}`;
        }
    }

    function openActivation(moduleId: string, bindingId?: string): void {
        controller.beginActivation(moduleId, bindingId);
        reviewReturnPage = bindingId ? 'modules:bindings' : 'modules:candidates';
        detailPage = bindingId ? 'modules:activation:bindings' : 'modules:activation';
    }

    function openDeactivation(bindingId: string): void {
        void controller.reviewDeactivation(bindingId);
        reviewReturnPage = 'modules:bindings';
        detailPage = 'modules:deactivation';
    }

    function openRollback(
        bindingId: string,
        revisionId: string,
        packageApprovalId: string | null,
    ): void {
        void controller.reviewRollback(bindingId, revisionId, packageApprovalId);
        reviewReturnPage = 'modules:bindings';
        detailPage = 'modules:rollback';
    }

    function closeReview(destination: ModuleLifecycleSubpage): void {
        detailPage = destination;
    }
</script>

{#snippet lifecycleContent()}
    <h3 id="module-lifecycle-title" class="sr-only">콘텐츠 모듈 활성화·롤백</h3>
    <p class="sr-only" aria-live="polite">{lifecycleState.announcement}</p>

    {#if conversationId === null || branchId === null}
        <p class="lifecycle-note">
            대화와 브랜치를 선택하면 모듈 활성화 상태를 검토할 수 있습니다.
        </p>
    {:else if lifecycleState.phase === 'loading'}
        <p role="status">모듈 후보와 활성 바인딩을 불러오는 중입니다.</p>
    {:else if lifecycleState.phase === 'unavailable'}
        <p class="lifecycle-note" role="note">{lifecycleState.error}</p>
    {:else if lifecycleState.error !== null}
        <p class="lifecycle-error" role="alert">{lifecycleState.error}</p>
    {/if}

    {#if lifecycleState.candidates_truncated || lifecycleState.bindings_truncated}
        <p class="lifecycle-note" role="note">
            안전한 검토를 위해 각 목록의 처음 100개만 표시합니다.
        </p>
    {/if}

    {#if !legacyFlatMode && lifecyclePage === 'modules'}
        <ul class="setting-list lifecycle-index" aria-label="콘텐츠 모듈 도구">
            <li>
                <button
                    class="setting-row lifecycle-index-row"
                    type="button"
                    onclick={() => (detailPage = 'modules:candidates')}
                >
                    <span class="setting-content">
                        <span class="setting-copy">
                            <strong>활성화 후보</strong>
                            <small>
                                불변 리비전 {lifecycleState.candidates.length}개를 검토하고
                                활성화합니다.
                            </small>
                        </span>
                    </span>
                </button>
            </li>
            <li>
                <button
                    class="setting-row lifecycle-index-row"
                    type="button"
                    onclick={() => (detailPage = 'modules:bindings')}
                >
                    <span class="setting-content">
                        <span class="setting-copy">
                            <strong>모듈 바인딩</strong>
                            <small>
                                저장된 바인딩 {lifecycleState.bindings.length}개와 롤백 가능한
                                리비전을 관리합니다.
                            </small>
                        </span>
                    </span>
                </button>
            </li>
        </ul>
    {/if}

    {#if (legacyFlatMode || lifecyclePage === 'modules:candidates') && lifecycleState.candidates.length > 0}
        <section aria-labelledby="module-candidates-title">
            <div class="subheading">
                <h4 id="module-candidates-title">활성화 후보</h4>
                <span>{lifecycleState.candidates.length}개</span>
            </div>
            <div class="candidate-grid">
                {#each lifecycleState.candidates as candidate (`${candidate.module_id}:${candidate.revision_id}`)}
                    <article class="candidate-card">
                        <header>
                            <div>
                                <strong>{candidate.name}</strong>
                                <span>
                                    v{candidate.version} · {candidate.source_kind} ·
                                    {candidate.component_count}개 구성요소
                                </span>
                            </div>
                            <code>{shortHash(candidate.revision_source_sha256)}</code>
                        </header>
                        <dl class="gate-grid">
                            <div>
                                <dt>로컬 사용</dt>
                                <dd class:allowed={candidate.local_use_allowed}>
                                    {candidate.local_use_allowed ? '허용' : '차단'}
                                </dd>
                            </div>
                            <div>
                                <dt>공유·재배포</dt>
                                <dd class:allowed={candidate.sharing_allowed}>
                                    {candidate.sharing_allowed ? '허용' : '차단'}
                                </dd>
                            </div>
                        </dl>
                        <p>
                            라이선스: <strong>{candidate.license}</strong>
                            {#if candidate.author}
                                · {candidate.author}{/if}
                        </p>
                        {#if candidate.share_reasons.length > 0}
                            <ul aria-label={`${candidate.name} 공유 판단 근거`}>
                                {#each candidate.share_reasons as reason (reason)}
                                    <li>{reason}</li>
                                {/each}
                            </ul>
                        {/if}
                        {#if candidate.required_capabilities.length > 0}
                            <p>요구 권한: {candidate.required_capabilities.join(', ')}</p>
                        {/if}
                        {#if candidate.source_kind === 'imported_package'}
                            <p class="lifecycle-note">
                                완료된 패키지 승인 {candidate.completed_package_approvals
                                    .length}개가 있습니다. 활성화 초안에서 하나를 직접 선택해야
                                합니다.
                            </p>
                        {/if}
                        <button
                            type="button"
                            disabled={busy ||
                                !candidate.local_use_allowed ||
                                candidate.source_kind === 'application_built_in'}
                            onclick={() => openActivation(candidate.module_id)}
                        >
                            {candidate.source_kind === 'application_built_in'
                                ? '앱 정책으로 사용자 활성화 불가'
                                : '이 불변 리비전 활성화 검토'}
                        </button>
                    </article>
                {/each}
            </div>
        </section>
    {:else if (legacyFlatMode || lifecyclePage === 'modules:candidates') && lifecycleState.phase === 'ready'}
        <p class="lifecycle-note">현재 컨텍스트에서 검토할 모듈 후보가 없습니다.</p>
    {/if}

    {#if (legacyFlatMode || lifecyclePage === 'modules:activation') && activation !== null}
        <section class="review-surface" aria-labelledby="module-activation-draft-title">
            <div class="subheading">
                <div>
                    <h4 id="module-activation-draft-title">
                        {activation.candidate.name} 활성화 초안
                    </h4>
                    <p>
                        바인딩 ID <code>{activation.request.binding.id}</code>는 이 검토와 재시도
                        동안 유지됩니다.
                    </p>
                </div>
            </div>
            <div class="draft-grid">
                <ChoiceField
                    id="module-activation-scope"
                    label="적용 범위"
                    value={activation.request.binding.scope}
                    options={lifecycleState.scope_targets.map((target) => ({
                        value: target.scope,
                        label: `${scopeLabel(target.scope)} · ${target.label}`,
                    }))}
                    disabled={busy}
                    onSelect={(value) =>
                        controller.setActivationScope(value as OrchestrationModuleScope)}
                />
                <label>
                    <span>우선순위</span>
                    <input
                        type="number"
                        min="-2147483648"
                        max="2147483647"
                        step="1"
                        value={activation.request.binding.priority}
                        disabled={busy}
                        onchange={(event) =>
                            controller.setActivationPriority(event.currentTarget.valueAsNumber)}
                    />
                </label>
                <ChoiceField
                    id="module-activation-resolution"
                    label="리비전 해석"
                    value={activation.request.binding.resolution_mode}
                    options={[
                        { value: 'pinned', label: '이 불변 리비전에 고정' },
                        { value: 'active', label: '활성 리비전 (검토 시 다시 검증)' },
                    ]}
                    disabled={busy}
                    onSelect={(value) =>
                        controller.setActivationResolutionMode(value as 'active' | 'pinned')}
                />
                {#if activation.candidate.source_kind === 'imported_package'}
                    <ChoiceField
                        id="module-activation-package-approval"
                        label="완료된 패키지 승인"
                        value={activation.request.binding.package_import_approval_id ?? ''}
                        options={[
                            { value: '', label: '명시적으로 선택하세요' },
                            ...activation.candidate.completed_package_approvals.map((approval) => ({
                                value: approval.approval_id,
                                label: `${approval.package_id} · ${approval.approval_id} · ${shortHash(approval.approval_sha256)}`,
                            })),
                        ]}
                        disabled={busy}
                        required
                        onSelect={(value: string) =>
                            controller.selectCompletedPackageApproval(value === '' ? null : value)}
                    />
                {/if}
            </div>
            <p>
                로컬 사용 {activation.candidate.local_use_allowed ? '허용' : '차단'} · 공유
                {activation.candidate.sharing_allowed ? '허용' : '차단'}
            </p>
            {#if !activation.candidate.sharing_allowed && activation.candidate.local_use_allowed}
                <p class="lifecycle-note">
                    공유가 차단되어도 로컬 사용 허용은 유지됩니다. 이 검토는 로컬 활성화만
                    승인합니다.
                </p>
            {/if}
            {#if activation.review !== null}
                {@const review = activation.review}
                <section class="hash-review" aria-labelledby="module-activation-review-title">
                    <h5 id="module-activation-review-title">Core 활성화 검토</h5>
                    <dl>
                        <div>
                            <dt>검토 SHA-256</dt>
                            <dd><code>{review.review.review_sha256}</code></dd>
                        </div>
                        <div>
                            <dt>상태 리비전</dt>
                            <dd>{review.review.state_revision}</dd>
                        </div>
                        <div>
                            <dt>대상 리비전</dt>
                            <dd>
                                <code>{review.proposed_revision.revision_id}</code> ·
                                <code
                                    >{shortHash(
                                        review.proposed_revision.revision_source_sha256,
                                    )}</code
                                >
                            </dd>
                        </div>
                    </dl>
                    {#if review.review.import_approvals.length > 0}
                        <h6>검토에 포함된 가져오기 권한</h6>
                        <ul class="authority-list" aria-label="검토에 포함된 가져오기 권한">
                            {#each review.review.import_approvals.slice(0, MAX_VISIBLE_HASH_APPROVAL_ITEMS) as approval, approvalIndex (`${approval.binding_id}:${approval.evidence.approval_id}:${String(approvalIndex)}`)}
                                <li>
                                    <strong>
                                        바인딩 <code>{approval.binding_id}</code> · 승인
                                        <code>{approval.evidence.approval_id}</code>
                                    </strong>
                                    <span>
                                        패키지 <code>{approval.evidence.package_id}</code> · import
                                        <code>{approval.evidence.import_id}</code> r{approval
                                            .evidence.import_revision}
                                    </span>
                                    <span>
                                        모듈 <code>{approval.evidence.module_id}</code> / 리비전
                                        <code>{approval.evidence.module_revision_id}</code>
                                    </span>
                                    <span>
                                        승인 근거 {shortHash(approval.evidence.approval_sha256)} · 패키지
                                        소스
                                        {shortHash(approval.evidence.package_source_sha256)} · 모듈 소스
                                        {shortHash(approval.evidence.module_revision_source_sha256)}
                                    </span>
                                    <span>
                                        승인 권한:
                                        {approval.evidence.authorized_capabilities.join(', ') ||
                                            '없음'}
                                    </span>
                                    <span>
                                        선택 패키지 구성요소:
                                        {approval.evidence.selected_package_component_ids
                                            .slice(0, MAX_VISIBLE_HASH_APPROVAL_ITEMS)
                                            .join(', ') || '없음'}
                                    </span>
                                    {#if approval.evidence.selected_package_component_ids.length > MAX_VISIBLE_HASH_APPROVAL_ITEMS}
                                        <span class="lifecycle-note">
                                            선택 구성요소는 처음 {MAX_VISIBLE_HASH_APPROVAL_ITEMS}개만
                                            표시합니다.
                                        </span>
                                    {/if}
                                    {#if approval.evidence.component_authorities.length > 0}
                                        <ul
                                            aria-label={`${approval.evidence.approval_id} 구성요소 권한`}
                                        >
                                            {#each approval.evidence.component_authorities.slice(0, MAX_VISIBLE_HASH_APPROVAL_ITEMS) as authority, authorityIndex (`${contentModuleComponentKey(authority.component)}:${String(authorityIndex)}`)}
                                                <li>
                                                    {componentLabel(authority.component)} →
                                                    <code
                                                        >{authority.committed_target_object_id}</code
                                                    >
                                                    /
                                                    <code
                                                        >{authority.committed_target_revision_id}</code
                                                    >
                                                </li>
                                            {/each}
                                        </ul>
                                    {/if}
                                    {#if approval.evidence.component_authorities.length > MAX_VISIBLE_HASH_APPROVAL_ITEMS}
                                        <span class="lifecycle-note">
                                            구성요소 권한은 처음 {MAX_VISIBLE_HASH_APPROVAL_ITEMS}개만
                                            표시합니다.
                                        </span>
                                    {/if}
                                </li>
                            {/each}
                        </ul>
                        {#if review.review.import_approvals.length > MAX_VISIBLE_HASH_APPROVAL_ITEMS}
                            <p class="lifecycle-note" role="note">
                                가져오기 권한은 처음 {MAX_VISIBLE_HASH_APPROVAL_ITEMS}개만
                                표시합니다.
                            </p>
                        {/if}
                    {/if}
                    {#if review.review.conflicts.length === 0}
                        <p class="lifecycle-note">명시적으로 해결할 구성요소 충돌이 없습니다.</p>
                    {:else}
                        <div class="conflict-list">
                            {#each review.review.conflicts as conflict (contentModuleComponentKey(conflict.component))}
                                <ChoiceField
                                    id={`module-activation-conflict-${contentModuleComponentKey(conflict.component)}`}
                                    label={`${componentLabel(conflict.component)} · ${conflict.reason}`}
                                    value={activation.conflict_choices[
                                        contentModuleComponentKey(conflict.component)
                                    ] ?? ''}
                                    options={[
                                        { value: '', label: '선택하세요' },
                                        ...conflict.candidates.map((candidate) => ({
                                            value: contentModuleCandidateKey(candidate),
                                            label: `${candidate.module_id} · ${candidate.revision_id} · ${shortHash(candidate.component_hash)}`,
                                        })),
                                        { value: 'omit', label: '모든 후보 명시적 제외' },
                                    ]}
                                    disabled={busy}
                                    required
                                    onSelect={(value: string) =>
                                        controller.chooseActivationConflict(
                                            conflict.component,
                                            value,
                                        )}
                                />
                            {/each}
                        </div>
                    {/if}
                </section>
            {/if}

            {#if activation.plan !== null}
                <section class="hash-review" aria-labelledby="module-activation-plan-title">
                    <h5 id="module-activation-plan-title">승인할 활성화 계획</h5>
                    <dl>
                        <div>
                            <dt>계획 SHA-256</dt>
                            <dd><code>{activation.plan.plan_sha256}</code></dd>
                        </div>
                        <div>
                            <dt>검토 SHA-256</dt>
                            <dd><code>{activation.plan.review_sha256}</code></dd>
                        </div>
                        <div>
                            <dt>적용 / 제외 구성요소</dt>
                            <dd>
                                {activation.plan.components.length} /
                                {activation.plan.omitted_components.length}
                            </dd>
                        </div>
                    </dl>
                    <section aria-labelledby="module-activation-components-title">
                        <h6 id="module-activation-components-title">
                            선택된 구성요소 원본과 런타임 효과
                        </h6>
                        {#if activation.plan.components.length === 0}
                            <p class="lifecycle-note">적용할 구성요소가 없습니다.</p>
                        {:else}
                            <ol class="plan-detail-list">
                                {#each activation.plan.components.slice(0, MAX_VISIBLE_HASH_APPROVAL_ITEMS) as component, componentIndex (`${contentModuleComponentKey(component.component)}:${String(componentIndex)}`)}
                                    <li>
                                        <strong>{componentLabel(component.component)}</strong>
                                        <span>구성요소 해시 {shortHash(component.sha256)}</span>
                                        <span>
                                            런타임 효과:
                                            {component.runtime_enabled
                                                ? '활성 — 런타임에 적용'
                                                : '비활성 — 저장하되 런타임 적용에서 제외'}
                                        </span>
                                        <span>
                                            선택 원본: {sourceLabel(component.selected_source)}
                                        </span>
                                        <span>
                                            병합 원본 {component.coalesced_sources.length}개
                                        </span>
                                        {#if component.coalesced_sources.length > 0}
                                            <ul
                                                aria-label={`${componentLabel(component.component)} 병합 원본`}
                                            >
                                                {#each component.coalesced_sources.slice(0, MAX_VISIBLE_HASH_APPROVAL_ITEMS) as source, sourceIndex (`${source.binding_id}:${source.module_id}:${source.revision_id}:${String(sourceIndex)}`)}
                                                    <li>{sourceLabel(source)}</li>
                                                {/each}
                                            </ul>
                                        {/if}
                                        {#if component.coalesced_sources.length > MAX_VISIBLE_HASH_APPROVAL_ITEMS}
                                            <span class="lifecycle-note">
                                                병합 원본은 처음 {MAX_VISIBLE_HASH_APPROVAL_ITEMS}개만
                                                표시합니다.
                                            </span>
                                        {/if}
                                    </li>
                                {/each}
                            </ol>
                        {/if}
                        {#if activation.plan.components.length > MAX_VISIBLE_HASH_APPROVAL_ITEMS}
                            <p class="lifecycle-note" role="note">
                                선택 구성요소는 처음 {MAX_VISIBLE_HASH_APPROVAL_ITEMS}개만
                                표시합니다.
                            </p>
                        {/if}
                    </section>

                    <section aria-labelledby="module-activation-omitted-title">
                        <h6 id="module-activation-omitted-title">명시적으로 제외된 구성요소</h6>
                        {#if activation.plan.omitted_components.length === 0}
                            <p class="lifecycle-note">명시적으로 제외된 구성요소가 없습니다.</p>
                        {:else}
                            <ul class="compact-list">
                                {#each activation.plan.omitted_components.slice(0, MAX_VISIBLE_HASH_APPROVAL_ITEMS) as component, componentIndex (`${contentModuleComponentKey(component)}:${String(componentIndex)}`)}
                                    <li>{componentLabel(component)}</li>
                                {/each}
                            </ul>
                        {/if}
                        {#if activation.plan.omitted_components.length > MAX_VISIBLE_HASH_APPROVAL_ITEMS}
                            <p class="lifecycle-note" role="note">
                                제외 구성요소는 처음 {MAX_VISIBLE_HASH_APPROVAL_ITEMS}개만
                                표시합니다.
                            </p>
                        {/if}
                    </section>

                    <section aria-labelledby="module-activation-variables-title">
                        <h6 id="module-activation-variables-title">효과적 변수 오버라이드</h6>
                        {#if activation.plan.effective_variable_overrides.values.length === 0}
                            <p class="lifecycle-note">효과적 변수 오버라이드가 없습니다.</p>
                        {:else}
                            <ul class="compact-list">
                                {#each activation.plan.effective_variable_overrides.values.slice(0, MAX_VISIBLE_HASH_APPROVAL_ITEMS) as entry, variableIndex (`${variableRefLabel(entry.variable)}:${String(variableIndex)}`)}
                                    <li>
                                        <code>{variableRefLabel(entry.variable)}</code> =
                                        {variableValueLabel(entry.value)}
                                    </li>
                                {/each}
                            </ul>
                        {/if}
                        {#if activation.plan.effective_variable_overrides.values.length > MAX_VISIBLE_HASH_APPROVAL_ITEMS}
                            <p class="lifecycle-note" role="note">
                                변수 오버라이드는 처음 {MAX_VISIBLE_HASH_APPROVAL_ITEMS}개만
                                표시합니다.
                            </p>
                        {/if}
                    </section>

                    <section aria-labelledby="module-activation-authority-title">
                        <h6 id="module-activation-authority-title">계획에 고정된 가져오기 권한</h6>
                        {#if activation.plan.import_approvals.length === 0}
                            <p class="lifecycle-note">가져오기 권한이 필요한 계획이 아닙니다.</p>
                        {:else}
                            <ul class="authority-list" aria-label="계획에 고정된 가져오기 권한">
                                {#each activation.plan.import_approvals.slice(0, MAX_VISIBLE_HASH_APPROVAL_ITEMS) as approval, approvalIndex (`${approval.binding_id}:${approval.evidence.approval_id}:${String(approvalIndex)}`)}
                                    <li>
                                        <strong>
                                            바인딩 <code>{approval.binding_id}</code> · 승인
                                            <code>{approval.evidence.approval_id}</code>
                                        </strong>
                                        <span>
                                            패키지 <code>{approval.evidence.package_id}</code> ·
                                            import <code>{approval.evidence.import_id}</code>
                                            r{approval.evidence.import_revision}
                                        </span>
                                        <span>
                                            모듈 <code>{approval.evidence.module_id}</code> /
                                            <code>{approval.evidence.module_revision_id}</code> ·
                                            권한
                                            {approval.evidence.authorized_capabilities.join(', ') ||
                                                '없음'}
                                        </span>
                                        <span>
                                            선택 패키지 구성요소
                                            {approval.evidence.selected_package_component_ids
                                                .length}개 · 구성요소 권한
                                            {approval.evidence.component_authorities.length}개
                                        </span>
                                    </li>
                                {/each}
                            </ul>
                        {/if}
                        {#if activation.plan.import_approvals.length > MAX_VISIBLE_HASH_APPROVAL_ITEMS}
                            <p class="lifecycle-note" role="note">
                                계획 권한은 처음 {MAX_VISIBLE_HASH_APPROVAL_ITEMS}개만 표시합니다.
                            </p>
                        {/if}
                    </section>
                    {#if activation.approval_id !== null}
                        <p class="lifecycle-note">
                            재시도 승인 ID <code>{activation.approval_id}</code>
                        </p>
                    {/if}
                </section>
            {/if}

            {#if activation.receipt !== null}
                <section class="receipt" aria-labelledby="module-activation-receipt-title">
                    <h5 id="module-activation-receipt-title">검증된 활성화 영수증</h5>
                    <p>
                        승인 <code>{activation.receipt.approval_id}</code> · 바인딩 CAS
                        {activation.receipt.binding.state_revision}
                    </p>
                    <p>
                        검토 <code>{shortHash(activation.receipt.review_sha256)}</code> · 계획
                        <code>{shortHash(activation.receipt.plan_sha256)}</code>
                    </p>
                </section>
            {/if}
        </section>
    {/if}

    {#if legacyFlatMode || lifecyclePage === 'modules:bindings'}
        <section aria-labelledby="active-module-bindings-title">
            <div class="subheading">
                <h4 id="active-module-bindings-title">저장된 모듈 바인딩과 불변 리비전</h4>
                <span>{lifecycleState.bindings.length}개</span>
            </div>
            {#if lifecycleState.bindings.length === 0}
                <p class="lifecycle-note">현재 컨텍스트와 관련된 저장 바인딩이 없습니다.</p>
            {:else}
                <div class="binding-list">
                    {#each lifecycleState.bindings as item (item.binding.binding.id)}
                        {@const reactivationCandidate = lifecycleState.candidates.find(
                            (candidate) => candidate.module_id === item.binding.binding.module_id,
                        )}
                        <article class="binding-card">
                            <header>
                                <div>
                                    <strong>{item.module_name}</strong>
                                    <span>
                                        {scopeLabel(item.binding.binding.scope)} ·
                                        {dispositionLabel(item.disposition)} · CAS
                                        {item.binding.state_revision}
                                    </span>
                                </div>
                                <code>{shortHash(item.revision_source_sha256)}</code>
                            </header>
                            <p>
                                현재 해석 리비전 <code>{item.binding.binding.revision_id}</code> ·
                                마지막 승인 리비전 <code>{item.approved_revision_id}</code>
                            </p>
                            {#if item.disposition === 'needs_reapproval'}
                                <p class="lifecycle-note" role="note">
                                    활성 리비전이 마지막 승인 이후 변경되었습니다. 새 리비전은 다시
                                    검토·승인하기 전까지 런타임에 적용되지 않습니다.
                                </p>
                            {/if}
                            {#if reactivationCandidate && reactivationCandidate.source_kind !== 'application_built_in'}
                                <button
                                    type="button"
                                    disabled={busy || !reactivationCandidate.local_use_allowed}
                                    onclick={() =>
                                        openActivation(
                                            reactivationCandidate.module_id,
                                            item.binding.binding.id,
                                        )}
                                >
                                    이 바인딩을 후보 리비전으로 다시 검토
                                </button>
                            {/if}
                            <button
                                type="button"
                                disabled={busy}
                                aria-label={`${item.module_name} 바인딩 비활성화 검토`}
                                onclick={() => openDeactivation(item.binding.binding.id)}
                            >
                                이 바인딩 비활성화 검토
                            </button>
                            {#if item.revisions_truncated}
                                <p class="lifecycle-note">최신 100개 리비전만 표시합니다.</p>
                            {/if}
                            <ul class="revision-list">
                                {#each item.revisions as revision (revision.revision_id)}
                                    {@const approvalKey = rollbackApprovalKey(
                                        item.binding.binding.id,
                                        revision.revision_id,
                                    )}
                                    {@const importedTarget =
                                        revision.source_kind === 'imported_package'}
                                    <li>
                                        <span>
                                            {revision.name} · v{revision.version} ·
                                            <code>{revision.revision_id}</code> ·
                                            {shortHash(revision.source_sha256)}
                                        </span>
                                        {#if revision.revision_id === item.approved_revision_id}
                                            <span class="current-badge">마지막 승인</span>
                                        {/if}
                                        {#if revision.active}
                                            <span class="current-badge">모듈 최신</span>
                                        {/if}
                                        {#if revision.revision_id !== item.approved_revision_id}
                                            {#if importedTarget}
                                                <ChoiceField
                                                    id={`module-rollback-approval-${approvalKey}`}
                                                    label="대상 리비전 패키지 승인"
                                                    value={rollbackPackageApprovalChoices[
                                                        approvalKey
                                                    ] ?? ''}
                                                    options={[
                                                        {
                                                            value: '',
                                                            label: '명시적으로 선택하세요',
                                                        },
                                                        ...revision.completed_package_approvals.map(
                                                            (approval) => ({
                                                                value: approval.approval_id,
                                                                label: `${approval.package_id} · ${approval.approval_id} · ${shortHash(approval.approval_sha256)}`,
                                                            }),
                                                        ),
                                                    ]}
                                                    disabled={busy || !revision.rollback_allowed}
                                                    required
                                                    onSelect={(value: string) => {
                                                        rollbackPackageApprovalChoices[
                                                            approvalKey
                                                        ] = value;
                                                    }}
                                                />
                                            {/if}
                                            <button
                                                type="button"
                                                disabled={busy ||
                                                    !revision.rollback_allowed ||
                                                    (importedTarget &&
                                                        !rollbackPackageApprovalChoices[
                                                            approvalKey
                                                        ])}
                                                aria-label={`${item.module_name} ${revision.revision_id} 롤백 검토`}
                                                onclick={() =>
                                                    openRollback(
                                                        item.binding.binding.id,
                                                        revision.revision_id,
                                                        importedTarget
                                                            ? (rollbackPackageApprovalChoices[
                                                                  approvalKey
                                                              ] ?? null)
                                                            : null,
                                                    )}
                                            >
                                                롤백 검토
                                            </button>
                                        {/if}
                                    </li>
                                {/each}
                            </ul>
                        </article>
                    {/each}
                </div>
            {/if}
        </section>
    {/if}

    {#if (legacyFlatMode || lifecyclePage === 'modules:deactivation') && deactivation?.review !== null && deactivation !== null}
        {@const deactivationReview = deactivation.review}
        <section class="review-surface" aria-labelledby="module-deactivation-review-title">
            <div class="subheading">
                <div>
                    <h4 id="module-deactivation-review-title">모듈 바인딩 비활성화 검토</h4>
                    <p>
                        {deactivation.binding.module_name} · 바인딩
                        <code>{deactivationReview.binding.id}</code>
                    </p>
                </div>
            </div>
            <dl class="hash-grid">
                <div>
                    <dt>비활성화 검토 SHA-256</dt>
                    <dd><code>{deactivationReview.review_sha256}</code></dd>
                </div>
                <div>
                    <dt>바인딩 CAS</dt>
                    <dd>{deactivationReview.expected_binding_revision}</dd>
                </div>
                <div>
                    <dt>적용 범위</dt>
                    <dd>{scopeLabel(deactivationReview.binding.scope)}</dd>
                </div>
                <div>
                    <dt>현재 상태</dt>
                    <dd>{dispositionLabel(deactivationReview.disposition)}</dd>
                </div>
                <div>
                    <dt>현재 해석 리비전</dt>
                    <dd><code>{deactivationReview.binding.revision_id}</code></dd>
                </div>
                <div>
                    <dt>마지막 승인 리비전</dt>
                    <dd><code>{deactivationReview.approved_revision_id}</code></dd>
                </div>
            </dl>
            <p class="lifecycle-note">
                이 작업은 현재 범위의 바인딩만 CAS로 삭제합니다. 모듈과 불변 리비전 자체는 삭제하지
                않습니다.
            </p>
            {#if deactivation.receipt !== null}
                <section class="receipt" aria-labelledby="module-deactivation-receipt-title">
                    <h5 id="module-deactivation-receipt-title">검증된 비활성화 영수증</h5>
                    <p>
                        바인딩 <code>{deactivation.receipt.binding.binding.id}</code> · 삭제 CAS
                        {deactivation.receipt.binding.state_revision}
                    </p>
                    <p>
                        검토 <code>{shortHash(deactivation.receipt.review.review_sha256)}</code> ·
                        삭제 시각
                        <time datetime={deactivation.receipt.deleted_at}
                            >{deactivation.receipt.deleted_at}</time
                        >
                    </p>
                </section>
            {/if}
        </section>
    {/if}

    {#if (legacyFlatMode || lifecyclePage === 'modules:rollback') && rollback?.review !== null && rollback !== null}
        {@const rollbackReview = rollback.review}
        <section class="review-surface" aria-labelledby="module-rollback-review-title">
            <div class="subheading">
                <div>
                    <h4 id="module-rollback-review-title">불변 리비전 롤백 검토</h4>
                    <p>
                        <code>{rollbackReview.review.rollback.current_revision_id}</code> →
                        <code>{rollbackReview.review.rollback.target_revision_id}</code>
                    </p>
                </div>
            </div>
            <dl class="hash-grid">
                <div>
                    <dt>롤백 검토 SHA-256</dt>
                    <dd><code>{rollbackReview.review.rollback.review_sha256}</code></dd>
                </div>
                <div>
                    <dt>활성화 검토 SHA-256</dt>
                    <dd><code>{rollbackReview.review.activation.review_sha256}</code></dd>
                </div>
                <div>
                    <dt>상태 리비전</dt>
                    <dd>{rollbackReview.review.rollback.expected_state_revision}</dd>
                </div>
                <div>
                    <dt>대상 리비전 SHA-256</dt>
                    <dd><code>{rollbackReview.review.rollback.target_source_sha256}</code></dd>
                </div>
            </dl>
            <p>
                대상 라이선스: {rollbackReview.target_revision.license} · 로컬 사용
                {rollbackReview.target_revision.local_use_allowed ? '허용' : '차단'} · 공유
                {rollbackReview.target_revision.sharing_allowed ? '허용' : '차단'}
            </p>
            {#if rollbackReview.review.rollback.diff}
                {@const diff = rollbackReview.review.rollback.diff}
                <section class="diff" aria-labelledby="module-rollback-diff-title">
                    <h5 id="module-rollback-diff-title">정확한 리비전 diff</h5>
                    <p>Diff SHA-256 <code>{diff.diff_sha256}</code></p>
                    <ul>
                        {#each diff.component_changes as change (`${contentModuleComponentKey(change.component)}:${change.kind}`)}
                            <li>
                                {change.kind} · {componentLabel(change.component)} ·
                                {change.previous_sha256
                                    ? shortHash(change.previous_sha256)
                                    : '없음'}
                                →
                                {change.next_sha256 ? shortHash(change.next_sha256) : '없음'}
                            </li>
                        {/each}
                    </ul>
                    {#if diff.metadata_changed_fields.length > 0}
                        <p>메타데이터 변경: {diff.metadata_changed_fields.join(', ')}</p>
                    {/if}
                    {#if diff.capability_changes.added.length > 0 || diff.capability_changes.removed.length > 0}
                        <p>
                            권한 추가: {diff.capability_changes.added.join(', ') || '없음'} · 제거:
                            {diff.capability_changes.removed.join(', ') || '없음'}
                        </p>
                    {/if}
                </section>
            {/if}
            {#if rollbackReview.review.rollback.blockers.length > 0}
                <ul class="blocker-list" aria-label="롤백 차단 사유">
                    {#each rollbackReview.review.rollback.blockers as blocker, index (`${String(index)}:${blocker.kind}`)}
                        <li>{blockerLabel(blocker)}</li>
                    {/each}
                </ul>
            {/if}
            {#if rollbackReview.review.activation.conflicts.length > 0}
                <div class="conflict-list">
                    {#each rollbackReview.review.activation.conflicts as conflict (contentModuleComponentKey(conflict.component))}
                        <ChoiceField
                            id={`module-rollback-conflict-${contentModuleComponentKey(conflict.component)}`}
                            label={`${componentLabel(conflict.component)} · ${conflict.reason}`}
                            value={rollback.conflict_choices[
                                contentModuleComponentKey(conflict.component)
                            ] ?? ''}
                            options={[
                                { value: '', label: '선택하세요' },
                                ...conflict.candidates.map((candidate) => ({
                                    value: contentModuleCandidateKey(candidate),
                                    label: `${candidate.module_id} · ${candidate.revision_id} · ${shortHash(candidate.component_hash)}`,
                                })),
                                { value: 'omit', label: '모든 후보 명시적 제외' },
                            ]}
                            disabled={busy}
                            required
                            onSelect={(value: string) =>
                                controller.chooseRollbackConflict(conflict.component, value)}
                        />
                    {/each}
                </div>
            {/if}
            {#if rollback.plan !== null}
                <section class="hash-review" aria-labelledby="module-rollback-plan-title">
                    <h5 id="module-rollback-plan-title">승인할 롤백 계획</h5>
                    <p>
                        롤백 <code>{rollback.plan.rollback.plan_sha256}</code>
                    </p>
                    <p>
                        재활성화 <code>{rollback.plan.activation.plan_sha256}</code>
                    </p>
                    {#if rollback.approval_id !== null}
                        <p class="lifecycle-note">
                            재시도 승인 ID <code>{rollback.approval_id}</code>
                        </p>
                    {/if}
                </section>
            {/if}

            {#if rollback.receipt !== null}
                <section class="receipt" aria-labelledby="module-rollback-receipt-title">
                    <h5 id="module-rollback-receipt-title">검증된 롤백 영수증</h5>
                    <p>
                        승인 <code>{rollback.receipt.approval_id}</code> · 바인딩 CAS
                        {rollback.receipt.binding.state_revision}
                    </p>
                    <p>
                        검토 <code>{shortHash(rollback.receipt.review_sha256)}</code> · 계획
                        <code>{shortHash(rollback.receipt.plan_sha256)}</code>
                    </p>
                </section>
            {/if}
        </section>
    {/if}

    {#if lifecyclePage === 'modules:activation' && activation === null}
        <p class="lifecycle-note">활성화 후보를 선택하면 검토 화면이 열립니다.</p>
    {:else if lifecyclePage === 'modules:deactivation' && deactivation?.review === null}
        <p class="lifecycle-note" role="status">비활성화 검토를 준비하고 있습니다.</p>
    {:else if lifecyclePage === 'modules:rollback' && rollback?.review === null}
        <p class="lifecycle-note" role="status">롤백 검토를 준비하고 있습니다.</p>
    {/if}
{/snippet}

{#snippet lifecycleActions()}
    {#if lifecyclePage === 'modules' || lifecyclePage === 'modules:candidates' || lifecyclePage === 'modules:bindings'}
        <DetailActionBar className="lifecycle-action-bar" ariaLabel="콘텐츠 모듈 목록 작업" fixed>
            <button
                class="primary detail-action detail-action--wide"
                type="button"
                disabled={busy || conversationId === null || branchId === null}
                onclick={() => void controller.loadContext(conversationId, branchId)}
            >
                후보·바인딩 새로고침
            </button>
        </DetailActionBar>
    {:else if lifecyclePage === 'modules:activation' && activation !== null}
        <DetailActionBar className="lifecycle-action-bar" ariaLabel="모듈 활성화 작업" fixed>
            <button
                class="detail-action"
                type="button"
                disabled={busy}
                onclick={() => closeReview(reviewReturnPage)}
            >
                닫기
            </button>
            {#if activation.plan !== null}
                <button
                    class="primary detail-action detail-action--grow"
                    type="button"
                    aria-label={activation.approval_id === null
                        ? '이 검토·계획 해시로 활성화 승인'
                        : '동일한 승인 ID로 결과 다시 확인'}
                    disabled={busy}
                    onclick={() => void controller.activateReviewedPlan()}
                >
                    {activation.approval_id === null ? '활성화 승인' : '결과 확인'}
                </button>
            {:else if activation.review !== null}
                <button
                    class="primary detail-action detail-action--grow"
                    type="button"
                    aria-label="선택한 충돌 해법으로 계획 만들기"
                    disabled={busy || !activationConflictsResolved}
                    onclick={() => void controller.resolveActivation()}
                >
                    계획 만들기
                </button>
            {:else}
                <button
                    class="primary detail-action detail-action--grow"
                    type="button"
                    aria-label="불변 리비전·라이선스·충돌 검토"
                    disabled={busy || !activationCanReview}
                    onclick={() => void controller.reviewActivation()}
                >
                    활성화 검토
                </button>
            {/if}
        </DetailActionBar>
    {:else if lifecyclePage === 'modules:deactivation' && deactivation?.review !== null && deactivation !== null}
        <DetailActionBar className="lifecycle-action-bar" ariaLabel="모듈 비활성화 작업" fixed>
            <button
                class="danger detail-action detail-action--destructive"
                type="button"
                aria-label="이 검토 해시로 바인딩 비활성화"
                disabled={busy || deactivation.receipt !== null}
                onclick={() => void controller.deactivateReviewedBinding()}
            >
                비활성화
            </button>
            <button
                class="detail-action detail-action--grow"
                type="button"
                disabled={busy}
                onclick={() => closeReview('modules:bindings')}
            >
                닫기
            </button>
        </DetailActionBar>
    {:else if lifecyclePage === 'modules:rollback' && rollback?.review !== null && rollback !== null}
        <DetailActionBar className="lifecycle-action-bar" ariaLabel="모듈 롤백 작업" fixed>
            {#if rollback.plan !== null}
                <button
                    class="danger detail-action detail-action--destructive"
                    type="button"
                    aria-label={rollback.approval_id === null
                        ? '이 롤백·활성화 해시로 승인'
                        : '동일한 승인 ID로 결과 다시 확인'}
                    disabled={busy}
                    onclick={() => void controller.applyReviewedRollback()}
                >
                    {rollback.approval_id === null ? '롤백 승인' : '결과 확인'}
                </button>
            {:else}
                <button
                    class="primary detail-action"
                    type="button"
                    aria-label="검토한 diff·충돌로 원자적 롤백 계획 만들기"
                    disabled={busy ||
                        !rollback.review.review.rollback.eligible ||
                        !rollbackConflictsResolved}
                    onclick={() => void controller.resolveRollback()}
                >
                    롤백 계획
                </button>
            {/if}
            <button
                class="detail-action detail-action--grow"
                type="button"
                disabled={busy}
                onclick={() => closeReview('modules:bindings')}
            >
                닫기
            </button>
        </DetailActionBar>
    {/if}
{/snippet}

<section class="module-lifecycle" aria-labelledby="module-lifecycle-title">
    {@render lifecycleContent()}
    {@render lifecycleActions()}
</section>

<style>
    .module-lifecycle {
        display: grid;
        width: 100%;
        min-width: 0;
        min-height: 0;
        padding: 0;
        border: 0;
        background: transparent;
        gap: 16px;
    }

    .lifecycle-index {
        width: 100%;
        margin: 0;
    }

    .lifecycle-index-row {
        min-height: clamp(62px, 17.849vw, 78px);
    }

    .lifecycle-index-row .setting-copy {
        display: grid;
        min-width: 0;
        gap: 5px;
        text-align: left;
    }

    .lifecycle-index-row :is(strong, small) {
        overflow: hidden;
        font-size: var(--detail-support-type);
        line-height: 1.35;
        text-overflow: ellipsis;
    }

    .lifecycle-index-row strong {
        color: var(--ink);
        font-weight: 550;
        white-space: nowrap;
    }

    .lifecycle-index-row small {
        display: -webkit-box;
        color: var(--ink-muted);
        font-weight: 550;
        overflow-wrap: anywhere;
        white-space: normal;
        line-clamp: 3;
        -webkit-box-orient: vertical;
        -webkit-line-clamp: 3;
    }

    .subheading,
    .candidate-card header,
    .binding-card header {
        display: flex;
        align-items: flex-start;
        justify-content: space-between;
        gap: 1rem;
    }

    .subheading h4,
    .hash-review h5,
    .diff h5,
    .receipt h5 {
        margin: 0;
    }

    .hash-review h6 {
        margin: 0.2rem 0 0;
    }

    .subheading p {
        margin: 0.35rem 0 0;
    }

    .candidate-grid,
    .binding-list {
        display: grid;
        gap: 0;
        margin-top: 8px;
    }

    .candidate-card,
    .binding-card {
        display: grid;
        gap: 12px;
        padding: 16px 0;
        border: 0;
        border-bottom: 1px solid var(--line);
        border-radius: 0;
        background: transparent;
    }

    .review-surface,
    .hash-review,
    .diff,
    .receipt {
        display: grid;
        min-width: 0;
        gap: 14px;
        padding: 0;
        border: 0;
        border-radius: 0;
        background: transparent;
    }

    .hash-review,
    .diff,
    .receipt {
        padding-top: 16px;
        border-top: 1px solid var(--line);
    }

    .candidate-card header div,
    .binding-card header div {
        display: grid;
        gap: 0.2rem;
    }

    .candidate-card p,
    .binding-card p,
    .review-surface p,
    .hash-review p,
    .diff p,
    .receipt p {
        margin: 0;
    }

    .gate-grid,
    .hash-grid,
    .hash-review dl {
        display: grid;
        grid-template-columns: 1fr;
        gap: 0;
        margin: 0;
    }

    .gate-grid div,
    .hash-grid div,
    .hash-review dl div {
        display: flex;
        min-width: 0;
        align-items: baseline;
        justify-content: space-between;
        padding: 10px 0;
        border-bottom: 1px solid var(--line);
        gap: 16px;
    }

    dt {
        color: var(--ink-muted);
        font-size: var(--detail-support-type);
        font-weight: 700;
    }

    dd {
        margin: 0;
        color: var(--ink);
        font-size: var(--detail-support-type);
        text-align: right;
        overflow-wrap: anywhere;
    }

    dd.allowed {
        color: var(--success);
    }

    .draft-grid,
    .conflict-list {
        display: grid;
        grid-template-columns: 1fr;
        gap: 14px;
    }

    .draft-grid label,
    .conflict-list label {
        display: grid;
        gap: 7px;
        color: var(--ink-muted);
        font-size: var(--detail-support-type);
        font-weight: 700;
    }

    input {
        width: 100%;
        min-width: 0;
        min-height: clamp(48px, 13.73vw, 60px);
        box-sizing: border-box;
        padding: clamp(12px, 3.432vw, 15px);
        border: 1.5px solid var(--line);
        border-radius: var(--radius-md);
        -webkit-appearance: none;
        appearance: none;
        background: color-mix(in srgb, var(--surface-sunken) 26%, var(--surface-raised));
        box-shadow: var(--control-inset-shadow);
        caret-color: var(--accent);
        color: var(--ink);
        font-size: var(--detail-support-type);
        line-height: 1.5;
        transition:
            background-color 140ms ease,
            box-shadow 140ms ease;
    }

    input:hover:not(:focus, :disabled) {
        border-color: var(--line);
    }

    input:focus {
        border-color: var(--accent);
        outline: none;
    }

    input:disabled {
        cursor: not-allowed;
        opacity: var(--disabled-opacity);
    }

    .revision-list {
        display: grid;
        gap: 0.5rem;
        margin: 0;
        padding: 0;
        list-style: none;
    }

    .plan-detail-list,
    .authority-list,
    .compact-list {
        display: grid;
        gap: 0.55rem;
        margin: 0.45rem 0 0;
        padding-left: 1.25rem;
    }

    .plan-detail-list > li,
    .authority-list > li {
        display: grid;
        gap: 0.3rem;
    }

    .plan-detail-list ul,
    .authority-list ul {
        margin: 0.2rem 0 0;
    }

    .revision-list li {
        display: grid;
        gap: 10px;
        padding-block: 12px;
        border-top: 1px solid var(--line);
    }

    .current-badge {
        padding: 0.2rem 0.55rem;
        border-radius: 999px;
        background: var(--surface-sunken);
        font-size: 0.82rem;
    }

    .lifecycle-note {
        color: var(--ink-muted);
    }

    .lifecycle-error,
    .blocker-list {
        padding: 10px 12px;
        border: 1px solid var(--status-error-border);
        border-radius: var(--radius-sm);
        color: var(--status-error-fg);
        background: var(--status-error-bg);
    }

    .receipt {
        color: var(--ink);
    }

    code {
        overflow-wrap: anywhere;
    }

    .candidate-card > button,
    .binding-card > button,
    .revision-list button {
        justify-self: stretch;
        min-height: 44px;
    }

    @container view (min-width: 700px) {
        .gate-grid,
        .hash-grid,
        .hash-review dl {
            grid-template-columns: repeat(2, minmax(0, 1fr));
            column-gap: 24px;
        }
    }
</style>
