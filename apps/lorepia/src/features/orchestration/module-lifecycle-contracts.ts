import type {
    OrchestrationModuleScope,
    OrchestrationVariableMapDto,
} from '../../lib/ipc/contracts';

export type ContentModuleSourceKindDto =
    | 'application_built_in'
    | 'user_created'
    | 'imported_standard'
    | 'imported_package'
    | 'generated';

export type ContentModuleRevisionResolutionModeDto = 'active' | 'pinned';

export type ContentModuleCapabilityDto =
    | 'prompt_fragments'
    | 'knowledge'
    | 'variables'
    | 'transforms'
    | 'declarative_interactions'
    | 'image_assets'
    | 'audio_assets'
    | 'video_assets'
    | 'attachment_assets'
    | 'high_risk_assets';

export type ContentModuleComponentRefDto =
    | { kind: 'prompt_block'; id: string }
    | { kind: 'control'; id: string }
    | { kind: 'knowledge_book'; id: string }
    | { kind: 'transform_set'; id: string }
    | { kind: 'interaction_rule_set'; id: string }
    | { kind: 'asset'; id: string };

export interface ContentModuleRuntimeTargetInput {
    conversation_id: string;
    branch_id: string;
}

export interface ContentModuleLifecycleScopeTargetDto {
    scope: OrchestrationModuleScope;
    target_id: string | null;
    conversation_id: string | null;
    label: string;
}

export interface CompletedContentPackageApprovalDto {
    approval_id: string;
    approval_sha256: string;
    import_id: string;
    import_revision: number;
    package_id: string;
    package_source_sha256: string;
    selection_sha256: string;
    capability_review_sha256: string;
    module_id: string;
    module_revision_id: string;
    module_revision_source_sha256: string;
}

export interface ContentModuleActivationRevisionDto {
    module_id: string;
    revision_id: string;
    revision_source_sha256: string;
    name: string;
    version: string;
    author: string | null;
    license: string;
    redistribution_allowed: boolean;
    required_capabilities: ContentModuleCapabilityDto[];
    source_kind: ContentModuleSourceKindDto;
    local_use_allowed: boolean;
    sharing_allowed: boolean;
    share_reasons: string[];
}

export interface ContentModuleLifecycleCandidateDto extends ContentModuleActivationRevisionDto {
    component_count: number;
    completed_package_approvals: CompletedContentPackageApprovalDto[];
}

export interface ContentModuleLifecycleCandidateListDto {
    scope_targets: ContentModuleLifecycleScopeTargetDto[];
    items: ContentModuleLifecycleCandidateDto[];
    truncated: boolean;
}

export interface ContentModuleLifecycleRevisionDto {
    revision_id: string;
    name: string;
    version: string;
    source_sha256: string;
    source_kind: ContentModuleSourceKindDto;
    previous_revision_id: string | null;
    created_at: string;
    active: boolean;
    rollback_allowed: boolean;
    completed_package_approvals: CompletedContentPackageApprovalDto[];
}

export interface ContentModuleBindingSnapshotDto {
    id: string;
    module_id: string;
    scope: OrchestrationModuleScope;
    target_id: string | null;
    conversation_id: string | null;
    priority: number;
    resolution_mode: ContentModuleRevisionResolutionModeDto;
    pinned_revision_id: string | null;
    enabled: boolean;
    approved: boolean;
    package_import_approval_id: string | null;
    activation_approval_id: string | null;
    activation_review_sha256: string | null;
    activation_plan_sha256: string | null;
    variable_overrides: OrchestrationVariableMapDto;
    revision_id: string;
    created_at: string;
}

export type ContentModuleBindingDispositionDto =
    'applied' | 'needs_reapproval' | 'disabled' | 'awaiting_approval';

export interface ContentModuleLifecycleBindingDto {
    binding: {
        binding: ContentModuleBindingSnapshotDto;
        state_revision: number;
        updated_at: string;
    };
    approved_revision_id: string;
    disposition: ContentModuleBindingDispositionDto;
    module_name: string;
    revision_source_sha256: string;
    revisions: ContentModuleLifecycleRevisionDto[];
    revisions_truncated: boolean;
}

