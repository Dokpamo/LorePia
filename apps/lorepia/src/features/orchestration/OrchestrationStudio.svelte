<script lang="ts">
    import { tick } from 'svelte';
    import { SvelteMap } from 'svelte/reactivity';

    import type { LorepiaAppController, LorepiaAppState } from '../../app/app-controller';
    import type {
        ApprovableContentPackageCapabilityDto,
        ContentPackageCapabilityDto,
        ContentPackageTargetReviewDocumentDto,
        CreatorPromptBlockDocumentDto,
        CreatorPromptBlockPlacementZone,
        LorepiaClient,
        MemoryRecordDto,
        MemoryRecordSourceNavigationDto,
        OrchestrationConditionExprDto,
        PromptBlockDto,
        PromptHistorySelectorDto,
        PromptPresetHistoryClientApi,
        SafePromptTemplateDto,
    } from '../../lib/ipc/contracts';
    import type { ContentModuleLifecycleClientApi } from './module-lifecycle-contracts';
    import {
        MAX_ROOM_PROMPT_NAME_CHARS,
        MAX_ROOM_PROMPT_TEMPLATE_SLOTS,
        MAX_ROOM_PROMPT_TEXT_CHARS,
        MAX_VISIBLE_PLAN_OPERATION_NONCE_CHARS,
        roomPromptSourceValidationError,
        taskProfileValidationError,
        type OrchestrationController,
        type OrchestrationState,
    } from './orchestration-controller';
    import {
        MAX_COMPLETED_CONTENT_PACKAGE_EXPORTS,
        MAX_VISIBLE_CONTENT_PACKAGE_TARGET_DOCUMENTS,
        type ContentPackageController,
        type ContentPackageState,
    } from './content-package-controller';
    import CreatorDocumentEditors from './CreatorDocumentEditors.svelte';
    import GenerationAttemptApprovals from '../chat/GenerationAttemptApprovals.svelte';
    import ContentModuleLifecyclePanel from './ContentModuleLifecyclePanel.svelte';
    import MemoryQueryRetryPanel from './MemoryQueryRetryPanel.svelte';
    import PromptPresetHistory from './PromptPresetHistory.svelte';

    interface Props {
        client?: LorepiaClient &
            Partial<PromptPresetHistoryClientApi & ContentModuleLifecycleClientApi>;
        appState: LorepiaAppState;
        orchestrationState: OrchestrationState;
        controller: OrchestrationController;
        appController?: LorepiaAppController;
        contentPackageState?: ContentPackageState;
        contentPackageController?: ContentPackageController;
        onNavigateToMemorySource?: (source: MemoryRecordSourceNavigationDto) => void;
    }

    const MAX_INLINE_ITEMS = 100;
    const MAX_MODULE_COMPONENTS = 200;
    const MAX_PLAN_MESSAGES = 200;
    const MAX_PLAN_DETAILS = 300;
    const MAX_DISPLAY_PROJECTION_MESSAGES = 64;
    const MAX_DISPLAY_TRANSFORM_DIAGNOSTICS = 512;
    type BlockJsonField = 'template' | 'condition' | 'history_selector';
    interface DisplayTransformDiagnosticItem {
        messageId: string;
        generationId: string;
        createdAt: string;
        canonicalContentSha256: string;
        displayContentSha256: string;
        diagnosticsSha256: string;
        diagnostics: NonNullable<
            LorepiaAppState['messages']['items'][number]['display_projection']
        >['diagnostics'];
    }
    const BLOCK_JSON_FIELDS: readonly (readonly [BlockJsonField, string])[] = [
        ['template', '안전 템플릿 AST'],
        ['condition', '조건 AST'],
        ['history_selector', '대화 구간 선택기'],
    ];

    let {
        client,
        appState,
        orchestrationState,
        controller,
        appController,
        contentPackageState,
        contentPackageController,
        onNavigateToMemorySource = () => undefined,
    }: Props = $props();
    let activeTab = $state<'advanced' | 'expert'>('advanced');
    let blockSearch = $state('');
    let blockZoneFilter = $state('all');
    let blockStatusFilter = $state<'all' | 'enabled' | 'disabled'>('all');
    let draggedBlockId = $state<string | null>(null);
    let knowledgeSample = $state('');
    let transformRuleId = $state('');
    let transformSample = $state('');
    let planUserText = $state('');
    let reviewedSendBusy = $state(false);
    let attemptApprovalRefreshEpoch = $state(0);
    let expertSearch = $state('');
    let expertFilter = $state<'all' | 'messages' | 'provider' | 'parameters' | 'diff'>('all');
    let memoryDrafts = $state<Record<string, string>>({});
    let memoryDraftContextKey = '';
    let blockDraftRevisionKey = '';
    let pendingMemoryDeleteId = $state<string | null>(null);
    let blockJsonDrafts = $state<Record<string, string>>({});
    let blockJsonErrors = $state<Record<string, string>>({});
    let newTaskProfileId = $state('');
    let pendingTaskProfileDeleteId = $state<string | null>(null);
    let advancedTabButton: HTMLButtonElement;
    let expertTabButton: HTMLButtonElement;

    function packageCapabilityNeedsApproval(
        capability: ContentPackageCapabilityDto,
    ): capability is ApprovableContentPackageCapabilityDto {
        return capability === 'transforms' || capability === 'declarative_interactions';
    }

    function updateTargetConfirmed(document: ContentPackageTargetReviewDocumentDto): boolean {
        if (contentPackageState === undefined || document.disposition !== 'update') return false;
        return contentPackageState.confirmed_update_targets.some(
            (confirmation) =>
                confirmation.source_component_id === document.source_component_id &&
                confirmation.component_document_ordinal === document.component_document_ordinal &&
                confirmation.target_object_id === document.target_object_id &&
                confirmation.expected_target_revision_id === document.expected_target_revision_id &&
                confirmation.expected_target_state_revision ===
                    document.expected_target_state_revision,
        );
    }

    const packageUpdateTargetsConfirmed = $derived.by(() => {
        const targetReview = contentPackageState?.selection?.target_review;
        if (targetReview === undefined) return false;
        const updates = targetReview.documents.filter(
            (document) => document.disposition === 'update',
        );
        return (
            updates.length === contentPackageState?.confirmed_update_targets.length &&
            updates.every(updateTargetConfirmed)
        );
    });

    const normalizedBlockSearch = $derived(blockSearch.trim().toLocaleLowerCase());
    const previewGenerationTarget = $derived(orchestrationState.workspace.generation_target);
    const roomPromptSourceError = $derived(
        roomPromptSourceValidationError(orchestrationState.workspace.room_config),
    );
    const displayTransformDiagnostics = $derived.by(() => {
        const items: DisplayTransformDiagnosticItem[] = [];
        let diagnosticCount = 0;
        let truncated = false;
        for (const message of [...appState.messages.items].reverse()) {
            const projection = message.display_projection;
            if (projection === undefined || message.generation_id === null) continue;
            if (
                items.length >= MAX_DISPLAY_PROJECTION_MESSAGES ||
                diagnosticCount + projection.diagnostics.length > MAX_DISPLAY_TRANSFORM_DIAGNOSTICS
            ) {
                truncated = true;
                continue;
            }
            items.push({
                messageId: message.id,
                generationId: message.generation_id,
                createdAt: message.created_at,
                canonicalContentSha256: projection.canonical_content_sha256,
                displayContentSha256: projection.display_content_sha256,
                diagnosticsSha256: projection.diagnostics_sha256,
                diagnostics: projection.diagnostics,
            });
            diagnosticCount += projection.diagnostics.length;
        }
        items.reverse();
        return { items, diagnosticCount, truncated };
    });
    const displayBlocks = $derived.by(() => {
        const document = orchestrationState.editable_prompt_preset?.value;
        if (!document) return orchestrationState.workspace.prompt_blocks;
        const editableById = new Map(document.blocks.map((block) => [block.id, block]));
        return orchestrationState.workspace.prompt_blocks.map((block) => {
            const editable = editableById.get(block.id);
            const cacheBoundaryAfter = document.cache_boundaries.some(
                (boundary) => boundary.after_block_id === block.id,
            );
            return editable
                ? {
                      ...block,
                      name: editable.name,
                      enabled: editable.enabled,
                      role_hint: editable.role_hint,
                      placement_zone: editable.placement_zone,
                      priority: editable.token_policy.priority,
                      minimum_tokens: editable.token_policy.min_tokens,
                      maximum_tokens: editable.token_policy.max_tokens,
                      overflow_policy: editable.overflow_policy,
                      cache_boundary_after: cacheBoundaryAfter,
                  }
                : block;
        });
    });
    const blockZoneOverview = $derived.by(() => {
        const overview = new SvelteMap<string, { total: number; enabled: number }>();
        for (const block of displayBlocks) {
            const current = overview.get(block.placement_zone) ?? { total: 0, enabled: 0 };
            current.total += 1;
            if (block.enabled) current.enabled += 1;
            overview.set(block.placement_zone, current);
        }
        return [...overview.entries()];
    });
    const filteredBlocks = $derived(
        displayBlocks.filter((block) => {
            if (blockZoneFilter !== 'all' && block.placement_zone !== blockZoneFilter) return false;
            if (blockStatusFilter === 'enabled' && !block.enabled) return false;
            if (blockStatusFilter === 'disabled' && block.enabled) return false;
            return (
                normalizedBlockSearch === '' ||
                [block.name, block.kind, block.placement_zone, block.source_label]
                    .join(' ')
                    .toLocaleLowerCase()
                    .includes(normalizedBlockSearch)
            );
        }),
    );
    const blockGroups = $derived.by(() => {
        const groups = new SvelteMap<string, typeof filteredBlocks>();
        for (const block of filteredBlocks) {
            const values = groups.get(block.placement_zone) ?? [];
            values.push(block);
            groups.set(block.placement_zone, values);
        }
        return [...groups.entries()];
    });

    function promptZoneDomId(zone: string): string {
        return `prompt-zone-${encodeURIComponent(zone)}`;
    }

    async function navigateToPromptZone(zone: string): Promise<void> {
        blockZoneFilter = zone;
        await tick();
        const heading = document.getElementById(promptZoneDomId(zone));
        heading?.focus();
    }

    $effect(() => {
        if (
            orchestrationState.context_key === memoryDraftContextKey &&
            orchestrationState.phase !== 'loading'
        ) {
            return;
        }
        memoryDraftContextKey = orchestrationState.context_key;
        memoryDrafts = {};
        pendingMemoryDeleteId = null;
    });

    $effect(() => {
        const editable = orchestrationState.editable_prompt_preset;
        const key = `${orchestrationState.context_key}:${editable?.value.id ?? ''}:${String(
            editable?.revision ?? '',
        )}`;
        if (key === blockDraftRevisionKey) return;
        blockDraftRevisionKey = key;
        blockJsonDrafts = {};
        blockJsonErrors = {};
    });

    function blockJsonKey(blockId: string, field: BlockJsonField): string {
        return `${blockId}:${field}`;
    }

    function blockJsonDraft(block: CreatorPromptBlockDocumentDto, field: BlockJsonField): string {
        const key = blockJsonKey(block.id, field);
        return blockJsonDrafts[key] ?? JSON.stringify(block[field], null, 2);
    }

    function setBlockJsonDraft(
        block: CreatorPromptBlockDocumentDto,
        field: BlockJsonField,
        value: string,
    ): void {
        blockJsonDrafts[blockJsonKey(block.id, field)] = value;
    }

    function commitBlockJson(block: CreatorPromptBlockDocumentDto, field: BlockJsonField): void {
        const key = blockJsonKey(block.id, field);
        const source = blockJsonDrafts[key] ?? JSON.stringify(block[field]);
        if (source.length > 32_768) {
            blockJsonErrors[key] = 'JSON은 32,768자 이하여야 합니다.';
            return;
        }
        try {
            const parsed: unknown = JSON.parse(source);
            if (parsed !== null && (typeof parsed !== 'object' || Array.isArray(parsed))) {
                blockJsonErrors[key] = '객체 또는 null만 입력할 수 있습니다.';
                return;
            }
            if (field === 'template') {
                controller.stageEditablePromptBlock(block.id, {
                    template: parsed as SafePromptTemplateDto | null,
                });
            } else if (field === 'condition') {
                controller.stageEditablePromptBlock(block.id, {
                    condition: parsed as OrchestrationConditionExprDto | null,
                });
            } else {
                controller.stageEditablePromptBlock(block.id, {
                    history_selector: parsed as PromptHistorySelectorDto | null,
                });
            }
            blockJsonErrors = Object.fromEntries(
                Object.entries(blockJsonErrors).filter(([candidate]) => candidate !== key),
            );
        } catch {
            blockJsonErrors[key] = '유효한 JSON이 아닙니다.';
        }
    }

    function optionalNumber(value: string): number | null {
        return value.trim() === '' ? null : Number(value);
    }

    function expertMatches(...values: unknown[]): boolean {
        const query = expertSearch.trim().toLocaleLowerCase();
        if (query === '') return true;
        return values
            .map((value) =>
                (typeof value === 'string' ? value : JSON.stringify(value)).toLocaleLowerCase(),
            )
            .join(' ')
            .includes(query);
    }

    function boundedJson(value: unknown, maxChars = 65_536): string {
        return JSON.stringify(value, null, 2).slice(0, maxChars);
    }

    function boundedPlanIdentifier(value: string): string {
        return value.slice(0, MAX_VISIBLE_PLAN_OPERATION_NONCE_CHARS);
    }

    function memoryDraft(record: MemoryRecordDto): string {
        return memoryDrafts[record.id] ?? record.summary;
    }

    function clearMemoryDraft(recordId: string): void {
        memoryDrafts = Object.fromEntries(
            Object.entries(memoryDrafts).filter(([id]) => id !== recordId),
        );
    }

    async function saveMemorySummary(record: MemoryRecordDto): Promise<void> {
        if (
            await controller.updateMemoryRecord(record.id, {
                summary: memoryDraft(record),
            })
        ) {
            clearMemoryDraft(record.id);
        }
    }

    async function confirmMemoryDelete(recordId: string): Promise<void> {
        if (pendingMemoryDeleteId !== recordId) {
            pendingMemoryDeleteId = recordId;
            return;
        }
        if (await controller.deleteMemoryRecord(recordId)) {
            clearMemoryDraft(recordId);
        }
        pendingMemoryDeleteId = null;
    }

    function canDropOn(target: PromptBlockDto): boolean {
        if (orchestrationState.editable_prompt_preset_dirty) return false;
        const dragged = orchestrationState.workspace.prompt_blocks.find(
            (block) => block.id === draggedBlockId,
        );
        return (
            dragged?.order_editable === true &&
            target.order_editable &&
            dragged.placement_zone === target.placement_zone
        );
    }

    function addTaskProfile(): void {
        if (controller.addTaskProfileDraft(newTaskProfileId)) {
            newTaskProfileId = '';
        }
    }

    async function confirmTaskProfileDelete(taskProfileId: string): Promise<void> {
        if (pendingTaskProfileDeleteId !== taskProfileId) {
            pendingTaskProfileDeleteId = taskProfileId;
            return;
        }
        await controller.deleteTaskProfile(taskProfileId);
        pendingTaskProfileDeleteId = null;
    }

    async function selectTab(tab: 'advanced' | 'expert'): Promise<void> {
        activeTab = tab;
        await tick();
        (tab === 'advanced' ? advancedTabButton : expertTabButton).focus();
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

    function handleTabKeydown(event: KeyboardEvent): void {
        if (event.key === 'ArrowRight' || event.key === 'ArrowLeft') {
            event.preventDefault();
            void selectTab(activeTab === 'advanced' ? 'expert' : 'advanced');
        } else if (event.key === 'Home') {
            event.preventDefault();
            void selectTab('advanced');
        } else if (event.key === 'End') {
            event.preventDefault();
            void selectTab('expert');
        }
    }

    function handleDrop(targetId: string): void {
        if (draggedBlockId !== null && draggedBlockId !== targetId) {
            void controller.movePromptBlockTo(draggedBlockId, targetId);
        }
        draggedBlockId = null;
    }

    function addRoomTemplateSlot(): void {
        const slots = orchestrationState.workspace.room_config.template_slots;
        if (slots.length >= MAX_ROOM_PROMPT_TEMPLATE_SLOTS) return;
        controller.stageRoomConfig({
            template_slots: [...slots, { name: '', value: '' }],
        });
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

<section class="orchestration-studio" aria-labelledby="orchestration-studio-title">
    <header class="studio-header">
        <div>
            <p class="eyebrow">Prompt orchestration</p>
            <h2 id="orchestration-studio-title">프롬프트 제작실</h2>
            <p>결정적 계획, 메모리·지식 근거, 안전한 콘텐츠 모듈을 한곳에서 검토합니다.</p>
        </div>
        <button
            type="button"
            disabled={orchestrationState.workspace.room_config.conversation_id === ''}
            onclick={() =>
                void controller.loadContext(
                    orchestrationState.workspace.room_config.conversation_id || null,
                    orchestrationState.workspace.room_config.branch_id || null,
                )}
        >
            새로고침
        </button>
    </header>

    <div class="studio-tabs" role="tablist" aria-label="프롬프트 제작실 보기">
        <button
            id="orchestration-advanced-tab"
            type="button"
            role="tab"
            aria-selected={activeTab === 'advanced'}
            aria-controls="orchestration-advanced-panel"
            tabindex={activeTab === 'advanced' ? 0 : -1}
            bind:this={advancedTabButton}
            onclick={() => void selectTab('advanced')}
            onkeydown={handleTabKeydown}
        >
            고급
        </button>
        <button
            id="orchestration-expert-tab"
            type="button"
            role="tab"
            aria-selected={activeTab === 'expert'}
            aria-controls="orchestration-expert-panel"
            tabindex={activeTab === 'expert' ? 0 : -1}
            bind:this={expertTabButton}
            onclick={() => void selectTab('expert')}
            onkeydown={handleTabKeydown}
        >
            전문가
        </button>
    </div>

    {#if orchestrationState.phase === 'loading'}
        <div class="studio-status" role="status">오케스트레이션 구성을 불러오는 중입니다.</div>
    {:else if orchestrationState.error !== null}
        <div
            class:error={orchestrationState.phase !== 'unavailable'}
            class="studio-status"
            role={orchestrationState.phase === 'unavailable' ? 'note' : 'alert'}
        >
            {orchestrationState.error}
        </div>
    {/if}

    {#if activeTab === 'advanced'}
        <div
            id="orchestration-advanced-panel"
            class="studio-panel"
            role="tabpanel"
            aria-labelledby="orchestration-advanced-tab"
        >
            {#if client}
                <PromptPresetHistory
                    {client}
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
            <section class="studio-card block-editor" aria-labelledby="prompt-blocks-title">
                <div class="section-heading">
                    <div>
                        <h3 id="prompt-blocks-title">프롬프트 블록</h3>
                        <p>구역 안에서 드래그하거나 키보드 버튼으로 순서를 바꿉니다.</p>
                    </div>
                    <div class="row-actions">
                        <span class="count-badge">
                            {filteredBlocks.length}/{displayBlocks.length}
                        </span>
                        <button
                            type="button"
                            disabled={!orchestrationState.editable_prompt_preset_dirty ||
                                orchestrationState.editable_prompt_preset_loading}
                            onclick={() => void controller.saveEditablePromptPreset()}
                        >
                            블록 변경 저장
                        </button>
                        <button
                            type="button"
                            disabled={orchestrationState.editable_prompt_preset_loading}
                            onclick={() => void controller.reloadEditablePromptPreset()}
                        >
                            Core에서 다시 불러오기
                        </button>
                    </div>
                </div>
                {#if orchestrationState.editable_prompt_preset_loading}
                    <p role="status">안전한 편집 문서를 불러오거나 저장하는 중입니다.</p>
                {:else if orchestrationState.editable_prompt_preset_error}
                    <p class="inline-diagnostic" role="alert">
                        {orchestrationState.editable_prompt_preset_error}
                    </p>
                {/if}
                <div class="block-discovery-controls">
                    <label class="search-field">
                        <span>블록 검색</span>
                        <input
                            type="search"
                            maxlength="256"
                            bind:value={blockSearch}
                            placeholder="이름, 종류, 구역, 출처"
                        />
                    </label>
                    <label>
                        <span>블록 구역 필터</span>
                        <select bind:value={blockZoneFilter}>
                            <option value="all">모든 구역</option>
                            {#each blockZoneOverview as [zone] (zone)}
                                <option value={zone}>{zone}</option>
                            {/each}
                        </select>
                    </label>
                    <label>
                        <span>블록 활성 상태 필터</span>
                        <select bind:value={blockStatusFilter}>
                            <option value="all">전체 상태</option>
                            <option value="enabled">사용 중</option>
                            <option value="disabled">꺼짐</option>
                        </select>
                    </label>
                    <button
                        type="button"
                        disabled={blockSearch === '' &&
                            blockZoneFilter === 'all' &&
                            blockStatusFilter === 'all'}
                        onclick={() => {
                            blockSearch = '';
                            blockZoneFilter = 'all';
                            blockStatusFilter = 'all';
                        }}
                    >
                        블록 필터 초기화
                    </button>
                </div>
                <nav class="block-minimap" aria-label="프롬프트 블록 미니맵">
                    <span>구역 미니맵</span>
                    <ol>
                        {#each blockZoneOverview as [zone, counts] (zone)}
                            <li>
                                <button
                                    type="button"
                                    class:active={blockZoneFilter === zone}
                                    aria-pressed={blockZoneFilter === zone}
                                    aria-label={`${zone} 구역으로 이동`}
                                    title={`${zone}: 전체 ${String(counts.total)}개, 사용 ${String(counts.enabled)}개`}
                                    onclick={() => void navigateToPromptZone(zone)}
                                >
                                    <span>{zone}</span>
                                    <small>{counts.enabled}/{counts.total}</small>
                                </button>
                            </li>
                        {/each}
                    </ol>
                </nav>
                {#if orchestrationState.list_truncation.prompt_blocks}
                    <p class="bounded-note" role="note">
                        안전한 편집을 위해 처음 200개 블록만 표시합니다.
                    </p>
                {/if}
                {#if blockGroups.length === 0}
                    <p class="empty-note">표시할 프롬프트 블록이 없습니다.</p>
                {:else}
                    <div class="block-groups">
                        {#each blockGroups as [zone, blocks] (zone)}
                            <section class="block-group" aria-labelledby={promptZoneDomId(zone)}>
                                <header>
                                    <h4 id={promptZoneDomId(zone)} tabindex="-1">{zone}</h4>
                                    <span>{blocks.length}개</span>
                                </header>
                                <ol class="block-list">
                                    {#each blocks as block (block.id)}
                                        {@const zoneIndex = blocks.findIndex(
                                            (candidate) => candidate.id === block.id,
                                        )}
                                        {@const editableBlock =
                                            orchestrationState.editable_prompt_preset?.value.blocks.find(
                                                (candidate) => candidate.id === block.id,
                                            )}
                                        {@const editableCache =
                                            orchestrationState.editable_prompt_preset?.value.cache_boundaries.find(
                                                (candidate) =>
                                                    candidate.after_block_id === block.id,
                                            )}
                                        <li
                                            draggable={block.order_editable &&
                                                !orchestrationState.editable_prompt_preset_dirty}
                                            class:dragging={draggedBlockId === block.id}
                                            ondragstart={() => (draggedBlockId = block.id)}
                                            ondragend={() => (draggedBlockId = null)}
                                            ondragover={(event) => {
                                                if (canDropOn(block)) event.preventDefault();
                                            }}
                                            ondrop={() => handleDrop(block.id)}
                                        >
                                            <div class="block-summary">
                                                <span class="drag-handle" aria-hidden="true"
                                                    >⋮⋮</span
                                                >
                                                <div>
                                                    <strong>{block.name}</strong>
                                                    <span>{block.kind} · {block.role_hint}</span>
                                                    {#if !block.order_editable}
                                                        <small>Core 정책 블록 · 읽기 전용</small>
                                                    {/if}
                                                </div>
                                                <span
                                                    class:disabled={!block.enabled}
                                                    class="status-badge"
                                                >
                                                    {block.enabled ? '사용' : '꺼짐'}
                                                </span>
                                                <div class="reorder-actions">
                                                    <button
                                                        type="button"
                                                        disabled={!block.order_editable ||
                                                            orchestrationState.editable_prompt_preset_dirty ||
                                                            zoneIndex <= 0 ||
                                                            !blocks[zoneIndex - 1]?.order_editable}
                                                        aria-label={`${block.name} 블록 위로 이동`}
                                                        onclick={() =>
                                                            void controller.movePromptBlock(
                                                                block.id,
                                                                -1,
                                                            )}
                                                    >
                                                        ↑
                                                    </button>
                                                    <button
                                                        type="button"
                                                        disabled={!block.order_editable ||
                                                            orchestrationState.editable_prompt_preset_dirty ||
                                                            zoneIndex < 0 ||
                                                            zoneIndex >= blocks.length - 1 ||
                                                            !blocks[zoneIndex + 1]?.order_editable}
                                                        aria-label={`${block.name} 블록 아래로 이동`}
                                                        onclick={() =>
                                                            void controller.movePromptBlock(
                                                                block.id,
                                                                1,
                                                            )}
                                                    >
                                                        ↓
                                                    </button>
                                                </div>
                                            </div>
                                            <details>
                                                <summary>조건·토큰·오버플로 세부정보</summary>
                                                <dl class="detail-grid">
                                                    <div>
                                                        <dt>조건</dt>
                                                        <dd>{block.condition_summary ?? '항상'}</dd>
                                                    </div>
                                                    <div>
                                                        <dt>출처</dt>
                                                        <dd>{block.source_label}</dd>
                                                    </div>
                                                    <div>
                                                        <dt>Provenance</dt>
                                                        <dd>{block.provenance_label}</dd>
                                                    </div>
                                                    <div>
                                                        <dt>토큰</dt>
                                                        <dd>
                                                            우선순위 {block.priority}, 최소
                                                            {block.minimum_tokens ?? '없음'}, 최대
                                                            {block.maximum_tokens ?? '없음'}
                                                        </dd>
                                                    </div>
                                                    <div>
                                                        <dt>오버플로</dt>
                                                        <dd>{block.overflow_policy}</dd>
                                                    </div>
                                                    <div>
                                                        <dt>캐시 경계</dt>
                                                        <dd>
                                                            {block.cache_boundary_after
                                                                ? '이 블록 뒤'
                                                                : '없음'}
                                                        </dd>
                                                    </div>
                                                </dl>
                                                {#if block.template_preview}
                                                    <div class="safe-text-preview">
                                                        <strong>템플릿 미리보기</strong>
                                                        <pre>{block.template_preview.slice(
                                                                0,
                                                                4000,
                                                            )}</pre>
                                                    </div>
                                                {/if}
                                                {#if editableBlock}
                                                    <fieldset class="structured-editor">
                                                        <legend>구조화된 블록 편집</legend>
                                                        <div class="editor-grid">
                                                            <label>
                                                                <span>이름</span>
                                                                <input
                                                                    type="text"
                                                                    maxlength="512"
                                                                    value={editableBlock.name}
                                                                    oninput={(event) =>
                                                                        controller.stageEditablePromptBlock(
                                                                            editableBlock.id,
                                                                            {
                                                                                name: event
                                                                                    .currentTarget
                                                                                    .value,
                                                                            },
                                                                        )}
                                                                />
                                                            </label>
                                                            <label class="checkbox-row">
                                                                <input
                                                                    type="checkbox"
                                                                    checked={editableBlock.enabled}
                                                                    onchange={(event) =>
                                                                        controller.stageEditablePromptBlock(
                                                                            editableBlock.id,
                                                                            {
                                                                                enabled:
                                                                                    event
                                                                                        .currentTarget
                                                                                        .checked,
                                                                            },
                                                                        )}
                                                                />
                                                                <span>블록 사용</span>
                                                            </label>
                                                            <label>
                                                                <span>역할</span>
                                                                <select
                                                                    value={editableBlock.role_hint}
                                                                    onchange={(event) =>
                                                                        controller.stageEditablePromptBlock(
                                                                            editableBlock.id,
                                                                            {
                                                                                role_hint: event
                                                                                    .currentTarget
                                                                                    .value as CreatorPromptBlockDocumentDto['role_hint'],
                                                                            },
                                                                        )}
                                                                >
                                                                    <option value="system"
                                                                        >system</option
                                                                    >
                                                                    <option value="developer"
                                                                        >developer</option
                                                                    >
                                                                    <option value="user"
                                                                        >user</option
                                                                    >
                                                                    <option value="assistant"
                                                                        >assistant</option
                                                                    >
                                                                    <option value="provider_default"
                                                                        >provider default</option
                                                                    >
                                                                </select>
                                                            </label>
                                                            <label>
                                                                <span>삽입 구역</span>
                                                                <select
                                                                    value={editableBlock.placement_zone}
                                                                    onchange={(event) =>
                                                                        controller.stageEditablePromptBlock(
                                                                            editableBlock.id,
                                                                            {
                                                                                placement_zone:
                                                                                    event
                                                                                        .currentTarget
                                                                                        .value as CreatorPromptBlockPlacementZone,
                                                                            },
                                                                        )}
                                                                >
                                                                    <option
                                                                        value="preset_instruction"
                                                                        >preset instruction</option
                                                                    >
                                                                    <option
                                                                        value="character_context"
                                                                        >character context</option
                                                                    >
                                                                    <option
                                                                        value="retrieved_context"
                                                                        >retrieved context</option
                                                                    >
                                                                    <option value="older_history"
                                                                        >older history</option
                                                                    >
                                                                    <option
                                                                        value="recent_enhancement"
                                                                        >recent enhancement</option
                                                                    >
                                                                    <option value="recent_history"
                                                                        >recent history</option
                                                                    >
                                                                    <option value="post_history"
                                                                        >post history</option
                                                                    >
                                                                    <option value="latest_user"
                                                                        >latest user</option
                                                                    >
                                                                    <option
                                                                        value="assistant_prefill"
                                                                        >assistant prefill</option
                                                                    >
                                                                </select>
                                                            </label>
                                                            <label>
                                                                <span>우선순위</span>
                                                                <input
                                                                    type="number"
                                                                    min="0"
                                                                    max="65535"
                                                                    value={editableBlock
                                                                        .token_policy.priority}
                                                                    oninput={(event) =>
                                                                        controller.stageEditablePromptBlock(
                                                                            editableBlock.id,
                                                                            {
                                                                                token_policy: {
                                                                                    ...editableBlock.token_policy,
                                                                                    priority:
                                                                                        Number(
                                                                                            event
                                                                                                .currentTarget
                                                                                                .value,
                                                                                        ),
                                                                                },
                                                                            },
                                                                        )}
                                                                />
                                                            </label>
                                                            <label>
                                                                <span>최소 토큰</span>
                                                                <input
                                                                    type="number"
                                                                    min="0"
                                                                    value={editableBlock
                                                                        .token_policy.min_tokens ??
                                                                        ''}
                                                                    oninput={(event) =>
                                                                        controller.stageEditablePromptBlock(
                                                                            editableBlock.id,
                                                                            {
                                                                                token_policy: {
                                                                                    ...editableBlock.token_policy,
                                                                                    min_tokens:
                                                                                        optionalNumber(
                                                                                            event
                                                                                                .currentTarget
                                                                                                .value,
                                                                                        ),
                                                                                },
                                                                            },
                                                                        )}
                                                                />
                                                            </label>
                                                            <label>
                                                                <span>최대 토큰</span>
                                                                <input
                                                                    type="number"
                                                                    min="0"
                                                                    value={editableBlock
                                                                        .token_policy.max_tokens ??
                                                                        ''}
                                                                    oninput={(event) =>
                                                                        controller.stageEditablePromptBlock(
                                                                            editableBlock.id,
                                                                            {
                                                                                token_policy: {
                                                                                    ...editableBlock.token_policy,
                                                                                    max_tokens:
                                                                                        optionalNumber(
                                                                                            event
                                                                                                .currentTarget
                                                                                                .value,
                                                                                        ),
                                                                                },
                                                                            },
                                                                        )}
                                                                />
                                                            </label>
                                                            <label>
                                                                <span>예약 토큰</span>
                                                                <input
                                                                    type="number"
                                                                    min="0"
                                                                    value={editableBlock
                                                                        .token_policy
                                                                        .reserve_tokens ?? ''}
                                                                    oninput={(event) =>
                                                                        controller.stageEditablePromptBlock(
                                                                            editableBlock.id,
                                                                            {
                                                                                token_policy: {
                                                                                    ...editableBlock.token_policy,
                                                                                    reserve_tokens:
                                                                                        optionalNumber(
                                                                                            event
                                                                                                .currentTarget
                                                                                                .value,
                                                                                        ),
                                                                                },
                                                                            },
                                                                        )}
                                                                />
                                                            </label>
                                                            <label>
                                                                <span>오버플로 정책</span>
                                                                <select
                                                                    value={editableBlock.overflow_policy}
                                                                    onchange={(event) =>
                                                                        controller.stageEditablePromptBlock(
                                                                            editableBlock.id,
                                                                            {
                                                                                overflow_policy:
                                                                                    event
                                                                                        .currentTarget
                                                                                        .value as CreatorPromptBlockDocumentDto['overflow_policy'],
                                                                            },
                                                                        )}
                                                                >
                                                                    <option value="reject"
                                                                        >reject</option
                                                                    >
                                                                    <option value="drop_block"
                                                                        >drop block</option
                                                                    >
                                                                    <option value="trim_head"
                                                                        >trim head</option
                                                                    >
                                                                    <option value="trim_tail"
                                                                        >trim tail</option
                                                                    >
                                                                    <option
                                                                        value="keep_latest_items"
                                                                        >keep latest items</option
                                                                    >
                                                                    <option value="summarize"
                                                                        >summarize</option
                                                                    >
                                                                    <option
                                                                        value="reduce_knowledge_entries"
                                                                        >reduce knowledge entries</option
                                                                    >
                                                                </select>
                                                            </label>
                                                            <label>
                                                                <span>내부 메시지 병합</span>
                                                                <select
                                                                    value={editableBlock.merge_policy}
                                                                    onchange={(event) =>
                                                                        controller.stageEditablePromptBlock(
                                                                            editableBlock.id,
                                                                            {
                                                                                merge_policy: event
                                                                                    .currentTarget
                                                                                    .value as CreatorPromptBlockDocumentDto['merge_policy'],
                                                                            },
                                                                        )}
                                                                >
                                                                    <option value="separate_message"
                                                                        >separate message</option
                                                                    >
                                                                    <option
                                                                        value="merge_with_previous_same_role"
                                                                        >merge with previous same
                                                                        role</option
                                                                    >
                                                                </select>
                                                            </label>
                                                        </div>

                                                        {#each BLOCK_JSON_FIELDS as [field, label] (`${editableBlock.id}:${field}`)}
                                                            <label class="json-editor">
                                                                <span>{label}</span>
                                                                <textarea
                                                                    rows="6"
                                                                    maxlength="32768"
                                                                    value={blockJsonDraft(
                                                                        editableBlock,
                                                                        field,
                                                                    )}
                                                                    oninput={(event) =>
                                                                        setBlockJsonDraft(
                                                                            editableBlock,
                                                                            field,
                                                                            event.currentTarget
                                                                                .value,
                                                                        )}></textarea>
                                                            </label>
                                                            <button
                                                                type="button"
                                                                onclick={() =>
                                                                    commitBlockJson(
                                                                        editableBlock,
                                                                        field,
                                                                    )}
                                                            >
                                                                {label} 적용
                                                            </button>
                                                            {#if blockJsonErrors[blockJsonKey(editableBlock.id, field)]}
                                                                <p
                                                                    class="inline-diagnostic"
                                                                    role="alert"
                                                                >
                                                                    {blockJsonErrors[
                                                                        blockJsonKey(
                                                                            editableBlock.id,
                                                                            field,
                                                                        )
                                                                    ]}
                                                                </p>
                                                            {/if}
                                                        {/each}

                                                        <div class="cache-editor">
                                                            <label class="checkbox-row">
                                                                <input
                                                                    type="checkbox"
                                                                    checked={editableCache !==
                                                                        undefined}
                                                                    onchange={(event) =>
                                                                        controller.setEditablePromptCacheBoundary(
                                                                            editableBlock.id,
                                                                            event.currentTarget
                                                                                .checked,
                                                                        )}
                                                                />
                                                                <span>이 블록 뒤 캐시 경계</span>
                                                            </label>
                                                            {#if editableCache}
                                                                <label>
                                                                    <span>캐시 모드</span>
                                                                    <select
                                                                        value={editableCache.mode}
                                                                        onchange={(event) =>
                                                                            controller.stageEditablePromptCacheBoundary(
                                                                                editableBlock.id,
                                                                                {
                                                                                    mode: event
                                                                                        .currentTarget
                                                                                        .value as typeof editableCache.mode,
                                                                                },
                                                                            )}
                                                                    >
                                                                        <option value="automatic"
                                                                            >automatic</option
                                                                        >
                                                                        <option value="explicit"
                                                                            >explicit</option
                                                                        >
                                                                        <option value="disabled"
                                                                            >disabled</option
                                                                        >
                                                                    </select>
                                                                </label>
                                                                <label>
                                                                    <span>캐시 TTL</span>
                                                                    <select
                                                                        value={editableCache.ttl}
                                                                        onchange={(event) =>
                                                                            controller.stageEditablePromptCacheBoundary(
                                                                                editableBlock.id,
                                                                                {
                                                                                    ttl: event
                                                                                        .currentTarget
                                                                                        .value as typeof editableCache.ttl,
                                                                                },
                                                                            )}
                                                                    >
                                                                        <option
                                                                            value="provider_default"
                                                                            >provider default</option
                                                                        >
                                                                        <option value="short"
                                                                            >short</option
                                                                        >
                                                                        <option value="long"
                                                                            >long</option
                                                                        >
                                                                    </select>
                                                                </label>
                                                                <label>
                                                                    <span>역할 필터</span>
                                                                    <select
                                                                        value={editableCache
                                                                            .role_filter.kind}
                                                                        onchange={(event) =>
                                                                            controller.stageEditablePromptCacheBoundary(
                                                                                editableBlock.id,
                                                                                {
                                                                                    role_filter:
                                                                                        event
                                                                                            .currentTarget
                                                                                            .value ===
                                                                                        'exact_role'
                                                                                            ? {
                                                                                                  kind: 'exact_role',
                                                                                                  role: 'system',
                                                                                              }
                                                                                            : {
                                                                                                  kind: event
                                                                                                      .currentTarget
                                                                                                      .value as
                                                                                                      | 'all'
                                                                                                      | 'system_like',
                                                                                              },
                                                                                },
                                                                            )}
                                                                    >
                                                                        <option value="all"
                                                                            >all</option
                                                                        >
                                                                        <option value="system_like"
                                                                            >system like</option
                                                                        >
                                                                        <option value="exact_role"
                                                                            >exact role</option
                                                                        >
                                                                    </select>
                                                                </label>
                                                                {#if editableCache.role_filter.kind === 'exact_role'}
                                                                    <label>
                                                                        <span>정확한 역할</span>
                                                                        <select
                                                                            value={editableCache
                                                                                .role_filter.role}
                                                                            onchange={(event) =>
                                                                                controller.stageEditablePromptCacheBoundary(
                                                                                    editableBlock.id,
                                                                                    {
                                                                                        role_filter:
                                                                                            {
                                                                                                kind: 'exact_role',
                                                                                                role: event
                                                                                                    .currentTarget
                                                                                                    .value as CreatorPromptBlockDocumentDto['role_hint'],
                                                                                            },
                                                                                    },
                                                                                )}
                                                                        >
                                                                            <option value="system"
                                                                                >system</option
                                                                            >
                                                                            <option
                                                                                value="developer"
                                                                                >developer</option
                                                                            >
                                                                            <option value="user"
                                                                                >user</option
                                                                            >
                                                                            <option
                                                                                value="assistant"
                                                                                >assistant</option
                                                                            >
                                                                            <option
                                                                                value="provider_default"
                                                                                >provider default</option
                                                                            >
                                                                        </select>
                                                                    </label>
                                                                {/if}
                                                            {/if}
                                                        </div>
                                                    </fieldset>
                                                {/if}
                                            </details>
                                        </li>
                                    {/each}
                                </ol>
                            </section>
                        {/each}
                    </div>
                {/if}
            </section>

            <section class="studio-card" aria-labelledby="room-prompt-sources-title">
                <div class="section-heading">
                    <div>
                        <h3 id="room-prompt-sources-title">방별 프롬프트 소스</h3>
                        <p>
                            사용자 표시 이름, 작가 메모, 그룹 문맥과 안전 템플릿 슬롯을 이 방의 CAS
                            설정으로 저장합니다.
                        </p>
                    </div>
                    <div class="row-actions">
                        <span class="count-badge">
                            슬롯 {orchestrationState.workspace.room_config.template_slots
                                .length}/{MAX_ROOM_PROMPT_TEMPLATE_SLOTS}
                        </span>
                        <button
                            type="button"
                            disabled={!orchestrationState.dirty_room_config ||
                                orchestrationState.saving ||
                                roomPromptSourceError !== null}
                            onclick={() => void controller.saveRoomConfig()}
                        >
                            {orchestrationState.saving
                                ? '방별 소스 저장 중…'
                                : '방별 프롬프트 소스 저장'}
                        </button>
                    </div>
                </div>

                <div class="editor-grid prompt-source-grid">
                    {#if orchestrationState.workspace.room_config.supported_fields.user_name_override}
                        <label>
                            <span>사용자 표시 이름</span>
                            <input
                                type="text"
                                maxlength={MAX_ROOM_PROMPT_NAME_CHARS}
                                value={orchestrationState.workspace.room_config
                                    .user_name_override ?? ''}
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
                    <fieldset class="structured-editor">
                        <legend>안전 템플릿 슬롯</legend>
                        <div class="section-heading">
                            <p>
                                이름은 고유해야 하며 <code>block_content</code>는 예약되어 있습니다.
                            </p>
                            <button
                                type="button"
                                disabled={orchestrationState.saving ||
                                    orchestrationState.workspace.room_config.template_slots
                                        .length >= MAX_ROOM_PROMPT_TEMPLATE_SLOTS}
                                onclick={addRoomTemplateSlot}
                            >
                                슬롯 추가
                            </button>
                        </div>
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
            </section>

            <section class="studio-card" aria-labelledby="variables-title">
                <div class="section-heading">
                    <div>
                        <h3 id="variables-title">변수와 제작자 컨트롤</h3>
                        <p>값의 타입과 범위는 Core가 검증하며 템플릿에서는 값 삽입만 허용합니다.</p>
                    </div>
                </div>
                {#if orchestrationState.workspace.creator_controls.length === 0}
                    <p class="empty-note">현재 프리셋이 공개한 변수가 없습니다.</p>
                {:else}
                    <div class="data-table-wrap">
                        <table>
                            <caption class="sr-only">프롬프트 변수 목록</caption>
                            <thead>
                                <tr>
                                    <th scope="col">이름</th>
                                    <th scope="col">타입</th>
                                    <th scope="col">범위</th>
                                    <th scope="col">현재 값</th>
                                </tr>
                            </thead>
                            <tbody>
                                {#each orchestrationState.workspace.creator_controls.slice(0, 100) as control (control.id)}
                                    <tr>
                                        <th scope="row">{control.label}</th>
                                        <td>{control.kind}</td>
                                        <td>
                                            {control.minimum ?? '—'}…{control.maximum ?? '—'}
                                        </td>
                                        <td>
                                            {JSON.stringify(
                                                orchestrationState.workspace.room_config
                                                    .creator_values[control.id] ?? control.value,
                                            ).slice(0, 300)}
                                        </td>
                                    </tr>
                                {/each}
                            </tbody>
                        </table>
                    </div>
                {/if}
            </section>

            <section class="studio-card" aria-labelledby="profiles-title">
                <div class="section-heading">
                    <div>
                        <h3 id="profiles-title">생성·작업 프로필</h3>
                        <p>주 응답과 보조 작업의 모델, fallback, 동시성 제한을 구분합니다.</p>
                    </div>
                </div>
                <div class="profile-columns">
                    <div>
                        <h4>생성 프리셋</h4>
                        {#if appState.providers.workspace.presets.length === 0}
                            <p class="empty-note">로드된 생성 프리셋이 없습니다.</p>
                        {:else}
                            <ul class="compact-list">
                                {#each appState.providers.workspace.presets.slice(0, 100) as preset (preset.id)}
                                    <li>
                                        <strong>{preset.display_name}</strong>
                                        <span
                                            >{preset.reasoning.mode} · {preset.prompt_cache
                                                .mode}</span
                                        >
                                    </li>
                                {/each}
                            </ul>
                        {/if}
                    </div>
                    <div>
                        <h4>보조 작업 프로필 편집</h4>
                        <div class="inline-create">
                            <label>
                                <span>새 프로필 ID</span>
                                <input type="text" maxlength="256" bind:value={newTaskProfileId} />
                            </label>
                            <button
                                type="button"
                                disabled={newTaskProfileId.trim() === ''}
                                onclick={addTaskProfile}
                            >
                                프로필 추가
                            </button>
                        </div>
                        {#if orchestrationState.editable_task_profiles_loading}
                            <p role="status">작업 프로필을 불러오거나 저장하는 중입니다.</p>
                        {/if}
                        {#if orchestrationState.editable_task_profiles_error}
                            <p class="inline-diagnostic" role="alert">
                                {orchestrationState.editable_task_profiles_error}
                            </p>
                        {/if}
                        {#if orchestrationState.editable_task_profiles.length === 0}
                            <p class="empty-note">편집 가능한 보조 작업 프로필이 없습니다.</p>
                        {:else}
                            <ul class="task-profile-list">
                                {#each orchestrationState.editable_task_profiles.slice(0, 100) as profile (profile.value.id)}
                                    {@const profileValidationError = taskProfileValidationError(
                                        profile.value,
                                    )}
                                    <li>
                                        <details>
                                            <summary>
                                                {profile.value.id} · {profile.value.kind}
                                                {profile.dirty ? ' · 저장 안 됨' : ''}
                                            </summary>
                                            <div class="editor-grid">
                                                <label>
                                                    <span>작업 종류</span>
                                                    <select
                                                        value={profile.value.kind}
                                                        onchange={(event) =>
                                                            controller.stageTaskProfile(
                                                                profile.value.id,
                                                                {
                                                                    kind: event.currentTarget
                                                                        .value as typeof profile.value.kind,
                                                                },
                                                            )}
                                                    >
                                                        <option value="memory_summary"
                                                            >memory summary</option
                                                        >
                                                        <option value="memory_embedding"
                                                            >memory embedding</option
                                                        >
                                                        <option value="translation"
                                                            >translation</option
                                                        >
                                                        <option value="emotion_classification"
                                                            >emotion classification</option
                                                        >
                                                        <option value="state_extraction"
                                                            >state extraction</option
                                                        >
                                                        <option value="image_prompt"
                                                            >image prompt</option
                                                        >
                                                        <option value="title_generation"
                                                            >title generation</option
                                                        >
                                                    </select>
                                                </label>
                                                <label>
                                                    <span>모델 route ID</span>
                                                    <input
                                                        type="text"
                                                        maxlength="256"
                                                        value={profile.value.route_id}
                                                        oninput={(event) =>
                                                            controller.stageTaskProfile(
                                                                profile.value.id,
                                                                {
                                                                    route_id:
                                                                        event.currentTarget.value,
                                                                },
                                                            )}
                                                    />
                                                </label>
                                                <label>
                                                    <span>생성 프리셋 ID</span>
                                                    <input
                                                        type="text"
                                                        maxlength="256"
                                                        value={profile.value.generation_preset_id}
                                                        oninput={(event) =>
                                                            controller.stageTaskProfile(
                                                                profile.value.id,
                                                                {
                                                                    generation_preset_id:
                                                                        event.currentTarget.value,
                                                                },
                                                            )}
                                                    />
                                                </label>
                                                {#if profile.value.kind === 'memory_embedding'}
                                                    <label>
                                                        <span>임베딩 차원</span>
                                                        <input
                                                            type="number"
                                                            min="1"
                                                            max="32768"
                                                            step="1"
                                                            required
                                                            value={profile.value
                                                                .embedding_dimensions ?? ''}
                                                            oninput={(event) =>
                                                                controller.stageTaskProfile(
                                                                    profile.value.id,
                                                                    {
                                                                        embedding_dimensions:
                                                                            event.currentTarget
                                                                                .value === ''
                                                                                ? null
                                                                                : Number(
                                                                                      event
                                                                                          .currentTarget
                                                                                          .value,
                                                                                  ),
                                                                    },
                                                                )}
                                                        />
                                                    </label>
                                                {/if}
                                                <label>
                                                    <span>Fallback route IDs (쉼표 구분)</span>
                                                    <input
                                                        type="text"
                                                        maxlength="4096"
                                                        disabled={profile.value.kind ===
                                                            'memory_embedding'}
                                                        value={profile.value.fallback_route_ids.join(
                                                            ', ',
                                                        )}
                                                        oninput={(event) =>
                                                            controller.stageTaskProfile(
                                                                profile.value.id,
                                                                {
                                                                    fallback_route_ids:
                                                                        event.currentTarget.value
                                                                            .split(',')
                                                                            .map((value) =>
                                                                                value.trim(),
                                                                            )
                                                                            .filter(Boolean),
                                                                },
                                                            )}
                                                    />
                                                    {#if profile.value.kind === 'memory_embedding'}
                                                        <small>
                                                            메모리 임베딩은 단일 route와 벡터 공간만
                                                            사용합니다.
                                                        </small>
                                                    {/if}
                                                </label>
                                                <label>
                                                    <span>Timeout (ms)</span>
                                                    <input
                                                        type="number"
                                                        min="1"
                                                        value={profile.value.timeout_ms}
                                                        oninput={(event) =>
                                                            controller.stageTaskProfile(
                                                                profile.value.id,
                                                                {
                                                                    timeout_ms: Number(
                                                                        event.currentTarget.value,
                                                                    ),
                                                                },
                                                            )}
                                                    />
                                                </label>
                                                <label>
                                                    <span>Rate requests</span>
                                                    <input
                                                        type="number"
                                                        min="1"
                                                        value={profile.value.rate_limit.requests}
                                                        oninput={(event) =>
                                                            controller.stageTaskProfile(
                                                                profile.value.id,
                                                                {
                                                                    rate_limit: {
                                                                        ...profile.value.rate_limit,
                                                                        requests: Number(
                                                                            event.currentTarget
                                                                                .value,
                                                                        ),
                                                                    },
                                                                },
                                                            )}
                                                    />
                                                </label>
                                                <label>
                                                    <span>Rate period (seconds)</span>
                                                    <input
                                                        type="number"
                                                        min="1"
                                                        value={profile.value.rate_limit.per_seconds}
                                                        oninput={(event) =>
                                                            controller.stageTaskProfile(
                                                                profile.value.id,
                                                                {
                                                                    rate_limit: {
                                                                        ...profile.value.rate_limit,
                                                                        per_seconds: Number(
                                                                            event.currentTarget
                                                                                .value,
                                                                        ),
                                                                    },
                                                                },
                                                            )}
                                                    />
                                                </label>
                                                <label>
                                                    <span>동시 실행 제한</span>
                                                    <input
                                                        type="number"
                                                        min="1"
                                                        value={profile.value.concurrency_limit}
                                                        oninput={(event) =>
                                                            controller.stageTaskProfile(
                                                                profile.value.id,
                                                                {
                                                                    concurrency_limit: Number(
                                                                        event.currentTarget.value,
                                                                    ),
                                                                },
                                                            )}
                                                    />
                                                </label>
                                            </div>
                                            {#if profileValidationError}
                                                <p class="inline-diagnostic" role="alert">
                                                    {profileValidationError}
                                                </p>
                                            {/if}
                                            <div class="row-actions">
                                                <button
                                                    type="button"
                                                    disabled={!profile.dirty ||
                                                        orchestrationState.editable_task_profiles_loading ||
                                                        profile.value.route_id === '' ||
                                                        profile.value.generation_preset_id === '' ||
                                                        profileValidationError !== null}
                                                    onclick={() =>
                                                        void controller.saveTaskProfile(
                                                            profile.value.id,
                                                        )}
                                                >
                                                    저장
                                                </button>
                                                <button
                                                    class="danger"
                                                    type="button"
                                                    aria-pressed={pendingTaskProfileDeleteId ===
                                                        profile.value.id}
                                                    onclick={() =>
                                                        void confirmTaskProfileDelete(
                                                            profile.value.id,
                                                        )}
                                                >
                                                    {pendingTaskProfileDeleteId === profile.value.id
                                                        ? '삭제 확인'
                                                        : '삭제'}
                                                </button>
                                                {#if pendingTaskProfileDeleteId === profile.value.id}
                                                    <button
                                                        type="button"
                                                        onclick={() =>
                                                            (pendingTaskProfileDeleteId = null)}
                                                    >
                                                        취소
                                                    </button>
                                                {/if}
                                            </div>
                                        </details>
                                    </li>
                                {/each}
                            </ul>
                            {#if orchestrationState.editable_task_profiles.length > 100}
                                <p class="bounded-note">처음 100개 작업 프로필만 표시합니다.</p>
                            {/if}
                        {/if}
                    </div>
                </div>
            </section>

            <CreatorDocumentEditors {orchestrationState} {controller} />

            <section class="studio-card" aria-labelledby="memory-title">
                <div class="section-heading">
                    <div>
                        <h3 id="memory-title">장기기억</h3>
                        <p>현재 분기에 적용되는 기록만 읽고 수정하거나 삭제합니다.</p>
                    </div>
                    <span class="count-badge">
                        {orchestrationState.workspace.memory_records.length}개
                    </span>
                </div>
                {#if appState.memory_supervisor.status !== null}
                    <p
                        class="bounded-note"
                        role={appState.memory_supervisor.status.phase === 'failed'
                            ? 'alert'
                            : 'status'}
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
                        {appState.memory_supervisor.status.recovered_interrupted_jobs}건 · 완료
                        {appState.memory_supervisor.status.completed_jobs}건
                    </p>
                {/if}
                {#if appState.memory_supervisor.error !== null}
                    <p class="bounded-note" role="status">
                        {appState.memory_supervisor.error}
                    </p>
                {/if}
                {#if orchestrationState.list_truncation.memory_records}
                    <p class="bounded-note">처음 250개 기억만 표시합니다.</p>
                {/if}
                {#if orchestrationState.workspace.memory_records.length === 0}
                    <p class="empty-note">현재 분기에 저장된 장기기억이 없습니다.</p>
                {:else}
                    <ul class="memory-list">
                        {#each orchestrationState.workspace.memory_records as record (record.id)}
                            <li>
                                <header>
                                    <div>
                                        <strong>{record.title}</strong>
                                        <span>{record.kind} · 중요도 {record.importance}</span>
                                    </div>
                                    {#if record.invalidated_at}
                                        <span class="status-badge disabled">무효화됨</span>
                                    {/if}
                                    {#if record.excluded_from_conversation}
                                        <span class="status-badge disabled"
                                            >현재 대화 선택 제외됨</span
                                        >
                                    {/if}
                                    {#if record.excluded_from_character}
                                        <span class="status-badge disabled"
                                            >캐릭터 기억 선택 제외됨</span
                                        >
                                    {/if}
                                </header>
                                {#if record.keywords.length > 0}
                                    <p class="bounded-note">
                                        키워드: {record.keywords.join(', ')}
                                    </p>
                                {/if}
                                <label>
                                    <span>요약</span>
                                    <textarea
                                        rows="3"
                                        maxlength="8192"
                                        value={memoryDraft(record)}
                                        oninput={(event) =>
                                            (memoryDrafts[record.id] = event.currentTarget.value)}
                                    ></textarea>
                                </label>
                                <div class="row-actions">
                                    <button
                                        type="button"
                                        onclick={() => void saveMemorySummary(record)}
                                    >
                                        수정 저장
                                    </button>
                                    <button
                                        type="button"
                                        aria-pressed={record.pinned}
                                        onclick={() =>
                                            void controller.setMemoryRecordPinned(
                                                record.id,
                                                !record.pinned,
                                            )}
                                    >
                                        {record.pinned ? '고정 해제' : '고정'}
                                    </button>
                                    <button
                                        type="button"
                                        aria-pressed={record.excluded_from_conversation}
                                        onclick={() =>
                                            void controller.setMemoryRecordExclusion(
                                                record.id,
                                                'conversation',
                                                !record.excluded_from_conversation,
                                            )}
                                    >
                                        {record.excluded_from_conversation
                                            ? '대화 제외 해제'
                                            : '현재 대화에서 제외'}
                                    </button>
                                    <button
                                        type="button"
                                        aria-pressed={record.excluded_from_character}
                                        onclick={() =>
                                            void controller.setMemoryRecordExclusion(
                                                record.id,
                                                'character',
                                                !record.excluded_from_character,
                                            )}
                                    >
                                        {record.excluded_from_character
                                            ? '캐릭터 제외 해제'
                                            : '캐릭터 기억에서 제외'}
                                    </button>
                                    <button
                                        type="button"
                                        onclick={() =>
                                            onNavigateToMemorySource(record.source_navigation)}
                                    >
                                        출처 메시지로 이동
                                    </button>
                                    <button
                                        class="danger"
                                        type="button"
                                        aria-pressed={pendingMemoryDeleteId === record.id}
                                        onclick={() => void confirmMemoryDelete(record.id)}
                                    >
                                        {pendingMemoryDeleteId === record.id ? '삭제 확인' : '삭제'}
                                    </button>
                                    {#if pendingMemoryDeleteId === record.id}
                                        <button
                                            type="button"
                                            onclick={() => (pendingMemoryDeleteId = null)}
                                        >
                                            취소
                                        </button>
                                    {/if}
                                </div>
                            </li>
                        {/each}
                    </ul>
                {/if}
            </section>

            <section class="studio-card split-card" aria-labelledby="knowledge-title">
                <div>
                    <div class="section-heading">
                        <div>
                            <h3 id="knowledge-title">세계관 지식 시뮬레이터</h3>
                            <p>입력에 어떤 항목이 선택되는지 실제 선택 근거로 확인합니다.</p>
                        </div>
                    </div>
                    <label>
                        <span>검사할 문장</span>
                        <textarea rows="4" maxlength="8192" bind:value={knowledgeSample}></textarea>
                    </label>
                    <button
                        type="button"
                        disabled={knowledgeSample.trim() === ''}
                        onclick={() => void controller.simulateKnowledge(knowledgeSample)}
                    >
                        활성화 시뮬레이션
                    </button>
                </div>
                <div>
                    <h4>선택 결과</h4>
                    {#if orchestrationState.knowledge_simulation === null}
                        <p class="empty-note">아직 실행하지 않았습니다.</p>
                    {:else}
                        <p>
                            예상 {orchestrationState.knowledge_simulation.total_estimated_tokens}
                            토큰
                        </p>
                        {#if orchestrationState.knowledge_simulation.truncated}
                            <p class="bounded-note" role="note">
                                Core의 안전한 응답 한도 또는 지식 예산 때문에 선택 근거 일부가
                                축약되었습니다. 이 결과를 전체 후보 목록으로 해석하지 마세요.
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
            </section>

            <section class="studio-card split-card" aria-labelledby="transform-title">
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
                        <textarea rows="5" maxlength="16384" bind:value={transformSample}
                        ></textarea>
                    </label>
                    <button
                        type="button"
                        disabled={transformRuleId === '' || transformSample === ''}
                        onclick={() =>
                            void controller.previewTransform(transformRuleId, transformSample)}
                    >
                        변환 diff 만들기
                    </button>
                </div>
                <div class="diff-preview">
                    <h4>변환 전후</h4>
                    {#if orchestrationState.transform_preview === null}
                        <p class="empty-note">미리보기 결과가 없습니다.</p>
                    {:else}
                        <p class="bounded-note" role="note">
                            출처 set <code
                                >{orchestrationState.transform_preview.transform_set_id}</code
                            >
                            · rule <code>{orchestrationState.transform_preview.rule_id}</code> ·
                            {orchestrationState.transform_preview.phase} ·
                            {orchestrationState.transform_preview.changed ? '변경됨' : '변경 없음'} ·
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
                                        <strong>
                                            {report.trace.rule_id} · {report.status}
                                        </strong>
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
                                                diff 앞 {report.diff.unchanged_prefix_chars}자 · 뒤
                                                {report.diff.unchanged_suffix_chars}자{report.diff
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
                                Core의 안전한 표시 한도에 따라 변환 입력, 출력, diff 또는 진단
                                일부가 축약되었습니다.
                            </p>
                        {/if}
                    {/if}
                </div>
            </section>

            <section class="studio-card" aria-labelledby="interactions-title">
                <div class="section-heading">
                    <div>
                        <h3 id="interactions-title">선언형 상호작용</h3>
                        <p>
                            상태와 제안을 검토하며 임의 코드, 네트워크, 파일 접근은 실행하지
                            않습니다.
                        </p>
                    </div>
                </div>
                <div class="profile-columns">
                    <div>
                        <h4>현재 상태</h4>
                        <dl class="state-list">
                            {#each orchestrationState.workspace.interaction_state.slice(0, 200) as entry (entry.id)}
                                <div>
                                    <dt>{entry.label}</dt>
                                    <dd>{JSON.stringify(entry.value).slice(0, 500)}</dd>
                                </div>
                            {/each}
                        </dl>
                        {#if orchestrationState.workspace.interaction_state.length > 200}
                            <p class="bounded-note">처음 200개 상태만 표시합니다.</p>
                        {/if}
                    </div>
                    <div>
                        <h4>사용자 승인 제안</h4>
                        <ul class="proposal-list">
                            {#each orchestrationState.workspace.interaction_proposals.slice(0, 100) as proposal (proposal.proposal.id)}
                                <li>
                                    {#if proposal.proposal.projection_rejection_reason === 'unsafe_native_text'}
                                        <strong>저장 제안 내용을 표시할 수 없음</strong>
                                        <span>
                                            안전한 표시 범위를 벗어난 원문은 숨겼습니다. 이 제안은
                                            거절만 할 수 있습니다.
                                        </span>
                                    {:else}
                                        <strong>{proposal.proposal.title}</strong>
                                        <span>{proposal.proposal.body}</span>
                                    {/if}
                                    <small>
                                        {proposal.proposal.status} · 상태 revision
                                        {proposal.state_revision} · 제안 revision
                                        {proposal.proposal_revision}
                                    </small>
                                    {#if proposal.proposal.status === 'pending'}
                                        <div class="row-actions">
                                            <button
                                                type="button"
                                                disabled={orchestrationState.busy_interaction_proposal_id !==
                                                    null ||
                                                    proposal.proposal
                                                        .projection_rejection_reason ===
                                                        'unsafe_native_text'}
                                                onclick={() =>
                                                    void controller.decideProposal(
                                                        proposal.proposal.id,
                                                        true,
                                                    )}
                                            >
                                                {orchestrationState.busy_interaction_proposal_id ===
                                                proposal.proposal.id
                                                    ? '반영 중…'
                                                    : '승인'}
                                            </button>
                                            <button
                                                type="button"
                                                disabled={orchestrationState.busy_interaction_proposal_id !==
                                                    null}
                                                onclick={() =>
                                                    void controller.decideProposal(
                                                        proposal.proposal.id,
                                                        false,
                                                    )}
                                            >
                                                거절
                                            </button>
                                        </div>
                                    {/if}
                                </li>
                            {/each}
                        </ul>
                        {#if orchestrationState.workspace.interaction_proposals.length > MAX_INLINE_ITEMS}
                            <p class="bounded-note">처음 100개 제안만 표시합니다.</p>
                        {/if}
                    </div>
                </div>
            </section>
        </div>
    {:else}
        <div
            id="orchestration-expert-panel"
            class="studio-panel"
            role="tabpanel"
            aria-labelledby="orchestration-expert-tab"
        >
            <section class="studio-card" aria-labelledby="display-transform-diagnostics-title">
                <div class="section-heading">
                    <div>
                        <h3 id="display-transform-diagnostics-title">메시지 표시 변환 진단</h3>
                        <p>
                            Core가 재오픈 시 해시를 검증한 DisplayOnly sidecar와 규칙 결과만
                            표시합니다. 정규 메시지 본문, 패턴, 치환문, 오류 원문은 이 진단에
                            포함하지 않습니다.
                        </p>
                    </div>
                </div>
                {#if displayTransformDiagnostics.items.length === 0}
                    <p class="empty-note">현재 분기에 저장된 표시 변환 진단이 없습니다.</p>
                {:else}
                    <p class="bounded-note" role="note">
                        메시지 {displayTransformDiagnostics.items.length}개 · 진단
                        {displayTransformDiagnostics.diagnosticCount}개
                    </p>
                    <ol class="message-preview-list">
                        {#each displayTransformDiagnostics.items as item (item.messageId)}
                            <li>
                                <header>
                                    <strong
                                        >메시지 <code>{item.messageId.slice(0, 256)}</code></strong
                                    >
                                    <span>{item.createdAt}</span>
                                </header>
                                <small>
                                    생성 <code>{item.generationId.slice(0, 256)}</code>
                                </small>
                                <dl class="state-list">
                                    <div>
                                        <dt>정규 내용 SHA-256</dt>
                                        <dd>
                                            <code>{item.canonicalContentSha256.slice(0, 64)}</code>
                                        </dd>
                                    </div>
                                    <div>
                                        <dt>표시 내용 SHA-256</dt>
                                        <dd>
                                            <code>{item.displayContentSha256.slice(0, 64)}</code>
                                        </dd>
                                    </div>
                                    <div>
                                        <dt>진단 SHA-256</dt>
                                        <dd><code>{item.diagnosticsSha256.slice(0, 64)}</code></dd>
                                    </div>
                                </dl>
                                {#if item.diagnostics.length === 0}
                                    <p class="empty-note">
                                        적용 규칙 또는 파이프라인 거부가 없습니다.
                                    </p>
                                {:else}
                                    <ol class="compact-list">
                                        {#each item.diagnostics as diagnostic, index (`${item.messageId}:${String(index)}:${diagnostic.stage}:${diagnostic.rule_id ?? 'pipeline'}`)}
                                            <li>
                                                <strong>
                                                    {diagnostic.stage} · {diagnostic.disposition}
                                                </strong>
                                                <span>
                                                    set revision
                                                    <code
                                                        >{diagnostic.set_revision_id?.slice(
                                                            0,
                                                            256,
                                                        ) ?? 'pipeline'}</code
                                                    >
                                                    · rule
                                                    <code
                                                        >{diagnostic.rule_id?.slice(0, 256) ??
                                                            'pipeline'}</code
                                                    >
                                                    · code
                                                    <code
                                                        >{diagnostic.code?.slice(0, 256) ??
                                                            'none'}</code
                                                    >
                                                </span>
                                                <small>
                                                    before
                                                    <code
                                                        >{diagnostic.before_sha256.slice(
                                                            0,
                                                            64,
                                                        )}</code
                                                    >
                                                    · after
                                                    <code
                                                        >{diagnostic.after_sha256?.slice(0, 64) ??
                                                            'none'}</code
                                                    >
                                                    · {diagnostic.recorded_at}
                                                </small>
                                            </li>
                                        {/each}
                                    </ol>
                                {/if}
                            </li>
                        {/each}
                    </ol>
                    {#if displayTransformDiagnostics.truncated}
                        <p class="bounded-note">
                            최신 64개 메시지와 최대 512개 진단까지만 표시합니다.
                        </p>
                    {/if}
                {/if}
            </section>

            <section class="studio-card" aria-labelledby="selection-evidence-title">
                <div class="section-heading">
                    <div>
                        <h3 id="selection-evidence-title">현재 방의 지식·기억 선택 근거</h3>
                        <p>
                            현재 분기 스냅샷에서 Core가 선택하거나 제외한 지식과 기억의 이유, 점수,
                            토큰, 삽입 위치를 표시합니다.
                        </p>
                    </div>
                </div>
                {#if orchestrationState.workspace.selection_evidence.length === 0}
                    <p class="empty-note">현재 스냅샷에 선택 근거가 없습니다.</p>
                {:else}
                    <ul class="evidence-list">
                        {#each orchestrationState.workspace.selection_evidence as evidence (evidence.id)}
                            <li class:selected={evidence.selected}>
                                <strong>{evidence.title.slice(0, 512)}</strong>
                                <span>{evidence.reason.slice(0, 4096)}</span>
                                <small>
                                    {evidence.source_kind} · {evidence.selected ? '선택' : '제외'} ·
                                    {evidence.estimated_tokens} tokens · score
                                    {evidence.score ?? '없음'} · 배치 {evidence.placement ?? '없음'}
                                </small>
                            </li>
                        {/each}
                    </ul>
                {/if}
                {#if orchestrationState.list_truncation.selection_evidence}
                    <p class="bounded-note" role="note">
                        안전한 UI 한도에 따라 처음 300개 선택 근거만 표시합니다. 전체 후보 목록으로
                        해석하지 마세요.
                    </p>
                {/if}
            </section>

            {#if contentPackageState && contentPackageController}
                <section class="studio-card" aria-labelledby="package-import-title">
                    <div class="section-heading">
                        <div>
                            <h3 id="package-import-title">LorePia 패키지 선택 가져오기</h3>
                            <p>
                                경로나 원본 바이트 없이 Core가 검사한 manifest, 라이선스, 충돌, 격리
                                결과만 검토합니다.
                            </p>
                        </div>
                        <button
                            class="primary"
                            type="button"
                            aria-label="새 LorePia 패키지 선택"
                            disabled={contentPackageState.phase === 'picking' ||
                                contentPackageState.phase === 'resuming' ||
                                contentPackageState.phase === 'selecting' ||
                                contentPackageState.phase === 'approving' ||
                                contentPackageState.phase === 'committing' ||
                                contentPackageState.phase === 'unavailable'}
                            onclick={() => void contentPackageController.pickAndInspect()}
                        >
                            패키지 선택
                        </button>
                    </div>

                    {#if contentPackageState.phase === 'unavailable'}
                        <p class="bounded-note" role="note">{contentPackageState.error}</p>
                    {:else if contentPackageState.phase === 'error'}
                        <p class="drawer-status error" role="alert">{contentPackageState.error}</p>
                    {:else if contentPackageState.phase === 'listing'}
                        <p role="status">중단된 패키지 검토를 확인하고 있습니다.</p>
                    {:else if contentPackageState.phase === 'picking'}
                        <p role="status">패키지를 선택하고 Core에서 안전하게 검사하는 중입니다.</p>
                    {:else if contentPackageState.phase === 'resuming'}
                        <p role="status">중단된 패키지 검토를 다시 여는 중입니다.</p>
                    {:else if contentPackageState.phase === 'selecting'}
                        <p role="status">
                            정규화 근거, 대상 쓰기 검토, 가져오기 계획을 계산하는 중입니다.
                        </p>
                    {:else if contentPackageState.phase === 'approving'}
                        <p role="status">
                            표시된 대상 쓰기 근거와 명시적 승인을 고정하는 중입니다.
                        </p>
                    {:else if contentPackageState.phase === 'committing'}
                        <p role="status">승인된 패키지를 원자적으로 가져오는 중입니다.</p>
                    {/if}

                    <section aria-labelledby="completed-package-exports-title">
                        <div class="section-heading">
                            <div>
                                <h4 id="completed-package-exports-title">완료된 패키지 내보내기</h4>
                                <p>재시작 후에도 Core가 다시 검증한 완료 패키지만 표시합니다.</p>
                            </div>
                            <button
                                type="button"
                                disabled={contentPackageState.completed_exports_loading}
                                onclick={() =>
                                    void contentPackageController.loadCompletedPackageExports()}
                            >
                                목록 새로고침
                            </button>
                        </div>
                        <div aria-live="polite" aria-atomic="true">
                            {#if contentPackageState.completed_exports_loading}
                                <p role="status">완료된 패키지를 다시 검증하고 있습니다.</p>
                            {/if}
                            {#if contentPackageState.completed_exports_error}
                                <p class="drawer-status error" role="alert">
                                    {contentPackageState.completed_exports_error}
                                </p>
                            {/if}
                        </div>
                        {#if contentPackageState.completed_package_exports.length === 0}
                            <p class="bounded-note">내보낼 수 있는 완료 패키지가 없습니다.</p>
                        {:else}
                            <ul class="compact-list" aria-label="완료된 패키지 내보내기 목록">
                                {#each contentPackageState.completed_package_exports.slice(0, MAX_COMPLETED_CONTENT_PACKAGE_EXPORTS) as descriptor (descriptor.source_id)}
                                    <li>
                                        <strong>{descriptor.suggested_file_name}</strong>
                                        <span>크기 {descriptor.size_bytes}바이트</span>
                                        <span>
                                            SHA-256 <code>{descriptor.sha256}</code>
                                        </span>
                                        <button
                                            type="button"
                                            aria-label={`${descriptor.suggested_file_name} 완료 패키지 내보내기`}
                                            disabled={contentPackageState.exporting_import_id !==
                                                null}
                                            onclick={() =>
                                                void contentPackageController.exportCompletedPackageFromCatalog(
                                                    descriptor.source_id,
                                                )}
                                        >
                                            {contentPackageState.exporting_import_id ===
                                            descriptor.source_id
                                                ? '내보내는 중…'
                                                : '내보내기'}
                                        </button>
                                    </li>
                                {/each}
                            </ul>
                        {/if}
                        {#if contentPackageState.completed_package_exports.length > MAX_COMPLETED_CONTENT_PACKAGE_EXPORTS}
                            <p class="bounded-note" role="note">
                                처음 {MAX_COMPLETED_CONTENT_PACKAGE_EXPORTS}개 완료 패키지만
                                표시합니다.
                            </p>
                        {/if}
                    </section>

                    {#if contentPackageState.pending_imports.length > 0}
                        <section aria-labelledby="pending-package-imports-title">
                            <h4 id="pending-package-imports-title">중단된 검토</h4>
                            <ul class="compact-list">
                                {#each contentPackageState.pending_imports.slice(0, MAX_INLINE_ITEMS) as pendingImport (pendingImport.import_id)}
                                    <li>
                                        <span>
                                            {pendingImport.package_id} · {pendingImport.status} · revision
                                            {pendingImport.revision}
                                        </span>
                                        <button
                                            type="button"
                                            disabled={contentPackageState.phase === 'picking' ||
                                                contentPackageState.phase === 'resuming' ||
                                                contentPackageState.phase === 'selecting' ||
                                                contentPackageState.phase === 'approving' ||
                                                contentPackageState.phase === 'committing'}
                                            onclick={() =>
                                                void contentPackageController.resume(
                                                    pendingImport.import_id,
                                                )}
                                        >
                                            검토 재개
                                        </button>
                                    </li>
                                {/each}
                            </ul>
                        </section>
                    {/if}

                    {#if contentPackageState.result}
                        <article class="revision-diff" aria-labelledby="package-result-title">
                            <h4 id="package-result-title">가져오기 완료</h4>
                            <p>
                                {contentPackageState.result.package_id} ·
                                {contentPackageState.result.status} · revision
                                {contentPackageState.result.revision}
                            </p>
                            <p>
                                문서 {contentPackageState.result.committed_document_ids.length}개 ·
                                자산 {contentPackageState.result.asset_ids.length}개
                            </p>
                            <button
                                type="button"
                                disabled={contentPackageState.exporting_import_id !== null}
                                onclick={() =>
                                    void contentPackageController.exportCompletedPackage()}
                            >
                                {contentPackageState.exporting_import_id === null
                                    ? '완료된 패키지 내보내기'
                                    : '내보내는 중…'}
                            </button>
                        </article>
                    {/if}

                    <div aria-live="polite" aria-atomic="true">
                        {#if contentPackageState.exporting_import_id !== null}
                            <p role="status">운영체제 저장 위치를 선택하고 있습니다.</p>
                        {/if}
                        {#if contentPackageState.export_error}
                            <p class="drawer-status error" role="alert">
                                {contentPackageState.export_error}
                            </p>
                        {/if}
                        {#if contentPackageState.export_receipt}
                            <article class="revision-diff" aria-labelledby="package-export-title">
                                <h4 id="package-export-title">최근 패키지 내보내기</h4>
                                <p>파일명 {contentPackageState.export_receipt.file_name}</p>
                                <p>크기 {contentPackageState.export_receipt.size_bytes}바이트</p>
                                <p>
                                    SHA-256
                                    <code>{contentPackageState.export_receipt.sha256}</code>
                                </p>
                            </article>
                        {/if}
                    </div>

                    {#if contentPackageState.inspection}
                        {@const packageReview = contentPackageState.inspection}
                        <article class="package-review">
                            <header>
                                <div>
                                    <h4>
                                        {packageReview.manifest.name} v{packageReview.manifest
                                            .version}
                                    </h4>
                                    <p>
                                        {packageReview.manifest.package_id} ·
                                        {packageReview.manifest.author ?? '작성자 정보 없음'}
                                    </p>
                                </div>
                                <span class="license-badge">
                                    {packageReview.manifest.license} ·
                                    {packageReview.redistribution_status}
                                </span>
                            </header>
                            <dl class="plan-summary">
                                <div>
                                    <dt>로컬 가져오기</dt>
                                    <dd>
                                        {packageReview.local_import_allowed ? '허용' : '차단'}
                                    </dd>
                                </div>
                                <div>
                                    <dt>원본 크기</dt>
                                    <dd>{packageReview.source_size_bytes} bytes</dd>
                                </div>
                                <div>
                                    <dt>압축 해제 크기</dt>
                                    <dd>{packageReview.total_uncompressed_size_bytes} bytes</dd>
                                </div>
                                <div>
                                    <dt>자산</dt>
                                    <dd>{packageReview.asset_count}개</dd>
                                </div>
                                <div>
                                    <dt>검토 해시</dt>
                                    <dd><code>{packageReview.review_sha256}</code></dd>
                                </div>
                                <div>
                                    <dt>기능 검토 해시</dt>
                                    <dd><code>{packageReview.capability_review_sha256}</code></dd>
                                </div>
                            </dl>

                            <p>
                                재배포 manifest:
                                {packageReview.manifest.redistribution_allowed
                                    ? '허용'
                                    : '허용 안 됨'}
                            </p>
                            {#if packageReview.manifest.required_app_version}
                                <p>요구 앱 버전: {packageReview.manifest.required_app_version}</p>
                            {/if}
                            {#if packageReview.manifest.required_capabilities.length > 0}
                                <p>
                                    manifest 요구 기능:
                                    {packageReview.manifest.required_capabilities
                                        .slice(0, MAX_INLINE_ITEMS)
                                        .join(', ')}
                                </p>
                            {/if}

                            {#if packageReview.capability_decisions.length > 0}
                                <h4>기능 지원 검토</h4>
                                <ul class="conflict-list">
                                    {#each packageReview.capability_decisions.slice(0, MAX_INLINE_ITEMS) as decision (decision.capability)}
                                        <li>
                                            {decision.capability} · {decision.support} ·
                                            {decision.approved ? '검토 통과' : '미승인'} ·
                                            {decision.reason.slice(0, 4096)}
                                        </li>
                                    {/each}
                                </ul>
                            {/if}

                            <fieldset>
                                <legend>가져올 구성요소</legend>
                                {#each packageReview.components.slice(0, MAX_MODULE_COMPONENTS) as component (component.id)}
                                    <label class="component-choice">
                                        <input
                                            type="checkbox"
                                            checked={contentPackageState.selected_component_ids.includes(
                                                component.id,
                                            )}
                                            disabled={contentPackageState.phase !== 'ready' ||
                                                component.disposition !== 'importable'}
                                            onchange={() =>
                                                contentPackageController.toggleComponent(
                                                    component.id,
                                                )}
                                        />
                                        <span>
                                            {component.id} · {component.kind}
                                            <small>
                                                {component.disposition} · 자산 {component.asset_count}개
                                            </small>
                                        </span>
                                    </label>
                                    {#if component.required_capabilities.length > 0}
                                        <p>
                                            요구 기능:
                                            {component.required_capabilities
                                                .slice(0, MAX_INLINE_ITEMS)
                                                .join(', ')}
                                        </p>
                                    {/if}
                                    {#if component.dependency_ids.length > 0}
                                        <p>
                                            의존:
                                            {component.dependency_ids
                                                .slice(0, MAX_INLINE_ITEMS)
                                                .join(', ')}
                                        </p>
                                    {/if}
                                    {#if component.conflict_ids.length > 0}
                                        <p>
                                            충돌:
                                            {component.conflict_ids
                                                .slice(0, MAX_INLINE_ITEMS)
                                                .join(', ')}
                                        </p>
                                    {/if}
                                {/each}
                            </fieldset>
                            {#if packageReview.components.length > MAX_MODULE_COMPONENTS}
                                <p class="bounded-note">처음 200개 구성요소만 표시합니다.</p>
                            {/if}

                            {#if packageReview.issues.length > 0}
                                <h4>검사 결과</h4>
                                <ul class="conflict-list">
                                    {#each packageReview.issues.slice(0, MAX_INLINE_ITEMS) as issue, index (`${issue.severity}:${issue.code}:${String(index)}`)}
                                        <li>
                                            {issue.severity} · {issue.code} ·
                                            <span>{issue.message.slice(0, 4096)}</span>
                                        </li>
                                    {/each}
                                </ul>
                            {/if}

                            {#if contentPackageState.selection}
                                {@const packageSelection = contentPackageState.selection}
                                <article
                                    class="revision-diff"
                                    aria-labelledby="package-normalization-title"
                                >
                                    <h4 id="package-normalization-title">승인 전 정규화 근거</h4>
                                    <p>
                                        아래 변경과 해시를 확인해야만 승인할 수 있습니다. 정규화
                                        근거 해시
                                        <code>{packageSelection.normalization_evidence_sha256}</code
                                        >
                                    </p>
                                    <p>
                                        선택 계획
                                        <code>{packageSelection.content_selection_plan_hash}</code>
                                        · 가져오기 계획
                                        <code>{packageSelection.import_plan_sha256}</code>
                                    </p>
                                    {#if packageSelection.normalization_evidence.length > 0}
                                        <ul class="compact-list">
                                            {#each packageSelection.normalization_evidence.slice(0, MAX_INLINE_ITEMS) as evidence (`${evidence.component_id}:${evidence.object_id}:${evidence.field}`)}
                                                <li>
                                                    {evidence.component_id} / {evidence.object_id} ·
                                                    {evidence.field}:
                                                    {evidence.before ? '켜짐' : '꺼짐'} →
                                                    {evidence.after ? '켜짐' : '꺼짐'} ·
                                                    {evidence.reason.slice(0, 4096)}
                                                </li>
                                            {/each}
                                        </ul>
                                    {:else}
                                        <p>활성 상태를 자동으로 낮춘 항목이 없습니다.</p>
                                    {/if}
                                </article>

                                <article
                                    class="revision-diff"
                                    aria-labelledby="package-target-review-title"
                                >
                                    <h4 id="package-target-review-title">대상 쓰기 검토</h4>
                                    <p>
                                        대상 검토 SHA-256
                                        <code
                                            >{packageSelection.target_review
                                                .target_review_sha256}</code
                                        >
                                    </p>
                                    <p class="bounded-note">
                                        기존 대상을 갱신하는 문서는 대상 리비전과 상태 CAS를 각각
                                        확인해야 합니다. 새 대상 생성은 별도 확인이 필요하지
                                        않습니다.
                                    </p>
                                    {#if packageSelection.target_review.documents.length === 0}
                                        <p>가져오기 계획에 쓸 문서 대상이 없습니다.</p>
                                    {:else}
                                        <ol class="compact-list" aria-label="패키지 문서 대상 검토">
                                            {#each packageSelection.target_review.documents.slice(0, MAX_VISIBLE_CONTENT_PACKAGE_TARGET_DOCUMENTS) as document (`${document.source_component_id}:${String(document.component_document_ordinal)}`)}
                                                <li>
                                                    <strong>
                                                        {document.source_component_id} · 전체 문서 인덱스
                                                        {document.document_index} · 구성요소 문서 순서
                                                        {document.component_document_ordinal}
                                                    </strong>
                                                    <span>
                                                        소스 구성요소 SHA-256
                                                        <code
                                                            >{document.source_component_sha256}</code
                                                        >
                                                    </span>
                                                    <span>
                                                        종류 <code>{document.document_kind}</code> ·
                                                        대상
                                                        <code>{document.target_object_id}</code> ·
                                                        처리
                                                        {document.disposition}
                                                    </span>
                                                    <span>
                                                        기대 불변 리비전
                                                        <code
                                                            >{document.expected_target_revision_id ??
                                                                '없음'}</code
                                                        >
                                                        · 기대 상태 CAS
                                                        {document.expected_target_state_revision ??
                                                            '없음'}
                                                    </span>
                                                    <span>
                                                        문서 SHA-256
                                                        <code>{document.document_sha256}</code>
                                                    </span>
                                                    {#if document.disposition === 'update'}
                                                        <label class="component-choice">
                                                            <input
                                                                type="checkbox"
                                                                aria-label={`${document.target_object_id} 기존 대상 업데이트 확인`}
                                                                checked={updateTargetConfirmed(
                                                                    document,
                                                                )}
                                                                disabled={contentPackageState.phase !==
                                                                    'selection_ready'}
                                                                onchange={() =>
                                                                    contentPackageController.toggleUpdateTargetConfirmation(
                                                                        document.source_component_id,
                                                                        document.component_document_ordinal,
                                                                    )}
                                                            />
                                                            <span>
                                                                이 불변 리비전과 상태 CAS의 기존
                                                                대상 업데이트 확인
                                                            </span>
                                                        </label>
                                                    {:else}
                                                        <span class="bounded-note">
                                                            새 대상 생성 — 별도 업데이트 확인 불필요
                                                        </span>
                                                    {/if}
                                                </li>
                                            {/each}
                                        </ol>
                                    {/if}
                                    {#if packageSelection.target_review.documents.length > MAX_VISIBLE_CONTENT_PACKAGE_TARGET_DOCUMENTS}
                                        <p class="bounded-note" role="note">
                                            처음 {MAX_VISIBLE_CONTENT_PACKAGE_TARGET_DOCUMENTS}개
                                            대상 문서만 표시합니다. 숨겨진 업데이트 대상이 있으면
                                            승인할 수 없습니다.
                                        </p>
                                    {/if}
                                </article>

                                <p>
                                    선택에서 요구된 기능:
                                    {contentPackageState.required_capabilities.length > 0
                                        ? contentPackageState.required_capabilities.join(', ')
                                        : '없음'}
                                </p>

                                <fieldset>
                                    <legend>가져온 뒤 활성화할 구성요소</legend>
                                    <p class="bounded-note">
                                        선택하지 않은 구성요소도 가져오지만 비활성 상태를
                                        유지합니다.
                                    </p>
                                    {#each contentPackageState.selected_component_ids as componentId (componentId)}
                                        <label class="component-choice">
                                            <input
                                                type="checkbox"
                                                checked={contentPackageState.enabled_component_ids.includes(
                                                    componentId,
                                                )}
                                                disabled={contentPackageState.phase !==
                                                    'selection_ready'}
                                                onchange={() =>
                                                    contentPackageController.toggleEnabledComponent(
                                                        componentId,
                                                    )}
                                            />
                                            <span>{componentId} 활성화</span>
                                        </label>
                                    {/each}
                                </fieldset>

                                {@const approvalCapabilities =
                                    contentPackageState.required_capabilities.filter(
                                        packageCapabilityNeedsApproval,
                                    )}
                                {#if approvalCapabilities.length > 0}
                                    <fieldset>
                                        <legend>명시적으로 승인할 기능</legend>
                                        {#each approvalCapabilities as capability (capability)}
                                            <label class="component-choice">
                                                <input
                                                    type="checkbox"
                                                    checked={contentPackageState.approved_capabilities.includes(
                                                        capability,
                                                    )}
                                                    disabled={contentPackageState.phase !==
                                                        'selection_ready'}
                                                    onchange={() =>
                                                        contentPackageController.toggleApprovedCapability(
                                                            capability,
                                                        )}
                                                />
                                                <span>{capability} 기능 승인</span>
                                            </label>
                                        {/each}
                                    </fieldset>
                                {:else}
                                    <p class="bounded-note">추가 승인이 필요한 기능은 없습니다.</p>
                                {/if}
                            {/if}

                            {#if contentPackageState.approval}
                                <article
                                    class="revision-diff"
                                    aria-labelledby="package-approval-title"
                                >
                                    <h4 id="package-approval-title">고정된 명시적 승인</h4>
                                    <p>
                                        승인 해시
                                        <code>{contentPackageState.approval.approval_sha256}</code>
                                    </p>
                                    <p>
                                        활성 구성요소:
                                        {contentPackageState.approval.enabled_component_ids.length >
                                        0
                                            ? contentPackageState.approval.enabled_component_ids.join(
                                                  ', ',
                                              )
                                            : '없음'}
                                    </p>
                                    <p>
                                        승인 기능:
                                        {contentPackageState.approval.approved_capabilities.length >
                                        0
                                            ? contentPackageState.approval.approved_capabilities.join(
                                                  ', ',
                                              )
                                            : '없음'}
                                    </p>
                                </article>
                            {/if}

                            <div class="row-actions">
                                <button
                                    type="button"
                                    disabled={contentPackageState.phase !== 'ready' ||
                                        contentPackageState.selected_component_ids.length === 0 ||
                                        !packageReview.local_import_allowed ||
                                        packageReview.issues.some(
                                            (issue) => issue.severity === 'blocker',
                                        )}
                                    onclick={() => void contentPackageController.reviewSelection()}
                                >
                                    선택 및 정규화 검토
                                </button>
                                <button
                                    class="primary"
                                    type="button"
                                    disabled={contentPackageState.phase !== 'selection_ready' ||
                                        !packageUpdateTargetsConfirmed ||
                                        contentPackageState.required_capabilities
                                            .filter(packageCapabilityNeedsApproval)
                                            .some(
                                                (capability) =>
                                                    !contentPackageState.approved_capabilities.includes(
                                                        capability,
                                                    ),
                                            )}
                                    onclick={() => void contentPackageController.approve()}
                                >
                                    표시된 근거와 기능 명시적 승인
                                </button>
                                <button
                                    class="primary"
                                    type="button"
                                    disabled={contentPackageState.phase !== 'approved'}
                                    onclick={() => void contentPackageController.commit()}
                                >
                                    승인된 패키지 가져오기 확정
                                </button>
                                <button
                                    class="danger"
                                    type="button"
                                    disabled={contentPackageState.phase === 'committing' ||
                                        contentPackageState.phase === 'picking'}
                                    onclick={() => void contentPackageController.discard()}
                                >
                                    검토 폐기
                                </button>
                            </div>
                        </article>
                    {/if}
                </section>
            {/if}

            <ContentModuleLifecyclePanel
                {client}
                conversationId={orchestrationState.workspace.room_config.conversation_id || null}
                branchId={orchestrationState.workspace.room_config.branch_id || null}
            />

            <section class="studio-card" aria-labelledby="plan-preview-title">
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
                    <GenerationAttemptApprovals
                        {client}
                        conversationId={orchestrationState.workspace.room_config.conversation_id ||
                            null}
                        sourceBranchId={orchestrationState.workspace.room_config.branch_id || null}
                        headingId="studio-generation-attempt-approvals-title"
                        refreshEpoch={attemptApprovalRefreshEpoch}
                        retryLabel="최종 계획 다시 검토"
                        onRetry={resolvePlanPreviewAndRefreshRetries}
                    />
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
                        placeholder="실제 전송 전 계획을 계산할 메시지를 입력하세요."></textarea>
                </label>
                {#if previewGenerationTarget === null}
                    <p class="bounded-note" role="note">
                        저장된 모델 라우트와 생성 프리셋을 먼저 선택해야 계획을 계산할 수 있습니다.
                    </p>
                {:else}
                    <p class="inline-note">
                        생성 대상: {previewGenerationTarget.model_route_id} ·
                        {previewGenerationTarget.generation_preset_id}
                    </p>
                {/if}
                <div class="button-row">
                    <button
                        class="primary"
                        type="button"
                        disabled={orchestrationState.workspace.room_config.conversation_id === '' ||
                            planUserText.trim() === '' ||
                            previewGenerationTarget === null}
                        onclick={() => void resolvePlanPreviewAndRefreshRetries()}
                    >
                        계획 다시 계산
                    </button>
                    <button
                        type="button"
                        disabled={orchestrationState.workspace.room_config.conversation_id === '' ||
                            planUserText.trim() === '' ||
                            previewGenerationTarget === null}
                        onclick={() => void resolveNewPlanPreviewAndRefreshRetries()}
                    >
                        새 작업 미리보기
                    </button>
                </div>
                <p class="bounded-note" role="note">
                    최초 응답 전 재시도는 현재 작업 nonce를 유지합니다. 계획 응답이나 승인을 받은
                    뒤에는 고정된 생성 시도 ID로 재개합니다. 다른 작업을 시작하려면 새 작업
                    미리보기를 선택하세요.
                </p>
                {#if appController !== undefined}
                    <MemoryQueryRetryPanel
                        state={appState.memory_query_retries}
                        controller={appController}
                        headingId="studio-memory-query-retry-title"
                    />
                {/if}
                {#if orchestrationState.workspace.plan_preview === null}
                    <p class="empty-note">계획 미리보기를 계산하지 않았습니다.</p>
                {:else}
                    {@const preview = orchestrationState.workspace.plan_preview}
                    <dl class="plan-summary">
                        <div>
                            <dt>계획 ID</dt>
                            <dd><code>{boundedPlanIdentifier(preview.plan_id)}</code></dd>
                        </div>
                        <div>
                            <dt>생성 시도 ID</dt>
                            <dd>
                                <code>{boundedPlanIdentifier(preview.generation_attempt_id)}</code>
                            </dd>
                        </div>
                        <div>
                            <dt>작업 nonce</dt>
                            <dd>
                                {#if orchestrationState.plan_operation_nonce === null}
                                    <span>기존 생성 시도 재개</span>
                                {:else}
                                    <code
                                        >{boundedPlanIdentifier(
                                            orchestrationState.plan_operation_nonce,
                                        )}</code
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
                            <dd>
                                {preview.prompt_preset_id} · revision
                                {preview.prompt_preset_revision}
                            </dd>
                        </div>
                        <div>
                            <dt>입력 토큰</dt>
                            <dd>
                                {preview.estimated_input_tokens} ·
                                {preview.token_estimator_id} ·
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

                    <button
                        class="primary"
                        type="button"
                        disabled={appController === undefined ||
                            reviewedSendBusy ||
                            controller.reviewedPromptSendInput() === null}
                        onclick={() => void sendReviewedPlan()}
                    >
                        {reviewedSendBusy ? '검토한 계획 전송 중…' : '검토한 계획으로 전송'}
                    </button>
                    <p class="bounded-note" role="note">
                        이 버튼은 위 미리보기의 시도 ID와 계획 해시를 모두 다시 검증한 뒤에만
                        전송합니다. 일반 채팅 전송은 별도의 검토되지 않은 동작입니다.
                    </p>

                    <div class="expert-preview-controls">
                        <label>
                            <span>최종 미리보기 검색</span>
                            <input
                                type="search"
                                maxlength="256"
                                bind:value={expertSearch}
                                placeholder="블록, 역할, 배치, 파라미터, 구조 diff"
                            />
                        </label>
                        <label>
                            <span>표시 필터</span>
                            <select
                                value={expertFilter}
                                onchange={(event) =>
                                    (expertFilter = event.currentTarget
                                        .value as typeof expertFilter)}
                            >
                                <option value="all">전체</option>
                                <option value="messages">최종 메시지 구조</option>
                                <option value="provider">제공자 변환 구조</option>
                                <option value="parameters">적용 파라미터</option>
                                <option value="diff">역할·배치 diff</option>
                            </select>
                        </label>
                    </div>
                    <p class="bounded-note" role="note">
                        비공개 프롬프트 본문과 원시 제공자 요청은 Rust 내부에만 유지됩니다. 이
                        화면은 검토한 계획을 식별하는 해시와 구조화된 메타데이터만 표시합니다.
                    </p>
                    {@const messageResults = preview.messages.filter((message) =>
                        expertMatches(
                            message.block_id,
                            message.block_kind,
                            message.requested_role,
                            message.effective_role,
                        ),
                    )}
                    {@const providerResults = preview.provider_messages.filter((message) =>
                        expertMatches(
                            message.block_id,
                            message.effective_role,
                            message.wire_role,
                            message.placement,
                        ),
                    )}
                    {@const parameterResults = preview.applied_parameters.filter((parameter) =>
                        expertMatches(parameter.field, parameter.value_kind, parameter.item_count),
                    )}
                    {@const diffResults = preview.prompt_diff.filter((entry) =>
                        expertMatches(
                            entry.block_id,
                            entry.requested_role,
                            entry.effective_role,
                            entry.wire_role,
                            entry.placement,
                        ),
                    )}

                    {#if expertFilter === 'all' || expertFilter === 'messages'}
                        <details class="expert-preview-section">
                            <summary>최종 메시지 구조 ({messageResults.length}개)</summary>
                            {#if messageResults.length === 0}
                                <p class="empty-note">검색과 일치하는 메시지가 없습니다.</p>
                            {:else}
                                <ol class="message-preview-list">
                                    {#each messageResults.slice(0, MAX_PLAN_MESSAGES) as message (`message:${String(message.sequence)}:${message.block_id}`)}
                                        <li>
                                            <header>
                                                <strong>
                                                    {message.requested_role} → {message.effective_role}
                                                </strong>
                                                <span>{message.estimated_tokens} tokens</span>
                                            </header>
                                            <small>
                                                순서 {message.sequence} · 블록 {message.block_id} ·
                                                {message.block_kind} · 출처 메시지
                                                {message.source_message_ids.length}개
                                                {message.truncated ? ' · 목록 축약' : ''}
                                            </small>
                                        </li>
                                    {/each}
                                </ol>
                                {#if messageResults.length > MAX_PLAN_MESSAGES}
                                    <p class="bounded-note">
                                        검색 결과 중 처음 200개 메시지 구조만 표시합니다.
                                    </p>
                                {/if}
                            {/if}
                        </details>
                    {/if}

                    {#if expertFilter === 'all' || expertFilter === 'provider'}
                        <details class="expert-preview-section">
                            <summary>제공자 변환 구조 ({providerResults.length}개)</summary>
                            <p class="bounded-note">
                                제공자 계열 <code>{preview.provider_family}</code> · 캐시 경계
                                {preview.provider_cache_boundaries.length}개
                            </p>
                            {#if providerResults.length === 0}
                                <p class="empty-note">검색과 일치하는 변환이 없습니다.</p>
                            {:else}
                                <ol class="message-preview-list">
                                    {#each providerResults.slice(0, MAX_PLAN_MESSAGES) as message (`provider:${String(message.sequence)}:${message.block_id}`)}
                                        <li>
                                            <header>
                                                <strong>
                                                    {message.effective_role} → {message.wire_role}
                                                </strong>
                                                <span>{message.estimated_tokens} tokens</span>
                                            </header>
                                            <small>
                                                순서 {message.sequence} · 블록 {message.block_id} · 배치
                                                {message.placement}
                                            </small>
                                        </li>
                                    {/each}
                                </ol>
                            {/if}
                            {#if preview.provider_cache_boundaries.length > 0}
                                <ul class="compact-list">
                                    {#each preview.provider_cache_boundaries.slice(0, MAX_INLINE_ITEMS) as boundary (boundary.boundary_id)}
                                        <li>
                                            {boundary.after_block_id} 뒤 · {boundary.mode} ·
                                            {boundary.ttl} ·
                                            {#if boundary.disposition.disposition === 'mapped'}
                                                매핑 {boundary.disposition.strategy}
                                            {:else if boundary.disposition.disposition === 'ignored'}
                                                무시 {boundary.disposition.warning}
                                            {:else}
                                                직접 지시 없음
                                            {/if}
                                        </li>
                                    {/each}
                                </ul>
                            {/if}
                        </details>
                    {/if}

                    {#if expertFilter === 'all' || expertFilter === 'parameters'}
                        <details class="expert-preview-section">
                            <summary>적용 파라미터 구조 ({parameterResults.length}개)</summary>
                            {#if parameterResults.length === 0}
                                <p class="empty-note">검색과 일치하는 파라미터가 없습니다.</p>
                            {:else}
                                <dl class="state-list">
                                    {#each parameterResults.slice(0, MAX_PLAN_DETAILS) as parameter (parameter.field)}
                                        <div>
                                            <dt>{parameter.field}</dt>
                                            <dd>
                                                <code>{parameter.value_kind}</code>
                                                {#if parameter.item_count !== null}
                                                    · 항목 {parameter.item_count}개
                                                {/if}
                                            </dd>
                                        </div>
                                    {/each}
                                </dl>
                                {#if parameterResults.length > MAX_PLAN_DETAILS}
                                    <p class="bounded-note">
                                        검색 결과 중 처음 300개 파라미터만 표시합니다.
                                    </p>
                                {/if}
                            {/if}
                        </details>
                    {/if}

                    {#if expertFilter === 'all' || expertFilter === 'diff'}
                        <details class="expert-preview-section">
                            <summary>역할·배치 구조 diff ({diffResults.length}개)</summary>
                            {#if diffResults.length === 0}
                                <p class="empty-note">검색과 일치하는 변경이 없습니다.</p>
                            {:else}
                                <ul class="compact-list">
                                    {#each diffResults.slice(0, MAX_PLAN_DETAILS) as entry (`diff:${String(entry.sequence)}:${entry.block_id}`)}
                                        <li>
                                            <strong>{entry.block_id} · 순서 {entry.sequence}</strong
                                            >
                                            <span>
                                                {entry.requested_role} → {entry.effective_role} →
                                                {entry.wire_role} · {entry.placement}
                                            </span>
                                        </li>
                                    {/each}
                                </ul>
                                {#if diffResults.length > MAX_PLAN_DETAILS}
                                    <p class="bounded-note">
                                        검색 결과 중 처음 300개 diff 항목만 표시합니다.
                                    </p>
                                {/if}
                            {/if}
                        </details>
                    {/if}

                    <h4>블록별 토큰·축소 결과</h4>
                    <div class="data-table-wrap">
                        <table>
                            <caption class="sr-only">해결된 프롬프트 블록</caption>
                            <thead>
                                <tr>
                                    <th scope="col">블록</th>
                                    <th scope="col">권한·출처</th>
                                    <th scope="col">원래/최종 토큰</th>
                                    <th scope="col">메시지</th>
                                    <th scope="col">결과</th>
                                </tr>
                            </thead>
                            <tbody>
                                {#each preview.blocks.slice(0, MAX_PLAN_DETAILS) as block (block.block_id)}
                                    <tr>
                                        <th scope="row">{block.block_id} · {block.block_kind}</th>
                                        <td>
                                            {block.source.authority} · {block.source.source_kind}
                                            <br />
                                            {block.source.source_id ?? '로컬 출처'}
                                            {#if block.source.source_revision}
                                                · rev {block.source.source_revision}
                                            {/if}
                                            {#if block.source.source_hash}
                                                · sha256 {block.source.source_hash.slice(0, 12)}…
                                            {/if}
                                        </td>
                                        <td>
                                            {block.original_estimated_tokens} /
                                            {block.final_estimated_tokens}
                                        </td>
                                        <td>{block.produced_message_count}</td>
                                        <td>
                                            {block.status}{block.truncated
                                                ? ' · 근거 목록 축약'
                                                : ''}
                                        </td>
                                    </tr>
                                {/each}
                            </tbody>
                        </table>
                    </div>
                    {#if preview.blocks.length > MAX_PLAN_DETAILS}
                        <p class="bounded-note">처음 300개 블록 결과만 표시합니다.</p>
                    {/if}
                    {@const memoryEvidenceBlocks = preview.blocks.filter(
                        (block) => block.memory_evidence.length > 0,
                    )}
                    {@const knowledgeEvidenceBlocks = preview.blocks.filter(
                        (block) => block.knowledge_evidence.length > 0,
                    )}
                    {#if knowledgeEvidenceBlocks.length > 0}
                        <h4>세계관 지식 선택 근거</h4>
                        {#each knowledgeEvidenceBlocks.slice(0, MAX_PLAN_DETAILS) as block (`knowledge-evidence:${block.block_id}`)}
                            <details>
                                <summary>
                                    {block.block_id} · 후보 {block.knowledge_evidence.length}개
                                </summary>
                                <ul class="compact-list">
                                    {#each block.knowledge_evidence.slice(0, MAX_INLINE_ITEMS) as evidence (evidence.entry_id)}
                                        <li>
                                            <strong>
                                                {evidence.entry_id} ·
                                                {evidence.selected ? '선택' : '제외'}
                                            </strong>
                                            <span>
                                                {evidence.estimated_tokens} tokens ·
                                                {evidence.exclusion_code ??
                                                    boundedJson(evidence.reasons, 4096)}
                                            </span>
                                        </li>
                                    {/each}
                                </ul>
                                {#if block.knowledge_evidence.length > MAX_INLINE_ITEMS}
                                    <p class="bounded-note" role="note">
                                        처음 100개 지식 후보 근거만 표시합니다. 전체 후보 목록으로
                                        해석하지 마세요.
                                    </p>
                                {/if}
                            </details>
                        {/each}
                        {#if knowledgeEvidenceBlocks.length > MAX_PLAN_DETAILS}
                            <p class="bounded-note" role="note">
                                처음 300개 지식 근거 블록만 표시합니다.
                            </p>
                        {/if}
                    {/if}
                    {#if memoryEvidenceBlocks.length > 0}
                        <h4>메모리 선택 근거</h4>
                        {#each memoryEvidenceBlocks.slice(0, MAX_PLAN_DETAILS) as block (`memory-evidence:${block.block_id}`)}
                            <details>
                                <summary>
                                    {block.block_id} · 후보 {block.memory_evidence.length}개
                                </summary>
                                <ul class="compact-list">
                                    {#each block.memory_evidence.slice(0, MAX_INLINE_ITEMS) as evidence (evidence.record_id)}
                                        <li>
                                            {evidence.record_id} ·
                                            {evidence.selected ? '선택' : '제외'} · lane {evidence.lane ??
                                                'none'} · rank
                                            {evidence.rank_millionths ?? 'none'} ·
                                            {evidence.estimated_tokens} tokens ·
                                            {evidence.exclusion_code ??
                                                boundedJson(evidence.reasons)}
                                        </li>
                                    {/each}
                                </ul>
                                {#if block.memory_evidence.length > MAX_INLINE_ITEMS}
                                    <p class="bounded-note">처음 100개 후보 근거만 표시합니다.</p>
                                {/if}
                            </details>
                        {/each}
                    {/if}

                    <div class="profile-columns">
                        <div>
                            <h4>역할 매핑</h4>
                            <ul class="compact-list">
                                {#each preview.role_mappings.slice(0, MAX_PLAN_DETAILS) as mapping (`${mapping.block_id}:${mapping.requested_role}:${mapping.effective_role}`)}
                                    <li>
                                        {mapping.block_id}: {mapping.requested_role} →
                                        {mapping.effective_role}
                                    </li>
                                {/each}
                            </ul>
                            {#if preview.role_mappings.length > MAX_PLAN_DETAILS}
                                <p class="bounded-note">처음 300개 역할 매핑만 표시합니다.</p>
                            {/if}
                        </div>
                        <div>
                            <h4>캐시 계획</h4>
                            <ul class="compact-list">
                                {#each preview.cache_directives.slice(0, MAX_INLINE_ITEMS) as cache (cache.boundary_id)}
                                    <li>
                                        {cache.after_block_id} 뒤 · {cache.mode} · {cache.status} ·
                                        {cache.ttl}
                                    </li>
                                {/each}
                            </ul>
                            {#if preview.cache_directives.length > MAX_INLINE_ITEMS}
                                <p class="bounded-note">처음 100개 캐시 항목만 표시합니다.</p>
                            {/if}
                            {#if preview.overflow.length > 0}
                                <h4>오버플로 처리</h4>
                                <ul class="compact-list">
                                    {#each preview.overflow.slice(0, MAX_INLINE_ITEMS) as overflow (`${overflow.block_id}:${overflow.policy}`)}
                                        <li>
                                            {overflow.block_id} · {overflow.policy} ·
                                            {overflow.tokens_before} → {overflow.tokens_after}
                                        </li>
                                    {/each}
                                </ul>
                            {/if}
                            {#if preview.warnings.length > 0}
                                <h4>경고</h4>
                                <ul class="conflict-list">
                                    {#each preview.warnings.slice(0, MAX_INLINE_ITEMS) as warning, index (`${String(index)}:${warning}`)}
                                        <li>{warning.slice(0, 4096)}</li>
                                    {/each}
                                </ul>
                                {#if preview.warnings.length > MAX_INLINE_ITEMS}
                                    <p class="bounded-note">처음 100개 경고만 표시합니다.</p>
                                {/if}
                            {/if}
                        </div>
                    </div>
                    {#if preview.truncated}
                        <p class="bounded-note">
                            안전한 표시 한도에 따라 일부 세부정보를 줄였습니다.
                        </p>
                    {/if}
                {/if}
            </section>
        </div>
    {/if}
</section>

<style>
    .orchestration-studio {
        display: grid;
        gap: 14px;
        padding: 20px;
        border: 1px solid var(--line);
        border-radius: var(--radius-md);
        background: var(--surface-raised);
    }

    .studio-header,
    .section-heading,
    .package-review > header,
    .memory-list header,
    .message-preview-list header {
        display: flex;
        gap: 14px;
        align-items: center;
        justify-content: space-between;
    }

    .studio-header h2,
    .studio-card h3,
    .studio-card h4,
    .studio-header p,
    .section-heading p {
        margin: 3px 0 0;
    }

    .studio-header > div > p:last-child,
    .section-heading p {
        color: var(--ink-muted);
    }

    .studio-tabs {
        display: flex;
        gap: 6px;
        padding: 4px;
        border-radius: 12px;
        background: var(--surface-muted);
    }

    .studio-tabs button {
        flex: 1;
    }

    .studio-tabs button[aria-selected='true'] {
        color: var(--accent-ink);
        background: var(--surface-raised);
        box-shadow: 0 2px 8px rgb(18 25 38 / 8%);
    }

    .studio-panel {
        display: grid;
        gap: 16px;
    }

    .studio-card {
        display: grid;
        gap: 14px;
        padding: 16px;
        border: 1px solid var(--line);
        border-radius: 13px;
        background: var(--surface);
    }

    .studio-status,
    .empty-note,
    .bounded-note {
        margin: 0;
        padding: 11px 13px;
        border-radius: 10px;
        color: var(--ink-muted);
        background: var(--surface-muted);
    }

    .studio-status.error,
    .conflict-list {
        color: var(--danger);
    }

    .bounded-note {
        color: var(--warning-ink, #7a4b00);
    }

    .search-field,
    .studio-card > label,
    .split-card > div > label,
    .memory-list label {
        display: grid;
        gap: 6px;
    }

    .block-discovery-controls {
        display: grid;
        grid-template-columns: minmax(220px, 2fr) repeat(2, minmax(150px, 1fr)) auto;
        gap: 10px;
        align-items: end;
    }

    .block-discovery-controls label {
        display: grid;
        gap: 6px;
    }

    .block-minimap {
        display: grid;
        gap: 8px;
        padding: 10px;
        border: 1px solid var(--line);
        border-radius: 12px;
        background: var(--surface-muted);
    }

    .block-minimap > span {
        color: var(--ink-muted);
        font-size: 0.8rem;
        font-weight: 700;
    }

    .block-minimap ol {
        display: flex;
        gap: 6px;
        padding: 0;
        margin: 0;
        overflow-x: auto;
        list-style: none;
    }

    .block-minimap button {
        display: grid;
        min-width: 108px;
        gap: 2px;
        text-align: left;
    }

    .block-minimap button.active {
        border-color: var(--accent);
        color: var(--accent);
    }

    .block-minimap small {
        color: var(--ink-muted);
    }

    .block-groups,
    .block-list,
    .compact-list,
    .memory-list,
    .evidence-list,
    .proposal-list,
    .message-preview-list {
        display: grid;
        gap: 9px;
        margin: 0;
        padding: 0;
        list-style: none;
    }

    .block-group {
        display: grid;
        gap: 8px;
    }

    .block-group > header {
        display: flex;
        align-items: baseline;
        justify-content: space-between;
        color: var(--ink-muted);
    }

    .block-group h4 {
        color: var(--ink);
    }

    .block-list > li,
    .memory-list > li,
    .proposal-list > li,
    .message-preview-list > li,
    .package-review,
    .revision-diff {
        padding: 12px;
        border: 1px solid var(--line);
        border-radius: 11px;
        background: var(--surface-raised);
    }

    .block-list > li.dragging {
        opacity: 0.55;
        outline: 2px solid var(--accent);
    }

    .block-summary {
        display: grid;
        grid-template-columns: auto minmax(0, 1fr) auto auto;
        gap: 10px;
        align-items: center;
    }

    .block-summary > div:nth-child(2),
    .package-review header > div,
    .memory-list header > div,
    .compact-list li,
    .evidence-list li,
    .proposal-list li {
        display: grid;
        gap: 3px;
    }

    .block-summary span,
    .compact-list span,
    .memory-list span,
    .evidence-list span,
    .proposal-list span,
    .message-preview-list small {
        color: var(--ink-muted);
        font-size: 0.78rem;
    }

    .drag-handle {
        cursor: grab;
        font-size: 1.1rem;
    }

    .reorder-actions,
    .row-actions {
        display: flex;
        flex-wrap: wrap;
        gap: 6px;
    }

    .reorder-actions button {
        min-width: 34px;
        padding: 5px 8px;
    }

    .status-badge,
    .license-badge,
    .count-badge {
        display: inline-flex;
        width: fit-content;
        padding: 4px 7px;
        border-radius: 999px;
        color: var(--accent-ink);
        background: var(--accent-soft);
        font-size: 0.72rem;
    }

    .status-badge.disabled {
        color: var(--ink-muted);
        background: var(--surface-muted);
    }

    details {
        margin-top: 9px;
    }

    details summary {
        cursor: pointer;
        color: var(--ink-muted);
    }

    .structured-editor {
        display: grid;
        gap: 12px;
        margin-top: 12px;
        padding: 12px;
        border: 1px solid var(--line);
        border-radius: 10px;
    }

    .editor-grid,
    .cache-editor {
        display: grid;
        grid-template-columns: repeat(auto-fit, minmax(180px, 1fr));
        gap: 10px;
    }

    .editor-grid label,
    .json-editor,
    .cache-editor label,
    .inline-create label {
        display: grid;
        gap: 5px;
    }

    .checkbox-row {
        display: flex !important;
        align-items: center;
    }

    .json-editor textarea {
        font-family: ui-monospace, SFMono-Regular, Menlo, monospace;
    }

    .inline-create {
        display: flex;
        gap: 8px;
        align-items: end;
    }

    .inline-create label {
        flex: 1;
    }

    .prompt-source-grid > label {
        min-width: 0;
    }

    .template-slot-list {
        display: grid;
        gap: 10px;
        margin: 0;
        padding: 0;
        list-style: none;
    }

    .template-slot-row {
        display: grid;
        grid-template-columns: minmax(160px, 0.35fr) minmax(220px, 1fr) auto;
        gap: 10px;
        align-items: end;
        padding: 10px;
        border: 1px solid var(--line);
        border-radius: 9px;
        background: var(--surface-muted);
    }

    .template-slot-row label {
        display: grid;
        gap: 5px;
        min-width: 0;
    }

    .task-profile-list {
        display: grid;
        gap: 8px;
        margin: 0;
        padding: 0;
        list-style: none;
    }

    .task-profile-list > li {
        padding: 9px;
        border: 1px solid var(--line);
        border-radius: 9px;
        background: var(--surface-muted);
    }

    .expert-preview-controls {
        display: grid;
        grid-template-columns: minmax(0, 1fr) minmax(180px, 0.35fr);
        gap: 10px;
    }

    .expert-preview-controls label {
        display: grid;
        gap: 5px;
    }

    .expert-preview-section {
        padding: 10px;
        border: 1px solid var(--line);
        border-radius: 10px;
        background: var(--surface-raised);
    }

    .detail-grid,
    .plan-summary,
    .state-list {
        display: grid;
        grid-template-columns: repeat(auto-fit, minmax(150px, 1fr));
        gap: 8px;
        margin: 10px 0 0;
    }

    .detail-grid > div,
    .plan-summary > div,
    .state-list > div {
        min-width: 0;
        padding: 9px;
        border-radius: 9px;
        background: var(--surface-muted);
    }

    dt {
        color: var(--ink-muted);
        font-size: 0.74rem;
    }

    dd {
        margin: 3px 0 0;
        overflow-wrap: anywhere;
    }

    .safe-text-preview pre,
    .diff-preview pre {
        max-height: 220px;
        padding: 10px;
        overflow: auto;
        border-radius: 9px;
        white-space: pre-wrap;
        overflow-wrap: anywhere;
        color: var(--ink);
        background: var(--surface-muted);
    }

    .profile-columns,
    .split-card {
        display: grid;
        grid-template-columns: repeat(2, minmax(0, 1fr));
        gap: 14px;
    }

    .compact-list li,
    .evidence-list li,
    .proposal-list li {
        padding: 9px;
        border-radius: 9px;
        background: var(--surface-muted);
    }

    .evidence-list li.selected {
        box-shadow: inset 3px 0 var(--accent);
    }

    .memory-list > li,
    .package-review {
        display: grid;
        gap: 10px;
    }

    .data-table-wrap {
        max-width: 100%;
        overflow-x: auto;
    }

    table {
        width: 100%;
        border-collapse: collapse;
    }

    th,
    td {
        padding: 9px;
        border-bottom: 1px solid var(--line);
        text-align: left;
        vertical-align: top;
        overflow-wrap: anywhere;
    }

    .package-review fieldset {
        display: grid;
        gap: 6px;
        margin: 0;
        padding: 10px;
        border: 1px solid var(--line);
        border-radius: 9px;
    }

    .component-choice {
        display: flex;
        gap: 8px;
        align-items: flex-start;
    }

    .component-choice span {
        display: grid;
    }

    code {
        word-break: break-all;
    }

    @media (max-width: 820px) {
        .block-discovery-controls {
            grid-template-columns: 1fr 1fr;
        }

        .profile-columns,
        .split-card,
        .template-slot-row {
            grid-template-columns: 1fr;
        }

        .studio-header,
        .section-heading {
            align-items: flex-start;
        }

        .block-summary {
            grid-template-columns: auto minmax(0, 1fr) auto;
        }

        .reorder-actions {
            grid-column: 2 / -1;
        }
    }

    @media (max-width: 640px) {
        .block-discovery-controls {
            grid-template-columns: 1fr;
        }

        .orchestration-studio {
            padding: 12px;
        }

        .studio-card {
            padding: 12px;
        }
    }
</style>
