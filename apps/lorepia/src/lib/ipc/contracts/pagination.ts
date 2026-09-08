import type {
    CreatorMemoryProfileDocumentDto,
    CreatorKnowledgeBookDocumentDto,
    CreatorTransformSetDocumentDto,
    CreatorInteractionRuleSetDocumentDto,
    CreatorContentModuleDocumentDto,
    RevisionedDto,
    MemoryRecordDto,
} from '../contracts';

export interface ReadPageCursor {
    scope: string;
    after_id: string;
}
export interface CreatorDocumentsByKind {
    memory_profile: CreatorMemoryProfileDocumentDto;
    knowledge_book: CreatorKnowledgeBookDocumentDto;
    transform_set: CreatorTransformSetDocumentDto;
    interaction_rule_set: CreatorInteractionRuleSetDocumentDto;
    content_module: CreatorContentModuleDocumentDto;
}
export type CreatorPageKind = keyof CreatorDocumentsByKind;
export interface CreatorPageRequest<K extends CreatorPageKind = CreatorPageKind> {
    kind: K;
    after: ReadPageCursor | null;
    limit: number;
}
export interface CreatorDocumentsPage<K extends CreatorPageKind = CreatorPageKind> {
    kind: K;
    documents: RevisionedDto<CreatorDocumentsByKind[K]>[];
    next_cursor: ReadPageCursor | null;
}
export interface MemoryRecordsPageRequest {
    conversation_id: string;
    branch_id: string;
    include_invalidated: boolean;
    after: ReadPageCursor | null;
    limit: number;
}
export interface MemoryRecordsPage {
    records: MemoryRecordDto[];
    next_cursor: ReadPageCursor | null;
}
export interface PaginationClientApi {
    listCreatorDocumentsPage<K extends CreatorPageKind>(
        input: CreatorPageRequest<K>,
    ): Promise<CreatorDocumentsPage<K>>;
    listMemoryRecordsPage(input: MemoryRecordsPageRequest): Promise<MemoryRecordsPage>;
}