export interface ContentModuleLifecycleBindingListDto {
    items: ContentModuleLifecycleBindingDto[];
    truncated: boolean;
    workspace_review_sha256: string;
    workspace_state_revision: number;
}

export interface ListContentModuleLifecycleCandidatesInput {
    runtime_target: ContentModuleRuntimeTargetInput;
    limit: number;
}

export interface ListContentModuleLifecycleBindingsInput {
    runtime_target: ContentModuleRuntimeTargetInput;
    limit: number;
}

export interface ContentModuleBindingDraftInput {
    id: string;
    module_id: string;
    scope: OrchestrationModuleScope;
    target_id: string | null;
    conversation_id: string | null;
    priority: number;
    resolution_mode: ContentModuleRevisionResolutionModeDto;
    pinned_revision_id: string | null;
    package_import_approval_id: string | null;
    variable_overrides: OrchestrationVariableMapDto;
}

export interface ContentModuleActivationRequestInput {
    runtime_target: ContentModuleRuntimeTargetInput;
    expected_binding_revision: number | null;
    binding: ContentModuleBindingDraftInput;
}

export interface ReviewContentModuleActivationInput {
    activation: ContentModuleActivationRequestInput;
}

export interface ContentModuleResolutionContextDto {
    local_user_id: string;
    persona_id: string | null;
    character_id: string | null;
    conversation_id: string | null;
    branch_id: string | null;
    supported_capabilities: ContentModuleCapabilityDto[];
}

export interface ContentModuleConflictCandidateDto {
    module_id: string;
    revision_id: string;
    component_hash: string;
}

export interface ContentModuleCandidateSourceDto {
    binding_id: string;
    module_id: string;
    revision_id: string;
    revision_source_sha256: string;
    scope: OrchestrationModuleScope;
    target_id: string | null;
    conversation_id: string | null;
    priority: number;
    module_ordinal: number;
    runtime_enabled_intent: boolean;
}

export interface ReviewedContentModuleCandidateDto {
    candidate: ContentModuleConflictCandidateDto;
    sources: ContentModuleCandidateSourceDto[];
}

export interface ReviewedContentModuleComponentDto {
    component: ContentModuleComponentRefDto;
    candidates: ReviewedContentModuleCandidateDto[];
}

export interface ContentModuleConflictDto {
    component: ContentModuleComponentRefDto;
    candidates: ContentModuleConflictCandidateDto[];
    reason: string;
}

export interface IgnoredContentModuleBindingDto {
    binding_id: string;
    reason: 'disabled' | 'awaiting_approval' | 'different_target';
}

export interface ReviewedContentModuleImportApprovalDto {
    binding_id: string;
    evidence: {
        approval_id: string;
        approval_sha256: string;
        import_id: string;
        import_revision: number;
        package_id: string;
        package_source_sha256: string;
        selection_sha256: string;
        capability_review_sha256: string;
        module_id: string;
        module_revision_id: string;
        module_revision_source_sha256: string;
        module_package_component_id: string;
        module_package_component_sha256: string;
        module_commit_result_sha256: string;
        selected_package_component_ids: string[];
        authorized_capabilities: ContentModuleCapabilityDto[];
        component_authorities: {
            component: ContentModuleComponentRefDto;
            component_sha256: string;
            package_component_id: string;
            package_component_sha256: string;
            committed_target_object_id: string;
            committed_target_revision_id: string;
            committed_result_sha256: string;
            committed_content_sha256: string | null;
        }[];
    };
}

export interface ContentModuleActivationReviewDto {
    review_sha256: string;
    state_revision: number;
    context: ContentModuleResolutionContextDto;
    activation_binding_ids: string[];
    ordered_bindings: ContentModuleBindingSnapshotDto[];
    ignored_bindings: IgnoredContentModuleBindingDto[];
    components: ReviewedContentModuleComponentDto[];
    conflicts: ContentModuleConflictDto[];
    import_approvals: ReviewedContentModuleImportApprovalDto[];
    effective_variable_overrides: OrchestrationVariableMapDto;
}

export interface ContentModuleActivationReviewPresentationDto {
    review: ContentModuleActivationReviewDto;
    proposed_revision: ContentModuleActivationRevisionDto;
}

