import { t } from '../../../lib/i18n';
import type {
    CreatorContentModuleDocumentDto,
    CreatorInteractionRuleSetDocumentDto,
    CreatorKnowledgeBookDocumentDto,
    CreatorMemoryProfileDocumentDto,
    CreatorTransformSetDocumentDto,
    RevisionedDto,
} from '../../../lib/ipc/contracts';

import {
    contentModuleDraft,
    editableCreatorDocuments,
    errorLabel,
    interactionRuleSetDraft,
    knowledgeBookDraft,
    memoryProfileDraft,
    replaceEditableCreatorDocument,
    replaceSavedCreatorDocument,
    stageEditableCreatorDocument,
    transformSetDraft,
    validNewCreatorDocumentId,
    type CreatorDocumentKind,
    type CreatorDocumentValue,
    type EditableCreatorDocumentState,
    type OrchestrationCapableClient,
    type OrchestrationState,
} from './orchestration-state';
import type { OrchestrationStateController } from './orchestration-state-controller';

export class CreatorDocumentController {
    constructor(
        private readonly client: OrchestrationCapableClient,
        private readonly state: OrchestrationStateController,
    ) {}

    async loadEditableCreatorDocumentsForContext(contextKey: string): Promise<void> {
        const listMemoryProfiles = this.client.listMemoryProfiles;
        const listKnowledgeBooks = this.client.listKnowledgeBooks;
        const listTransformSets = this.client.listTransformSets;
        const listInteractionRuleSets = this.client.listInteractionRuleSets;
        const listContentModules = this.client.listContentModules;
        if (
            listMemoryProfiles === undefined ||
            listKnowledgeBooks === undefined ||
            listTransformSets === undefined ||
            listInteractionRuleSets === undefined ||
            listContentModules === undefined
        ) {
            this.state.updateForContext(contextKey, (state) => ({
                ...state,
                editable_creator_documents_loading: false,
                editable_creator_documents_error: t('orchestration.error.unsupported_creator_edit'),
            }));
            return;
        }
        this.state.updateForContext(contextKey, (state) => ({
            ...state,
            editable_creator_documents_loading: true,
            editable_creator_documents_error: null,
        }));
        try {
            const [
                memoryProfiles,
                knowledgeBooks,
                transformSets,
                interactionRuleSets,
                contentModules,
            ] = await Promise.all([
                listMemoryProfiles.call(this.client),
                listKnowledgeBooks.call(this.client),
                listTransformSets.call(this.client),
                listInteractionRuleSets.call(this.client),
                listContentModules.call(this.client),
            ]);
            this.state.updateForContext(contextKey, (state) => ({
                ...state,
                editable_memory_profiles: editableCreatorDocuments(memoryProfiles),
                editable_knowledge_books: editableCreatorDocuments(knowledgeBooks),
                editable_transform_sets: editableCreatorDocuments(transformSets),
                editable_interaction_rule_sets: editableCreatorDocuments(interactionRuleSets),
                editable_content_modules: editableCreatorDocuments(contentModules),
                editable_creator_documents_loading: false,
                editable_creator_documents_error: null,
            }));
        } catch (error: unknown) {
            this.state.updateForContext(contextKey, (state) => ({
                ...state,
                editable_creator_documents_loading: false,
                editable_creator_documents_error: errorLabel(error),
            }));
        }
    }
    addCreatorDocumentDraft(kind: CreatorDocumentKind, requestedId: string): boolean {
        const state = this.state.snapshot();
        const id = requestedId.trim();
        if (state.phase !== 'ready' || !validNewCreatorDocumentId(id)) return false;
        const duplicate =
            (kind === 'memory_profile' &&
                state.editable_memory_profiles.some((document) => document.value.id === id)) ||
            (kind === 'knowledge_book' &&
                state.editable_knowledge_books.some((document) => document.value.id === id)) ||
            (kind === 'transform_set' &&
                state.editable_transform_sets.some((document) => document.value.id === id)) ||
            (kind === 'interaction_rule_set' &&
                state.editable_interaction_rule_sets.some(
                    (document) => document.value.id === id,
                )) ||
            (kind === 'content_module' &&
                state.editable_content_modules.some((document) => document.value.id === id));
        if (duplicate) return false;
        return this.state.updateForContext(state.context_key, (current) => {
            const base = {
                ...current,
                editable_creator_documents_error: null,
            };
            if (kind === 'memory_profile') {
                return {
                    ...base,
                    editable_memory_profiles: [
                        ...current.editable_memory_profiles,
                        {
                            value: memoryProfileDraft(id),
                            expected_revision: null,
                            dirty: true,
                        },
                    ],
                };
            }
            if (kind === 'knowledge_book') {
                return {
                    ...base,
                    editable_knowledge_books: [
                        ...current.editable_knowledge_books,
                        {
                            value: knowledgeBookDraft(id),
                            expected_revision: null,
                            dirty: true,
                        },
                    ],
                };
            }
            if (kind === 'transform_set') {
                return {
                    ...base,
                    editable_transform_sets: [
                        ...current.editable_transform_sets,
                        {
                            value: transformSetDraft(id),
                            expected_revision: null,
                            dirty: true,
                        },
                    ],
                };
            }
            if (kind === 'interaction_rule_set') {
                return {
                    ...base,
                    editable_interaction_rule_sets: [
                        ...current.editable_interaction_rule_sets,
                        {
                            value: interactionRuleSetDraft(id),
                            expected_revision: null,
                            dirty: true,
                        },
                    ],
                };
            }
            return {
                ...base,
                editable_content_modules: [
                    ...current.editable_content_modules,
                    {
                        value: contentModuleDraft(id),
                        expected_revision: null,
                        dirty: true,
                    },
                ],
            };
        });
    }

