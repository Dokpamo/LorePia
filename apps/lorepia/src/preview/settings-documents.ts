import type {
    CreatorPromptPresetDocumentDto,
    OrchestrationDocumentClientApi,
    RevisionedDto,
} from '../lib/ipc/contracts';
import {
    DEMO_EDITABLE_PROMPT_PRESET,
    DEMO_MEMORY_PROFILE_DOCUMENTS,
    DEMO_TASK_PROFILE_DOCUMENTS,
} from './demo-data';

function documents<T extends { id: string }>(initial: RevisionedDto<T>[]) {
    const items = new Map(initial.map((item) => [item.value.id, structuredClone(item)]));
    const read = (id: string) => {
        const item = items.get(id);
        if (!item) throw new Error('Demo document not found.');
        return structuredClone(item);
    };
    return {
        list: () => Promise.resolve(structuredClone([...items.values()])),
        read,
        save: (value: T, expected: number | null) => {
            if ((items.get(value.id)?.revision ?? null) !== expected)
                throw new Error('Demo document revision conflict.');
            const result: RevisionedDto<T> = {
                value: structuredClone(value),
                revision: (expected ?? 0) + 1,
                created_at: items.get(value.id)?.created_at ?? '2026-08-24T13:00:00.000Z',
                updated_at: '2026-08-24T13:00:00.000Z',
                deleted_at: null,
            };
            items.set(value.id, result);
            return structuredClone(result);
        },
        remove: (id: string, expected: number) => {
            const item = read(id);
            if (item.revision !== expected) throw new Error('Demo document revision conflict.');
            items.delete(id);
            return { ...item, revision: expected + 1 };
        },
    };
}
function summary(item: RevisionedDto<CreatorPromptPresetDocumentDto>) {
    const { id, name, schema_version, default_generation_preset_id } = item.value;
    return {
        ...item,
        value: {
            id,
            name,
            schema_version,
            default_generation_preset_id,
            block_count: item.value.blocks.length,
        },
    };
}
/** Browser fixtures only; no persistence or native/network access. */
export function createDemoSettingsDocuments(): Partial<OrchestrationDocumentClientApi> {
    const prompts = documents([DEMO_EDITABLE_PROMPT_PRESET]);
    const memories = documents(DEMO_MEMORY_PROFILE_DOCUMENTS);
    const tasks = documents(DEMO_TASK_PROFILE_DOCUMENTS);
    return {
        listPromptPresets: async () => (await prompts.list()).map(summary),
        getPromptPreset: ({ prompt_preset_id }) =>
            response(() => summary(prompts.read(prompt_preset_id))),
        getEditablePromptPreset: ({ prompt_preset_id }) =>
            response(() => prompts.read(prompt_preset_id)),
        upsertPromptPreset: ({ value, expected_revision }) =>
            response(() => summary(prompts.save(value, expected_revision))),
        deletePromptPreset: ({ prompt_preset_id, expected_revision }) =>
            response(() => summary(prompts.remove(prompt_preset_id, expected_revision))),
        listMemoryProfiles: memories.list,
        getMemoryProfile: ({ memory_profile_id }) =>
            response(() => memories.read(memory_profile_id)),
        upsertMemoryProfile: ({ value, expected_revision }) =>
            response(() => memories.save(value, expected_revision)),
        deleteMemoryProfile: ({ memory_profile_id, expected_revision }) =>
            response(() => memories.remove(memory_profile_id, expected_revision)),
        listTaskProfiles: tasks.list,
        upsertTaskProfile: ({ value, expected_revision }) =>
            response(() => tasks.save(value, expected_revision)),
        deleteTaskProfile: ({ task_profile_id, expected_revision }) =>
            response(() => tasks.remove(task_profile_id, expected_revision)),
    };
}
function response<T>(operation: () => T): Promise<T> {
    return Promise.resolve().then(operation);
}
