<script lang="ts">
    import {
        discoveryCredentialTarget,
        type LorepiaAppController,
        type LorepiaAppState,
    } from '../../app/app-controller';
    import type {
        BeginProviderDiscoveryCurlInput,
        BeginProviderDiscoveryInput,
        ContinueProviderDiscoveryActionInput,
        DiscoveryAssistantFailureKindInput,
        DiscoveryAssistantHostActionDto,
        DiscoveryCandidateSummaryDto,
        ProviderDiscoveryConnectionOptionsInput,
    } from '../../lib/ipc/contracts';

    interface Props {
        appState: LorepiaAppState;
        controller: LorepiaAppController;
    }

    let { appState, controller }: Props = $props();
    let sourceMode = $state<'site' | 'known_provider' | 'curl'>('site');
    let connectionId = $state('');
    let displayName = $state('');
    let siteUrl = $state('');
    let docsUrl = $state('');
    let templateId = $state('');
    let preferredAssistantId = $state('');
    let credentialRequested = $state(false);
    let documentEvidenceUrl = $state('');
    let unknownResolution = $state<
        'confirmed_no_effect' | 'confirmed_compensated' | 'manually_reconciled_as_failed'
    >('confirmed_no_effect');
    let assistantFailureKind = $state<DiscoveryAssistantFailureKindInput>('transport');
    let assistantFailureRetryable = $state(true);
    let busy = $state(false);

    const workspace = $derived(appState.providers.workspace);
    const selectedSession = $derived(
        workspace.discoveries.find((session) => session.id === workspace.selected_discovery_id) ??
            null,
    );
    const latestEvent = $derived(
        workspace.discovery_event?.session_id === selectedSession?.id
            ? workspace.discovery_event
            : null,
    );
    const actionKind = $derived(selectedSession?.action_required?.kind ?? null);
    const assistantBoundary = $derived(workspace.discovery_assistant_resume_boundary);
    const selectedCredentialTarget = $derived(
        selectedSession === null ? null : discoveryCredentialTarget(selectedSession),
    );
    const selectedCredentialStatus = $derived(
        selectedCredentialTarget === null
            ? null
            : (workspace.credential_statuses[
                  `discovery_session:${selectedCredentialTarget.session_id}`
              ] ?? 'missing'),
    );
    const actionNeedsCredential = $derived(
        selectedSession !== null &&
            selectedCredentialTarget !== null &&
            ((selectedSession.state === 'awaiting_credential_origin_approval' &&
                actionKind === 'approve_credential_origin') ||
                (selectedSession.state === 'awaiting_probe_consent' &&
                    actionKind === 'approve_probes') ||
                (selectedSession.state === 'interrupted' &&
                    actionKind === 'restart_interrupted' &&
                    (selectedSession.recovery_operation === 'list_models' ||
                        selectedSession.recovery_operation === 'probe_capabilities'))),
    );

    function options(): ProviderDiscoveryConnectionOptionsInput {
        return {
            values: [],
            api_base_path: null,
            timeout_seconds: 30,
            network_mode: 'public',
            local_network_approval: null,
        };
    }

    async function startDiscovery(): Promise<void> {
        if (connectionId.trim() === '' || displayName.trim() === '') return;
        busy = true;
        try {
            if (sourceMode === 'curl') {
                const input: BeginProviderDiscoveryCurlInput = {
                    connection_id: connectionId.trim(),
                    display_name: displayName.trim(),
                    docs_url: docsUrl.trim() === '' ? null : docsUrl.trim(),
                    credential_binding_requested: credentialRequested,
                    preferred_assistant: preferredAssistantId === '' ? null : preferredAssistantId,
                    connection_options: options(),
                    supplied_evidence_ids: [],
                };
                await controller.beginProviderDiscovery({
                    kind: 'curl',
                    input,
                });
                return;
            }

            if (siteUrl.trim() === '' || (sourceMode === 'known_provider' && templateId === '')) {
                return;
            }
            const input: BeginProviderDiscoveryInput = {
                connection_id: connectionId.trim(),
                display_name: displayName.trim(),
                site_url: siteUrl.trim(),
                docs_url: docsUrl.trim() === '' ? null : docsUrl.trim(),
                credential_binding_requested: credentialRequested,
                preferred_assistant: preferredAssistantId === '' ? null : preferredAssistantId,
                connection_options: options(),
                supplied_evidence_ids: [],
                source:
                    sourceMode === 'known_provider'
                        ? { kind: 'known_provider', template_id: templateId }
                        : { kind: 'site' },
            };
            await controller.beginProviderDiscovery({ kind: 'site', input });
        } finally {
            busy = false;
        }
    }

    async function continueWith(action: ContinueProviderDiscoveryActionInput): Promise<void> {
        busy = true;
        try {
            await controller.continueProviderDiscovery(action);
        } finally {
            busy = false;
        }
    }

    async function submitDocumentEvidence(): Promise<void> {
        busy = true;
        try {
            if (await controller.supplyProviderDiscoveryDocumentEvidence(documentEvidenceUrl)) {
                documentEvidenceUrl = '';
            }
        } finally {
            busy = false;
        }
    }

    async function submitCurlEvidence(): Promise<void> {
        busy = true;
        try {
            await controller.supplyProviderDiscoveryCurlEvidence();
        } finally {
            busy = false;
        }
    }

    async function commitDiscovery(): Promise<void> {
        busy = true;
        try {
            await controller.commitProviderDiscovery();
        } finally {
            busy = false;
        }
    }

    async function captureDiscoveryCredential(): Promise<void> {
        const target = selectedCredentialTarget;
        if (target === null) return;
        busy = true;
        try {
            await controller.captureProviderCredential(target);
        } finally {
            busy = false;
        }
    }

    function candidateLabel(summary: DiscoveryCandidateSummaryDto): string {
        switch (summary.kind) {
            case 'provider_template':
                return `${summary.template_id} v${String(summary.template_version)}`;
            case 'api_origin':
                return summary.origin;
            case 'official_document':
                return summary.url;
            case 'model_route':
                return summary.model_id;
            case 'manifest_draft':
                return `manifest v${String(summary.schema_version)}`;
        }
    }

    function assistantHostActionSummary(action: DiscoveryAssistantHostActionDto): string {
        return action.kind === 'request_more_evidence'
            ? `추가 질문 ${String(action.questions.length)}개`
            : action.review.draft.summary;
    }

    function terminalState(state: string): boolean {
        return ['completed', 'cancelled', 'failed'].includes(state);
    }
