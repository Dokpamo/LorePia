import type { CharacterRenderAssetDto } from '../../lib/ipc/contracts';
import { assetFolderKinds, defaultImageSuffix } from '../../lib/i18n/asset-name-aliases';
import type { ProfileImageGroup, ProfileImageGroupDefinition } from './character-profile-types';

const genericFolders = new Set(['assets', 'asset', 'image', 'images', 'other', 'icon', 'icons']);

function stem(value: string) {
    return value.trim().replace(/(?:\.(?:png|jpe?g|webp|gif|avif)){1,2}$/i, '');
}

function nameOf(asset: CharacterRenderAssetDto) {
    const alias =
        asset.aliases.find((value) => value.includes('/') && !value.includes('://')) ??
        asset.aliases.find((value) => value.trim() && !value.includes('/')) ??
        asset.aliases[0] ??
        asset.asset_id;
    return stem(alias.split('/').at(-1) ?? alias);
}

function convention(asset: CharacterRenderAssetDto): {
    title: string;
    kind: ProfileImageGroup['kind'];
    family: string;
} {
    const path = asset.aliases.find((value) => value.includes('/') && !value.includes('://'));
    const parts = path?.split('/').filter(Boolean) ?? [];
    const folder = parts
        .slice(0, -1)
        .filter((value) => !genericFolders.has(value.toLowerCase()))
        .at(-1);
    const category = parts.map((value) => assetFolderKinds[value.toLowerCase()]).find(Boolean);
    const name = nameOf(asset);
    // Existing chat asset aliases use name_expression / name_expression1 families.
    const prefix = name.includes('_') ? name.slice(0, name.indexOf('_')) : null;
    return {
        title: folder ?? prefix ?? name,
        kind: category ?? 'other',
        family: folder ? `folder:${parts.slice(0, -1).join('/')}` : `name:${prefix ?? name}`,
    };
}

/** Presentation grouping only: it never creates a lore binding or changes asset identity. */
export function profileImageGroups(
    assets: CharacterRenderAssetDto[],
    definitions: ProfileImageGroupDefinition[] = [],
): ProfileImageGroup[] {
    const available = new Map(
        assets
            .filter((asset) => !asset.media_type || asset.media_type.startsWith('image/'))
            .map((asset) => [asset.asset_id, asset]),
    );
    const used = new Set<string>();
    const groups: ProfileImageGroup[] = [];
    const append = (
        id: string,
        title: string,
        kind: ProfileImageGroup['kind'],
        ids: string[],
        cover?: string,
    ) => {
        const images = [...new Set(ids)]
            .filter((assetId) => available.has(assetId) && !used.has(assetId))
            .flatMap((assetId) => {
                const asset = available.get(assetId);
                return asset ? [{ assetId, title: nameOf(asset) }] : [];
            });
        const first = images[0];
        if (!first) return;
        for (const image of images) used.add(image.assetId);
        const representative =
            images.find((image) => image.assetId === cover) ??
            images.find((image) => defaultImageSuffix.test(image.title)) ??
            first;
        groups.push({ id, title, kind, images, coverAssetId: representative.assetId });
    };
    definitions.forEach((group, index) =>
        append(
            `declared:${String(index)}:${group.id}`,
            group.title,
            group.kind,
            group.assetIds,
            group.coverAssetId,
        ),
    );
    const pending = [...available.values()].filter((asset) => !used.has(asset.asset_id));
    const families = new Map<string, CharacterRenderAssetDto[]>();
    for (const asset of pending) {
        const { family } = convention(asset);
        const siblings = families.get(family);
        if (siblings) siblings.push(asset);
        else families.set(family, [asset]);
    }
    for (const asset of pending) {
        if (used.has(asset.asset_id)) continue;
        const info = convention(asset);
        const siblings = info.family ? families.get(info.family) : undefined;
        if (siblings && siblings.length > 1)
            append(
                info.family,
                info.title,
                info.kind,
                siblings.map((item) => item.asset_id),
            );
        else append(`asset:${asset.asset_id}`, nameOf(asset), info.kind, [asset.asset_id]);
    }
    return groups;
}
