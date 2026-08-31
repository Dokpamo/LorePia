import type { JsonValue } from './common';

import type {
    AuthBindingDto,
    NativeCaptureStatusDto,
    ParameterSpecDto,
    ProviderConfigEntryDto,
    ProviderLocalNetworkApprovalInput,
    ProviderNetworkModeInput,
    RequestPreviewDto,
} from './provider';

export interface ProviderDiscoveryConnectionOptionsInput {
    values: ProviderConfigEntryDto[];
    api_base_path: string | null;
    timeout_seconds: number;
    network_mode: ProviderNetworkModeInput;
    local_network_approval: ProviderLocalNetworkApprovalInput | null;
}

export interface ProviderDiscoveryConnectionOptionsDto {
    values: ProviderConfigEntryDto[];
    api_base_path: string | null;
    timeout_seconds: number;
    network_mode: string;
    local_network_approval: ProviderLocalNetworkApprovalInput | null;
}

export type BeginProviderDiscoverySourceInput =
    { kind: 'site' } | { kind: 'known_provider'; template_id: string };

export interface BeginProviderDiscoveryInput {
    connection_id: string;
    display_name: string;
    site_url: string;
    docs_url: string | null;
    credential_binding_requested: boolean;
    preferred_assistant: string | null;
    connection_options: ProviderDiscoveryConnectionOptionsInput;
    supplied_evidence_ids: string[];
    source: BeginProviderDiscoverySourceInput;
}

export interface BeginProviderDiscoveryCurlInput {
    connection_id: string;
    display_name: string;
    docs_url: string | null;
    credential_binding_requested: boolean;
    preferred_assistant: string | null;
    connection_options: ProviderDiscoveryConnectionOptionsInput;
    supplied_evidence_ids: string[];
}

export interface DiscoveryFailureDto {
    code: string;
    message_key: string;
    recoverable: boolean;
}

export interface DiscoveryReviewChangeDto {
    kind: string;
    target_kind: string;
    target_id: string;
    summary_key: string;
    evidence_ids: string[];
}

export interface DiscoveryReviewDto {
    sha256: string;
    graph_sha256: string;
    changes: DiscoveryReviewChangeDto[];
    unresolved_question_count: number;
    warning_count: number;
}

export interface ProviderDiscoveryApprovalProposalDto {
    id: string;
    grant: Record<string, JsonValue>;
    grant_sha256: string;
}

export interface ProviderDiscoveryReviewProposalDto {
    review: DiscoveryReviewDto;
    approval: ProviderDiscoveryApprovalProposalDto;
    commit_attempt_id: string;
    commit_plan_sha256: string;
    request_preview: RequestPreviewDto | null;
}

export type DiscoveryAssistantFailureKindInput =
    | 'transport'
    | 'timeout'
    | 'rate_limited'
    | 'invalid_structured_output'
    | 'draft_revision_required'
    | 'provider_rejected'
    | 'internal';

export type DiscoveryAssistantInterruptionOutcomeInput =
    'confirmed_no_external_effect' | 'external_outcome_unknown';

export type DiscoveryAssistantDraftFieldDto =
    | { kind: 'api_family' }
    | { kind: 'default_api_origin' }
    | { kind: 'auth' }
    | { kind: 'generate_endpoint' }
    | { kind: 'models_endpoint' }
    | { kind: 'response_decoder' }
    | { kind: 'streaming_decoder' }
    | { kind: 'parameter'; parameter_id: string };

export interface DiscoveryAssistantQuestionDto {
    id: string;
    field: DiscoveryAssistantDraftFieldDto | null;
    question: string;
    required_evidence: string;
}

export interface DiscoveryAssistantEvidenceMappingDto {
    field: DiscoveryAssistantDraftFieldDto;
    evidence_ids: string[];
    explanation: string;
}

export interface DiscoveryAssistantFieldConfidenceDto {
    field: DiscoveryAssistantDraftFieldDto;
    level: string;
    rationale: string;
}

export type DiscoveryAssistantConflictDispositionDto =
    | { status: 'unresolved' }
    | { status: 'resolved'; selected_evidence_id: string; rationale: string };

