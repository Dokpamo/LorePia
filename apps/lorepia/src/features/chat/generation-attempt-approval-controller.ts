import { get, writable, type Readable } from 'svelte/store';

import type {
    GenerationAttemptProposalDecisionReceiptDto,
    GenerationAttemptProposalListItemDto,
    LorepiaClient,
    RetryableGenerationAttemptDto,
} from '../../lib/ipc/contracts';
import { t } from '../../lib/i18n';
import { normalizeClientError } from '../../lib/ipc/errors';

export const MAX_GENERATION_ATTEMPT_PROPOSALS = 100;
export const MAX_GENERATION_ATTEMPT_PENDING_PROPOSALS = 1_024;
export const MAX_GENERATION_ATTEMPT_RETRY_IDENTITIES = 100;

type GenerationAttemptApprovalApi = Pick<
    LorepiaClient,
    | 'expireGenerationAttemptProposals'
    | 'listGenerationAttemptProposals'
    | 'listRetryableGenerationAttempts'
    | 'decideGenerationAttemptProposal'
>;

export type GenerationAttemptApprovalCapableClient = Partial<GenerationAttemptApprovalApi>;

export interface GenerationAttemptApprovalState {
    phase: 'idle' | 'loading' | 'ready' | 'unavailable' | 'error';
    conversation_id: string | null;
    source_branch_id: string | null;
    proposals: GenerationAttemptProposalListItemDto[];
    busy_proposal_key: string | null;
    has_more_due: boolean;
    retry_generation_ids: string[];
    retry_available: boolean;
    error: string | null;
    announcement: string;
}

export const INITIAL_GENERATION_ATTEMPT_APPROVAL_STATE: GenerationAttemptApprovalState = {
    phase: 'idle',
    conversation_id: null,
    source_branch_id: null,
    proposals: [],
    busy_proposal_key: null,
    has_more_due: false,
    retry_generation_ids: [],
    retry_available: false,
    error: null,
    announcement: '',
};

const MAX_U64 = 18_446_744_073_709_551_615n;
const UTF8_ENCODER = new TextEncoder();

function proposalKey(generationId: string, proposalRecordId: string): string {
    return JSON.stringify([generationId, proposalRecordId]);
}

function isOpaqueId(value: unknown): value is string {
    if (
        typeof value !== 'string' ||
        value.length === 0 ||
        value.trim() !== value ||
        /\p{Cc}/u.test(value) ||
        Array.from(value).length > 256
    ) {
        return false;
    }
    return UTF8_ENCODER.encode(value).byteLength <= 512;
}

function isCanonicalU64(value: unknown): value is string {
    if (typeof value !== 'string' || !/^(0|[1-9][0-9]{0,19})$/.test(value)) return false;
    return BigInt(value) <= MAX_U64;
}

function isCanonicalPositiveU64(value: unknown): value is string {
    return isCanonicalU64(value) && value !== '0';
}

function isSafeEpoch(value: unknown): value is number {
    return Number.isSafeInteger(value) && Number(value) >= 0;
}

function isSha256(value: unknown): value is string {
    return typeof value === 'string' && /^[0-9a-f]{64}$/.test(value);
}

function isRecord(value: unknown): value is Record<string, unknown> {
    return typeof value === 'object' && value !== null && !Array.isArray(value);
}

function exactKeys(value: Record<string, unknown>, expected: readonly string[]): boolean {
    const keys = Object.keys(value);
    const expectedKeys = new Set(expected);
    return keys.length === expectedKeys.size && keys.every((key) => expectedKeys.has(key));
}

function parseUtcTimestamp(value: unknown): number | null {
    if (
        typeof value !== 'string' ||
        !/^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}(?:\.\d{1,9})?Z$/.test(value)
    ) {
        return null;
    }
    const epochMilliseconds = Date.parse(value);
    if (!Number.isFinite(epochMilliseconds)) return null;
    const canonicalSecond = `${new Date(epochMilliseconds).toISOString().slice(0, 19)}Z`;
    return canonicalSecond === `${value.slice(0, 19)}Z` ? epochMilliseconds : null;
}

