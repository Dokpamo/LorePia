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
    const filtered = conversations.filter(
        (item) =>
            (!characterId || item.characterId === characterId) &&
            (!term || normalize(`${item.title} ${item.characterName}`).includes(term)),
    );
    const dates = new Map(
        sort === 'name' ? [] : filtered.map((item) => [item.id, Date.parse(item.updatedAt) || 0]),
    );
    return filtered.sort((left, right) => {
        const leftDate = dates.get(left.id) ?? 0;
        const rightDate = dates.get(right.id) ?? 0;
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
