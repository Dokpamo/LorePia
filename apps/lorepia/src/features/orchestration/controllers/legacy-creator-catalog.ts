import { t } from '../../../lib/i18n';
import {
    editableCreatorDocuments,
    errorLabel,
    type OrchestrationCapableClient,
} from './orchestration-state';
import type { OrchestrationStateController } from './orchestration-state-controller';

export async function loadLegacyCreatorDocuments(
    client: OrchestrationCapableClient,
    stateController: OrchestrationStateController,
    contextKey: string,
): Promise<void> {
    const listMemoryProfiles = client.listMemoryProfiles;
    const listKnowledgeBooks = client.listKnowledgeBooks;
    const listTransformSets = client.listTransformSets;
    const listInteractionRuleSets = client.listInteractionRuleSets;
    const listContentModules = client.listContentModules;
    if (
        listMemoryProfiles === undefined ||
        listKnowledgeBooks === undefined ||
        listTransformSets === undefined ||
        listInteractionRuleSets === undefined ||
        listContentModules === undefined
    ) {
        stateController.updateForContext(contextKey, (state) => ({
            ...state,
            editable_creator_documents_loading: false,
            editable_creator_documents_error: t('orchestration.error.unsupported_creator_edit'),
        }));
        return;
    }
    stateController.updateForContext(contextKey, (state) => ({
        ...state,
        editable_creator_documents_loading: true,
        editable_creator_documents_error: null,
    }));
    try {
        const [memoryProfiles, knowledgeBooks, transformSets, interactionRuleSets, contentModules] =
            await Promise.all([
                listMemoryProfiles.call(client),
                listKnowledgeBooks.call(client),
                listTransformSets.call(client),
                listInteractionRuleSets.call(client),
                listContentModules.call(client),
            ]);
        stateController.updateForContext(contextKey, (state) => ({
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
        stateController.updateForContext(contextKey, (state) => ({
            ...state,
            editable_creator_documents_loading: false,
            editable_creator_documents_error: errorLabel(error),
        }));
    }
}
