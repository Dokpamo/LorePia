import { describe, expect, it } from 'vitest';
import { ko } from '../../lib/i18n/ko';
import {
    STUDIO_DETAIL_TITLE_KEYS,
    studioDetailHasFixedActions,
    studioDetailParent,
    studioNestedDetailTitleKey,
} from './studio-contracts';

describe('Studio pushed-screen shell', () => {
    it.each([
        ['history', 'studio.page.history'],
        ['blocks', 'studio.page.blocks'],
        ['room', 'studio.page.room'],
        ['variables', 'studio.page.variables'],
        ['profiles', 'studio.page.profiles'],
        ['documents', 'studio.page.documents'],
        ['records', 'studio.page.records'],
        ['knowledge', 'studio.page.knowledge'],
        ['transforms', 'studio.page.transforms.memory'],
        ['interactions', 'studio.page.interactions'],
        ['packages', 'studio.page.packages'],
        ['modules', 'studio.page.modules'],
        ['display', 'studio.page.display'],
        ['selection', 'studio.page.selection'],
        ['plan', 'studio.page.plan'],
    ] as const)('maps the %s leaf to one stable shell title key', (page, titleKey) => {
        expect(STUDIO_DETAIL_TITLE_KEYS[page]).toBe(titleKey);
        expect(ko[titleKey].length).toBeGreaterThan(0);
    });

    it.each([
        ['blocks/system', 'blocks'],
        ['profiles/create', 'profiles'],
        ['profiles/edit/main', 'profiles'],
        ['history/review/7', 'history'],
        ['documents/memory_profile', 'documents'],
        ['documents/memory_profile/create', 'documents/memory_profile'],
        ['documents/memory_profile/edit/main', 'documents/memory_profile'],
        ['records/edit/memory-1', 'records'],
        ['interactions/review/proposal-1', 'interactions'],
        ['modules:candidates', 'modules'],
        ['modules:activation', 'modules:candidates'],
        ['modules:activation:bindings', 'modules:bindings'],
        ['modules:deactivation', 'modules:bindings'],
        ['modules:rollback', 'modules:bindings'],
    ] as const)('pops %s by one visual level to %s', (page, parent) => {
        expect(studioDetailParent(page)).toBe(parent);
    });

    it.each([
        ['history/review/7', 'studio.page.history.review'],
        ['blocks/system', 'studio.page.blocks.edit'],
        ['profiles/create', 'studio.page.profiles.create'],
        ['profiles/edit/main', 'studio.page.profiles.edit'],
        ['documents/memory_profile', 'studio.page.documents.memory_profile'],
        ['documents/knowledge_book/create', 'studio.page.documents.knowledge_book.create'],
        ['documents/transform_set/edit/clean', 'studio.page.documents.transform_set.edit'],
        ['records/edit/memory-1', 'studio.page.records.edit'],
        ['interactions/review/proposal-1', 'studio.page.interactions.review'],
        ['modules:candidates', 'studio.page.modules.candidates'],
        ['modules:bindings', 'studio.page.modules.bindings'],
        ['modules:activation', 'studio.page.modules.activation'],
        ['modules:activation:bindings', 'studio.page.modules.activation'],
        ['modules:deactivation', 'studio.page.modules.deactivation'],
        ['modules:rollback', 'studio.page.modules.rollback'],
    ] as const)('gives %s the editor-specific title key', (page, titleKey) => {
        expect(studioNestedDetailTitleKey(page)).toBe(titleKey);
        expect(ko[titleKey].length).toBeGreaterThan(0);
    });

    it('reserves fixed-action space only for destinations with actions', () => {
        expect(studioDetailHasFixedActions('variables')).toBe(false);
        expect(studioDetailHasFixedActions('documents')).toBe(false);
        expect(studioDetailHasFixedActions('records')).toBe(false);
        expect(studioDetailHasFixedActions('blocks/system')).toBe(true);
    });
});
