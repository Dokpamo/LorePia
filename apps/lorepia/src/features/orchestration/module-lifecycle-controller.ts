import { get, writable, type Readable } from 'svelte/store';

import { normalizeClientError } from '../../lib/ipc/errors';
import type {
    ApplyContentModuleRollbackInput,
    CompletedContentPackageApprovalDto,
    ContentModuleActivationPlanDto,
    ContentModuleActivationReceiptDto,
    ContentModuleActivationRequestInput,
    ContentModuleActivationReviewPresentationDto,
    ContentModuleBindingSnapshotDto,
    ContentModuleDeactivationReceiptDto,
    ContentModuleDeactivationReviewDto,
    ContentModuleConflictCandidateDto,
    ContentModuleConflictResolutionInput,
    ContentModuleComponentRefDto,
    ContentModuleLifecycleBindingDto,
    ContentModuleLifecycleCandidateDto,
    ContentModuleLifecycleClientApi,
    ContentModuleLifecycleScopeTargetDto,
    ContentModuleRollbackPlanDto,
    ContentModuleRollbackReviewPresentationDto,
    ContentModuleRuntimeTargetInput,
    ReviewedContentModuleImportApprovalDto,
    ResolveContentModuleRollbackInput,
} from './module-lifecycle-contracts';

export const MAX_VISIBLE_LIFECYCLE_ITEMS = 100;

export type ModuleLifecyclePhase =
    | 'idle'
    | 'loading'
    | 'ready'
    | 'reviewing'
    | 'reviewed'
    | 'resolving'
    | 'resolved'
    | 'applying'
    | 'completed'
    | 'unavailable'
    | 'error';

export type ModuleConflictChoice = string;

export interface ContentModuleActivationState {
    candidate: ContentModuleLifecycleCandidateDto;
    request: ContentModuleActivationRequestInput;
    review: ContentModuleActivationReviewPresentationDto | null;
    plan: ContentModuleActivationPlanDto | null;
    conflict_choices: Record<string, ModuleConflictChoice>;
    approval_id: string | null;
    receipt: ContentModuleActivationReceiptDto | null;
}

export interface ContentModuleRollbackState {
    binding_id: string;
    target_revision_id: string;
    target_package_import_approval_id: string | null;
    review: ContentModuleRollbackReviewPresentationDto | null;
    plan: ContentModuleRollbackPlanDto | null;
    conflict_choices: Record<string, ModuleConflictChoice>;
    approval_id: string | null;
    receipt: ContentModuleActivationReceiptDto | null;
}

export interface ContentModuleDeactivationState {
    binding: ContentModuleLifecycleBindingDto;
    review: ContentModuleDeactivationReviewDto | null;
    receipt: ContentModuleDeactivationReceiptDto | null;
}

export interface ContentModuleLifecycleState {
    phase: ModuleLifecyclePhase;
    runtime_target: ContentModuleRuntimeTargetInput | null;
    scope_targets: ContentModuleLifecycleScopeTargetDto[];
    candidates: ContentModuleLifecycleCandidateDto[];
    bindings: ContentModuleLifecycleBindingDto[];
    candidates_truncated: boolean;
    bindings_truncated: boolean;
    activation: ContentModuleActivationState | null;
    rollback: ContentModuleRollbackState | null;
    deactivation: ContentModuleDeactivationState | null;
    error: string | null;
    announcement: string;
}

export const INITIAL_CONTENT_MODULE_LIFECYCLE_STATE: ContentModuleLifecycleState = {
    phase: 'idle',
    runtime_target: null,
    scope_targets: [],
    candidates: [],
    bindings: [],
    candidates_truncated: false,
    bindings_truncated: false,
    activation: null,
    rollback: null,
    deactivation: null,
    error: null,
    announcement: '',
};

function hasLifecycleApi(
    client: Partial<ContentModuleLifecycleClientApi>,
): client is ContentModuleLifecycleClientApi {
    return (
        client.listContentModuleLifecycleCandidates !== undefined &&
        client.listContentModuleLifecycleBindings !== undefined &&
        client.reviewContentModuleActivation !== undefined &&
        client.resolveContentModuleActivation !== undefined &&
        client.activateContentModule !== undefined &&
        client.reviewContentModuleRollback !== undefined &&
        client.resolveContentModuleRollback !== undefined &&
        client.applyContentModuleRollback !== undefined &&
        client.reviewContentModuleDeactivation !== undefined &&
        client.deactivateContentModule !== undefined
    );
}

function errorLabel(error: unknown): string {
    const normalized = normalizeClientError(error);
    switch (normalized.code) {
        case 'invalid_input':
            return '검토한 모듈·바인딩·패키지 승인 상태가 변경되었습니다. 최신 상태를 다시 검토해 주세요.';
        case 'not_found':
            return '콘텐츠 모듈, 바인딩 또는 불변 리비전을 찾을 수 없습니다.';
        case 'permission_denied':
            return '이 콘텐츠 모듈은 현재 범위에서 로컬 활성화할 수 없습니다.';
        default:
            return normalized.messageKey === 'error.unexpected'
                ? '콘텐츠 모듈 수명주기 작업을 완료하지 못했습니다.'
                : normalized.messageKey;
    }
}

export function contentModuleComponentKey(component: ContentModuleComponentRefDto): string {
    return `${component.kind}:${component.id}`;
}

export function contentModuleCandidateKey(candidate: ContentModuleConflictCandidateDto): string {
    return `${candidate.module_id}:${candidate.revision_id}:${candidate.component_hash}`;
}

