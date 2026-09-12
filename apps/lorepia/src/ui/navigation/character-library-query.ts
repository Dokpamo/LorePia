import type { SampleCharacter } from '../workspace/view-types';
import type { LibrarySortOrder } from './navigation-types';

export type CharacterLibrarySort = LibrarySortOrder;

const normalize = (value: string) => value.normalize('NFKC').trim().toLowerCase();
const byName = new Intl.Collator('ko', { numeric: true, sensitivity: 'base' });

export function queryCharacterLibrary(
    characters: SampleCharacter[],
    query: string,
    sort: CharacterLibrarySort,
): SampleCharacter[] {
    const term = normalize(query);
    const filtered = term
        ? characters.filter((character) =>
              normalize(`${character.name} ${character.description}`).includes(term),
          )
        : [...characters];
    const dates = new Map(
        sort === 'name'
            ? []
            : filtered.map((item) => [item.id, Date.parse(item.createdAt ?? '') || 0]),
    );
    return filtered.sort((left, right) => {
        const leftDate = dates.get(left.id) ?? 0;
        const rightDate = dates.get(right.id) ?? 0;
        const order =
            sort === 'name'
                ? byName.compare(left.name, right.name)
                : sort === 'newest'
                  ? rightDate - leftDate
                  : leftDate - rightDate;
        return (
            order ||
            byName.compare(left.name, right.name) ||
            (left.id < right.id ? -1 : left.id > right.id ? 1 : 0)
        );
    });
}
