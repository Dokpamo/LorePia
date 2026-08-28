<script lang="ts">
    import {
        Activity,
        ArrowDown,
        ArrowUp,
        BriefcaseBusiness,
        ChevronRight,
        GripVertical,
        Lightbulb,
        TextAlignStart,
    } from '@lucide/svelte';
    import { tr as translate } from '../../lib/i18n';
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
    import { STUDIO_SECTIONS, type StudioSection } from './studio-contracts';
    import {
        MAX_ROOM_PROMPT_NAME_CHARS,
        MAX_ROOM_PROMPT_TEMPLATE_SLOTS,
        MAX_ROOM_PROMPT_TEXT_CHARS,
        MAX_VISIBLE_PLAN_OPERATION_NONCE_CHARS,
        roomPromptSourceValidationError,
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
    import TaskProfilesPanel from './TaskProfilesPanel.svelte';
    import DetailActionBar from '../../components/detail/DetailActionBar.svelte';
    import ChoiceField from '../../components/ChoiceField.svelte';

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
        section?: StudioSection | null;
        detailPage?: string | null;
        onOpenSection?: (section: StudioSection) => void;
        desktop?: boolean;
        showIndexHeader?: boolean;
        titlebarOverlay?: boolean;
    }

    type StudioDetailPage =
        | 'history'
        | 'blocks'
        | 'room'
        | 'variables'
        | 'profiles'
        | 'documents'
        | 'records'
        | 'knowledge'
        | 'transforms'
        | 'interactions'
        | 'packages'
        | 'modules'
        | 'display'
        | 'selection'
        | 'plan';

    interface StudioDetailDestination {
        id: StudioDetailPage;
        title: string;
        description: string;
    }

    const STUDIO_DETAIL_DESTINATIONS: Record<StudioSection, readonly StudioDetailDestination[]> = {
        prompt: [
            {
                id: 'history',
                title: '프롬프트 기록',
                description: '저장된 프롬프트 리비전을 확인하고 적용합니다.',
            },
            {
                id: 'blocks',
                title: '프롬프트 블록',
                description: '블록의 내용과 순서, 토큰 정책을 편집합니다.',
            },
            {
                id: 'room',
                title: '방별 프롬프트 소스',
                description: '현재 방에 적용할 이름과 메모, 문맥을 설정합니다.',
            },
            {
                id: 'variables',
                title: '변수와 제작자 컨트롤',
                description: '프리셋이 공개한 변수와 현재 값을 확인합니다.',
            },
            {
                id: 'profiles',
                title: '생성·작업 프로필',
                description: '주 응답과 보조 작업의 실행 프로필을 편집합니다.',
            },
            {
                id: 'documents',
                title: '제작자 문서',
                description: '프롬프트와 관련된 제작자 문서를 관리합니다.',
            },
        ],
        memory: [
            {
                id: 'records',
                title: '장기기억',
                description: '현재 분기에 적용되는 기억을 확인하고 편집합니다.',
            },
            {
                id: 'knowledge',
                title: '세계관 지식 시뮬레이터',
                description: '입력에 어떤 지식이 선택되는지 미리 확인합니다.',
            },
            {
                id: 'transforms',
                title: '안전한 변환 미리보기',
                description: '변환 규칙의 적용 전후를 비교합니다.',
            },
            {
                id: 'interactions',
                title: '선언형 상호작용',
                description: '현재 상태와 사용자 승인 제안을 관리합니다.',
            },
        ],
        content: [
            {
                id: 'packages',
                title: 'LorePia 패키지',
                description: '패키지를 검토하고 선택적으로 가져옵니다.',
            },
            {
                id: 'modules',
                title: '콘텐츠 모듈',
                description: '설치된 콘텐츠 모듈의 생명주기를 관리합니다.',
            },
        ],
        diagnostics: [
            {
                id: 'display',
                title: '메시지 표시 변환',
                description: '메시지 표시 변환의 검증 결과를 확인합니다.',
            },
            {
                id: 'selection',
                title: '지식·기억 선택 근거',
                description: '현재 방에서 선택되거나 제외된 이유를 확인합니다.',
            },
            {
                id: 'plan',
                title: '최종 요청 계획',
                description: '실제 생성 전에 최종 요청 구성을 검토합니다.',
            },
        ],
    };

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
        section = null,
        detailPage = $bindable(null),
        onOpenSection = () => undefined,
        desktop = false,
        showIndexHeader = true,
        titlebarOverlay = false,
    }: Props = $props();
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
    const packageBusy = $derived(
        contentPackageState?.phase === 'listing' ||
            contentPackageState?.phase === 'picking' ||
            contentPackageState?.phase === 'resuming' ||
            contentPackageState?.phase === 'selecting' ||
            contentPackageState?.phase === 'approving' ||
            contentPackageState?.phase === 'committing',
    );
    const packageCanReviewSelection = $derived.by(() => {
        const state = contentPackageState;
        const review = state?.inspection;
        return (
            state?.phase === 'ready' &&
            review !== null &&
            review !== undefined &&
            state.selected_component_ids.length > 0 &&
            review.local_import_allowed &&
            !review.issues.some((issue) => issue.severity === 'blocker')
        );
    });
    const packageCanApprove = $derived.by(() => {
        const state = contentPackageState;
        return (
            state?.phase === 'selection_ready' &&
            packageUpdateTargetsConfirmed &&
            !state.required_capabilities
                .filter(packageCapabilityNeedsApproval)
                .some((capability) => !state.approved_capabilities.includes(capability))
        );
    });

    const normalizedBlockSearch = $derived(blockSearch.trim().toLocaleLowerCase());
    const selectedPromptBlockId = $derived(promptBlockIdFromDetailPage(detailPage));
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
            if (selectedPromptBlockId !== null) return block.id === selectedPromptBlockId;
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

    function promptBlockDetailPage(blockId: string): string {
        return `blocks/${encodeURIComponent(blockId)}`;
    }

    function promptBlockIdFromDetailPage(page: string | null): string | null {
        if (!page?.startsWith('blocks/')) return null;
        try {
            return decodeURIComponent(page.slice('blocks/'.length));
        } catch {
            return null;
        }
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
        if (
            await controller.updateMemoryRecord(record.id, {
                summary: memoryDraft(record),
            })
        ) {
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
        if (await controller.decideProposal(proposalId, approved)) {
            detailPage = 'interactions';
        }
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

    function openDesktopDestination(
        studioSection: StudioSection,
        destination: StudioDetailPage,
    ): void {
        onOpenSection(studioSection);
        detailPage = destination;
    }
</script>

{#snippet tileMark(id: StudioSection)}
    {#if id === 'prompt'}
        <TextAlignStart class="studio-destination-icon" aria-hidden="true" />
    {:else if id === 'memory'}
        <Lightbulb class="studio-destination-icon" aria-hidden="true" />
    {:else if id === 'content'}
        <BriefcaseBusiness class="studio-destination-icon" aria-hidden="true" />
    {:else}
        <Activity class="studio-destination-icon" aria-hidden="true" />
    {/if}
{/snippet}

<section
    class="orchestration-studio"
    class:index={section === null}
    aria-labelledby={section === null && showIndexHeader ? 'orchestration-studio-title' : undefined}
    aria-label={section === null && !showIndexHeader
        ? $translate('studio.title')
        : section === null
          ? undefined
          : $translate(`studio.section.${section}.title`)}
>
    {#if section === null}
        {#if showIndexHeader}
            <header
                class="index-header studio-index-header"
                data-tauri-drag-region={titlebarOverlay ? '' : undefined}
            >
                <h2
                    id="orchestration-studio-title"
                    data-tauri-drag-region={titlebarOverlay ? '' : undefined}
                >
                    {$translate('studio.title')}
                </h2>
            </header>
        {/if}

        {#if desktop}
            <div class="studio-desktop-dashboard" aria-label={$translate('studio.tools.label')}>
                {#each STUDIO_SECTIONS as id (id)}
                    <section class="studio-desktop-group">
                        <header class="studio-desktop-group-header">
                            <span class="studio-desktop-group-icon" aria-hidden="true">
                                {@render tileMark(id)}
                            </span>
                            <span>
                                <strong>
                                    {$translate(
                                        id === 'prompt'
                                            ? 'studio.feature.prompt.title'
                                            : `studio.section.${id}.title`,
                                    )}
                                </strong>
                                <small>{$translate(`studio.section.${id}.hint`)}</small>
                            </span>
                        </header>
                        <div class="studio-desktop-tools">
                            {#each STUDIO_DETAIL_DESTINATIONS[id] as destination (destination.id)}
                                <button
                                    type="button"
                                    onclick={() => openDesktopDestination(id, destination.id)}
                                >
                                    <span>
                                        <strong>{destination.title}</strong>
                                        <small>{destination.description}</small>
                                    </span>
                                    <ChevronRight aria-hidden="true" />
                                </button>
                            {/each}
                        </div>
                    </section>
                {/each}
            </div>
        {:else}
            <div class="studio-home">
                <ul
                    class="setting-list studio-destination-list"
                    aria-label={$translate('studio.tools.label')}
                >
                    {#each STUDIO_SECTIONS as id (id)}
                        <li>
                            <button
                                class="setting-row studio-destination-row"
                                type="button"
                                onclick={() => onOpenSection(id)}
                            >
                                <span class="setting-icon" aria-hidden="true">
                                    {@render tileMark(id)}
                                </span>
                                <span class="setting-content">
                                    <span class="setting-copy">
                                        <strong>
                                            {$translate(
                                                id === 'prompt'
                                                    ? 'studio.feature.prompt.title'
                                                    : `studio.section.${id}.title`,
                                            )}
                                        </strong>
                                    </span>
                                </span>
                            </button>
                        </li>
                    {/each}
                </ul>
            </div>
        {/if}
    {/if}

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

    {#if section !== null && detailPage === null}
        <div class="studio-home detail-index">
            <ul class="setting-list studio-detail-list" aria-label="세부 도구">
                {#each STUDIO_DETAIL_DESTINATIONS[section] as destination (destination.id)}
                    <li>
                        <button
                            class="setting-row studio-detail-row"
                            type="button"
                            onclick={() => (detailPage = destination.id)}
                        >
                            <span class="setting-content">
                                <span class="setting-copy">
                                    <strong>{destination.title}</strong>
                                    <small>{destination.description}</small>
                                </span>
                            </span>
                        </button>
                    </li>
                {/each}
            </ul>
        </div>
    {:else if section === 'prompt' || section === 'memory'}
        <div class="studio-panel">
            {#if section === 'prompt'}
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
                {#if detailPage === 'blocks' || selectedPromptBlockId !== null}
                    <section
                        class="studio-card block-editor"
                        class:block-editor-page={selectedPromptBlockId !== null}
                        aria-label={selectedPromptBlockId === null
                            ? '프롬프트 블록'
                            : '프롬프트 블록 편집'}
                    >
                        {#if orchestrationState.editable_prompt_preset_loading}
                            <p role="status">안전한 편집 문서를 불러오거나 저장하는 중입니다.</p>
                        {:else if orchestrationState.editable_prompt_preset_error}
                            <p class="inline-diagnostic" role="alert">
                                {orchestrationState.editable_prompt_preset_error}
                            </p>
                        {/if}
                        {#if selectedPromptBlockId === null}
                            <p class="block-list-summary">
                                {filteredBlocks.length}/{displayBlocks.length}개 블록
                            </p>
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
                                <ChoiceField
                                    id="prompt-block-zone-filter"
                                    label="블록 구역 필터"
                                    value={blockZoneFilter}
                                    options={[
                                        { value: 'all', label: '모든 구역' },
                                        ...blockZoneOverview.map(([zone]) => ({
                                            value: zone,
                                            label: zone,
                                        })),
                                    ]}
                                    onSelect={(value: string) => (blockZoneFilter = value)}
                                />
                                <ChoiceField
                                    id="prompt-block-status-filter"
                                    label="블록 활성 상태 필터"
                                    value={blockStatusFilter}
                                    options={[
                                        { value: 'all', label: '전체 상태' },
                                        { value: 'enabled', label: '사용 중' },
                                        { value: 'disabled', label: '꺼짐' },
                                    ]}
                                    onSelect={(value) =>
                                        (blockStatusFilter = value as typeof blockStatusFilter)}
                                />
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
                        {/if}
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
                                    <section
                                        class="block-group"
                                        aria-labelledby={promptZoneDomId(zone)}
                                    >
                                        {#if selectedPromptBlockId === null}
                                            <header>
                                                <h4 id={promptZoneDomId(zone)} tabindex="-1">
                                                    {zone}
                                                </h4>
                                                <span>{blocks.length}개</span>
                                            </header>
                                        {/if}
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
                                                    class:block-editor-item={selectedPromptBlockId !==
                                                        null}
                                                    draggable={selectedPromptBlockId === null &&
                                                        block.order_editable &&
                                                        !orchestrationState.editable_prompt_preset_dirty}
                                                    class:dragging={draggedBlockId === block.id}
                                                    ondragstart={() => (draggedBlockId = block.id)}
                                                    ondragend={() => (draggedBlockId = null)}
                                                    ondragover={(event) => {
                                                        if (canDropOn(block))
                                                            event.preventDefault();
                                                    }}
                                                    ondrop={() => handleDrop(block.id)}
                                                >
                                                    {#if selectedPromptBlockId === null}
                                                        <div class="block-summary">
                                                            <span
                                                                class="drag-handle"
                                                                aria-hidden="true"
                                                            >
                                                                <GripVertical
                                                                    class="drag-handle-icon"
                                                                />
                                                            </span>
                                                            <button
                                                                class="block-open-button"
                                                                type="button"
                                                                onclick={() =>
                                                                    (detailPage =
                                                                        promptBlockDetailPage(
                                                                            block.id,
                                                                        ))}
                                                            >
                                                                <strong>{block.name}</strong>
                                                                <span
                                                                    >{block.kind} · {block.role_hint}</span
                                                                >
                                                                {#if !block.order_editable}
                                                                    <small
                                                                        >Core 정책 블록 · 읽기 전용</small
                                                                    >
                                                                {/if}
                                                            </button>
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
                                                                        !blocks[zoneIndex - 1]
                                                                            ?.order_editable}
                                                                    aria-label={`${block.name} 블록 위로 이동`}
                                                                    onclick={() =>
                                                                        void controller.movePromptBlock(
                                                                            block.id,
                                                                            -1,
                                                                        )}
                                                                >
                                                                    <ArrowUp
                                                                        class="reorder-icon"
                                                                        aria-hidden="true"
                                                                    />
                                                                </button>
                                                                <button
                                                                    type="button"
                                                                    disabled={!block.order_editable ||
                                                                        orchestrationState.editable_prompt_preset_dirty ||
                                                                        zoneIndex < 0 ||
                                                                        zoneIndex >=
                                                                            blocks.length - 1 ||
                                                                        !blocks[zoneIndex + 1]
                                                                            ?.order_editable}
                                                                    aria-label={`${block.name} 블록 아래로 이동`}
                                                                    onclick={() =>
                                                                        void controller.movePromptBlock(
                                                                            block.id,
                                                                            1,
                                                                        )}
                                                                >
                                                                    <ArrowDown
                                                                        class="reorder-icon"
                                                                        aria-hidden="true"
                                                                    />
                                                                </button>
                                                            </div>
                                                        </div>
                                                    {/if}
                                                    {#if selectedPromptBlockId !== null}
                                                        <div class="block-detail-page">
                                                            <dl class="detail-grid">
                                                                <div>
                                                                    <dt>조건</dt>
                                                                    <dd>
                                                                        {block.condition_summary ??
                                                                            '항상'}
                                                                    </dd>
                                                                </div>
                                                                <div>
                                                                    <dt>출처</dt>
                                                                    <dd>{block.source_label}</dd>
                                                                </div>
                                                                <div>
                                                                    <dt>Provenance</dt>
                                                                    <dd>
                                                                        {block.provenance_label}
                                                                    </dd>
                                                                </div>
                                                                <div>
                                                                    <dt>토큰</dt>
                                                                    <dd>
                                                                        우선순위 {block.priority},
                                                                        최소
                                                                        {block.minimum_tokens ??
                                                                            '없음'}, 최대
                                                                        {block.maximum_tokens ??
                                                                            '없음'}
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
                                                                    <legend
                                                                        >구조화된 블록 편집</legend
                                                                    >
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
                                                                        <ChoiceField
                                                                            id={`prompt-block-role-${editableBlock.id}`}
                                                                            label="역할"
                                                                            value={editableBlock.role_hint}
                                                                            options={[
                                                                                {
                                                                                    value: 'system',
                                                                                    label: 'system',
                                                                                },
                                                                                {
                                                                                    value: 'developer',
                                                                                    label: 'developer',
                                                                                },
                                                                                {
                                                                                    value: 'user',
                                                                                    label: 'user',
                                                                                },
                                                                                {
                                                                                    value: 'assistant',
                                                                                    label: 'assistant',
                                                                                },
                                                                                {
                                                                                    value: 'provider_default',
                                                                                    label: 'provider default',
                                                                                },
                                                                            ]}
                                                                            onSelect={(value) =>
                                                                                controller.stageEditablePromptBlock(
                                                                                    editableBlock.id,
                                                                                    {
                                                                                        role_hint:
                                                                                            value as CreatorPromptBlockDocumentDto['role_hint'],
                                                                                    },
                                                                                )}
                                                                        />
                                                                        <ChoiceField
                                                                            id={`prompt-block-placement-${editableBlock.id}`}
                                                                            label="삽입 구역"
                                                                            value={editableBlock.placement_zone}
                                                                            options={[
                                                                                {
                                                                                    value: 'preset_instruction',
                                                                                    label: 'preset instruction',
                                                                                },
                                                                                {
                                                                                    value: 'character_context',
                                                                                    label: 'character context',
                                                                                },
                                                                                {
                                                                                    value: 'retrieved_context',
                                                                                    label: 'retrieved context',
                                                                                },
                                                                                {
                                                                                    value: 'older_history',
                                                                                    label: 'older history',
                                                                                },
                                                                                {
                                                                                    value: 'recent_enhancement',
                                                                                    label: 'recent enhancement',
                                                                                },
                                                                                {
                                                                                    value: 'recent_history',
                                                                                    label: 'recent history',
                                                                                },
                                                                                {
                                                                                    value: 'post_history',
                                                                                    label: 'post history',
                                                                                },
                                                                                {
                                                                                    value: 'latest_user',
                                                                                    label: 'latest user',
                                                                                },
                                                                                {
                                                                                    value: 'assistant_prefill',
                                                                                    label: 'assistant prefill',
                                                                                },
                                                                            ]}
                                                                            onSelect={(value) =>
                                                                                controller.stageEditablePromptBlock(
                                                                                    editableBlock.id,
                                                                                    {
                                                                                        placement_zone:
                                                                                            value as CreatorPromptBlockPlacementZone,
                                                                                    },
                                                                                )}
                                                                        />
                                                                        <label>
                                                                            <span>우선순위</span>
                                                                            <input
                                                                                type="number"
                                                                                min="0"
                                                                                max="65535"
                                                                                value={editableBlock
                                                                                    .token_policy
                                                                                    .priority}
                                                                                oninput={(event) =>
                                                                                    controller.stageEditablePromptBlock(
                                                                                        editableBlock.id,
                                                                                        {
                                                                                            token_policy:
                                                                                                {
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
                                                                                    .token_policy
                                                                                    .min_tokens ??
                                                                                    ''}
                                                                                oninput={(event) =>
                                                                                    controller.stageEditablePromptBlock(
                                                                                        editableBlock.id,
                                                                                        {
                                                                                            token_policy:
                                                                                                {
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
                                                                                    .token_policy
                                                                                    .max_tokens ??
                                                                                    ''}
                                                                                oninput={(event) =>
                                                                                    controller.stageEditablePromptBlock(
                                                                                        editableBlock.id,
                                                                                        {
                                                                                            token_policy:
                                                                                                {
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
                                                                                    .reserve_tokens ??
                                                                                    ''}
                                                                                oninput={(event) =>
                                                                                    controller.stageEditablePromptBlock(
                                                                                        editableBlock.id,
                                                                                        {
                                                                                            token_policy:
                                                                                                {
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
                                                                        <ChoiceField
                                                                            id={`prompt-block-overflow-${editableBlock.id}`}
                                                                            label="오버플로 정책"
                                                                            value={editableBlock.overflow_policy}
                                                                            options={[
                                                                                {
                                                                                    value: 'reject',
                                                                                    label: 'reject',
                                                                                },
                                                                                {
                                                                                    value: 'drop_block',
                                                                                    label: 'drop block',
                                                                                },
                                                                                {
                                                                                    value: 'trim_head',
                                                                                    label: 'trim head',
                                                                                },
                                                                                {
                                                                                    value: 'trim_tail',
                                                                                    label: 'trim tail',
                                                                                },
                                                                                {
                                                                                    value: 'keep_latest_items',
                                                                                    label: 'keep latest items',
                                                                                },
                                                                                {
                                                                                    value: 'summarize',
                                                                                    label: 'summarize',
                                                                                },
                                                                                {
                                                                                    value: 'reduce_knowledge_entries',
                                                                                    label: 'reduce knowledge entries',
                                                                                },
                                                                            ]}
                                                                            onSelect={(value) =>
                                                                                controller.stageEditablePromptBlock(
                                                                                    editableBlock.id,
                                                                                    {
                                                                                        overflow_policy:
                                                                                            value as CreatorPromptBlockDocumentDto['overflow_policy'],
                                                                                    },
                                                                                )}
                                                                        />
                                                                        <ChoiceField
                                                                            id={`prompt-block-merge-${editableBlock.id}`}
                                                                            label="내부 메시지 병합"
                                                                            value={editableBlock.merge_policy}
                                                                            options={[
                                                                                {
                                                                                    value: 'separate_message',
                                                                                    label: 'separate message',
                                                                                },
                                                                                {
                                                                                    value: 'merge_with_previous_same_role',
                                                                                    label: 'merge with previous same role',
                                                                                },
                                                                            ]}
                                                                            onSelect={(value) =>
                                                                                controller.stageEditablePromptBlock(
                                                                                    editableBlock.id,
                                                                                    {
                                                                                        merge_policy:
                                                                                            value as CreatorPromptBlockDocumentDto['merge_policy'],
                                                                                    },
                                                                                )}
                                                                        />
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
                                                                                        event
                                                                                            .currentTarget
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
                                                                                        event
                                                                                            .currentTarget
                                                                                            .checked,
                                                                                    )}
                                                                            />
                                                                            <span
                                                                                >이 블록 뒤 캐시
                                                                                경계</span
                                                                            >
                                                                        </label>
                                                                        {#if editableCache}
                                                                            <ChoiceField
                                                                                id={`prompt-cache-mode-${editableBlock.id}`}
                                                                                label="캐시 모드"
                                                                                value={editableCache.mode}
                                                                                options={[
                                                                                    {
                                                                                        value: 'automatic',
                                                                                        label: 'automatic',
                                                                                    },
                                                                                    {
                                                                                        value: 'explicit',
                                                                                        label: 'explicit',
                                                                                    },
                                                                                    {
                                                                                        value: 'disabled',
                                                                                        label: 'disabled',
                                                                                    },
                                                                                ]}
                                                                                onSelect={(value) =>
                                                                                    controller.stageEditablePromptCacheBoundary(
                                                                                        editableBlock.id,
                                                                                        {
                                                                                            mode: value as typeof editableCache.mode,
                                                                                        },
                                                                                    )}
                                                                            />
                                                                            <ChoiceField
                                                                                id={`prompt-cache-ttl-${editableBlock.id}`}
                                                                                label="캐시 TTL"
                                                                                value={editableCache.ttl}
                                                                                options={[
                                                                                    {
                                                                                        value: 'provider_default',
                                                                                        label: 'provider default',
                                                                                    },
                                                                                    {
                                                                                        value: 'short',
                                                                                        label: 'short',
                                                                                    },
                                                                                    {
                                                                                        value: 'long',
                                                                                        label: 'long',
                                                                                    },
                                                                                ]}
                                                                                onSelect={(value) =>
                                                                                    controller.stageEditablePromptCacheBoundary(
                                                                                        editableBlock.id,
                                                                                        {
                                                                                            ttl: value as typeof editableCache.ttl,
                                                                                        },
                                                                                    )}
                                                                            />
                                                                            <ChoiceField
                                                                                id={`prompt-cache-role-filter-${editableBlock.id}`}
                                                                                label="역할 필터"
                                                                                value={editableCache
                                                                                    .role_filter
                                                                                    .kind}
                                                                                options={[
                                                                                    {
                                                                                        value: 'all',
                                                                                        label: 'all',
                                                                                    },
                                                                                    {
                                                                                        value: 'system_like',
                                                                                        label: 'system like',
                                                                                    },
                                                                                    {
                                                                                        value: 'exact_role',
                                                                                        label: 'exact role',
                                                                                    },
                                                                                ]}
                                                                                onSelect={(value) =>
                                                                                    controller.stageEditablePromptCacheBoundary(
                                                                                        editableBlock.id,
                                                                                        {
                                                                                            role_filter:
                                                                                                value ===
                                                                                                'exact_role'
                                                                                                    ? {
                                                                                                          kind: 'exact_role',
                                                                                                          role: 'system',
                                                                                                      }
                                                                                                    : {
                                                                                                          kind: value as
                                                                                                              | 'all'
                                                                                                              | 'system_like',
                                                                                                      },
                                                                                        },
                                                                                    )}
                                                                            />
                                                                            {#if editableCache.role_filter.kind === 'exact_role'}
                                                                                <ChoiceField
                                                                                    id={`prompt-cache-exact-role-${editableBlock.id}`}
                                                                                    label="정확한 역할"
                                                                                    value={editableCache
                                                                                        .role_filter
                                                                                        .role}
                                                                                    options={[
                                                                                        {
                                                                                            value: 'system',
                                                                                            label: 'system',
                                                                                        },
                                                                                        {
                                                                                            value: 'developer',
                                                                                            label: 'developer',
                                                                                        },
                                                                                        {
                                                                                            value: 'user',
                                                                                            label: 'user',
                                                                                        },
                                                                                        {
                                                                                            value: 'assistant',
                                                                                            label: 'assistant',
                                                                                        },
                                                                                        {
                                                                                            value: 'provider_default',
                                                                                            label: 'provider default',
                                                                                        },
                                                                                    ]}
                                                                                    onSelect={(
                                                                                        value,
                                                                                    ) =>
                                                                                        controller.stageEditablePromptCacheBoundary(
                                                                                            editableBlock.id,
                                                                                            {
                                                                                                role_filter:
                                                                                                    {
                                                                                                        kind: 'exact_role',
                                                                                                        role: value as CreatorPromptBlockDocumentDto['role_hint'],
                                                                                                    },
                                                                                            },
                                                                                        )}
                                                                                />
                                                                            {/if}
                                                                        {/if}
                                                                    </div>
                                                                </fieldset>
                                                            {/if}
                                                        </div>
                                                    {/if}
                                                </li>
                                            {/each}
                                        </ol>
                                    </section>
                                {/each}
                            </div>
                        {/if}
                        <DetailActionBar fixed ariaLabel="프롬프트 블록 작업">
                            <button
                                class="detail-action"
                                type="button"
                                disabled={orchestrationState.editable_prompt_preset_loading}
                                onclick={() => void controller.reloadEditablePromptPreset()}
                            >
                                다시 불러오기
                            </button>
                            <button
                                class="primary detail-action detail-action--grow"
                                type="button"
                                disabled={!orchestrationState.editable_prompt_preset_dirty ||
                                    orchestrationState.editable_prompt_preset_loading}
                                onclick={() => void controller.saveEditablePromptPreset()}
                            >
                                저장
                            </button>
                        </DetailActionBar>
                    </section>
                {/if}

                {#if detailPage === 'room'}
                    <section class="studio-card prompt-room-page" aria-label="방별 프롬프트 소스">
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
                                        value={orchestrationState.workspace.room_config
                                            .author_note ?? ''}
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
                                        value={orchestrationState.workspace.room_config
                                            .group_context ?? ''}
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
                            <fieldset class="room-template-slots">
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
                            <p class="bounded-note" role="status">
                                {orchestrationState.announcement}
                            </p>
                        {/if}
                        <DetailActionBar fixed ariaLabel="방별 프롬프트 소스 작업">
                            {#if orchestrationState.workspace.room_config.supported_fields.template_slots}
                                <button
                                    class="detail-action"
                                    type="button"
                                    disabled={orchestrationState.saving ||
                                        orchestrationState.workspace.room_config.template_slots
                                            .length >= MAX_ROOM_PROMPT_TEMPLATE_SLOTS}
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
                                                    {control.kind} · {control.minimum ??
                                                        '—'}…{control.maximum ?? '—'}
                                                </small>
                                            </div>
                                            <span class="setting-value variable-value">
                                                {JSON.stringify(
                                                    orchestrationState.workspace.room_config
                                                        .creator_values[control.id] ??
                                                        control.value,
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
                    <TaskProfilesPanel
                        {appState}
                        {orchestrationState}
                        {controller}
                        bind:detailPage
                    />
                {/if}

                {#if detailPage === 'documents' || detailPage?.startsWith('documents/')}
                    <CreatorDocumentEditors {orchestrationState} {controller} bind:detailPage />
                {/if}
            {/if}
            {#if section === 'memory'}
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
                                class="bounded-note"
                                class:error={appState.memory_supervisor.status.phase === 'failed'}
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
                                                    <small class="memory-record-summary"
                                                        >{record.summary}</small
                                                    >
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
                        class="studio-card memory-record-editor has-fixed-actions"
                        aria-labelledby="memory-editor-title"
                    >
                        <h3 id="memory-editor-title" class="sr-only">장기기억 편집</h3>

                        <dl class="memory-record-metadata">
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
                                    (memoryDrafts[selectedMemoryRecord.id] =
                                        event.currentTarget.value)}></textarea>
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
                                onclick={() =>
                                    onNavigateToMemorySource(
                                        selectedMemoryRecord.source_navigation,
                                    )}
                            >
                                출처 메시지로 이동
                            </button>
                        </div>

                        <DetailActionBar fixed ariaLabel="장기기억 편집 작업">
                            {#if pendingMemoryDeleteId === selectedMemoryRecord.id}
                                <button
                                    class="danger detail-action detail-action--destructive"
                                    type="button"
                                    onclick={() =>
                                        void confirmMemoryDelete(selectedMemoryRecord.id)}
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
                                    onclick={() =>
                                        void confirmMemoryDelete(selectedMemoryRecord.id)}
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
                    <section
                        class="studio-card split-card has-fixed-actions"
                        aria-labelledby="knowledge-title"
                    >
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
                                <textarea rows="4" maxlength="8192" bind:value={knowledgeSample}
                                ></textarea>
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
                    <section
                        class="studio-card split-card has-fixed-actions"
                        aria-labelledby="transform-title"
                    >
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
                                    <pre>{orchestrationState.transform_preview.input.slice(
                                            0,
                                            16000,
                                        )}</pre>
                                </div>
                                <div>
                                    <strong>출력</strong>
                                    <pre>{orchestrationState.transform_preview.output.slice(
                                            0,
                                            16000,
                                        )}</pre>
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
                                onclick={() =>
                                    void controller.previewTransform(
                                        transformRuleId,
                                        transformSample,
                                    )}
                            >
                                변환 diff 만들기
                            </button>
                        </DetailActionBar>
                    </section>
                {/if}

                {#if detailPage === 'interactions'}
                    <section
                        class="studio-card interactions-page"
                        aria-labelledby="interactions-title"
                    >
                        <div class="section-heading">
                            <div>
                                <h3 id="interactions-title">선언형 상호작용</h3>
                                <p>상태를 확인하고 사용자 승인 제안을 별도 화면에서 검토합니다.</p>
                            </div>
                        </div>
                        <section
                            class="interaction-state-section"
                            aria-labelledby="interaction-state-title"
                        >
                            <h4 id="interaction-state-title">현재 상태</h4>
                            {#if orchestrationState.workspace.interaction_state.length === 0}
                                <p class="empty-note">표시할 상호작용 상태가 없습니다.</p>
                            {:else}
                                <dl class="interaction-state-list">
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

                        <section
                            class="interaction-proposal-section"
                            aria-labelledby="interaction-proposals-title"
                        >
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
                                                onclick={() =>
                                                    openInteractionProposal(proposal.proposal.id)}
                                            >
                                                <span class="setting-content">
                                                    <span class="setting-copy">
                                                        <strong>
                                                            {proposal.proposal
                                                                .projection_rejection_reason ===
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
                        class:has-fixed-actions={selectedInteractionProposal.proposal.status ===
                            'pending'}
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

                        <dl class="interaction-review-metadata">
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
                                    disabled={orchestrationState.busy_interaction_proposal_id !==
                                        null}
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
                                    disabled={orchestrationState.busy_interaction_proposal_id !==
                                        null ||
                                        selectedInteractionProposal.proposal
                                            .projection_rejection_reason === 'unsafe_native_text'}
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
            {/if}
        </div>
    {:else if section !== null}
        <div class="studio-panel">
            {#if section === 'diagnostics' && detailPage === 'display'}
                <section
                    class="studio-card diagnostic-flat"
                    aria-labelledby="display-transform-diagnostics-title"
                >
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
                                            >메시지 <code>{item.messageId.slice(0, 256)}</code
                                            ></strong
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
                                                <code
                                                    >{item.canonicalContentSha256.slice(
                                                        0,
                                                        64,
                                                    )}</code
                                                >
                                            </dd>
                                        </div>
                                        <div>
                                            <dt>표시 내용 SHA-256</dt>
                                            <dd>
                                                <code>{item.displayContentSha256.slice(0, 64)}</code
                                                >
                                            </dd>
                                        </div>
                                        <div>
                                            <dt>진단 SHA-256</dt>
                                            <dd>
                                                <code>{item.diagnosticsSha256.slice(0, 64)}</code>
                                            </dd>
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
                                                            >{diagnostic.after_sha256?.slice(
                                                                0,
                                                                64,
                                                            ) ?? 'none'}</code
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
            {/if}

            {#if section === 'diagnostics' && detailPage === 'selection'}
                <section
                    class="studio-card diagnostic-flat"
                    aria-labelledby="selection-evidence-title"
                >
                    <div class="section-heading">
                        <div>
                            <h3 id="selection-evidence-title">현재 방의 지식·기억 선택 근거</h3>
                            <p>
                                현재 분기 스냅샷에서 Core가 선택하거나 제외한 지식과 기억의 이유,
                                점수, 토큰, 삽입 위치를 표시합니다.
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
                                        {evidence.source_kind} · {evidence.selected
                                            ? '선택'
                                            : '제외'} ·
                                        {evidence.estimated_tokens} tokens · score
                                        {evidence.score ?? '없음'} · 배치 {evidence.placement ??
                                            '없음'}
                                    </small>
                                </li>
                            {/each}
                        </ul>
                    {/if}
                    {#if orchestrationState.list_truncation.selection_evidence}
                        <p class="bounded-note" role="note">
                            안전한 UI 한도에 따라 처음 300개 선택 근거만 표시합니다. 전체 후보
                            목록으로 해석하지 마세요.
                        </p>
                    {/if}
                </section>
            {/if}
            {#if section === 'content'}
                {#if detailPage === 'packages' && contentPackageState && contentPackageController}
                    <section
                        class="studio-card package-detail"
                        aria-labelledby="package-import-title"
                    >
                        <div class="section-heading">
                            <div>
                                <h3 id="package-import-title">LorePia 패키지 선택 가져오기</h3>
                                <p>
                                    경로나 원본 바이트 없이 Core가 검사한 manifest, 라이선스, 충돌,
                                    격리 결과만 검토합니다.
                                </p>
                            </div>
                        </div>

                        {#if contentPackageState.phase === 'unavailable'}
                            <p class="bounded-note" role="note">{contentPackageState.error}</p>
                        {:else if contentPackageState.phase === 'error'}
                            <p class="drawer-status error" role="alert">
                                {contentPackageState.error}
                            </p>
                        {:else if contentPackageState.phase === 'listing'}
                            <p role="status">중단된 패키지 검토를 확인하고 있습니다.</p>
                        {:else if contentPackageState.phase === 'picking'}
                            <p role="status">
                                패키지를 선택하고 Core에서 안전하게 검사하는 중입니다.
                            </p>
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
                                    <h4 id="completed-package-exports-title">
                                        완료된 패키지 내보내기
                                    </h4>
                                    <p>
                                        재시작 후에도 Core가 다시 검증한 완료 패키지만 표시합니다.
                                    </p>
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
                                                {pendingImport.package_id} · {pendingImport.status} ·
                                                revision
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
                                    문서 {contentPackageState.result.committed_document_ids
                                        .length}개 · 자산 {contentPackageState.result.asset_ids
                                        .length}개
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
                                <article
                                    class="revision-diff"
                                    aria-labelledby="package-export-title"
                                >
                                    <h4 id="package-export-title">최근 패키지 내보내기</h4>
                                    <p>파일명 {contentPackageState.export_receipt.file_name}</p>
                                    <p>
                                        크기 {contentPackageState.export_receipt.size_bytes}바이트
                                    </p>
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
                                        <dd>
                                            <code>{packageReview.capability_review_sha256}</code>
                                        </dd>
                                    </div>
                                </dl>

                                <p>
                                    재배포 manifest:
                                    {packageReview.manifest.redistribution_allowed
                                        ? '허용'
                                        : '허용 안 됨'}
                                </p>
                                {#if packageReview.manifest.required_app_version}
                                    <p>
                                        요구 앱 버전: {packageReview.manifest.required_app_version}
                                    </p>
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
                                        <h4 id="package-normalization-title">
                                            승인 전 정규화 근거
                                        </h4>
                                        <p>
                                            아래 변경과 해시를 확인해야만 승인할 수 있습니다. 정규화
                                            근거 해시
                                            <code
                                                >{packageSelection.normalization_evidence_sha256}</code
                                            >
                                        </p>
                                        <p>
                                            선택 계획
                                            <code
                                                >{packageSelection.content_selection_plan_hash}</code
                                            >
                                            · 가져오기 계획
                                            <code>{packageSelection.import_plan_sha256}</code>
                                        </p>
                                        {#if packageSelection.normalization_evidence.length > 0}
                                            <ul class="compact-list">
                                                {#each packageSelection.normalization_evidence.slice(0, MAX_INLINE_ITEMS) as evidence (`${evidence.component_id}:${evidence.object_id}:${evidence.field}`)}
                                                    <li>
                                                        {evidence.component_id} / {evidence.object_id}
                                                        ·
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
                                            기존 대상을 갱신하는 문서는 대상 리비전과 상태 CAS를
                                            각각 확인해야 합니다. 새 대상 생성은 별도 확인이
                                            필요하지 않습니다.
                                        </p>
                                        {#if packageSelection.target_review.documents.length === 0}
                                            <p>가져오기 계획에 쓸 문서 대상이 없습니다.</p>
                                        {:else}
                                            <ol
                                                class="compact-list"
                                                aria-label="패키지 문서 대상 검토"
                                            >
                                                {#each packageSelection.target_review.documents.slice(0, MAX_VISIBLE_CONTENT_PACKAGE_TARGET_DOCUMENTS) as document (`${document.source_component_id}:${String(document.component_document_ordinal)}`)}
                                                    <li>
                                                        <strong>
                                                            {document.source_component_id} · 전체 문서
                                                            인덱스
                                                            {document.document_index} · 구성요소 문서
                                                            순서
                                                            {document.component_document_ordinal}
                                                        </strong>
                                                        <span>
                                                            소스 구성요소 SHA-256
                                                            <code
                                                                >{document.source_component_sha256}</code
                                                            >
                                                        </span>
                                                        <span>
                                                            종류 <code
                                                                >{document.document_kind}</code
                                                            >
                                                            · 대상
                                                            <code>{document.target_object_id}</code>
                                                            · 처리
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
                                                                새 대상 생성 — 별도 업데이트 확인
                                                                불필요
                                                            </span>
                                                        {/if}
                                                    </li>
                                                {/each}
                                            </ol>
                                        {/if}
                                        {#if packageSelection.target_review.documents.length > MAX_VISIBLE_CONTENT_PACKAGE_TARGET_DOCUMENTS}
                                            <p class="bounded-note" role="note">
                                                처음 {MAX_VISIBLE_CONTENT_PACKAGE_TARGET_DOCUMENTS}개
                                                대상 문서만 표시합니다. 숨겨진 업데이트 대상이
                                                있으면 승인할 수 없습니다.
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
                                        <p class="bounded-note">
                                            추가 승인이 필요한 기능은 없습니다.
                                        </p>
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
                                            <code
                                                >{contentPackageState.approval
                                                    .approval_sha256}</code
                                            >
                                        </p>
                                        <p>
                                            활성 구성요소:
                                            {contentPackageState.approval.enabled_component_ids
                                                .length > 0
                                                ? contentPackageState.approval.enabled_component_ids.join(
                                                      ', ',
                                                  )
                                                : '없음'}
                                        </p>
                                        <p>
                                            승인 기능:
                                            {contentPackageState.approval.approved_capabilities
                                                .length > 0
                                                ? contentPackageState.approval.approved_capabilities.join(
                                                      ', ',
                                                  )
                                                : '없음'}
                                        </p>
                                    </article>
                                {/if}
                            </article>
                        {/if}
                        <DetailActionBar fixed ariaLabel="LorePia 패키지 작업">
                            {#if contentPackageState.inspection === null}
                                <button
                                    class="detail-action primary"
                                    type="button"
                                    aria-label="새 LorePia 패키지 선택"
                                    disabled={packageBusy ||
                                        contentPackageState.phase === 'unavailable'}
                                    onclick={() => void contentPackageController.pickAndInspect()}
                                >
                                    패키지 선택
                                </button>
                            {:else}
                                <button
                                    class="detail-action detail-action--destructive danger"
                                    type="button"
                                    disabled={contentPackageState.phase === 'committing' ||
                                        contentPackageState.phase === 'picking'}
                                    onclick={() => void contentPackageController.discard()}
                                >
                                    검토 폐기
                                </button>
                                {#if contentPackageState.phase === 'selection_ready'}
                                    <button
                                        class="detail-action detail-action--grow primary"
                                        type="button"
                                        aria-label="표시된 근거와 기능 명시적 승인"
                                        disabled={!packageCanApprove}
                                        onclick={() => void contentPackageController.approve()}
                                    >
                                        명시적 승인
                                    </button>
                                {:else if contentPackageState.phase === 'approved'}
                                    <button
                                        class="detail-action detail-action--grow primary"
                                        type="button"
                                        aria-label="승인된 패키지 가져오기 확정"
                                        onclick={() => void contentPackageController.commit()}
                                    >
                                        가져오기 확정
                                    </button>
                                {:else}
                                    <button
                                        class="detail-action detail-action--grow primary"
                                        type="button"
                                        aria-label="선택 및 정규화 검토"
                                        disabled={!packageCanReviewSelection}
                                        onclick={() =>
                                            void contentPackageController.reviewSelection()}
                                    >
                                        선택 검토
                                    </button>
                                {/if}
                            {/if}
                        </DetailActionBar>
                    </section>
                {/if}

                {#if detailPage === 'modules' || detailPage?.startsWith('modules:')}
                    <ContentModuleLifecyclePanel
                        {client}
                        conversationId={orchestrationState.workspace.room_config.conversation_id ||
                            null}
                        branchId={orchestrationState.workspace.room_config.branch_id || null}
                        bind:detailPage
                    />
                {/if}
            {/if}
            {#if section === 'diagnostics' && detailPage === 'plan'}
                <section class="studio-card plan-detail" aria-labelledby="plan-preview-title">
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
                                conversationId={orchestrationState.workspace.room_config
                                    .conversation_id || null}
                                sourceBranchId={orchestrationState.workspace.room_config
                                    .branch_id || null}
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
                        <dl class="plan-summary">
                            <div>
                                <dt>계획 ID</dt>
                                <dd><code>{boundedPlanIdentifier(preview.plan_id)}</code></dd>
                            </div>
                            <div>
                                <dt>생성 시도 ID</dt>
                                <dd>
                                    <code
                                        >{boundedPlanIdentifier(
                                            preview.generation_attempt_id,
                                        )}</code
                                    >
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
                            <ChoiceField
                                id="expert-preview-filter"
                                label="표시 필터"
                                value={expertFilter}
                                options={[
                                    { value: 'all', label: '전체' },
                                    { value: 'messages', label: '최종 메시지 구조' },
                                    { value: 'provider', label: '제공자 변환 구조' },
                                    { value: 'parameters', label: '적용 파라미터' },
                                    { value: 'diff', label: '역할·배치 diff' },
                                ]}
                                onSelect={(value) => (expertFilter = value as typeof expertFilter)}
                            />
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
                            expertMatches(
                                parameter.field,
                                parameter.value_kind,
                                parameter.item_count,
                            ),
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
                                                    순서 {message.sequence} · 블록 {message.block_id}
                                                    ·
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
                                                    순서 {message.sequence} · 블록 {message.block_id}
                                                    · 배치
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
                                                <strong
                                                    >{entry.block_id} · 순서 {entry.sequence}</strong
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
                                            <th scope="row"
                                                >{block.block_id} · {block.block_kind}</th
                                            >
                                            <td>
                                                {block.source.authority} · {block.source
                                                    .source_kind}
                                                <br />
                                                {block.source.source_id ?? '로컬 출처'}
                                                {#if block.source.source_revision}
                                                    · rev {block.source.source_revision}
                                                {/if}
                                                {#if block.source.source_hash}
                                                    · sha256 {block.source.source_hash.slice(
                                                        0,
                                                        12,
                                                    )}…
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
                                            처음 100개 지식 후보 근거만 표시합니다. 전체 후보
                                            목록으로 해석하지 마세요.
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
                                        <p class="bounded-note">
                                            처음 100개 후보 근거만 표시합니다.
                                        </p>
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
                                            {cache.after_block_id} 뒤 · {cache.mode} · {cache.status}
                                            ·
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
                    <DetailActionBar fixed ariaLabel="최종 요청 계획 작업">
                        <button
                            class="detail-action"
                            type="button"
                            aria-label="새 작업 미리보기"
                            disabled={orchestrationState.workspace.room_config.conversation_id ===
                                '' ||
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
                                disabled={orchestrationState.workspace.room_config
                                    .conversation_id === '' ||
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
                                disabled={orchestrationState.workspace.room_config
                                    .conversation_id === '' ||
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
            {/if}
        </div>
    {/if}
</section>

<style>
    .orchestration-studio {
        display: grid;
        min-width: 0;
        gap: 16px;
        padding: 0;
        border: 0;
        border-radius: 0;
        background: transparent;
    }

    .orchestration-studio.index {
        padding: 0;
        border: 0;
        border-radius: 0;
        background: transparent;
        gap: 8px;
    }

    .studio-index-header {
        min-height: var(--mobile-toolbar);
        padding: 0;
    }

    .studio-index-header h2 {
        font-size: 1.5rem;
    }

    :global(.app-shell[data-layout='desktop']) .studio-index-header {
        min-height: 34px;
    }

    :global(.app-shell[data-layout='desktop']) .studio-index-header h2 {
        font-size: 28px;
        font-weight: 600;
        line-height: 1.2;
        letter-spacing: -0.025em;
    }

    :global(.app-shell[data-layout='desktop']) .orchestration-studio.index {
        gap: 24px;
    }

    .studio-desktop-dashboard {
        display: grid;
        grid-template-columns: repeat(2, minmax(0, 1fr));
        align-items: start;
        gap: 12px;
    }

    .studio-desktop-group {
        min-width: 0;
        overflow: hidden;
        border: 1px solid var(--desktop-divider);
        border-radius: 14px;
        background: var(--surface-raised);
    }

    .studio-desktop-group-header {
        display: flex;
        min-height: 64px;
        align-items: center;
        padding: 12px 14px;
        border-bottom: 1px solid var(--desktop-divider);
        gap: 11px;
    }

    .studio-desktop-group-header > span:last-child {
        display: grid;
        min-width: 0;
        gap: 3px;
    }

    .studio-desktop-group-header strong,
    .studio-desktop-tools strong {
        color: var(--ink);
        font-size: 13px;
        font-weight: 620;
        line-height: 1.25;
    }

    .studio-desktop-group-header small,
    .studio-desktop-tools small {
        overflow: hidden;
        color: var(--ink-muted);
        font-size: 11px;
        font-weight: 500;
        line-height: 1.35;
        text-overflow: ellipsis;
    }

    .studio-desktop-group-header small {
        white-space: nowrap;
    }

    .studio-desktop-group-icon {
        display: grid;
        width: 30px;
        height: 30px;
        flex: none;
        border-radius: 8px;
        background: var(--surface-sunken);
        color: var(--ink);
        place-items: center;
    }

    .studio-desktop-group-icon :global(svg) {
        width: 16px;
        height: 16px;
        stroke-width: 1.8;
    }

    .studio-desktop-tools {
        display: grid;
    }

    .studio-desktop-tools button {
        display: flex;
        min-width: 0;
        min-height: 52px;
        align-items: center;
        justify-content: space-between;
        padding: 9px 12px 9px 14px;
        border: 0;
        border-bottom: 1px solid var(--desktop-divider);
        border-radius: 0;
        background: transparent;
        gap: 10px;
        text-align: left;
    }

    .studio-desktop-tools button:last-child {
        border-bottom: 0;
    }

    .studio-desktop-tools button > span {
        display: grid;
        min-width: 0;
        gap: 2px;
    }

    .studio-desktop-tools button small {
        display: block;
        white-space: nowrap;
    }

    .studio-desktop-tools button > :global(svg) {
        width: 15px;
        height: 15px;
        flex: none;
        color: var(--ink-muted);
        stroke-width: 1.7;
    }

    .studio-desktop-tools button:hover {
        background: var(--surface-sunken);
    }

    .studio-desktop-tools button:focus-visible {
        outline-offset: -3px;
    }

    @media (max-width: 1120px) {
        .studio-desktop-dashboard {
            grid-template-columns: minmax(0, 1fr);
        }
    }

    .studio-home {
        display: grid;
    }

    .detail-index,
    .studio-detail-list {
        width: 100%;
        min-width: 0;
    }

    .studio-detail-list {
        margin: 0;
    }

    .studio-detail-row {
        min-height: clamp(62px, 17.849vw, 78px);
    }

    .studio-detail-row .setting-copy {
        display: grid;
        min-width: 0;
        gap: 5px;
        text-align: left;
    }

    .studio-detail-row :is(strong, small) {
        overflow: hidden;
        font-size: var(--detail-support-type);
        line-height: 1.35;
        text-overflow: ellipsis;
    }

    .studio-detail-row strong {
        color: var(--ink);
        font-weight: 550;
        white-space: nowrap;
    }

    .studio-detail-row small {
        display: -webkit-box;
        color: var(--ink-muted);
        font-weight: 550;
        overflow-wrap: anywhere;
        white-space: normal;
        line-clamp: 3;
        -webkit-box-orient: vertical;
        -webkit-line-clamp: 3;
    }

    .setting-icon :global(.studio-destination-icon) {
        width: clamp(20px, 5.492vw, 24px);
        height: clamp(20px, 5.492vw, 24px);
    }

    .studio-panel {
        display: grid;
        gap: 16px;
    }

    .studio-card {
        display: grid;
        min-width: 0;
        gap: 18px;
        padding: 0;
        border: 0;
        border-radius: 0;
        background: transparent;
    }

    .memory-record-list,
    .interaction-proposal-list {
        margin: 0;
    }

    .memory-record-row,
    .interaction-proposal-row {
        min-height: clamp(72px, 20.595vw, 90px);
    }

    .memory-record-row .setting-copy,
    .interaction-proposal-row .setting-copy {
        display: grid;
        min-width: 0;
        gap: 4px;
        text-align: left;
    }

    .memory-record-row .setting-copy strong,
    .interaction-proposal-row .setting-copy strong {
        overflow: hidden;
        color: var(--ink);
        font-size: var(--detail-support-type);
        font-weight: 600;
        line-height: 1.35;
        text-overflow: ellipsis;
        white-space: nowrap;
    }

    .memory-record-row .setting-copy small,
    .interaction-proposal-row .setting-copy small {
        overflow: hidden;
        color: var(--ink-muted);
        font-size: calc(var(--detail-support-type) * 0.9);
        font-weight: 550;
        line-height: 1.35;
        text-overflow: ellipsis;
        white-space: nowrap;
    }

    .memory-record-row .setting-copy .memory-record-summary {
        display: -webkit-box;
        overflow-wrap: anywhere;
        white-space: normal;
        line-clamp: 2;
        -webkit-box-orient: vertical;
        -webkit-line-clamp: 2;
    }

    .memory-record-metadata,
    .interaction-review-metadata,
    .interaction-state-list {
        display: grid;
        grid-template-columns: 1fr;
        gap: 0;
        margin: 0;
    }

    .memory-record-metadata > div,
    .interaction-review-metadata > div,
    .interaction-state-list > div {
        min-width: 0;
        padding: clamp(12px, 3.661vw, 16px) 0;
        border-bottom: 1px solid var(--line);
        border-radius: 0;
        background: transparent;
    }

    .memory-record-controls {
        display: grid;
        grid-template-columns: repeat(2, minmax(0, 1fr));
        gap: 8px;
    }

    .memory-record-controls button {
        min-height: var(--touch);
        padding-inline: 10px;
    }

    .interaction-state-section,
    .interaction-proposal-section,
    .interaction-review-copy {
        display: grid;
        gap: 10px;
    }

    .interaction-state-section > h4,
    .interaction-proposal-section > h4,
    .interaction-review-copy > strong {
        margin: 0;
        color: var(--ink);
        font-size: var(--detail-support-type);
    }

    .interaction-review-copy > p {
        margin: 0;
        color: var(--ink-muted);
        line-height: 1.55;
        overflow-wrap: anywhere;
        white-space: pre-wrap;
    }

    .diagnostic-flat :is(.message-preview-list, .evidence-list, .compact-list),
    .package-detail :is(.compact-list, .message-preview-list) {
        gap: 0;
    }

    .diagnostic-flat > :is(.empty-note, .bounded-note),
    .plan-detail > :is(.empty-note, .bounded-note, .inline-note) {
        padding: 0;
        border-radius: 0;
        background: transparent;
    }

    .diagnostic-flat :is(.message-preview-list, .evidence-list, .compact-list) > li,
    .package-detail :is(.package-review, .revision-diff),
    .package-detail :is(.compact-list, .message-preview-list) > li {
        padding: clamp(14px, 4.119vw, 18px) 0;
        border: 0;
        border-bottom: 1px solid var(--line);
        border-radius: 0;
        background: transparent;
        box-shadow: none;
    }

    .diagnostic-flat .state-list,
    .plan-detail :is(.plan-summary, .state-list) {
        grid-template-columns: 1fr;
        gap: 0;
        margin: 0;
    }

    .diagnostic-flat .state-list > div,
    .plan-detail :is(.plan-summary, .state-list) > div {
        padding: clamp(12px, 3.661vw, 16px) 0;
        border-bottom: 1px solid var(--line);
        border-radius: 0;
        background: transparent;
    }

    .package-detail > section,
    .package-detail > article {
        padding-top: 2px;
        border: 0;
        background: transparent;
    }

    .package-detail .package-review fieldset {
        padding: clamp(12px, 3.661vw, 16px) 0;
        border: 0;
        border-top: 1px solid var(--line);
        border-radius: 0;
    }

    .plan-detail .expert-preview-section {
        padding: clamp(12px, 3.661vw, 16px) 0;
        border: 0;
        border-bottom: 1px solid var(--line);
        border-radius: 0;
        background: transparent;
    }

    .plan-embedded-panel :global(.attempt-approvals),
    .plan-embedded-panel :global(.memory-query-retry) {
        padding: clamp(12px, 3.661vw, 16px) 0;
        border: 0;
        border-top: 1px solid var(--line);
        border-bottom: 1px solid var(--line);
        border-radius: 0;
        background: transparent;
        box-shadow: none;
    }

    .plan-embedded-panel :global(.attempt-approvals article),
    .plan-embedded-panel :global(.memory-query-retry li) {
        padding: clamp(12px, 3.661vw, 16px) 0;
        border: 0;
        border-bottom: 1px solid var(--line);
        border-radius: 0;
        background: transparent;
        box-shadow: none;
    }

    /* App's fixed header is the single visible page title on a pushed tool. */
    .studio-card > .section-heading:first-child h3:first-child,
    .studio-card.split-card > div:first-child > .section-heading:first-child h3:first-child {
        display: none;
    }

    .orchestration-studio label:not(.component-choice):not(.checkbox-row) > span {
        color: var(--ink-muted);
        font-size: var(--detail-support-type);
        font-weight: 700;
    }

    .orchestration-studio
        :is(input[type='text'], input[type='search'], input[type='number'], textarea) {
        width: 100%;
        min-width: 0;
        box-sizing: border-box;
        padding: clamp(12px, 3.432vw, 15px);
        border: 1.5px solid var(--line);
        border-radius: var(--radius-md);
        -webkit-appearance: none;
        appearance: none;
        background: color-mix(in srgb, var(--surface-sunken) 26%, var(--surface-raised));
        box-shadow: var(--control-inset-shadow);
        caret-color: var(--accent);
        color: var(--ink);
        font-size: var(--detail-support-type);
        line-height: 1.5;
        transition:
            background-color 140ms ease,
            box-shadow 140ms ease;
    }

    .orchestration-studio :is(input[type='text'], input[type='search'], input[type='number']) {
        min-height: clamp(48px, 13.73vw, 60px);
    }

    .orchestration-studio textarea {
        min-height: clamp(112px, 32.037vw, 140px);
        resize: vertical;
    }

    .orchestration-studio
        :is(input[type='text'], input[type='search'], input[type='number'], textarea):hover:not(
            :focus,
            :disabled
        ) {
        border-color: var(--line);
    }

    .orchestration-studio
        :is(input[type='text'], input[type='search'], input[type='number'], textarea):focus {
        border-color: var(--accent);
        outline: none;
    }

    .orchestration-studio
        :is(input[type='text'], input[type='search'], input[type='number'], textarea):disabled {
        cursor: not-allowed;
        opacity: var(--disabled-opacity);
    }

    .studio-status,
    .empty-note,
    .bounded-note {
        margin: 0;
        padding: 11px 13px;
        border-radius: 10px;
        color: var(--ink-muted);
        background: var(--surface-sunken);
    }

    .studio-status.error,
    .bounded-note.error,
    .conflict-list {
        border-color: var(--status-error-border);
        color: var(--status-error-fg);
        background: var(--status-error-bg);
    }

    .bounded-note {
        border: 1px solid var(--status-warning-border);
        color: var(--status-warning-fg);
        background: var(--status-warning-bg);
    }

    .drawer-status.error {
        padding: 10px 12px;
        border: 1px solid var(--status-error-border);
        border-radius: var(--radius-sm);
        color: var(--status-error-fg);
        background: var(--status-error-bg);
    }

    .search-field,
    .studio-card > label,
    .split-card > div > label {
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
        padding: 0;
        border: 0;
        border-radius: 0;
        background: transparent;
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
        padding: 8px 2px;
        border: 0;
        border-bottom: 2px solid transparent;
        border-radius: 0;
        background: transparent;
        box-shadow: none;
        gap: 2px;
        text-align: left;
    }

    .block-minimap button.active {
        border-bottom-color: var(--accent);
        color: var(--accent);
    }

    .block-minimap small {
        color: var(--ink-muted);
    }

    .block-groups,
    .block-list,
    .compact-list,
    .evidence-list,
    .message-preview-list {
        display: grid;
        gap: 9px;
        margin: 0;
        padding: 0;
        list-style: none;
    }

    .block-editor:not(.block-editor-page) .block-list {
        padding: 0;
        border-radius: clamp(18px, 5.492vw, 24px);
        background: var(--bg);
        box-shadow: var(--shadow-1);
        gap: clamp(2px, 0.686vw, 3px);
        overflow: hidden;
    }

    .block-editor:not(.block-editor-page) .block-list > li {
        padding: clamp(10px, 2.746vw, 12px) clamp(14px, 4.577vw, 20px);
        border: 0;
        border-radius: clamp(3px, 0.915vw, 4px);
        background: var(--surface-raised);
        box-shadow: none;
    }

    .block-editor-page .block-list > li {
        padding: 0;
        border: 0;
        border-radius: 0;
        background: transparent;
        box-shadow: none;
    }

    .block-detail-page {
        display: grid;
        gap: 16px;
    }

    .block-editor-page .structured-editor {
        padding: 0;
        border: 0;
        border-radius: 0;
    }

    .block-editor-page .detail-grid {
        grid-template-columns: 1fr;
        gap: 0;
        margin: 0;
    }

    .block-editor-page .detail-grid > div {
        padding: clamp(12px, 3.661vw, 16px) 0;
        border-bottom: 1px solid var(--line);
        border-radius: 0;
        background: transparent;
    }

    .block-editor-page .safe-text-preview pre {
        max-height: none;
        overflow: visible;
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

    .block-open-button {
        display: grid;
        min-width: 0;
        justify-items: start;
        padding: 0;
        border: 0;
        background: transparent;
        box-shadow: none;
        gap: 4px;
        text-align: left;
    }

    .block-open-button > :is(strong, span, small) {
        max-width: 100%;
        overflow: hidden;
        text-overflow: ellipsis;
        white-space: nowrap;
    }

    .block-summary > div:nth-child(2),
    .package-review header > div,
    .compact-list li,
    .evidence-list li {
        display: grid;
        gap: 3px;
    }

    .block-summary span,
    .compact-list span,
    .evidence-list span,
    .message-preview-list small {
        color: var(--ink-muted);
        font-size: 0.78rem;
    }

    .drag-handle {
        display: grid;
        cursor: grab;
        place-items: center;
    }

    .drag-handle :global(.drag-handle-icon) {
        width: 18px;
        height: 18px;
    }

    .reorder-actions {
        display: flex;
        flex-wrap: wrap;
        gap: 6px;
    }

    .reorder-actions button {
        min-width: 34px;
        padding: 5px 8px;
    }

    .reorder-actions :global(.reorder-icon) {
        width: 16px;
        height: 16px;
    }

    .status-badge,
    .license-badge {
        display: inline-flex;
        width: fit-content;
        padding: 4px 7px;
        border-radius: 999px;
        color: var(--accent);
        background: var(--accent-soft);
        font-size: 0.72rem;
    }

    .status-badge.disabled {
        color: var(--ink-muted);
        background: var(--surface-sunken);
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
        grid-template-columns: repeat(auto-fit, minmax(min(180px, 100%), 1fr));
        gap: 10px;
    }

    .editor-grid label,
    .json-editor,
    .cache-editor label {
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

    .prompt-source-grid > label {
        min-width: 0;
    }

    .prompt-source-grid {
        grid-template-columns: 1fr;
    }

    .room-template-slots {
        display: grid;
        padding: 0;
        border: 0;
        margin: 0;
        gap: 8px;
    }

    .room-template-slots > legend {
        padding: 0;
        margin-bottom: 8px;
        color: var(--ink-muted);
        font-size: var(--detail-support-type);
        font-weight: 700;
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
        grid-template-columns: 1fr;
        gap: 10px;
        align-items: stretch;
        padding: clamp(12px, 3.661vw, 16px) 0;
        border: 0;
        border-bottom: 1px solid var(--line);
        border-radius: 0;
        background: transparent;
    }

    .template-slot-row label {
        display: grid;
        gap: 5px;
        min-width: 0;
    }

    .template-slot-row > button {
        justify-self: end;
        border: 0;
        color: var(--status-error-fg);
        background: transparent;
        box-shadow: none;
    }

    .variable-list {
        margin: 0;
    }

    .variable-row {
        cursor: default;
    }

    .variable-copy {
        min-width: 0;
        flex-direction: column;
        gap: 5px;
    }

    .variable-copy small {
        color: var(--ink-muted);
        font-size: var(--detail-support-type);
        line-height: 1.35;
    }

    .variable-value {
        max-width: 44%;
        margin: 0;
        overflow-wrap: anywhere;
        white-space: normal;
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
        grid-template-columns: repeat(auto-fit, minmax(min(150px, 100%), 1fr));
        gap: 8px;
        margin: 10px 0 0;
    }

    .detail-grid > div,
    .plan-summary > div,
    .state-list > div {
        min-width: 0;
        padding: 9px;
        border-radius: 9px;
        background: var(--surface-sunken);
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
        max-height: none;
        padding: 10px;
        overflow-x: auto;
        overflow-y: visible;
        border-radius: 9px;
        white-space: pre-wrap;
        overflow-wrap: anywhere;
        color: var(--ink);
        background: var(--surface-sunken);
    }

    .profile-columns,
    .split-card {
        display: grid;
        grid-template-columns: repeat(2, minmax(0, 1fr));
        gap: 14px;
    }

    .compact-list li,
    .evidence-list li {
        padding: 9px;
        border-radius: 9px;
        background: var(--surface-sunken);
    }

    .evidence-list li.selected {
        box-shadow: inset 3px 0 var(--accent);
    }

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

    @container view (max-width: 820px) {
        .block-discovery-controls {
            grid-template-columns: 1fr 1fr;
        }

        .profile-columns,
        .split-card,
        .template-slot-row {
            grid-template-columns: 1fr;
        }

        .block-summary {
            grid-template-columns: auto minmax(0, 1fr) auto;
        }

        .reorder-actions {
            grid-column: 2 / -1;
        }
    }

    @container view (max-width: 640px) {
        .block-discovery-controls,
        .expert-preview-controls {
            grid-template-columns: 1fr;
        }

        .orchestration-studio {
            padding: 0;
            border: 0;
            border-radius: 0;
            background: transparent;
        }

        .studio-card {
            padding: 0;
        }
    }
</style>