    replaceCreatorDocument(
        kind: CreatorDocumentKind,
        documentId: string,
        value: CreatorDocumentValue,
    ): boolean {
        const state = this.state.snapshot();
        if (value.id !== documentId) return false;
        if (kind === 'memory_profile') {
            if (
                !state.editable_memory_profiles.some((document) => document.value.id === documentId)
            ) {
                return false;
            }
            return this.state.updateForContext(state.context_key, (current) => ({
                ...current,
                editable_memory_profiles: replaceEditableCreatorDocument(
                    current.editable_memory_profiles,
                    documentId,
                    value as CreatorMemoryProfileDocumentDto,
                ),
                editable_creator_documents_error: null,
            }));
        }
        if (kind === 'knowledge_book') {
            if (
                !state.editable_knowledge_books.some((document) => document.value.id === documentId)
            ) {
                return false;
            }
            return this.state.updateForContext(state.context_key, (current) => ({
                ...current,
                editable_knowledge_books: replaceEditableCreatorDocument(
                    current.editable_knowledge_books,
                    documentId,
                    value as CreatorKnowledgeBookDocumentDto,
                ),
                editable_creator_documents_error: null,
            }));
        }
        if (kind === 'transform_set') {
            if (
                !state.editable_transform_sets.some((document) => document.value.id === documentId)
            ) {
                return false;
            }
            return this.state.updateForContext(state.context_key, (current) => ({
                ...current,
                editable_transform_sets: replaceEditableCreatorDocument(
                    current.editable_transform_sets,
                    documentId,
                    value as CreatorTransformSetDocumentDto,
                ),
                editable_creator_documents_error: null,
            }));
        }
        if (kind === 'interaction_rule_set') {
            if (
                !state.editable_interaction_rule_sets.some(
                    (document) => document.value.id === documentId,
                )
            ) {
                return false;
            }
            return this.state.updateForContext(state.context_key, (current) => ({
                ...current,
                editable_interaction_rule_sets: replaceEditableCreatorDocument(
                    current.editable_interaction_rule_sets,
                    documentId,
                    value as CreatorInteractionRuleSetDocumentDto,
                ),
                editable_creator_documents_error: null,
            }));
        }
        if (!state.editable_content_modules.some((document) => document.value.id === documentId)) {
            return false;
        }
        return this.state.updateForContext(state.context_key, (current) => ({
            ...current,
            editable_content_modules: replaceEditableCreatorDocument(
                current.editable_content_modules,
                documentId,
                value as CreatorContentModuleDocumentDto,
            ),
            editable_creator_documents_error: null,
        }));
    }

