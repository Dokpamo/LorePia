import { get, writable, type Readable } from 'svelte/store';

import type {
    InteractionChoiceStatusDto,
    InteractionEffectDto,
    InteractionEffectEventDto,
    InteractionEffectHistoryItemDto,
    InteractionProposalListItemDto,
    LorepiaClient,
    RoomInteractionClientApi,
} from '../../lib/ipc/contracts';
import { t } from '../../lib/i18n';
import { normalizeClientError } from '../../lib/ipc/errors';

export type InteractionRoomCapableClient = LorepiaClient & Partial<RoomInteractionClientApi>;

export interface RoomInteractionEffect {
    effect_id: string;
    conversation_id: string;
    branch_id: string;
    resulting_state_revision: number;
    event_created_at: string;
    choice_status: InteractionChoiceStatusDto | null;
    selected_choice_id: string | null;
    effect: InteractionEffectDto;
}

export interface InteractionRoomState {
    phase: 'idle' | 'loading' | 'ready' | 'unavailable' | 'error';
    conversation_id: string | null;
    branch_id: string | null;
    current_state_revision: number;
    effects: RoomInteractionEffect[];
    pending_proposals: InteractionProposalListItemDto[];
    has_more_expired_proposals: boolean;
    has_older_effects: boolean;
    busy_effect_id: string | null;
    busy_proposal_id: string | null;
    error: string | null;
    announcement: string;
}

export const INITIAL_INTERACTION_ROOM_STATE: InteractionRoomState = {
    phase: 'idle',
    conversation_id: null,
    branch_id: null,
    current_state_revision: 0,
    effects: [],
    pending_proposals: [],
    has_more_expired_proposals: false,
    has_older_effects: false,
    busy_effect_id: null,
    busy_proposal_id: null,
    error: null,
    announcement: '',
};

function errorLabel(error: unknown): string {
    const normalized = normalizeClientError(error);
    return normalized.messageKey === 'error.unexpected'
        ? t('interaction.error.load')
        : normalized.messageKey;
}

function isSafeRevision(value: unknown): value is number {
    return Number.isSafeInteger(value) && Number(value) >= 0;
}

function isSafeProposalProjectionReason(value: unknown): value is undefined | 'unsafe_native_text' {
    return value === undefined || value === 'unsafe_native_text';
}

function hasValidProposalRecord(
    item: InteractionProposalListItemDto,
    expectedStatus: 'pending' | 'expired',
): boolean {
    const { proposal } = item;
    return (
        typeof proposal.id === 'string' &&
        proposal.id.length > 0 &&
        proposal.id.length <= 512 &&
        typeof proposal.title === 'string' &&
        typeof proposal.body === 'string' &&
        isSafeProposalProjectionReason(proposal.projection_rejection_reason) &&
        proposal.status === expectedStatus &&
        isSafeRevision(item.state_revision) &&
        isSafeRevision(item.proposal_revision) &&
        isSafeRevision(proposal.source_interaction_state_revision) &&
        proposal.source_interaction_state_revision <= item.state_revision &&
        isSafeRevision(proposal.requested_at_epoch_seconds) &&
        (proposal.expires_at_epoch_seconds === null ||
            (isSafeRevision(proposal.expires_at_epoch_seconds) &&
                proposal.expires_at_epoch_seconds >= proposal.requested_at_epoch_seconds)) &&
        (expectedStatus === 'pending'
            ? proposal.decided_at_epoch_seconds === null
            : proposal.expires_at_epoch_seconds !== null &&
              isSafeRevision(proposal.decided_at_epoch_seconds) &&
              proposal.decided_at_epoch_seconds >= proposal.expires_at_epoch_seconds)
    );
}

function fromHistory(item: InteractionEffectHistoryItemDto): RoomInteractionEffect {
    return {
        effect_id: item.effect_id,
        conversation_id: item.conversation_id,
        branch_id: item.branch_id,
        resulting_state_revision: item.resulting_state_revision,
        event_created_at: item.event_created_at,
        choice_status: item.choice_status,
        selected_choice_id: item.selected_choice_id,
        effect: item.effect,
    };
}

function fromDelivery(delivery: InteractionEffectEventDto): RoomInteractionEffect {
    return {
        effect_id: delivery.effect_id,
        conversation_id: delivery.conversation_id,
        branch_id: delivery.branch_id,
        resulting_state_revision: delivery.resulting_state_revision,
        event_created_at: delivery.event_created_at,
        choice_status: delivery.effect.kind === 'present_choices' ? 'pending' : null,
        selected_choice_id: null,
        effect: delivery.effect,
    };
}

