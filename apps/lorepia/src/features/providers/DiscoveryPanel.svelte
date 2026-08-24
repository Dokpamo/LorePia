<script lang="ts">
    import { tick } from 'svelte';
    import DetailActionBar from '../../components/detail/DetailActionBar.svelte';
    import DetailPage from '../../components/detail/DetailPage.svelte';
    import { tr } from '../../lib/i18n';
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
        nestedPage?: string | null;
        nestedTitle?: string;
    }

    let {
        appState,
        controller,
        nestedPage = $bindable(null),
        nestedTitle = $bindable(''),
    }: Props = $props();
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
    const routedSessionId = $derived(
        nestedPage?.startsWith('session:') ? nestedPage.slice('session:'.length) : null,
    );
    const selectedSession = $derived(
        workspace.discoveries.find((session) => session.id === routedSessionId) ?? null,
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

    async function run(action: () => Promise<unknown>): Promise<void> {
        if (busy) return;
        busy = true;
        try {
            await action();
        } finally {
            busy = false;
        }
    }

    async function startDiscovery(): Promise<void> {
        if (busy || connectionId.trim() === '' || displayName.trim() === '') return;
        const previousSessionId = workspace.selected_discovery_id;
        busy = true;
        try {
            let started: boolean;
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
                started = await controller.beginProviderDiscovery({
                    kind: 'curl',
                    input,
                });
            } else {
                if (
                    siteUrl.trim() === '' ||
                    (sourceMode === 'known_provider' && templateId === '')
                ) {
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
                started = await controller.beginProviderDiscovery({ kind: 'site', input });
            }
            if (!started) return;
            await tick();
            const sessionId = workspace.selected_discovery_id;
            const session = workspace.discoveries.find((candidate) => candidate.id === sessionId);
            if (sessionId !== null && sessionId !== previousSessionId && session) {
                nestedTitle = session.display_name;
                nestedPage = `session:${sessionId}`;
            }
        } finally {
            busy = false;
        }
    }

    function beginCreate(): void {
        sourceMode = 'site';
        connectionId = '';
        displayName = '';
        siteUrl = '';
        docsUrl = '';
        templateId = '';
        preferredAssistantId = '';
        credentialRequested = false;
        nestedTitle = $tr('settings.page.discovery.create');
        nestedPage = 'create';
    }

    async function continueWith(action: ContinueProviderDiscoveryActionInput): Promise<void> {
        if (busy) return;
        busy = true;
        try {
            await controller.continueProviderDiscovery(action);
        } finally {
            busy = false;
        }
    }

    async function submitDocumentEvidence(): Promise<void> {
        if (busy) return;
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
        if (busy) return;
        busy = true;
        try {
            await controller.supplyProviderDiscoveryCurlEvidence();
        } finally {
            busy = false;
        }
    }

    async function commitDiscovery(): Promise<void> {
        if (busy) return;
        busy = true;
        try {
            await controller.commitProviderDiscovery();
        } finally {
            busy = false;
        }
    }

    async function captureDiscoveryCredential(): Promise<void> {
        const target = selectedCredentialTarget;
        if (busy || target === null) return;
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

    async function openSession(sessionId: string, title: string): Promise<void> {
        if (busy) return;
        busy = true;
        try {
            await controller.refreshProviderDiscovery(sessionId);
            if (nestedTitle !== title) nestedTitle = title;
            nestedPage = `session:${sessionId}`;
        } finally {
            busy = false;
        }
    }

    $effect(() => {
        if (nestedPage === 'create') {
            const title = $tr('settings.page.discovery.create');
            if (nestedTitle !== title) nestedTitle = title;
            return;
        }
        if (nestedPage === null) {
            if (nestedTitle !== '') nestedTitle = '';
            return;
        }
        if (routedSessionId === null) return;
        const session = workspace.discoveries.find((candidate) => candidate.id === routedSessionId);
        if (session) {
            if (nestedTitle !== session.display_name) nestedTitle = session.display_name;
            return;
        }
        nestedPage = null;
        nestedTitle = '';
    });
</script>

{#snippet content()}
    <section class="workflow-section" aria-label="프로바이더 탐색">
        {#if nestedPage === 'create'}
            <form
                id="provider-discovery-start"
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
                        cURL을 클립보드에 복사한 뒤 탐색 시작을 누르세요. 네이티브 계층이 한 번만
                        캡처하고 WebView에는 원문을 전달하지 않습니다.
                    </p>
                {/if}
                <label class="check-row">
                    <input type="checkbox" bind:checked={credentialRequested} />
                    <span>운영체제 자격증명 슬롯 필요</span>
                </label>
            </form>
        {:else if routedSessionId === null}
            {#if workspace.discoveries.length === 0}
                <p class="notice">저장된 탐색 기록이 없습니다.</p>
            {:else}
                <div class="setting-list session-list" aria-label="저장된 탐색 세션">
                    {#each workspace.discoveries as session (session.id)}
                        <button
                            class="setting-row session-row"
                            type="button"
                            disabled={busy}
                            onclick={() => void openSession(session.id, session.display_name)}
                        >
                            <span>
                                <strong>{session.display_name}</strong>
                                <small>{session.state} · revision {session.revision}</small>
                            </span>
                        </button>
                    {/each}
                </div>
            {/if}
        {:else if selectedSession}
            <article class="workflow-card">
                <header>
                    <p>{selectedSession.state} · revision {selectedSession.revision}</p>
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
                                정책으로 계산할 때까지 사용할 수 없습니다. 수동 입력과 결정론적
                                탐색은 계속 사용할 수 있습니다.
                            </p>
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
                                type="button"
                                disabled={busy}
                                onclick={() =>
                                    void run(() =>
                                        controller.requestProviderDiscoveryAssistantRevision(),
                                    )}
                            >
                                초안 수정 요청
                            </button>
                        {:else if assistantBoundary.action === 'wait_for_assistant_outcome'}
                            <p>외부 요청 결과를 모르면 자동 재시도하지 않습니다.</p>
                            <button
                                type="button"
                                disabled={busy}
                                onclick={() =>
                                    void run(() =>
                                        controller.interruptProviderDiscoveryAssistant(
                                            'confirmed_no_external_effect',
                                        ),
                                    )}
                            >
                                외부 효과 없음 확인 후 중단
                            </button>
                            <button
                                class="danger"
                                type="button"
                                disabled={busy}
                                onclick={() =>
                                    void run(() =>
                                        controller.interruptProviderDiscoveryAssistant(
                                            'external_outcome_unknown',
                                        ),
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
                                    void run(() =>
                                        controller.recordProviderDiscoveryAssistantFailure(
                                            assistantFailureKind,
                                            assistantFailureRetryable,
                                        ),
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
                    </div>
                {/if}

                {#if actionKind === 'approve_probes' && workspace.discovery_approval_proposal}
                    {@const proposal = workspace.discovery_approval_proposal}
                    <div class="action-block">
                        <h4>제한된 capability probe 검토</h4>
                        <pre>{JSON.stringify(proposal.grant, null, 2)}</pre>
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
                    </div>
                {/if}

                {#if actionKind === 'restart_interrupted'}
                    <div class="action-block">
                        <p>중단된 네트워크 작업은 자동 재실행하지 않습니다.</p>
                    </div>
                {/if}

                {#if actionKind === 'reconcile_unknown_outcome' && workspace.discovery_approval_proposal}
                    <div class="action-block">
                        <h4>알 수 없는 결과 수동 확정</h4>
                        <select bind:value={unknownResolution}>
                            <option value="confirmed_no_effect">외부 효과 없음 확인</option>
                            <option value="confirmed_compensated">보상 완료 확인</option>
                            <option value="manually_reconciled_as_failed">실패로 수동 정리</option>
                        </select>
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
                            onclick={() =>
                                void run(() =>
                                    controller.continueProviderDiscoveryCompensation(false),
                                )}
                        >
                            보상 계속
                        </button>
                        <button
                            type="button"
                            disabled={busy}
                            onclick={() =>
                                void run(() =>
                                    controller.continueProviderDiscoveryCompensation(true),
                                )}
                        >
                            보상 작업 재개
                        </button>
                    </div>
                {/if}

                {#if selectedSession.review !== null && selectedSession.committed_connection_id === null && actionKind === null}
                    <div class="action-block" aria-label="탐색 결과 적용">
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
                    </div>
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
        {:else}
            <p class="notice" role="status">선택한 탐색 기록을 찾을 수 없습니다.</p>
        {/if}
    </section>
{/snippet}

{#snippet actions()}
    {#if nestedPage === null}
        <DetailActionBar ariaLabel="프로바이더 탐색 목록 작업">
            <button
                class="detail-action detail-action--borderless"
                type="button"
                disabled={busy}
                onclick={() => void run(() => controller.recoverProviderDiscoveries())}
                >중단 작업 복구</button
            >
            <button
                class="primary detail-action detail-action--grow"
                type="button"
                disabled={busy}
                onclick={beginCreate}>새 탐색</button
            >
        </DetailActionBar>
    {:else if nestedPage === 'create'}
        <DetailActionBar ariaLabel="프로바이더 탐색 시작 작업">
            <button
                class="primary detail-action detail-action--wide"
                type="submit"
                form="provider-discovery-start"
                disabled={busy}>탐색 시작</button
            >
        </DetailActionBar>
    {:else if selectedSession && !terminalState(selectedSession.state)}
        <DetailActionBar ariaLabel="프로바이더 탐색 세션 작업">
            <button
                class="danger detail-action detail-action--destructive detail-action--borderless"
                type="button"
                disabled={busy}
                onclick={() => void run(() => controller.cancelProviderDiscovery())}
                >탐색 취소</button
            >

            {#if actionKind === 'approve_assistant' && workspace.discovery_approval_proposal}
                {@const proposal = workspace.discovery_approval_proposal}
                <button
                    class="primary detail-action detail-action--grow"
                    type="button"
                    disabled={busy}
                    onclick={() =>
                        void continueWith({
                            kind: 'approve_assistant',
                            approval_id: proposal.id,
                            approval_grant_sha256: proposal.grant_sha256,
                        })}>이 권한만 승인</button
                >
            {:else if actionKind === 'approve_credential_origin' && workspace.discovery_approval_proposal}
                {@const proposal = workspace.discovery_approval_proposal}
                <button
                    class="primary detail-action detail-action--grow"
                    type="button"
                    disabled={busy ||
                        (actionNeedsCredential && selectedCredentialStatus !== 'available')}
                    onclick={() =>
                        void continueWith({
                            kind: 'approve_credential_origin',
                            approval_id: proposal.id,
                        })}>표시된 origin 승인</button
                >
            {:else if actionKind === 'approve_probes' && workspace.discovery_approval_proposal}
                {@const proposal = workspace.discovery_approval_proposal}
                <button
                    class="primary detail-action detail-action--grow"
                    type="button"
                    disabled={busy ||
                        (actionNeedsCredential && selectedCredentialStatus !== 'available')}
                    onclick={() =>
                        void continueWith({
                            kind: 'approve_probes',
                            approval_id: proposal.id,
                            approval_grant_sha256: proposal.grant_sha256,
                        })}>표시된 probe만 승인</button
                >
            {:else if actionKind === 'review' && workspace.discovery_review_proposal}
                {@const proposal = workspace.discovery_review_proposal}
                <button
                    class="primary detail-action detail-action--grow"
                    type="button"
                    disabled={busy || proposal.review.unresolved_question_count > 0}
                    onclick={() =>
                        void continueWith({
                            kind: 'approve_review',
                            approval_id: proposal.approval.id,
                            commit_attempt_id: proposal.commit_attempt_id,
                            commit_plan_sha256: proposal.commit_plan_sha256,
                            graph_sha256: proposal.review.graph_sha256,
                        })}>검토한 정확한 계획 승인</button
                >
            {:else if actionKind === 'restart_interrupted'}
                <button
                    class="primary detail-action detail-action--grow"
                    type="button"
                    disabled={busy ||
                        (actionNeedsCredential && selectedCredentialStatus !== 'available')}
                    onclick={() => void continueWith({ kind: 'restart_interrupted' })}
                    >중단 작업 명시적으로 재개</button
                >
            {:else if actionKind === 'reconcile_unknown_outcome' && workspace.discovery_approval_proposal}
                {@const proposal = workspace.discovery_approval_proposal}
                <button
                    class="primary detail-action detail-action--grow"
                    type="button"
                    disabled={busy}
                    onclick={() =>
                        void continueWith({
                            kind: 'resolve_unknown_outcome',
                            approval_id: proposal.id,
                            resolution: { resolution: unknownResolution },
                        })}>선택한 결과로 확정</button
                >
            {:else if selectedSession.review !== null && selectedSession.committed_connection_id === null && actionKind === null}
                <button
                    class="primary detail-action detail-action--grow"
                    type="button"
                    disabled={busy ||
                        (selectedCredentialTarget !== null &&
                            selectedCredentialStatus !== 'available')}
                    onclick={() => void commitDiscovery()}>승인된 연결 적용</button
                >
            {:else if assistantBoundary?.action === 'resume_core_host_action'}
                <button
                    class="primary detail-action detail-action--grow"
                    type="button"
                    disabled={busy}
                    onclick={() =>
                        void run(() => controller.resumeProviderDiscoveryAssistantCoreHostAction())}
                    >저장된 Core 작업 재개</button
                >
            {:else if assistantBoundary?.action === 'approve_retry'}
                <button
                    class="primary detail-action detail-action--grow"
                    type="button"
                    disabled={busy}
                    onclick={() =>
                        void run(() => controller.approveProviderDiscoveryAssistantRetry())}
                    >도우미 재시도 승인</button
                >
            {:else if assistantBoundary?.action === 'review_draft' && assistantBoundary.draft_review}
                <button
                    class="primary detail-action detail-action--grow"
                    type="button"
                    disabled={busy ||
                        assistantBoundary.draft_review.unresolved_conflicts.length > 0 ||
                        assistantBoundary.draft_review.draft.unresolved_questions.length > 0}
                    onclick={() =>
                        void run(() => controller.acceptProviderDiscoveryAssistantDraft())}
                    >검토한 도우미 초안 채택</button
                >
            {:else if assistantBoundary?.action === 'restart_interrupted'}
                <button
                    class="primary detail-action detail-action--grow"
                    type="button"
                    disabled={busy}
                    onclick={() =>
                        void run(() =>
                            controller.restartProviderDiscoveryAssistantAfterInterruption(),
                        )}>도우미 중단 지점에서 명시적 재시작</button
                >
            {:else}
                <button
                    class="primary detail-action detail-action--grow"
                    type="button"
                    disabled={busy}
                    onclick={() => void run(() => controller.pollSelectedProviderDiscoveryEvents())}
                    >이벤트 확인</button
                >
            {/if}
        </DetailActionBar>
    {/if}
{/snippet}

<DetailPage
    ariaLabel="프로바이더 탐색"
    scrollClassName="provider-scroll settings-detail-scroll discovery-scroll"
    resetKey={nestedPage ?? 'discovery-root'}
    hasActions={nestedPage === null ||
        nestedPage === 'create' ||
        (selectedSession !== null && !terminalState(selectedSession.state))}
    {content}
    {actions}
/>

<style>
    .workflow-section {
        display: grid;
        width: 100%;
        min-width: 0;
        align-content: start;
        gap: 18px;
    }

    .workflow-card > header {
        display: flex;
        gap: 12px;
        align-items: center;
        justify-content: space-between;
    }

    .action-block h4 {
        margin: 0;
        font-size: var(--detail-support-type);
        font-weight: 700;
        line-height: 1.35;
    }

    .workflow-card header p,
    .notice {
        margin: 5px 0 0;
        color: var(--ink-muted);
        font-size: var(--detail-support-type);
        line-height: 1.45;
    }

    .workflow-form {
        display: grid;
        grid-template-columns: repeat(2, minmax(0, 1fr));
        gap: 14px;
    }

    label {
        display: grid;
        min-width: 0;
        gap: 7px;
        color: var(--ink-muted);
        font-size: var(--detail-support-type);
        font-weight: 700;
    }

    label :is(input:not([type='checkbox']), select),
    .action-block > select {
        width: 100%;
        min-width: 0;
        min-height: clamp(48px, 13.73vw, 60px);
        padding: clamp(12px, 3.432vw, 15px);
        border: 1.5px solid var(--line);
        border-radius: var(--radius-md);
        background: color-mix(in srgb, var(--surface-sunken) 26%, var(--surface-raised));
        box-shadow: inset 0 1px 2px rgb(16 18 24 / 3%);
        caret-color: var(--accent);
        color: var(--ink);
        font-size: var(--detail-support-type);
        line-height: 1.5;
        transition:
            background-color 140ms ease,
            box-shadow 140ms ease;
    }

    label :is(input:not([type='checkbox']), select):hover:not(:focus, :disabled),
    .action-block > select:hover:not(:focus, :disabled) {
        border-color: var(--line);
    }

    label :is(input:not([type='checkbox']), select):focus,
    .action-block > select:focus {
        border-color: var(--accent);
        outline: none;
    }

    button {
        min-height: clamp(44px, 12.586vw, 55px);
        border-radius: var(--radius-md);
        font-size: var(--detail-support-type);
        font-weight: 700;
    }

    .wide {
        grid-column: 1 / -1;
    }

    .check-row {
        display: flex;
        align-items: center;
        gap: 10px;
    }

    .check-row input {
        width: 20px;
        height: 20px;
        flex: none;
        margin: 0;
        accent-color: var(--accent);
    }

    .session-list {
        width: 100%;
        margin: 0;
    }

    .session-row > span {
        display: grid;
        min-width: 0;
        gap: 5px;
        text-align: left;
    }

    .session-row :is(strong, small) {
        overflow: hidden;
        color: var(--ink);
        font-size: var(--detail-support-type);
        font-weight: 550;
        line-height: 1.35;
        text-overflow: ellipsis;
    }

    .session-row small {
        color: var(--ink-muted);
        white-space: normal;
    }

    .workflow-card {
        display: grid;
        min-width: 0;
        gap: 14px;
    }

    .status-grid {
        display: grid;
        grid-template-columns: repeat(3, minmax(0, 1fr));
        margin: 0;
        border-block: 1px solid var(--line);
    }

    .status-grid > div {
        min-width: 0;
        padding: 12px;
    }

    .status-grid > div + div {
        border-left: 1px solid var(--line);
    }

    dt {
        color: var(--ink-muted);
        font-size: var(--detail-support-type);
    }

    dd {
        margin: 5px 0 0;
        overflow-wrap: anywhere;
        color: var(--ink);
        font-size: var(--detail-support-type);
        font-weight: 650;
    }

    .action-block {
        display: flex;
        min-width: 0;
        gap: 10px;
        align-items: flex-start;
        padding-top: 16px;
        border-top: 1px solid var(--line);
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
        max-height: none;
        padding: 12px;
        overflow-x: auto;
        overflow-y: visible;
        border: 0;
        border-radius: var(--radius-sm);
        background: color-mix(in srgb, var(--surface-sunken) 58%, transparent);
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
        gap: 5px;
        padding: 2px 0 2px 12px;
        border-left: 2px solid var(--line-strong);
        font-size: var(--detail-support-type);
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
        margin-top: 10px;
    }

    details {
        padding-top: 14px;
        border-top: 1px solid var(--line);
        margin-top: 0;
        color: var(--ink-muted);
        font-size: var(--detail-support-type);
    }

    summary {
        cursor: pointer;
        font-weight: 700;
    }

    @container view (max-width: 640px) {
        .workflow-card > header {
            align-items: stretch;
            flex-direction: column;
        }

        .workflow-form,
        .status-grid {
            grid-template-columns: 1fr;
        }

        .status-grid > div + div {
            border-top: 1px solid var(--line);
            border-left: 0;
        }

        .action-block > button,
        .action-block > form > button {
            width: 100%;
        }
    }
</style>
