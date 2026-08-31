<script lang="ts">
    import { ArrowDown, ArrowUp, GripVertical } from '@lucide/svelte';

    import ChoiceField from '../../../components/ChoiceField.svelte';
    import type {
        CreatorPromptBlockDocumentDto,
        CreatorPromptBlockPlacementZone,
        OrchestrationConditionExprDto,
        PromptBlockDto,
        PromptHistorySelectorDto,
        SafePromptTemplateDto,
    } from '../../../lib/ipc/contracts';
    import type { OrchestrationController, OrchestrationState } from '../orchestration-controller';

    type BlockJsonField = 'template' | 'condition' | 'history_selector';

    interface Props {
        block: PromptBlockDto;
        blocks: PromptBlockDto[];
        selectedPromptBlockId: string | null;
        orchestrationState: OrchestrationState;
        controller: OrchestrationController;
        onOpenDetail?: (detailPage: string) => void;
        draggedBlockId?: string | null;
        blockJsonDrafts?: Record<string, string>;
        blockJsonErrors?: Record<string, string>;
    }

    const BLOCK_JSON_FIELDS: readonly (readonly [BlockJsonField, string])[] = [
        ['template', '안전 템플릿 AST'],
        ['condition', '조건 AST'],
        ['history_selector', '대화 구간 선택기'],
    ];

    let {
        block,
        blocks,
        selectedPromptBlockId,
        orchestrationState,
        controller,
        onOpenDetail = () => undefined,
        draggedBlockId = $bindable(null),
        blockJsonDrafts = $bindable({}),
        blockJsonErrors = $bindable({}),
    }: Props = $props();

    const zoneIndex = $derived(blocks.findIndex((candidate) => candidate.id === block.id));
    const editableBlock = $derived(
        orchestrationState.editable_prompt_preset?.value.blocks.find(
            (candidate) => candidate.id === block.id,
        ),
    );
    const editableCache = $derived(
        orchestrationState.editable_prompt_preset?.value.cache_boundaries.find(
            (candidate) => candidate.after_block_id === block.id,
        ),
    );

    function promptBlockDetailPage(blockId: string): string {
        return `blocks/${encodeURIComponent(blockId)}`;
    }

    function canDropOn(target: PromptBlockDto): boolean {
        if (orchestrationState.editable_prompt_preset_dirty) return false;
        const dragged = orchestrationState.workspace.prompt_blocks.find(
            (candidate) => candidate.id === draggedBlockId,
        );
        return (
            dragged?.order_editable === true &&
            target.order_editable &&
            dragged.placement_zone === target.placement_zone
        );
    }

    function handleDrop(targetId: string): void {
        if (draggedBlockId !== null && draggedBlockId !== targetId) {
            void controller.movePromptBlockTo(draggedBlockId, targetId);
        }
        draggedBlockId = null;
    }

    function optionalNumber(value: string): number | null {
        return value.trim() === '' ? null : Number(value);
    }

    function blockJsonKey(blockId: string, field: BlockJsonField): string {
        return `${blockId}:${field}`;
    }

    function blockJsonDraft(
        blockValue: CreatorPromptBlockDocumentDto,
        field: BlockJsonField,
    ): string {
        const key = blockJsonKey(blockValue.id, field);
        return blockJsonDrafts[key] ?? JSON.stringify(blockValue[field], null, 2);
    }

    function setBlockJsonDraft(
        blockValue: CreatorPromptBlockDocumentDto,
        field: BlockJsonField,
        value: string,
    ): void {
        blockJsonDrafts[blockJsonKey(blockValue.id, field)] = value;
    }

    function commitBlockJson(
        blockValue: CreatorPromptBlockDocumentDto,
        field: BlockJsonField,
    ): void {
        const key = blockJsonKey(blockValue.id, field);
        const source = blockJsonDrafts[key] ?? JSON.stringify(blockValue[field]);
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
                controller.stageEditablePromptBlock(blockValue.id, {
                    template: parsed as SafePromptTemplateDto | null,
                });
            } else if (field === 'condition') {
                controller.stageEditablePromptBlock(blockValue.id, {
                    condition: parsed as OrchestrationConditionExprDto | null,
                });
            } else {
                controller.stageEditablePromptBlock(blockValue.id, {
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
</script>

<!-- prettier-ignore-start -->

<li data-studio-owned-fields=""
    class:block-editor-item={selectedPromptBlockId !== null}
    draggable={selectedPromptBlockId === null &&
        block.order_editable &&
        !orchestrationState.editable_prompt_preset_dirty}
    class:dragging={draggedBlockId === block.id}
    ondragstart={() => (draggedBlockId = block.id)}
    ondragend={() => (draggedBlockId = null)}
    ondragover={(event) => {
        if (canDropOn(block)) event.preventDefault();
    }}
    ondrop={() => handleDrop(block.id)}
>
    {#if selectedPromptBlockId === null}
        <div class="block-summary">
            <span class="drag-handle" aria-hidden="true">
                <GripVertical class="drag-handle-icon" />
            </span>
            <button
                class="block-open-button"
                type="button"
                onclick={() => onOpenDetail(promptBlockDetailPage(block.id))}
            >
                <strong>{block.name}</strong>
                <span>{block.kind} · {block.role_hint}</span>
                {#if !block.order_editable}
                    <small
                        >Core 정책 블록 · 읽기 전용</small
                    >
                {/if}
            </button>
            <span class:disabled={!block.enabled} class="status-badge">
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
                    onclick={() => void controller.movePromptBlock(block.id, -1)}
                >
                    <ArrowUp class="reorder-icon" aria-hidden="true" />
                </button>
                <button
                    type="button"
                    disabled={!block.order_editable ||
                        orchestrationState.editable_prompt_preset_dirty ||
                        zoneIndex < 0 ||
                        zoneIndex >= blocks.length - 1 ||
                        !blocks[zoneIndex + 1]?.order_editable}
                    aria-label={`${block.name} 블록 아래로 이동`}
                    onclick={() => void controller.movePromptBlock(block.id, 1)}
                >
                    <ArrowDown class="reorder-icon" aria-hidden="true" />
                </button>
            </div>
        </div>
    {/if}
    {#if selectedPromptBlockId !== null}
        <div class="block-detail-page">
            <dl class="detail-grid" data-studio-owned-definition="">
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
                    <dd>{block.provenance_label}</dd>
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
                    <pre>{block.template_preview.slice(0, 4000)}</pre>
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
                                    controller.stageEditablePromptBlock(editableBlock.id, {
                                        name: event.currentTarget.value,
                                    })}
                            />
                        </label>
                        <label class="checkbox-row">
                            <input
                                type="checkbox"
                                checked={editableBlock.enabled}
                                onchange={(event) =>
                                    controller.stageEditablePromptBlock(editableBlock.id, {
                                        enabled: event.currentTarget.checked,
                                    })}
                            />
                            <span>블록 사용</span>
                        </label>
                        <ChoiceField
                            id={`prompt-block-role-${editableBlock.id}`}
                            label="역할"
                            value={editableBlock.role_hint}
                            options={[
                                { value: 'system', label: 'system' },
                                { value: 'developer', label: 'developer' },
                                { value: 'user', label: 'user' },
                                { value: 'assistant', label: 'assistant' },
                                { value: 'provider_default', label: 'provider default' },
                            ]}
                            onSelect={(value) =>
                                controller.stageEditablePromptBlock(editableBlock.id, {
                                    role_hint: value as CreatorPromptBlockDocumentDto['role_hint'],
                                })}
                        />
                        <ChoiceField
                            id={`prompt-block-placement-${editableBlock.id}`}
                            label="삽입 구역"
                            value={editableBlock.placement_zone}
                            options={[
                                { value: 'preset_instruction', label: 'preset instruction' },
                                { value: 'character_context', label: 'character context' },
                                { value: 'retrieved_context', label: 'retrieved context' },
                                { value: 'older_history', label: 'older history' },
                                { value: 'recent_enhancement', label: 'recent enhancement' },
                                { value: 'recent_history', label: 'recent history' },
                                { value: 'post_history', label: 'post history' },
                                { value: 'latest_user', label: 'latest user' },
                                { value: 'assistant_prefill', label: 'assistant prefill' },
                            ]}
                            onSelect={(value) =>
                                controller.stageEditablePromptBlock(editableBlock.id, {
                                    placement_zone: value as CreatorPromptBlockPlacementZone,
                                })}
                        />
                        <label>
                            <span>우선순위</span>
                            <input
                                type="number"
                                min="0"
                                max="65535"
                                value={editableBlock.token_policy.priority}
                                oninput={(event) =>
                                    controller.stageEditablePromptBlock(editableBlock.id, {
                                        token_policy: {
                                            ...editableBlock.token_policy,
                                            priority: Number(event.currentTarget.value),
                                        },
                                    })}
                            />
                        </label>
                        <label>
                            <span>최소 토큰</span>
                            <input
                                type="number"
                                min="0"
                                value={editableBlock.token_policy.min_tokens ?? ''}
                                oninput={(event) =>
                                    controller.stageEditablePromptBlock(editableBlock.id, {
                                        token_policy: {
                                            ...editableBlock.token_policy,
                                            min_tokens: optionalNumber(event.currentTarget.value),
                                        },
                                    })}
                            />
                        </label>
                        <label>
                            <span>최대 토큰</span>
                            <input
                                type="number"
                                min="0"
                                value={editableBlock.token_policy.max_tokens ?? ''}
                                oninput={(event) =>
                                    controller.stageEditablePromptBlock(editableBlock.id, {
                                        token_policy: {
                                            ...editableBlock.token_policy,
                                            max_tokens: optionalNumber(event.currentTarget.value),
                                        },
                                    })}
                            />
                        </label>
                        <label>
                            <span>예약 토큰</span>
                            <input
                                type="number"
                                min="0"
                                value={editableBlock.token_policy.reserve_tokens ?? ''}
                                oninput={(event) =>
                                    controller.stageEditablePromptBlock(editableBlock.id, {
                                        token_policy: {
                                            ...editableBlock.token_policy,
                                            reserve_tokens: optionalNumber(
                                                event.currentTarget.value,
                                            ),
                                        },
                                    })}
                            />
                        </label>
                        <ChoiceField
                            id={`prompt-block-overflow-${editableBlock.id}`}
                            label="오버플로 정책"
                            value={editableBlock.overflow_policy}
                            options={[
                                { value: 'reject', label: 'reject' },
                                { value: 'drop_block', label: 'drop block' },
                                { value: 'trim_head', label: 'trim head' },
                                { value: 'trim_tail', label: 'trim tail' },
                                { value: 'keep_latest_items', label: 'keep latest items' },
                                { value: 'summarize', label: 'summarize' },
                                {
                                    value: 'reduce_knowledge_entries',
                                    label: 'reduce knowledge entries',
                                },
                            ]}
                            onSelect={(value) =>
                                controller.stageEditablePromptBlock(editableBlock.id, {
                                    overflow_policy:
                                        value as CreatorPromptBlockDocumentDto['overflow_policy'],
                                })}
                        />
                        <ChoiceField
                            id={`prompt-block-merge-${editableBlock.id}`}
                            label="내부 메시지 병합"
                            value={editableBlock.merge_policy}
                            options={[
                                { value: 'separate_message', label: 'separate message' },
                                {
                                    value: 'merge_with_previous_same_role',
                                    label: 'merge with previous same role',
                                },
                            ]}
                            onSelect={(value) =>
                                controller.stageEditablePromptBlock(editableBlock.id, {
                                    merge_policy:
                                        value as CreatorPromptBlockDocumentDto['merge_policy'],
                                })}
                        />
                    </div>
                    {#each BLOCK_JSON_FIELDS as [field, label] (`${editableBlock.id}:${field}`)}
                        <label class="json-editor" data-studio-owned-json="">
                            <span>{label}</span>
                            <textarea
                                rows="6"
                                maxlength="32768"
                                value={blockJsonDraft(editableBlock, field)}
                                oninput={(event) =>
                                    setBlockJsonDraft(
                                        editableBlock,
                                        field,
                                        event.currentTarget.value,
                                    )}></textarea>
                        </label>
                        <button type="button" onclick={() => commitBlockJson(editableBlock, field)}>
                            {label} 적용
                        </button>
                        {#if blockJsonErrors[blockJsonKey(editableBlock.id, field)]}
                            <p class="inline-diagnostic" role="alert">
                                {blockJsonErrors[blockJsonKey(editableBlock.id, field)]}
                            </p>
                        {/if}
                    {/each}
                    <div class="cache-editor">
                        <label class="checkbox-row">
                            <input
                                type="checkbox"
                                checked={editableCache !== undefined}
                                onchange={(event) =>
                                    controller.setEditablePromptCacheBoundary(
                                        editableBlock.id,
                                        event.currentTarget.checked,
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
                                    { value: 'automatic', label: 'automatic' },
                                    { value: 'explicit', label: 'explicit' },
                                    { value: 'disabled', label: 'disabled' },
                                ]}
                                onSelect={(value) =>
                                    controller.stageEditablePromptCacheBoundary(editableBlock.id, {
                                        mode: value as typeof editableCache.mode,
                                    })}
                            />
                            <ChoiceField
                                id={`prompt-cache-ttl-${editableBlock.id}`}
                                label="캐시 TTL"
                                value={editableCache.ttl}
                                options={[
                                    { value: 'provider_default', label: 'provider default' },
                                    { value: 'short', label: 'short' },
                                    { value: 'long', label: 'long' },
                                ]}
                                onSelect={(value) =>
                                    controller.stageEditablePromptCacheBoundary(editableBlock.id, {
                                        ttl: value as typeof editableCache.ttl,
                                    })}
                            />
                            <ChoiceField
                                id={`prompt-cache-role-filter-${editableBlock.id}`}
                                label="역할 필터"
                                value={editableCache.role_filter.kind}
                                options={[
                                    { value: 'all', label: 'all' },
                                    { value: 'system_like', label: 'system like' },
                                    { value: 'exact_role', label: 'exact role' },
                                ]}
                                onSelect={(value) =>
                                    controller.stageEditablePromptCacheBoundary(editableBlock.id, {
                                        role_filter:
                                            value === 'exact_role'
                                                ? { kind: 'exact_role', role: 'system' }
                                                : { kind: value as 'all' | 'system_like' },
                                    })}
                            />
                            {#if editableCache.role_filter.kind === 'exact_role'}
                                <ChoiceField
                                    id={`prompt-cache-exact-role-${editableBlock.id}`}
                                    label="정확한 역할"
                                    value={editableCache.role_filter.role}
                                    options={[
                                        { value: 'system', label: 'system' },
                                        { value: 'developer', label: 'developer' },
                                        { value: 'user', label: 'user' },
                                        { value: 'assistant', label: 'assistant' },
                                        { value: 'provider_default', label: 'provider default' },
                                    ]}
                                    onSelect={(value) =>
                                        controller.stageEditablePromptCacheBoundary(
                                            editableBlock.id,
                                            {
                                                role_filter: {
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

<!-- prettier-ignore-end -->
