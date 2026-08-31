export interface CharacterDto {
    id: string;
    name: string;
    description: string;
    source_hash: string;
    avatar_asset_id: string | null;
    created_at: string;
}

export interface CharacterGreetingCatalogDto {
    character_id: string;
    character_content_revision_id: string | null;
    greetings: {
        id: string;
        kind: 'default' | 'alternate';
        enabled: boolean;
    }[];
}

export interface CharacterGreetingSelectionInput {
    character_content_revision_id: string | null;
    greeting_id: string | null;
}

export type AssetDeliverySelector =
    { kind: 'asset_id'; asset_id: string } | { kind: 'sha256'; sha256: string };

export interface ResolveAssetDeliveryInput {
    selector: AssetDeliverySelector;
}

export interface AssetDeliveryDto {
    asset_id: string;
    sha256: string;
    media_type: string;
    kind: 'image' | 'audio' | 'video';
    size_bytes: number;
    width: number | null;
    height: number | null;
    duration_ms: number | null;
    url: string;
}