export interface DiscoveryAssistantEvidenceConflictDto {
    field: DiscoveryAssistantDraftFieldDto;
    evidence_ids: string[];
    disposition: DiscoveryAssistantConflictDispositionDto;
}

export interface DiscoveryAssistantManifestSourceDto {
    kind: string;
    url: string;
    content_sha256: string | null;
}

export interface DiscoveryAssistantEndpointDto {
    method: string;
    path: string;
}

export interface DiscoveryAssistantManifestDto {
    schema_version: number;
    api_family: string;
    sources: DiscoveryAssistantManifestSourceDto[];
    default_api_origin: string | null;
    auth: AuthBindingDto;
    models_endpoint: DiscoveryAssistantEndpointDto | null;
    generate_endpoint: DiscoveryAssistantEndpointDto;
    response_decoder: string;
    streaming_decoder: string | null;
    parameters: ParameterSpecDto[];
}

export interface DiscoveryAssistantManifestDraftDto {
    manifest: DiscoveryAssistantManifestDto;
    evidence_mappings: DiscoveryAssistantEvidenceMappingDto[];
    conflicts: DiscoveryAssistantEvidenceConflictDto[];
    unresolved_questions: DiscoveryAssistantQuestionDto[];
    confidence: DiscoveryAssistantFieldConfidenceDto[];
    summary: string;
}

export interface DiscoveryAssistantDraftReviewDto {
    draft: DiscoveryAssistantManifestDraftDto;
    unresolved_conflicts: DiscoveryAssistantDraftFieldDto[];
    required_checks: string[];
    persistence: string;
}

export type DiscoveryAssistantHostActionDto =
    | {
          kind: 'request_more_evidence';
          session_id: string;
          questions: DiscoveryAssistantQuestionDto[];
      }
    | { kind: 'review_draft'; review: DiscoveryAssistantDraftReviewDto };

export type DiscoveryAssistantResumeAction =
    | 'approve_consent'
    | 'run_assistant'
    | 'wait_for_assistant_outcome'
    | 'resume_core_host_action'
    | 'supply_more_evidence'
    | 'approve_retry'
    | 'review_draft'
    | 'restart_interrupted'
    | 'resolve_unknown_outcome';

export interface DiscoveryAssistantResumeBoundaryDto {
    checkpoint: string | null;
    action: DiscoveryAssistantResumeAction;
    questions: DiscoveryAssistantQuestionDto[];
    draft_review: DiscoveryAssistantDraftReviewDto | null;
}

export interface DiscoveryStepDto {
    id: string;
    title_key: string;
    state: string;
}

export interface ProviderDiscoverySessionDto {
    snapshot_schema_version: number;
    id: string;
    connection_id: string;
    display_name: string;
    site_url: string;
    docs_url: string | null;
    credential_binding_requested: boolean;
    preferred_assistant: string | null;
    connection_options: ProviderDiscoveryConnectionOptionsDto;
    supplied_evidence_ids: string[];
    state: string;
    revision: number;
    next_event_sequence: number;
    steps: DiscoveryStepDto[];
    action_required: { kind: string; operation: string | null } | null;
    active_operation_id: string | null;
    recovery_operation: string | null;
    unknown_operation: string | null;
    manifest_sha256: string | null;
    commit_plan_sha256: string | null;
    commit_attempt_id: string | null;
    committed_connection_id: string | null;
    cancellation_pending: boolean;
    active_effect_approval: {
        approval_id: string;
        grant_sha256: string;
    } | null;
    failure: DiscoveryFailureDto | null;
    has_private_draft: boolean;
    review: DiscoveryReviewDto | null;
    assistant_resume_boundary: DiscoveryAssistantResumeBoundaryDto | null;
    created_at: string;
    updated_at: string;
}

export interface CapturedProviderDiscoveryDto {
    session: ProviderDiscoverySessionDto;
    capture: NativeCaptureStatusDto;
}

export type DiscoveryCandidateSummaryDto =
    | { kind: 'provider_template'; template_id: string; template_version: number }
    | { kind: 'api_origin'; origin: string }
    | { kind: 'official_document'; url: string; content_sha256: string }
    | { kind: 'model_route'; model_id: string }
    | { kind: 'manifest_draft'; schema_version: number; manifest_sha256: string };

