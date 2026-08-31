import { t } from '../../../lib/i18n';
import type {
    PromptPlanPreviewDto,
    PromptPlanRequestInput,
    ReviewedPromptSendInput,
} from '../../../lib/ipc/contracts';

import {
    errorLabel,
    isValidGenerationAttemptId,
    type OrchestrationCapableClient,
} from './orchestration-state';
import type { OrchestrationStateController } from './orchestration-state-controller';

export class PlanInteractionController {
    constructor(
        private readonly client: OrchestrationCapableClient,
        private readonly state: OrchestrationStateController,
    ) {}

    async resolvePlanPreview(userText: string): Promise<PromptPlanPreviewDto | null> {
        return this.resolvePlanPreviewOperation(userText, false, null);
    }

    async resolveNewPlanPreview(userText: string): Promise<PromptPlanPreviewDto | null> {
        return this.resolvePlanPreviewOperation(userText, true, null);
    }

    async resumePlanPreview(
        generationAttemptId: string,
        userText: string,
    ): Promise<PromptPlanPreviewDto | null> {
        if (!isValidGenerationAttemptId(generationAttemptId)) {
            const contextKey = this.state.snapshot().context_key;
            this.state.updateForContext(contextKey, (state) => ({
                ...state,
                error: t('orchestration.error.invalid_attempt_id'),
            }));
            return null;
        }
        return this.resolvePlanPreviewOperation(userText, false, generationAttemptId);
    }

    private async resolvePlanPreviewOperation(
        userText: string,
        rotateOperationNonce: boolean,
        requestedResumeAttemptId: string | null,
    ): Promise<PromptPlanPreviewDto | null> {
        const state = this.state.snapshot();
        const contextKey = state.context_key;
        const resolve = this.client.resolvePromptPreview;
        const generationTarget = state.workspace.generation_target;
        if (
            resolve === undefined ||
            state.workspace.room_config.conversation_id === '' ||
            userText.trim() === '' ||
            generationTarget === null
        ) {
            if (resolve === undefined) {
                this.state.updateForContext(contextKey, (current) => ({
                    ...current,
                    error: t('orchestration.error.unsupported_plan_preview'),
                }));
            }
            return null;
        }

        const resumeAttemptId = rotateOperationNonce
            ? null
            : (requestedResumeAttemptId ?? state.plan_generation_attempt_id);
        const operationNonce =
            resumeAttemptId === null
                ? rotateOperationNonce || state.plan_operation_nonce === null
                    ? crypto.randomUUID()
                    : state.plan_operation_nonce
                : state.plan_operation_nonce;
        const request: PromptPlanRequestInput = {
            conversation_id: state.workspace.room_config.conversation_id,
            branch_id: state.workspace.room_config.branch_id,
            expected_head: state.workspace.expected_head,
            user_text: userText,
            generation_target: generationTarget,
            prompt_preset_id: state.workspace.room_config.prompt_preset_id,
            variable_overrides: structuredClone(state.workspace.room_config.variable_overrides),
            expected_plan_hash: rotateOperationNonce
                ? null
                : (state.workspace.plan_preview?.plan_hash ?? null),
            ...(resumeAttemptId === null
                ? { operation_nonce: operationNonce }
                : { generation_attempt_id: resumeAttemptId }),
        };
        const previewEpoch = this.state.beginPlanPreviewRequest();
        if (
            !this.state.updateForContext(contextKey, (current) => ({
                ...current,
                plan_operation_nonce: operationNonce,
                plan_generation_attempt_id: resumeAttemptId,
                plan_preview_request: request,
                workspace: { ...current.workspace, plan_preview: null },
                error: null,
            }))
        ) {
            return null;
        }

        try {
            const preview = await resolve.call(this.client, request);
            const contextApplied = this.state.updateForContext(contextKey, (current) => {
                if (
                    !this.state.isPlanPreviewEpoch(previewEpoch) ||
                    (resumeAttemptId === null
                        ? current.plan_operation_nonce !== operationNonce ||
                          current.plan_generation_attempt_id !== null
                        : current.plan_generation_attempt_id !== resumeAttemptId)
                ) {
                    return current;
                }
                if (
                    !isValidGenerationAttemptId(preview.generation_attempt_id) ||
                    (resumeAttemptId !== null && preview.generation_attempt_id !== resumeAttemptId)
                ) {
                    return {
                        ...current,
                        error: isValidGenerationAttemptId(preview.generation_attempt_id)
                            ? t('orchestration.error.plan_attempt_mismatch')
                            : t('orchestration.error.invalid_attempt_id'),
                    };
                }
                return {
                    ...current,
                    workspace: { ...current.workspace, plan_preview: preview },
                    plan_generation_attempt_id: preview.generation_attempt_id,
                    plan_preview_request: request,
                    error: null,
                };
            });
            const current = this.state.snapshot();
            return contextApplied &&
                this.state.isPlanPreviewEpoch(previewEpoch) &&
                current.plan_generation_attempt_id === preview.generation_attempt_id &&
                current.workspace.plan_preview === preview
                ? preview
                : null;
        } catch (error: unknown) {
            this.state.updateForContext(contextKey, (current) =>
                this.state.isPlanPreviewEpoch(previewEpoch) &&
                (resumeAttemptId === null
                    ? current.plan_operation_nonce === operationNonce &&
                      current.plan_generation_attempt_id === null
                    : current.plan_generation_attempt_id === resumeAttemptId)
                    ? { ...current, error: errorLabel(error) }
                    : current,
            );
            return null;
        }
    }

