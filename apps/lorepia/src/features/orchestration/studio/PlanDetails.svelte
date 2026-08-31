<script lang="ts">
    import ChoiceField from '../../../components/ChoiceField.svelte';
    import type { OrchestrationState } from '../orchestration-controller';

    type PlanPreview = NonNullable<OrchestrationState['workspace']['plan_preview']>;

    interface Props {
        preview: PlanPreview;
        expertSearch?: string;
        expertFilter?: 'all' | 'messages' | 'provider' | 'parameters' | 'diff';
    }

    const MAX_INLINE_ITEMS = 100;
    const MAX_PLAN_MESSAGES = 200;
    const MAX_PLAN_DETAILS = 300;

    let {
        preview,
        expertSearch = $bindable(''),
        expertFilter = $bindable<'all' | 'messages' | 'provider' | 'parameters' | 'diff'>('all'),
    }: Props = $props();

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

    const messageResults = $derived(
        preview.messages.filter((message) =>
            expertMatches(
                message.block_id,
                message.block_kind,
                message.requested_role,
                message.effective_role,
            ),
        ),
    );
    const providerResults = $derived(
        preview.provider_messages.filter((message) =>
            expertMatches(
                message.block_id,
                message.effective_role,
                message.wire_role,
                message.placement,
            ),
        ),
    );
    const parameterResults = $derived(
        preview.applied_parameters.filter((parameter) =>
            expertMatches(parameter.field, parameter.value_kind, parameter.item_count),
        ),
    );
    const diffResults = $derived(
        preview.prompt_diff.filter((entry) =>
            expertMatches(
                entry.block_id,
                entry.requested_role,
                entry.effective_role,
                entry.wire_role,
                entry.placement,
            ),
        ),
    );
    const memoryEvidenceBlocks = $derived(
        preview.blocks.filter((block) => block.memory_evidence.length > 0),
    );
    const knowledgeEvidenceBlocks = $derived(
        preview.blocks.filter((block) => block.knowledge_evidence.length > 0),
    );
</script>

<!-- prettier-ignore-start -->

<div class="expert-preview-controls" data-studio-owned-fields="">
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
{#if expertFilter === 'all' || expertFilter === 'messages'}
    <details class="expert-preview-section" data-studio-owned-details="">
        <summary>최종 메시지 구조 ({messageResults.length}개)</summary>
        {#if messageResults.length === 0}
            <p class="empty-note">검색과 일치하는 메시지가 없습니다.</p>
        {:else}
            <ol class="message-preview-list">
                {#each messageResults.slice(0, MAX_PLAN_MESSAGES) as message (`message:${String(message.sequence)}:${message.block_id}`)}
                    <li>
                        <header>
                            <strong>{message.requested_role} → {message.effective_role}</strong>
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
    <details class="expert-preview-section" data-studio-owned-details="" data-studio-owned-lists="" data-studio-owned-code="">
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
                            <strong>{message.effective_role} → {message.wire_role}</strong>
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
    <details class="expert-preview-section" data-studio-owned-details="" data-studio-owned-code="">
        <summary>적용 파라미터 구조 ({parameterResults.length}개)</summary>
        {#if parameterResults.length === 0}
            <p class="empty-note">검색과 일치하는 파라미터가 없습니다.</p>
        {:else}
            <dl class="state-list" data-studio-owned-definition="">
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
    <details class="expert-preview-section" data-studio-owned-details="" data-studio-owned-lists="">
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
                            {entry.requested_role} → {entry.effective_role} → {entry.wire_role} ·
                            {entry.placement}
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
    <table data-studio-owned-table="">
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
                    <td>{block.original_estimated_tokens} / {block.final_estimated_tokens}</td>
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
{#if knowledgeEvidenceBlocks.length > 0}
    <h4>세계관 지식 선택 근거</h4>
    {#each knowledgeEvidenceBlocks.slice(0, MAX_PLAN_DETAILS) as block (`knowledge-evidence:${block.block_id}`)}
        <details data-studio-owned-details="" data-studio-owned-lists="">
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
                            {evidence.exclusion_code ?? boundedJson(evidence.reasons, 4096)}
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
        <details data-studio-owned-details="" data-studio-owned-lists="">
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
                        {evidence.exclusion_code ?? boundedJson(evidence.reasons)}
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

<div class="profile-columns" data-studio-owned-lists="">
    <div>
        <h4>역할 매핑</h4>
        <ul class="compact-list">
            {#each preview.role_mappings.slice(0, MAX_PLAN_DETAILS) as mapping (`${mapping.block_id}:${mapping.requested_role}:${mapping.effective_role}`)}
                <li>
                    {mapping.block_id}: {mapping.requested_role} → {mapping.effective_role}
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
                        {overflow.block_id} · {overflow.policy} · {overflow.tokens_before} →
                        {overflow.tokens_after}
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

<!-- prettier-ignore-end -->
