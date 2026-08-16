import { cleanup, fireEvent, render, screen, waitFor, within } from '@testing-library/svelte';
import { afterEach, describe, expect, it, vi } from 'vitest';

import ContentModuleLifecyclePanel from './ContentModuleLifecyclePanel.svelte';
import type {
    CompletedContentPackageApprovalDto,
    ContentModuleActivationPlanDto,
    ContentModuleActivationReviewPresentationDto,
    ContentModuleDeactivationReceiptDto,
    ContentModuleDeactivationReviewDto,
    ContentModuleLifecycleClientApi,
    ContentModuleRollbackReviewPresentationDto,
    ReviewedContentModuleImportApprovalDto,
    ReviewContentModuleActivationInput,
} from './module-lifecycle-contracts';

const MODULE_SHA = 'a'.repeat(64);
const REVIEW_SHA = 'b'.repeat(64);
const TARGET_SHA = 'c'.repeat(64);
const CURRENT_SHA = 'd'.repeat(64);
const DIFF_SHA = 'e'.repeat(64);
const BINDING_ID = '00000000-0000-4000-8000-000000000001';
const PACKAGE_APPROVAL_ID = 'package-approval-1';
const TARGET_PACKAGE_APPROVAL_ID = 'package-approval-target-1';
const DEACTIVATION_REVIEW_SHA = 'f'.repeat(64);

function completedPackageApproval(): CompletedContentPackageApprovalDto {
    return {
        approval_id: PACKAGE_APPROVAL_ID,
        approval_sha256: '2'.repeat(64),
        import_id: 'import-1',
        import_revision: 7,
        package_id: 'package.synthetic',
        package_source_sha256: '3'.repeat(64),
        selection_sha256: '4'.repeat(64),
        capability_review_sha256: '5'.repeat(64),
        module_id: 'module-1',
        module_revision_id: 'revision-2',
        module_revision_source_sha256: MODULE_SHA,
    };
}

function activationImportAuthority(): ReviewedContentModuleImportApprovalDto {
    return {
        binding_id: BINDING_ID,
        evidence: {
            ...completedPackageApproval(),
            module_package_component_id: 'module-component',
            module_package_component_sha256: '7'.repeat(64),
            module_commit_result_sha256: '8'.repeat(64),
            selected_package_component_ids: ['module-component', 'transform-component'],
            authorized_capabilities: ['transforms'],
            component_authorities: [
                {
                    component: { kind: 'transform_set', id: 'transform-1' },
                    component_sha256: '9'.repeat(64),
                    package_component_id: 'transform-component',
                    package_component_sha256: 'a'.repeat(64),
                    committed_target_object_id: 'transform-1',
                    committed_target_revision_id: 'transform-revision-1',
                    committed_result_sha256: 'b'.repeat(64),
                    committed_content_sha256: 'c'.repeat(64),
                },
            ],
        },
    };
}

function activationReview(): ContentModuleActivationReviewPresentationDto {
    return {
        proposed_revision: {
            module_id: 'module-1',
            revision_id: 'revision-2',
            revision_source_sha256: MODULE_SHA,
            name: '가져온 합성 모듈',
            version: '2.0.0',
            author: 'LorePia',
            license: 'project-owned synthetic',
            redistribution_allowed: false,
            required_capabilities: ['transforms'],
            source_kind: 'imported_package',
            local_use_allowed: true,
            sharing_allowed: false,
            share_reasons: ['manifest denies redistribution'],
        },
        review: {
            review_sha256: REVIEW_SHA,
            state_revision: 9,
            context: {
                local_user_id: 'local-user-1',
                persona_id: null,
                character_id: 'character-1',
                conversation_id: 'conversation-1',
                branch_id: 'branch-1',
                supported_capabilities: ['transforms'],
            },
            activation_binding_ids: [BINDING_ID],
            ordered_bindings: [],
            ignored_bindings: [],
            components: [],
            conflicts: [],
            import_approvals: [activationImportAuthority()],
            effective_variable_overrides: {
                values: [
                    {
                        variable: { scope: 'module', namespace: 'module-1', id: 'tone' },
                        value: { type: 'enum', value: 'careful' },
                    },
                ],
            },
        },
    };
}

