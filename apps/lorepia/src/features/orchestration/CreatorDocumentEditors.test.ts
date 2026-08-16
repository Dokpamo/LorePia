import { cleanup, fireEvent, render, screen, waitFor, within } from '@testing-library/svelte';
import { afterEach, describe, expect, it, vi } from 'vitest';

import type {
    CreatorContentModuleDocumentDto,
    CreatorMemoryProfileDocumentDto,
} from '../../lib/ipc/contracts';
import CreatorDocumentEditors from './CreatorDocumentEditors.svelte';
import {
    INITIAL_ORCHESTRATION_STATE,
    type OrchestrationController,
    type OrchestrationState,
} from './orchestration-controller';

afterEach(cleanup);

const MEMORY_PROFILE: CreatorMemoryProfileDocumentDto = {
    id: 'memory-default',
    name: 'Default memory',
    summary_task: 'summary-task',
    embedding_task: null,
    turns_per_summary: 8,
    recent_raw_budget: { max_tokens: 2_048 },
    episodic_budget: { max_tokens: 1_024 },
    semantic_budget: { max_tokens: 1_024 },
    retrieval_count: 8,
    recency_weight: 1,
    similarity_weight: 1,
    importance_weight: 1,
    preserve_invalidated_records: false,
    summary_schema: 'summary-v1',
};

const CONTENT_MODULE: CreatorContentModuleDocumentDto = {
    id: 'module-default',
    name: 'Default module',
    version: '0.1.0',
    prompt_fragments: [],
    knowledge_book_ids: [],
    control_specs: [],
    transform_set_ids: [],
    interaction_rule_set_ids: [],
    asset_ids: [],
    required_capabilities: [],
    metadata: {
        author: null,
        license: 'proprietary',
        redistribution_allowed: false,
        homepage: null,
        description: '',
        tags: [],
    },
};

function readyState(): OrchestrationState {
    const state = structuredClone(INITIAL_ORCHESTRATION_STATE);
    state.phase = 'ready';
    state.context_key = 'conversation-1:branch-1';
    state.editable_memory_profiles = [
        {
            value: MEMORY_PROFILE,
            expected_revision: 3,
            dirty: false,
        },
    ];
    state.editable_content_modules = [
        {
            value: CONTENT_MODULE,
            expected_revision: 2,
            dirty: false,
        },
    ];
    return state;
}

function controllerFixture() {
    const addCreatorDocumentDraft = vi.fn(() => true);
    const replaceCreatorDocument = vi.fn(() => true);
    const saveCreatorDocument = vi.fn(() => Promise.resolve(true));
    const deleteCreatorDocument = vi.fn(() => Promise.resolve(true));
    const controller = {
        addCreatorDocumentDraft,
        replaceCreatorDocument,
        saveCreatorDocument,
        deleteCreatorDocument,
    } as unknown as OrchestrationController;
    return {
        controller,
        addCreatorDocumentDraft,
        replaceCreatorDocument,
        saveCreatorDocument,
        deleteCreatorDocument,
    };
}

function closestDetails(element: HTMLElement): HTMLElement {
    const details = element.closest('details');
    if (!(details instanceof HTMLElement)) {
        throw new Error('expected a details ancestor');
    }
    return details;
}

function deferred<Value>(): {
    promise: Promise<Value>;
    resolve: (value: Value) => void;
} {
    let resolvePromise!: (value: Value) => void;
    const promise = new Promise<Value>((resolve) => {
        resolvePromise = resolve;
    });
    return { promise, resolve: resolvePromise };
}