    stageMemoryProfile(
        documentId: string,
        patch: Partial<CreatorMemoryProfileDocumentDto>,
    ): boolean {
        const state = this.state.snapshot();
        if (!state.editable_memory_profiles.some((document) => document.value.id === documentId)) {
            return false;
        }
        return this.state.updateForContext(state.context_key, (current) => ({
            ...current,
            editable_memory_profiles: stageEditableCreatorDocument(
                current.editable_memory_profiles,
                documentId,
                patch,
            ),
            editable_creator_documents_error: null,
        }));
    }

    stageKnowledgeBook(
        documentId: string,
        patch: Partial<CreatorKnowledgeBookDocumentDto>,
    ): boolean {
        const state = this.state.snapshot();
        if (!state.editable_knowledge_books.some((document) => document.value.id === documentId)) {
            return false;
        }
        return this.state.updateForContext(state.context_key, (current) => ({
            ...current,
            editable_knowledge_books: stageEditableCreatorDocument(
                current.editable_knowledge_books,
                documentId,
                patch,
            ),
            editable_creator_documents_error: null,
        }));
    }

    stageTransformSet(documentId: string, patch: Partial<CreatorTransformSetDocumentDto>): boolean {
        const state = this.state.snapshot();
        if (!state.editable_transform_sets.some((document) => document.value.id === documentId)) {
            return false;
        }
        return this.state.updateForContext(state.context_key, (current) => ({
            ...current,
            editable_transform_sets: stageEditableCreatorDocument(
                current.editable_transform_sets,
                documentId,
                patch,
            ),
            editable_creator_documents_error: null,
        }));
    }

    stageInteractionRuleSet(
        documentId: string,
        patch: Partial<CreatorInteractionRuleSetDocumentDto>,
    ): boolean {
        const state = this.state.snapshot();
        if (
            !state.editable_interaction_rule_sets.some(
                (document) => document.value.id === documentId,
            )
        ) {
            return false;
        }
        return this.state.updateForContext(state.context_key, (current) => ({
            ...current,
            editable_interaction_rule_sets: stageEditableCreatorDocument(
                current.editable_interaction_rule_sets,
                documentId,
                patch,
            ),
            editable_creator_documents_error: null,
        }));
    }

    stageContentModule(
        documentId: string,
        patch: Partial<CreatorContentModuleDocumentDto>,
    ): boolean {
        const state = this.state.snapshot();
        if (!state.editable_content_modules.some((document) => document.value.id === documentId)) {
            return false;
        }
        return this.state.updateForContext(state.context_key, (current) => ({
            ...current,
            editable_content_modules: stageEditableCreatorDocument(
                current.editable_content_modules,
                documentId,
                patch,
            ),
            editable_creator_documents_error: null,
        }));
    }