function activationPlan(): ContentModuleActivationPlanDto {
    const selectedSource = {
        binding_id: BINDING_ID,
        module_id: 'module-1',
        revision_id: 'revision-2',
        revision_source_sha256: MODULE_SHA,
        scope: 'branch' as const,
        target_id: 'branch-1',
        conversation_id: 'conversation-1',
        priority: 10,
        module_ordinal: 0,
        runtime_enabled_intent: false,
    };
    return {
        plan_sha256: '6'.repeat(64),
        review_sha256: REVIEW_SHA,
        expected_state_revision: 9,
        activation_binding_ids: [BINDING_ID],
        ordered_binding_ids: [BINDING_ID],
        components: [
            {
                component: { kind: 'transform_set', id: 'transform-1' },
                sha256: '9'.repeat(64),
                selected_source: selectedSource,
                coalesced_sources: [
                    selectedSource,
                    {
                        ...selectedSource,
                        binding_id: '00000000-0000-4000-8000-000000000003',
                        scope: 'user',
                        target_id: null,
                        conversation_id: null,
                        priority: 1,
                        module_ordinal: 1,
                        runtime_enabled_intent: true,
                    },
                ],
                runtime_enabled: false,
            },
        ],
        omitted_components: [{ kind: 'asset', id: 'asset-omitted' }],
        import_approvals: [activationImportAuthority()],
        effective_variable_overrides: {
            values: [
                {
                    variable: { scope: 'module', namespace: 'module-1', id: 'tone' },
                    value: { type: 'enum', value: 'careful' },
                },
            ],
        },
    };
}

function rollbackReview(): ContentModuleRollbackReviewPresentationDto {
    const activation = activationReview().review;
    activation.import_approvals = [
        {
            binding_id: BINDING_ID,
            evidence: {
                approval_id: TARGET_PACKAGE_APPROVAL_ID,
                approval_sha256: '2'.repeat(64),
                import_id: 'import-target-1',
                import_revision: 6,
                package_id: 'package.synthetic',
                package_source_sha256: '3'.repeat(64),
                selection_sha256: '4'.repeat(64),
                capability_review_sha256: '5'.repeat(64),
                module_id: 'module-1',
                module_revision_id: 'revision-1',
                module_revision_source_sha256: TARGET_SHA,
                module_package_component_id: 'module-component',
                module_package_component_sha256: '8'.repeat(64),
                module_commit_result_sha256: '9'.repeat(64),
                selected_package_component_ids: ['module-component'],
                authorized_capabilities: ['transforms'],
                component_authorities: [],
            },
        },
    ];
    return {
        target_revision: {
            ...activationReview().proposed_revision,
            revision_id: 'revision-1',
            revision_source_sha256: TARGET_SHA,
            version: '1.0.0',
        },
        review: {
            rollback: {
                review_sha256: '1'.repeat(64),
                expected_state_revision: 11,
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
                            component: { kind: 'transform_set', id: 'transform-1' },
                            kind: 'removed',
                            previous_sha256: 'f'.repeat(64),
                            next_sha256: null,
                        },
                    ],
                    capability_changes: { added: [], removed: ['transforms'] },
                    metadata_changed_fields: ['version'],
                },
                blockers: [{ kind: 'target_not_ancestor' }],
                eligible: false,
            },
            activation,
        },
    };
}

function deactivationReview(): ContentModuleDeactivationReviewDto {
    return {
        review_sha256: DEACTIVATION_REVIEW_SHA,
        runtime_target: {
            conversation_id: 'conversation-1',
            branch_id: 'branch-1',
        },
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
            package_import_approval_id: 'package-approval-1',
            activation_approval_id: 'activation-1',
            activation_review_sha256: REVIEW_SHA,
            activation_plan_sha256: '6'.repeat(64),
            variable_overrides: { values: [] },
            revision_id: 'revision-2',
            created_at: '2026-08-03T00:00:00Z',
        },
        approved_revision_id: 'revision-2',
        expected_binding_revision: 7,
        binding_updated_at: '2026-08-03T00:01:00Z',
        disposition: 'applied',
    };
}

