import type { CharacterRenderProfileDto } from '../../lib/ipc/contracts';

/** Optional presentation metadata; this does not extend the IPC character DTO. */
export interface CharacterProfilePresentation {
    tags?: string[];
    creator?: { name: string; avatarAssetId?: string };
    note?: string;
    guide?: string;
    recommendedLanguage?: string | null;
    introductions?: Record<
        string,
        { title?: string; body: string; language?: string | null; groupId?: string | null }
    >;
    imageGroups?: ProfileImageGroupDefinition[];
    heroAssetIds?: string[];
}

export interface ProfileImage {
    assetId: string;
    title: string;
}

export interface ProfileImageGroupDefinition {
    id: string;
    title: string;
    kind: 'person' | 'place' | 'item' | 'other';
    assetIds: string[];
    coverAssetId?: string;
}

export interface ProfileImageGroup {
    id: string;
    title: string;
    kind: ProfileImageGroupDefinition['kind'];
    images: ProfileImage[];
    coverAssetId: string;
}

export interface ProfileResourceDetail {
    title: string;
    collection?: { kind: 'lorebook' | 'scripts'; profile: CharacterRenderProfileDto };
    openings?: boolean;
    imageGroups?: ProfileImageGroup[];
    assetId?: string;
    images?: ProfileImage[];
    text?: string;
    code?: string;
}
