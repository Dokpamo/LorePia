import { get } from 'svelte/store';
import { afterEach, describe, expect, it, vi } from 'vitest';

import type {
    ActivateContentModuleInput,
    ApplyContentModuleRollbackInput,
    ContentModuleActivationPlanDto,
    ContentModuleActivationReceiptDto,
    ContentModuleActivationReviewPresentationDto,
    ContentModuleDeactivationReceiptDto,
    ContentModuleDeactivationReviewDto,
    ContentModuleLifecycleBindingDto,
    ContentModuleLifecycleCandidateDto,
    ContentModuleLifecycleClientApi,
    ContentModuleRollbackPlanDto,
    ContentModuleRollbackReviewPresentationDto,
    ReviewedContentModuleImportApprovalDto,
    ResolveContentModuleActivationInput,
    ResolveContentModuleRollbackInput,
    ReviewContentModuleActivationInput,
} from './module-lifecycle-contracts';
import { ContentModuleLifecycleController } from './module-lifecycle-controller';

const MODULE_SHA = 'a'.repeat(64);
const REVIEW_SHA = 'b'.repeat(64);
const PLAN_SHA = 'c'.repeat(64);
const APPROVAL_SHA = 'd'.repeat(64);
const COMPONENT_SHA = 'e'.repeat(64);
const ROLLBACK_REVIEW_SHA = '1'.repeat(64);
const ROLLBACK_PLAN_SHA = '2'.repeat(64);
const DIFF_SHA = '3'.repeat(64);
const CURRENT_SHA = '4'.repeat(64);
const TARGET_SHA = '5'.repeat(64);
const DEACTIVATION_REVIEW_SHA = 'f'.repeat(64);
const PACKAGE_APPROVAL_ID = 'package-approval-1';
const TARGET_PACKAGE_APPROVAL_ID = 'package-approval-target-1';
const BINDING_ID = '00000000-0000-4000-8000-000000000001';
const ACTIVATION_APPROVAL_ID = '00000000-0000-4000-8000-000000000002';
const COMPONENT = { kind: 'transform_set' as const, id: 'transform-1' };
const CONFLICT_CANDIDATE = {
    module_id: 'module-1',
    revision_id: 'revision-2',
    component_hash: COMPONENT_SHA,
};

function candidate(): ContentModuleLifecycleCandidateDto {
    return {
        module_id: 'module-1',
        revision_id: 'revision-2',
        revision_source_sha256: MODULE_SHA,
        name: '합성 모듈',
        version: '2.0.0',
        author: 'LorePia',
        license: 'project-owned synthetic',
        redistribution_allowed: false,
        required_capabilities: ['transforms'],
        source_kind: 'imported_package',
        local_use_allowed: true,
        sharing_allowed: false,
        share_reasons: ['manifest denies redistribution'],
        component_count: 1,
        completed_package_approvals: [
            {
                approval_id: PACKAGE_APPROVAL_ID,
                approval_sha256: APPROVAL_SHA,
                import_id: 'import-1',
                import_revision: 9,
                package_id: 'package.synthetic',
                package_source_sha256: '6'.repeat(64),
                selection_sha256: '7'.repeat(64),
                capability_review_sha256: '8'.repeat(64),
                module_id: 'module-1',
                module_revision_id: 'revision-2',
                module_revision_source_sha256: MODULE_SHA,
            },
        ],
    };
}

function activationImportAuthority(bindingId = BINDING_ID): ReviewedContentModuleImportApprovalDto {
    const selectedApproval = candidate().completed_package_approvals[0];
    if (selectedApproval === undefined) {
        throw new Error('synthetic package approval is missing');
    }
    return {
        binding_id: bindingId,
        evidence: {
            ...selectedApproval,
            module_package_component_id: 'module-component',
            module_package_component_sha256: '9'.repeat(64),
            module_commit_result_sha256: '0'.repeat(64),
            selected_package_component_ids: ['module-component'],
            authorized_capabilities: ['transforms'],
            component_authorities: [
                {
                    component: COMPONENT,
                    component_sha256: COMPONENT_SHA,
                    package_component_id: 'transform-component',
                    package_component_sha256: '1'.repeat(64),
                    committed_target_object_id: 'transform-1',
                    committed_target_revision_id: 'transform-revision-1',
                    committed_result_sha256: '2'.repeat(64),
                    committed_content_sha256: '3'.repeat(64),
                },
            ],
        },
    };
}

