import { describe, expect, it } from 'vitest';

import appSource from '../../app/App.svelte?raw';
import detailActionBarSource from '../../components/detail/DetailActionBar.svelte?raw';
import { ko } from '../../lib/i18n/ko';
import appCss from '../../styles/app.css?raw';
import contentModuleSource from './ContentModuleLifecyclePanel.svelte?raw';
import creatorDocumentsSource from './CreatorDocumentEditors.svelte?raw';
import orchestrationStudioSource from './OrchestrationStudio.svelte?raw';
import promptHistorySource from './PromptPresetHistory.svelte?raw';
import taskProfilesSource from './TaskProfilesPanel.svelte?raw';
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

    it('wires the route resolver into the App back button and title', () => {
        expect(appSource).toContain('studioDetailPage = studioDetailParent(studioDetailPage)');
        expect(appSource).toContain('studioNestedDetailTitleKey(studioDetailPage)');
        expect(appSource).toContain('studioBaseDetailTitleKey(studioDetailPage)');
        expect(appSource).toContain('class:studio-detail-scroll={studioSection !== null}');
        expect(appSource).toContain(
            'class:studio-detail-has-actions={studioDetailHasFixedActions(',
        );
        expect(appSource).toMatch(
            /!isDesktop\s+&&\s+studioSection === null[\s\S]*?<nav class="tab-bar"/,
        );
    });

    it('centres fixed actions in the active workspace and reserves their scroll space', () => {
        expect(detailActionBarSource).toMatch(
            /\.detail-action-bar\.fixed\s*\{[^}]*position:\s*fixed;[^}]*left:\s*var\(--detail-action-center,\s*50%\);[^}]*var\(--detail-action-workspace-width,\s*100vw\)/s,
        );
        expect(appCss).toContain('--detail-action-workspace-width: min(100vw, 591px)');
        expect(appCss).toContain('--detail-action-workspace-width: calc(100vw - var(--sidebar))');
        expect(appCss).toMatch(
            /\.view-scroll\.studio-detail-scroll\s*\{[^}]*padding-bottom:\s*24px;/s,
        );
        expect(appCss).toMatch(
            /\.studio-detail-has-actions[^}]*padding-bottom:\s*calc\(var\(--mobile-nav\) \+ 28px \+ env\(safe-area-inset-bottom\)\);/s,
        );
        expect(studioDetailHasFixedActions('variables')).toBe(false);
        expect(studioDetailHasFixedActions('documents')).toBe(false);
        expect(studioDetailHasFixedActions('records')).toBe(false);
        expect(studioDetailHasFixedActions('blocks/system')).toBe(true);
        expect(appCss).toMatch(
            /\.view-scroll\.studio-detail-scroll[\s\S]*?:is\(\.data-table-wrap, \.block-minimap ol, \.safe-text-preview pre, \.diff-preview pre\)[\s\S]*?touch-action:\s*pan-x pan-y;/s,
        );
    });

    it('keeps App as the single Studio scroll and fixed-action reserve owner', () => {
        for (const source of [creatorDocumentsSource, contentModuleSource]) {
            expect(source).not.toMatch(/import DetailPage|<DetailPage\b/);
        }
        for (const source of [
            promptHistorySource,
            taskProfilesSource,
            creatorDocumentsSource,
            contentModuleSource,
        ]) {
            expect(source).not.toContain('calc(var(--mobile-nav)');
        }
        expect(promptHistorySource).not.toMatch(/max-height:\s*14rem;[\s\S]*?overflow:\s*auto;/);
        expect(orchestrationStudioSource).toMatch(
            /\.safe-text-preview pre,[\s\S]*?\.diff-preview pre\s*\{[^}]*max-height:\s*none;[^}]*overflow-x:\s*auto;[^}]*overflow-y:\s*visible;/s,
        );
    });

    it('fixes every Studio action bar to the shared workspace centre', () => {
        for (const source of [
            promptHistorySource,
            taskProfilesSource,
            creatorDocumentsSource,
            contentModuleSource,
            orchestrationStudioSource,
        ]) {
            const actionBars = source.match(/<DetailActionBar\b[^>]*>/g) ?? [];
            expect(actionBars.length).toBeGreaterThan(0);
            expect(actionBars.every((tag) => /\bfixed\b/.test(tag))).toBe(true);
        }
    });
});
