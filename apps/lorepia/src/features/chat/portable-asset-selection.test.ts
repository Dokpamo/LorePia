import { afterEach, describe, expect, it, vi } from 'vitest';
import type { CharacterRenderAssetDto } from '../../lib/ipc/contracts';
import { createPortableAssetSelector } from './portable-asset-selection';

// Frozen pre-optimization hash: hashing must preserve UTF-16 code units, including lone surrogates.
function previousIndex(source: string, reference: string, length: number): number {
    const value = `${source}\0${reference}`;
    let hash = 2166136261;
    for (let index = 0; index < value.length; index += 1) {
        hash ^= value.charCodeAt(index);
        hash = Math.imul(hash, 16777619);
    }
    return (hash >>> 0) % length;
}

afterEach(() => vi.restoreAllMocks());

describe('portable asset selection', () => {
    it('preserves old full-string hash choices across sources, references and candidate counts', () => {
        const sources = [
            '',
            'plain',
            '\ud55c\uae00😀',
            '\ud800',
            '\udc00',
            'a\0b',
            'x'.repeat(262_144),
        ];
        const references = [
            'face',
            'FACE',
            ' face ',
            'folder/face',
            'folder\\face',
            '\ud45c\uc815😀',
            '\ud800',
        ];
        for (const count of [2, 3, 7, 16, 129]) {
            const assets = Array.from({ length: count }, (_, index) => ({
                asset_id: `asset-${String(index).padStart(3, '0')}`,
                aliases: ['face', '\ud45c\uc815😀', '\ud800'],
            }));
            for (const source of sources) {
                const select = createPortableAssetSelector(assets, source);
                for (const reference of references)
                    expect(select(reference)).toBe(assets[previousIndex(source, reference, count)]);
            }
        }
    });

    it('keeps exact matches, duplicate aliases, fallback ranking and absent references', () => {
        const assets: CharacterRenderAssetDto[] = [
            { asset_id: 'long', aliases: ['face_smile_long.png'] },
            { asset_id: 'b', aliases: ['face_smile.png', 'face_default.png'] },
            { asset_id: 'a', aliases: ['face_smile.png', 'face_smile.png', 'face_default.png'] },
            { asset_id: 'exact', aliases: ['face_smile'] },
        ];
        const source = 'deterministic';
        const select = createPortableAssetSelector(assets, source);
        expect(select('face_smile')).toBe(assets[3]);
        expect(select('face_smile.png')).toBe(
            [assets[2], assets[2], assets[1]][previousIndex(source, 'face_smile.png', 3)],
        );
        expect(select('face_unknown')).toBe(
            [assets[2], assets[1]][previousIndex(source, 'face_unknown', 2)],
        );
        expect(select('missing')).toBeNull();
        expect(select('')).toBeNull();
    });

    it('does not index aliases without references and preserves the alias cap', () => {
        const aliases = vi.fn(() => ['face']);
        const asset = {
            asset_id: 'a',
            get aliases() {
                return aliases();
            },
        };
        createPortableAssetSelector([asset], 'source');
        expect(aliases).not.toHaveBeenCalled();
        const select = createPortableAssetSelector(
            [
                { asset_id: 'first', aliases: Array<string>(32_768).fill('face') },
                { asset_id: 'outside-cap', aliases: ['last'] },
            ],
            'source',
        );
        expect(select('last')).toBeNull();
    });

    it('skips singleton hashing and hashes a shared source prefix only once per build', () => {
        const source = 'x'.repeat(262_144);
        const hashSteps = vi.spyOn(String.prototype, 'charCodeAt');
        const singleton = createPortableAssetSelector(
            [{ asset_id: 'a', aliases: ['face'] }],
            source,
        );
        expect(singleton('face')?.asset_id).toBe('a');
        expect(hashSteps).not.toHaveBeenCalled();
        const assets = [
            { asset_id: 'a', aliases: ['face'] },
            { asset_id: 'b', aliases: ['face'] },
        ];
        const select = createPortableAssetSelector(assets, source);
        for (let index = 0; index < 128; index += 1) select('face');
        expect(hashSteps).toHaveBeenCalledTimes(source.length + 1 + 128 * 'face'.length);
        hashSteps.mockClear();
        createPortableAssetSelector(assets, 'new')('face');
        expect(hashSteps).toHaveBeenCalledTimes('new'.length + 1 + 'face'.length);
    });
});