function exactReceipt(
    receipt: ContentModuleActivationReceiptDto,
    approvalId: string,
    reviewSha256: string,
    planSha256: string,
    bindingId: string,
    moduleId: string,
    expectedRevisionId: string,
    expectedStateRevision: number,
): boolean {
    const nextStateRevision = expectedStateRevision + 1;
    return (
        receipt.verified &&
        Number.isSafeInteger(expectedStateRevision) &&
        expectedStateRevision >= 0 &&
        Number.isSafeInteger(nextStateRevision) &&
        receipt.binding.state_revision === nextStateRevision &&
        typeof receipt.binding.updated_at === 'string' &&
        receipt.binding.updated_at.length > 0 &&
        receipt.approval_id === approvalId &&
        receipt.review_sha256 === reviewSha256 &&
        receipt.plan_sha256 === planSha256 &&
        receipt.approved_plan.review_sha256 === reviewSha256 &&
        receipt.approved_plan.plan_sha256 === planSha256 &&
        receipt.binding.binding.id === bindingId &&
        receipt.binding.binding.module_id === moduleId &&
        receipt.binding.binding.revision_id === expectedRevisionId &&
        receipt.binding.binding.activation_approval_id === approvalId &&
        receipt.binding.binding.activation_review_sha256 === reviewSha256 &&
        receipt.binding.binding.activation_plan_sha256 === planSha256
    );
}

function exactRuntimeTarget(
    left: ContentModuleRuntimeTargetInput,
    right: ContentModuleRuntimeTargetInput,
): boolean {
    return left.conversation_id === right.conversation_id && left.branch_id === right.branch_id;
}

function exactCompletedImportApproval(
    returned: ReviewedContentModuleImportApprovalDto,
    selected: CompletedContentPackageApprovalDto,
    bindingId: string,
    moduleId: string,
    revisionId: string,
    revisionSourceSha256: string,
): boolean {
    const evidence = returned.evidence;
    return (
        returned.binding_id === bindingId &&
        selected.module_id === moduleId &&
        selected.module_revision_id === revisionId &&
        selected.module_revision_source_sha256 === revisionSourceSha256 &&
        evidence.approval_id === selected.approval_id &&
        evidence.approval_sha256 === selected.approval_sha256 &&
        evidence.import_id === selected.import_id &&
        evidence.import_revision === selected.import_revision &&
        evidence.package_id === selected.package_id &&
        evidence.package_source_sha256 === selected.package_source_sha256 &&
        evidence.selection_sha256 === selected.selection_sha256 &&
        evidence.capability_review_sha256 === selected.capability_review_sha256 &&
        evidence.module_id === moduleId &&
        evidence.module_revision_id === revisionId &&
        evidence.module_revision_source_sha256 === revisionSourceSha256
    );
}

function hasExactImportedActivationAuthority(
    activation: ContentModuleActivationState,
    review: ContentModuleActivationReviewPresentationDto,
): boolean {
    if (activation.candidate.source_kind !== 'imported_package') return true;
    const selectedApprovalId = activation.request.binding.package_import_approval_id;
    if (selectedApprovalId === null) return false;
    const selectedApprovals = activation.candidate.completed_package_approvals.filter(
        (approval) => approval.approval_id === selectedApprovalId,
    );
    const returnedAuthorities = review.review.import_approvals.filter(
        (approval) => approval.binding_id === activation.request.binding.id,
    );
    if (selectedApprovals.length !== 1 || returnedAuthorities.length !== 1) return false;
    const selected = selectedApprovals[0];
    const returned = returnedAuthorities[0];
    if (selected === undefined || returned === undefined) return false;
    return exactCompletedImportApproval(
        returned,
        selected,
        activation.request.binding.id,
        activation.candidate.module_id,
        activation.candidate.revision_id,
        activation.candidate.revision_source_sha256,
    );
}

function exactBindingSnapshot(
    left: ContentModuleBindingSnapshotDto,
    right: ContentModuleBindingSnapshotDto,
): boolean {
    return (
        left.id === right.id &&
        left.module_id === right.module_id &&
        left.scope === right.scope &&
        left.target_id === right.target_id &&
        left.conversation_id === right.conversation_id &&
        left.priority === right.priority &&
        left.resolution_mode === right.resolution_mode &&
        left.pinned_revision_id === right.pinned_revision_id &&
        left.enabled === right.enabled &&
        left.approved === right.approved &&
        left.package_import_approval_id === right.package_import_approval_id &&
        left.activation_approval_id === right.activation_approval_id &&
        left.activation_review_sha256 === right.activation_review_sha256 &&
        left.activation_plan_sha256 === right.activation_plan_sha256 &&
        JSON.stringify(left.variable_overrides) === JSON.stringify(right.variable_overrides) &&
        left.revision_id === right.revision_id &&
        left.created_at === right.created_at
    );
}

function exactDeactivationReview(
    left: ContentModuleDeactivationReviewDto,
    right: ContentModuleDeactivationReviewDto,
): boolean {
    return (
        left.review_sha256 === right.review_sha256 &&
        exactRuntimeTarget(left.runtime_target, right.runtime_target) &&
        exactBindingSnapshot(left.binding, right.binding) &&
        left.approved_revision_id === right.approved_revision_id &&
        left.expected_binding_revision === right.expected_binding_revision &&
        left.binding_updated_at === right.binding_updated_at &&
        left.disposition === right.disposition
    );
}