function isValidRetryableAttemptList(value: unknown): value is RetryableGenerationAttemptDto[] {
    if (!Array.isArray(value) || value.length > MAX_GENERATION_ATTEMPT_RETRY_IDENTITIES) {
        return false;
    }
    const seen = new Set<string>();
    for (const item of value) {
        if (
            !isRecord(item) ||
            !exactKeys(item, ['generation_id', 'status', 'created_at', 'updated_at']) ||
            !isOpaqueId(item.generation_id) ||
            (item.status !== 'before_generation_applied' && item.status !== 'dispatch_ready')
        ) {
            return false;
        }
        const createdAt = parseUtcTimestamp(item.created_at);
        const updatedAt = parseUtcTimestamp(item.updated_at);
        if (createdAt === null || updatedAt === null || updatedAt < createdAt) return false;
        if (seen.has(item.generation_id)) return false;
        seen.add(item.generation_id);
    }
    return true;
}

function isValidProposal(
    item: GenerationAttemptProposalListItemDto | GenerationAttemptProposalDecisionReceiptDto,
    expectedStatus: 'pending' | 'approved' | 'rejected' | 'expired',
): boolean {
    const { proposal } = item;
    if (
        !isOpaqueId(proposal.id) ||
        typeof proposal.title !== 'string' ||
        typeof proposal.body !== 'string' ||
        !isSafeProposalProjectionReason(proposal.projection_rejection_reason) ||
        proposal.status !== expectedStatus ||
        !isCanonicalU64(proposal.source_interaction_state_revision) ||
        !isSafeEpoch(proposal.requested_at_epoch_seconds) ||
        (proposal.expires_at_epoch_seconds !== null &&
            !isSafeEpoch(proposal.expires_at_epoch_seconds)) ||
        (proposal.decided_at_epoch_seconds !== null &&
            !isSafeEpoch(proposal.decided_at_epoch_seconds))
    ) {
        return false;
    }
    if (
        proposal.expires_at_epoch_seconds !== null &&
        proposal.expires_at_epoch_seconds < proposal.requested_at_epoch_seconds
    ) {
        return false;
    }
    if (expectedStatus === 'pending') return proposal.decided_at_epoch_seconds === null;
    if (expectedStatus === 'expired') {
        return (
            proposal.expires_at_epoch_seconds !== null &&
            proposal.decided_at_epoch_seconds !== null &&
            proposal.decided_at_epoch_seconds >= proposal.expires_at_epoch_seconds
        );
    }
    return (
        proposal.decided_at_epoch_seconds !== null &&
        proposal.decided_at_epoch_seconds >= proposal.requested_at_epoch_seconds
    );
}

function isSafeProposalProjectionReason(value: unknown): value is undefined | 'unsafe_native_text' {
    return value === undefined || value === 'unsafe_native_text';
}

function hasValidAttemptAuthority(
    item: GenerationAttemptProposalListItemDto | GenerationAttemptProposalDecisionReceiptDto,
    conversationId: string,
    sourceBranchId: string,
): boolean {
    return (
        item.conversation_id === conversationId &&
        item.source_branch_id === sourceBranchId &&
        isOpaqueId(item.proposed_branch_id) &&
        isOpaqueId(item.generation_id) &&
        isCanonicalPositiveU64(item.aggregate_revision) &&
        isCanonicalPositiveU64(item.interaction_state_revision) &&
        isCanonicalPositiveU64(item.proposal_revision) &&
        Number.isSafeInteger(item.pending_proposal_count) &&
        item.pending_proposal_count >= 0 &&
        item.pending_proposal_count <= MAX_GENERATION_ATTEMPT_PENDING_PROPOSALS &&
        isCanonicalU64(item.proposal.source_interaction_state_revision) &&
        BigInt(item.proposal.source_interaction_state_revision) <=
            BigInt(item.interaction_state_revision)
    );
}

