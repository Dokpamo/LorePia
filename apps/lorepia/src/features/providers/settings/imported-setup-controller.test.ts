import { get } from 'svelte/store';
import { describe, expect, it, vi } from 'vitest';
import { createPreviewClient } from '../../../preview/mock-client';
import type { OrchestrationDocumentClientApi } from '../../../lib/ipc/contracts';
import { appState, orchestrationState } from '../../orchestration/tests/fixtures';
import type { SettingsServices } from './settings-services';
import { newPrompt, newTask } from './settings-defaults';
import { ImportedSetupController, type ImportedSetupSelection } from './imported-setup-controller';

function source(kind: 'generation' | 'memory') {
    const prompt = newPrompt();
    prompt.id = `imported-${kind}`;
    prompt.name = `Imported ${kind}`;
    const hints = { import_kind: kind, summary_task_id: 'summary', memory_profile_id: 'memory' };
    prompt.default_values.values = Object.entries(hints).map(([id, value]) => ({
        variable: { scope: 'app', namespace: null, id: `lorepia_imported_${id}` },
        value: { type: 'text', value },
    }));
    if (kind === 'memory') {
        const block = prompt.blocks[0];
        if (!block) throw new Error('Expected summary block');
        block.id = 'imported-memory-summary-test';
        block.template = {
            parts: [{ kind: 'text', value: 'Summarize.\n{{slot}}' }],
            max_output_chars: 32000,
        };
    }
    return prompt;
}

async function fixture() {
    const client = createPreviewClient() as ReturnType<typeof createPreviewClient> &
        OrchestrationDocumentClientApi;
    for (const kind of ['generation', 'memory'] as const)
        await client.upsertPromptPreset({ value: source(kind), expected_revision: null });
    const writeGeneration = vi.fn().mockResolvedValue(true);
    const loadContext = vi.fn().mockResolvedValue(undefined);
    const stageRoomConfig = vi.fn();
    const saveRoomConfig = vi.fn().mockResolvedValue(true);
    const services = {
        client,
        appState: appState(),
        appController: { upsertProviderGenerationPreset: writeGeneration },
        orchestrationState: orchestrationState(),
        orchestrationController: { loadContext, stageRoomConfig, saveRoomConfig },
    } as unknown as SettingsServices;
    const controller = new ImportedSetupController(() => services);
    const selection: ImportedSetupSelection = {
        kind: 'generation',
        sourceId: 'imported-generation',
        routeId: 'route-1',
        targetPromptId: 'imported-generation',
        embeddingTaskId: '',
        applyToRoom: true,
    };
    return {
        controller,
        services,
        client,
        selection,
        writeGeneration,
        loadContext,
        stageRoomConfig,
        saveRoomConfig,
    };
}

