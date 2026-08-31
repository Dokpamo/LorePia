import type {
    BeginProviderDiscoveryCurlInput,
    BeginProviderDiscoveryInput,
    CapturedProviderDiscoveryDto,
    ContinueProviderDiscoveryInput,
    ProviderCatalogDiffDto,
    ProviderCatalogHistoryDto,
    ProviderCatalogImportResultDto,
    ProviderCatalogImportTicketDto,
    ProviderCatalogRollbackPlanDto,
    ProviderCatalogRollbackResultDto,
    ProviderCatalogStatusDto,
    ProviderConnectionDto,
    ProviderDiscoveryApprovalProposalDto,
    ProviderDiscoveryReviewProposalDto,
    ProviderDiscoverySessionDto,
    DiscoveryApprovalRecordDto,
    DiscoveryAssistantFailureKindInput,
    DiscoveryAssistantHostActionDto,
    DiscoveryAssistantInterruptionOutcomeInput,
    DiscoveryAssistantResumeBoundaryDto,
    DiscoveryCandidateDto,
    DiscoveryCompensationRecordDto,
    DiscoveryEvidenceDto,
    DiscoveryOutboxEventDto,
    DiscoveryRecoveryResultDto,
    DiscoveryReviewDto,
} from '../contracts';

import { LOREPIA_COMMANDS } from '../commands';

import { ProviderClient } from './provider';

export abstract class DiscoveryClient extends ProviderClient {
    beginProviderDiscovery(
        input: BeginProviderDiscoveryInput,
    ): Promise<ProviderDiscoverySessionDto> {
        return this.call(LOREPIA_COMMANDS.beginProviderDiscovery, {
            request: { input },
        });
    }

    beginProviderDiscoveryCurl(
        input: BeginProviderDiscoveryCurlInput,
    ): Promise<CapturedProviderDiscoveryDto> {
        return this.call(LOREPIA_COMMANDS.beginProviderDiscoveryCurl, {
            request: { input },
        });
    }

    listProviderDiscoveries(limit: number): Promise<ProviderDiscoverySessionDto[]> {
        return this.call(LOREPIA_COMMANDS.listProviderDiscoveries, {
            request: { limit },
        });
    }

    getProviderDiscovery(sessionId: string): Promise<ProviderDiscoverySessionDto> {
        return this.call(LOREPIA_COMMANDS.getProviderDiscovery, {
            request: { session_id: sessionId },
        });
    }

    listProviderDiscoveryCandidates(sessionId: string): Promise<DiscoveryCandidateDto[]> {
        return this.call(LOREPIA_COMMANDS.listProviderDiscoveryCandidates, {
            request: { session_id: sessionId },
        });
    }

    listProviderDiscoveryEvidence(sessionId: string): Promise<DiscoveryEvidenceDto[]> {
        return this.call(LOREPIA_COMMANDS.listProviderDiscoveryEvidence, {
            request: { session_id: sessionId },
        });
    }

    listProviderDiscoveryApprovals(sessionId: string): Promise<DiscoveryApprovalRecordDto[]> {
        return this.call(LOREPIA_COMMANDS.listProviderDiscoveryApprovals, {
            request: { session_id: sessionId },
        });
    }

    getProviderDiscoveryReview(sessionId: string): Promise<DiscoveryReviewDto | null> {
        return this.call(LOREPIA_COMMANDS.getProviderDiscoveryReview, {
            request: { session_id: sessionId },
        });
    }

    getProviderDiscoveryApprovalProposal(
        sessionId: string,
    ): Promise<ProviderDiscoveryApprovalProposalDto | null> {
        return this.call(LOREPIA_COMMANDS.getProviderDiscoveryApprovalProposal, {
            request: { session_id: sessionId },
        });
    }

    getProviderDiscoveryReviewProposal(
        sessionId: string,
    ): Promise<ProviderDiscoveryReviewProposalDto | null> {
        return this.call(LOREPIA_COMMANDS.getProviderDiscoveryReviewProposal, {
            request: { session_id: sessionId },
        });
    }

