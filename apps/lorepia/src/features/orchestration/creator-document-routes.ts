import type { CreatorDocumentKind } from './orchestration-controller';
type CreatorDocumentRoute =
    | { mode: 'index'; kind: null; id: null }
    | { mode: 'list' | 'create'; kind: CreatorDocumentKind; id: null }
    | { mode: 'edit'; kind: CreatorDocumentKind; id: string };

export function familyRoute(kind: CreatorDocumentKind): string {
    return `documents/${kind}`;
}

export function createRoute(kind: CreatorDocumentKind): string {
    return `${familyRoute(kind)}/create`;
}

export function editRoute(kind: CreatorDocumentKind, id: string): string {
    return `${familyRoute(kind)}/edit/${encodeURIComponent(id)}`;
}

export function parseDocumentRoute(
    page: string | null | undefined,
    families: readonly { kind: CreatorDocumentKind }[],
): CreatorDocumentRoute {
    if (page === null || page === undefined || page === 'documents') {
        return { mode: 'index', kind: null, id: null };
    }
    for (const family of families) {
        const base = familyRoute(family.kind);
        if (page === base) return { mode: 'list', kind: family.kind, id: null };
        if (page === `${base}/create`) {
            return { mode: 'create', kind: family.kind, id: null };
        }
        const editPrefix = `${base}/edit/`;
        if (page.startsWith(editPrefix)) {
            const encodedId = page.slice(editPrefix.length);
            try {
                return { mode: 'edit', kind: family.kind, id: decodeURIComponent(encodedId) };
            } catch {
                return { mode: 'index', kind: null, id: null };
            }
        }
    }
    return { mode: 'index', kind: null, id: null };
}
