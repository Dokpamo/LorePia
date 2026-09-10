import { expect, it } from 'vitest';
import { openingGroups, preferredOpening, type ProfileOpeningGroup } from './profile-openings';
import { openingParts } from './profile-opening-text';

const translatedScene: ProfileOpeningGroup = {
    id: 'scene',
    kind: 'alternate',
    number: 1,
    variants: [
        { id: 'ja', language: 'ja' },
        { id: 'en', language: 'en' },
        { id: 'ko', language: 'ko' },
        { id: 'en-us', language: 'en-US' },
    ],
};

it.each([
    ['ko', 'ja', 'en', 'ko'],
    ['ko-KR', 'en', 'ja', 'ko'],
    ['en-US', 'ja', 'en', 'en-us'],
    ['EN_us', 'ja', 'ko', 'en-us'],
    ['fr', 'JA-jp', 'en', 'ja'],
    ['fr', 'de', 'ja', 'en'],
    ['fr', undefined, 'ja', 'en'],
])(
    'defaults to app language, author recommendation, then English (%s, %s)',
    (locale, recommended, previous, expected) => {
        expect(preferredOpening(translatedScene, locale, recommended, previous).id).toBe(expected);
    },
);

it('keeps stable source order when no preferred language exists and skips unknown tags', () => {
    const variants = [{ id: 'unknown' }, { id: 'ja', language: 'ja' }];
    expect(preferredOpening({ ...translatedScene, variants }, 'ko', 'fr', 'ja').id).toBe('unknown');
    expect(() => preferredOpening({ ...translatedScene, variants: [] }, 'ko')).toThrow();
});

it('groups display variants without renumbering native IDs or mixing distinct scenes', () => {
    const greetings = ['default', 'alternate-0', 'alternate-1', 'alternate-2'].map((id) => ({
        id,
        kind: id === 'default' ? ('default' as const) : ('alternate' as const),
        enabled: true,
    }));
    const groups = openingGroups(greetings, {
        introductions: {
            'alternate-0': { body: 'First version', language: 'ko', groupId: 'alternate-0' },
            'alternate-1': { body: 'Second version', language: 'en', groupId: 'alternate-0' },
        },
    });
    expect(groups).toHaveLength(3);
    expect(groups[1]?.variants.map((v) => v.id)).toEqual(['alternate-0', 'alternate-1']);
    expect(groups[2]?.number).toBe(2);
    if (!groups[1]) throw new Error('Missing scene group');
    expect(preferredOpening(groups[1], 'en').id).toBe('alternate-1');
    expect(openingGroups(greetings)).toHaveLength(4);
    const defaultVariants = openingGroups(greetings, {
        introductions: {
            default: { body: 'Default scene', language: 'ko', groupId: 'default' },
            'alternate-0': { body: 'Translated scene', language: 'en', groupId: 'default' },
        },
    });
    expect(defaultVariants[0]?.variants.map((v) => v.id)).toEqual(['default', 'alternate-0']);
    expect(new Set(defaultVariants.map((group) => group.id)).size).toBe(defaultVariants.length);
});

it('resolves only known unambiguous image aliases and preserves all other source text', () => {
    const text = 'First <img="smile"> Middle <img="unknown"> <script>plain</script> End';
    expect(
        openingParts(text, [{ asset_id: 'safe', aliases: ['smile'], media_type: 'image/png' }]),
    ).toEqual([
        { kind: 'text', text: 'First ' },
        {
            kind: 'image',
            assetId: 'safe',
            name: 'smile',
            selector: { kind: 'asset_id', asset_id: 'safe' },
        },
        { kind: 'text', text: ' Middle <img="unknown"> <script>plain</script> End' },
    ]);
    expect(
        openingParts(text, [
            { asset_id: 'one', aliases: ['smile'] },
            { asset_id: 'two', aliases: ['smile'] },
        ]),
    ).toEqual([{ kind: 'text', text }]);
});
