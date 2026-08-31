<script lang="ts">
    import type { LorepiaAppState } from '../../../app/app-controller';
    import DetailActionBar from '../../../components/detail/DetailActionBar.svelte';
    import type {
        MemoryRecordDto,
        MemoryRecordSourceNavigationDto,
    } from '../../../lib/ipc/contracts';
    import type { OrchestrationController, OrchestrationState } from '../orchestration-controller';
    interface Props {
        appState: LorepiaAppState;
        orchestrationState: OrchestrationState;
        controller: OrchestrationController;
        detailPage?: string | null;
        onNavigateToMemorySource?: (source: MemoryRecordSourceNavigationDto) => void;
        memoryDrafts?: Record<string, string>;
        pendingMemoryDeleteId?: string | null;
        knowledgeSample?: string;
        transformRuleId?: string;
        transformSample?: string;
    }
    let {
        appState,
        orchestrationState,
        controller,
        detailPage = $bindable(null),
        onNavigateToMemorySource = () => undefined,
        memoryDrafts = $bindable({}),
        pendingMemoryDeleteId = $bindable(null),
        knowledgeSample = $bindable(''),
        transformRuleId = $bindable(''),
        transformSample = $bindable(''),
    }: Props = $props();

    const MAX_INLINE_ITEMS = 100;
    const MAX_PLAN_DETAILS = 300;
    const MEMORY_RECORD_EDITOR_PREFIX = 'records/edit/';
    const INTERACTION_REVIEW_PREFIX = 'interactions/review/';
    const selectedMemoryRecord = $derived(
        detailPage?.startsWith(MEMORY_RECORD_EDITOR_PREFIX)
            ? (orchestrationState.workspace.memory_records.find(
                  (record) => record.id === detailPage?.slice(MEMORY_RECORD_EDITOR_PREFIX.length),
              ) ?? null)
            : null,
    );
    const selectedInteractionProposal = $derived(
        detailPage?.startsWith(INTERACTION_REVIEW_PREFIX)
            ? (orchestrationState.workspace.interaction_proposals.find(
                  (proposal) =>
                      proposal.proposal.id === detailPage?.slice(INTERACTION_REVIEW_PREFIX.length),
              ) ?? null)
            : null,
    );
    function memoryDraft(record: MemoryRecordDto): string {
        return memoryDrafts[record.id] ?? record.summary;
    }
    function openMemoryRecord(record: MemoryRecordDto): void {
        pendingMemoryDeleteId = null;
        detailPage = `${MEMORY_RECORD_EDITOR_PREFIX}${record.id}`;
    }
    function openInteractionProposal(proposalId: string): void {
        detailPage = `${INTERACTION_REVIEW_PREFIX}${proposalId}`;
    }
    function clearMemoryDraft(recordId: string): void {
        memoryDrafts = Object.fromEntries(
            Object.entries(memoryDrafts).filter(([id]) => id !== recordId),
        );
    }
    async function saveMemorySummary(record: MemoryRecordDto): Promise<void> {
        if (await controller.updateMemoryRecord(record.id, { summary: memoryDraft(record) })) {
            clearMemoryDraft(record.id);
            detailPage = 'records';
        }
    }

    async function confirmMemoryDelete(recordId: string): Promise<void> {
        if (pendingMemoryDeleteId !== recordId) {
            pendingMemoryDeleteId = recordId;
            return;
        }
        if (await controller.deleteMemoryRecord(recordId)) {
            clearMemoryDraft(recordId);
            detailPage = 'records';
        }
        pendingMemoryDeleteId = null;
    }

    async function decideInteractionProposal(proposalId: string, approved: boolean): Promise<void> {
        if (await controller.decideProposal(proposalId, approved)) detailPage = 'interactions';
    }
</script>

