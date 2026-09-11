import { describe, expect, it } from 'vitest';
import liveEntrySource from './main.ts?raw';
import previewEntrySource from './preview/main.ts?raw';
import previewHtml from '../preview.html?raw';
import uiPreviewHtml from '../ui-preview.html?raw';

describe('application entrypoint boundary', () => {
    it('boots the native application with the live client', () => {
        expect(liveEntrySource).toContain('const app = mount(App, { target });');
        expect(liveEntrySource).not.toContain('createPreviewClient');
        expect(liveEntrySource).not.toContain('DEMO_INITIAL_CHARACTER_ID');
    });

    it('keeps the preview client isolated to the preview entrypoint', () => {
        expect(previewEntrySource).toContain('client: createPreviewClient()');
        expect(previewEntrySource).toContain("'../app/workspace/WorkspaceApp.svelte'");
        expect(previewEntrySource).toContain(
            'characterPresentations: createPreviewCharacterPresentations()',
        );
        expect(previewEntrySource).not.toContain("import '../main'");
        expect(previewEntrySource).not.toContain("'../app/App.svelte'");
        for (const html of [previewHtml, uiPreviewHtml])
            expect(html).toContain('src="/src/preview/main.ts"');
    });
});