function exactDeactivationReceipt(
    receipt: ContentModuleDeactivationReceiptDto,
    review: ContentModuleDeactivationReviewDto,
): boolean {
    const nextRevision = review.expected_binding_revision + 1;
    return (
        receipt.verified &&
        exactDeactivationReview(receipt.review, review) &&
        Number.isSafeInteger(review.expected_binding_revision) &&
        review.expected_binding_revision >= 0 &&
        Number.isSafeInteger(nextRevision) &&
        receipt.binding.state_revision === nextRevision &&
        exactBindingSnapshot(receipt.binding.binding, review.binding) &&
        typeof receipt.deleted_at === 'string' &&
        receipt.deleted_at.length > 0 &&
        typeof receipt.binding.updated_at === 'string' &&
        receipt.binding.updated_at.length > 0
    );
}

function conflictResolutions(
    review: ContentModuleActivationReviewPresentationDto['review'],
    choices: Record<string, ModuleConflictChoice>,
): ContentModuleConflictResolutionInput[] | null {
    const resolutions: ContentModuleConflictResolutionInput[] = [];
    for (const conflict of review.conflicts) {
        const choice = choices[contentModuleComponentKey(conflict.component)];
        if (choice === undefined) return null;
        const selected =
            choice === 'omit'
                ? null
                : (conflict.candidates.find(
                      (candidate) => contentModuleCandidateKey(candidate) === choice,
                  ) ?? undefined);
        if (selected === undefined) return null;
        resolutions.push({
            component: structuredClone(conflict.component),
            expected_candidates: structuredClone(conflict.candidates),
            selected: selected === null ? null : structuredClone(selected),
        });
    }
    return resolutions;
}

export class ContentModuleLifecycleController {
    private readonly mutable = writable<ContentModuleLifecycleState>(
        structuredClone(INITIAL_CONTENT_MODULE_LIFECYCLE_STATE),
    );
    readonly state: Readable<ContentModuleLifecycleState> = this.mutable;

    private operationEpoch = 0;
    private readonly available: boolean;

    constructor(private readonly client: Partial<ContentModuleLifecycleClientApi>) {
        this.available = hasLifecycleApi(client);
        if (!this.available) {
            const message =
                '현재 Core가 해시로 고정된 콘텐츠 모듈 활성화·롤백 API(비활성화 포함)를 제공하지 않습니다.';
            this.mutable.set({
                ...structuredClone(INITIAL_CONTENT_MODULE_LIFECYCLE_STATE),
                phase: 'unavailable',
                error: message,
                announcement: message,
            });
        }
    }

    private update(
        updater: (state: ContentModuleLifecycleState) => ContentModuleLifecycleState,
    ): void {
        this.mutable.update(updater);
    }

    private isCurrent(epoch: number): boolean {
        return epoch === this.operationEpoch;
    }

    private fail(error: unknown, epoch = this.operationEpoch): false {
        if (!this.isCurrent(epoch)) return false;
        const message = errorLabel(error);
        this.update((state) => ({
            ...state,
            phase: 'error',
            error: message,
            announcement: message,
        }));
        return false;
    }

    private failInvariant(message: string): false {
        this.update((state) => ({
            ...state,
            phase: 'error',
            error: message,
            announcement: message,
        }));
        return false;
    }

    async loadContext(conversationId: string | null, branchId: string | null): Promise<boolean> {
        const epoch = ++this.operationEpoch;
        if (conversationId === null || branchId === null) {
            this.mutable.set({
                ...structuredClone(INITIAL_CONTENT_MODULE_LIFECYCLE_STATE),
                phase: this.available ? 'idle' : 'unavailable',
                error: this.available
                    ? null
                    : '현재 Core가 해시로 고정된 콘텐츠 모듈 활성화·롤백 API(비활성화 포함)를 제공하지 않습니다.',
            });
            return false;
        }
        if (!this.available || !hasLifecycleApi(this.client)) return false;
        const runtimeTarget = {
            conversation_id: conversationId,
            branch_id: branchId,
        };
        this.update((state) => ({
            ...state,
            phase: 'loading',
            runtime_target: runtimeTarget,
            activation: null,
            rollback: null,
            deactivation: null,
            error: null,
        }));
        try {
            const [candidates, bindings] = await Promise.all([
                this.client.listContentModuleLifecycleCandidates({
                    runtime_target: runtimeTarget,
                    limit: MAX_VISIBLE_LIFECYCLE_ITEMS,
                }),
                this.client.listContentModuleLifecycleBindings({
                    runtime_target: runtimeTarget,
                    limit: MAX_VISIBLE_LIFECYCLE_ITEMS,
                }),
            ]);
            if (!this.isCurrent(epoch)) return false;
            if (
                candidates.items.length > MAX_VISIBLE_LIFECYCLE_ITEMS ||
                bindings.items.length > MAX_VISIBLE_LIFECYCLE_ITEMS
            ) {
                return this.failInvariant(
                    'Core의 콘텐츠 모듈 목록이 화면의 안전한 표시 한도를 초과했습니다.',
                );
            }
            this.mutable.set({
                phase: 'ready',
                runtime_target: runtimeTarget,
                scope_targets: structuredClone(candidates.scope_targets),
                candidates: structuredClone(candidates.items),
                bindings: structuredClone(bindings.items),
                candidates_truncated: candidates.truncated,
                bindings_truncated: bindings.truncated,
                activation: null,
                rollback: null,
                deactivation: null,
                error: null,
                announcement: '콘텐츠 모듈 후보와 활성 바인딩을 다시 불러왔습니다.',
            });
            return true;
        } catch (error: unknown) {
            return this.fail(error, epoch);
        }
    }

