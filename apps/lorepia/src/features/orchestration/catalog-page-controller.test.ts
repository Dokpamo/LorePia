import { describe, expect, it, vi } from 'vitest';
import type { CreatorKnowledgeBookDocumentDto, MemoryRecordDto } from '../../lib/ipc/contracts';
import type {
    CreatorDocumentsPage,
    CreatorPageRequest,
    MemoryRecordsPageRequest,
    ReadPageCursor,
} from '../../lib/ipc/contracts/pagination';
import { CatalogPageController } from './controllers/catalog-page-controller';
import {
    INITIAL_ORCHESTRATION_STATE,
    emptyOrchestrationWorkspace,
    type OrchestrationCapableClient,
} from './controllers/orchestration-state';
import { OrchestrationStateController } from './controllers/orchestration-state-controller';
import { LiveLorepiaClient, type LorepiaTransport } from '../../lib/ipc/client';

const scope = 'a'.repeat(64);
const revision = {
    revision: 1,
    created_at: '2026-09-08T00:00:00Z',
    updated_at: '2026-09-08T00:00:00Z',
    deleted_at: null,
};
function book(index: number) {
    const value: CreatorKnowledgeBookDocumentDto = {
        id: `book-${String(index).padStart(3, '0')}`,
        name: `Book ${String(index)}`,
        entries: [],
        scan_depth: 4,
        token_budget: { max_tokens: 1024 },
        recursive: false,
        max_recursion_depth: 0,
    };
    return { ...revision, value };
}
function ready() {
    const state = new OrchestrationStateController();
    state.set({
        ...structuredClone(INITIAL_ORCHESTRATION_STATE),
        phase: 'ready',
        context_key: 'conversation:branch',
        workspace: emptyOrchestrationWorkspace('conversation', 'branch'),
    });
    return state;
}
function memory(index: number): MemoryRecordDto {
    return {
        id: `memory-${String(index).padStart(3, '0')}`,
        conversation_id: 'conversation',
        branch_id: 'branch',
        title: `Memory ${String(index)}`,
        summary: 'Editable memory',
        kind: 'conversation_summary',
        importance: 50,
        keywords: [],
        pinned: false,
        excluded_from_conversation: false,
        excluded_from_character: false,
        source_navigation: {
            conversation_id: 'conversation',
            branch_id: 'branch',
            start_message_id: 'head',
            end_message_id: 'head',
        },
        invalidated_at: null,
        updated_at: revision.updated_at,
        revision: 1,
    };
}
function cursor(after_id: string, after_updated_at?: string): ReadPageCursor {
    return { scope, after_id, after_updated_at };
}

function recentBooks(count: number) {
    return Array.from({ length: count }, (_, index) => ({
        ...book(index),
        updated_at: new Date(Date.UTC(2026, 8, 8, 0, 0, index)).toISOString(),
    })).reverse();
}