function isValidList(
    items: readonly GenerationAttemptProposalListItemDto[],
    conversationId: string,
    sourceBranchId: string,
): boolean {
    if (items.length > MAX_GENERATION_ATTEMPT_PROPOSALS) return false;
    const seen = new Set<string>();
    const attemptAuthority = new Map<
        string,
        {
            proposed_branch_id: string;
            aggregate_revision: string;
            interaction_state_revision: string;
            pending_proposal_count: number;
            visible_count: number;
        }
    >();
    for (const item of items) {
        const key = proposalKey(item.generation_id, item.proposal.id);
        if (
            seen.has(key) ||
            !hasValidAttemptAuthority(item, conversationId, sourceBranchId) ||
            item.pending_proposal_count < 1 ||
            !isValidProposal(item, 'pending')
        ) {
            return false;
        }
        seen.add(key);
        const existing = attemptAuthority.get(item.generation_id);
        if (existing === undefined) {
            attemptAuthority.set(item.generation_id, {
                proposed_branch_id: item.proposed_branch_id,
                aggregate_revision: item.aggregate_revision,
                interaction_state_revision: item.interaction_state_revision,
                pending_proposal_count: item.pending_proposal_count,
                visible_count: 1,
            });
        } else {
            if (
                existing.proposed_branch_id !== item.proposed_branch_id ||
                existing.aggregate_revision !== item.aggregate_revision ||
                existing.interaction_state_revision !== item.interaction_state_revision ||
                existing.pending_proposal_count !== item.pending_proposal_count
            ) {
                return false;
            }
            existing.visible_count += 1;
        }
    }
    return [...attemptAuthority.values()].every(
        ({ visible_count, pending_proposal_count }) => visible_count <= pending_proposal_count,
    );
}

function hasValidDecisionEvidence(receipt: GenerationAttemptProposalDecisionReceiptDto): boolean {
    if (typeof receipt.exact_replay !== 'boolean') return false;
    if (receipt.pending_proposal_count === 0) {
        return isSha256(receipt.approval_evidence_sha256);
    }
    return receipt.approval_evidence_sha256 === null;
}

function sameImmutableProposal(
    receipt: GenerationAttemptProposalDecisionReceiptDto,
    target: GenerationAttemptProposalListItemDto,
): boolean {
    return (
        receipt.proposal.id === target.proposal.id &&
        receipt.proposal.title === target.proposal.title &&
        receipt.proposal.body === target.proposal.body &&
        receipt.proposal.projection_rejection_reason ===
            target.proposal.projection_rejection_reason &&
        receipt.proposal.source_interaction_state_revision ===
            target.proposal.source_interaction_state_revision &&
        receipt.proposal.requested_at_epoch_seconds ===
            target.proposal.requested_at_epoch_seconds &&
        receipt.proposal.expires_at_epoch_seconds === target.proposal.expires_at_epoch_seconds
    );
}

function isExactDecisionReceipt(
    receipt: GenerationAttemptProposalDecisionReceiptDto,
    target: GenerationAttemptProposalListItemDto,
    expectedStatus: 'approved' | 'rejected',
): boolean {
    if (
        !hasValidAttemptAuthority(receipt, target.conversation_id, target.source_branch_id) ||
        receipt.proposed_branch_id !== target.proposed_branch_id ||
        receipt.generation_id !== target.generation_id ||
        !isValidProposal(receipt, expectedStatus) ||
        !sameImmutableProposal(receipt, target) ||
        !hasValidDecisionEvidence(receipt)
    ) {
        return false;
    }
    const expectedAggregateRevision = BigInt(target.aggregate_revision) + 1n;
    const expectedProposalRevision = BigInt(target.proposal_revision) + 1n;
    const maximumPending = target.pending_proposal_count - 1;
    return (
        maximumPending >= 0 &&
        BigInt(receipt.proposal_revision) === expectedProposalRevision &&
        (receipt.exact_replay
            ? BigInt(receipt.aggregate_revision) >= expectedAggregateRevision &&
              receipt.pending_proposal_count <= maximumPending
            : BigInt(receipt.aggregate_revision) === expectedAggregateRevision &&
              receipt.pending_proposal_count === maximumPending) &&
        BigInt(receipt.interaction_state_revision) >= BigInt(target.interaction_state_revision) &&
        receipt.pending_proposal_count >= 0
    );
}