    private async refreshWorkspaceAfterReceipt(
        epoch: number,
        runtimeTarget: ContentModuleRuntimeTargetInput,
    ): Promise<void> {
        if (!hasLifecycleApi(this.client)) return;
        try {
            const [candidates, bindings] = await Promise.all([
                this.client.listContentModuleLifecycleCandidates({
                    runtime_target: structuredClone(runtimeTarget),
                    limit: MAX_VISIBLE_LIFECYCLE_ITEMS,
                }),
                this.client.listContentModuleLifecycleBindings({
                    runtime_target: structuredClone(runtimeTarget),
                    limit: MAX_VISIBLE_LIFECYCLE_ITEMS,
                }),
            ]);
            if (
                !this.isCurrent(epoch) ||
                candidates.items.length > MAX_VISIBLE_LIFECYCLE_ITEMS ||
                bindings.items.length > MAX_VISIBLE_LIFECYCLE_ITEMS
            ) {
                return;
            }
            this.update((state) => ({
                ...state,
                scope_targets: structuredClone(candidates.scope_targets),
                candidates: structuredClone(candidates.items),
                bindings: structuredClone(bindings.items),
                candidates_truncated: candidates.truncated,
                bindings_truncated: bindings.truncated,
            }));
        } catch {
            // The verified receipt remains authoritative. A manual refresh can
            // retry this reader-only projection without replaying the mutation.
        }
    }

    beginActivation(moduleId: string, bindingId: string | null = null): boolean {
        const state = get(this.mutable);
        const candidate = state.candidates.find((item) => item.module_id === moduleId);
        const runtimeTarget = state.runtime_target;
        const existingBinding =
            bindingId === null
                ? null
                : (state.bindings.find((item) => item.binding.binding.id === bindingId) ?? null);
        if (candidate === undefined || runtimeTarget === null) return false;
        if (
            existingBinding !== null &&
            existingBinding.binding.binding.module_id !== candidate.module_id
        ) {
            return this.failInvariant('선택한 바인딩과 모듈 후보가 일치하지 않습니다.');
        }
        if (candidate.source_kind === 'application_built_in') {
            return this.failInvariant(
                '앱 내장 모듈은 제품 정책에 따라 사용자 활성화 바인딩을 만들 수 없습니다.',
            );
        }
        if (!candidate.local_use_allowed) {
            return this.failInvariant(
                '이 리비전은 로컬 사용이 허용되지 않아 활성화 검토를 시작할 수 없습니다.',
            );
        }
        const existingValue = existingBinding?.binding.binding;
        const scopeTarget =
            state.scope_targets.find(
                (target) =>
                    target.scope === existingValue?.scope &&
                    target.target_id === existingValue.target_id &&
                    target.conversation_id === existingValue.conversation_id,
            ) ??
            state.scope_targets.find((target) => target.scope === 'branch') ??
            state.scope_targets.find((target) => target.scope === 'conversation') ??
            state.scope_targets.find((target) => target.scope === 'user') ??
            state.scope_targets[0];
        if (scopeTarget === undefined) {
            return this.failInvariant(
                'Core가 이 대화에 사용할 수 있는 모듈 범위를 제공하지 않았습니다.',
            );
        }
        const request: ContentModuleActivationRequestInput = {
            runtime_target: structuredClone(runtimeTarget),
            expected_binding_revision: existingBinding?.binding.state_revision ?? null,
            binding: {
                id: existingValue?.id ?? globalThis.crypto.randomUUID(),
                module_id: candidate.module_id,
                scope: scopeTarget.scope,
                target_id: scopeTarget.target_id,
                conversation_id: scopeTarget.conversation_id,
                priority: existingValue?.priority ?? 0,
                resolution_mode: 'pinned',
                pinned_revision_id: candidate.revision_id,
                package_import_approval_id: null,
                variable_overrides: structuredClone(
                    existingValue?.variable_overrides ?? { values: [] },
                ),
            },
        };
        this.update((current) => ({
            ...current,
            phase: 'ready',
            activation: {
                candidate: structuredClone(candidate),
                request,
                review: null,
                plan: null,
                conflict_choices: {},
                approval_id: null,
                receipt: null,
            },
            rollback: null,
            deactivation: null,
            error: null,
            announcement: `${candidate.name} 모듈의 활성화 초안을 만들었습니다.`,
        }));
        return true;
    }

    private updateActivationDraft(
        updater: (request: ContentModuleActivationRequestInput) => void,
    ): boolean {
        const state = get(this.mutable);
        if (state.activation === null) return false;
        const request = structuredClone(state.activation.request);
        updater(request);
        this.update((current) => ({
            ...current,
            phase: 'ready',
            activation:
                current.activation === null
                    ? null
                    : {
                          ...current.activation,
                          request,
                          review: null,
                          plan: null,
                          conflict_choices: {},
                          approval_id: null,
                          receipt: null,
                      },
            error: null,
        }));
        return true;
    }

    setActivationScope(scope: ContentModuleLifecycleScopeTargetDto['scope']): boolean {
        const state = get(this.mutable);
        const target = state.scope_targets.find((candidate) => candidate.scope === scope);
        if (target === undefined) return false;
        return this.updateActivationDraft((request) => {
            request.binding.scope = target.scope;
            request.binding.target_id = target.target_id;
            request.binding.conversation_id = target.conversation_id;
        });
    }

    setActivationPriority(priority: number): boolean {
        if (
            !Number.isSafeInteger(priority) ||
            priority < -2_147_483_648 ||
            priority > 2_147_483_647
        ) {
            return false;
        }
        return this.updateActivationDraft((request) => {
            request.binding.priority = priority;
        });
    }

    setActivationResolutionMode(mode: 'active' | 'pinned'): boolean {
        const state = get(this.mutable);
        if (state.activation === null) return false;
        return this.updateActivationDraft((request) => {
            request.binding.resolution_mode = mode;
            request.binding.pinned_revision_id =
                mode === 'pinned' ? (state.activation?.candidate.revision_id ?? null) : null;
        });
    }