    private async saveCreatorDocumentValue<Value extends { id: string }>(
        document: EditableCreatorDocumentState<Value>,
        save:
            | ((input: {
                  value: Value;
                  expected_revision: number | null;
              }) => Promise<RevisionedDto<Value>>)
            | undefined,
        currentDocuments: (state: OrchestrationState) => EditableCreatorDocumentState<Value>[],
        applySaved: (state: OrchestrationState, saved: RevisionedDto<Value>) => OrchestrationState,
        label: string,
    ): Promise<boolean> {
        const state = this.state.snapshot();
        if (!document.dirty) return false;
        if (save === undefined) {
            this.state.updateForContext(state.context_key, (current) => ({
                ...current,
                editable_creator_documents_error: t(
                    'orchestration.error.unsupported_document_save',
                    {
                        label,
                    },
                ),
            }));
            return false;
        }
        const contextKey = state.context_key;
        this.state.updateForContext(contextKey, (current) => ({
            ...current,
            editable_creator_documents_loading: true,
            editable_creator_documents_error: null,
        }));
        try {
            const saved = await save.call(this.client, {
                value: document.value,
                expected_revision: document.expected_revision,
            });
            return this.state.updateForContext(contextKey, (current) => {
                const currentDocument = currentDocuments(current).find(
                    (candidate) => candidate.value.id === document.value.id,
                );
                const hasNewerDraft =
                    currentDocument !== undefined &&
                    currentDocument !== document &&
                    currentDocument.dirty;
                return {
                    ...applySaved(current, saved),
                    editable_creator_documents_loading: false,
                    editable_creator_documents_error: null,
                    announcement: hasNewerDraft
                        ? t('orchestration.notice.document_saved_partial', {
                              id: saved.value.id,
                              label,
                          })
                        : t('orchestration.notice.document_saved', { id: saved.value.id, label }),
                };
            });
        } catch (error: unknown) {
            this.state.updateForContext(contextKey, (current) => ({
                ...current,
                editable_creator_documents_loading: false,
                editable_creator_documents_error: errorLabel(error),
            }));
            return false;
        }
    }

    async saveCreatorDocument(kind: CreatorDocumentKind, documentId: string): Promise<boolean> {
        const state = this.state.snapshot();
        if (kind === 'memory_profile') {
            const document = state.editable_memory_profiles.find(
                (candidate) => candidate.value.id === documentId,
            );
            if (document === undefined) return false;
            return this.saveCreatorDocumentValue(
                document,
                this.client.upsertMemoryProfile,
                (current) => current.editable_memory_profiles,
                (current, saved) => ({
                    ...current,
                    editable_memory_profiles: replaceSavedCreatorDocument(
                        current.editable_memory_profiles,
                        saved,
                        document,
                    ),
                }),
                t('orchestration.label.memory_profile'),
            );
        }
        if (kind === 'knowledge_book') {
            const document = state.editable_knowledge_books.find(
                (candidate) => candidate.value.id === documentId,
            );
            if (document === undefined) return false;
            return this.saveCreatorDocumentValue(
                document,
                this.client.upsertKnowledgeBook,
                (current) => current.editable_knowledge_books,
                (current, saved) => ({
                    ...current,
                    editable_knowledge_books: replaceSavedCreatorDocument(
                        current.editable_knowledge_books,
                        saved,
                        document,
                    ),
                }),
                t('orchestration.label.knowledge_book'),
            );
        }
        if (kind === 'transform_set') {
            const document = state.editable_transform_sets.find(
                (candidate) => candidate.value.id === documentId,
            );
            if (document === undefined) return false;
            return this.saveCreatorDocumentValue(
                document,
                this.client.upsertTransformSet,
                (current) => current.editable_transform_sets,
                (current, saved) => ({
                    ...current,
                    editable_transform_sets: replaceSavedCreatorDocument(
                        current.editable_transform_sets,
                        saved,
                        document,
                    ),
                }),
                t('orchestration.label.transform_set'),
            );
        }
        if (kind === 'interaction_rule_set') {
            const document = state.editable_interaction_rule_sets.find(
                (candidate) => candidate.value.id === documentId,
            );
            if (document === undefined) return false;
            return this.saveCreatorDocumentValue(
                document,
                this.client.upsertInteractionRuleSet,
                (current) => current.editable_interaction_rule_sets,
                (current, saved) => ({
                    ...current,
                    editable_interaction_rule_sets: replaceSavedCreatorDocument(
                        current.editable_interaction_rule_sets,
                        saved,
                        document,
                    ),
                }),
                t('orchestration.label.interaction_rule_set'),
            );
        }
        const document = state.editable_content_modules.find(
            (candidate) => candidate.value.id === documentId,
        );
        if (document === undefined) return false;
        return this.saveCreatorDocumentValue(
            document,
            this.client.upsertContentModule,
            (current) => current.editable_content_modules,
            (current, saved) => ({
                ...current,
                editable_content_modules: replaceSavedCreatorDocument(
                    current.editable_content_modules,
                    saved,
                    document,
                ),
            }),
            t('orchestration.label.content_module'),
        );
    }

