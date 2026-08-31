<script lang="ts">
    import type { LorepiaAppController, LorepiaAppState } from '../../../app/app-controller';
    import DetailActionBar from '../../../components/detail/DetailActionBar.svelte';
    import type { LorepiaClient } from '../../../lib/ipc/contracts';
    import GenerationAttemptApprovals from '../../chat/GenerationAttemptApprovals.svelte';
    import MemoryQueryRetryPanel from '../MemoryQueryRetryPanel.svelte';
    import {
        MAX_VISIBLE_PLAN_OPERATION_NONCE_CHARS,
        type OrchestrationController,
        type OrchestrationState,
    } from '../orchestration-controller';
    import PlanDetails from './PlanDetails.svelte';

    interface Props {
        client?: LorepiaClient;
        appState: LorepiaAppState;
        orchestrationState: OrchestrationState;
        controller: OrchestrationController;
        appController?: LorepiaAppController;
        planUserText?: string;
        reviewedSendBusy?: boolean;
        attemptApprovalRefreshEpoch?: number;
        expertSearch?: string;
        expertFilter?: 'all' | 'messages' | 'provider' | 'parameters' | 'diff';
    }

    let {
        client,
        appState,
        orchestrationState,
        controller,
        appController,
        planUserText = $bindable(''),
        reviewedSendBusy = $bindable(false),
        attemptApprovalRefreshEpoch = $bindable(0),
        expertSearch = $bindable(''),
        expertFilter = $bindable<'all' | 'messages' | 'provider' | 'parameters' | 'diff'>('all'),
    }: Props = $props();

    const previewGenerationTarget = $derived(orchestrationState.workspace.generation_target);

    function boundedPlanIdentifier(value: string): string {
        return value.slice(0, MAX_VISIBLE_PLAN_OPERATION_NONCE_CHARS);
    }

    async function resolvePlanPreviewAndRefreshRetries(
        generationAttemptId?: string,
    ): Promise<void> {
        appController?.clearMemoryQueryRetryNotice();
        if (generationAttemptId === undefined) {
            await controller.resolvePlanPreview(planUserText);
        } else {
            await controller.resumePlanPreview(generationAttemptId, planUserText);
        }
        attemptApprovalRefreshEpoch += 1;
        await appController?.refreshMemoryQueryRetries();
    }

    async function resolveNewPlanPreviewAndRefreshRetries(): Promise<void> {
        appController?.clearMemoryQueryRetryNotice();
        await controller.resolveNewPlanPreview(planUserText);
        attemptApprovalRefreshEpoch += 1;
        await appController?.refreshMemoryQueryRetries();
    }

    async function sendReviewedPlan(): Promise<void> {
        const input = controller.reviewedPromptSendInput();
        if (appController === undefined || input === null || reviewedSendBusy) return;
        reviewedSendBusy = true;
        try {
            const sent = await appController.sendReviewedPrompt(input);
            attemptApprovalRefreshEpoch += 1;
            if (sent) {
                planUserText = '';
                controller.completePlanOperation();
            }
        } finally {
            reviewedSendBusy = false;
        }
    }
</script>

<!-- prettier-ignore-start -->

