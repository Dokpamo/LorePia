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
    return characters
        .filter((character) =>
            normalize(`${character.name} ${character.description}`).includes(term),
        )
        .sort((left, right) => {
            const leftDate = Date.parse(left.createdAt ?? '') || 0;
            const rightDate = Date.parse(right.createdAt ?? '') || 0;
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