    getProviderDiscoveryAssistantResumeBoundary(
        sessionId: string,
    ): Promise<DiscoveryAssistantResumeBoundaryDto | null> {
        return this.call(LOREPIA_COMMANDS.getProviderDiscoveryAssistantResumeBoundary, {
            request: { session_id: sessionId },
        });
    }

    runProviderDiscoveryAssistantTurn(sessionId: string): Promise<DiscoveryAssistantHostActionDto> {
        return this.call(LOREPIA_COMMANDS.runProviderDiscoveryAssistantTurn, {
            request: { session_id: sessionId },
        });
    }

    resumeProviderDiscoveryAssistantCoreHostAction(
        sessionId: string,
    ): Promise<ProviderDiscoverySessionDto> {
        return this.call(LOREPIA_COMMANDS.resumeProviderDiscoveryAssistantCoreHostAction, {
            request: { session_id: sessionId },
        });
    }

    approveProviderDiscoveryAssistantRetry(
        sessionId: string,
    ): Promise<ProviderDiscoverySessionDto> {
        return this.call(LOREPIA_COMMANDS.approveProviderDiscoveryAssistantRetry, {
            request: { session_id: sessionId },
        });
    }

    requestProviderDiscoveryAssistantRevision(
        sessionId: string,
    ): Promise<ProviderDiscoverySessionDto> {
        return this.call(LOREPIA_COMMANDS.requestProviderDiscoveryAssistantRevision, {
            request: { session_id: sessionId },
        });
    }

    acceptProviderDiscoveryAssistantDraft(sessionId: string): Promise<ProviderDiscoverySessionDto> {
        return this.call(LOREPIA_COMMANDS.acceptProviderDiscoveryAssistantDraft, {
            request: { session_id: sessionId },
        });
    }

    recordProviderDiscoveryAssistantFailure(
        sessionId: string,
        kind: DiscoveryAssistantFailureKindInput,
        retryable: boolean,
    ): Promise<ProviderDiscoverySessionDto> {
        return this.call(LOREPIA_COMMANDS.recordProviderDiscoveryAssistantFailure, {
            request: { session_id: sessionId, kind, retryable },
        });
    }

    interruptProviderDiscoveryAssistant(
        sessionId: string,
        outcome: DiscoveryAssistantInterruptionOutcomeInput,
    ): Promise<ProviderDiscoverySessionDto> {
        return this.call(LOREPIA_COMMANDS.interruptProviderDiscoveryAssistant, {
            request: { session_id: sessionId, outcome },
        });
    }

    restartProviderDiscoveryAssistantAfterInterruption(
        sessionId: string,
    ): Promise<ProviderDiscoverySessionDto> {
        return this.call(LOREPIA_COMMANDS.restartProviderDiscoveryAssistantAfterInterruption, {
            request: { session_id: sessionId },
        });
    }

    continueProviderDiscovery(
        input: ContinueProviderDiscoveryInput,
    ): Promise<ProviderDiscoverySessionDto> {
        return this.call(LOREPIA_COMMANDS.continueProviderDiscovery, {
            request: { input },
        });
    }

    supplyProviderDiscoveryDocumentEvidence(
        sessionId: string,
        expectedRevision: number,
        documentUrl: string,
    ): Promise<ProviderDiscoverySessionDto> {
        return this.call(LOREPIA_COMMANDS.supplyProviderDiscoveryDocumentEvidence, {
            request: {
                session_id: sessionId,
                expected_revision: expectedRevision,
                document_url: documentUrl,
            },
        });
    }

    supplyProviderDiscoveryCurlEvidence(
        sessionId: string,
        expectedRevision: number,
    ): Promise<CapturedProviderDiscoveryDto> {
        return this.call(LOREPIA_COMMANDS.supplyProviderDiscoveryCurlEvidence, {
            request: {
                session_id: sessionId,
                expected_revision: expectedRevision,
            },
        });
    }

    cancelProviderDiscovery(
        sessionId: string,
        expectedRevision: number,
    ): Promise<ProviderDiscoverySessionDto> {
        return this.call(LOREPIA_COMMANDS.cancelProviderDiscovery, {
            request: {
                session_id: sessionId,
                expected_revision: expectedRevision,
            },
        });
    }

