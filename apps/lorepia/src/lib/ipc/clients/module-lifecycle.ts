import type {
    ActivateContentModuleInput,
    ApplyContentModuleRollbackInput,
    ContentModuleDeactivationReceiptDto,
    ContentModuleDeactivationReviewDto,
    ContentModuleActivationPlanDto,
    ContentModuleActivationReceiptDto,
    ContentModuleActivationReviewPresentationDto,
    ContentModuleLifecycleBindingListDto,
    ContentModuleLifecycleCandidateListDto,
    ContentModuleRollbackPlanDto,
    ContentModuleRollbackReviewPresentationDto,
    DeactivateContentModuleInput,
    ListContentModuleLifecycleBindingsInput,
    ListContentModuleLifecycleCandidatesInput,
    ResolveContentModuleActivationInput,
    ResolveContentModuleRollbackInput,
    ReviewContentModuleActivationInput,
    ReviewContentModuleDeactivationInput,
    ReviewContentModuleRollbackInput,
} from '../../../features/orchestration/module-lifecycle-contracts';

import { LOREPIA_COMMANDS } from '../commands';

import { OrchestrationClient } from './orchestration';

export abstract class ModuleLifecycleClient extends OrchestrationClient {
    listContentModuleLifecycleCandidates(
        input: ListContentModuleLifecycleCandidatesInput,
    ): Promise<ContentModuleLifecycleCandidateListDto> {
        return this.call(LOREPIA_COMMANDS.listContentModuleLifecycleCandidates, { request: input });
    }

    listContentModuleLifecycleBindings(
        input: ListContentModuleLifecycleBindingsInput,
    ): Promise<ContentModuleLifecycleBindingListDto> {
        return this.call(LOREPIA_COMMANDS.listContentModuleLifecycleBindings, { request: input });
    }

    reviewContentModuleActivation(
        input: ReviewContentModuleActivationInput,
    ): Promise<ContentModuleActivationReviewPresentationDto> {
        return this.call(LOREPIA_COMMANDS.reviewContentModuleActivation, { request: input });
    }

    resolveContentModuleActivation(
        input: ResolveContentModuleActivationInput,
    ): Promise<ContentModuleActivationPlanDto> {
        return this.call(LOREPIA_COMMANDS.resolveContentModuleActivation, { request: input });
    }

    activateContentModule(
        input: ActivateContentModuleInput,
    ): Promise<ContentModuleActivationReceiptDto> {
        return this.call(LOREPIA_COMMANDS.activateContentModule, { request: input });
    }

    reviewContentModuleDeactivation(
        input: ReviewContentModuleDeactivationInput,
    ): Promise<ContentModuleDeactivationReviewDto> {
        return this.call(LOREPIA_COMMANDS.reviewContentModuleDeactivation, { request: input });
    }

    deactivateContentModule(
        input: DeactivateContentModuleInput,
    ): Promise<ContentModuleDeactivationReceiptDto> {
        return this.call(LOREPIA_COMMANDS.deactivateContentModule, { request: input });
    }

    reviewContentModuleRollback(
        input: ReviewContentModuleRollbackInput,
    ): Promise<ContentModuleRollbackReviewPresentationDto> {
        return this.call(LOREPIA_COMMANDS.reviewContentModuleRollback, { request: input });
    }

    resolveContentModuleRollback(
        input: ResolveContentModuleRollbackInput,
    ): Promise<ContentModuleRollbackPlanDto> {
        return this.call(LOREPIA_COMMANDS.resolveContentModuleRollback, { request: input });
    }

    applyContentModuleRollback(
        input: ApplyContentModuleRollbackInput,
    ): Promise<ContentModuleActivationReceiptDto> {
        return this.call(LOREPIA_COMMANDS.applyContentModuleRollback, { request: input });
    }
}
