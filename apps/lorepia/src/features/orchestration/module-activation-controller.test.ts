import { get } from 'svelte/store';
import { describe, expect, it, vi } from 'vitest';
import { ModuleActivationController } from './module-activation-controller';
import type { ContentModuleActivationState } from './module-lifecycle-controller';
import type { ActivateContentModuleInput } from './module-lifecycle-contracts';
import type {
    ContentModulePagedClientApi,
    ModulePlanSummary,
    ModuleReceiptSummary,
    ModuleReviewPage,
} from './module-activation-pages';

const hash = (character: string) => character.repeat(64);

function fixture() {
    const candidate = {
        module_id: 'module',
        revision_id: 'revision',
        revision_source_sha256: hash('a'),
        name: 'Synthetic module',
        version: '1',
        author: null,
        license: 'synthetic',
        redistribution_allowed: false,
        required_capabilities: [],
        source_kind: 'imported_package' as const,
        local_use_allowed: true,
        sharing_allowed: false,
        share_reasons: [],
        component_count: 128,
        completed_package_approvals: [
            {
                approval_id: 'import-approval',
                approval_sha256: hash('b'),
                import_id: 'import',
                import_revision: 1,
                package_id: 'package',
                package_source_sha256: hash('c'),
                selection_sha256: hash('d'),
                capability_review_sha256: hash('e'),
                module_id: 'module',
                module_revision_id: 'revision',
                module_revision_source_sha256: hash('a'),
            },
        ],
    };
    const draft: ContentModuleActivationState = {
        candidate,
        request: {
            runtime_target: { conversation_id: 'conversation', branch_id: 'branch' },
            expected_binding_revision: null,
            binding: {
                id: 'binding',
                module_id: 'module',
                scope: 'branch',
                target_id: 'branch',
                conversation_id: 'conversation',
                priority: 0,
                resolution_mode: 'pinned',
                pinned_revision_id: 'revision',
                package_import_approval_id: 'import-approval',
                variable_overrides: { values: [] },
            },
        },
        review: null,
        plan: null,
        conflict_choices: {},
        approval_id: null,
        receipt: null,
    };
    const packageApproval = candidate.completed_package_approvals[0];
    if (!packageApproval) throw new Error('Fixture approval is missing');
    const page: ModuleReviewPage = {
        review_sha256: hash('f'),
        state_revision: 0,
        activation_binding_id: 'binding',
        proposed_revision: candidate,
        package_approval: packageApproval,
        component_count: 128,
        offset: 0,
        next_offset: 64,
        components: Array.from({ length: 64 }, (_, i) => ({
            component: { kind: 'asset' as const, id: `asset-${String(i)}` },
            candidates: [],
        })),
        conflicts: [],
    };
    const plan: ModulePlanSummary = {
        review_sha256: page.review_sha256,
        plan_sha256: hash('1'),
        expected_state_revision: 0,
        activation_binding_id: 'binding',
        component_count: 128,
        runtime_enabled_count: 128,
        omitted_component_count: 0,
    };
    const client = {
        reviewContentModuleActivationPage: vi
            .fn<ContentModulePagedClientApi['reviewContentModuleActivationPage']>()
            .mockResolvedValue(page),
        resolveContentModuleActivationSummary: vi
            .fn<ContentModulePagedClientApi['resolveContentModuleActivationSummary']>()
            .mockResolvedValue(plan),
        activateContentModuleSummary: vi.fn(
            (input: ActivateContentModuleInput): Promise<ModuleReceiptSummary> =>
                Promise.resolve({
                    verified: true,
                    approval_id: input.approval.approval_id,
                    approval_sha256: hash('2'),
                    plan,
                    binding: {
                        state_revision: 1,
                        updated_at: '2026-09-11T00:00:00Z',
                        binding: {
                            ...draft.request.binding,
                            revision_id: 'revision',
                            created_at: '2026-09-11T00:00:00Z',
                            approved: true,
                            enabled: true,
                            activation_approval_id: input.approval.approval_id,
                            activation_review_sha256: plan.review_sha256,
                            activation_plan_sha256: plan.plan_sha256,
                        },
                    },
                }),
        ),
    };
    return { draft, page, plan, client, controller: new ModuleActivationController(client, draft) };
}

describe('paged module activation authority', () => {
    it('keeps the approval id on an uncertain result and accepts only the exact verified receipt', async () => {
        const f = fixture();
        expect(await f.controller.review()).toBe(true);
        expect(await f.controller.resolve()).toBe(true);
        f.client.activateContentModuleSummary.mockRejectedValueOnce(
            new Error('transport interrupted'),
        );
        expect(await f.controller.activate()).toBe(false);
        expect(await f.controller.activate()).toBe(true);
        expect(f.client.activateContentModuleSummary.mock.calls[0]?.[0].approval).toEqual(
            f.client.activateContentModuleSummary.mock.calls[1]?.[0].approval,
        );
        expect(get(f.controller.state).receipt?.verified).toBe(true);
        expect(await f.controller.activate()).toBe(false);
        expect(f.client.activateContentModuleSummary).toHaveBeenCalledTimes(2);
    });

    it('rejects changed review pages and mismatched completed import authority', async () => {
        const f = fixture();
        await f.controller.review();
        f.client.reviewContentModuleActivationPage.mockResolvedValueOnce({
            ...f.page,
            offset: 64,
            next_offset: null,
            review_sha256: hash('3'),
        });
        expect(await f.controller.review(64)).toBe(false);
        expect(get(f.controller.state).page?.offset).toBe(0);
        if (!f.page.package_approval) throw new Error('Fixture approval is missing');
        const wrong = {
            ...f.page,
            package_approval: { ...f.page.package_approval, approval_id: 'different' },
        };
        f.client.reviewContentModuleActivationPage.mockResolvedValueOnce(wrong);
        expect(await f.controller.review()).toBe(false);
        expect(f.client.resolveContentModuleActivationSummary).not.toHaveBeenCalled();
    });

    it('does not publish a late result after the draft view is disposed', async () => {
        const f = fixture();
        let deliver!: (value: ModuleReviewPage) => void;
        f.client.reviewContentModuleActivationPage.mockReturnValueOnce(
            new Promise((resolve) => (deliver = resolve)),
        );
        const work = f.controller.review();
        f.controller.destroy();
        deliver(f.page);
        expect(await work).toBe(false);
        expect(get(f.controller.state).page).toBeNull();
    });

    it('requires an explicit choice for every conflict before resolution', async () => {
        const f = fixture();
        const conflict = {
            component: { kind: 'asset' as const, id: 'shared' },
            reason: 'different_hash',
            candidates: [
                { module_id: 'module', revision_id: 'revision', component_hash: hash('4') },
            ],
        };
        f.page.conflicts = [conflict];
        await f.controller.review();
        expect(await f.controller.resolve()).toBe(false);
        expect(f.client.resolveContentModuleActivationSummary).not.toHaveBeenCalled();
        f.controller.choose(conflict, 'omit');
        f.client.resolveContentModuleActivationSummary.mockResolvedValueOnce({
            ...f.plan,
            component_count: 127,
            omitted_component_count: 1,
            runtime_enabled_count: 127,
        });
        expect(await f.controller.resolve()).toBe(true);
        expect(
            f.client.resolveContentModuleActivationSummary.mock.calls[0]?.[0].resolutions
                .resolutions[0]?.selected,
        ).toBeNull();
    });
});