describe('CreatorDocumentEditors', () => {
    it('creates, edits, validates through the controller, and confirms deletion', async () => {
        const fixture = controllerFixture();
        render(CreatorDocumentEditors, {
            orchestrationState: readyState(),
            controller: fixture.controller,
        });

        const knowledgeFamily = closestDetails(screen.getByText('지식 책'));
        await fireEvent.click(within(knowledgeFamily).getByText('지식 책'));
        await fireEvent.input(within(knowledgeFamily).getByLabelText('새 지식 책 ID'), {
            target: { value: 'knowledge-new' },
        });
        await fireEvent.click(within(knowledgeFamily).getByRole('button', { name: '새 문서' }));
        expect(fixture.addCreatorDocumentDraft).toHaveBeenCalledWith(
            'knowledge_book',
            'knowledge-new',
        );

        await fireEvent.click(screen.getByText('memory-default'));
        const memoryEditor = closestDetails(screen.getByText('memory-default'));
        const editedMemory = { ...MEMORY_PROFILE, name: 'Edited memory' };
        await fireEvent.input(within(memoryEditor).getByLabelText('안전 문서 JSON'), {
            target: { value: JSON.stringify(editedMemory) },
        });
        await fireEvent.click(
            within(memoryEditor).getByRole('button', { name: 'Core 검증 후 저장' }),
        );
        await waitFor(() =>
            expect(fixture.replaceCreatorDocument).toHaveBeenCalledWith(
                'memory_profile',
                MEMORY_PROFILE.id,
                editedMemory,
            ),
        );
        expect(fixture.saveCreatorDocument).toHaveBeenCalledWith(
            'memory_profile',
            MEMORY_PROFILE.id,
        );

        await fireEvent.click(within(memoryEditor).getByRole('button', { name: '삭제' }));
        await fireEvent.click(within(memoryEditor).getByRole('button', { name: '삭제 확인' }));
        await waitFor(() =>
            expect(fixture.deleteCreatorDocument).toHaveBeenCalledWith(
                'memory_profile',
                MEMORY_PROFILE.id,
            ),
        );
    });

    it('fails closed before save when a content module tries to cross asset identifiers', async () => {
        const fixture = controllerFixture();
        render(CreatorDocumentEditors, {
            orchestrationState: readyState(),
            controller: fixture.controller,
        });

        await fireEvent.click(screen.getByText('module-default'));
        const moduleEditor = closestDetails(screen.getByText('module-default'));
        await fireEvent.input(within(moduleEditor).getByLabelText('안전 문서 JSON'), {
            target: {
                value: JSON.stringify({
                    ...CONTENT_MODULE,
                    asset_ids: ['asset-not-resolved'],
                }),
            },
        });
        await fireEvent.click(
            within(moduleEditor).getByRole('button', { name: 'Core 검증 후 저장' }),
        );

        expect(
            await within(moduleEditor).findByText(
                '현재 안전 CRUD 경로에서는 asset_ids가 빈 배열이어야 합니다.',
            ),
        ).toBeVisible();
        expect(fixture.replaceCreatorDocument).not.toHaveBeenCalled();
        expect(fixture.saveCreatorDocument).not.toHaveBeenCalled();
    });

    it('keeps JSON typed after a save started as a newer unsaved draft', async () => {
        const fixture = controllerFixture();
        const pendingSave = deferred<boolean>();
        fixture.saveCreatorDocument.mockImplementationOnce(() => pendingSave.promise);
        render(CreatorDocumentEditors, {
            orchestrationState: readyState(),
            controller: fixture.controller,
        });

        await fireEvent.click(screen.getByText('memory-default'));
        const memoryEditor = closestDetails(screen.getByText('memory-default'));
        const editor = within(memoryEditor).getByLabelText('안전 문서 JSON');
        const submitted = { ...MEMORY_PROFILE, name: 'Submitted memory' };
        const newerDraft = { ...submitted, summary_schema: 'newer-unsaved-schema' };
        await fireEvent.input(editor, {
            target: { value: JSON.stringify(submitted) },
        });
        await fireEvent.click(
            within(memoryEditor).getByRole('button', { name: 'Core 검증 후 저장' }),
        );
        await waitFor(() =>
            expect(fixture.saveCreatorDocument).toHaveBeenCalledWith(
                'memory_profile',
                MEMORY_PROFILE.id,
            ),
        );

        await fireEvent.input(editor, {
            target: { value: JSON.stringify(newerDraft) },
        });
        pendingSave.resolve(true);

        await waitFor(() => expect(editor).toHaveValue(JSON.stringify(newerDraft)));
        expect(fixture.replaceCreatorDocument).toHaveBeenCalledOnce();
        expect(fixture.replaceCreatorDocument).toHaveBeenCalledWith(
            'memory_profile',
            MEMORY_PROFILE.id,
            submitted,
        );
    });
});