export interface DiscoveryCandidateDto {
    id: string;
    session_id: string;
    summary: DiscoveryCandidateSummaryDto;
    evidence_ids: string[];
    created_at: string;
    proposed_revision: number;
}

export interface DiscoveryEvidenceDto {
    id: string;
    session_id: string;
    kind: string;
    source_url: string;
    content_sha256: string;
    fetched_at: string;
}

export interface DiscoveryApprovalRecordDto {
    id: string;
    session_id: string;
    session_revision: number;
    decision: string;
    grant: Record<string, JsonValue>;
    created_at: string;
}

export type DiscoveryUnknownOutcomeResolutionInput =
    | { resolution: 'confirmed_no_effect' }
    | { resolution: 'confirmed_commit_completed'; connection_id: string }
    | { resolution: 'confirmed_compensated' }
    | { resolution: 'manually_reconciled_as_failed' };

export type ContinueProviderDiscoveryActionInput =
    | { kind: 'select_template'; candidate_id: string }
    | { kind: 'continue_without_template' }
    | { kind: 'supply_more_evidence'; evidence_ids: string[] }
    | { kind: 'request_assistant' }
    | {
          kind: 'approve_assistant';
          approval_id: string;
          approval_grant_sha256: string;
      }
    | { kind: 'decline_assistant' }
    | { kind: 'approve_credential_origin'; approval_id: string }
    | {
          kind: 'approve_probes';
          approval_id: string;
          approval_grant_sha256: string;
      }
    | { kind: 'skip_probes' }
    | {
          kind: 'approve_review';
          approval_id: string;
          commit_attempt_id: string;
          commit_plan_sha256: string;
          graph_sha256: string;
      }
    | { kind: 'resume_compensation' }
    | { kind: 'restart_interrupted' }
    | {
          kind: 'resolve_unknown_outcome';
          approval_id: string;
          resolution: DiscoveryUnknownOutcomeResolutionInput;
      };

export interface ContinueProviderDiscoveryInput {
    session_id: string;
    action_id: string;
    expected_revision: number;
    action: ContinueProviderDiscoveryActionInput;
}

export interface ProviderDiscoveryEventDto {
    version: number;
    id: string;
    session_id: string;
    sequence: number;
    session_revision: number;
    state: string;
    progress: { phase: string; completed: number; total: number | null } | null;
    action_required: { kind: string; operation: string | null } | null;
    warning: string | null;
    action_id: string;
    failure: DiscoveryFailureDto | null;
}

export interface DiscoveryOutboxEventDto {
    event: ProviderDiscoveryEventDto;
    delivery_attempts: number;
    available_at: string;
    created_at: string;
}

export interface DiscoveryRecoveryResultDto {
    operation_id: string;
    session_id: string;
    state: string;
    event: ProviderDiscoveryEventDto;
}

export interface DiscoveryCompensationRecordDto {
    id: string;
    commit_attempt_id: string;
    ordinal: number;
    action_id: string;
    kind: string;
    status: string;
    attempt_count: number;
    last_failure: DiscoveryFailureDto | null;
    created_at: string;
    updated_at: string;
    completed_at: string | null;
}

export type CatalogChangeKind = 'added' | 'updated' | 'removed';

export type CatalogHttpMethodDto = 'GET' | 'POST';

export interface CatalogEndpointDto {
    method: CatalogHttpMethodDto;
    path: string;
}

export interface CatalogManifestEndpointsDto {
    models: CatalogEndpointDto | null;
    generate: CatalogEndpointDto;
    embeddings?: CatalogEndpointDto;
}

export type CatalogDecoderIdDto =
    | 'open_ai_json_v1'
    | 'open_ai_sse_v1'
    | 'anthropic_json_v1'
    | 'anthropic_sse_v1'
    | 'gemini_json_v1'
    | 'gemini_sse_v1'
    | 'ollama_json_v1'
    | 'ollama_jsonl_v1';

export interface CatalogManifestDecodersDto {
    response: CatalogDecoderIdDto;
    streaming: CatalogDecoderIdDto | null;
}