    clearPlanPreview(): void {
        this.state.invalidatePlanPreview();
        this.state.update((state) => ({
            ...state,
            plan_preview_request: null,
            workspace:
                state.workspace.plan_preview === null
                    ? state.workspace
                    : { ...state.workspace, plan_preview: null },
        }));
    }

    completePlanOperation(): void {
        this.state.invalidatePlanPreview();
        this.state.update((state) => ({
            ...state,
            plan_operation_nonce: null,
            plan_generation_attempt_id: null,
            plan_preview_request: null,
            workspace:
                state.workspace.plan_preview === null
                    ? state.workspace
                    : { ...state.workspace, plan_preview: null },
        }));
    }

    reviewedPromptSendInput(): ReviewedPromptSendInput | null {
        const state = this.state.snapshot();
        const preview = state.workspace.plan_preview;
        const request = state.plan_preview_request;
        if (
            preview === null ||
            request === null ||
            state.plan_generation_attempt_id === null ||
            preview.generation_attempt_id !== state.plan_generation_attempt_id
        ) {
            return null;
        }
        return {
            conversation_id: request.conversation_id,
            branch_id: request.branch_id,
            expected_head: request.expected_head,
            user_text: request.user_text,
            generation_target: structuredClone(request.generation_target),
            prompt_preset_id: request.prompt_preset_id,
            variable_overrides: structuredClone(request.variable_overrides),
            expected_plan_hash: preview.plan_hash,
            generation_attempt_id: preview.generation_attempt_id,
        };
    }

    async decideProposal(proposalId: string, approved: boolean): Promise<boolean> {
        const snapshot = this.state.snapshot();
        const contextKey = snapshot.context_key;
        const target = snapshot.workspace.interaction_proposals.find(
            (candidate) => candidate.proposal.id === proposalId,
        );
        if (
            target === undefined ||
            snapshot.phase !== 'ready' ||
            snapshot.busy_interaction_proposal_id !== null ||
            target.conversation_id !== snapshot.workspace.room_config.conversation_id ||
            target.branch_id !== snapshot.workspace.room_config.branch_id ||
            target.proposal.status !== 'pending'
        ) {
            return false;
        }
        if (approved && target.proposal.projection_rejection_reason === 'unsafe_native_text') {
            this.state.updateForContext(contextKey, (state) => ({
                ...state,
                announcement: t('interaction.error.unreviewable'),
            }));
            return false;
        }
        this.state.updateForContext(contextKey, (state) => ({
            ...state,
            busy_interaction_proposal_id: proposalId,
            error: null,
            announcement: approved
                ? t('interaction.notice.approving')
                : t('interaction.notice.rejecting'),
        }));
        try {
            const receipt = await this.client.decideInteractionProposal({
                conversation_id: target.conversation_id,
                branch_id: target.branch_id,
                proposal_record_id: proposalId,
                expected_state_revision: target.state_revision,
                expected_proposal_revision: target.proposal_revision,
                decision: approved ? 'approve' : 'reject',
            });
            const decidedAt = receipt.proposal.decided_at_epoch_seconds;
            if (
                receipt.proposal.id !== proposalId ||
                receipt.proposal.status !== (approved ? 'approved' : 'rejected') ||
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
                decidedAt === null ||
                !Number.isSafeInteger(decidedAt) ||
                decidedAt < target.proposal.requested_at_epoch_seconds ||
                !Number.isSafeInteger(receipt.state_revision) ||
                receipt.state_revision < target.state_revision
            ) {
                throw new Error('Core interaction proposal receipt did not match the decision.');
            }
            return this.state.updateForContext(contextKey, (state) => ({
                ...state,
                busy_interaction_proposal_id: null,
                announcement: approved
                    ? t('interaction.notice.approved')
                    : t('interaction.notice.rejected'),
                workspace: {
                    ...state.workspace,
                    interaction_state_revision: receipt.state_revision,
                    interaction_proposals: state.workspace.interaction_proposals.filter(
                        (proposal) => proposal.proposal.id !== proposalId,
                    ),
                },
                error: null,
            }));
        } catch (error: unknown) {
            this.state.updateForContext(contextKey, (state) => ({
                ...state,
                busy_interaction_proposal_id: null,
                error: errorLabel(error),
                announcement: t('orchestration.notice.decision_failed'),
            }));
            return false;
        }
    }
}