<section class="studio-card plan-detail" data-studio-owned-fields="" aria-labelledby="plan-preview-title">
    <div class="section-heading">
        <div>
            <h3 id="plan-preview-title">최종 요청 계획</h3>
            <p>
                사용자가 요청할 때만 실제 생성과 같은 Core resolver가 bounded 최종
                내용과 credential-free 제공자 요청을 만듭니다.
            </p>
        </div>
    </div>
    {#if client !== undefined}
        <div class="plan-embedded-panel">
            <GenerationAttemptApprovals
                {client}
                conversationId={orchestrationState.workspace.room_config.conversation_id || null}
                sourceBranchId={orchestrationState.workspace.room_config.branch_id || null}
                headingId="studio-generation-attempt-approvals-title"
                refreshEpoch={attemptApprovalRefreshEpoch}
                retryLabel="최종 계획 다시 검토"
                onRetry={resolvePlanPreviewAndRefreshRetries}
            />
        </div>
    {/if}
    <label>
        <span>다음 사용자 메시지</span>
        <textarea
            rows="4"
            maxlength="16384"
            value={planUserText}
            oninput={(event) => {
                planUserText = event.currentTarget.value;
                controller.clearPlanPreview();
            }}
            placeholder="실제 전송 전 계획을 계산할 메시지를 입력하세요."
        ></textarea>
    </label>
    {#if previewGenerationTarget === null}
        <p class="bounded-note" role="note">
            저장된 모델 라우트와 생성 프리셋을 먼저 선택해야 계획을 계산할 수
            있습니다.
        </p>
    {:else}
        <p class="inline-note">
            생성 대상: {previewGenerationTarget.model_route_id} ·
            {previewGenerationTarget.generation_preset_id}
        </p>
    {/if}
    <p class="bounded-note" role="note">
        최초 응답 전 재시도는 현재 작업 nonce를 유지합니다. 계획 응답이나 승인을
        받은 뒤에는 고정된 생성 시도 ID로 재개합니다. 다른 작업을 시작하려면 새 작업
        미리보기를 선택하세요.
    </p>
    {#if appController !== undefined}
        <div class="plan-embedded-panel">
            <MemoryQueryRetryPanel
                state={appState.memory_query_retries}
                controller={appController}
                headingId="studio-memory-query-retry-title"
            />
        </div>
    {/if}
    {#if orchestrationState.workspace.plan_preview === null}
        <p class="empty-note">계획 미리보기를 계산하지 않았습니다.</p>
    {:else}
        {@const preview = orchestrationState.workspace.plan_preview}
        <dl class="plan-summary" data-studio-owned-definition="" data-studio-owned-code="">
            <div>
                <dt>계획 ID</dt>
                <dd><code>{boundedPlanIdentifier(preview.plan_id)}</code></dd>
            </div>
            <div>
                <dt>생성 시도 ID</dt>
                <dd><code>{boundedPlanIdentifier(preview.generation_attempt_id)}</code></dd>
            </div>
            <div>
                <dt>작업 nonce</dt>
                <dd>
                    {#if orchestrationState.plan_operation_nonce === null}
                        <span>기존 생성 시도 재개</span>
                    {:else}
                        <code>{boundedPlanIdentifier(orchestrationState.plan_operation_nonce)}</code
                        >
                    {/if}
                </dd>
            </div>
            <div>
                <dt>계획 해시</dt>
                <dd><code>{preview.plan_hash}</code></dd>
            </div>
            <div>
                <dt>프롬프트 프리셋</dt>
                <dd>{preview.prompt_preset_id} · revision {preview.prompt_preset_revision}</dd>
            </div>
            <div>
                <dt>입력 토큰</dt>
                <dd>
                    {preview.estimated_input_tokens} · {preview.token_estimator_id} ·
                    {preview.token_estimate_exact ? 'exact' : 'estimate'}
                </dd>
            </div>
            <div>
                <dt>사용 가능 입력 토큰</dt>
                <dd>{preview.available_input_tokens}</dd>
            </div>
            <div>
                <dt>생성 대상</dt>
                <dd>
                    {preview.generation_target.model_route_id} ·
                    {preview.generation_target.generation_preset_id}
                </dd>
            </div>
        </dl>

        <p class="bounded-note" role="note">
            이 버튼은 위 미리보기의 시도 ID와 계획 해시를 모두 다시 검증한 뒤에만
            전송합니다. 일반 채팅 전송은 별도의 검토되지 않은 동작입니다.
        </p>

        <PlanDetails {preview} bind:expertSearch bind:expertFilter />
    {/if}
    <DetailActionBar fixed ariaLabel="최종 요청 계획 작업">
        <button
            class="detail-action"
            type="button"
            aria-label="새 작업 미리보기"
            disabled={orchestrationState.workspace.room_config.conversation_id === '' ||
                planUserText.trim() === '' ||
                previewGenerationTarget === null}
            onclick={() => void resolveNewPlanPreviewAndRefreshRetries()}
        >
            새 작업
        </button>
        {#if orchestrationState.workspace.plan_preview === null}
            <button
                class="detail-action detail-action--grow primary"
                type="button"
                aria-label="계획 다시 계산"
                disabled={orchestrationState.workspace.room_config.conversation_id === '' ||
                    planUserText.trim() === '' ||
                    previewGenerationTarget === null}
                onclick={() => void resolvePlanPreviewAndRefreshRetries()}
            >
                계획 계산
            </button>
        {:else}
            <button
                class="detail-action"
                type="button"
                aria-label="계획 다시 계산"
                disabled={orchestrationState.workspace.room_config.conversation_id === '' ||
                    planUserText.trim() === '' ||
                    previewGenerationTarget === null}
                onclick={() => void resolvePlanPreviewAndRefreshRetries()}
            >
                다시 계산
            </button>
            <button
                class="detail-action detail-action--grow primary"
                type="button"
                aria-label="검토한 계획으로 전송"
                disabled={appController === undefined ||
                    reviewedSendBusy ||
                    controller.reviewedPromptSendInput() === null}
                onclick={() => void sendReviewedPlan()}
            >
                {reviewedSendBusy ? '전송 중…' : '검토 계획 전송'}
            </button>
        {/if}
    </DetailActionBar>
</section>

<!-- prettier-ignore-end -->