function requiredActivationImportAuthority(
    review: ContentModuleActivationReviewPresentationDto,
): ReviewedContentModuleImportApprovalDto {
    const authority = review.review.import_approvals[0];
    if (authority === undefined) throw new Error('synthetic import authority is missing');
    return authority;
}

function activationReview(bindingId = BINDING_ID): ContentModuleActivationReviewPresentationDto {
    return {
        proposed_revision: {
            ...candidate(),
        },
        review: {
            review_sha256: REVIEW_SHA,
            state_revision: 12,
            context: {
                local_user_id: 'local-user-1',
                persona_id: null,
                character_id: 'character-1',
                conversation_id: 'conversation-1',
                branch_id: 'branch-1',
                supported_capabilities: ['transforms'],
            },
            activation_binding_ids: [bindingId],
            ordered_bindings: [],
            ignored_bindings: [],
            components: [
                {
                    component: COMPONENT,
                    candidates: [
                        {
                            candidate: CONFLICT_CANDIDATE,
                            sources: [
                                {
                                    binding_id: bindingId,
                                    module_id: 'module-1',
                                    revision_id: 'revision-2',
                                    revision_source_sha256: MODULE_SHA,
                                    scope: 'branch',
                                    target_id: 'branch-1',
                                    conversation_id: 'conversation-1',
                                    priority: 0,
                                    module_ordinal: 0,
                                    runtime_enabled_intent: true,
                                },
                            ],
                        },
                    ],
                },
            ],
            conflicts: [
                {
                    component: COMPONENT,
                    candidates: [CONFLICT_CANDIDATE],
                    reason: 'synthetic exact conflict',
                },
            ],
            import_approvals: [activationImportAuthority(bindingId)],
            effective_variable_overrides: { values: [] },
        },
    };
}

function activationPlan(bindingId = BINDING_ID): ContentModuleActivationPlanDto {
    return {
        plan_sha256: PLAN_SHA,
        review_sha256: REVIEW_SHA,
        expected_state_revision: 12,
        activation_binding_ids: [bindingId],
        ordered_binding_ids: [bindingId],
        components: [],
        omitted_components: [COMPONENT],
        import_approvals: [activationImportAuthority(bindingId)],
        effective_variable_overrides: { values: [] },
    };
}

function bindingItem(): ContentModuleLifecycleBindingDto {
    const sourceApproval = candidate().completed_package_approvals[0];
    if (sourceApproval === undefined) throw new Error('synthetic package approval is missing');
    return {
        binding: {
            binding: {
                id: BINDING_ID,
                module_id: 'module-1',
                scope: 'branch',
                target_id: 'branch-1',
                conversation_id: 'conversation-1',
                priority: 0,
                resolution_mode: 'pinned',
                pinned_revision_id: 'revision-2',
                enabled: true,
                approved: true,
                package_import_approval_id: PACKAGE_APPROVAL_ID,
                activation_approval_id: 'previous-approval',
                activation_review_sha256: '9'.repeat(64),
                activation_plan_sha256: '0'.repeat(64),
                variable_overrides: { values: [] },
                revision_id: 'revision-2',
                created_at: '2026-08-03T00:00:00Z',
            },
            state_revision: 7,
            updated_at: '2026-08-03T00:01:00Z',
        },
        approved_revision_id: 'revision-2',
        disposition: 'applied',
        module_name: '합성 모듈',
        revision_source_sha256: CURRENT_SHA,
        revisions: [
            {
                revision_id: 'revision-1',
                name: '합성 모듈 v1',
                version: '1.0.0',
                source_sha256: TARGET_SHA,
                source_kind: 'imported_package',
                previous_revision_id: null,
                created_at: '2026-08-02T00:00:00Z',
                active: false,
                rollback_allowed: true,
                completed_package_approvals: [
                    {
                        ...sourceApproval,
                        approval_id: TARGET_PACKAGE_APPROVAL_ID,
                        module_revision_id: 'revision-1',
                        module_revision_source_sha256: TARGET_SHA,
                    },
                ],
            },
            {
                revision_id: 'revision-2',
                name: '합성 모듈',
                version: '2.0.0',
                source_sha256: CURRENT_SHA,
                source_kind: 'imported_package',
                previous_revision_id: 'revision-1',
                created_at: '2026-08-03T00:00:00Z',
                active: true,
                rollback_allowed: false,
                completed_package_approvals: candidate().completed_package_approvals,
            },
        ],
        revisions_truncated: false,
    };
}