    private async deleteCreatorDocumentValue<Value, Input>(
        remove: ((input: Input) => Promise<RevisionedDto<Value>>) | undefined,
        input: Input,
        applyDeleted: (state: OrchestrationState) => OrchestrationState,
        label: string,
        documentId: string,
    ): Promise<boolean> {
        const state = this.state.snapshot();
        if (remove === undefined) {
            this.state.updateForContext(state.context_key, (current) => ({
                ...current,
                editable_creator_documents_error: t(
                    'orchestration.error.unsupported_document_delete',
                    {
                        label,
                    },
                ),
            }));
            return false;
        }
        const contextKey = state.context_key;
        this.state.updateForContext(contextKey, (current) => ({
            ...current,
            editable_creator_documents_loading: true,
            editable_creator_documents_error: null,
        }));
        try {
            await remove.call(this.client, input);
            return this.state.updateForContext(contextKey, (current) => ({
                ...applyDeleted(current),
                editable_creator_documents_loading: false,
                editable_creator_documents_error: null,
                announcement: t('orchestration.notice.document_deleted', { id: documentId, label }),
            }));
        } catch (error: unknown) {
            this.state.updateForContext(contextKey, (current) => ({
                ...current,
                editable_creator_documents_loading: false,
                editable_creator_documents_error: errorLabel(error),
            }));
            return false;
        }
    }