export interface ContentModuleConflictResolutionInput {
    component: ContentModuleComponentRefDto;
    expected_candidates: ContentModuleConflictCandidateDto[];
    selected: ContentModuleConflictCandidateDto | null;
}

export interface ContentModuleResolutionSetInput {
    expected_review_sha256: string;
    resolutions: ContentModuleConflictResolutionInput[];
}

export interface ResolveContentModuleActivationInput {
    activation: ContentModuleActivationRequestInput;
    resolutions: ContentModuleResolutionSetInput;
}

export interface ResolvedContentModuleComponentDto {
    component: ContentModuleComponentRefDto;
    sha256: string;
    selected_source: ContentModuleCandidateSourceDto;
    coalesced_sources: ContentModuleCandidateSourceDto[];
    runtime_enabled: boolean;
}

export interface ContentModuleActivationPlanDto {
    plan_sha256: string;
    review_sha256: string;
    expected_state_revision: number;
    activation_binding_ids: string[];
    ordered_binding_ids: string[];
    components: ResolvedContentModuleComponentDto[];
    omitted_components: ContentModuleComponentRefDto[];
    import_approvals: ReviewedContentModuleImportApprovalDto[];
    effective_variable_overrides: OrchestrationVariableMapDto;
}

export interface ContentModuleActivationApprovalInput {
    approval_id: string;
    expected_review_sha256: string;
    expected_plan_sha256: string;
}

export interface ActivateContentModuleInput {
    activation: ContentModuleActivationRequestInput;
    resolutions: ContentModuleResolutionSetInput;
    approval: ContentModuleActivationApprovalInput;
}

export interface ApprovedContentModuleComponentDto {
    component: ContentModuleComponentRefDto;
    component_sha256: string;
    selected_source: ContentModuleCandidateSourceDto;
    runtime_enabled: boolean;
}

export interface ContentModuleActivationReceiptDto {
    verified: boolean;
    binding: {
        binding: ContentModuleBindingSnapshotDto;
        state_revision: number;
        updated_at: string;
    };
    approval_id: string;
    approval_sha256: string;
    review_sha256: string;
    plan_sha256: string;
    approved_plan: ContentModuleActivationPlanDto;
    approved_components: ApprovedContentModuleComponentDto[];
}

export type ContentModuleComponentChangeKindDto = 'added' | 'modified' | 'removed';

export interface ContentModuleComponentChangeDto {
    component: ContentModuleComponentRefDto;
    kind: ContentModuleComponentChangeKindDto;
    previous_sha256: string | null;
    next_sha256: string | null;
}

export interface ContentModuleRevisionDiffDto {
    diff_sha256: string;
    module_id: string;
    from_revision_id: string;
    to_revision_id: string;
    from_source_sha256: string;
    to_source_sha256: string;
    component_changes: ContentModuleComponentChangeDto[];
    capability_changes: {
        added: ContentModuleCapabilityDto[];
        removed: ContentModuleCapabilityDto[];
    };
    metadata_changed_fields: string[];
}

export type ContentModuleRollbackBlockerDto =
    | { kind: 'binding_disabled' }
    | { kind: 'binding_awaiting_approval' }
    | { kind: 'stale_binding' }
    | { kind: 'different_module' }
    | { kind: 'target_already_active' }
    | { kind: 'target_not_ancestor' }
    | { kind: 'corrupt_revision_lineage' }
    | { kind: 'corrupt_snapshot' }
    | { kind: 'unsupported_schema_version'; schema_version: number }
    | { kind: 'scope_target_missing' }
    | { kind: 'missing_asset'; asset_id: string }
    | { kind: 'unsupported_capability'; capability: ContentModuleCapabilityDto }
    | { kind: 'quarantined_target' }
    | { kind: 'unresolved_conflict'; component: ContentModuleComponentRefDto };

export interface ContentModuleRollbackReviewDto {
    review_sha256: string;
    expected_state_revision: number;
    binding_id: string;
    current_revision_id: string;
    current_source_sha256: string;
    target_revision_id: string;
    target_source_sha256: string;
    diff: ContentModuleRevisionDiffDto | null;
    blockers: ContentModuleRollbackBlockerDto[];
    eligible: boolean;
}