function receipt(
    plan: ContentModuleActivationPlanDto,
    approvalId = ACTIVATION_APPROVAL_ID,
    verified = true,
    revisionId = 'revision-2',
    stateRevision = 1,
    updatedAt = '2026-08-03T00:02:00Z',
): ContentModuleActivationReceiptDto {
    return {
        verified,
        binding: {
            binding: {
                ...bindingItem().binding.binding,
                activation_approval_id: approvalId,
                activation_review_sha256: plan.review_sha256,
                activation_plan_sha256: plan.plan_sha256,
                revision_id: revisionId,
            },
            state_revision: stateRevision,
            updated_at: updatedAt,
        },
        approval_id: approvalId,
        approval_sha256: APPROVAL_SHA,
        review_sha256: plan.review_sha256,
        plan_sha256: plan.plan_sha256,
        approved_plan: plan,
        approved_components: [],
    };
}

function deactivationReview(): ContentModuleDeactivationReviewDto {
    const binding = bindingItem();
    return {
        review_sha256: DEACTIVATION_REVIEW_SHA,
        runtime_target: {
            conversation_id: 'conversation-1',
            branch_id: 'branch-1',
        },
        binding: binding.binding.binding,
        approved_revision_id: binding.approved_revision_id,
        expected_binding_revision: binding.binding.state_revision,
        binding_updated_at: binding.binding.updated_at,
        disposition: binding.disposition,
    };
}

function deactivationReceipt(
    stateRevision = 8,
    revisionId = 'revision-2',
): ContentModuleDeactivationReceiptDto {
    const review = deactivationReview();
    const deletedAt = '2026-08-03T00:03:00Z';
    return {
        verified: true,
        review,
        binding: {
            binding: { ...review.binding, revision_id: revisionId },
            state_revision: stateRevision,
            updated_at: deletedAt,
        },
        deleted_at: deletedAt,
    };
}

function rollbackReview(): ContentModuleRollbackReviewPresentationDto {
    const activation = activationReview().review;
    activation.import_approvals = [
        {
            binding_id: BINDING_ID,
            evidence: {
                approval_id: TARGET_PACKAGE_APPROVAL_ID,
                approval_sha256: APPROVAL_SHA,
                import_id: 'import-target-1',
                import_revision: 8,
                package_id: 'package.synthetic',
                package_source_sha256: '6'.repeat(64),
                selection_sha256: '7'.repeat(64),
                capability_review_sha256: '8'.repeat(64),
                module_id: 'module-1',
                module_revision_id: 'revision-1',
                module_revision_source_sha256: TARGET_SHA,
                module_package_component_id: 'module-component',
                module_package_component_sha256: 'a'.repeat(64),
                module_commit_result_sha256: 'b'.repeat(64),
                selected_package_component_ids: ['module-component'],
                authorized_capabilities: ['transforms'],
                component_authorities: [],
            },
        },
    ];
    return {
        target_revision: {
            ...candidate(),
            revision_id: 'revision-1',
            revision_source_sha256: TARGET_SHA,
            version: '1.0.0',
        },
        review: {
            rollback: {
                review_sha256: ROLLBACK_REVIEW_SHA,
                expected_state_revision: 18,
                binding_id: BINDING_ID,
                current_revision_id: 'revision-2',
                current_source_sha256: CURRENT_SHA,
                target_revision_id: 'revision-1',
                target_source_sha256: TARGET_SHA,
                diff: {
                    diff_sha256: DIFF_SHA,
                    module_id: 'module-1',
                    from_revision_id: 'revision-2',
                    to_revision_id: 'revision-1',
                    from_source_sha256: CURRENT_SHA,
                    to_source_sha256: TARGET_SHA,
                    component_changes: [
                        {
                            component: COMPONENT,
                            kind: 'removed',
                            previous_sha256: COMPONENT_SHA,
                            next_sha256: null,
                        },
                    ],
                    capability_changes: {
                        added: [],
                        removed: ['transforms'],
                    },
                    metadata_changed_fields: ['version'],
                },
                blockers: [],
                eligible: true,
            },
            activation,
        },
    };
}

