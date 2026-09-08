import type { RevisionedDto } from '../contracts';
import type {
    CreatorDocumentsByKind,
    CreatorDocumentsPage,
    CreatorPageKind,
    CreatorPageRequest,
    MemoryRecordsPage,
    MemoryRecordsPageRequest,
    ReadPageCursor,
} from '../contracts/pagination';
import { LOREPIA_COMMANDS } from '../commands';
import { DiscoveryClient } from './discovery';

export abstract class PaginationClient extends DiscoveryClient {
    listCreatorDocumentsPage<K extends CreatorPageKind>(
        input: CreatorPageRequest<K>,
    ): Promise<CreatorDocumentsPage<K>> {
        return this.call(LOREPIA_COMMANDS.listCreatorDocumentsPage, { request: input });
    }
    listMemoryRecordsPage(input: MemoryRecordsPageRequest): Promise<MemoryRecordsPage> {
        return this.call(LOREPIA_COMMANDS.listMemoryRecordsPage, { request: input });
    }
    /** Compatibility collection callers consume bounded pages; creator editors use explicit pages. */
    protected async allCreatorDocuments<K extends CreatorPageKind>(
        kind: K,
    ): Promise<RevisionedDto<CreatorDocumentsByKind[K]>[]> {
        const documents: RevisionedDto<CreatorDocumentsByKind[K]>[] = [];
        let after: ReadPageCursor | null = null;
        do {
            const page: CreatorDocumentsPage<K> = await this.listCreatorDocumentsPage({
                kind,
                after,
                limit: 100,
            });
            if (
                page.kind !== kind ||
                (page.next_cursor !== null &&
                    (page.documents.length === 0 ||
                        (after !== null && page.next_cursor.after_id <= after.after_id)))
            ) {
                throw new Error('Invalid creator page continuation');
            }
            documents.push(...page.documents);
            after = page.next_cursor;
        } while (after !== null);
        return documents;
    }
}
