<script lang="ts">
    import type { LorepiaAppState } from '../../../app/app-controller';
    import DetailActionBar from '../../../components/detail/DetailActionBar.svelte';
    import type { LorepiaClient, PromptPresetHistoryClientApi } from '../../../lib/ipc/contracts';
    import CreatorDocumentEditors from '../CreatorDocumentEditors.svelte';
    import type { ContentModuleLifecycleClientApi } from '../module-lifecycle-contracts';
    import {
        MAX_ROOM_PROMPT_NAME_CHARS,
        MAX_ROOM_PROMPT_TEMPLATE_SLOTS,
        MAX_ROOM_PROMPT_TEXT_CHARS,
        roomPromptSourceValidationError,
        type OrchestrationController,
        type OrchestrationState,
    } from '../orchestration-controller';
    import PromptPresetHistory from '../PromptPresetHistory.svelte';
    import TaskProfilesPanel from '../TaskProfilesPanel.svelte';
    import PromptBlocksSection from './PromptBlocksSection.svelte';

    interface Props {
        client?: LorepiaClient &
            Partial<PromptPresetHistoryClientApi & ContentModuleLifecycleClientApi>;
        appState: LorepiaAppState;
        orchestrationState: OrchestrationState;
        controller: OrchestrationController;
        detailPage?: string | null;
        blockSearch?: string;
        blockZoneFilter?: string;
        blockStatusFilter?: 'all' | 'enabled' | 'disabled';
        draggedBlockId?: string | null;
        blockJsonDrafts?: Record<string, string>;
        blockJsonErrors?: Record<string, string>;
    }

    let {
        client,
        appState,
        orchestrationState,
        controller,
        detailPage = $bindable(null),
        blockSearch = $bindable(''),
        blockZoneFilter = $bindable('all'),
        blockStatusFilter = $bindable<'all' | 'enabled' | 'disabled'>('all'),
        draggedBlockId = $bindable(null),
        blockJsonDrafts = $bindable({}),
        blockJsonErrors = $bindable({}),
    }: Props = $props();

    const roomPromptSourceError = $derived(
        roomPromptSourceValidationError(orchestrationState.workspace.room_config),
    );

    function addRoomTemplateSlot(): void {
        const slots = orchestrationState.workspace.room_config.template_slots;
        if (slots.length >= MAX_ROOM_PROMPT_TEMPLATE_SLOTS) return;
        controller.stageRoomConfig({ template_slots: [...slots, { name: '', value: '' }] });
    }

    function updateRoomTemplateSlot(index: number, field: 'name' | 'value', value: string): void {
        controller.stageRoomConfig({
            template_slots: orchestrationState.workspace.room_config.template_slots.map(
                (slot, slotIndex) => (slotIndex === index ? { ...slot, [field]: value } : slot),
            ),
        });
    }

    function removeRoomTemplateSlot(index: number): void {
        controller.stageRoomConfig({
            template_slots: orchestrationState.workspace.room_config.template_slots.filter(
                (_, slotIndex) => slotIndex !== index,
            ),
        });
    }
</script>

