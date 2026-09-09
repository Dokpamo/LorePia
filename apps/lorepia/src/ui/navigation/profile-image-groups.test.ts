import { expect, it } from 'vitest';
import { profileImageGroups } from './profile-image-groups';
import type { CharacterRenderAssetDto } from '../../lib/ipc/contracts';

function asset(id: string, path: string, media_type = 'image/webp'): CharacterRenderAssetDto {
    return { asset_id: id, aliases: [path, path.split('/').at(-1) ?? path], media_type };
}

it('groups imported expression aliases, keeps default covers and excludes audio', () => {
    const assets = [
        asset('smile', 'assets/other/image/Guide_smile1.png.png'),
        asset('normal', 'assets/other/image/Guide_normal1.png.png'),
        asset('map', 'assets/other/image/map.png.png'),
        asset('music', 'music.mp3', 'audio/mpeg'),
    ];
    const groups = profileImageGroups(assets);
    expect(groups.map((group) => [group.title, group.images.length, group.coverAssetId])).toEqual([
        ['Guide', 2, 'normal'],
        ['map', 1, 'map'],
    ]);
    expect(groups[0]?.images.map((image) => image.title)).toEqual([
        'Guide_smile1',
        'Guide_normal1',
    ]);
});

it('distinguishes duplicate icon aliases by path and joins base images with variations', () => {
    const assets = [
        asset('base', 'assets/icon/image/iconx.png'),
        asset('variant', 'assets/icon/image/iconx_1.png'),
    ].map((item) => ({ ...item, aliases: ['iconx', ...item.aliases] }));
    const groups = profileImageGroups(assets);
    expect(groups).toHaveLength(1);
    expect(groups[0]?.images.map((image) => image.title)).toEqual(['iconx', 'iconx_1']);
});

it('keeps equally named folders separate and preserves every unique image', () => {
    const assets = [
        asset('a', 'places/one/default.png'),
        asset('b', 'places/one/night.png'),
        asset('c', 'characters/one/default.png'),
        asset('d', 'characters/one/smile.png'),
    ];
    const first = assets[0];
    if (!first) throw new Error('Missing asset fixture');
    const groups = profileImageGroups([...assets, first]);
    expect(groups.map((group) => group.kind)).toEqual(['place', 'person']);
    expect(groups.flatMap((group) => group.images.map((image) => image.assetId))).toEqual([
        'a',
        'b',
        'c',
        'd',
    ]);
});

it('validates explicit groups without hiding unknown or unassigned images', () => {
    const assets = [asset('a', 'a.png'), asset('b', 'b.png'), asset('c', 'c.png')];
    const groups = profileImageGroups(assets, [
        {
            id: 'group',
            title: 'Declared',
            kind: 'item',
            assetIds: ['missing', 'a', 'a', 'b'],
            coverAssetId: 'b',
        },
        { id: 'duplicate', title: 'Duplicate', kind: 'other', assetIds: ['a'] },
    ]);
    expect(groups).toHaveLength(2);
    expect(groups[0]?.coverAssetId).toBe('b');
    expect(groups.flatMap((group) => group.images.map((image) => image.assetId))).toEqual([
        'a',
        'b',
        'c',
    ]);
});
