import { describe, expect, it, vi } from 'vitest';
import { get } from 'svelte/store';
import { MaterialLibraryController } from './material-library-controller';
import { createPreviewClient } from '../../preview/mock-client';

describe('material library independent of conversation context', () => {
    it('loads and saves revisioned material without loading a conversation workspace', async () => {
        const client = createPreviewClient();
        const room = vi.spyOn(client, 'getOrchestrationWorkspace');
        const save = vi.spyOn(client, 'upsertKnowledgeBook');
        const controller = new MaterialLibraryController(client);
        await controller.load();
        expect(get(controller.state).phase).toBe('ready');
        expect(room).not.toHaveBeenCalled();
        expect(controller.documents.addCreatorDocumentDraft('knowledge_book', 'new-lorebook')).toBe(
            true,
        );
        controller.documents.stageKnowledgeBook('new-lorebook', { name: 'Library stories' });
        expect(
            await controller.documents.saveCreatorDocument('knowledge_book', 'new-lorebook'),
        ).toBe(true);
        expect(save).toHaveBeenCalledWith(expect.objectContaining({ expected_revision: null }));
        expect(
            get(controller.state).editable_knowledge_books.find(
                (item) => item.value.id === 'new-lorebook',
            )?.expected_revision,
        ).toBe(1);
        controller.documents.stageKnowledgeBook('new-lorebook', {
            name: 'Revised library stories',
        });
        expect(
            await controller.documents.saveCreatorDocument('knowledge_book', 'new-lorebook'),
        ).toBe(true);
        expect(save).toHaveBeenLastCalledWith(expect.objectContaining({ expected_revision: 1 }));
        controller.destroy();
    });
    it('does not publish a delayed catalog after leaving the authoring flow', async () => {
        const client = createPreviewClient();
        const original = client.listKnowledgeBooks;
        if (!original) throw new Error('Fixture API missing');
        let release!: () => void;
        const pending = new Promise<void>((resolve) => (release = resolve));
        vi.spyOn(client, 'listKnowledgeBooks').mockImplementation(async () => {
            await pending;
            return original();
        });
        const controller = new MaterialLibraryController(client);
        const loading = controller.load();
        controller.destroy();
        release();
        await loading;
        expect(get(controller.state).phase).toBe('idle');
        expect(get(controller.state).editable_knowledge_books).toEqual([]);
    });
});
