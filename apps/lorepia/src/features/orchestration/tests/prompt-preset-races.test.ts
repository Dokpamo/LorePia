import { describe, expect, it, vi } from 'vitest';
import type { OrchestrationCapableClient } from '../controllers/orchestration-state';
import { PromptTaskController } from '../controllers/prompt-task-controller';
import { OrchestrationStateController } from '../controllers/orchestration-state-controller';
import type { CreatorPromptPresetDocumentDto, RevisionedDto } from '../../../lib/ipc/contracts';

function deferred<T>() {
    let resolve!: (value: T) => void;
    let reject!: (error: Error) => void;
    const promise = new Promise<T>((yes, no) => {
        resolve = yes;
        reject = no;
    });
    return { promise, resolve, reject };
}
type Document = RevisionedDto<CreatorPromptPresetDocumentDto>;
function document(id: string): Document {
    return { value: { id }, revision: 1 } as Document;
}
function fixture() {
    const state = new OrchestrationStateController();
    state.update((value) => ({ ...value, phase: 'ready', context_key: 'room:branch' }));
    const requests = [deferred<Document>(), deferred<Document>()] as const;
    const loader = vi
        .fn()
        .mockImplementationOnce(() => requests[0].promise)
        .mockImplementationOnce(() => requests[1].promise);
    const controller = new PromptTaskController(
        { getEditablePromptPreset: loader } as unknown as OrchestrationCapableClient,
        state,
    );
    return { state, requests, controller };
}

describe('prompt preset request ownership', () => {
    it.each(['response', 'error'] as const)(
        'ignores an old %s after a newer selection completes',
        async (kind) => {
            const { state, requests, controller } = fixture();
            const older = controller.loadEditablePromptPresetForContext('room:branch', 'a');
            const newer = controller.loadEditablePromptPresetForContext('room:branch', 'b');
            requests[1].resolve(document('b'));
            await newer;
            if (kind === 'response') requests[0].resolve(document('a'));
            else requests[0].reject(new Error('old failure'));
            await older;
            expect(state.snapshot().editable_prompt_preset?.value.id).toBe('b');
            expect(state.snapshot().editable_prompt_preset_error).toBeNull();
        },
    );

    it('invalidates an in-flight load when the selection is cleared', async () => {
        const { state, requests, controller } = fixture();
        const older = controller.loadEditablePromptPresetForContext('room:branch', 'a');
        await controller.loadEditablePromptPresetForContext('room:branch', null);
        requests[0].resolve(document('a'));
        await older;
        expect(state.snapshot().editable_prompt_preset).toBeNull();
    });

    it('does not restore a document after leaving and returning to the same room', async () => {
        const { state, requests, controller } = fixture();
        const older = controller.loadEditablePromptPresetForContext('room:branch', 'a');
        state.beginContextLoad();
        requests[0].resolve(document('a'));
        await older;
        expect(state.snapshot().editable_prompt_preset).toBeNull();
    });

    it('does not dispatch a save for a document that differs from the selected preset', async () => {
        const state = new OrchestrationStateController();
        state.update((value) => ({
            ...value,
            editable_prompt_preset: document('a'),
            editable_prompt_preset_dirty: true,
            workspace: {
                ...value.workspace,
                room_config: { ...value.workspace.room_config, prompt_preset_id: 'b' },
            },
        }));
        const save = vi.fn();
        const controller = new PromptTaskController(
            { upsertPromptPreset: save } as unknown as OrchestrationCapableClient,
            state,
        );
        expect(await controller.saveEditablePromptPreset()).toBe(false);
        expect(save).not.toHaveBeenCalled();
    });

    it.each(['response', 'error'] as const)(
        'ignores a saved preset reload %s after selecting another preset',
        async (kind) => {
            const state = new OrchestrationStateController();
            state.update((value) => ({
                ...value,
                context_key: 'room:branch',
                editable_prompt_preset: document('a'),
                editable_prompt_preset_dirty: true,
                workspace: {
                    ...value.workspace,
                    room_config: { ...value.workspace.room_config, prompt_preset_id: 'a' },
                },
            }));
            const savedReload = deferred<Document>();
            const reload = vi
                .fn()
                .mockReturnValueOnce(savedReload.promise)
                .mockResolvedValueOnce(document('b'));
            const controller = new PromptTaskController(
                {
                    upsertPromptPreset: vi
                        .fn()
                        .mockResolvedValue({ value: { id: 'a', name: 'A' } }),
                    getEditablePromptPreset: reload,
                } as unknown as OrchestrationCapableClient,
                state,
            );
            const saving = controller.saveEditablePromptPreset();
            await vi.waitFor(() => expect(reload).toHaveBeenCalledOnce());
            await controller.loadEditablePromptPresetForContext('room:branch', 'b');
            if (kind === 'response') savedReload.resolve(document('a'));
            else savedReload.reject(new Error('old save reload'));
            expect(await saving).toBe(false);
            expect(state.snapshot().editable_prompt_preset?.value.id).toBe('b');
            expect(state.snapshot().editable_prompt_preset_error).toBeNull();
        },
    );
});
