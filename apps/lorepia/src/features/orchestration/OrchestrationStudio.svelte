<script lang="ts">
    import {
        Activity,
        BriefcaseBusiness,
        ChevronRight,
        Lightbulb,
        TextAlignStart,
    } from '@lucide/svelte';

    import type { LorepiaAppController, LorepiaAppState } from '../../app/app-controller';
    import { tr as translate } from '../../lib/i18n';
    import type {
        LorepiaClient,
        MemoryRecordSourceNavigationDto,
        PromptPresetHistoryClientApi,
    } from '../../lib/ipc/contracts';
    import type { ContentModuleLifecycleClientApi } from './module-lifecycle-contracts';
    import { STUDIO_SECTIONS, type StudioSection } from './studio-contracts';
    import type { OrchestrationController, OrchestrationState } from './orchestration-controller';
    import type {
        ContentPackageController,
        ContentPackageState,
    } from './content-package-controller';
    import ContentSection from './studio/ContentSection.svelte';
    import DiagnosticsSection from './studio/DiagnosticsSection.svelte';
    import MemorySection from './studio/MemorySection.svelte';
    import PromptSection from './studio/PromptSection.svelte';
    import RuntimePlanSection from './studio/RuntimePlanSection.svelte';
    import './studio/styles/studio-a.css';
    import './studio/styles/studio-b.css';

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
                <PromptSection
                    {client}
                    {appState}
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
            {/if}
            {#if section === 'memory'}
                <MemorySection
                    {appState}
                    {orchestrationState}
                    {controller}
                    {onNavigateToMemorySource}
                    bind:detailPage
                    bind:memoryDrafts
                    bind:pendingMemoryDeleteId
                    bind:knowledgeSample
                    bind:transformRuleId
                    bind:transformSample
                />
            {/if}
        </div>
    {:else if section !== null}
        <div class="studio-panel">
            {#if section === 'diagnostics' && detailPage !== 'plan'}
                <DiagnosticsSection {appState} {orchestrationState} {detailPage} />
            {/if}
            {#if section === 'content'}
                <ContentSection
                    {client}
                    {orchestrationState}
                    {contentPackageState}
                    {contentPackageController}
                    bind:detailPage
                />
            {/if}
            {#if section === 'diagnostics' && detailPage === 'plan'}
                <RuntimePlanSection
                    {client}
                    {appState}
                    {orchestrationState}
                    {controller}
                    {appController}
                    bind:planUserText
                    bind:reviewedSendBusy
                    bind:attemptApprovalRefreshEpoch
                    bind:expertSearch
                    bind:expertFilter
                />
            {/if}
        </div>
    {/if}
</section>