function boundedMergedEffects(
    existing: readonly RoomInteractionEffect[],
    history: readonly InteractionEffectHistoryItemDto[],
): RoomInteractionEffect[] {
    const byId = new Map(existing.map((effect) => [effect.effect_id, effect]));
    for (const item of history) byId.set(item.effect_id, fromHistory(item));
    return [...byId.values()]
        .sort(
            (left, right) =>
                left.resulting_state_revision - right.resulting_state_revision ||
                left.event_created_at.localeCompare(right.event_created_at) ||
                left.effect_id.localeCompare(right.effect_id),
        )
        .slice(-100);
}

export class InteractionRoomController {
    private readonly mutable = writable<InteractionRoomState>(
        structuredClone(INITIAL_INTERACTION_ROOM_STATE),
    );
    readonly state: Readable<InteractionRoomState> = this.mutable;

    private operationEpoch = 0;
    private unlisten: (() => void) | null = null;
    private subscription: Promise<void> | null = null;
    private destroyed = false;

    constructor(private readonly client: InteractionRoomCapableClient) {
        if (
            client.expireInteractionProposals === undefined ||
            client.listInteractionProposals === undefined ||
            client.listReopenInteractionEffects === undefined ||
            client.submitInteractionChoice === undefined
        ) {
            const message = t('interaction.error.unsupported');
            this.mutable.set({
                ...structuredClone(INITIAL_INTERACTION_ROOM_STATE),
                phase: 'unavailable',
                error: message,
                announcement: message,
            });
        }
    }

    private async ensureSubscription(): Promise<void> {
        if (this.unlisten !== null) return;
        if (this.subscription !== null) return this.subscription;
        this.subscription = this.client
            .subscribeInteractionEffects((delivery) => this.receiveDelivery(delivery))
            .then((unlisten) => {
                if (this.destroyed) {
                    unlisten();
                } else {
                    this.unlisten = unlisten;
                }
            })
            .finally(() => {
                this.subscription = null;
            });
        return this.subscription;
    }

    private receiveDelivery(delivery: InteractionEffectEventDto): void {
        if (this.destroyed) return;
        const state = get(this.mutable);
        if (
            state.conversation_id !== delivery.conversation_id ||
            state.branch_id !== delivery.branch_id
        ) {
            return;
        }
        const effects = boundedMergedEffects([...state.effects, fromDelivery(delivery)], []);
        this.mutable.set({
            ...state,
            phase: state.phase === 'loading' ? 'loading' : 'ready',
            current_state_revision: Math.max(
                state.current_state_revision,
                delivery.resulting_state_revision,
            ),
            effects,
            announcement:
                delivery.effect.kind === 'projection_rejected'
                    ? t('interaction.notice.hidden')
                    : t('interaction.notice.shown'),
        });
        void this.acknowledgeDelivery(delivery);
    }

    private async acknowledgeDelivery(delivery: InteractionEffectEventDto): Promise<void> {
        try {
            await this.client.acknowledgeInteractionEffect(delivery.delivery_id);
        } catch {
            try {
                await this.client.retryInteractionEffect(delivery.delivery_id);
            } catch {
                // The durable lease is still owned by Core and will be recovered.
            }
            if (this.destroyed) return;
            const state = get(this.mutable);
            if (
                state.conversation_id === delivery.conversation_id &&
                state.branch_id === delivery.branch_id
            ) {
                this.mutable.set({
                    ...state,
                    error: t('interaction.error.ack_failed'),
                });
            }
        }
    }