describe('imported settings connection', () => {
    it.each([
        ['\ud55c\uae00', '\ub2e4\ub978'],
        ['preset::one', 'preset/?one'],
        ['a'.repeat(210) + 'first', 'a'.repeat(210) + 'second'],
    ])('keeps distinct imported sources separate: %s', async (firstId, secondId) => {
        const f = await fixture();
        const savedIds: string[] = [];
        for (const sourceId of [firstId, secondId]) {
            await f.client.upsertPromptPreset({
                value: { ...source('generation'), id: sourceId },
                expected_revision: null,
            });
            expect(await f.controller.save({ ...f.selection, sourceId, applyToRoom: false })).toBe(
                true,
            );
            const saved = await f.client.getEditablePromptPreset({ prompt_preset_id: sourceId });
            const generationId = saved.value.default_generation_preset_id;
            expect(generationId).toMatch(/^imported-[a-f0-9]{64}-provider$/);
            if (!generationId) throw new Error('Expected generated preset id');
            savedIds.push(generationId);
        }
        expect(new Set(savedIds).size).toBe(2);
        expect(
            await f.controller.save({ ...f.selection, sourceId: firstId, applyToRoom: false }),
        ).toBe(true);
        expect(
            (await f.client.getEditablePromptPreset({ prompt_preset_id: firstId })).value
                .default_generation_preset_id,
        ).toBe(savedIds[0]);
        expect(
            (await f.client.getEditablePromptPreset({ prompt_preset_id: secondId })).value
                .default_generation_preset_id,
        ).toBe(savedIds[1]);
    });

    it('loads imported documents and connects their generation preset using fresh revisions', async () => {
        const f = await fixture();
        await f.controller.load(await f.client.listPromptPresets());
        expect(get(f.controller.state).sources.map((item) => item.value.id)).toContain(
            'imported-generation',
        );
        expect(await f.controller.save(f.selection)).toBe(true);
        const saved = await f.client.getEditablePromptPreset({
            prompt_preset_id: f.selection.sourceId,
        });
        expect(saved.value.default_generation_preset_id).toBe(
            'imported-ad7fa24f8ece8f67ba15b4e4bf49f8b4153efb4bbf44fd1d080e7723c3b3622a-provider',
        );
        expect(f.stageRoomConfig).toHaveBeenCalledWith({
            prompt_preset_id: saved.value.id,
            generation_preset_id:
                'imported-ad7fa24f8ece8f67ba15b4e4bf49f8b4153efb4bbf44fd1d080e7723c3b3622a-provider',
        });
        expect(f.saveRoomConfig).toHaveBeenCalledOnce();
        expect(await f.controller.save(f.selection)).toBe(true);
        expect(
            (await f.client.getEditablePromptPreset({ prompt_preset_id: saved.value.id })).revision,
        ).toBe(saved.revision + 1);
    });

    it('creates the summary task and memory, then binds the selected prompt and embedding task', async () => {
        const f = await fixture();
        const embedding = {
            ...newTask('memory_embedding'),
            id: 'embedding',
            route_id: 'route-1',
            generation_preset_id: 'generation-1',
        };
        await f.client.upsertTaskProfile({ value: embedding, expected_revision: null });
        expect(
            await f.controller.save({
                ...f.selection,
                kind: 'memory',
                sourceId: 'imported-memory',
                embeddingTaskId: 'embedding',
            }),
        ).toBe(true);
        const memory = (await f.client.listMemoryProfiles()).find(
            (item) => item.value.id === 'memory',
        );
        if (!memory) throw new Error('Expected memory');
        expect(memory.value).toMatchObject({
            summary_task: 'summary',
            embedding_task: 'embedding',
            similarity_weight: 0.4,
        });
        expect(memory.value.summary_template?.parts).toContainEqual({
            kind: 'slot',
            name: 'memory_source',
        });
        expect(
            (await f.client.listTaskProfiles()).find((item) => item.value.id === 'summary')?.value,
        ).toMatchObject({
            kind: 'memory_summary',
            route_id: 'route-1',
            generation_preset_id:
                'imported-b5e97230126eb13d9a69ffd167e1c10c73e1928afbb3fa1583d32430f28391b6-summary-provider',
        });
        expect(
            (await f.client.getEditablePromptPreset({ prompt_preset_id: 'imported-generation' }))
                .value.memory_profile_id,
        ).toBe('memory');
        expect(f.stageRoomConfig).toHaveBeenCalledWith({
            prompt_preset_id: 'imported-generation',
            memory_enabled: true,
        });
        expect(
            await f.controller.save({
                ...f.selection,
                kind: 'memory',
                sourceId: 'imported-memory',
                embeddingTaskId: 'embedding',
            }),
        ).toBe(true);
        expect(
            (await f.client.listMemoryProfiles()).find((item) => item.value.id === 'memory')
                ?.revision,
        ).toBe(memory.revision + 1);
    });

    it('does not bind a different conversation when navigation wins during a save', async () => {
        const f = await fixture();
        f.writeGeneration.mockImplementationOnce(() => {
            const room = f.services.appState.conversation_state;
            if (!room) throw new Error('Expected active room');
            room.active_branch_id = 'other-branch';
            return Promise.resolve(true);
        });
        expect(await f.controller.save(f.selection)).toBe(false);
        expect(f.loadContext).not.toHaveBeenCalled();
        expect(f.stageRoomConfig).not.toHaveBeenCalled();
        expect(get(f.controller.state).error).toBeTruthy();
        expect(
            (await f.client.getEditablePromptPreset({ prompt_preset_id: 'imported-generation' }))
                .value.default_generation_preset_id,
        ).toBe(
            'imported-ad7fa24f8ece8f67ba15b4e4bf49f8b4153efb4bbf44fd1d080e7723c3b3622a-provider',
        );
    });

    it('stops dependent writes after a failed task save and permits a safe retry', async () => {
        const f = await fixture();
        const task = vi
            .spyOn(f.client, 'upsertTaskProfile')
            .mockRejectedValueOnce(new Error('write failed'));
        const memory = vi.spyOn(f.client, 'upsertMemoryProfile');
        const input = { ...f.selection, kind: 'memory' as const, sourceId: 'imported-memory' };
        expect(await f.controller.save(input)).toBe(false);
        expect(memory).not.toHaveBeenCalled();
        expect(f.stageRoomConfig).not.toHaveBeenCalled();
        expect(get(f.controller.state).saving).toBe(false);
        expect(await f.controller.save(input)).toBe(true);
        expect(task).toHaveBeenCalledTimes(2);
        expect(memory).toHaveBeenCalledOnce();
    });

    it('validates an embedding selection before any writes', async () => {
        const f = await fixture();
        expect(
            await f.controller.save({
                ...f.selection,
                kind: 'memory',
                sourceId: 'imported-memory',
                embeddingTaskId: 'missing',
            }),
        ).toBe(false);
        expect(f.writeGeneration).not.toHaveBeenCalled();
        expect(get(f.controller.state).error).toBeTruthy();
    });

    it('never writes after disposal while loading the source', async () => {
        const f = await fixture();
        const original = f.client.getEditablePromptPreset.bind(f.client);
        vi.spyOn(f.client, 'getEditablePromptPreset').mockImplementationOnce(async (input) => {
            f.controller.destroy();
            return original(input);
        });
        expect(await f.controller.save(f.selection)).toBe(false);
        expect(f.writeGeneration).not.toHaveBeenCalled();
    });
});