function deactivationReceipt(): ContentModuleDeactivationReceiptDto {
    const review = deactivationReview();
    const deletedAt = '2026-08-03T00:03:00Z';
    return {
        verified: true,
        review,
        binding: {
            binding: review.binding,
            state_revision: 8,
            updated_at: deletedAt,
        },
        deleted_at: deletedAt,
    };
}

function client(
    overrides: Partial<ContentModuleLifecycleClientApi> = {},
): ContentModuleLifecycleClientApi {
    return {
        listContentModuleLifecycleCandidates: vi.fn().mockResolvedValue({
            scope_targets: [
                {
                    scope: 'branch',
                    target_id: 'branch-1',
                    conversation_id: 'conversation-1',
                    label: '현재 브랜치',
                },
            ],
            items: [
                {
                    ...activationReview().proposed_revision,
                    component_count: 1,
                    completed_package_approvals: [completedPackageApproval()],
                },
            ],
            truncated: false,
        }),
        listContentModuleLifecycleBindings: vi.fn().mockResolvedValue({
            items: [
                {
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
                            package_import_approval_id: 'package-approval-1',
                            activation_approval_id: 'activation-1',
                            activation_review_sha256: REVIEW_SHA,
                            activation_plan_sha256: '6'.repeat(64),
                            variable_overrides: { values: [] },
                            revision_id: 'revision-2',
                            created_at: '2026-08-03T00:00:00Z',
                        },
                        state_revision: 7,
                        updated_at: '2026-08-03T00:01:00Z',
                    },
                    approved_revision_id: 'revision-2',
                    disposition: 'applied',
                    module_name: '가져온 합성 모듈',
                    revision_source_sha256: CURRENT_SHA,
                    revisions: [
                        {
                            revision_id: 'revision-1',
                            name: '가져온 합성 모듈 v1',
                            version: '1.0.0',
                            source_sha256: TARGET_SHA,
                            source_kind: 'imported_package',
                            previous_revision_id: null,
                            created_at: '2026-08-02T00:00:00Z',
                            active: false,
                            rollback_allowed: true,
                            completed_package_approvals: [
                                {
                                    approval_id: TARGET_PACKAGE_APPROVAL_ID,
                                    approval_sha256: '2'.repeat(64),
                                    import_id: 'import-target-1',
                                    import_revision: 6,
                                    package_id: 'package.synthetic',
                                    package_source_sha256: '3'.repeat(64),
                                    selection_sha256: '4'.repeat(64),
                                    capability_review_sha256: '5'.repeat(64),
                                    module_id: 'module-1',
                                    module_revision_id: 'revision-1',
                                    module_revision_source_sha256: TARGET_SHA,
                                },
                            ],
                        },
                        {
                            revision_id: 'revision-2',
                            name: '가져온 합성 모듈',
                            version: '2.0.0',
                            source_sha256: CURRENT_SHA,
                            source_kind: 'imported_package',
                            previous_revision_id: 'revision-1',
                            created_at: '2026-08-03T00:00:00Z',
                            active: true,
                            rollback_allowed: false,
                            completed_package_approvals: [],
                        },
                    ],
                    revisions_truncated: false,
                },
            ],
            truncated: false,
            workspace_review_sha256: '7'.repeat(64),
            workspace_state_revision: 12,
        }),
        reviewContentModuleActivation: vi.fn().mockResolvedValue(activationReview()),
        resolveContentModuleActivation: vi.fn(),
        activateContentModule: vi.fn(),
        reviewContentModuleRollback: vi.fn().mockResolvedValue(rollbackReview()),
        resolveContentModuleRollback: vi.fn(),
        applyContentModuleRollback: vi.fn(),
        reviewContentModuleDeactivation: vi.fn().mockResolvedValue(deactivationReview()),
        deactivateContentModule: vi.fn().mockResolvedValue(deactivationReceipt()),
        ...overrides,
    };
}