    async loadRoom(conversationId: string | null, branchId: string | null): Promise<boolean> {
        if (this.destroyed) return false;
        if (conversationId === null || branchId === null) {
            ++this.operationEpoch;
            const unavailable = get(this.mutable).phase === 'unavailable';
            this.mutable.set({
                ...structuredClone(INITIAL_INTERACTION_ROOM_STATE),
                ...(unavailable
                    ? {
                          phase: 'unavailable' as const,
                          error: t('interaction.error.unsupported'),
                      }
                    : {}),
            });
            return true;
        }
        if (
            this.client.expireInteractionProposals === undefined ||
            this.client.listReopenInteractionEffects === undefined ||
            this.client.listInteractionProposals === undefined
        ) {
            return false;
        }
        const epoch = ++this.operationEpoch;
        this.mutable.set({
            ...structuredClone(INITIAL_INTERACTION_ROOM_STATE),
            phase: 'loading',
            conversation_id: conversationId,
            branch_id: branchId,
        });
        try {
            await this.ensureSubscription();
            const expiry = await this.client.expireInteractionProposals({
                conversation_id: conversationId,
                branch_id: branchId,
                limit: 100,
            });
            if (
                expiry.conversation_id !== conversationId ||
                expiry.branch_id !== branchId ||
                typeof expiry.has_more_expired !== 'boolean' ||
                !Number.isSafeInteger(expiry.current_state_revision) ||
                expiry.current_state_revision < 0 ||
                expiry.expired_proposals.length > 100 ||
                expiry.expired_proposals.some(
                    (item) =>
                        item.conversation_id !== conversationId ||
                        item.branch_id !== branchId ||
                        !hasValidProposalRecord(item, 'expired'),
                )
            ) {
                throw new Error('Core proposal expiry receipt did not match the current room.');
            }
            const [claimed, snapshot, proposals] = await Promise.all([
                this.client.listInteractionEffects(),
                this.client.listReopenInteractionEffects({
                    conversation_id: conversationId,
                    branch_id: branchId,
                    limit: 100,
                }),
                this.client.listInteractionProposals({
                    conversation_id: conversationId,
                    branch_id: branchId,
                    status: 'pending',
                    limit: 100,
                }),
            ]);
            if (epoch !== this.operationEpoch) return false;
            for (const delivery of claimed) this.receiveDelivery(delivery);
            const current = get(this.mutable);
            if (current.conversation_id !== conversationId || current.branch_id !== branchId) {
                return false;
            }
            const roomProposals = proposals
                .filter(
                    (item) =>
                        item.conversation_id === conversationId &&
                        item.branch_id === branchId &&
                        item.proposal.status === 'pending',
                )
                .slice(0, 100);
            if (
                proposals.length > 100 ||
                roomProposals.length !== proposals.length ||
                new Set(roomProposals.map((item) => item.proposal.id)).size !==
                    roomProposals.length ||
                roomProposals.some(
                    (item) =>
                        item.state_revision !== expiry.current_state_revision ||
                        !hasValidProposalRecord(item, 'pending'),
                )
            ) {
                throw new Error('Core pending proposal authority did not match the current room.');
            }
            this.mutable.set({
                ...current,
                phase: 'ready',
                current_state_revision: Math.max(
                    current.current_state_revision,
                    expiry.current_state_revision,
                    snapshot.current_state_revision,
                ),
                effects: boundedMergedEffects(current.effects, snapshot.items),
                pending_proposals: roomProposals,
                has_more_expired_proposals: expiry.has_more_expired,
                has_older_effects: snapshot.older_cursor !== null,
                error: null,
                announcement:
                    expiry.expired_proposals.length > 0 || expiry.has_more_expired
                        ? t('interaction.notice.expired_cleared')
                        : snapshot.items.length > 0 || roomProposals.length > 0
                          ? t('interaction.notice.restored')
                          : '',
            });
            return true;
        } catch (error: unknown) {
            if (epoch !== this.operationEpoch) return false;
            const message = errorLabel(error);
            this.mutable.update((state) => ({
                ...state,
                phase: 'error',
                error: message,
                announcement: message,
            }));
            return false;
        }
    }

    async reload(): Promise<boolean> {
        const state = get(this.mutable);
        return this.loadRoom(state.conversation_id, state.branch_id);
    }