function rollbackPlan(): ContentModuleRollbackPlanDto {
    return {
        rollback: {
            plan_sha256: ROLLBACK_PLAN_SHA,
            review_sha256: ROLLBACK_REVIEW_SHA,
            expected_state_revision: 18,
            binding_id: BINDING_ID,
            expected_current_revision_id: 'revision-2',
            expected_current_source_sha256: CURRENT_SHA,
            target_revision_id: 'revision-1',
            target_source_sha256: TARGET_SHA,
            diff_sha256: DIFF_SHA,
        },
        activation: activationPlan(),
    };
}

function candidateList() {
    return {
        items: [candidate()],
        truncated: false,
        scope_targets: [
            {
                scope: 'branch' as const,
                target_id: 'branch-1',
                conversation_id: 'conversation-1',
                label: '현재 브랜치',
            },
            {
                scope: 'user' as const,
                target_id: null,
                conversation_id: null,
                label: '로컬 사용자',
            },
        ],
    };
}

function bindingList() {
    return {
        items: [bindingItem()],
        truncated: false,
        workspace_review_sha256: 'f'.repeat(64),
        workspace_state_revision: 20,
    };
}

function capableClient(
    overrides: Partial<ContentModuleLifecycleClientApi> = {},
): ContentModuleLifecycleClientApi {
    return {
        listContentModuleLifecycleCandidates: vi.fn().mockResolvedValue(candidateList()),
        listContentModuleLifecycleBindings: vi.fn().mockResolvedValue(bindingList()),
        reviewContentModuleActivation: vi.fn().mockResolvedValue(activationReview()),
        resolveContentModuleActivation: vi.fn().mockResolvedValue(activationPlan()),
        activateContentModule: vi.fn().mockResolvedValue(receipt(activationPlan())),
        reviewContentModuleRollback: vi.fn().mockResolvedValue(rollbackReview()),
        resolveContentModuleRollback: vi.fn().mockResolvedValue(rollbackPlan()),
        applyContentModuleRollback: vi
            .fn()
            .mockResolvedValue(
                receipt(rollbackPlan().activation, ACTIVATION_APPROVAL_ID, true, 'revision-1', 19),
            ),
        reviewContentModuleDeactivation: vi.fn().mockResolvedValue(deactivationReview()),
        deactivateContentModule: vi.fn().mockResolvedValue(deactivationReceipt()),
        ...overrides,
    };
}

afterEach(() => {
    vi.restoreAllMocks();
});