    deleteCreatorDocument(kind: CreatorDocumentKind, documentId: string): Promise<boolean> {
        const state = this.state.snapshot();
        if (kind === 'memory_profile') {
            const document = state.editable_memory_profiles.find(
                (candidate) => candidate.value.id === documentId,
            );
            if (document === undefined) return Promise.resolve(false);
            if (document.expected_revision === null) {
                return Promise.resolve(
                    this.state.updateForContext(state.context_key, (current) => ({
                        ...current,
                        editable_memory_profiles: current.editable_memory_profiles.filter(
                            (candidate) => candidate.value.id !== documentId,
                        ),
                        announcement: t('orchestration.notice.draft_discarded', {
                            id: documentId,
                            label: t('orchestration.label.memory_profile'),
                        }),
                    })),
                );
            }
            return this.deleteCreatorDocumentValue(
                this.client.deleteMemoryProfile,
                {
                    memory_profile_id: documentId,
                    expected_revision: document.expected_revision,
                },
                (current) => ({
                    ...current,
                    editable_memory_profiles: current.editable_memory_profiles.filter(
                        (candidate) => candidate.value.id !== documentId,
                    ),
                }),
                t('orchestration.label.memory_profile'),
                documentId,
            );
        }
        if (kind === 'knowledge_book') {
            const document = state.editable_knowledge_books.find(
                (candidate) => candidate.value.id === documentId,
            );
            if (document === undefined) return Promise.resolve(false);
            if (document.expected_revision === null) {
                return Promise.resolve(
                    this.state.updateForContext(state.context_key, (current) => ({
                        ...current,
                        editable_knowledge_books: current.editable_knowledge_books.filter(
                            (candidate) => candidate.value.id !== documentId,
                        ),
                        announcement: t('orchestration.notice.draft_discarded', {
                            id: documentId,
                            label: t('orchestration.label.knowledge_book'),
                        }),
                    })),
                );
            }
            return this.deleteCreatorDocumentValue(
                this.client.deleteKnowledgeBook,
                {
                    knowledge_book_id: documentId,
                    expected_revision: document.expected_revision,
                },
                (current) => ({
                    ...current,
                    editable_knowledge_books: current.editable_knowledge_books.filter(
                        (candidate) => candidate.value.id !== documentId,
                    ),
                }),
                t('orchestration.label.knowledge_book'),
                documentId,
            );
        }
        if (kind === 'transform_set') {
            const document = state.editable_transform_sets.find(
                (candidate) => candidate.value.id === documentId,
            );
            if (document === undefined) return Promise.resolve(false);
            if (document.expected_revision === null) {
                return Promise.resolve(
                    this.state.updateForContext(state.context_key, (current) => ({
                        ...current,
                        editable_transform_sets: current.editable_transform_sets.filter(
                            (candidate) => candidate.value.id !== documentId,
                        ),
                        announcement: t('orchestration.notice.draft_discarded', {
                            id: documentId,
                            label: t('orchestration.label.transform_set'),
                        }),
                    })),
                );
            }
            return this.deleteCreatorDocumentValue(
                this.client.deleteTransformSet,
                {
                    transform_set_id: documentId,
                    expected_revision: document.expected_revision,
                },
                (current) => ({
                    ...current,
                    editable_transform_sets: current.editable_transform_sets.filter(
                        (candidate) => candidate.value.id !== documentId,
                    ),
                }),
                t('orchestration.label.transform_set'),
                documentId,
            );
        }
        if (kind === 'interaction_rule_set') {
            const document = state.editable_interaction_rule_sets.find(
                (candidate) => candidate.value.id === documentId,
            );
            if (document === undefined) return Promise.resolve(false);
            if (document.expected_revision === null) {
                return Promise.resolve(
                    this.state.updateForContext(state.context_key, (current) => ({
                        ...current,
                        editable_interaction_rule_sets:
                            current.editable_interaction_rule_sets.filter(
                                (candidate) => candidate.value.id !== documentId,
                            ),
                        announcement: t('orchestration.notice.draft_discarded', {
                            id: documentId,
                            label: t('orchestration.label.interaction_rule_set'),
                        }),
                    })),
                );
            }
            return this.deleteCreatorDocumentValue(
                this.client.deleteInteractionRuleSet,
                {
                    interaction_rule_set_id: documentId,
                    expected_revision: document.expected_revision,
                },
                (current) => ({
                    ...current,
                    editable_interaction_rule_sets: current.editable_interaction_rule_sets.filter(
                        (candidate) => candidate.value.id !== documentId,
                    ),
                }),
                t('orchestration.label.interaction_rule_set'),
                documentId,
            );
        }
        const document = state.editable_content_modules.find(
            (candidate) => candidate.value.id === documentId,
        );
        if (document === undefined) return Promise.resolve(false);
        if (document.expected_revision === null) {
            return Promise.resolve(
                this.state.updateForContext(state.context_key, (current) => ({
                    ...current,
                    editable_content_modules: current.editable_content_modules.filter(
                        (candidate) => candidate.value.id !== documentId,
                    ),
                    announcement: t('orchestration.notice.draft_discarded', {
                        id: documentId,
                        label: t('orchestration.label.content_module'),
                    }),
                })),
            );
        }
        return this.deleteCreatorDocumentValue(
            this.client.deleteContentModule,
            {
                content_module_id: documentId,
                expected_revision: document.expected_revision,
            },
            (current) => ({
                ...current,
                editable_content_modules: current.editable_content_modules.filter(
                    (candidate) => candidate.value.id !== documentId,
                ),
            }),
            t('orchestration.label.content_module'),
            documentId,
        );
    }
}
