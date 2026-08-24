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

        await fireEvent.click(screen.getByRole('button', { name: /^지식 책/ }));
        const knowledgeListActions = screen.getByRole('toolbar', { name: '지식 책 작업' });
        expect(knowledgeListActions).toHaveClass('fixed');
        await fireEvent.click(
            within(knowledgeListActions).getByRole('button', { name: '문서 추가하기' }),
        );

        await fireEvent.input(screen.getByLabelText('새 지식 책 ID'), {
            target: { value: 'knowledge-new' },
        });
        const knowledgeCreateActions = screen.getByRole('toolbar', {
            name: '지식 책 만들기 작업',
        });
        expect(knowledgeCreateActions).toHaveClass('fixed');
        await fireEvent.click(
            within(knowledgeCreateActions).getByRole('button', { name: '문서 만들기' }),
        );
        expect(fixture.addCreatorDocumentDraft).toHaveBeenCalledWith(
            'knowledge_book',
            'knowledge-new',
        );

        cleanup();
        render(CreatorDocumentEditors, {
            orchestrationState: readyState(),
            controller: fixture.controller,
        });

        await fireEvent.click(screen.getByRole('button', { name: /^메모리 프로필/ }));
        await fireEvent.click(screen.getByRole('button', { name: /^memory-default/ }));
        const memoryEditor = screen.getByRole('form', { name: '메모리 프로필 JSON 편집' });
        const editedMemory = { ...MEMORY_PROFILE, name: 'Edited memory' };
        await fireEvent.input(
            within(memoryEditor).getByRole('textbox', { name: /^안전 문서 JSON/ }),
            {
                target: { value: JSON.stringify(editedMemory) },
            },
        );
        const memoryEditActions = screen.getByRole('toolbar', {
            name: '메모리 프로필 편집 작업',
        });
        expect(memoryEditActions).toHaveClass('fixed');
        await fireEvent.click(within(memoryEditActions).getByRole('button', { name: '저장' }));
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

        await fireEvent.click(screen.getByRole('button', { name: /^memory-default/ }));
        const memoryDeleteActions = screen.getByRole('toolbar', {
            name: '메모리 프로필 편집 작업',
        });
        await fireEvent.click(within(memoryDeleteActions).getByRole('button', { name: '삭제' }));
        await fireEvent.click(
            within(memoryDeleteActions).getByRole('button', { name: '삭제 확인' }),
        );
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

        await fireEvent.click(screen.getByRole('button', { name: /^콘텐츠 모듈/ }));
        await fireEvent.click(screen.getByRole('button', { name: /^module-default/ }));
        const moduleEditor = screen.getByRole('form', { name: '콘텐츠 모듈 JSON 편집' });
        await fireEvent.input(
            within(moduleEditor).getByRole('textbox', { name: /^안전 문서 JSON/ }),
            {
                target: {
                    value: JSON.stringify({
                        ...CONTENT_MODULE,
                        asset_ids: ['asset-not-resolved'],
                    }),
                },
            },
        );
        const moduleEditActions = screen.getByRole('toolbar', {
            name: '콘텐츠 모듈 편집 작업',
        });
        expect(moduleEditActions).toHaveClass('fixed');
        await fireEvent.click(within(moduleEditActions).getByRole('button', { name: '저장' }));

        expect(
            await screen.findByText('현재 안전 CRUD 경로에서는 asset_ids가 빈 배열이어야 합니다.'),
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

        await fireEvent.click(screen.getByRole('button', { name: /^메모리 프로필/ }));
        await fireEvent.click(screen.getByRole('button', { name: /^memory-default/ }));
        const memoryEditor = screen.getByRole('form', { name: '메모리 프로필 JSON 편집' });
        const editor = within(memoryEditor).getByRole('textbox', { name: /^안전 문서 JSON/ });
        const submitted = { ...MEMORY_PROFILE, name: 'Submitted memory' };
        const newerDraft = { ...submitted, summary_schema: 'newer-unsaved-schema' };
        await fireEvent.input(editor, {
            target: { value: JSON.stringify(submitted) },
        });
        const memoryEditActions = screen.getByRole('toolbar', {
            name: '메모리 프로필 편집 작업',
        });
        expect(memoryEditActions).toHaveClass('fixed');
        await fireEvent.click(within(memoryEditActions).getByRole('button', { name: '저장' }));
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

        await screen.findByRole('toolbar', { name: '메모리 프로필 작업' });
        await fireEvent.click(screen.getByRole('button', { name: /^memory-default/ }));
        expect(screen.getByRole('textbox', { name: /^안전 문서 JSON/ })).toHaveValue(
            JSON.stringify(newerDraft),
        );
        expect(fixture.replaceCreatorDocument).toHaveBeenCalledOnce();
        expect(fixture.replaceCreatorDocument).toHaveBeenCalledWith(
            'memory_profile',
            MEMORY_PROFILE.id,
            submitted,
        );
    });
});
