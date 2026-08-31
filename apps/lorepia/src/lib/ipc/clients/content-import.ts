import type {
    ApproveContentPackageImportInput,
    ApproveContentPackageImportReceiptDto,
    CommitContentPackageImportInput,
    CommitContentPackageImportReceiptDto,
    ContentPackageImportReviewDto,
    ContentPackageImportSummaryDto,
    ContentPackageInspectionReviewDto,
    ContentPackageWorkspaceDto,
    ContentSourceExportDescriptorDto,
    ContentSourceExportInput,
    ContentSourceExportReceiptDto,
    ListCompletedContentPackageExportsInput,
    CharacterDto,
    ImportInspectionDto,
    ImportTicketDto,
    ListPendingContentPackageImportsInput,
    ReopenContentPackageImportInput,
    SelectContentPackageImportInput,
    SelectContentPackageImportReceiptDto,
    DiscardContentPackageImportInput,
} from '../contracts';

import { LOREPIA_COMMANDS } from '../commands';

import { InteractionClient } from './interaction';

export abstract class ContentImportClient extends InteractionClient {
    listPendingContentPackageImports(
        input: ListPendingContentPackageImportsInput,
    ): Promise<ContentPackageImportReviewDto[]> {
        return this.call(LOREPIA_COMMANDS.listPendingContentPackageImports, {
            request: input,
        });
    }

    pickContentPackageImport(): Promise<ContentPackageInspectionReviewDto | null> {
        return this.call(LOREPIA_COMMANDS.pickContentPackageImport);
    }

    reopenContentPackageImport(
        input: ReopenContentPackageImportInput,
    ): Promise<ContentPackageWorkspaceDto> {
        return this.call(LOREPIA_COMMANDS.reopenContentPackageImport, { request: input });
    }

    selectContentPackageImport(
        input: SelectContentPackageImportInput,
    ): Promise<SelectContentPackageImportReceiptDto> {
        return this.call(LOREPIA_COMMANDS.selectContentPackageImport, { request: input });
    }

    approveContentPackageImport(
        input: ApproveContentPackageImportInput,
    ): Promise<ApproveContentPackageImportReceiptDto> {
        return this.call(LOREPIA_COMMANDS.approveContentPackageImport, { request: input });
    }

    commitContentPackageImport(
        input: CommitContentPackageImportInput,
    ): Promise<CommitContentPackageImportReceiptDto> {
        return this.call(LOREPIA_COMMANDS.commitContentPackageImport, { request: input });
    }

    discardContentPackageImport(
        input: DiscardContentPackageImportInput,
    ): Promise<ContentPackageImportSummaryDto> {
        return this.call(LOREPIA_COMMANDS.discardContentPackageImport, { request: input });
    }

    listCompletedContentPackageExports(
        input: ListCompletedContentPackageExportsInput,
    ): Promise<ContentSourceExportDescriptorDto[]> {
        return this.call(LOREPIA_COMMANDS.listCompletedContentPackageExports, { request: input });
    }

    exportContentSource(
        input: ContentSourceExportInput,
    ): Promise<ContentSourceExportReceiptDto | null> {
        return this.call(LOREPIA_COMMANDS.exportContentSource, { request: input });
    }

    selectImportSource(): Promise<ImportTicketDto | null> {
        return this.call(LOREPIA_COMMANDS.pickImport);
    }

    inspectImport(ticketId: string): Promise<ImportInspectionDto> {
        return this.call(LOREPIA_COMMANDS.inspectImport, {
            request: { ticket_id: ticketId },
        });
    }

    commitImport(inspectionId: string): Promise<CharacterDto> {
        return this.call(LOREPIA_COMMANDS.commitImport, {
            request: { inspection_id: inspectionId },
        });
    }

    discardImport(inspectionId: string): Promise<void> {
        return this.call(LOREPIA_COMMANDS.discardImport, {
            request: { kind: 'inspection', inspection_id: inspectionId },
        });
    }
}