{#if (detailPage === 'history' || detailPage?.startsWith('history/review/')) && client}
    <PromptPresetHistory
        {client}
        bind:detailPage
        presetId={orchestrationState.workspace.room_config.prompt_preset_id}
        currentRevision={orchestrationState.workspace.prompt_preset_revision}
        disabled={orchestrationState.editable_prompt_preset_dirty}
        onApplied={() =>
            controller.loadContext(
                orchestrationState.workspace.room_config.conversation_id || null,
                orchestrationState.workspace.room_config.branch_id || null,
            )}
    />
{/if}

<PromptBlocksSection
    {orchestrationState}
    {controller}
    bind:detailPage
    bind:blockSearch
    bind:blockZoneFilter
    bind:blockStatusFilter
    bind:draggedBlockId
    bind:blockJsonDrafts
    bind:blockJsonErrors
/>

{#if detailPage === 'room'}
    <section class="studio-card prompt-room-page" aria-label="방별 프롬프트 소스">
        <div class="editor-grid prompt-source-grid" data-studio-owned-fields="">
            {#if orchestrationState.workspace.room_config.supported_fields.user_name_override}
                <label>
                    <span>사용자 표시 이름</span>
                    <input
                        type="text"
                        maxlength={MAX_ROOM_PROMPT_NAME_CHARS}
                        value={orchestrationState.workspace.room_config.user_name_override ?? ''}
                        placeholder="비어 있으면 선택한 페르소나 또는 로컬 별칭 사용"
                        oninput={(event) =>
                            controller.stageRoomConfig({
                                user_name_override:
                                    event.currentTarget.value === ''
                                        ? null
                                        : event.currentTarget.value,
                            })}
                    />
                </label>
            {/if}
            {#if orchestrationState.workspace.room_config.supported_fields.author_note}
                <label>
                    <span>작가 메모</span>
                    <textarea
                        rows="5"
                        maxlength={MAX_ROOM_PROMPT_TEXT_CHARS}
                        value={orchestrationState.workspace.room_config.author_note ?? ''}
                        placeholder="이 방에서 사용할 작가 지침"
                        oninput={(event) =>
                            controller.stageRoomConfig({
                                author_note:
                                    event.currentTarget.value === ''
                                        ? null
                                        : event.currentTarget.value,
                            })}></textarea>
                </label>
            {/if}
            {#if orchestrationState.workspace.room_config.supported_fields.group_context}
                <label>
                    <span>그룹 문맥</span>
                    <textarea
                        rows="5"
                        maxlength={MAX_ROOM_PROMPT_TEXT_CHARS}
                        value={orchestrationState.workspace.room_config.group_context ?? ''}
                        placeholder="참가자와 발화 규칙"
                        oninput={(event) =>
                            controller.stageRoomConfig({
                                group_context:
                                    event.currentTarget.value === ''
                                        ? null
                                        : event.currentTarget.value,
                            })}></textarea>
                </label>
            {/if}
        </div>

        {#if orchestrationState.workspace.room_config.supported_fields.template_slots}
            <fieldset class="room-template-slots" data-studio-owned-fields="">
                <legend>안전 템플릿 슬롯</legend>
                {#if orchestrationState.workspace.room_config.template_slots.length === 0}
                    <p class="empty-note">이 방에 정의된 템플릿 슬롯이 없습니다.</p>
                {:else}
                    <ol class="template-slot-list">
                        {#each orchestrationState.workspace.room_config.template_slots as slot, index (index)}
                            <li class="template-slot-row">
                                <label>
                                    <span>템플릿 슬롯 {index + 1} 이름</span>
                                    <input
                                        type="text"
                                        maxlength={MAX_ROOM_PROMPT_NAME_CHARS}
                                        value={slot.name}
                                        oninput={(event) =>
                                            updateRoomTemplateSlot(
                                                index,
                                                'name',
                                                event.currentTarget.value,
                                            )}
                                    />
                                </label>
                                <label>
                                    <span>템플릿 슬롯 {index + 1} 값</span>
                                    <textarea
                                        rows="3"
                                        maxlength={MAX_ROOM_PROMPT_TEXT_CHARS}
                                        value={slot.value}
                                        oninput={(event) =>
                                            updateRoomTemplateSlot(
                                                index,
                                                'value',
                                                event.currentTarget.value,
                                            )}></textarea>
                                </label>
                                <button
                                    type="button"
                                    aria-label={`템플릿 슬롯 ${String(index + 1)} 삭제`}
                                    disabled={orchestrationState.saving}
                                    onclick={() => removeRoomTemplateSlot(index)}
                                >
                                    삭제
                                </button>
                            </li>
                        {/each}
                    </ol>
                {/if}
            </fieldset>
        {/if}

        {#if roomPromptSourceError !== null}
            <p class="inline-diagnostic" role="alert">{roomPromptSourceError}</p>
        {:else if orchestrationState.announcement !== ''}
            <p class="bounded-note" role="status">{orchestrationState.announcement}</p>
        {/if}
        <DetailActionBar fixed ariaLabel="방별 프롬프트 소스 작업">
            {#if orchestrationState.workspace.room_config.supported_fields.template_slots}
                <button
                    class="detail-action"
                    type="button"
                    disabled={orchestrationState.saving ||
                        orchestrationState.workspace.room_config.template_slots.length >=
                            MAX_ROOM_PROMPT_TEMPLATE_SLOTS}
                    onclick={addRoomTemplateSlot}
                >
                    슬롯 추가
                </button>
            {/if}
            <button
                class="primary detail-action detail-action--grow"
                type="button"
                disabled={!orchestrationState.dirty_room_config ||
                    orchestrationState.saving ||
                    roomPromptSourceError !== null}
                onclick={() => void controller.saveRoomConfig()}
            >
                {orchestrationState.saving ? '저장 중…' : '저장'}
            </button>
        </DetailActionBar>
    </section>
{/if}

{#if detailPage === 'variables'}
    <section class="studio-card" aria-label="변수와 제작자 컨트롤">
        {#if orchestrationState.workspace.creator_controls.length === 0}
            <p class="empty-note">현재 프리셋이 공개한 변수가 없습니다.</p>
        {:else}
            <div class="setting-list variable-list" aria-label="프롬프트 변수 목록">
                {#each orchestrationState.workspace.creator_controls.slice(0, 100) as control (control.id)}
                    <div class="setting-row variable-row">
                        <div class="setting-content">
                            <div class="setting-copy variable-copy">
                                <strong>{control.label}</strong>
                                <small>
                                    {control.kind} · {control.minimum ?? '—'}…{control.maximum ??
                                        '—'}
                                </small>
                            </div>
                            <span class="setting-value variable-value">
                                {JSON.stringify(
                                    orchestrationState.workspace.room_config.creator_values[
                                        control.id
                                    ] ?? control.value,
                                ).slice(0, 300)}
                            </span>
                        </div>
                    </div>
                {/each}
            </div>
            {#if orchestrationState.workspace.creator_controls.length > 100}
                <p class="bounded-note">처음 100개 변수만 표시합니다.</p>
            {/if}
        {/if}
    </section>
{/if}

{#if detailPage?.startsWith('profiles')}
    <TaskProfilesPanel {appState} {orchestrationState} {controller} bind:detailPage />
{/if}

{#if detailPage === 'documents' || detailPage?.startsWith('documents/')}
    <CreatorDocumentEditors {orchestrationState} {controller} bind:detailPage />
{/if}