describe('creator and memory page access', () => {
    it('keeps every creator document reachable after100 and preserves edits when appending', async () => {
        const books = recentBooks(105);
        const listCreatorDocumentsPage = vi.fn((request: CreatorPageRequest) => {
            const start =
                request.after === null
                    ? 0
                    : books.findIndex((b) => b.value.id === request.after?.after_id) + 1;
            const documents =
                request.kind === 'knowledge_book' ? books.slice(start, start + request.limit) : [];
            return Promise.resolve({
                kind: request.kind,
                documents,
                next_cursor:
                    start + documents.length < books.length && documents.length > 0
                        ? cursor(documents.at(-1)?.value.id ?? '', documents.at(-1)?.updated_at)
                        : null,
            });
        });
        const state = ready();
        const controller = new CatalogPageController(
            { listCreatorDocumentsPage } as unknown as OrchestrationCapableClient,
            state,
        );
        await controller.loadInitialCreatorPages('conversation:branch');
        expect(state.snapshot().editable_knowledge_books).toHaveLength(50);
        state.update((value) => ({
            ...value,
            editable_knowledge_books: value.editable_knowledge_books.map((document, index) =>
                index === 0
                    ? {
                          ...document,
                          dirty: true,
                          value: { ...document.value, name: 'Unsaved edit' },
                      }
                    : document,
            ),
        }));
        await controller.loadCreatorPage('knowledge_book');
        await controller.loadCreatorPage('knowledge_book');
        const documents = state.snapshot().editable_knowledge_books;
        expect(documents).toHaveLength(105);
        expect(documents.at(-1)?.value.id).toBe('book-000');
        expect(documents[0]?.value.name).toBe('Unsaved edit');
        expect(state.snapshot().creator_document_cursors?.knowledge_book).toBeNull();
        state.update((value) => ({
            ...value,
            editable_knowledge_books: value.editable_knowledge_books.map((document) =>
                document.value.id === 'book-000' ? { ...document, dirty: true } : document,
            ),
        }));
        await controller.loadCreatorPage('knowledge_book', true);
        expect(
            state
                .snapshot()
                .editable_knowledge_books.find((document) => document.value.id === 'book-000')
                ?.dirty,
        ).toBe(true);
    });
    it('loads memory beyond250 and ignores pages from an earlier same-room refresh', async () => {
        const all = Array.from({ length: 275 }, (_, index) => memory(index));
        let pending:
            | ((page: { records: MemoryRecordDto[]; next_cursor: ReadPageCursor | null }) => void)
            | undefined;
        const listMemoryRecordsPage = vi.fn((request: MemoryRecordsPageRequest) => {
            if (request.after?.after_id === 'memory-274')
                return new Promise<{
                    records: MemoryRecordDto[];
                    next_cursor: ReadPageCursor | null;
                }>((resolve) => {
                    pending = resolve;
                });
            const start = request.after === null ? 0 : Number(request.after.after_id.slice(7)) + 1;
            const records = all.slice(start, start + request.limit);
            return Promise.resolve({
                records,
                next_cursor:
                    start + records.length < all.length ? cursor(records.at(-1)?.id ?? '') : null,
            });
        });
        const state = ready();
        const controller = new CatalogPageController(
            { listMemoryRecordsPage } as unknown as OrchestrationCapableClient,
            state,
        );
        await controller.loadMemoryPage(true);
        await controller.loadMemoryPage();
        await controller.loadMemoryPage();
        expect(state.snapshot().workspace.memory_records).toHaveLength(275);
        expect(state.snapshot().workspace.memory_records.at(-1)?.id).toBe('memory-274');
        state.update((value) => ({
            ...value,
            workspace: { ...value.workspace, memory_records_next_cursor: cursor('memory-274') },
        }));
        const oldLoad = controller.loadMemoryPage();
        state.beginContextLoad();
        state.update((value) => ({
            ...value,
            workspace: { ...value.workspace, memory_records: [] },
        }));
        pending?.({ records: [memory(999)], next_cursor: null });
        await oldLoad;
        expect(state.snapshot().workspace.memory_records).toEqual([]);
    });
    it('finishes a new context independently of outstanding creator pages in the old context', async () => {
        const state = ready();
        const pending: (() => void)[] = [];
        let calls = 0;
        const loader = vi.fn((request: CreatorPageRequest) => {
            if (calls++ < 5)
                return new Promise((resolve) =>
                    pending.push(() =>
                        resolve({ kind: request.kind, documents: [], next_cursor: null }),
                    ),
                );
            return Promise.resolve({ kind: request.kind, documents: [], next_cursor: null });
        });
        const controller = new CatalogPageController(
            { listCreatorDocumentsPage: loader } as unknown as OrchestrationCapableClient,
            state,
        );
        const old = controller.loadInitialCreatorPages('conversation:branch');
        state.beginContextLoad();
        state.update((value) => ({ ...value, context_key: 'next:branch' }));
        await controller.loadInitialCreatorPages('next:branch');
        expect(state.snapshot().editable_creator_documents_loading).toBe(false);
        pending.forEach((resolve) => resolve());
        await old;
        expect(state.snapshot().editable_creator_documents_loading).toBe(false);
    });
    it('keeps existing settings collection clients complete via bounded IPC pages', async () => {
        const books = recentBooks(205);
        const invoke = vi.fn((_command: string, args?: Record<string, unknown>) => {
            const request = args?.request as CreatorPageRequest;
            const start =
                request.after === null
                    ? 0
                    : books.findIndex((b) => b.value.id === request.after?.after_id) + 1;
            const documents = books.slice(start, start + request.limit);
            const last = documents.at(-1);
            if (last === undefined) throw new Error('Expected a nonempty fixture page');
            return Promise.resolve({
                kind: request.kind,
                documents,
                next_cursor:
                    start + documents.length < books.length
                        ? cursor(last.value.id, last.updated_at)
                        : null,
            } satisfies CreatorDocumentsPage);
        });
        const client = new LiveLorepiaClient({ invoke } as unknown as LorepiaTransport);
        expect(await client.listKnowledgeBooks()).toEqual(books);
        expect(invoke.mock.calls.map((call) => call[0])).toEqual([
            'list_creator_documents_page',
            'list_creator_documents_page',
            'list_creator_documents_page',
        ]);
    });
});