function isValidExpiryReceipt(
    receipt: GenerationAttemptProposalDecisionReceiptDto,
    conversationId: string,
    sourceBranchId: string,
): boolean {
    return (
        hasValidAttemptAuthority(receipt, conversationId, sourceBranchId) &&
        isValidProposal(receipt, 'expired') &&
        hasValidDecisionEvidence(receipt)
    );
}

function mergeRetryGenerationIds(
    retained: readonly string[],
    additions: readonly string[],
): string[] {
    const generationIds = new Set(retained);
    for (const generationId of additions) generationIds.add(generationId);
    if (generationIds.size > MAX_GENERATION_ATTEMPT_RETRY_IDENTITIES) {
        throw new Error('Too many terminal generation attempts were returned for exact retry.');
    }
    return [...generationIds];
}

function errorLabel(error: unknown): string {
    const normalized = normalizeClientError(error);
    return normalized.messageKey === 'error.unexpected'
        ? t('attempt_approval.error.generic')
        : normalized.messageKey;
}

function hasApprovalApi(
    client: GenerationAttemptApprovalCapableClient,
): client is GenerationAttemptApprovalApi {
    return (
        client.expireGenerationAttemptProposals !== undefined &&
        client.listGenerationAttemptProposals !== undefined &&
        client.listRetryableGenerationAttempts !== undefined &&
        client.decideGenerationAttemptProposal !== undefined
    );
}

export class GenerationAttemptApprovalController {
    private readonly mutable = writable<GenerationAttemptApprovalState>(
        structuredClone(INITIAL_GENERATION_ATTEMPT_APPROVAL_STATE),
    );
    readonly state: Readable<GenerationAttemptApprovalState> = this.mutable;

    private operationEpoch = 0;
    private destroyed = false;

    constructor(private readonly client: GenerationAttemptApprovalCapableClient) {
        if (!hasApprovalApi(client)) {
            const message = t('attempt_approval.error.unsupported');
            this.mutable.set({
                ...structuredClone(INITIAL_GENERATION_ATTEMPT_APPROVAL_STATE),
                phase: 'unavailable',
                error: message,
                announcement: message,
            });
        }
    }

