import type { JsonValue } from './common';

export type CredentialStatus = 'missing' | 'available' | 'unreadable';

export interface ConnectionFieldSpecDto {
    key: string;
    label_key: string;
    description_key: string | null;
    value_type: string;
    required: boolean;
}

export interface ProviderTemplateDto {
    id: string;
    display_name: string;
    manifest_version: number;
    source: string;
    api_family: string;
    connection_fields: ConnectionFieldSpecDto[];
    default_network_mode: string;
    default_api_origin: string | null;
    credential_required: boolean;
    supports_model_listing: boolean;
    auth_binding: AuthBindingDto;
    parameters: ParameterSpecDto[];
}

export type AuthBindingDto =
    { kind: 'none' } | { kind: 'bearer_header' } | { kind: 'header_api_key'; header_name: string };

export type ConnectionConfigValueDto =
    | { type: 'text'; value: string }
    | { type: 'integer'; value: number }
    | { type: 'boolean'; value: boolean };

export interface ProviderConfigEntryDto {
    key: string;
    value: ConnectionConfigValueDto;
}

export interface CredentialScopeDto {
    allowed_origins: string[];
    auth_binding: AuthBindingDto;
    redirect_policy: string;
}

export interface ProviderConnectionDto {
    id: string;
    template_id: string;
    template_version: number;
    display_name: string;
    api_origin: string;
    api_base_path: string | null;
    network_mode: string;
    local_network_approval: ProviderLocalNetworkApprovalInput | null;
    config_values: ProviderConfigEntryDto[];
    credential_binding_required: boolean;
    credential_status?: CredentialStatus;
    credential_scope: CredentialScopeDto | null;
    approved_credential_origins: string[];
    timeout_seconds: number;
    status: string;
    created_at: string;
    updated_at: string;
}

export interface ModelRouteDto {
    id: string;
    connection_id: string;
    api_family: string;
    model_id: string;
    display_name: string | null;
    route_config: {
        deployment_id: string | null;
        region: string | null;
        endpoint_path: string | null;
        values: ProviderConfigEntryDto[];
    };
    status: string;
    miss_count: number;
    metadata_source: string;
    metadata_observed_at: string | null;
    first_seen_at: string;
    last_seen_at: string | null;
}

export const CAPABILITY_KEYS = [
    'streaming',
    'reasoning',
    'prompt_caching',
    'tool_calling',
    'parallel_tool_calling',
    'structured_output',
    'json_mode',
    'image_input',
    'audio_input',
    'audio_output',
    'logprobs',
    'seed',
    'batch',
    'background',
    'context_window',
    'max_output_tokens',
] as const;

export type CapabilityKeyInput = (typeof CAPABILITY_KEYS)[number];

export type CapabilityValueDto =
    | { type: 'boolean'; value: boolean }
    | { type: 'integer'; value: number }
    | { type: 'enum_values'; value: string[] }
    | { type: 'structured'; value: JsonValue };

export type CapabilityOverrideValueInput =
    | { type: 'boolean'; value: boolean }
    | { type: 'integer'; value: number }
    | { type: 'enum_values'; value: string[] };

export type CapabilityOverrideStatusInput = 'verified' | 'unsupported' | 'unknown' | 'conditional';

export interface UpsertCapabilityOverrideInput {
    id: string;
    model_route_id: string;
    key: CapabilityKeyInput;
    value: CapabilityOverrideValueInput;
    status: CapabilityOverrideStatusInput;
    expires_at: string | null;
}

export interface CapabilityObservationDto {
    id: string;
    model_route_id: string;
    key: string;
    value: CapabilityValueDto;
    status: string;
    source: string;
    confidence: string;
    observed_at: string;
    expires_at: string | null;
    evidence_ref: string | null;
}

export interface EffectiveCapabilityDto {
    selected: CapabilityObservationDto;
    alternatives: CapabilityObservationDto[];
    evaluated_at: string;
    selected_is_stale: boolean;
    has_conflict: boolean;
}

export type ParameterLiteralDto =
    | { type: 'boolean'; value: boolean }
    | { type: 'integer'; value: number }
    | { type: 'number'; value: number }
    | { type: 'string'; value: string }
    | { type: 'enum'; value: string }
    | { type: 'string_list'; value: string[] }
    | { type: 'json_schema'; value: string }
    | { type: 'stop_sequence_list'; value: string[] }
    | { type: 'tool_policy'; value: string };

export interface ParameterChoiceDto {
    value: ParameterLiteralDto;
    label_key: string;
}

export interface ParameterConditionDto {
    parameter_id: string;
    operator: string;
    value: ParameterLiteralDto;
}

export interface ParameterConflictDto {
    parameter_id: string;
    kind: string;
    message_key: string;
}

export interface ProviderParameterMappingDto {
    target: string;
    field_name: string;
}

export interface ParameterSpecDto {
    id: string;
    label_key: string;
    description_key: string | null;
    value_type: string;
    allowed_values: ParameterChoiceDto[];
    minimum: number | null;
    maximum: number | null;
    step: number | null;
    default_mode: string;
    visibility: ParameterConditionDto | null;
    conflicts: ParameterConflictDto[];
    provider_mapping: ProviderParameterMappingDto;
    level: string;
}

