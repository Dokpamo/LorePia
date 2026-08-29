export type PortableRuntimeCapabilityDto =
    | 'runtime:callbacks'
    | 'chat:read'
    | 'chat:write'
    | 'state:readwrite'
    | 'profile:read'
    | 'lore:read'
    | 'ui:write'
    | 'model:primary'
    | 'model:auxiliary'
    | 'elevated';

export interface CharacterRenderAssetDto {
    asset_id: string;
    aliases: string[];
}

export interface CharacterDisplayTransformDto {
    pattern: string;
    replacement: string;
    flags: string;
}

export interface CharacterRuntimeScriptDto {
    id: string;
    name: string;
    event: string;
    language: string;
    source: string;
    elevated_access: boolean;
}

export interface CharacterRuntimeKnowledgeDto {
    id: string;
    name: string;
    content: string;
    enabled: boolean;
    primary_keys: string[];
    secondary_keys: string[];
    constant: boolean;
    selective: boolean;
    case_sensitive: boolean;
    whole_word: boolean;
    use_regex: boolean;
    probability_basis_points: number;
    folder: boolean;
}

export interface CharacterRenderProfileDto {
    character_id: string;
    character_content_revision_id: string | null;
    assets: CharacterRenderAssetDto[];
    background_markup: string;
    toggle_schema: string;
    initial_variables: Record<string, string>;
    output_transforms: CharacterDisplayTransformDto[];
    display_transforms: CharacterDisplayTransformDto[];
    runtime_scripts: CharacterRuntimeScriptDto[];
    required_runtime_capabilities: PortableRuntimeCapabilityDto[];
    runtime_capabilities_declared: boolean;
    runtime_knowledge: CharacterRuntimeKnowledgeDto[];
    runtime_script_count: number;
}
