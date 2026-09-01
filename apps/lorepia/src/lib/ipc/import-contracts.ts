import type { PortableRuntimeCapabilityDto } from './portable-runtime-contracts';
import type { CharacterDto } from './contracts/character';

export type ImportContentKindDto =
    | 'character_card_v3'
    | 'character_card_png'
    | 'charx_package'
    | 'risu_module'
    | 'risu_preset'
    | 'risu_memory_preset';

export interface ImportTicketDto {
    ticket_id: string;
    display_name: string;
    size_bytes: number;
}

export interface ImportIssueDto {
    code: string;
    message: string;
}

export interface ImportImagePreviewDto {
    logical_asset_id: string;
    media_type: string;
    size_bytes: number;
}

export interface ImportRegexRuleReviewDto {
    id: string;
    name: string;
    phase: 'request_context' | 'provider_output' | 'display' | 'lore';
    runtime_index: number;
    pattern: string;
    flags: string;
}

export interface ImportDynamicContentReviewDto {
    runtime_script_count: number;
    elevated_runtime_script_count: number;
    required_runtime_capabilities: PortableRuntimeCapabilityDto[];
    runtime_capabilities_declared: boolean;
    regex_rule_count: number;
    enabled_regex_rule_count: number;
    model_calls_possible: boolean;
    custom_markup_present: boolean;
    regex_rules: ImportRegexRuleReviewDto[];
}

export interface ImportInspectionDto {
    inspection_id: string;
    kind: ImportContentKindDto;
    display_name: string;
    description: string;
    source_sha256: string;
    source_size: number;
    estimated_stored_size: number;
    asset_count: number;
    dynamic_content: ImportDynamicContentReviewDto;
    representative_image: ImportImagePreviewDto | null;
    warnings: ImportIssueDto[];
    blocked_reasons: string[];
    unsupported_optional_fields: string[];
    allowed: boolean;
}

export interface ImportedContentSummaryDto {
    kind: 'risu_module' | 'risu_preset' | 'risu_memory_preset';
    import_id: string;
    display_name: string;
    document_count: number;
    asset_count: number;
}

export type ImportCommitResultDto =
    | { kind: 'character'; character: CharacterDto }
    | { kind: 'content'; content: ImportedContentSummaryDto };