export type ParameterValueStateDto =
    { state: 'inherit_provider_default' } | { state: 'explicit'; value: ParameterLiteralDto };

export interface GenerationParameterDto {
    parameter_id: string;
    state: ParameterValueStateDto;
}

export interface GenerationPresetDto {
    id: string;
    model_route_id: string;
    display_name: string;
    values: GenerationParameterDto[];
    reasoning: {
        mode: string;
        effort: string | null;
        budget_tokens: number | null;
        summary: string;
        preserve_opaque_state: boolean;
    };
    prompt_cache: {
        mode: string;
        ttl_kind: string;
        ttl_seconds: number | null;
        context_reference: string | null;
    };
    created_at: string;
    updated_at: string;
}

export interface PromptPresetSummaryDto {
    id: string;
    name: string;
    schema_version: number;
    block_count: number;
    default_generation_preset_id: string | null;
}

export interface ListPromptPresetRevisionsInput {
    prompt_preset_id: string;
    limit: number;
}

export interface PromptPresetRevisionSummaryDto {
    revision_id: string;
    revision: number;
    sha256: string;
    name: string;
    created_at: string;
    rollback_allowed: boolean;
}

export interface PromptPresetRevisionListDto {
    revisions: PromptPresetRevisionSummaryDto[];
    truncated: boolean;
}

export interface DiffPromptPresetRevisionsInput {
    prompt_preset_id: string;
    from_revision: number;
    to_revision: number;
}

export interface PromptPresetRevisionDiffDto {
    preset_id: string;
    from_revision_id: string;
    from_revision: number;
    from_sha256: string;
    to_revision_id: string;
    to_revision: number;
    to_sha256: string;
    changed_paths: string[];
    truncated: boolean;
    diff_sha256: string;
}

export interface ReviewPromptPresetRollbackInput {
    prompt_preset_id: string;
    expected_current_revision: number;
    target_revision: number;
}

export interface PromptPresetRollbackReviewDto {
    review_sha256: string;
    preset_id: string;
    expected_current_state_revision: number;
    expected_current_revision_id: string;
    expected_current_sha256: string;
    target_revision_id: string;
    target_revision: number;
    target_sha256: string;
    target_document_sha256: string;
    target_dependency_sha256: string;
    binding_snapshot_sha256: string;
    diff: PromptPresetRevisionDiffDto;
    reviewed_at: string;
}

export interface ApplyPromptPresetRollbackInput {
    prompt_preset_id: string;
    expected_current_revision: number;
    target_revision: number;
    approval_id: string;
    expected_review_sha256: string;
}

export interface PromptPresetRollbackReceiptDto {
    preset_id: string;
    target_revision: number;
    applied_revision_id: string;
    applied_revision: number;
    applied_sha256: string;
    review_sha256: string;
    approval_id: string;
    approval_sha256: string;
    approved_at: string;
}

export interface PromptPresetHistoryClientApi {
    listPromptPresetRevisions(
        input: ListPromptPresetRevisionsInput,
    ): Promise<PromptPresetRevisionListDto>;
    diffPromptPresetRevisions(
        input: DiffPromptPresetRevisionsInput,
    ): Promise<PromptPresetRevisionDiffDto>;
    reviewPromptPresetRollback(
        input: ReviewPromptPresetRollbackInput,
    ): Promise<PromptPresetRollbackReviewDto>;
    applyPromptPresetRollback(
        input: ApplyPromptPresetRollbackInput,
    ): Promise<PromptPresetRollbackReceiptDto>;
}

export interface AppSettingsDto {
    preserve_partial_generations: boolean;
    selected_provider_profile_id: string | null;
    selected_model_route_id: string | null;
    selected_generation_preset_id: string | null;
}

export interface ProviderProfileDto {
    id: string;
    display_name: string;
    base_url: string;
    model: string;
    timeout_seconds: number;
}

export type ProviderNetworkModeInput = 'public' | 'local_loopback' | 'approved_local_network';

export interface ProviderLocalNetworkApprovalInput {
    origin: string;
    addresses: string[];
}

export interface CreateProviderConnectionInput {
    id: string;
    template_id: string;
    template_version: number;
    display_name: string;
    api_origin: string;
    api_base_path: string | null;
    network_mode: ProviderNetworkModeInput;
    local_network_approval: ProviderLocalNetworkApprovalInput | null;
    values: ProviderConfigEntryDto[];
    approved_credential_origin: string | null;
    timeout_seconds: number;
}

export interface UpdateProviderConnectionInput {
    id: string;
    display_name: string;
    timeout_seconds: number;
}

export type ApiFamilyInput =
    | 'open_ai_responses'
    | 'open_ai_chat_completions'
    | 'anthropic_messages'
    | 'gemini_generate_content'
    | 'ollama_native';