    selectCompletedPackageApproval(approvalId: string | null): boolean {
        const state = get(this.mutable);
        const activation = state.activation;
        if (activation === null) return false;
        if (
            approvalId !== null &&
            !activation.candidate.completed_package_approvals.some(
                (approval) => approval.approval_id === approvalId,
            )
        ) {
            return false;
        }
        return this.updateActivationDraft((request) => {
            request.binding.package_import_approval_id = approvalId;
        });
    }

    chooseActivationConflict(component: ContentModuleComponentRefDto, choice: string): boolean {
        const state = get(this.mutable);
        const review = state.activation?.review?.review;
        if (review === undefined) return false;
        const conflict = review.conflicts.find(
            (candidate) =>
                contentModuleComponentKey(candidate.component) ===
                contentModuleComponentKey(component),
        );
        if (
            conflict === undefined ||
            (choice !== 'omit' &&
                !conflict.candidates.some(
                    (candidate) => contentModuleCandidateKey(candidate) === choice,
                ))
        ) {
            return false;
        }
        this.update((current) => ({
            ...current,
            phase: 'reviewed',
            activation:
                current.activation === null
                    ? null
                    : {
                          ...current.activation,
                          plan: null,
                          approval_id: null,
                          receipt: null,
                          conflict_choices: {
                              ...current.activation.conflict_choices,
                              [contentModuleComponentKey(component)]: choice,
                          },
                      },
            error: null,
        }));
        return true;
    }

    async reviewActivation(): Promise<boolean> {
        const state = get(this.mutable);
        if (!this.available || !hasLifecycleApi(this.client) || state.activation === null) {
            return false;
        }
        const activation = structuredClone(state.activation);
        if (!activation.candidate.local_use_allowed) {
            return this.failInvariant('이 리비전은 로컬 사용이 허용되지 않습니다.');
        }
        if (
            activation.candidate.source_kind === 'imported_package' &&
            activation.request.binding.package_import_approval_id === null
        ) {
            return this.failInvariant(
                '가져온 패키지 모듈은 완료된 패키지 승인 ID를 명시적으로 선택해야 합니다.',
            );
        }
        const epoch = ++this.operationEpoch;
        this.update((current) => ({ ...current, phase: 'reviewing', error: null }));
        try {
            const review = await this.client.reviewContentModuleActivation({
                activation: structuredClone(activation.request),
            });
            if (!this.isCurrent(epoch)) return false;
            if (
                review.proposed_revision.module_id !== activation.candidate.module_id ||
                review.proposed_revision.revision_id !== activation.candidate.revision_id ||
                review.proposed_revision.revision_source_sha256 !==
                    activation.candidate.revision_source_sha256 ||
                review.review.activation_binding_ids.filter(
                    (bindingId) => bindingId === activation.request.binding.id,
                ).length !== 1 ||
                !review.proposed_revision.local_use_allowed
            ) {
                return this.failInvariant(
                    'Core 검토 결과가 선택한 불변 모듈 리비전 또는 바인딩과 일치하지 않습니다.',
                );
            }
            if (!hasExactImportedActivationAuthority(activation, review)) {
                return this.failInvariant(
                    'Core 검토 결과가 선택한 바인딩의 단일 패키지 승인 체인과 정확히 일치하지 않습니다.',
                );
            }
            this.update((current) => ({
                ...current,
                phase: 'reviewed',
                activation:
                    current.activation === null
                        ? null
                        : {
                              ...current.activation,
                              review,
                              plan: null,
                              conflict_choices: {},
                              approval_id: null,
                              receipt: null,
                          },
                error: null,
                announcement: `${review.proposed_revision.name} 모듈의 정확한 리비전과 충돌을 검토했습니다.`,
            }));
            return true;
        } catch (error: unknown) {
            return this.fail(error, epoch);
        }
    }

    async resolveActivation(): Promise<boolean> {
        const state = get(this.mutable);
        const activation = state.activation;
        if (
            !this.available ||
            !hasLifecycleApi(this.client) ||
            activation?.review === null ||
            activation === null
        ) {
            return false;
        }
        const resolutions = conflictResolutions(
            activation.review.review,
            activation.conflict_choices,
        );
        if (resolutions === null) {
            return this.failInvariant(
                '모든 충돌에서 사용할 후보 또는 명시적 제외를 선택해 주세요.',
            );
        }
        const input = {
            activation: structuredClone(activation.request),
            resolutions: {
                expected_review_sha256: activation.review.review.review_sha256,
                resolutions,
            },
        };
        const epoch = ++this.operationEpoch;
        this.update((current) => ({ ...current, phase: 'resolving', error: null }));
        try {
            const plan = await this.client.resolveContentModuleActivation(input);
            if (!this.isCurrent(epoch)) return false;
            if (
                plan.review_sha256 !== activation.review.review.review_sha256 ||
                plan.expected_state_revision !== activation.review.review.state_revision ||
                !plan.activation_binding_ids.includes(activation.request.binding.id)
            ) {
                return this.failInvariant(
                    'Core 활성화 계획이 검토 해시, 상태 리비전 또는 바인딩과 일치하지 않습니다.',
                );
            }
            this.update((current) => ({
                ...current,
                phase: 'resolved',
                activation:
                    current.activation === null
                        ? null
                        : {
                              ...current.activation,
                              plan,
                              approval_id: null,
                              receipt: null,
                          },
                error: null,
                announcement: '검토한 충돌 선택으로 활성화 계획을 만들었습니다.',
            }));
            return true;
        } catch (error: unknown) {
            return this.fail(error, epoch);
        }
    }

