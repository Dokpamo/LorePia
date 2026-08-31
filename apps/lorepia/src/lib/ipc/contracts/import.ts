import type { OrchestrationModuleScope } from './common';

export type {
    ImportDynamicContentReviewDto,
    ImportImagePreviewDto,
    ImportInspectionDto,
    ImportIssueDto,
    ImportRegexRuleReviewDto,
    ImportTicketDto,
} from '../import-contracts';

export interface ContentModuleComponentDto {
    id: string;
    kind: string;
    name: string;
    selected: boolean;
    enabled: boolean;
}

export interface ContentModuleReviewDto {
    id: string;
    name: string;
    version: string;
    source_label: string;
    license_label: string;
    redistribution_status: 'allowed' | 'blocked' | 'unknown';
    conflicts: string[];
    required_capabilities: string[];
    components: ContentModuleComponentDto[];
    active_revision: number | null;
    available_revision: number;
    revision: number;
    state_revision: number;
    merge_review_sha256: string | null;
    merge_plan_sha256: string | null;
}

export interface ContentRevisionDiffDto {
    module_id: string;
    from_revision: number;
    to_revision: number;
    summary: string[];
    expected_state_revision: number;
    rollback_review_sha256: string | null;
    rollback_plan_sha256: string | null;
}

/**
 * Opaque, platform-owned hand-off for a staged LorePia package. No host path
 * or archive bytes cross the webview boundary.
 */
export type ContentPackageImportStatusDto =
    | 'inspected'
    | 'awaiting_review'
    | 'approved'
    | 'committing'
    | 'completed'
    | 'failed'
    | 'discarded'
    | 'rolled_back';

export type ContentPackageComponentKindDto =
    | 'prompt_preset'
    | 'memory_profile'
    | 'knowledge_book'
    | 'transform_set'
    | 'interaction_rule_set'
    | 'content_module'
    | 'asset_index'
    | 'raw_extension';

export type ContentPackageComponentDispositionDto = 'importable' | 'unsupported' | 'quarantined';

export type ContentPackageIssueSeverityDto = 'warning' | 'blocker';

export type ContentPackageRedistributionStatusDto =
    | 'allowed'
    | 'denied_by_manifest'
    | 'license_unclear'
    | 'provenance_incomplete'
    | 'validation_blocked';

export type ContentPackageCapabilityDto =
    | 'prompt_fragments'
    | 'knowledge'
    | 'variables'
    | 'transforms'
    | 'declarative_interactions'
    | 'image_assets'
    | 'audio_assets'
    | 'video_assets'
    | 'attachment_assets'
    | 'high_risk_assets'
    | 'external_urls'
    | 'html'
    | 'script'
    | 'native_code'
    | 'network'
    | 'filesystem'
    | 'shell'
    | 'credentials';

export type ApprovableContentPackageCapabilityDto = 'transforms' | 'declarative_interactions';

export type ContentPackageCapabilitySupportDto = 'supported' | 'unsupported' | 'approval_required';

export interface ContentPackageManifestReviewDto {
    package_id: string;
    name: string;
    version: string;
    author: string | null;
    license: string;
    redistribution_allowed: boolean;
    required_app_version: string | null;
    required_capabilities: ContentPackageCapabilityDto[];
}

export interface ContentPackageComponentReviewDto {
    id: string;
    kind: ContentPackageComponentKindDto;
    disposition: ContentPackageComponentDispositionDto;
    dependency_ids: string[];
    conflict_ids: string[];
    required_capabilities: ContentPackageCapabilityDto[];
    asset_count: number;
}

export interface ContentPackageIssueDto {
    severity: ContentPackageIssueSeverityDto;
    code: string;
    message: string;
}

export interface ContentPackageCapabilityDecisionDto {
    capability: ContentPackageCapabilityDto;
    support: ContentPackageCapabilitySupportDto;
    approved: boolean;
    reason: string;
}

export interface ContentPackageInspectionReviewDto {
    import_id: string;
    revision: number;
    manifest: ContentPackageManifestReviewDto;
    source_size_bytes: number;
    total_uncompressed_size_bytes: number;
    components: ContentPackageComponentReviewDto[];
    asset_count: number;
    issues: ContentPackageIssueDto[];
    local_import_allowed: boolean;
    redistribution_status: ContentPackageRedistributionStatusDto;
    package_plan_hash: string;
    review_sha256: string;
    capability_review_sha256: string;
    capability_decisions: ContentPackageCapabilityDecisionDto[];
}

