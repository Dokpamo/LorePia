import type {
    DecideGenerationAttemptProposalInput,
    DecideInteractionProposalInput,
    ExpireGenerationAttemptProposalsInput,
    ExpireInteractionProposalsInput,
    ListRetryableGenerationAttemptsInput,
    GenerationAttemptProposalDecisionReceiptDto,
    GenerationAttemptProposalExpiryReceiptDto,
    GenerationAttemptProposalListItemDto,
    RetryableGenerationAttemptDto,
    InteractionEffectEventDto,
    InteractionEffectHistoryPageDto,
    InteractionChoiceSelectionReceiptDto,
    InteractionProposalListItemDto,
    InteractionProposalExpiryReceiptDto,
    InteractionReopenSnapshotDto,
    InteractionProposalDecisionReceiptDto,
    ListInteractionEffectHistoryInput,
    ListGenerationAttemptProposalsInput,
    ListInteractionProposalsInput,
    ListReopenInteractionEffectsInput,
    SubmitInteractionChoiceInput,
} from '../contracts';

import { isInteractionEffectEvent } from '../client-payload-guards';

import { LOREPIA_COMMANDS, LOREPIA_EVENTS } from '../commands';

import { LibraryClient } from './library';

export abstract class InteractionClient extends LibraryClient {
    listInteractionEffects(): Promise<InteractionEffectEventDto[]> {
        return this.call(LOREPIA_COMMANDS.listInteractionEffects);
    }

    acknowledgeInteractionEffect(deliveryId: string): Promise<void> {
        return this.call(LOREPIA_COMMANDS.acknowledgeInteractionEffect, {
            request: { delivery_id: deliveryId },
        });
    }

    retryInteractionEffect(deliveryId: string): Promise<void> {
        return this.call(LOREPIA_COMMANDS.retryInteractionEffect, {
            request: { delivery_id: deliveryId },
        });
    }

    decideInteractionProposal(
        input: DecideInteractionProposalInput,
    ): Promise<InteractionProposalDecisionReceiptDto> {
        return this.call(LOREPIA_COMMANDS.decideInteractionProposal, { request: input });
    }

    decideGenerationAttemptProposal(
        input: DecideGenerationAttemptProposalInput,
    ): Promise<GenerationAttemptProposalDecisionReceiptDto> {
        return this.call(LOREPIA_COMMANDS.decideGenerationAttemptProposal, { request: input });
    }

    listInteractionProposals(
        input: ListInteractionProposalsInput,
    ): Promise<InteractionProposalListItemDto[]> {
        return this.call(LOREPIA_COMMANDS.listInteractionProposals, { request: input });
    }

    listGenerationAttemptProposals(
        input: ListGenerationAttemptProposalsInput,
    ): Promise<GenerationAttemptProposalListItemDto[]> {
        return this.call(LOREPIA_COMMANDS.listGenerationAttemptProposals, { request: input });
    }

    listRetryableGenerationAttempts(
        input: ListRetryableGenerationAttemptsInput,
    ): Promise<RetryableGenerationAttemptDto[]> {
        return this.call(LOREPIA_COMMANDS.listRetryableGenerationAttempts, { request: input });
    }

    expireInteractionProposals(
        input: ExpireInteractionProposalsInput,
    ): Promise<InteractionProposalExpiryReceiptDto> {
        return this.call(LOREPIA_COMMANDS.expireInteractionProposals, { request: input });
    }

    expireGenerationAttemptProposals(
        input: ExpireGenerationAttemptProposalsInput,
    ): Promise<GenerationAttemptProposalExpiryReceiptDto> {
        return this.call(LOREPIA_COMMANDS.expireGenerationAttemptProposals, { request: input });
    }

    listInteractionEffectHistory(
        input: ListInteractionEffectHistoryInput,
    ): Promise<InteractionEffectHistoryPageDto> {
        return this.call(LOREPIA_COMMANDS.listInteractionEffectHistory, { request: input });
    }

    listReopenInteractionEffects(
        input: ListReopenInteractionEffectsInput,
    ): Promise<InteractionReopenSnapshotDto> {
        return this.call(LOREPIA_COMMANDS.listReopenInteractionEffects, { request: input });
    }

    submitInteractionChoice(
        input: SubmitInteractionChoiceInput,
    ): Promise<InteractionChoiceSelectionReceiptDto> {
        return this.call(LOREPIA_COMMANDS.submitInteractionChoice, { request: input });
    }

    subscribeInteractionEffects(
        onEffect: (effect: InteractionEffectEventDto) => void,
    ): Promise<() => void> {
        return this.transport.listen(LOREPIA_EVENTS.interactionEffect, (payload) => {
            if (isInteractionEffectEvent(payload)) onEffect(payload);
        });
    }
}