export interface CatalogManifestParameterMappingDto {
    parameter_id: string;
    mapping: {
        target: 'request_body' | 'request_header';
        field_name: string;
    };
}

export interface CatalogManifestSecuritySurfaceDto {
    origin: string | null;
    authentication: AuthBindingDto;
    endpoints: CatalogManifestEndpointsDto;
    decoders: CatalogManifestDecodersDto;
    parameter_mappings: CatalogManifestParameterMappingDto[];
}

export interface CatalogManifestSecurityReviewDto {
    before: CatalogManifestSecuritySurfaceDto | null;
    after: CatalogManifestSecuritySurfaceDto | null;
}

export interface CatalogManifestDiffDto {
    provider_template_id: string;
    change: CatalogChangeKind;
    previous_manifest_version: number | null;
    next_manifest_version: number | null;
    previous_sha256: string | null;
    next_sha256: string | null;
    changed_sections: string[];
    security_review?: CatalogManifestSecurityReviewDto;
}

export interface CatalogModelMetadataDiffDto {
    model_entry_id: string;
    provider_template_id: string;
    change: CatalogChangeKind;
    previous_metadata_version: number | null;
    next_metadata_version: number | null;
    previous_sha256: string | null;
    next_sha256: string | null;
    changed_sections: string[];
}

export interface ProviderCatalogDiffDto {
    diff_schema_version: number;
    from_revision: number;
    to_revision: number;
    manifest_changes: CatalogManifestDiffDto[];
    model_changes: CatalogModelMetadataDiffDto[];
}

export interface ProviderCatalogStatusDto {
    status_schema_version: number;
    state_version: number;
    active_revision: number;
    active_snapshot_sha256: string;
    bundled_baseline_sha256: string;
    snapshot_count: number;
    signed_update_count: number;
    highest_accepted_revision: number;
    latest_issued_at: string | null;
    active_signed_revisions: number[];
}

export interface ProviderCatalogRevisionSummaryDto {
    revision: number;
    captured_at: string;
    snapshot_sha256: string;
    signed_revisions: number[];
    active: boolean;
}

export interface ProviderCatalogHistoryDto {
    history_schema_version: number;
    active_revision: number;
    revisions: ProviderCatalogRevisionSummaryDto[];
    activations: {
        action_id: string;
        state_version: number;
        kind: string;
        from_revision: number | null;
        to_revision: number;
        activated_at: string;
        diff: ProviderCatalogDiffDto;
    }[];
    next_before_revision: number | null;
    next_before_state_version: number | null;
}

export interface ProviderCatalogImportPlanDto {
    review: {
        plan_schema_version: number;
        action_id: string;
        expected_state_version: number;
        expected_active_revision: number;
        expected_active_snapshot_sha256: string;
        expected_highest_accepted_revision: number;
        envelope_byte_count: number;
        envelope_sha256: string;
        signing_key_id: string;
        payload_sha256: string;
        signed_catalog_revision: number;
        candidate_revision: number;
        candidate_snapshot_sha256: string;
        prepared_at: string;
        expires_at: string;
        diff: ProviderCatalogDiffDto;
    };
    plan_sha256: string;
}

export interface ProviderCatalogImportTicketDto {
    ticket_id: string;
    plan: ProviderCatalogImportPlanDto;
}

export interface ProviderCatalogImportResultDto {
    signed_catalog_revision: number;
    activated_revision: number;
    diff: ProviderCatalogDiffDto;
    status: ProviderCatalogStatusDto;
}

export interface ProviderCatalogRollbackPlanDto {
    plan_schema_version: number;
    action_id: string;
    expected_state_version: number;
    plan_sha256: string;
    catalog_plan: {
        rollback_plan_version: number;
        from_revision: number;
        to_revision: number;
        expected_active_sha256: string;
        target_sha256: string;
        created_at: string;
        expires_at: string;
        diff: ProviderCatalogDiffDto;
    };
}

export interface ProviderCatalogRollbackResultDto {
    from_revision: number;
    activated_revision: number;
    status: ProviderCatalogStatusDto;
}
