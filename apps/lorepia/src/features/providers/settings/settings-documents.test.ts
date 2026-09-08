import { get } from 'svelte/store';
import { describe, expect, it, vi } from 'vitest';
import { createPreviewClient } from '../../../preview/mock-client';
import type { CreatorPromptPresetDocumentDto, RevisionedDto } from '../../../lib/ipc/contracts';
import { SettingsDocumentsController } from './settings-documents';
import { newMemory, newPrompt, summaryGuidance, summaryTemplate } from './settings-defaults';

function deferred<T>() {
    let resolve!: (value: T) => void;
    const promise = new Promise<T>((done) => {
        resolve = done;
    });
    return { promise, resolve };
}
describe('global settings documents', () => {
    it('persists a prompt without a conversation and reports revision conflicts without success', async () => {
        const client = createPreviewClient();
        const controller = new SettingsDocumentsController(client);
        await controller.load();
        const prompt = newPrompt();
        prompt.name = 'My prompt';
        const saved = await controller.savePrompt(prompt, null);
        expect(saved?.revision).toBe(1);
        expect((await controller.openPrompt(prompt.id))?.value).toEqual(prompt);
        const failed = await controller.savePrompt({ ...prompt, name: 'Stale write' }, null);
        expect(failed).toBeNull();
        expect(get(controller.state)).toMatchObject({ busy: false, announcement: '' });
        expect(get(controller.state).error).toBeTruthy();
        expect(
            get(controller.state).prompts.find((item) => item.value.id === prompt.id)?.value.name,
        ).toBe('My prompt');
        controller.destroy();
    });
    it('serializes writes and ignores results after disposal', async () => {
        const pending = deferred<RevisionedDto<ReturnType<typeof newMemory>>>();
        const save = vi.fn(() => pending.promise);
        const controller = new SettingsDocumentsController({
            ...createPreviewClient(),
            upsertMemoryProfile: save,
        });
        await controller.load();
        const document = newMemory();
        const first = controller.saveMemory(document, null);
        expect(await controller.saveMemory(document, null)).toBeNull();
        expect(save).toHaveBeenCalledTimes(1);
        controller.destroy();
        pending.resolve({
            value: document,
            revision: 1,
            created_at: '',
            updated_at: '',
            deleted_at: null,
        });
        expect(await first).toBeNull();
        expect(get(controller.state).memories.some((item) => item.value.id === document.id)).toBe(
            false,
        );
    });
    it('drops a stale prompt selection when another selection wins', async () => {
        const a = deferred<RevisionedDto<CreatorPromptPresetDocumentDto>>();
        const b = deferred<RevisionedDto<CreatorPromptPresetDocumentDto>>();
        const controller = new SettingsDocumentsController({
            getEditablePromptPreset: ({ prompt_preset_id }) =>
                prompt_preset_id === 'a' ? a.promise : b.promise,
        });
        const first = controller.openPrompt('a');
        const second = controller.openPrompt('b');
        const document = newPrompt();
        b.resolve({
            value: { ...document, id: 'b' },
            revision: 2,
            created_at: '',
            updated_at: '',
            deleted_at: null,
        });
        expect((await second)?.value.id).toBe('b');
        a.resolve({
            value: { ...document, id: 'a' },
            revision: 1,
            created_at: '',
            updated_at: '',
            deleted_at: null,
        });
        expect(await first).toBeNull();
        controller.destroy();
    });
    it('updates and removes the exact saved memory revision', async () => {
        const controller = new SettingsDocumentsController(createPreviewClient());
        await controller.load();
        const document = newMemory();
        document.name = 'Memory';
        const saved = await controller.saveMemory(document, null);
        if (!saved) throw new Error('Expected saved memory');
        expect(
            (await controller.saveMemory({ ...saved.value, name: 'Updated' }, saved.revision))
                ?.revision,
        ).toBe(2);
        expect(await controller.deleteMemory(saved)).toBeNull();
        const current = get(controller.state).memories.find(
            (item) => item.value.id === document.id,
        );
        if (!current) throw new Error('Expected current memory');
        expect(await controller.deleteMemory(current)).not.toBeNull();
        expect(
            get(controller.state).memories.find((item) => item.value.id === document.id),
        ).toBeUndefined();
        controller.destroy();
    });
    it('preserves exactly one conversation slot and leaves imported templates intact', () => {
        const template = summaryTemplate('Summarize events and promises.');
        expect(template?.parts.filter((part) => part.kind === 'slot')).toEqual([
            { kind: 'slot', name: 'memory_source' },
        ]);
        expect(summaryGuidance(template)).toBe('Summarize events and promises.');
        expect(summaryTemplate('  ')).toBeNull();
        const imported = {
            parts: [
                { kind: 'slot' as const, name: 'memory_source' },
                { kind: 'text' as const, value: 'Custom order' },
            ],
            max_output_chars: 10000,
        };
        const snapshot = structuredClone(imported);
        expect(summaryGuidance(imported)).toBeNull();
        expect(imported).toEqual(snapshot);
    });
    it('keeps conversation roles and the latest user turn in newly created presets', () => {
        const blocks = newPrompt().blocks;
        expect(blocks.find((block) => block.kind === 'history_slice')).toMatchObject({
            role_hint: 'provider_default',
            overflow_policy: 'keep_latest_items',
        });
        expect(blocks.find((block) => block.kind === 'latest_user_turn')).toMatchObject({
            role_hint: 'user',
            overflow_policy: 'reject',
            token_policy: { priority: 65535, min_tokens: 1 },
        });
    });
});
