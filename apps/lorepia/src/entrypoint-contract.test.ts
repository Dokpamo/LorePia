import { describe, expect, it } from 'vitest';
import liveEntrySource from './main.ts?raw';
import previewEntrySource from './preview/main.ts?raw';

describe('application entrypoint boundary', () => {
    it('boots the native application with the live client', () => {
        expect(liveEntrySource).toContain('const app = mount(App, { target });');
        expect(liveEntrySource).not.toContain('createPreviewClient');
        expect(liveEntrySource).not.toContain('DEMO_INITIAL_CHARACTER_ID');
    });

    it('keeps the preview client isolated to the preview entrypoint', () => {
        expect(previewEntrySource).toContain('client: createPreviewClient()');
        expect(previewEntrySource).toContain('characterId: DEMO_INITIAL_CHARACTER_ID');
        expect(previewEntrySource).not.toContain("import '../main'");
    });
});