<!-- prettier-ignore-start -->
{#if detailPage === 'records'}
    <section class="studio-card memory-records-page" aria-labelledby="memory-title">
        <div class="section-heading">
            <div>
                <h3 id="memory-title">장기기억</h3>
                <p>기억을 선택해 별도 화면에서 확인하고 편집합니다.</p>
            </div>
        </div>
        {#if appState.memory_supervisor.status !== null}
            <p
                class="bounded-note" class:error={appState.memory_supervisor.status.phase === 'failed'}
                role={appState.memory_supervisor.status.phase === 'failed' ? 'alert' : 'status'}
            >
                기억 작업
                {appState.memory_supervisor.status.phase === 'not_started'
                    ? '시작 전'
                    : appState.memory_supervisor.status.phase === 'recovered'
                      ? '중단 작업 복구 완료'
                      : appState.memory_supervisor.status.phase === 'running'
                        ? '감시 중'
                        : '확인 필요'}
                · 중단 복구
                {appState.memory_supervisor.status.recovered_interrupted_jobs}건 ·
                완료
                {appState.memory_supervisor.status.completed_jobs}건
            </p>
        {/if}
        {#if appState.memory_supervisor.error !== null}
            <p class="bounded-note error" role="alert">
                {appState.memory_supervisor.error}
            </p>
        {/if}
        {#if orchestrationState.list_truncation.memory_records}
            <p class="bounded-note">처음 250개 기억만 표시합니다.</p>
        {/if}
        {#if orchestrationState.workspace.memory_records.length === 0}
            <p class="empty-note">현재 분기에 저장된 장기기억이 없습니다.</p>
        {:else}
            <ul class="setting-list memory-record-list" aria-label="장기기억 목록">
                {#each orchestrationState.workspace.memory_records as record (record.id)}
                    <li>
                        <button
                            class="setting-row memory-record-row"
                            type="button"
                            onclick={() => openMemoryRecord(record)}
                        >
                            <span class="setting-content">
                                <span class="setting-copy">
                                    <strong>{record.title}</strong>
                                    <small>
                                        {record.kind} · 중요도 {record.importance}{record.pinned
                                            ? ' · 고정됨'
                                            : ''}
                                    </small>
                                    <small class="memory-record-summary">{record.summary}</small>
                                </span>
                            </span>
                        </button>
                    </li>
                {/each}
            </ul>
        {/if}
    </section>
{:else if selectedMemoryRecord !== null}
    <section
        class="studio-card memory-record-editor has-fixed-actions" data-studio-owned-fields=""
        aria-labelledby="memory-editor-title"
    >
        <h3 id="memory-editor-title" class="sr-only">장기기억 편집</h3>
        <dl class="memory-record-metadata" data-studio-owned-definition="">
            <div>
                <dt>제목</dt>
                <dd>{selectedMemoryRecord.title}</dd>
            </div>
            <div>
                <dt>종류</dt>
                <dd>{selectedMemoryRecord.kind}</dd>
            </div>
            <div>
                <dt>중요도</dt>
                <dd>{selectedMemoryRecord.importance}</dd>
            </div>
            {#if selectedMemoryRecord.keywords.length > 0}
                <div>
                    <dt>키워드</dt>
                    <dd>{selectedMemoryRecord.keywords.join(', ')}</dd>
                </div>
            {/if}
        </dl>
        {#if selectedMemoryRecord.invalidated_at}
            <p class="bounded-note">무효화된 기억입니다.</p>
        {/if}
        {#if selectedMemoryRecord.excluded_from_conversation}
            <p class="bounded-note">현재 대화 선택에서 제외되어 있습니다.</p>
        {/if}
        {#if selectedMemoryRecord.excluded_from_character}
            <p class="bounded-note">캐릭터 기억 선택에서 제외되어 있습니다.</p>
        {/if}
        <label>
            <span>요약</span>
            <textarea
                rows="5"
                maxlength="8192"
                value={memoryDraft(selectedMemoryRecord)}
                oninput={(event) =>
                    (memoryDrafts[selectedMemoryRecord.id] = event.currentTarget.value)}></textarea>
        </label>
        <div class="memory-record-controls" aria-label="장기기억 속성">
            <button
                type="button"
                aria-pressed={selectedMemoryRecord.pinned}
                onclick={() =>
                    void controller.setMemoryRecordPinned(
                        selectedMemoryRecord.id,
                        !selectedMemoryRecord.pinned,
                    )}
            >
                {selectedMemoryRecord.pinned ? '고정 해제' : '고정'}
            </button>
            <button
                type="button"
                aria-pressed={selectedMemoryRecord.excluded_from_conversation}
                onclick={() =>
                    void controller.setMemoryRecordExclusion(
                        selectedMemoryRecord.id,
                        'conversation',
                        !selectedMemoryRecord.excluded_from_conversation,
                    )}
            >
                {selectedMemoryRecord.excluded_from_conversation
                    ? '대화 제외 해제'
                    : '현재 대화에서 제외'}
            </button>
            <button
                type="button"
                aria-pressed={selectedMemoryRecord.excluded_from_character}
                onclick={() =>
                    void controller.setMemoryRecordExclusion(
                        selectedMemoryRecord.id,
                        'character',
                        !selectedMemoryRecord.excluded_from_character,
                    )}
            >
                {selectedMemoryRecord.excluded_from_character
                    ? '캐릭터 제외 해제'
                    : '캐릭터 기억에서 제외'}
            </button>
            <button
                type="button"
                onclick={() => onNavigateToMemorySource(selectedMemoryRecord.source_navigation)}
            >
                출처 메시지로 이동
            </button>
        </div>
        <DetailActionBar fixed ariaLabel="장기기억 편집 작업">
            {#if pendingMemoryDeleteId === selectedMemoryRecord.id}
                <button
                    class="danger detail-action detail-action--destructive"
                    type="button"
                    onclick={() => void confirmMemoryDelete(selectedMemoryRecord.id)}
                >
                    삭제 확인
                </button>
                <button
                    class="detail-action detail-action--grow"
                    type="button"
                    onclick={() => (pendingMemoryDeleteId = null)}
                >
                    취소
                </button>
            {:else}
                <button
                    class="detail-action detail-action--destructive detail-action--borderless"
                    type="button"
                    onclick={() => void confirmMemoryDelete(selectedMemoryRecord.id)}
                >
                    삭제
                </button>
                <button
                    class="primary detail-action detail-action--grow"
                    type="button"
                    onclick={() => void saveMemorySummary(selectedMemoryRecord)}
                >
                    저장
                </button>
            {/if}
        </DetailActionBar>
    </section>
{/if}
{#if detailPage === 'knowledge'}
    <section class="studio-card split-card has-fixed-actions" data-studio-owned-fields="" aria-labelledby="knowledge-title">
        <div>
            <div class="section-heading">
                <div>
                    <h3 id="knowledge-title">세계관 지식 시뮬레이터</h3>
                    <p>
                        입력에 어떤 항목이 선택되는지 실제 선택 근거로 확인합니다.
                    </p>
                </div>
            </div>
            <label>
                <span>검사할 문장</span>
                <textarea rows="4" maxlength="8192" bind:value={knowledgeSample}></textarea>
            </label>
        </div>
        <div>
            <h4>선택 결과</h4>
            {#if orchestrationState.knowledge_simulation === null}
                <p class="empty-note">아직 실행하지 않았습니다.</p>
            {:else}
                <p>
                    예상 {orchestrationState.knowledge_simulation
                        .total_estimated_tokens}
                    토큰
                </p>
                {#if orchestrationState.knowledge_simulation.truncated}
                    <p class="bounded-note" role="note">
                        Core의 안전한 응답 한도 또는 지식 예산 때문에 선택 근거
                        일부가 축약되었습니다. 이 결과를 전체 후보 목록으로 해석하지
                        마세요.
                    </p>
                {/if}
                <ul class="evidence-list">
                    {#each orchestrationState.knowledge_simulation.entries.slice(0, MAX_PLAN_DETAILS) as evidence (evidence.id)}
                        <li class:selected={evidence.selected}>
                            <strong>{evidence.title.slice(0, 512)}</strong>
                            <span>{evidence.reason.slice(0, 4096)}</span>
                            <small>
                                {evidence.source_kind} ·
                                {evidence.selected ? '선택' : '제외'} ·
                                {evidence.estimated_tokens} tokens · score
                                {evidence.score ?? '없음'} · 배치
                                {evidence.placement ?? '없음'}
                            </small>
                        </li>
                    {/each}
                </ul>
                {#if orchestrationState.knowledge_simulation.entries.length > MAX_PLAN_DETAILS}
                    <p class="bounded-note">처음 300개 선택 근거만 표시합니다.</p>
                {/if}
            {/if}
        </div>
        <DetailActionBar fixed ariaLabel="세계관 지식 시뮬레이션 작업">
            <button
                class="primary detail-action detail-action--wide"
                type="button"
                disabled={knowledgeSample.trim() === ''}
                onclick={() => void controller.simulateKnowledge(knowledgeSample)}
            >
                활성화 시뮬레이션
            </button>
        </DetailActionBar>
    </section>
{/if}
{#if detailPage === 'transforms'}
    <section class="studio-card split-card has-fixed-actions" data-studio-owned-fields="" data-studio-owned-lists="" data-studio-owned-code="" aria-labelledby="transform-title">
        <div>
            <div class="section-heading">
                <div>
                    <h3 id="transform-title">안전한 변환 미리보기</h3>
                    <p>저장된 원문은 유지하며 오류가 나면 원문을 사용합니다.</p>
                </div>
            </div>
            <label>
                <span>규칙 ID</span>
                <input type="text" maxlength="256" bind:value={transformRuleId} />
            </label>
            <label>
                <span>합성 테스트 입력</span>
                <textarea rows="5" maxlength="16384" bind:value={transformSample}></textarea>
            </label>
        </div>
        <div class="diff-preview">
            <h4>변환 전후</h4>
            {#if orchestrationState.transform_preview === null}
                <p class="empty-note">미리보기 결과가 없습니다.</p>
            {:else}
                <p class="bounded-note" role="note">
                    출처 set <code
                        >{orchestrationState.transform_preview
                            .transform_set_id}</code
                    >
                    · rule
                    <code>{orchestrationState.transform_preview.rule_id}</code>
                    ·
                    {orchestrationState.transform_preview.phase} ·
                    {orchestrationState.transform_preview.changed
                        ? '변경됨'
                        : '변경 없음'} ·
                    {orchestrationState.transform_preview.rendering}
                </p>
                <div>
                    <strong>입력</strong>
                    <pre>{orchestrationState.transform_preview.input.slice(0, 16000)}</pre>
                </div>
                <div>
                    <strong>출력</strong>
                    <pre>{orchestrationState.transform_preview.output.slice(0, 16000)}</pre>
                </div>
                {#if orchestrationState.transform_preview.used_original}
                    <p class="bounded-note">
                        변환 오류로 byte-identical 원문을 유지했습니다.
                    </p>
                {/if}
                {#each orchestrationState.transform_preview.diagnostics.slice(0, MAX_INLINE_ITEMS) as diagnostic, index (`${String(index)}:${diagnostic}`)}
                    <p class="inline-diagnostic">{diagnostic.slice(0, 4096)}</p>
                {/each}
                {#if orchestrationState.transform_preview.diagnostics.length > MAX_INLINE_ITEMS}
                    <p class="bounded-note">처음 100개 진단만 표시합니다.</p>
                {/if}
                {#if orchestrationState.transform_preview.reports.length > 0}
                    <h5>규칙별 진단</h5>
                    <ol class="compact-list">
                        {#each orchestrationState.transform_preview.reports.slice(0, MAX_INLINE_ITEMS) as report, index (`${String(index)}:${report.trace.rule_id}`)}
                            <li>
                                <strong>{report.trace.rule_id} · {report.status}</strong>
                                <span>
                                    치환 {report.trace.replacements}회 ·
                                    {report.trace.input_chars} → {report.trace
                                        .output_chars}자
                                </span>
                                {#if report.trace.error !== null}
                                    <span class="inline-diagnostic"
                                        >{report.trace.error.slice(0, 4096)}</span
                                    >
                                {/if}
                                {#if report.diff !== null}
                                    <small>
                                        diff 앞 {report.diff
                                            .unchanged_prefix_chars}자 · 뒤
                                        {report.diff
                                            .unchanged_suffix_chars}자{report.diff
                                            .truncated
                                            ? ' · fragment 축약'
                                            : ''}
                                    </small>
                                {/if}
                            </li>
                        {/each}
                    </ol>
                {/if}
                {#if orchestrationState.transform_preview.error !== null}
                    <p class="inline-diagnostic" role="alert">
                        {orchestrationState.transform_preview.error.code}: {orchestrationState.transform_preview.error.message.slice(
                            0,
                            4096,
                        )}
                    </p>
                {/if}
                {#if orchestrationState.transform_preview.truncated}
                    <p class="bounded-note" role="note">
                        Core의 안전한 표시 한도에 따라 변환 입력, 출력, diff 또는
                        진단 일부가 축약되었습니다.
                    </p>
                {/if}
            {/if}
        </div>
        <DetailActionBar fixed ariaLabel="안전한 변환 미리보기 작업">
            <button
                class="primary detail-action detail-action--wide"
                type="button"
                disabled={transformRuleId === '' || transformSample === ''}
                onclick={() => void controller.previewTransform(transformRuleId, transformSample)}
            >
                변환 diff 만들기
            </button>
        </DetailActionBar>
    </section>
{/if}
{#if detailPage === 'interactions'}
    <section class="studio-card interactions-page" aria-labelledby="interactions-title">
        <div class="section-heading">
            <div>
                <h3 id="interactions-title">선언형 상호작용</h3>
                <p>상태를 확인하고 사용자 승인 제안을 별도 화면에서 검토합니다.</p>
            </div>
        </div>
        <section class="interaction-state-section" aria-labelledby="interaction-state-title">
            <h4 id="interaction-state-title">현재 상태</h4>
            {#if orchestrationState.workspace.interaction_state.length === 0}
                <p class="empty-note">표시할 상호작용 상태가 없습니다.</p>
            {:else}
                <dl class="interaction-state-list" data-studio-owned-definition="">
                    {#each orchestrationState.workspace.interaction_state.slice(0, 200) as entry (entry.id)}
                        <div>
                            <dt>{entry.label}</dt>
                            <dd>{JSON.stringify(entry.value).slice(0, 500)}</dd>
                        </div>
                    {/each}
                </dl>
            {/if}
            {#if orchestrationState.workspace.interaction_state.length > 200}
                <p class="bounded-note">처음 200개 상태만 표시합니다.</p>
            {/if}
        </section>
        <section class="interaction-proposal-section" aria-labelledby="interaction-proposals-title">
            <h4 id="interaction-proposals-title">사용자 승인 제안</h4>
            {#if orchestrationState.workspace.interaction_proposals.length === 0}
                <p class="empty-note">검토할 사용자 승인 제안이 없습니다.</p>
            {:else}
                <ul
                    class="setting-list interaction-proposal-list"
                    aria-label="사용자 승인 제안 목록"
                >
                    {#each orchestrationState.workspace.interaction_proposals.slice(0, 100) as proposal (proposal.proposal.id)}
                        <li>
                            <button
                                class="setting-row interaction-proposal-row"
                                type="button"
                                onclick={() => openInteractionProposal(proposal.proposal.id)}
                            >
                                <span class="setting-content">
                                    <span class="setting-copy">
                                        <strong>
                                            {proposal.proposal.projection_rejection_reason ===
                                            'unsafe_native_text'
                                                ? '저장 제안 내용을 표시할 수 없음'
                                                : proposal.proposal.title}
                                        </strong>
                                        <small>
                                            {proposal.proposal.status} · 상태 revision
                                            {proposal.state_revision} · 제안 revision
                                            {proposal.proposal_revision}
                                        </small>
                                    </span>
                                </span>
                            </button>
                        </li>
                    {/each}
                </ul>
            {/if}
            {#if orchestrationState.workspace.interaction_proposals.length > MAX_INLINE_ITEMS}
                <p class="bounded-note">처음 100개 제안만 표시합니다.</p>
            {/if}
        </section>
    </section>
{:else if selectedInteractionProposal !== null}
    <section
        class:has-fixed-actions={selectedInteractionProposal.proposal.status === 'pending'}
        class="studio-card interaction-review-page"
        aria-labelledby="interaction-review-title"
    >
        <h3 id="interaction-review-title" class="sr-only">상호작용 검토</h3>
        {#if selectedInteractionProposal.proposal.projection_rejection_reason === 'unsafe_native_text'}
            <strong>저장 제안 내용을 표시할 수 없음</strong>
            <p class="bounded-note">
                안전한 표시 범위를 벗어난 원문은 숨겼습니다. 이 제안은 거절만 할 수
                있습니다.
            </p>
        {:else}
            <div class="interaction-review-copy">
                <strong>{selectedInteractionProposal.proposal.title}</strong>
                <p>{selectedInteractionProposal.proposal.body}</p>
            </div>
        {/if}
        <dl class="interaction-review-metadata" data-studio-owned-definition="">
            <div>
                <dt>상태</dt>
                <dd>{selectedInteractionProposal.proposal.status}</dd>
            </div>
            <div>
                <dt>상태 revision</dt>
                <dd>{selectedInteractionProposal.state_revision}</dd>
            </div>
            <div>
                <dt>제안 revision</dt>
                <dd>{selectedInteractionProposal.proposal_revision}</dd>
            </div>
        </dl>
        {#if selectedInteractionProposal.proposal.status === 'pending'}
            <DetailActionBar fixed ariaLabel="상호작용 제안 검토 작업">
                <button
                    class="detail-action detail-action--destructive detail-action--borderless"
                    type="button"
                    disabled={orchestrationState.busy_interaction_proposal_id !== null}
                    onclick={() =>
                        void decideInteractionProposal(
                            selectedInteractionProposal.proposal.id,
                            false,
                        )}
                >
                    {orchestrationState.busy_interaction_proposal_id ===
                    selectedInteractionProposal.proposal.id
                        ? '반영 중…'
                        : '거절'}
                </button>
                <button
                    class="primary detail-action detail-action--grow"
                    type="button"
                    disabled={orchestrationState.busy_interaction_proposal_id !== null ||
                        selectedInteractionProposal.proposal.projection_rejection_reason ===
                            'unsafe_native_text'}
                    onclick={() =>
                        void decideInteractionProposal(
                            selectedInteractionProposal.proposal.id,
                            true,
                        )}
                >
                    {orchestrationState.busy_interaction_proposal_id ===
                    selectedInteractionProposal.proposal.id
                        ? '반영 중…'
                        : '승인'}
                </button>
            </DetailActionBar>
        {/if}
    </section>
{/if}
<!-- prettier-ignore-end -->