</script>

<section class="workflow-section" aria-labelledby="discovery-title">
    <header class="workflow-heading">
        <div>
            <p class="eyebrow">Durable setup assistant</p>
            <h2 id="discovery-title">프로바이더 탐색</h2>
            <p>원문과 자격증명은 저장하지 않고 Core의 검토·승인 상태만 표시합니다.</p>
        </div>
        <button
            type="button"
            disabled={busy}
            onclick={() => void controller.recoverProviderDiscoveries()}
        >
            중단 작업 복구
        </button>
    </header>

    <form
        class="workflow-form discovery-start"
        aria-label="프로바이더 탐색 시작"
        onsubmit={(event) => {
            event.preventDefault();
            void startDiscovery();
        }}
    >
        <label>
            <span>탐색 입력</span>
            <select bind:value={sourceMode}>
                <option value="site">사이트 URL</option>
                <option value="known_provider">알려진 템플릿</option>
                <option value="curl">cURL 붙여넣기</option>
            </select>
        </label>
        <label>
            <span>연결 ID</span>
            <input bind:value={connectionId} required autocomplete="off" />
        </label>
        <label>
            <span>표시 이름</span>
            <input bind:value={displayName} required autocomplete="off" />
        </label>
        {#if sourceMode !== 'curl'}
            <label>
                <span>사이트 URL</span>
                <input bind:value={siteUrl} type="url" required autocomplete="url" />
            </label>
        {/if}
        <label>
            <span>문서 URL (선택)</span>
            <input bind:value={docsUrl} type="url" autocomplete="url" />
        </label>
        <label>
            <span>설정 도우미 모델 (선택)</span>
            <select bind:value={preferredAssistantId}>
                <option value="">사용 안 함</option>
                {#each workspace.routes as route (route.id)}
                    <option value={route.id}>{route.display_name ?? route.model_id}</option>
                {/each}
            </select>
        </label>
        {#if sourceMode === 'known_provider'}
            <label>
                <span>템플릿</span>
                <select bind:value={templateId} required>
                    <option value="">선택</option>
                    {#each workspace.templates as template (template.id)}
                        <option value={template.id}>{template.display_name}</option>
                    {/each}
                </select>
            </label>
        {:else if sourceMode === 'curl'}
            <p class="wide">
                cURL을 클립보드에 복사한 뒤 탐색 시작을 누르세요. 네이티브 계층이 한 번만 캡처하고
                WebView에는 원문을 전달하지 않습니다.
            </p>
        {/if}
        <label class="check-row">
            <input type="checkbox" bind:checked={credentialRequested} />
            <span>운영체제 자격증명 슬롯 필요</span>
        </label>
        <button class="primary" type="submit" disabled={busy}>탐색 시작</button>
    </form>

    {#if workspace.discoveries.length > 0}
        <div class="workflow-toolbar">
            <label>
                <span>저장된 탐색 세션</span>
                <select
                    value={workspace.selected_discovery_id ?? ''}
                    onchange={(event) => {
                        const id = event.currentTarget.value;
                        if (id !== '') void controller.refreshProviderDiscovery(id);
                    }}
                >
                    <option value="">선택</option>
                    {#each workspace.discoveries as session (session.id)}
                        <option value={session.id}>
                            {session.display_name} · {session.state} · r{session.revision}
                        </option>
                    {/each}
                </select>
            </label>
            <button
                type="button"
                disabled={workspace.selected_discovery_id === null || busy}
                onclick={() => void controller.pollSelectedProviderDiscoveryEvents()}
            >
                이벤트 확인·새로고침
            </button>
        </div>
    {/if}

    {#if selectedSession}
        <article class="workflow-card">
            <header>
                <div>
                    <h3>{selectedSession.display_name}</h3>
                    <p>{selectedSession.state} · revision {selectedSession.revision}</p>
                </div>
                <button
                    class="danger"
                    type="button"
                    disabled={busy || terminalState(selectedSession.state)}
                    onclick={() => void controller.cancelProviderDiscovery()}
                >
                    탐색 취소
                </button>
            </header>

            {#if latestEvent}
                <dl class="status-grid">
                    <div>
                        <dt>최신 단계</dt>
                        <dd>{latestEvent.state}</dd>
                    </div>
                    <div>
                        <dt>필요 작업</dt>
                        <dd>{actionKind ?? '없음'}</dd>
                    </div>
                    <div>
                        <dt>Sequence</dt>
                        <dd>{latestEvent.sequence}</dd>
                    </div>
                </dl>
            {:else}
                <p class="notice">아직 확인하지 않은 탐색 이벤트가 없습니다.</p>
            {/if}

            {#if assistantBoundary}
                <section class="action-block assistant-block" aria-labelledby="assistant-title">
                    <h4 id="assistant-title">설정 도우미 체크포인트</h4>
                    <p>
                        {assistantBoundary.checkpoint ?? '승인 대기'} · 다음 작업
                        {assistantBoundary.action}
                    </p>

                    {#if assistantBoundary.questions.length > 0}
                        <ul>
                            {#each assistantBoundary.questions as question (question.id)}
                                <li>
                                    <strong>{question.question}</strong>
                                    <small>{question.required_evidence}</small>
                                </li>
                            {/each}
                        </ul>
                    {/if}

                    {#if assistantBoundary.action === 'run_assistant'}
                        <p class="notice" role="status">
                            원격 설정 도우미는 Rust가 정확한 요청을 신뢰할 수 있는 가격·토큰
                            정책으로 계산할 때까지 사용할 수 없습니다. 수동 입력과 결정론적 탐색은
                            계속 사용할 수 있습니다.
                        </p>
                    {:else if assistantBoundary.action === 'resume_core_host_action'}
                        <button
                            type="button"
                            disabled={busy}
                            onclick={() =>
                                void controller.resumeProviderDiscoveryAssistantCoreHostAction()}
                        >
                            저장된 Core 작업 재개
                        </button>
                    {:else if assistantBoundary.action === 'approve_retry'}
                        <button
                            type="button"
                            disabled={busy}
                            onclick={() => void controller.approveProviderDiscoveryAssistantRetry()}
                        >
                            도우미 재시도 승인
                        </button>
                    {:else if assistantBoundary.action === 'review_draft' && assistantBoundary.draft_review}
                        {@const draftReview = assistantBoundary.draft_review}
                        <div class="draft-review">
                            <strong>{draftReview.draft.summary}</strong>
                            <span>
                                충돌 {draftReview.unresolved_conflicts.length}개 · 질문
                                {draftReview.draft.unresolved_questions.length}개
                            </span>
                            <small>필수 검사: {draftReview.required_checks.join(', ')}</small>
                        </div>
                        <button
                            class="primary"
                            type="button"
                            disabled={busy ||
                                draftReview.unresolved_conflicts.length > 0 ||
                                draftReview.draft.unresolved_questions.length > 0}
                            onclick={() => void controller.acceptProviderDiscoveryAssistantDraft()}
                        >
                            검토한 도우미 초안 채택
                        </button>
                        <button
                            type="button"
                            disabled={busy}
                            onclick={() =>
                                void controller.requestProviderDiscoveryAssistantRevision()}
                        >
                            초안 수정 요청
                        </button>
                    {:else if assistantBoundary.action === 'restart_interrupted'}
                        <button
                            type="button"
                            disabled={busy}
                            onclick={() =>
                                void controller.restartProviderDiscoveryAssistantAfterInterruption()}
                        >
                            도우미 중단 지점에서 명시적 재시작
                        </button>
                    {:else if assistantBoundary.action === 'wait_for_assistant_outcome'}
                        <p>외부 요청 결과를 모르면 자동 재시도하지 않습니다.</p>
                        <button
                            type="button"
                            disabled={busy}
                            onclick={() =>
                                void controller.interruptProviderDiscoveryAssistant(
                                    'confirmed_no_external_effect',
                                )}
                        >
                            외부 효과 없음 확인 후 중단
                        </button>
                        <button
                            class="danger"
                            type="button"
                            disabled={busy}
                            onclick={() =>
                                void controller.interruptProviderDiscoveryAssistant(
                                    'external_outcome_unknown',
                                )}
                        >
                            결과 불명으로 중단
                        </button>
                    {/if}

                    {#if workspace.discovery_assistant_host_action}
                        {@const hostAction = workspace.discovery_assistant_host_action}
                        <div class="host-action">
                            <strong>도우미 반환: {hostAction.kind}</strong>
                            <span>{assistantHostActionSummary(hostAction)}</span>
                        </div>
                    {/if}

                    <details class="assistant-failure">
                        <summary>도우미 실패를 기록해야 하는 경우</summary>
                        <label>
                            <span>실패 종류</span>
                            <select bind:value={assistantFailureKind}>
                                <option value="transport">transport</option>
                                <option value="timeout">timeout</option>
                                <option value="rate_limited">rate_limited</option>
                                <option value="invalid_structured_output"
                                    >invalid_structured_output</option
                                >
                                <option value="draft_revision_required"
                                    >draft_revision_required</option
                                >
                                <option value="provider_rejected">provider_rejected</option>
                                <option value="internal">internal</option>
                            </select>
                        </label>
                        <label class="check-row">
                            <input type="checkbox" bind:checked={assistantFailureRetryable} />
                            <span>재시도 가능</span>
                        </label>
                        <button
                            type="button"
                            disabled={busy}
                            onclick={() =>
                                void controller.recordProviderDiscoveryAssistantFailure(
                                    assistantFailureKind,
                                    assistantFailureRetryable,
                                )}
                        >
                            실패 상태 기록
                        </button>
                    </details>
                </section>
            {/if}

            {#if actionKind === 'select_template'}
                <div class="action-block">
                    <h4>템플릿 후보 검토</h4>
                    {#each workspace.discovery_candidates as candidate (candidate.id)}
                        <button
                            type="button"
                            disabled={busy}
                            onclick={() =>
                                void continueWith({
                                    kind: 'select_template',
                                    candidate_id: candidate.id,
                                })}
                        >
                            {candidateLabel(candidate.summary)} 선택
                        </button>
                    {/each}
                    <button
                        type="button"
                        disabled={busy}
                        onclick={() => void continueWith({ kind: 'continue_without_template' })}
                    >
                        템플릿 없이 계속
                    </button>
                </div>
            {/if}

            {#if actionKind === 'supply_more_evidence'}
                <div class="action-block">
                    <h4>추가 근거 제출</h4>
                    <form
                        onsubmit={(event) => {
                            event.preventDefault();
                            void submitDocumentEvidence();
                        }}
                    >
                        <label>
                            <span>공식 문서 URL</span>
                            <input bind:value={documentEvidenceUrl} type="url" required />
                        </label>
                        <button type="submit" disabled={busy}>문서 근거 제출</button>
                    </form>
                    <form
                        onsubmit={(event) => {
                            event.preventDefault();
                            void submitCurlEvidence();
                        }}
                    >
                        <p>민감한 cURL을 클립보드에 복사한 뒤 네이티브 캡처로 제출하세요.</p>
                        <button type="submit" disabled={busy}> 클립보드 cURL 근거 캡처 </button>
                    </form>
                    <button
                        type="button"
                        disabled={busy || selectedSession.preferred_assistant === null}
                        onclick={() => void continueWith({ kind: 'request_assistant' })}
                    >
                        설정 도우미 요청
                    </button>
                    {#if selectedSession.preferred_assistant === null}
                        <small class="wide">
                            이 세션에는 설정 도우미 모델이 지정되지 않았습니다.
                        </small>
                    {/if}
                </div>
            {/if}

            {#if actionKind === 'approve_assistant' && workspace.discovery_approval_proposal}
                {@const proposal = workspace.discovery_approval_proposal}
                <div class="action-block">
                    <h4>도우미 권한 검토</h4>
                    <pre>{JSON.stringify(proposal.grant, null, 2)}</pre>
                    <button
                        class="primary"
                        type="button"
                        disabled={busy}
                        onclick={() =>
                            void continueWith({
                                kind: 'approve_assistant',
                                approval_id: proposal.id,
                                approval_grant_sha256: proposal.grant_sha256,
                            })}
                    >
                        이 권한만 승인
                    </button>
                    <button
                        type="button"
                        disabled={busy}
                        onclick={() => void continueWith({ kind: 'decline_assistant' })}
                    >
                        도우미 거절
                    </button>
                </div>
            {/if}

            {#if selectedCredentialTarget !== null && selectedSession.state !== 'committing' && selectedCredentialStatus !== 'available'}
                <div class="action-block">
                    <h4>자격증명 준비</h4>
                    <p>
                        {selectedCredentialStatus === 'unreadable'
                            ? '현재 세션과 자격증명 바인딩을 확인할 수 없습니다. 다시 캡처해 주세요.'
                            : '다음 인증 단계 전에 자격증명을 네이티브 저장소에 캡처해 주세요.'}
                    </p>
                    <button
                        type="button"
                        disabled={busy}
                        onclick={() => void captureDiscoveryCredential()}
                    >
                        자격증명 네이티브 캡처
                    </button>
                </div>
            {/if}

            {#if actionKind === 'approve_credential_origin' && workspace.discovery_approval_proposal}
                {@const proposal = workspace.discovery_approval_proposal}
                <div class="action-block">
                    <h4>자격증명 origin 검토</h4>
                    <pre>{JSON.stringify(proposal.grant, null, 2)}</pre>
                    <button
                        class="primary"
                        type="button"
                        disabled={busy ||
                            (actionNeedsCredential && selectedCredentialStatus !== 'available')}
                        onclick={() =>
                            void continueWith({
                                kind: 'approve_credential_origin',
                                approval_id: proposal.id,
                            })}
                    >
                        표시된 origin 승인
                    </button>
                </div>
            {/if}

            {#if actionKind === 'approve_probes' && workspace.discovery_approval_proposal}
                {@const proposal = workspace.discovery_approval_proposal}
                <div class="action-block">
                    <h4>제한된 capability probe 검토</h4>
                    <pre>{JSON.stringify(proposal.grant, null, 2)}</pre>
                    <button
                        class="primary"
                        type="button"
                        disabled={busy ||
                            (actionNeedsCredential && selectedCredentialStatus !== 'available')}
                        onclick={() =>
                            void continueWith({
                                kind: 'approve_probes',
                                approval_id: proposal.id,
                                approval_grant_sha256: proposal.grant_sha256,
                            })}
                    >
                        표시된 probe만 승인
                    </button>
                    <button
                        type="button"
                        disabled={busy}
                        onclick={() => void continueWith({ kind: 'skip_probes' })}
                    >
                        Probe 건너뛰기
                    </button>
                </div>
            {/if}

            {#if actionKind === 'review' && workspace.discovery_review_proposal}
                {@const proposal = workspace.discovery_review_proposal}
                <div class="action-block review-block">
                    <h4>최종 변경 검토</h4>
                    <p>
                        변경 {proposal.review.changes.length}개 · 경고
                        {proposal.review.warning_count}개 · 미해결
                        {proposal.review.unresolved_question_count}개
                    </p>
                    <ul>
                        {#each proposal.review.changes as change (change.target_kind + change.target_id)}
                            <li>{change.kind} · {change.target_kind} · {change.target_id}</li>
                        {/each}
                    </ul>
                    <code>{proposal.commit_plan_sha256}</code>
                    <button
                        class="primary"
                        type="button"
                        disabled={busy || proposal.review.unresolved_question_count > 0}
                        onclick={() =>
                            void continueWith({
                                kind: 'approve_review',
                                approval_id: proposal.approval.id,
                                commit_attempt_id: proposal.commit_attempt_id,
                                commit_plan_sha256: proposal.commit_plan_sha256,
                                graph_sha256: proposal.review.graph_sha256,
                            })}
                    >
                        검토한 정확한 계획 승인
                    </button>
                </div>
            {/if}

            {#if actionKind === 'restart_interrupted'}
                <div class="action-block">
                    <p>중단된 네트워크 작업은 자동 재실행하지 않습니다.</p>
                    <button
                        type="button"
                        disabled={busy ||
                            (actionNeedsCredential && selectedCredentialStatus !== 'available')}
                        onclick={() => void continueWith({ kind: 'restart_interrupted' })}
                    >
                        중단 작업 명시적으로 재개
                    </button>
                </div>
            {/if}

            {#if actionKind === 'reconcile_unknown_outcome' && workspace.discovery_approval_proposal}
                {@const proposal = workspace.discovery_approval_proposal}
                <div class="action-block">
                    <h4>알 수 없는 결과 수동 확정</h4>
                    <select bind:value={unknownResolution}>
                        <option value="confirmed_no_effect">외부 효과 없음 확인</option>
                        <option value="confirmed_compensated">보상 완료 확인</option>
                        <option value="manually_reconciled_as_failed">실패로 수동 정리</option>
                    </select>
                    <button
                        type="button"
                        disabled={busy}
                        onclick={() =>
                            void continueWith({
                                kind: 'resolve_unknown_outcome',
                                approval_id: proposal.id,
                                resolution: { resolution: unknownResolution },
                            })}
                    >
                        선택한 결과로 확정
                    </button>
                </div>
            {/if}

            {#if selectedSession.commit_attempt_id !== null && workspace.discovery_compensation_steps.length > 0}
                <div class="action-block">
                    <h4>보상 단계</h4>
                    <ul>
                        {#each workspace.discovery_compensation_steps as step (step.id)}
                            <li>{step.ordinal}. {step.kind} · {step.status}</li>
                        {/each}
                    </ul>
                    <button
                        type="button"
                        disabled={busy}
                        onclick={() => void controller.continueProviderDiscoveryCompensation(false)}
                    >
                        보상 계속
                    </button>
                    <button
                        type="button"
                        disabled={busy}
                        onclick={() => void controller.continueProviderDiscoveryCompensation(true)}
                    >
                        보상 작업 재개
                    </button>
                </div>
            {/if}

            {#if selectedSession.review !== null && selectedSession.committed_connection_id === null && actionKind === null}
                <form
                    class="action-block"
                    aria-label="탐색 결과 적용"
                    onsubmit={(event) => {
                        event.preventDefault();
                        void commitDiscovery();
                    }}
                >
                    {#if selectedCredentialTarget !== null}
                        <p>
                            자격증명을 클립보드에 복사한 뒤 네이티브 저장소로 캡처하고, 그 다음
                            승인된 연결을 적용하세요.
                        </p>
                        <button
                            type="button"
                            disabled={busy || selectedCredentialStatus === 'available'}
                            onclick={() => void captureDiscoveryCredential()}
                        >
                            자격증명 네이티브 캡처
                        </button>
                    {/if}
                    <button
                        class="primary"
                        type="submit"
                        disabled={busy ||
                            (selectedCredentialTarget !== null &&
                                selectedCredentialStatus !== 'available')}
                    >
                        승인된 연결 적용
                    </button>
                </form>
            {/if}

            <details>
                <summary>근거·승인 이력</summary>
                <p>
                    후보 {workspace.discovery_candidates.length}개 · 근거
                    {workspace.discovery_evidence.length}개 · 승인
                    {workspace.discovery_approvals.length}개
                </p>
            </details>
        </article>
    {/if}
</section>

<style>
    .workflow-section {
        padding: 20px;
        border: 1px solid var(--line);
        border-radius: var(--radius-md);
        background: var(--surface-raised);
    }

    .workflow-heading,
    .workflow-toolbar,
    .workflow-card > header {
        display: flex;
        gap: 12px;
        align-items: center;
        justify-content: space-between;
    }

    .workflow-heading h2,
    .workflow-card h3,
    .action-block h4 {
        margin: 3px 0;
    }

    .workflow-heading p:last-child,
    .workflow-card header p,
    .notice {
        margin: 5px 0 0;
        color: var(--ink-muted);
        line-height: 1.45;
    }

    .workflow-form {
        display: grid;
        grid-template-columns: repeat(2, minmax(0, 1fr));
        gap: 12px;
        margin-top: 18px;
    }

    label {
        display: grid;
        gap: 6px;
        color: var(--ink-muted);
        font-size: 0.78rem;
        font-weight: 700;
    }

    .wide {
        grid-column: 1 / -1;
    }

    .check-row {
        display: flex;
        align-items: center;
    }

    .workflow-toolbar {
        margin-top: 18px;
    }

    .workflow-toolbar label {
        flex: 1;
    }

    .workflow-card {
        margin-top: 16px;
        padding: 16px;
        border: 1px solid var(--line);
        border-radius: 14px;
        background: var(--surface);
    }

    .status-grid {
        display: grid;
        grid-template-columns: repeat(3, minmax(0, 1fr));
        gap: 8px;
        margin: 14px 0;
    }

    .status-grid > div,
    .action-block {
        padding: 12px;
        border-radius: 12px;
        background: var(--surface-sunken);
    }

    dt {
        color: var(--ink-muted);
        font-size: 0.7rem;
    }

    dd {
        margin: 3px 0 0;
        overflow-wrap: anywhere;
        font-weight: 800;
    }

    .action-block {
        display: flex;
        gap: 8px;
        align-items: flex-start;
        margin-top: 10px;
        flex-wrap: wrap;
    }

    .action-block h4,
    .action-block p,
    .action-block ul,
    .action-block pre,
    .action-block code,
    .action-block form,
    .action-block label {
        width: 100%;
    }

    pre,
    code {
        max-height: 190px;
        padding: 9px;
        overflow: auto;
        border-radius: 8px;
        background: var(--surface);
        font-size: 0.72rem;
        white-space: pre-wrap;
        overflow-wrap: anywhere;
    }

    .action-block form {
        display: grid;
        gap: 8px;
    }

    .draft-review,
    .host-action {
        display: grid;
        width: 100%;
        gap: 4px;
        padding: 10px;
        border-radius: 9px;
        background: var(--surface);
    }

    .draft-review span,
    .draft-review small,
    .host-action span {
        color: var(--ink-muted);
    }

    .assistant-failure {
        width: 100%;
    }

    .assistant-failure label,
    .assistant-failure button {
        margin-top: 8px;
    }

    details {
        margin-top: 12px;
        color: var(--ink-muted);
    }

    @container view (max-width: 640px) {
        .workflow-heading,
        .workflow-toolbar,
        .workflow-card > header {
            align-items: stretch;
            flex-direction: column;
        }

        .workflow-form,
        .status-grid {
            grid-template-columns: 1fr;
        }
    }
</style>