    async submitChoice(effectId: string, choiceId: string): Promise<boolean> {
        if (this.client.submitInteractionChoice === undefined) return false;
        const state = get(this.mutable);
        const target = state.effects.find(({ effect_id }) => effect_id === effectId);
        if (
            state.phase !== 'ready' ||
            state.conversation_id === null ||
            state.branch_id === null ||
            target?.effect.kind !== 'present_choices' ||
            target.choice_status !== 'pending' ||
            !target.effect.choices.some(({ id }) => id === choiceId)
        ) {
            return false;
        }
        const epoch = ++this.operationEpoch;
        this.mutable.set({
            ...state,
            busy_effect_id: effectId,
            error: null,
            announcement: t('interaction.notice.selecting'),
        });
        try {
            const receipt = await this.client.submitInteractionChoice({
                conversation_id: state.conversation_id,
                branch_id: state.branch_id,
                effect_id: effectId,
                choice_id: choiceId,
                expected_state_revision: state.current_state_revision,
            });
            if (epoch !== this.operationEpoch) return false;
            if (
                receipt.choice_effect.effect_id !== effectId ||
                receipt.choice_effect.conversation_id !== state.conversation_id ||
                receipt.choice_effect.branch_id !== state.branch_id ||
                receipt.choice_effect.choice_status !== 'consumed' ||
                receipt.choice_effect.selected_choice_id !== choiceId ||
                receipt.choice_effect.resulting_state_revision !== receipt.resulting_state_revision
            ) {
                const message = t('interaction.error.snapshot_mismatch');
                this.mutable.update((current) => ({
                    ...current,
                    phase: 'error',
                    busy_effect_id: null,
                    error: message,
                    announcement: message,
                }));
                return false;
            }
            const current = get(this.mutable);
            this.mutable.set({
                ...current,
                current_state_revision: receipt.resulting_state_revision,
                effects: boundedMergedEffects(
                    current.effects.filter(({ effect_id }) => effect_id !== effectId),
                    [receipt.choice_effect],
                ),
                busy_effect_id: null,
                error: null,
                announcement: t('interaction.notice.selected'),
            });
            return true;
        } catch (error: unknown) {
            if (epoch !== this.operationEpoch) return false;
            const message = errorLabel(error);
            this.mutable.update((current) => ({
                ...current,
                busy_effect_id: null,
                error: message,
                announcement: message,
            }));
            return false;
        }
    }

    async decideProposal(proposalId: string, decision: 'approve' | 'reject'): Promise<boolean> {
        const state = get(this.mutable);
        const target = state.pending_proposals.find(({ proposal }) => proposal.id === proposalId);
        if (
            state.phase !== 'ready' ||
            state.conversation_id === null ||
            state.branch_id === null ||
            state.has_more_expired_proposals ||
            state.busy_proposal_id !== null ||
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
        const epoch = ++this.operationEpoch;
        this.mutable.set({
            ...state,
            busy_proposal_id: proposalId,
            error: null,
            announcement:
                decision === 'approve'
                    ? t('interaction.notice.approving')
                    : t('interaction.notice.rejecting'),
        });
        try {
            const receipt = await this.client.decideInteractionProposal({
                conversation_id: state.conversation_id,
                branch_id: state.branch_id,
                proposal_record_id: proposalId,
                expected_state_revision: target.state_revision,
                expected_proposal_revision: target.proposal_revision,
                decision,
            });
            if (epoch !== this.operationEpoch) return false;
            if (
                receipt.proposal.id !== proposalId ||
                receipt.proposal.status !== (decision === 'approve' ? 'approved' : 'rejected') ||
                receipt.proposal.title !== target.proposal.title ||
                receipt.proposal.body !== target.proposal.body ||
                receipt.proposal.projection_rejection_reason !==
                    target.proposal.projection_rejection_reason ||
                receipt.proposal.source_interaction_state_revision !==
                    target.proposal.source_interaction_state_revision ||
                receipt.proposal.requested_at_epoch_seconds !==
                    target.proposal.requested_at_epoch_seconds ||
                receipt.proposal.expires_at_epoch_seconds !==
                    target.proposal.expires_at_epoch_seconds ||
                !isSafeRevision(receipt.proposal.decided_at_epoch_seconds) ||
                receipt.proposal.decided_at_epoch_seconds <
                    target.proposal.requested_at_epoch_seconds ||
                !isSafeRevision(receipt.state_revision) ||
                receipt.state_revision < target.state_revision
            ) {
                const message = t('interaction.error.decision_mismatch');
                this.mutable.update((current) => ({
                    ...current,
                    phase: 'error',
                    busy_proposal_id: null,
                    error: message,
                    announcement: message,
                }));
                return false;
            }
            const current = get(this.mutable);
            this.mutable.set({
                ...current,
                current_state_revision: receipt.state_revision,
                pending_proposals: current.pending_proposals.filter(
                    ({ proposal }) => proposal.id !== proposalId,
                ),
                busy_proposal_id: null,
                error: null,
                announcement:
                    decision === 'approve'
                        ? t('interaction.notice.approved')
                        : t('interaction.notice.rejected'),
            });
            return true;
        } catch (error: unknown) {
            if (epoch !== this.operationEpoch) return false;
            const message = errorLabel(error);
            this.mutable.update((current) => ({
                ...current,
                busy_proposal_id: null,
                error: message,
                announcement: message,
            }));
            return false;
        }
    }

    destroy(): void {
        this.destroyed = true;
        ++this.operationEpoch;
        this.unlisten?.();
        this.unlisten = null;
    }
}