export interface PackageNormalizationEvidenceDto {
    component_id: string;
    object_id: string;
    field: string;
    before: boolean;
    after: boolean;
    reason: string;
}

export type ContentPackageTargetDispositionDto = 'create' | 'update';

export type ContentPackageTargetDocumentKindDto =
    | 'prompt_preset'
    | 'knowledge_book'
    | 'memory_profile'
    | 'transform_set'
    | 'interaction_rule_set'
    | 'content_module'
    | 'character_content';

export interface ContentPackageTargetReviewDocumentDto {
    source_component_id: string;
    component_document_ordinal: number;
    document_index: number;
    document_kind: ContentPackageTargetDocumentKindDto;
    target_object_id: string;
    disposition: ContentPackageTargetDispositionDto;
    expected_target_revision_id: string | null;
    expected_target_state_revision: number | null;
    source_component_sha256: string;
    document_sha256: string;
}

export interface ContentPackageTargetReviewDto {
    target_review_sha256: string;
    documents: ContentPackageTargetReviewDocumentDto[];
}

export interface ConfirmedContentPackageUpdateTargetDto {
    source_component_id: string;
    component_document_ordinal: number;
    target_object_id: string;
    expected_target_revision_id: string;
    expected_target_state_revision: number;
}

export interface ContentPackageSelectionReviewDto {
    content_selection_plan_hash: string;
    import_plan_sha256: string;
    normalization_evidence_sha256: string;
    normalization_evidence: PackageNormalizationEvidenceDto[];
    target_review: ContentPackageTargetReviewDto;
}

export interface ContentPackageApprovalReviewDto {
    approval_sha256: string;
    approval_id: string;
    enabled_component_ids: string[];
    approved_capabilities: ApprovableContentPackageCapabilityDto[];
}

export interface ContentPackageImportReviewDto {
    import_id: string;
    package_id: string;
    status: ContentPackageImportStatusDto;
    revision: number;
    package_plan_hash: string;
    review_sha256: string;
    capability_review_sha256: string;
    selected_component_ids: string[];
    selection: ContentPackageSelectionReviewDto | null;
    approval: ContentPackageApprovalReviewDto | null;
}

export interface ContentPackageWorkspaceDto {
    inspection: ContentPackageInspectionReviewDto;
    lifecycle: ContentPackageImportReviewDto;
}

export interface ReopenContentPackageImportInput {
    import_id: string;
}

export interface ListPendingContentPackageImportsInput {
    limit: number;
}

export interface SelectContentPackageImportInput {
    import_id: string;
    expected_revision: number;
    expected_package_plan_hash: string;
    expected_review_sha256: string;
    expected_capability_review_sha256: string;
    selected_component_ids: string[];
}

export interface SelectContentPackageImportReceiptDto {
    import_id: string;
    status: ContentPackageImportStatusDto;
    revision: number;
    package_plan_hash: string;
    review_sha256: string;
    capability_review_sha256: string;
    selected_component_ids: string[];
    selection: ContentPackageSelectionReviewDto;
    required_capabilities: ContentPackageCapabilityDto[];
}

export interface ApproveContentPackageImportInput {
    import_id: string;
    expected_revision: number;
    expected_package_plan_hash: string;
    expected_content_selection_plan_hash: string;
    expected_review_sha256: string;
    expected_import_plan_sha256: string;
    expected_capability_review_sha256: string;
    expected_normalization_evidence_sha256: string;
    expected_target_review_sha256: string;
    approval_id: string;
    enable_component_ids: string[];
    approved_capabilities: ApprovableContentPackageCapabilityDto[];
    confirmed_update_targets: ConfirmedContentPackageUpdateTargetDto[];
}

export interface ApproveContentPackageImportReceiptDto {
    import_id: string;
    status: ContentPackageImportStatusDto;
    revision: number;
    package_plan_hash: string;
    content_selection_plan_hash: string;
    review_sha256: string;
    import_plan_sha256: string;
    capability_review_sha256: string;
    normalization_evidence_sha256: string;
    normalization_evidence: PackageNormalizationEvidenceDto[];
    target_review: ContentPackageTargetReviewDto;
    approval_sha256: string;
    approval_id: string;
    enabled_component_ids: string[];
    approved_capabilities: ApprovableContentPackageCapabilityDto[];
}

