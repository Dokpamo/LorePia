import { t } from '../../../lib/i18n';
import type {
    CreatorPageKind,
    CreatorDocumentsPage,
    ReadPageCursor,
} from '../../../lib/ipc/contracts/pagination';
import { creatorCursorAdvances } from '../../../lib/ipc/pagination-cursor';
import {
    editableCreatorDocuments,
    errorLabel,
    type CreatorDocumentValue,
    type EditableCreatorDocumentState,
    type OrchestrationCapableClient,
    type OrchestrationState,
} from './orchestration-state';
import type { OrchestrationStateController } from './orchestration-state-controller';

const collections = {
    memory_profile: 'editable_memory_profiles',
    knowledge_book: 'editable_knowledge_books',
    transform_set: 'editable_transform_sets',
    interaction_rule_set: 'editable_interaction_rule_sets',
    content_module: 'editable_content_modules',
} as const;

/** Context and per-page request ownership prevents stale pages from replacing another room. */
export class CatalogPageController {
    private readonly epochs = new Map<string, number>();
    private readonly activeCreatorLoads = new Map<number, number>();
    constructor(
        private readonly client: OrchestrationCapableClient,
        private readonly state: OrchestrationStateController,
    ) {}

    async loadInitialCreatorPages(contextKey: string): Promise<void> {
        this.state.updateForContext(contextKey, (state) => ({
            ...state,
            creator_document_cursors: {},
            editable_creator_documents_error: null,
        }));
        await Promise.all(
            (Object.keys(collections) as CreatorPageKind[]).map((kind) =>
                this.loadCreatorPage(kind, true, contextKey),
            ),
        );
    }
    async loadCreatorPage(
        kind: CreatorPageKind,
        restart = false,
        contextKey = this.state.snapshot().context_key,
    ): Promise<void> {
        const loader = this.client.listCreatorDocumentsPage;
        if (loader === undefined) return;
        const initial = this.state.snapshot();
        const contextEpoch = this.state.currentContextEpoch();
        const after = restart ? null : (initial.creator_document_cursors?.[kind] ?? null);
        if (!restart && after === null) return;
        const key = kind;
        const epoch = (this.epochs.get(key) ?? 0) + 1;
        this.epochs.set(key, epoch);
        this.activeCreatorLoads.set(
            contextEpoch,
            (this.activeCreatorLoads.get(contextEpoch) ?? 0) + 1,
        );
        this.state.updateForContext(contextKey, (state) => ({
            ...state,
            editable_creator_documents_loading: true,
            editable_creator_documents_error: null,
        }));
        try {
            const page = await loader.call(this.client, { kind, after, limit: 50 });
            if (this.epochs.get(key) !== epoch || !this.state.isContextEpoch(contextEpoch)) return;
            validateCreatorPage(page, kind, after);
            this.state.updateForContext(contextKey, (state) =>
                mergeCreatorPage(state, kind, page, restart),
            );
        } catch (error: unknown) {
            if (this.epochs.get(key) !== epoch || !this.state.isContextEpoch(contextEpoch)) return;
            this.state.updateForContext(contextKey, (state) => ({
                ...state,
                editable_creator_documents_error: errorLabel(error),
            }));
        } finally {
            const remaining = (this.activeCreatorLoads.get(contextEpoch) ?? 1) - 1;
            if (remaining === 0) this.activeCreatorLoads.delete(contextEpoch);
            else this.activeCreatorLoads.set(contextEpoch, remaining);
            if (this.state.isContextEpoch(contextEpoch))
                this.state.updateForContext(contextKey, (state) => ({
                    ...state,
                    editable_creator_documents_loading: remaining > 0,
                }));
        }
    }
    async loadMemoryPage(restart = false): Promise<void> {
        const initial = this.state.snapshot();
        const contextEpoch = this.state.currentContextEpoch();
        const loader = this.client.listMemoryRecordsPage;
        if (initial.phase !== 'ready' || loader === undefined || initial.memory_page_loading)
            return;
        const after = restart ? null : (initial.workspace.memory_records_next_cursor ?? null);
        if (!restart && after === null) return;
        const contextKey = initial.context_key;
        const epoch = (this.epochs.get('memory') ?? 0) + 1;
        this.epochs.set('memory', epoch);
        const { conversation_id, branch_id } = initial.workspace.room_config;
        this.state.updateForContext(contextKey, (state) => ({
            ...state,
            memory_page_loading: true,
            error: null,
        }));
        try {
            const page = await loader.call(this.client, {
                conversation_id,
                branch_id,
                include_invalidated: false,
                after,
                limit: 100,
            });
            if (this.epochs.get('memory') !== epoch || !this.state.isContextEpoch(contextEpoch))
                return;
            if (
                page.records.length > 100 ||
                page.records.some(
                    (record) =>
                        record.conversation_id !== conversation_id ||
                        record.source_navigation.conversation_id !== conversation_id ||
                        record.branch_id !== record.source_navigation.branch_id,
                ) ||
                (page.next_cursor !== null &&
                    (page.records.length === 0 ||
                        (after !== null && page.next_cursor.after_id <= after.after_id)))
            ) {
                throw new Error(t('pagination.invalid_page'));
            }
            this.state.updateForContext(contextKey, (state) => {
                const loadedIds = new Set(
                    state.workspace.memory_records.map((record) => record.id),
                );
                return {
                    ...state,
                    workspace: {
                        ...state.workspace,
                        memory_records: restart
                            ? page.records
                            : [
                                  ...state.workspace.memory_records,
                                  ...page.records.filter((record) => !loadedIds.has(record.id)),
                              ],
                        memory_records_next_cursor: page.next_cursor,
                    },
                    list_truncation: { ...state.list_truncation, memory_records: false },
                };
            });
        } catch (error: unknown) {
            if (this.epochs.get('memory') === epoch && this.state.isContextEpoch(contextEpoch))
                this.state.updateForContext(contextKey, (state) => ({
                    ...state,
                    error: errorLabel(error),
                }));
        } finally {
            if (this.state.isContextEpoch(contextEpoch))
                this.state.updateForContext(contextKey, (state) => ({
                    ...state,
                    memory_page_loading: false,
                }));
        }
    }
}
function validateCreatorPage(
    page: CreatorDocumentsPage,
    kind: CreatorPageKind,
    after: ReadPageCursor | null,
): void {
    if (
        page.kind !== kind ||
        page.documents.length > 100 ||
        (page.next_cursor !== null &&
            (page.documents.length === 0 ||
                (after !== null && !creatorCursorAdvances(after, page.next_cursor))))
    )
        throw new Error(t('pagination.invalid_page'));
}
function mergeCreatorPage(
    state: OrchestrationState,
    kind: CreatorPageKind,
    page: CreatorDocumentsPage,
    restart: boolean,
): OrchestrationState {
    const key = collections[kind];
    const existing: EditableCreatorDocumentState<CreatorDocumentValue>[] = state[key];
    const incoming = editableCreatorDocuments(page.documents);
    const byId = new Map(existing.map((document) => [document.value.id, document]));
    const incomingIds = new Set(incoming.map((document) => document.value.id));
    const documents = restart
        ? [
              ...incoming.map((document) => {
                  const current = byId.get(document.value.id);
                  return current?.dirty ? current : document;
              }),
              ...existing.filter(
                  (document) => document.dirty && !incomingIds.has(document.value.id),
              ),
          ]
        : [...existing, ...incoming.filter((document) => !byId.has(document.value.id))];
    return {
        ...state,
        [key]: documents,
        creator_document_cursors: { ...state.creator_document_cursors, [kind]: page.next_cursor },
    };
}
