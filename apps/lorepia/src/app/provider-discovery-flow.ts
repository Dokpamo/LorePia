import type {
    LorepiaClient,
    ProviderDiscoveryEventDto,
    ProviderDiscoverySessionDto,
    ProviderWorkspaceDto,
} from '../lib/ipc/contracts';
import { credentialKey, discoveryCredentialTarget } from './provider-credential';

const MAX_PROVIDER_DISCOVERY_EVENT_DRAIN = 100;

export interface ProviderDiscoverySnapshot {
    session: ProviderDiscoverySessionDto;
    candidates: ProviderWorkspaceDto['discovery_candidates'];
    evidence: ProviderWorkspaceDto['discovery_evidence'];
    approvals: ProviderWorkspaceDto['discovery_approvals'];
    review: ProviderWorkspaceDto['discovery_review'];
    approvalProposal: ProviderWorkspaceDto['discovery_approval_proposal'];
    reviewProposal: ProviderWorkspaceDto['discovery_review_proposal'];
    assistantResumeBoundary: ProviderWorkspaceDto['discovery_assistant_resume_boundary'];
    compensationSteps: ProviderWorkspaceDto['discovery_compensation_steps'];
    credentialTarget: ReturnType<typeof discoveryCredentialTarget>;
    credentialStatus: ProviderWorkspaceDto['credential_statuses'][string] | null;
}

export async function loadProviderDiscoverySnapshot(
    client: LorepiaClient,
    sessionId: string,
    isCurrent: () => boolean,
): Promise<ProviderDiscoverySnapshot | null> {
    const [
        session,
        candidates,
        evidence,
        approvals,
        review,
        approvalProposal,
        reviewProposal,
        assistantResumeBoundary,
    ] = await Promise.all([
        client.getProviderDiscovery(sessionId),
        client.listProviderDiscoveryCandidates(sessionId),
        client.listProviderDiscoveryEvidence(sessionId),
        client.listProviderDiscoveryApprovals(sessionId),
        client.getProviderDiscoveryReview(sessionId),
        client.getProviderDiscoveryApprovalProposal(sessionId),
        client.getProviderDiscoveryReviewProposal(sessionId),
        client.getProviderDiscoveryAssistantResumeBoundary(sessionId),
    ]);
    if (session.id !== sessionId || !isCurrent()) return null;

    const compensationSteps =
        session.commit_attempt_id === null
            ? []
            : await client.listProviderDiscoveryCompensationSteps(session.commit_attempt_id);
    const credentialTarget = discoveryCredentialTarget(session);
    const credentialStatus =
        credentialTarget === null ? null : (await client.credentialStatus(credentialTarget)).status;
    if (!isCurrent()) return null;

    return {
        session,
        candidates,
        evidence,
        approvals,
        review,
        approvalProposal,
        reviewProposal,
        assistantResumeBoundary,
        compensationSteps,
        credentialTarget,
        credentialStatus,
    };
}

export function mergeProviderDiscoverySnapshot(
    workspace: ProviderWorkspaceDto,
    snapshot: ProviderDiscoverySnapshot,
): ProviderWorkspaceDto {
    const sessionCredentialKey = `discovery_session:${snapshot.session.id}`;
    const credentialStatuses = Object.fromEntries(
        Object.entries(workspace.credential_statuses).filter(
            ([key]) => key !== sessionCredentialKey,
        ),
    );
    if (snapshot.credentialTarget !== null && snapshot.credentialStatus !== null) {
        credentialStatuses[credentialKey(snapshot.credentialTarget)] = snapshot.credentialStatus;
    }
    return {
        ...workspace,
        credential_statuses: credentialStatuses,
        discoveries: [
            snapshot.session,
            ...workspace.discoveries.filter((candidate) => candidate.id !== snapshot.session.id),
        ],
        selected_discovery_id: snapshot.session.id,
        discovery_candidates: snapshot.candidates,
        discovery_evidence: snapshot.evidence,
        discovery_approvals: snapshot.approvals,
        discovery_review: snapshot.review,
        discovery_approval_proposal: snapshot.approvalProposal,
        discovery_review_proposal: snapshot.reviewProposal,
        discovery_assistant_resume_boundary: snapshot.assistantResumeBoundary,
        discovery_compensation_steps: snapshot.compensationSteps,
    };
}

export function storeProviderDiscoverySession(
    workspace: ProviderWorkspaceDto,
    session: ProviderDiscoverySessionDto,
): ProviderWorkspaceDto {
    return {
        ...workspace,
        discoveries: [
            session,
            ...workspace.discoveries.filter((candidate) => candidate.id !== session.id),
        ],
        selected_discovery_id: session.id,
    };
}

export interface ProviderDiscoveryEventDrain {
    latest: ProviderDiscoveryEventDto | null;
    drained: boolean;
}

export async function drainProviderDiscoveryEvents(
    client: LorepiaClient,
    sessionId: string,
    isCurrent: () => boolean,
): Promise<ProviderDiscoveryEventDrain | null> {
    let latest: ProviderDiscoveryEventDto | null = null;
    let acknowledgedCount = 0;
    let drained = false;
    while (acknowledgedCount < MAX_PROVIDER_DISCOVERY_EVENT_DRAIN) {
        if (!isCurrent()) return null;
        const remaining = MAX_PROVIDER_DISCOVERY_EVENT_DRAIN - acknowledgedCount;
        const events = await client.pollProviderDiscoveryEventsForSession(sessionId, remaining);
        if (!isCurrent()) return null;
        if (events.some((item) => item.event.session_id !== sessionId)) {
            throw new Error('session-filtered discovery poll returned a foreign event');
        }
        if (events.length > remaining) {
            throw new Error('session-filtered discovery poll exceeded its requested limit');
        }
        if (events.length === 0) {
            drained = true;
            break;
        }
        for (const item of events) {
            if (!isCurrent()) return null;
            const acknowledged = await client.ackProviderDiscoveryEvent(item.event.id);
            if (!isCurrent()) return null;
            if (!acknowledged) {
                throw new Error('provider discovery event acknowledgement was rejected');
            }
            latest = item.event;
            acknowledgedCount += 1;
        }
    }
    return { latest, drained };
}
