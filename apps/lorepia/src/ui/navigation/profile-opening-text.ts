import type { AssetDeliverySelector, CharacterRenderAssetDto } from '../../lib/ipc/contracts';

export type OpeningPart =
    | { kind: 'text'; text: string }
    | { kind: 'image'; assetId: string; name: string; selector: AssetDeliverySelector };

/** Only known literal image aliases resolve to approved assets. All other markup stays inert. */
export function openingParts(text: string, assets: CharacterRenderAssetDto[]): OpeningPart[] {
    const aliases = new Map<string, string | null>();
    for (const asset of assets) {
        if (asset.media_type && !asset.media_type.startsWith('image/')) continue;
        for (const alias of asset.aliases) {
            const previous = aliases.get(alias);
            aliases.set(
                alias,
                previous === undefined || previous === asset.asset_id ? asset.asset_id : null,
            );
        }
    }
    const parts: OpeningPart[] = [];
    let cursor = 0;
    let images = 0;
    for (const match of text.matchAll(/<img="([^"\r\n]{1,160})">/g)) {
        const name = match[1] ?? '';
        const assetId = aliases.get(name);
        if (!assetId || images >= 64) continue;
        if (match.index > cursor)
            parts.push({ kind: 'text', text: text.slice(cursor, match.index) });
        parts.push({
            kind: 'image',
            assetId,
            name,
            selector: /^[a-f0-9]{64}$/.test(assetId)
                ? { kind: 'sha256', sha256: assetId }
                : { kind: 'asset_id', asset_id: assetId },
        });
        images++;
        cursor = match.index + match[0].length;
    }
    if (cursor < text.length) parts.push({ kind: 'text', text: text.slice(cursor) });
    return parts;
}
