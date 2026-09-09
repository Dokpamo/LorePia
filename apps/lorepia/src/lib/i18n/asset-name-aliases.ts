// File-name vocabulary is matched in either language, independently of UI locale.
export const assetFolderKinds: Record<string, 'person' | 'place' | 'item' | undefined> = {
    characters: 'person',
    character: 'person',
    people: 'person',
    인물: 'person',
    캐릭터: 'person',
    places: 'place',
    locations: 'place',
    backgrounds: 'place',
    장소: 'place',
    배경: 'place',
    items: 'item',
    objects: 'item',
    props: 'item',
    물품: 'item',
    소품: 'item',
};
export const defaultImageSuffix = /(?:^|_)(?:default|normal|기본)(?:\d+)?$/i;