export interface CommitContentPackageImportInput {
    import_id: string;
    expected_revision: number;
    expected_package_plan_hash: string;
    expected_content_selection_plan_hash: string;
    expected_review_sha256: string;
    expected_import_plan_sha256: string;
    expected_approval_sha256: string;
    expected_capability_review_sha256: string;
    expected_normalization_evidence_sha256: string;
}

export interface CommitContentPackageImportReceiptDto {
    import_id: string;
    package_id: string;
    status: ContentPackageImportStatusDto;
    revision: number;
    committed_document_ids: string[];
    asset_ids: string[];
}

export interface DiscardContentPackageImportInput {
    import_id: string;
    expected_revision: number;
    expected_review_sha256: string;
    expected_import_plan_sha256: string | null;
    expected_capability_review_sha256: string;
}

export interface ContentPackageImportSummaryDto {
    import_id: string;
    package_id: string;
    status: ContentPackageImportStatusDto;
    revision: number;
    selected_component_ids: string[];
    failure_code: string | null;
    created_at: string;
    updated_at: string;
}

export type ContentSourceExportInput =
    | { kind: 'character_source'; character_id: string }
    | { kind: 'content_package'; import_id: string };

export type ContentSourceExportKindDto = 'character_card_v3' | 'charx_package' | 'lorepia_package';

export interface ContentSourceExportDescriptorDto {
    kind: ContentSourceExportKindDto;
    source_id: string;
    sha256: string;
    size_bytes: number;
    suggested_file_name: string;
}

export interface ContentSourceExportReceiptDto {
    kind: ContentSourceExportKindDto;
    source_id: string;
    sha256: string;
    size_bytes: number;
    file_name: string;
}

export interface ListCompletedContentPackageExportsInput {
    limit: number;
}

export interface ContentPackageClientApi {
    listPendingContentPackageImports(
        input: ListPendingContentPackageImportsInput,
    ): Promise<ContentPackageImportReviewDto[]>;
    pickContentPackageImport(): Promise<ContentPackageInspectionReviewDto | null>;
    reopenContentPackageImport(
        input: ReopenContentPackageImportInput,
    ): Promise<ContentPackageWorkspaceDto>;
    selectContentPackageImport(
        input: SelectContentPackageImportInput,
    ): Promise<SelectContentPackageImportReceiptDto>;
    approveContentPackageImport(
        input: ApproveContentPackageImportInput,
    ): Promise<ApproveContentPackageImportReceiptDto>;
    commitContentPackageImport(
        input: CommitContentPackageImportInput,
    ): Promise<CommitContentPackageImportReceiptDto>;
    discardContentPackageImport(
        input: DiscardContentPackageImportInput,
    ): Promise<ContentPackageImportSummaryDto>;
    listCompletedContentPackageExports(
        input: ListCompletedContentPackageExportsInput,
    ): Promise<ContentSourceExportDescriptorDto[]>;
    exportContentSource(
        input: ContentSourceExportInput,
    ): Promise<ContentSourceExportReceiptDto | null>;
}

export interface ModuleBindingDocumentDto {
    id: string;
    module_id: string;
    scope: OrchestrationModuleScope;
    target_id: string | null;
    enabled: boolean;
    approved: boolean;
    revision_id: string;
    created_at: string;
}

export interface ListContentModuleBindingsInput {
    content_module_id: string;
}

export interface ListContentModuleRevisionsInput {
    content_module_id: string;
}

export interface ContentModuleRevisionSummaryDocumentDto {
    revision_id: string;
    revision: number;
    sha256: string;
    created_at: string;
}

export interface ContentModuleRevisionListResultDto {
    revisions: ContentModuleRevisionSummaryDocumentDto[];
    truncated: boolean;
}

export interface DiffContentModuleRevisionsInput {
    content_module_id: string;
    from_revision: number;
    to_revision: number;
}

export interface ContentModuleRevisionDiffDocumentDto {
    content_module_id: string;
    from_revision: number;
    to_revision: number;
    from_sha256: string;
    to_sha256: string;
    changed_paths: string[];
    truncated: boolean;
}

export interface EvaluateContentModuleShareInput {
    content_module_id: string;
}

export interface ContentShareGateDto {
    module_id: string;
    local_use_allowed: boolean;
    sharing_allowed: boolean;
    reasons: string[];
}
