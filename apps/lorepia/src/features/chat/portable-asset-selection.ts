import type { CharacterRenderAssetDto } from '../../lib/ipc/contracts';

const MAX_PORTABLE_ASSET_ALIASES = 32_768;

interface IndexedAssetAlias {
    asset: CharacterRenderAssetDto;
    alias: string;
}

/** Build-local selector: no delivery decisions or aliases survive a document rebuild. */
export function createPortableAssetSelector(
    assets: readonly CharacterRenderAssetDto[],
    source: string,
): (reference: string) => CharacterRenderAssetDto | null {
    let aliases: IndexedAssetAlias[] | undefined;
    let sourceHash: number | undefined;
    return (reference) => {
        aliases ??= indexAssetAliases(assets);
        return selectAsset(aliases, reference, (length) => {
            // Continue the exact UTF-16 FNV sequence: source, NUL, then reference.
            sourceHash ??= appendHash(appendHash(2166136261, source), '\0');
            return (appendHash(sourceHash, reference) >>> 0) % length;
        });
    };
}

function selectAsset(
    aliases: readonly IndexedAssetAlias[],
    reference: string,
    selectIndex: (length: number) => number,
): CharacterRenderAssetDto | null {
    const wanted = normalizedAlias(reference);
    if (wanted === '') return null;
    let ranked = aliases.filter(
        ({ alias }) =>
            alias === wanted || alias.startsWith(`${wanted}_`) || alias.startsWith(`${wanted}.`),
    );
    if (ranked.length === 0 && wanted.includes('_')) {
        const fallback = `${wanted.slice(0, wanted.indexOf('_'))}_default`;
        const fallbackExtension = `${fallback}.`;
        ranked = aliases.filter(
            ({ alias }) => alias === fallback || alias.startsWith(fallbackExtension),
        );
    }
    ranked = ranked.sort(
        (left, right) =>
            left.alias.length - right.alias.length ||
            left.alias.localeCompare(right.alias) ||
            left.asset.asset_id.localeCompare(right.asset.asset_id),
    );
    if (ranked.length === 0) return null;
    const exact = ranked.filter(({ alias }) => alias === wanted);
    const candidates = exact.length > 0 ? exact : ranked;
    if (candidates.length === 1) return candidates[0]?.asset ?? null;
    return candidates[selectIndex(candidates.length)]?.asset ?? null;
}

function indexAssetAliases(assets: readonly CharacterRenderAssetDto[]): IndexedAssetAlias[] {
    const aliases: IndexedAssetAlias[] = [];
    for (const asset of assets) {
        for (const sourceAlias of asset.aliases) {
            const alias = normalizedAlias(sourceAlias);
            if (alias !== '') aliases.push({ asset, alias });
            if (aliases.length >= MAX_PORTABLE_ASSET_ALIASES) return aliases;
        }
    }
    return aliases;
}

function normalizedAlias(value: string): string {
    return (
        value
            .trim()
            .replace(/^['"]|['"]$/g, '')
            .replaceAll('\\', '/')
            .split('/')
            .at(-1)
            ?.toLocaleLowerCase() ?? ''
    );
}

function appendHash(hash: number, value: string): number {
    for (let index = 0; index < value.length; index += 1) {
        hash ^= value.charCodeAt(index);
        hash = Math.imul(hash, 16777619);
    }
    return hash;
}