    async activateReviewedPlan(): Promise<ContentModuleActivationReceiptDto | null> {
        const state = get(this.mutable);
        const activation = state.activation;
        if (
            !this.available ||
            !hasLifecycleApi(this.client) ||
            activation?.review === null ||
            activation?.plan === null ||
            activation === null
        ) {
            return null;
        }
        const resolutions = conflictResolutions(
            activation.review.review,
            activation.conflict_choices,
        );
        if (resolutions === null) {
            this.failInvariant('검토한 모든 충돌 선택이 필요합니다.');
            return null;
        }
        const approvalId = activation.approval_id ?? globalThis.crypto.randomUUID();
        this.update((current) => ({
            ...current,
            phase: 'applying',
            activation:
                current.activation === null
                    ? null
                    : { ...current.activation, approval_id: approvalId },
            error: null,
        }));
        const epoch = ++this.operationEpoch;
        try {
            const receipt = await this.client.activateContentModule({
                activation: structuredClone(activation.request),
                resolutions: {
                    expected_review_sha256: activation.review.review.review_sha256,
                    resolutions,
                },
                approval: {
                    approval_id: approvalId,
                    expected_review_sha256: activation.review.review.review_sha256,
                    expected_plan_sha256: activation.plan.plan_sha256,
                },
            });
            if (!this.isCurrent(epoch)) return null;
            if (
                !exactReceipt(
                    receipt,
                    approvalId,
                    activation.review.review.review_sha256,
                    activation.plan.plan_sha256,
                    activation.request.binding.id,
                    activation.request.binding.module_id,
                    activation.review.proposed_revision.revision_id,
                    activation.request.expected_binding_revision ?? 0,
                )
            ) {
                this.failInvariant(
                    'Core 영수증이 검증 완료 상태 또는 승인·검토·계획·바인딩 해시와 일치하지 않습니다.',
                );
                return null;
            }
            this.update((current) => ({
                ...current,
                phase: 'completed',
                activation:
                    current.activation === null
                        ? null
                        : { ...current.activation, approval_id: approvalId, receipt },
                error: null,
                announcement: `${activation.candidate.name} 모듈을 검증된 영수증으로 활성화했습니다.`,
            }));
            await this.refreshWorkspaceAfterReceipt(epoch, activation.request.runtime_target);
            return receipt;
        } catch (error: unknown) {
            this.fail(error, epoch);
            return null;
        }
    }

    async reviewDeactivation(bindingId: string): Promise<boolean> {
        const state = get(this.mutable);
        const runtimeTarget = state.runtime_target;
        const binding = state.bindings.find((item) => item.binding.binding.id === bindingId);
        if (
            !this.available ||
            !hasLifecycleApi(this.client) ||
            runtimeTarget === null ||
            binding === undefined
        ) {
            return false;
        }
        const epoch = ++this.operationEpoch;
        this.update((current) => ({
            ...current,
            phase: 'reviewing',
            activation: null,
            rollback: null,
            deactivation: {
                binding: structuredClone(binding),
                review: null,
                receipt: null,
            },
            error: null,
        }));
        try {
            const review = await this.client.reviewContentModuleDeactivation({
                deactivation: {
                    runtime_target: structuredClone(runtimeTarget),
                    binding_id: bindingId,
                },
            });
            if (!this.isCurrent(epoch)) return false;
            if (
                !/^[0-9a-f]{64}$/.test(review.review_sha256) ||
                !exactRuntimeTarget(review.runtime_target, runtimeTarget) ||
                !exactBindingSnapshot(review.binding, binding.binding.binding) ||
                review.approved_revision_id !== binding.approved_revision_id ||
                review.expected_binding_revision !== binding.binding.state_revision ||
                review.binding_updated_at !== binding.binding.updated_at ||
                review.disposition !== binding.disposition
            ) {
                return this.failInvariant(
                    'Core 비활성화 검토가 선택한 바인딩, 승인 리비전 또는 상태 CAS와 일치하지 않습니다.',
                );
            }
            this.update((current) => ({
                ...current,
                phase: 'reviewed',
                deactivation:
                    current.deactivation === null
                        ? null
                        : { ...current.deactivation, review, receipt: null },
                error: null,
                announcement: `${binding.module_name} 모듈 바인딩의 정확한 상태와 해시를 비활성화 전에 검토했습니다.`,
            }));
            return true;
        } catch (error: unknown) {
            return this.fail(error, epoch);
        }
    }

    async deactivateReviewedBinding(): Promise<ContentModuleDeactivationReceiptDto | null> {
        const state = get(this.mutable);
        const deactivation = state.deactivation;
        const runtimeTarget = state.runtime_target;
        if (
            !this.available ||
            !hasLifecycleApi(this.client) ||
            deactivation?.review === null ||
            deactivation === null ||
            runtimeTarget === null
        ) {
            return null;
        }
        const epoch = ++this.operationEpoch;
        this.update((current) => ({ ...current, phase: 'applying', error: null }));
        try {
            const receipt = await this.client.deactivateContentModule({
                deactivation: {
                    runtime_target: structuredClone(runtimeTarget),
                    binding_id: deactivation.binding.binding.binding.id,
                },
                expected_review_sha256: deactivation.review.review_sha256,
            });
            if (!this.isCurrent(epoch)) return null;
            if (!exactDeactivationReceipt(receipt, deactivation.review)) {
                this.failInvariant(
                    'Core 비활성화 영수증이 검증 완료 상태 또는 검토·바인딩·삭제 CAS와 일치하지 않습니다.',
                );
                return null;
            }
            this.update((current) => ({
                ...current,
                phase: 'completed',
                deactivation:
                    current.deactivation === null ? null : { ...current.deactivation, receipt },
                error: null,
                announcement: `${deactivation.binding.module_name} 모듈 바인딩을 검증된 영수증으로 비활성화했습니다.`,
            }));
            await this.refreshWorkspaceAfterReceipt(epoch, runtimeTarget);
            return receipt;
        } catch (error: unknown) {
            this.fail(error, epoch);
            return null;
        }
    }

