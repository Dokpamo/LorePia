<script lang="ts">
    import { tick } from 'svelte';
    import { SvelteMap } from 'svelte/reactivity';

    import ChoiceField from '../../../components/ChoiceField.svelte';
    import DetailActionBar from '../../../components/detail/DetailActionBar.svelte';
    import type { PromptBlockDto } from '../../../lib/ipc/contracts';
    import type { OrchestrationController, OrchestrationState } from '../orchestration-controller';
    import PromptBlockItem from './PromptBlockItem.svelte';

    interface Props {
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

    const normalizedBlockSearch = $derived(blockSearch.trim().toLocaleLowerCase());
    const selectedPromptBlockId = $derived(promptBlockIdFromDetailPage(detailPage));
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
        const groups = new SvelteMap<string, PromptBlockDto[]>();
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
</script>

<!-- prettier-ignore-start -->

{#if detailPage === 'blocks' || selectedPromptBlockId !== null}
    <section
        class="studio-card block-editor" data-studio-owned-fields=""
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
                        ...blockZoneOverview.map(([zone]) => ({ value: zone, label: zone })),
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
                    onSelect={(value) => (blockStatusFilter = value as typeof blockStatusFilter)}
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
                    <section class="block-group" aria-labelledby={promptZoneDomId(zone)}>
                        {#if selectedPromptBlockId === null}
                            <header>
                                <h4 id={promptZoneDomId(zone)} tabindex="-1">{zone}</h4>
                                <span>{blocks.length}개</span>
                            </header>
                        {/if}
                        <ol class="block-list">
                            {#each blocks as block (block.id)}
                                <PromptBlockItem
                                    {block}
                                    {blocks}
                                    {selectedPromptBlockId}
                                    {orchestrationState}
                                    {controller}
                                    onOpenDetail={(page: string) => (detailPage = page)}
                                    bind:draggedBlockId
                                    bind:blockJsonDrafts
                                    bind:blockJsonErrors
                                />
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

<!-- prettier-ignore-end -->