export type ModelAvailabilityInput =
    | 'available'
    | 'missing_temporarily'
    | 'documented_only'
    | 'access_denied'
    | 'deprecated'
    | 'retired'
    | 'unknown';

export type UpsertModelRouteInput =
    | {
          kind: 'create';
          id: string;
          connection_id: string;
          api_family: ApiFamilyInput;
          model_id: string;
          display_name: string | null;
          route_config: ModelRouteDto['route_config'];
          status: ModelAvailabilityInput;
      }
    | {
          kind: 'update';
          id: string;
          display_name: string | null;
          status: ModelAvailabilityInput;
      };

export interface GenerationPresetInput {
    id: string;
    model_route_id: string;
    display_name: string;
    values: GenerationParameterDto[];
    reasoning: GenerationPresetDto['reasoning'];
    prompt_cache: GenerationPresetDto['prompt_cache'];
}

export interface ParameterIssueDto {
    code: string;
    parameter_id: string | null;
    related_parameter_id: string | null;
}

export interface ReasoningControlDto {
    state: string;
    settings: GenerationPresetDto['reasoning'];
    allowed_modes: string[];
    allowed_efforts: string[];
    allowed_summaries: string[];
    budget_bounds: { minimum: number; maximum: number } | null;
    effort_field: string;
    budget_field: string;
    summary_field: string;
    issues: ParameterIssueDto[];
}

export type PromptCacheTtlDto =
    | { kind: 'provider_default' }
    | { kind: 'short' }
    | { kind: 'long' }
    | { kind: 'custom_seconds'; seconds: number };

export interface PromptCacheControlDto {
    state: string;
    settings: GenerationPresetDto['prompt_cache'];
    allowed_modes: string[];
    allowed_ttls: PromptCacheTtlDto[];
    supports_custom_ttl: boolean;
    custom_ttl_bounds: {
        minimum_seconds: number;
        maximum_seconds: number;
    } | null;
    ttl_field: string;
    context_reference_field: string;
    issues: ParameterIssueDto[];
}

export interface ProviderOverviewDto {
    settings: AppSettingsDto;
    templates: ProviderTemplateDto[];
    connections: ProviderConnectionDto[];
    legacy_profiles: ProviderProfileDto[];
}

export type CredentialTargetDto =
    | { kind: 'legacy_profile'; provider_profile_id: string }
    | { kind: 'connection'; connection_id: string }
    | {
          kind: 'discovery_session';
          session_id: string;
          expected_revision: number;
      };

export interface CredentialStatusDto {
    status: CredentialStatus;
}

export type ClipboardCleanupStatus = 'cleared' | 'already_replaced' | 'clear_failed';

export interface NativeCaptureStatusDto {
    clipboard_cleanup: ClipboardCleanupStatus;
}

export type RequestBodyShapeDto =
    | { kind: 'null' }
    | { kind: 'boolean' }
    | { kind: 'number' }
    | { kind: 'string' }
    | { kind: 'array'; items: RequestBodyShapeDto[]; truncated: boolean }
    | { kind: 'object'; fields: RequestBodyFieldDto[]; truncated: boolean }
    | { kind: 'redacted' }
    | { kind: 'truncated' };

export interface RequestBodyFieldDto {
    name: string;
    shape: RequestBodyShapeDto;
}

export interface RequestPreviewDto {
    method: string;
    origin: string;
    path: string;
    query_parameter_names: string[];
    header_names: string[];
    body: RequestBodyShapeDto | null;
    body_truncated: boolean;
}

export interface ModelSyncStartedDto {
    job_id: string;
}

export interface ModelSyncFailureDto {
    code: string;
    message_key: string;
    recoverable: boolean;
}

export interface ModelSyncSourceProvenanceDto {
    source: string;
    api_family: string;
    api_origin: string;
    endpoint_path: string;
    pages_fetched: number;
    response_bytes: number;
}

export interface ModelSyncDiffDto {
    connection_id: string;
    expected_connection: ProviderConnectionDto;
    expected_model_routes: ModelRouteDto[];
    observed_at: string;
    listed_routes: ModelRouteDto[];
    newly_seen_model_route_ids: string[];
    missing_model_route_ids: string[];
    initial_presets: GenerationPresetDto[];
    capability_observation_count: number;
    routes_requiring_preset_configuration: string[];
    provenance: ModelSyncSourceProvenanceDto;
}

export interface ModelSyncReviewDto {
    sha256: string;
    diff: ModelSyncDiffDto;
}

export interface ModelSyncJobDto {
    id: string;
    connection_id: string;
    state: string;
    revision: number;
    review: ModelSyncReviewDto | null;
    failure: ModelSyncFailureDto | null;
    created_at: string;
    updated_at: string;
}

export interface ModelSyncEventDto {
    version: number;
    job_id: string;
    sequence: number;
    job_revision: number;
    redaction_version: number;
    state: string;
    progress: {
        completed_steps: number;
        total_steps: number;
        message_key: string;
    };
    review_sha256: string | null;
    failure: ModelSyncFailureDto | null;
    emitted_at: string;
}
