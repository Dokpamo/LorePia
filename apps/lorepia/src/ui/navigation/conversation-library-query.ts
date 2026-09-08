import type { ConversationListItem, LibrarySortOrder } from './navigation-types';

const normalize = (value: string) => value.normalize('NFKC').trim().toLowerCase();
const byTitle = new Intl.Collator('ko', { numeric: true, sensitivity: 'base' });

export function queryConversationLibrary(
    conversations: ConversationListItem[],
    query: string,
    characterId: string,
    sort: LibrarySortOrder,
): ConversationListItem[] {
    const term = normalize(query);
    return conversations
        .filter(
            (item) =>
                (!characterId || item.characterId === characterId) &&
                normalize(`${item.title} ${item.characterName}`).includes(term),
        )
        .sort((left, right) => {
            const leftDate = Date.parse(left.updatedAt) || 0;
            const rightDate = Date.parse(right.updatedAt) || 0;
            const order =
                sort === 'name'
                    ? byTitle.compare(left.title, right.title)
                    : sort === 'newest'
                      ? rightDate - leftDate
                      : leftDate - rightDate;
            return (
                order ||
                byTitle.compare(left.title, right.title) ||
                (left.id < right.id ? -1 : left.id > right.id ? 1 : 0)
            );
        });
}