    commitProviderDiscovery(sessionId: string): Promise<ProviderConnectionDto> {
        return this.call(LOREPIA_COMMANDS.commitProviderDiscovery, {
            request: { session_id: sessionId },
        });
    }

    pollProviderDiscoveryEvents(limit: number): Promise<DiscoveryOutboxEventDto[]> {
        return this.call(LOREPIA_COMMANDS.pollProviderDiscoveryEvents, {
            request: { limit },
        });
    }

    pollProviderDiscoveryEventsForSession(
        sessionId: string,
        limit: number,
    ): Promise<DiscoveryOutboxEventDto[]> {
        return this.call(LOREPIA_COMMANDS.pollProviderDiscoveryEventsForSession, {
            request: {
                session_id: sessionId,
                limit,
            },
        });
    }

    ackProviderDiscoveryEvent(eventId: string): Promise<boolean> {
        return this.call(LOREPIA_COMMANDS.ackProviderDiscoveryEvent, {
            request: { event_id: eventId },
        });
    }

    recoverProviderDiscovery(): Promise<DiscoveryRecoveryResultDto[]> {
        return this.call(LOREPIA_COMMANDS.recoverProviderDiscovery);
    }

    listProviderDiscoveryCompensationSteps(
        commitAttemptId: string,
    ): Promise<DiscoveryCompensationRecordDto[]> {
        return this.call(LOREPIA_COMMANDS.listProviderDiscoveryCompensationSteps, {
            request: { commit_attempt_id: commitAttemptId },
        });
    }

    continueProviderDiscoveryCompensation(sessionId: string): Promise<ProviderDiscoverySessionDto> {
        return this.call(LOREPIA_COMMANDS.continueProviderDiscoveryCompensation, {
            request: { session_id: sessionId },
        });
    }

    resumeProviderDiscoveryCompensation(sessionId: string): Promise<ProviderDiscoverySessionDto> {
        return this.call(LOREPIA_COMMANDS.resumeProviderDiscoveryCompensation, {
            request: { session_id: sessionId },
        });
    }

    pickProviderCatalogImport(): Promise<ProviderCatalogImportTicketDto | null> {
        return this.call(LOREPIA_COMMANDS.pickProviderCatalogImport);
    }

    activateProviderCatalogImport(ticketId: string): Promise<ProviderCatalogImportResultDto> {
        return this.call(LOREPIA_COMMANDS.activateProviderCatalogImport, {
            request: { ticket_id: ticketId },
        });
    }

    discardProviderCatalogImport(ticketId: string): Promise<void> {
        return this.call(LOREPIA_COMMANDS.discardProviderCatalogImport, {
            request: { ticket_id: ticketId },
        });
    }

    providerCatalogStatus(): Promise<ProviderCatalogStatusDto> {
        return this.call(LOREPIA_COMMANDS.providerCatalogStatus);
    }

    providerCatalogHistory(
        limit: number,
        beforeRevision: number | null,
        beforeStateVersion: number | null,
    ): Promise<ProviderCatalogHistoryDto> {
        return this.call(LOREPIA_COMMANDS.providerCatalogHistory, {
            request: {
                limit,
                before_revision: beforeRevision,
                before_state_version: beforeStateVersion,
            },
        });
    }

    diffProviderCatalogRevisions(
        fromRevision: number,
        toRevision: number,
    ): Promise<ProviderCatalogDiffDto> {
        return this.call(LOREPIA_COMMANDS.diffProviderCatalogRevisions, {
            request: { from_revision: fromRevision, to_revision: toRevision },
        });
    }

    prepareProviderCatalogRollback(
        targetRevision: number,
    ): Promise<ProviderCatalogRollbackPlanDto> {
        return this.call(LOREPIA_COMMANDS.prepareProviderCatalogRollback, {
            request: { target_revision: targetRevision },
        });
    }

    activateProviderCatalogRollback(
        plan: ProviderCatalogRollbackPlanDto,
    ): Promise<ProviderCatalogRollbackResultDto> {
        return this.call(LOREPIA_COMMANDS.activateProviderCatalogRollback, {
            request: { plan },
        });
    }
}
