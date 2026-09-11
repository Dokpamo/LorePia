import type {
    ActivateContentModuleInput,
    CompletedContentPackageApprovalDto,
    ContentModuleActivationPlanDto,
    ContentModuleActivationReceiptDto,
    ContentModuleActivationRequestInput,
    ContentModuleActivationRevisionDto,
    ContentModuleConflictDto,
    ResolveContentModuleActivationInput,
    ReviewedContentModuleComponentDto,
} from './module-lifecycle-contracts';

export interface ModuleReviewPage {
    review_sha256: string;
    state_revision: number;
    activation_binding_id: string;
    proposed_revision: ContentModuleActivationRevisionDto;
    package_approval: CompletedContentPackageApprovalDto | null;
    component_count: number;
    offset: number;
    next_offset: number | null;
    components: ReviewedContentModuleComponentDto[];
    conflicts: ContentModuleConflictDto[];
}

export interface ReviewModulePageInput {
    activation: ContentModuleActivationRequestInput;
    offset: number;
    expected_review_sha256: string | null;
}

export interface ModulePlanSummary extends Pick<
    ContentModuleActivationPlanDto,
    'review_sha256' | 'plan_sha256' | 'expected_state_revision'
> {
    activation_binding_id: string;
    component_count: number;
    runtime_enabled_count: number;
    omitted_component_count: number;
}

export interface ModuleReceiptSummary extends Pick<
    ContentModuleActivationReceiptDto,
    'verified' | 'binding' | 'approval_id' | 'approval_sha256'
> {
    plan: ModulePlanSummary;
}

export interface ContentModulePagedClientApi {
    reviewContentModuleActivationPage(input: ReviewModulePageInput): Promise<ModuleReviewPage>;
    resolveContentModuleActivationSummary(
        input: ResolveContentModuleActivationInput,
    ): Promise<ModulePlanSummary>;
    activateContentModuleSummary(input: ActivateContentModuleInput): Promise<ModuleReceiptSummary>;
}

export function hasModulePages(
    client: Partial<ContentModulePagedClientApi>,
): client is ContentModulePagedClientApi {
    return (
        typeof client.reviewContentModuleActivationPage === 'function' &&
        typeof client.resolveContentModuleActivationSummary === 'function' &&
        typeof client.activateContentModuleSummary === 'function'
    );
}
