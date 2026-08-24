import type { MessageKey } from '../../lib/i18n';

/**
 * Studio destinations.
 *
 * The prompt workshop is too large for one scroll on a phone, so it is an
 * index of four screens. Naming them here keeps the list, the titles, and the
 * screen that renders each one from drifting apart.
 */
export const STUDIO_SECTIONS = ['prompt', 'memory', 'content', 'diagnostics'] as const;

export type StudioSection = (typeof STUDIO_SECTIONS)[number];

/** A pushed tool inside one of the four Studio destinations. */
export type StudioDetailPage = string | null;

/** The fifteen first-level tools rendered by the Studio section indexes. */
export const STUDIO_DETAIL_TITLE_KEYS = {
    history: 'studio.page.history',
    blocks: 'studio.page.blocks',
    room: 'studio.page.room',
    variables: 'studio.page.variables',
    profiles: 'studio.page.profiles',
    documents: 'studio.page.documents',
    records: 'studio.page.records',
    knowledge: 'studio.page.knowledge',
    transforms: 'studio.page.transforms.memory',
    interactions: 'studio.page.interactions',
    packages: 'studio.page.packages',
    modules: 'studio.page.modules',
    display: 'studio.page.display',
    selection: 'studio.page.selection',
    plan: 'studio.page.plan',
} as const;

export type StudioBaseDetailPage = keyof typeof STUDIO_DETAIL_TITLE_KEYS;

/** Resolve a first-level title key without trusting an arbitrary route string. */
export function studioBaseDetailTitleKey(page: StudioDetailPage): MessageKey | null {
    if (page === null) return null;
    const root = page.split(/[/:]/, 1)[0] ?? '';
    const titleKeys: Readonly<Partial<Record<string, MessageKey>>> = STUDIO_DETAIL_TITLE_KEYS;
    return titleKeys[root] ?? null;
}

const NESTED_STUDIO_ROOTS = new Set<StudioBaseDetailPage>([
    'history',
    'blocks',
    'profiles',
    'documents',
    'records',
    'interactions',
    'modules',
]);

const CREATOR_DOCUMENT_TITLE_KEY_PREFIXES: Readonly<Record<string, string>> = {
    memory_profile: 'studio.page.documents.memory_profile',
    knowledge_book: 'studio.page.documents.knowledge_book',
    transform_set: 'studio.page.documents.transform_set',
    interaction_rule_set: 'studio.page.documents.interaction_rule_set',
    content_module: 'studio.page.documents.content_module',
};

/**
 * Resolve one visual back step without coupling the app shell to editor state.
 * Most editors return directly to their tool list. Creator documents have an
 * additional family list, so their editor returns to that family first.
 */
export function studioDetailParent(page: StudioDetailPage): StudioDetailPage {
    if (page === null) return null;
    if (page === 'modules:activation') return 'modules:candidates';
    if (page === 'modules:activation:bindings') return 'modules:bindings';
    if (page === 'modules:deactivation' || page === 'modules:rollback') {
        return 'modules:bindings';
    }
    if (page.startsWith('modules:')) return 'modules';
    if (!page.includes('/')) return null;

    const [root = '', family] = page.split('/');
    if (!NESTED_STUDIO_ROOTS.has(root as StudioBaseDetailPage)) return null;

    if (root === 'documents' && family !== undefined) {
        const segmentCount = page.split('/').length;
        return segmentCount >= 3 ? `documents/${family}` : 'documents';
    }

    return root;
}

/** The localized shell-title key for a pushed editor that differs from its list. */
export function studioNestedDetailTitleKey(page: StudioDetailPage): MessageKey | null {
    if (page === null) return null;

    if (page.startsWith('modules:')) {
        const modulePageTitleKeys: Readonly<Record<string, MessageKey>> = {
            candidates: 'studio.page.modules.candidates',
            bindings: 'studio.page.modules.bindings',
            activation: 'studio.page.modules.activation',
            deactivation: 'studio.page.modules.deactivation',
            rollback: 'studio.page.modules.rollback',
        };
        const modulePage = page.split(':')[1] ?? '';
        return modulePageTitleKeys[modulePage] ?? 'studio.page.modules.detail';
    }

    if (!page.includes('/')) return null;

    const [root, second, third] = page.split('/');
    if (root === 'history' && second === 'review') return 'studio.page.history.review';
    if (root === 'blocks') return 'studio.page.blocks.edit';
    if (root === 'profiles' && second === 'create') return 'studio.page.profiles.create';
    if (root === 'profiles' && second === 'edit') return 'studio.page.profiles.edit';
    if (root === 'records' && second === 'edit') return 'studio.page.records.edit';
    if (root === 'interactions' && second === 'review') {
        return 'studio.page.interactions.review';
    }

    if (root === 'documents' && second !== undefined) {
        const familyKeyPrefix =
            CREATOR_DOCUMENT_TITLE_KEY_PREFIXES[second] ?? 'studio.page.documents.unknown';
        const suffix = third === 'create' || third === 'edit' ? `.${third}` : '';
        return `${familyKeyPrefix}${suffix}` as MessageKey;
    }

    if (root === 'modules') {
        return 'studio.page.modules.detail';
    }

    return null;
}

/** Whether a pushed Studio route renders a viewport-fixed bottom action bar. */
export function studioDetailHasFixedActions(page: StudioDetailPage): boolean {
    if (page === null) return false;
    if (page === 'variables' || page === 'display' || page === 'selection') return false;
    if (page === 'documents' || page === 'records' || page === 'interactions') return false;
    return true;
}
