export interface RailEntry {
    id: string;
    name: string;
    cards: string[];
}

export function syncRail(entries: RailEntry[], ids: string[]): RailEntry[] {
    const allowed = new Set(ids);
    const seen = new Set<string>();
    const result: RailEntry[] = [];
    for (const entry of entries) {
        const cards = entry.cards.filter((id) => {
            if (!allowed.has(id) || seen.has(id)) return false;
            seen.add(id);
            return true;
        });
        if (cards.length)
            result.push({
                ...entry,
                id: cards.length === 1 ? (cards[0] ?? entry.id) : entry.id,
                cards,
            });
    }
    for (const id of ids) if (!seen.has(id)) result.push({ id, name: '', cards: [id] });
    return result;
}

export function groupCards(
    entries: RailEntry[],
    sourceId: string,
    targetId: string,
    folder: { id: string; name: string },
): RailEntry[] {
    const source = entries.find((entry) => entry.id === sourceId || entry.cards.includes(sourceId));
    const target = entries.find((entry) => entry.id === targetId || entry.cards.includes(targetId));
    if (!source || !target || source === target) return entries;
    const moving = source.id === sourceId ? source.cards : [sourceId];
    return entries.flatMap((entry) => {
        if (entry === target)
            return [
                {
                    id: target.cards.length > 1 ? target.id : folder.id,
                    name: target.cards.length > 1 ? target.name : folder.name,
                    cards: [...target.cards, ...moving],
                },
            ];
        if (entry !== source) return [entry];
        const cards = entry.cards.filter((id) => !moving.includes(id));
        return cards.length
            ? [{ ...entry, id: cards.length === 1 ? (cards[0] ?? entry.id) : entry.id, cards }]
            : [];
    });
}

export function extractCard(entries: RailEntry[], id: string): RailEntry[] {
    const folder = entries.find((entry) => entry.cards.includes(id) && entry.cards.length > 1);
    if (!folder) return entries;
    return entries.flatMap((entry) => {
        if (entry !== folder) return [entry];
        const cards = entry.cards.filter((card) => card !== id);
        return [
            { ...entry, id: cards.length === 1 ? (cards[0] ?? entry.id) : entry.id, cards },
            { id, name: '', cards: [id] },
        ];
    });
}