    async loadRoom(conversationId: string | null, sourceBranchId: string | null): Promise<boolean> {
        if (this.destroyed) return false;
        if (conversationId === null || sourceBranchId === null) {
            ++this.operationEpoch;
            const unavailable = get(this.mutable).phase === 'unavailable';
            this.mutable.set({
                ...structuredClone(INITIAL_GENERATION_ATTEMPT_APPROVAL_STATE),
                ...(unavailable
                    ? {
                          phase: 'unavailable' as const,
                          error: t('attempt_approval.error.unsupported'),
                      }
                    : {}),
            });
            return true;
        }
        if (!hasApprovalApi(this.client)) return false;

        const epoch = ++this.operationEpoch;
        this.mutable.set({
            ...structuredClone(INITIAL_GENERATION_ATTEMPT_APPROVAL_STATE),
            phase: 'loading',
            conversation_id: conversationId,
            source_branch_id: sourceBranchId,
        });
        try {
            const expiry = await this.client.expireGenerationAttemptProposals({
                conversation_id: conversationId,
                source_branch_id: sourceBranchId,
                limit: MAX_GENERATION_ATTEMPT_PROPOSALS,
            });
            if (epoch !== this.operationEpoch) return false;
            if (
                expiry.conversation_id !== conversationId ||
                expiry.source_branch_id !== sourceBranchId ||
                typeof expiry.has_more_due !== 'boolean' ||
                expiry.decisions.length > MAX_GENERATION_ATTEMPT_PROPOSALS ||
                expiry.decisions.some(
                    (receipt) => !isValidExpiryReceipt(receipt, conversationId, sourceBranchId),
                ) ||
                new Set(
                    expiry.decisions.map((receipt) =>
                        proposalKey(receipt.generation_id, receipt.proposal.id),
                    ),
                ).size !== expiry.decisions.length
            ) {
                throw new Error('Core attempt expiry authority did not match the current room.');
            }

            const proposals = await this.client.listGenerationAttemptProposals({
                conversation_id: conversationId,
                source_branch_id: sourceBranchId,
                status: 'pending',
                limit: MAX_GENERATION_ATTEMPT_PROPOSALS,
            });
            if (epoch !== this.operationEpoch) return false;
            if (!isValidList(proposals, conversationId, sourceBranchId)) {
                throw new Error('Core attempt proposal authority did not match the current room.');
            }
            const retryableAttempts = await this.client.listRetryableGenerationAttempts({
                conversation_id: conversationId,
                source_branch_id: sourceBranchId,
                limit: MAX_GENERATION_ATTEMPT_RETRY_IDENTITIES,
            });
            if (epoch !== this.operationEpoch) return false;
            if (!isValidRetryableAttemptList(retryableAttempts)) {
                throw new Error('Core retryable attempt authority did not match the current room.');
            }
            const retryGenerationIds = mergeRetryGenerationIds(
                retryableAttempts.map((attempt) => attempt.generation_id),
                expiry.decisions
                    .filter((receipt) => receipt.pending_proposal_count === 0)
                    .map((receipt) => receipt.generation_id),
            );
            const retryAvailable = !expiry.has_more_due && retryGenerationIds.length > 0;
            this.mutable.set({
                phase: 'ready',
                conversation_id: conversationId,
                source_branch_id: sourceBranchId,
                proposals: [...proposals],
                busy_proposal_key: null,
                has_more_due: expiry.has_more_due,
                retry_generation_ids: retryGenerationIds,
                retry_available: retryAvailable,
                error: null,
                announcement:
                    expiry.decisions.length > 0
                        ? expiry.has_more_due
                            ? t('attempt_approval.notice.expired_more')
                            : expiry.decisions.some(
                                    (receipt) => receipt.pending_proposal_count === 0,
                                )
                              ? t('attempt_approval.notice.expired_retry')
                              : t('attempt_approval.notice.expired_review')
                        : retryAvailable
                          ? t('attempt_approval.notice.resumable')
                          : proposals.length > 0
                            ? t('attempt_approval.notice.restored')
                            : '',
            });
            return true;
        } catch (error: unknown) {
            if (epoch !== this.operationEpoch) return false;
            const message = errorLabel(error);
            this.mutable.set({
                ...structuredClone(INITIAL_GENERATION_ATTEMPT_APPROVAL_STATE),
                phase: 'error',
                conversation_id: conversationId,
                source_branch_id: sourceBranchId,
                error: message,
                announcement: message,
            });
            return false;
        }
    }

    async reload(): Promise<boolean> {
        const state = get(this.mutable);
        return this.loadRoom(state.conversation_id, state.source_branch_id);
    }

