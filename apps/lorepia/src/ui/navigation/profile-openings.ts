import type { CharacterGreetingCatalogDto } from '../../lib/ipc/contracts';
import type { CharacterProfilePresentation } from './character-profile-types';

export interface ProfileOpeningVariant {
    id: string;
    title?: string;
    body?: string;
    language?: string | null;
}
export interface ProfileOpeningGroup {
    id: string;
    kind: 'default' | 'alternate';
    number: number;
    variants: ProfileOpeningVariant[];
}

export function openingGroups(
    greetings: CharacterGreetingCatalogDto['greetings'],
    presentation?: CharacterProfilePresentation,
): ProfileOpeningGroup[] {
    const groups: ProfileOpeningGroup[] = [];
    let number = 0;
    for (const greeting of greetings.filter((item) => item.enabled)) {
        const intro = presentation?.introductions?.[greeting.id];
        const groupId = intro?.groupId ?? greeting.id;
        const variant = { id: greeting.id, ...intro };
        // A card's default scene can itself have an alternate-language version.
        const group = groups.find((item) => item.id === groupId);
        if (group) group.variants.push(variant);
        else
            groups.push({
                id: groupId,
                kind: greeting.kind,
                number: greeting.kind === 'alternate' ? ++number : 0,
                variants: [variant],
            });
    }
    return groups;
}

export function preferredOpening(
    group: ProfileOpeningGroup,
    locale: string,
    recommendedLanguage?: string | null,
    selectedId?: string | null,
) {
    const normalize = (language?: string | null) =>
        language?.trim().toLowerCase().replaceAll('_', '-');
    for (const language of [locale, recommendedLanguage, 'en']) {
        const tag = normalize(language);
        if (!tag || tag === 'und') continue;
        const exact = group.variants.filter((item) => normalize(item.language) === tag);
        const matches = exact.length
            ? exact
            : group.variants.filter(
                  (item) => normalize(item.language)?.split('-')[0] === tag.split('-')[0],
              );
        // A previously chosen scene cannot override the user's language preference.
        const match = matches.find((item) => item.id === selectedId) ?? matches[0];
        if (match) return match;
    }
    const result = group.variants[0];
    if (!result) throw new Error('Opening group is empty');
    return result;
}