describe('ContentModuleLifecycleController', () => {
    it('does not manufacture lifecycle success when the Core boundary is unavailable', async () => {
        const controller = new ContentModuleLifecycleController({});

        await expect(controller.loadContext('conversation-1', 'branch-1')).resolves.toBe(false);
        expect(get(controller.state)).toMatchObject({
            phase: 'unavailable',
            candidates: [],
            bindings: [],
            activation: null,
            rollback: null,
            deactivation: null,
        });
        await expect(controller.activateReviewedPlan()).resolves.toBeNull();
        await expect(controller.deactivateReviewedBinding()).resolves.toBeNull();
    });

    it('reviews the exact binding before echoing the review hash into a CAS deactivation', async () => {
        const reviewContentModuleDeactivation = vi.fn().mockResolvedValue(deactivationReview());
        const deactivateContentModule = vi.fn().mockResolvedValue(deactivationReceipt());
        const controller = new ContentModuleLifecycleController(
            capableClient({ reviewContentModuleDeactivation, deactivateContentModule }),
        );

        await controller.loadContext('conversation-1', 'branch-1');
        await expect(controller.reviewDeactivation(BINDING_ID)).resolves.toBe(true);
        expect(reviewContentModuleDeactivation).toHaveBeenCalledWith({
            deactivation: {
                runtime_target: {
                    conversation_id: 'conversation-1',
                    branch_id: 'branch-1',
                },
                binding_id: BINDING_ID,
            },
        });
        expect(get(controller.state)).toMatchObject({
            phase: 'reviewed',
            activation: null,
            rollback: null,
            deactivation: {
                review: {
                    review_sha256: DEACTIVATION_REVIEW_SHA,
                    expected_binding_revision: 7,
                    approved_revision_id: 'revision-2',
                },
                receipt: null,
            },
        });

        await expect(controller.deactivateReviewedBinding()).resolves.toMatchObject({
            verified: true,
            deleted_at: '2026-08-03T00:03:00Z',
        });
        expect(deactivateContentModule).toHaveBeenCalledWith({
            deactivation: {
                runtime_target: {
                    conversation_id: 'conversation-1',
                    branch_id: 'branch-1',
                },
                binding_id: BINDING_ID,
            },
            expected_review_sha256: DEACTIVATION_REVIEW_SHA,
        });
        expect(get(controller.state)).toMatchObject({
            phase: 'completed',
            deactivation: {
                receipt: {
                    verified: true,
                    binding: { state_revision: 8 },
                },
            },
        });
    });

    it('rejects a deactivation receipt whose deleted binding revision misses the reviewed CAS', async () => {
        const controller = new ContentModuleLifecycleController(
            capableClient({
                deactivateContentModule: vi.fn().mockResolvedValue(deactivationReceipt(9)),
            }),
        );

        await controller.loadContext('conversation-1', 'branch-1');
        await controller.reviewDeactivation(BINDING_ID);
        await expect(controller.deactivateReviewedBinding()).resolves.toBeNull();
        expect(get(controller.state)).toMatchObject({
            phase: 'error',
            deactivation: { receipt: null },
        });
        expect(get(controller.state).announcement).not.toContain('비활성화했습니다');
    });

    it('echoes the exact durable binding CAS when re-reviewing an existing binding', async () => {
        const randomUUID = vi.spyOn(globalThis.crypto, 'randomUUID');
        const reviewContentModuleActivation = vi
            .fn<
                (
                    input: ReviewContentModuleActivationInput,
                ) => Promise<ContentModuleActivationReviewPresentationDto>
            >()
            .mockResolvedValue(activationReview());
        const controller = new ContentModuleLifecycleController(
            capableClient({ reviewContentModuleActivation }),
        );

        await controller.loadContext('conversation-1', 'branch-1');
        expect(controller.beginActivation('module-1', BINDING_ID)).toBe(true);
        expect(randomUUID).not.toHaveBeenCalled();
        expect(controller.selectCompletedPackageApproval(PACKAGE_APPROVAL_ID)).toBe(true);
        await expect(controller.reviewActivation()).resolves.toBe(true);

        expect(reviewContentModuleActivation).toHaveBeenCalledOnce();
        const input = reviewContentModuleActivation.mock.calls[0]?.[0];
        expect(input?.activation.expected_binding_revision).toBe(7);
        expect(input?.activation.binding.id).toBe(BINDING_ID);
        expect(input?.activation.binding.priority).toBe(0);
        expect(input?.activation.binding.variable_overrides).toEqual({ values: [] });
    });

    it.each<[string, (review: ContentModuleActivationReviewPresentationDto) => void]>([
        [
            'missing binding authority',
            (review) => {
                review.review.import_approvals = [];
            },
        ],
        [
            'duplicate binding authority',
            (review) => {
                review.review.import_approvals.push(
                    structuredClone(requiredActivationImportAuthority(review)),
                );
            },
        ],
        [
            'wrong binding',
            (review) => {
                requiredActivationImportAuthority(review).binding_id = 'different-binding';
            },
        ],
        [
            'wrong selected package approval',
            (review) => {
                requiredActivationImportAuthority(review).evidence.approval_sha256 = 'f'.repeat(64);
            },
        ],
        [
            'wrong module',
            (review) => {
                requiredActivationImportAuthority(review).evidence.module_id = 'different-module';
            },
        ],
        [
            'wrong revision',
            (review) => {
                requiredActivationImportAuthority(review).evidence.module_revision_id =
                    'different-revision';
            },
        ],
        [
            'wrong revision source',
            (review) => {
                requiredActivationImportAuthority(review).evidence.module_revision_source_sha256 =
                    'f'.repeat(64);
            },
        ],
    ])('rejects an imported activation review with %s', async (_label, mutateReview) => {
        vi.spyOn(globalThis.crypto, 'randomUUID').mockReturnValue(BINDING_ID);
        const returnedReview = activationReview();
        mutateReview(returnedReview);
        const controller = new ContentModuleLifecycleController(
            capableClient({
                reviewContentModuleActivation: vi.fn().mockResolvedValue(returnedReview),
            }),
        );

        await controller.loadContext('conversation-1', 'branch-1');
        expect(controller.beginActivation('module-1')).toBe(true);
        expect(controller.selectCompletedPackageApproval(PACKAGE_APPROVAL_ID)).toBe(true);
        await expect(controller.reviewActivation()).resolves.toBe(false);
        expect(get(controller.state)).toMatchObject({
            phase: 'error',
            activation: { review: null, plan: null, receipt: null },
        });
        expect(get(controller.state).announcement).toContain('단일 패키지 승인 체인');
    });

    it('requires explicit completed import authority, exact conflict choices, and reuses one approval id after response loss', async () => {
        vi.spyOn(globalThis.crypto, 'randomUUID')
            .mockReturnValueOnce(BINDING_ID)
            .mockReturnValueOnce(ACTIVATION_APPROVAL_ID);
        const activateContentModule = vi
            .fn<(input: ActivateContentModuleInput) => Promise<ContentModuleActivationReceiptDto>>()
            .mockRejectedValueOnce(new Error('synthetic response loss'))
            .mockResolvedValueOnce(receipt(activationPlan()));
        const resolveContentModuleActivation = vi
            .fn<
                (
                    input: ResolveContentModuleActivationInput,
                ) => Promise<ContentModuleActivationPlanDto>
            >()
            .mockResolvedValue(activationPlan());
        const listContentModuleLifecycleCandidates = vi.fn().mockResolvedValue(candidateList());
        const listContentModuleLifecycleBindings = vi.fn().mockResolvedValue(bindingList());
        const reviewContentModuleActivation = vi.fn().mockResolvedValue(activationReview());
        const client = capableClient({
            activateContentModule,
            resolveContentModuleActivation,
            listContentModuleLifecycleCandidates,
            listContentModuleLifecycleBindings,
            reviewContentModuleActivation,
        });
        const controller = new ContentModuleLifecycleController(client);

        await expect(controller.loadContext('conversation-1', 'branch-1')).resolves.toBe(true);
        expect(listContentModuleLifecycleCandidates).toHaveBeenCalledWith({
            runtime_target: {
                conversation_id: 'conversation-1',
                branch_id: 'branch-1',
            },
            limit: 100,
        });
        expect(listContentModuleLifecycleBindings).toHaveBeenCalledWith({
            runtime_target: {
                conversation_id: 'conversation-1',
                branch_id: 'branch-1',
            },
            limit: 100,
        });
        expect(controller.beginActivation('module-1')).toBe(true);

        await expect(controller.reviewActivation()).resolves.toBe(false);
        expect(reviewContentModuleActivation).not.toHaveBeenCalled();
        expect(controller.selectCompletedPackageApproval(PACKAGE_APPROVAL_ID)).toBe(true);
        await expect(controller.reviewActivation()).resolves.toBe(true);
        expect(reviewContentModuleActivation).toHaveBeenCalledWith({
            activation: {
                runtime_target: {
                    conversation_id: 'conversation-1',
                    branch_id: 'branch-1',
                },
                expected_binding_revision: null,
                binding: {
                    id: BINDING_ID,
                    module_id: 'module-1',
                    scope: 'branch',
                    target_id: 'branch-1',
                    conversation_id: 'conversation-1',
                    priority: 0,
                    resolution_mode: 'pinned',
                    pinned_revision_id: 'revision-2',
                    package_import_approval_id: PACKAGE_APPROVAL_ID,
                    variable_overrides: { values: [] },
                },
            },
        });

        await expect(controller.resolveActivation()).resolves.toBe(false);
        expect(resolveContentModuleActivation).not.toHaveBeenCalled();
        expect(controller.chooseActivationConflict(COMPONENT, 'omit')).toBe(true);
        await expect(controller.resolveActivation()).resolves.toBe(true);
        expect(resolveContentModuleActivation).toHaveBeenCalledWith(
            expect.objectContaining({
                resolutions: {
                    expected_review_sha256: REVIEW_SHA,
                    resolutions: [
                        {
                            component: COMPONENT,
                            expected_candidates: [CONFLICT_CANDIDATE],
                            selected: null,
                        },
                    ],
                },
            }),
        );

        await expect(controller.activateReviewedPlan()).resolves.toBeNull();
        expect(get(controller.state).activation?.approval_id).toBe(ACTIVATION_APPROVAL_ID);
        await expect(controller.activateReviewedPlan()).resolves.toMatchObject({
            verified: true,
            approval_id: ACTIVATION_APPROVAL_ID,
        });
        expect(activateContentModule).toHaveBeenCalledTimes(2);
        expect(activateContentModule.mock.calls[0]?.[0].approval.approval_id).toBe(
            ACTIVATION_APPROVAL_ID,
        );
        expect(activateContentModule.mock.calls[1]?.[0].approval.approval_id).toBe(
            ACTIVATION_APPROVAL_ID,
        );
        expect(get(controller.state)).toMatchObject({
            phase: 'completed',
            error: null,
            activation: {
                receipt: {
                    verified: true,
                    review_sha256: REVIEW_SHA,
                    plan_sha256: PLAN_SHA,
                },
            },
        });
    });

    it('rejects an unverified or hash-mismatched receipt instead of announcing success', async () => {
        vi.spyOn(globalThis.crypto, 'randomUUID')
            .mockReturnValueOnce(BINDING_ID)
            .mockReturnValueOnce(ACTIVATION_APPROVAL_ID);
        const controller = new ContentModuleLifecycleController(
            capableClient({
                activateContentModule: vi
                    .fn()
                    .mockResolvedValue(receipt(activationPlan(), ACTIVATION_APPROVAL_ID, false)),
            }),
        );

        await controller.loadContext('conversation-1', 'branch-1');
        controller.beginActivation('module-1');
        controller.selectCompletedPackageApproval(PACKAGE_APPROVAL_ID);
        await controller.reviewActivation();
        controller.chooseActivationConflict(COMPONENT, 'omit');
        await controller.resolveActivation();

        await expect(controller.activateReviewedPlan()).resolves.toBeNull();
        expect(get(controller.state)).toMatchObject({
            phase: 'error',
            activation: { receipt: null },
        });
        expect(get(controller.state).announcement).not.toContain('활성화했습니다');
    });

    it.each<[string, number, string]>([
        ['tampered binding state revision', 2, '2026-08-03T00:02:00Z'],
        ['missing binding update timestamp', 1, ''],
    ])('rejects an activation receipt with %s', async (_label, stateRevision, updatedAt) => {
        vi.spyOn(globalThis.crypto, 'randomUUID')
            .mockReturnValueOnce(BINDING_ID)
            .mockReturnValueOnce(ACTIVATION_APPROVAL_ID);
        const controller = new ContentModuleLifecycleController(
            capableClient({
                activateContentModule: vi
                    .fn()
                    .mockResolvedValue(
                        receipt(
                            activationPlan(),
                            ACTIVATION_APPROVAL_ID,
                            true,
                            'revision-2',
                            stateRevision,
                            updatedAt,
                        ),
                    ),
            }),
        );

        await controller.loadContext('conversation-1', 'branch-1');
        controller.beginActivation('module-1');
        controller.selectCompletedPackageApproval(PACKAGE_APPROVAL_ID);
        await controller.reviewActivation();
        controller.chooseActivationConflict(COMPONENT, 'omit');
        await controller.resolveActivation();

        await expect(controller.activateReviewedPlan()).resolves.toBeNull();
        expect(get(controller.state)).toMatchObject({
            phase: 'error',
            activation: { receipt: null },
        });
        expect(get(controller.state).announcement).not.toContain('활성화했습니다');
    });

    it('echoes immutable rollback review, diff, state revision, exact candidate sets, and both plan hashes', async () => {
        vi.spyOn(globalThis.crypto, 'randomUUID').mockReturnValue(ACTIVATION_APPROVAL_ID);
        const resolveContentModuleRollback = vi
            .fn<
                (input: ResolveContentModuleRollbackInput) => Promise<ContentModuleRollbackPlanDto>
            >()
            .mockResolvedValue(rollbackPlan());
        const applyContentModuleRollback = vi
            .fn<
                (
                    input: ApplyContentModuleRollbackInput,
                ) => Promise<ContentModuleActivationReceiptDto>
            >()
            .mockResolvedValue(
                receipt(rollbackPlan().activation, ACTIVATION_APPROVAL_ID, true, 'revision-1', 19),
            );
        const reviewContentModuleRollback = vi.fn().mockResolvedValue(rollbackReview());
        const client = capableClient({
            resolveContentModuleRollback,
            applyContentModuleRollback,
            reviewContentModuleRollback,
        });
        const controller = new ContentModuleLifecycleController(client);

        await controller.loadContext('conversation-1', 'branch-1');
        await expect(
            controller.reviewRollback(BINDING_ID, 'revision-1', TARGET_PACKAGE_APPROVAL_ID),
        ).resolves.toBe(true);
        expect(reviewContentModuleRollback).toHaveBeenCalledWith({
            runtime_target: {
                conversation_id: 'conversation-1',
                branch_id: 'branch-1',
            },
            binding_id: BINDING_ID,
            target_revision_id: 'revision-1',
            target_package_import_approval_id: TARGET_PACKAGE_APPROVAL_ID,
        });
        expect(get(controller.state).rollback?.review?.review.rollback.diff).toMatchObject({
            diff_sha256: DIFF_SHA,
            from_revision_id: 'revision-2',
            to_revision_id: 'revision-1',
        });

        expect(controller.chooseRollbackConflict(COMPONENT, 'omit')).toBe(true);
        await expect(controller.resolveRollback()).resolves.toBe(true);
        expect(resolveContentModuleRollback).toHaveBeenCalledWith({
            runtime_target: {
                conversation_id: 'conversation-1',
                branch_id: 'branch-1',
            },
            binding_id: BINDING_ID,
            target_revision_id: 'revision-1',
            target_package_import_approval_id: TARGET_PACKAGE_APPROVAL_ID,
            expected_state_revision: 18,
            expected_rollback_review_sha256: ROLLBACK_REVIEW_SHA,
            resolutions: {
                expected_review_sha256: REVIEW_SHA,
                resolutions: [
                    {
                        component: COMPONENT,
                        expected_candidates: [CONFLICT_CANDIDATE],
                        selected: null,
                    },
                ],
            },
        });

        await expect(controller.applyReviewedRollback()).resolves.toMatchObject({
            verified: true,
            approval_id: ACTIVATION_APPROVAL_ID,
        });
        expect(applyContentModuleRollback).toHaveBeenCalledOnce();
        const applyInput = applyContentModuleRollback.mock.calls[0]?.[0];
        expect(applyInput?.resolution.expected_state_revision).toBe(18);
        expect(applyInput?.resolution.expected_rollback_review_sha256).toBe(ROLLBACK_REVIEW_SHA);
        expect(applyInput?.expected_rollback_plan_sha256).toBe(ROLLBACK_PLAN_SHA);
        expect(applyInput?.activation_approval).toEqual({
            approval_id: ACTIVATION_APPROVAL_ID,
            expected_review_sha256: REVIEW_SHA,
            expected_plan_sha256: PLAN_SHA,
        });
        expect(get(controller.state)).toMatchObject({
            phase: 'completed',
            rollback: { receipt: { verified: true } },
        });
    });
});