afterEach(cleanup);

describe('ContentModuleLifecyclePanel', () => {
    it('keeps local-use permission separate from sharing and requires an explicit completed import approval', async () => {
        vi.spyOn(globalThis.crypto, 'randomUUID').mockReturnValue(BINDING_ID);
        const reviewContentModuleActivation = vi
            .fn<
                (
                    input: ReviewContentModuleActivationInput,
                ) => Promise<ContentModuleActivationReviewPresentationDto>
            >()
            .mockResolvedValue(activationReview());
        const api = client({ reviewContentModuleActivation });
        render(ContentModuleLifecyclePanel, {
            props: {
                client: api,
                conversationId: 'conversation-1',
                branchId: 'branch-1',
            },
        });

        const candidateButton = await screen.findByRole('button', {
            name: '이 불변 리비전 활성화 검토',
        });
        const card = candidateButton.closest('article');
        if (card === null) throw new Error('candidate card is missing');
        expect(within(card).getByText('허용')).toBeInTheDocument();
        expect(within(card).getByText('차단')).toBeInTheDocument();
        expect(
            within(card).getByRole('button', { name: '이 불변 리비전 활성화 검토' }),
        ).toBeEnabled();

        await fireEvent.click(candidateButton);
        expect(
            screen.getByText(/공유가 차단되어도 로컬 사용 허용은 유지됩니다/),
        ).toBeInTheDocument();
        const reviewButton = screen.getByRole('button', {
            name: '불변 리비전·라이선스·충돌 검토',
        });
        expect(reviewButton).toBeDisabled();

        await fireEvent.change(screen.getByLabelText('완료된 패키지 승인'), {
            target: { value: 'package-approval-1' },
        });
        expect(reviewButton).toBeEnabled();
        await fireEvent.click(reviewButton);
        await waitFor(() => {
            expect(reviewContentModuleActivation).toHaveBeenCalledOnce();
        });
        const reviewInput = reviewContentModuleActivation.mock.calls[0]?.[0];
        expect(reviewInput?.activation.binding.package_import_approval_id).toBe(
            'package-approval-1',
        );
        expect(
            await screen.findByRole('heading', { name: '검토에 포함된 가져오기 권한' }),
        ).toBeInTheDocument();
        expect(screen.getByText('transform-revision-1')).toBeInTheDocument();
    });

    it('shows bounded selected and coalesced sources, runtime effects, omissions, variables, and import authority before hash approval', async () => {
        vi.spyOn(globalThis.crypto, 'randomUUID').mockReturnValue(BINDING_ID);
        const plan = activationPlan();
        const component = plan.components[0];
        if (component === undefined) throw new Error('synthetic resolved component is missing');
        component.coalesced_sources = Array.from({ length: 101 }, (_, index) => ({
            ...component.selected_source,
            binding_id: `binding-${String(index)}`,
            module_ordinal: index,
        }));
        const resolveContentModuleActivation = vi.fn().mockResolvedValue(plan);
        const api = client({ resolveContentModuleActivation });
        render(ContentModuleLifecyclePanel, {
            props: {
                client: api,
                conversationId: 'conversation-1',
                branchId: 'branch-1',
            },
        });

        await fireEvent.click(
            await screen.findByRole('button', { name: '이 불변 리비전 활성화 검토' }),
        );
        await fireEvent.change(screen.getByLabelText('완료된 패키지 승인'), {
            target: { value: PACKAGE_APPROVAL_ID },
        });
        await fireEvent.click(
            screen.getByRole('button', { name: '불변 리비전·라이선스·충돌 검토' }),
        );
        await fireEvent.click(
            await screen.findByRole('button', { name: '선택한 충돌 해법으로 계획 만들기' }),
        );

        const planHeading = await screen.findByRole('heading', { name: '승인할 활성화 계획' });
        const planSurface = planHeading.closest('section');
        if (planSurface === null) throw new Error('activation plan surface is missing');
        expect(
            within(planSurface).getByRole('heading', {
                name: '선택된 구성요소 원본과 런타임 효과',
            }),
        ).toBeInTheDocument();
        expect(within(planSurface).getByText(/런타임 효과:\s*비활성/)).toBeInTheDocument();
        expect(
            within(planSurface).getByText(/선택 원본: module-1 \/ revision-2/),
        ).toBeInTheDocument();
        expect(within(planSurface).getByText(/병합 원본은 처음 100개만/)).toBeInTheDocument();
        expect(within(planSurface).getByText(/binding-99/)).toBeInTheDocument();
        expect(within(planSurface).queryByText(/binding-100/)).not.toBeInTheDocument();
        expect(within(planSurface).getByText('에셋 · asset-omitted')).toBeInTheDocument();
        expect(within(planSurface).getByText('module:module-1:tone')).toBeInTheDocument();
        expect(within(planSurface).getByText(/careful/)).toBeInTheDocument();
        expect(
            within(planSurface).getByRole('heading', { name: '계획에 고정된 가져오기 권한' }),
        ).toBeInTheDocument();
        expect(within(planSurface).getByText(/package\.synthetic/)).toBeInTheDocument();
        expect(resolveContentModuleActivation).toHaveBeenCalledOnce();
    });

    it('shows immutable rollback diff and blockers before keeping apply disabled', async () => {
        const resolveContentModuleRollback = vi.fn();
        const api = client({ resolveContentModuleRollback });
        render(ContentModuleLifecyclePanel, {
            props: {
                client: api,
                conversationId: 'conversation-1',
                branchId: 'branch-1',
            },
        });

        const rollbackButton = await screen.findByRole('button', {
            name: '가져온 합성 모듈 revision-1 롤백 검토',
        });
        expect(rollbackButton).toBeDisabled();
        await fireEvent.change(
            screen.getByLabelText('가져온 합성 모듈 revision-1 대상 리비전 패키지 승인'),
            { target: { value: TARGET_PACKAGE_APPROVAL_ID } },
        );
        expect(rollbackButton).toBeEnabled();
        await fireEvent.click(rollbackButton);
        expect(await screen.findByText(DIFF_SHA)).toBeInTheDocument();
        expect(
            screen.getByText('대상 리비전이 현재 리비전의 정확한 조상이 아닙니다.'),
        ).toBeInTheDocument();
        expect(
            screen.getByRole('button', {
                name: '검토한 diff·충돌로 원자적 롤백 계획 만들기',
            }),
        ).toBeDisabled();
        expect(resolveContentModuleRollback).not.toHaveBeenCalled();
    });

    it('requires a hash-bound review before deactivating one exact binding', async () => {
        const reviewContentModuleDeactivation = vi.fn().mockResolvedValue(deactivationReview());
        const deactivateContentModule = vi.fn().mockResolvedValue(deactivationReceipt());
        const api = client({ reviewContentModuleDeactivation, deactivateContentModule });
        render(ContentModuleLifecyclePanel, {
            props: {
                client: api,
                conversationId: 'conversation-1',
                branchId: 'branch-1',
            },
        });

        await fireEvent.click(
            await screen.findByRole('button', {
                name: '가져온 합성 모듈 바인딩 비활성화 검토',
            }),
        );
        expect(await screen.findByText(DEACTIVATION_REVIEW_SHA)).toBeInTheDocument();
        expect(screen.getByText(/모듈과 불변 리비전 자체는 삭제하지 않습니다/)).toBeInTheDocument();
        expect(reviewContentModuleDeactivation).toHaveBeenCalledWith({
            deactivation: {
                runtime_target: {
                    conversation_id: 'conversation-1',
                    branch_id: 'branch-1',
                },
                binding_id: BINDING_ID,
            },
        });
        expect(deactivateContentModule).not.toHaveBeenCalled();

        await fireEvent.click(
            screen.getByRole('button', { name: '이 검토 해시로 바인딩 비활성화' }),
        );
        expect(
            await screen.findByRole('heading', { name: '검증된 비활성화 영수증' }),
        ).toBeInTheDocument();
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
        expect(screen.getByText(/삭제 CAS/)).toHaveTextContent('삭제 CAS 8');
    });
});