    async decideProposal(
        generationId: string,
        proposalRecordId: string,
        decision: 'approve' | 'reject',
    ): Promise<boolean> {
        if (this.destroyed || !hasApprovalApi(this.client)) return false;
        const state = get(this.mutable);
        const target = state.proposals.find(
            (item) => item.generation_id === generationId && item.proposal.id === proposalRecordId,
        );
        if (
            state.phase !== 'ready' ||
            state.conversation_id === null ||
            state.source_branch_id === null ||
            state.busy_proposal_key !== null ||
            target === undefined
        ) {
            return false;
        }
        if (
            decision === 'approve' &&
            target.proposal.projection_rejection_reason === 'unsafe_native_text'
        ) {
            this.mutable.set({
                ...state,
                announcement: t('interaction.error.unreviewable'),
            });
            return false;
        }

        const busyKey = proposalKey(generationId, proposalRecordId);
        const epoch = ++this.operationEpoch;
        this.mutable.set({
            ...state,
            busy_proposal_key: busyKey,
            error: null,
            announcement:
                decision === 'approve'
                    ? t('attempt_approval.notice.approving')
                    : t('attempt_approval.notice.rejecting'),
        });
        try {
            const receipt = await this.client.decideGenerationAttemptProposal({
                conversation_id: target.conversation_id,
                source_branch_id: target.source_branch_id,
                generation_id: target.generation_id,
                proposal_record_id: target.proposal.id,
                expected_aggregate_revision: target.aggregate_revision,
                expected_proposal_revision: target.proposal_revision,
                decision,
            });
            if (epoch !== this.operationEpoch) return false;
            const expectedStatus = decision === 'approve' ? 'approved' : 'rejected';
            if (!isExactDecisionReceipt(receipt, target, expectedStatus)) {
                throw new Error(
                    'Core attempt decision receipt did not match the reviewed proposal.',
                );
            }
            const replayPrefix = receipt.exact_replay
                ? t('attempt_approval.notice.replay_prefix')
                : '';
            const actionLabel =
                decision === 'approve'
                    ? t('interaction.notice.approved')
                    : t('interaction.notice.rejected');
            if (receipt.exact_replay) {
                const restored = await this.loadRoom(
                    target.conversation_id,
                    target.source_branch_id,
                );
                if (!restored) return false;
                const refreshed = get(this.mutable);
                if (
                    refreshed.conversation_id !== target.conversation_id ||
                    refreshed.source_branch_id !== target.source_branch_id
                ) {
                    return false;
                }
                const retryGenerationIds = mergeRetryGenerationIds(
                    refreshed.retry_generation_ids,
                    receipt.pending_proposal_count === 0 ? [receipt.generation_id] : [],
                );
                const retryAvailable = !refreshed.has_more_due && retryGenerationIds.length > 0;
                this.mutable.set({
                    ...refreshed,
                    retry_generation_ids: retryGenerationIds,
                    retry_available: retryAvailable,
                    announcement: refreshed.has_more_due
                        ? t('attempt_approval.notice.after.more', {
                              prefix: replayPrefix,
                              action: actionLabel,
                          })
                        : receipt.pending_proposal_count === 0
                          ? t('attempt_approval.notice.after.retry', {
                                prefix: replayPrefix,
                                action: actionLabel,
                            })
                          : t('attempt_approval.notice.after.review', {
                                prefix: replayPrefix,
                                action: actionLabel,
                            }),
                });
                return true;
            }
            const current = get(this.mutable);
            if (
                current.conversation_id !== target.conversation_id ||
                current.source_branch_id !== target.source_branch_id
            ) {
                return false;
            }
            const remaining = current.proposals
                .filter((item) => proposalKey(item.generation_id, item.proposal.id) !== busyKey)
                .map((item) =>
                    item.generation_id === target.generation_id
                        ? {
                              ...item,
                              aggregate_revision: receipt.aggregate_revision,
                              interaction_state_revision: receipt.interaction_state_revision,
                              pending_proposal_count: receipt.pending_proposal_count,
                          }
                        : item,
                );
            const retryGenerationIds = mergeRetryGenerationIds(
                current.retry_generation_ids,
                receipt.pending_proposal_count === 0 ? [receipt.generation_id] : [],
            );
            this.mutable.set({
                ...current,
                phase: 'ready',
                proposals: remaining,
                busy_proposal_key: null,
                retry_generation_ids: retryGenerationIds,
                retry_available: !current.has_more_due && retryGenerationIds.length > 0,
                error: null,
                announcement:
                    receipt.pending_proposal_count === 0
                        ? t('attempt_approval.notice.local.retry', {
                              prefix: replayPrefix,
                              action: actionLabel,
                          })
                        : t('attempt_approval.notice.local.review', {
                              prefix: replayPrefix,
                              action: actionLabel,
                          }),
            });
            return true;
        } catch (error: unknown) {
            if (epoch !== this.operationEpoch) return false;
            const message = errorLabel(error);
            this.mutable.update((current) => ({
                ...current,
                phase: 'ready',
                busy_proposal_key: null,
                error: message,
                announcement: t('attempt_approval.notice.reload', { message }),
            }));
            return false;
        }
    }

    destroy(): void {
        this.destroyed = true;
        ++this.operationEpoch;
    }
}