    chooseRollbackConflict(component: ContentModuleComponentRefDto, choice: string): boolean {
        const state = get(this.mutable);
        const review = state.rollback?.review?.review.activation;
        if (review === undefined) return false;
        const conflict = review.conflicts.find(
            (candidate) =>
                contentModuleComponentKey(candidate.component) ===
                contentModuleComponentKey(component),
        );
        if (
            conflict === undefined ||
            (choice !== 'omit' &&
                !conflict.candidates.some(
                    (candidate) => contentModuleCandidateKey(candidate) === choice,
                ))
        ) {
            return false;
        }
        this.update((current) => ({
            ...current,
            phase: 'reviewed',
            rollback:
                current.rollback === null
                    ? null
                    : {
                          ...current.rollback,
                          plan: null,
                          approval_id: null,
                          receipt: null,
                          conflict_choices: {
                              ...current.rollback.conflict_choices,
                              [contentModuleComponentKey(component)]: choice,
                          },
                      },
            error: null,
        }));
        return true;
    }

    async reviewRollback(
        bindingId: string,
        targetRevisionId: string,
        targetPackageImportApprovalId: string | null,
    ): Promise<boolean> {
        const state = get(this.mutable);
        const runtimeTarget = state.runtime_target;
        const binding = state.bindings.find((item) => item.binding.binding.id === bindingId);
        const revision = binding?.revisions.find((item) => item.revision_id === targetRevisionId);
        if (
            !this.available ||
            !hasLifecycleApi(this.client) ||
            runtimeTarget === null ||
            binding === undefined ||
            revision === undefined
        ) {
            return false;
        }
        if (!revision.rollback_allowed) {
            return this.failInvariant('선택한 불변 리비전은 이 바인딩의 롤백 대상이 아닙니다.');
        }
        const importedTarget = revision.source_kind === 'imported_package';
        if (
            (importedTarget &&
                !revision.completed_package_approvals.some(
                    (approval) => approval.approval_id === targetPackageImportApprovalId,
                )) ||
            (!importedTarget && targetPackageImportApprovalId !== null)
        ) {
            return this.failInvariant(
                '가져온 롤백 대상은 해당 불변 리비전의 완료된 패키지 승인을 명시적으로 선택해야 합니다.',
            );
        }
        const epoch = ++this.operationEpoch;
        this.update((current) => ({
            ...current,
            phase: 'reviewing',
            activation: null,
            deactivation: null,
            rollback: {
                binding_id: bindingId,
                target_revision_id: targetRevisionId,
                target_package_import_approval_id: targetPackageImportApprovalId,
                review: null,
                plan: null,
                conflict_choices: {},
                approval_id: null,
                receipt: null,
            },
            error: null,
        }));
        try {
            const review = await this.client.reviewContentModuleRollback({
                runtime_target: structuredClone(runtimeTarget),
                binding_id: bindingId,
                target_revision_id: targetRevisionId,
                target_package_import_approval_id: targetPackageImportApprovalId,
            });
            if (!this.isCurrent(epoch)) return false;
            if (
                review.review.rollback.binding_id !== bindingId ||
                review.review.rollback.target_revision_id !== targetRevisionId ||
                review.target_revision.revision_id !== targetRevisionId ||
                review.target_revision.module_id !== binding.binding.binding.module_id
            ) {
                return this.failInvariant(
                    'Core 롤백 검토가 선택한 바인딩 또는 불변 대상 리비전과 일치하지 않습니다.',
                );
            }
            const targetAuthorities = review.review.activation.import_approvals.filter(
                (approval) => approval.binding_id === bindingId,
            );
            const targetAuthority = targetAuthorities[0];
            if (
                (importedTarget &&
                    (targetAuthorities.length !== 1 ||
                        targetAuthority?.evidence.approval_id !== targetPackageImportApprovalId ||
                        targetAuthority.evidence.module_id !== binding.binding.binding.module_id ||
                        targetAuthority.evidence.module_revision_id !== targetRevisionId ||
                        targetAuthority.evidence.module_revision_source_sha256 !==
                            revision.source_sha256)) ||
                (!importedTarget && targetAuthorities.length !== 0)
            ) {
                return this.failInvariant(
                    'Core 롤백 검토의 패키지 승인 근거가 선택한 불변 대상 리비전과 일치하지 않습니다.',
                );
            }
            this.update((current) => ({
                ...current,
                phase: 'reviewed',
                rollback:
                    current.rollback === null
                        ? null
                        : { ...current.rollback, review, conflict_choices: {} },
                error: null,
                announcement: `${binding.module_name} 모듈의 불변 리비전 차이와 차단 사유를 검토했습니다.`,
            }));
            return true;
        } catch (error: unknown) {
            return this.fail(error, epoch);
        }
    }