export interface ContentModuleRollbackReviewPresentationDto {
    review: {
        rollback: ContentModuleRollbackReviewDto;
        activation: ContentModuleActivationReviewDto;
    };
    target_revision: ContentModuleActivationRevisionDto;
}

export interface ReviewContentModuleRollbackInput {
    runtime_target: ContentModuleRuntimeTargetInput;
    binding_id: string;
    target_revision_id: string;
    target_package_import_approval_id: string | null;
}

export interface ResolveContentModuleRollbackInput {
    runtime_target: ContentModuleRuntimeTargetInput;
    binding_id: string;
    target_revision_id: string;
    target_package_import_approval_id: string | null;
    expected_state_revision: number;
    expected_rollback_review_sha256: string;
    resolutions: ContentModuleResolutionSetInput;
}

export interface ContentModuleRollbackPlanDto {
    rollback: {
        plan_sha256: string;
        review_sha256: string;
        expected_state_revision: number;
        binding_id: string;
        expected_current_revision_id: string;
        expected_current_source_sha256: string;
        target_revision_id: string;
        target_source_sha256: string;
        diff_sha256: string;
    };
    activation: ContentModuleActivationPlanDto;
}

export interface ApplyContentModuleRollbackInput {
    resolution: ResolveContentModuleRollbackInput;
    expected_rollback_plan_sha256: string;
    activation_approval: ContentModuleActivationApprovalInput;
}

export interface ContentModuleDeactivationRequestInput {
    runtime_target: ContentModuleRuntimeTargetInput;
    binding_id: string;
}

export interface ReviewContentModuleDeactivationInput {
    deactivation: ContentModuleDeactivationRequestInput;
}

export interface ContentModuleDeactivationReviewDto {
    review_sha256: string;
    runtime_target: ContentModuleRuntimeTargetInput;
    binding: ContentModuleBindingSnapshotDto;
    approved_revision_id: string;
    expected_binding_revision: number;
    binding_updated_at: string;
    disposition: ContentModuleBindingDispositionDto;
}

export interface DeactivateContentModuleInput {
    deactivation: ContentModuleDeactivationRequestInput;
    expected_review_sha256: string;
}

export interface ContentModuleDeactivationReceiptDto {
    verified: boolean;
    review: ContentModuleDeactivationReviewDto;
    binding: {
        binding: ContentModuleBindingSnapshotDto;
        state_revision: number;
        updated_at: string;
    };
    deleted_at: string;
}

export interface ContentModuleLifecycleClientApi {
    listContentModuleLifecycleCandidates(
        input: ListContentModuleLifecycleCandidatesInput,
    ): Promise<ContentModuleLifecycleCandidateListDto>;
    listContentModuleLifecycleBindings(
        input: ListContentModuleLifecycleBindingsInput,
    ): Promise<ContentModuleLifecycleBindingListDto>;
    reviewContentModuleActivation(
        input: ReviewContentModuleActivationInput,
    ): Promise<ContentModuleActivationReviewPresentationDto>;
    resolveContentModuleActivation(
        input: ResolveContentModuleActivationInput,
    ): Promise<ContentModuleActivationPlanDto>;
    activateContentModule(
        input: ActivateContentModuleInput,
    ): Promise<ContentModuleActivationReceiptDto>;
    reviewContentModuleRollback(
        input: ReviewContentModuleRollbackInput,
    ): Promise<ContentModuleRollbackReviewPresentationDto>;
    resolveContentModuleRollback(
        input: ResolveContentModuleRollbackInput,
    ): Promise<ContentModuleRollbackPlanDto>;
    applyContentModuleRollback(
        input: ApplyContentModuleRollbackInput,
    ): Promise<ContentModuleActivationReceiptDto>;
    reviewContentModuleDeactivation(
        input: ReviewContentModuleDeactivationInput,
    ): Promise<ContentModuleDeactivationReviewDto>;
    deactivateContentModule(
        input: DeactivateContentModuleInput,
    ): Promise<ContentModuleDeactivationReceiptDto>;
}