    private rollbackResolution(
        rollback: ContentModuleRollbackState,
        runtimeTarget: ContentModuleRuntimeTargetInput,
    ): ResolveContentModuleRollbackInput | null {
        const presentation = rollback.review;
        if (presentation === null) return null;
        const resolutions = conflictResolutions(
            {
                ...presentation.review.activation,
            },
            rollback.conflict_choices,
        );
        if (resolutions === null) return null;
        return {
            runtime_target: structuredClone(runtimeTarget),
            binding_id: rollback.binding_id,
            target_revision_id: rollback.target_revision_id,
            target_package_import_approval_id: rollback.target_package_import_approval_id,
            expected_state_revision: presentation.review.rollback.expected_state_revision,
            expected_rollback_review_sha256: presentation.review.rollback.review_sha256,
            resolutions: {
                expected_review_sha256: presentation.review.activation.review_sha256,
                resolutions,
            },
        };
    }

    async resolveRollback(): Promise<boolean> {
        const state = get(this.mutable);
        const rollback = state.rollback;
        const runtimeTarget = state.runtime_target;
        if (
            !this.available ||
            !hasLifecycleApi(this.client) ||
            rollback?.review === null ||
            rollback === null ||
            runtimeTarget === null
        ) {
            return false;
        }
        if (!rollback.review.review.rollback.eligible) {
            return this.failInvariant('차단 사유가 있는 롤백은 계획으로 만들 수 없습니다.');
        }
        const input = this.rollbackResolution(rollback, runtimeTarget);
        if (input === null) {
            return this.failInvariant('모든 롤백 충돌에서 사용할 후보 또는 제외를 선택해 주세요.');
        }
        const epoch = ++this.operationEpoch;
        this.update((current) => ({ ...current, phase: 'resolving', error: null }));
        try {
            const plan = await this.client.resolveContentModuleRollback(input);
            if (!this.isCurrent(epoch)) return false;
            if (
                plan.rollback.review_sha256 !== rollback.review.review.rollback.review_sha256 ||
                plan.rollback.expected_state_revision !==
                    rollback.review.review.rollback.expected_state_revision ||
                plan.rollback.binding_id !== rollback.binding_id ||
                plan.rollback.target_revision_id !== rollback.target_revision_id ||
                plan.activation.review_sha256 !== rollback.review.review.activation.review_sha256
            ) {
                return this.failInvariant(
                    'Core 롤백 계획이 검토 해시, 상태 리비전, 바인딩 또는 대상 리비전과 일치하지 않습니다.',
                );
            }
            this.update((current) => ({
                ...current,
                phase: 'resolved',
                rollback:
                    current.rollback === null
                        ? null
                        : {
                              ...current.rollback,
                              plan,
                              approval_id: null,
                              receipt: null,
                          },
                error: null,
                announcement: '검토한 불변 리비전으로 원자적 롤백 계획을 만들었습니다.',
            }));
            return true;
        } catch (error: unknown) {
            return this.fail(error, epoch);
        }
    }

    async applyReviewedRollback(): Promise<ContentModuleActivationReceiptDto | null> {
        const state = get(this.mutable);
        const rollback = state.rollback;
        const runtimeTarget = state.runtime_target;
        if (
            !this.available ||
            !hasLifecycleApi(this.client) ||
            rollback?.review === null ||
            rollback?.plan === null ||
            rollback === null ||
            runtimeTarget === null
        ) {
            return null;
        }
        const resolution = this.rollbackResolution(rollback, runtimeTarget);
        if (resolution === null) {
            this.failInvariant('검토한 모든 롤백 충돌 선택이 필요합니다.');
            return null;
        }
        const approvalId = rollback.approval_id ?? globalThis.crypto.randomUUID();
        const input: ApplyContentModuleRollbackInput = {
            resolution,
            expected_rollback_plan_sha256: rollback.plan.rollback.plan_sha256,
            activation_approval: {
                approval_id: approvalId,
                expected_review_sha256: rollback.plan.activation.review_sha256,
                expected_plan_sha256: rollback.plan.activation.plan_sha256,
            },
        };
        this.update((current) => ({
            ...current,
            phase: 'applying',
            rollback:
                current.rollback === null ? null : { ...current.rollback, approval_id: approvalId },
            error: null,
        }));
        const epoch = ++this.operationEpoch;
        try {
            const receipt = await this.client.applyContentModuleRollback(input);
            if (!this.isCurrent(epoch)) return null;
            const binding = state.bindings.find(
                (candidate) => candidate.binding.binding.id === rollback.binding_id,
            );
            if (
                binding === undefined ||
                !exactReceipt(
                    receipt,
                    approvalId,
                    rollback.plan.activation.review_sha256,
                    rollback.plan.activation.plan_sha256,
                    rollback.binding_id,
                    binding.binding.binding.module_id,
                    rollback.target_revision_id,
                    rollback.review.review.rollback.expected_state_revision,
                )
            ) {
                this.failInvariant(
                    'Core 롤백 영수증이 검증 완료 상태 또는 승인·검토·계획·바인딩 해시와 일치하지 않습니다.',
                );
                return null;
            }
            this.update((current) => ({
                ...current,
                phase: 'completed',
                rollback:
                    current.rollback === null
                        ? null
                        : { ...current.rollback, approval_id: approvalId, receipt },
                error: null,
                announcement: `${binding.module_name} 모듈을 검증된 영수증으로 롤백했습니다.`,
            }));
            await this.refreshWorkspaceAfterReceipt(epoch, runtimeTarget);
            return receipt;
        } catch (error: unknown) {
            this.fail(error, epoch);
            return null;
        }
    }

    clearReview(): void {
        this.operationEpoch += 1;
        this.update((state) => ({
            ...state,
            phase: state.runtime_target === null ? 'idle' : 'ready',
            activation: null,
            rollback: null,
            deactivation: null,
            error: null,
        }));
    }

    destroy(): void {
        this.operationEpoch += 1;
    }
}
