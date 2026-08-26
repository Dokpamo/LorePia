<script lang="ts">
    import {
        BriefcaseBusiness,
        Check,
        CircleDot,
        Compass,
        EllipsisVertical,
        GitBranch,
        LayoutTemplate,
        Link2,
        ListChecks,
        Route,
        Scale,
        Search,
        SlidersHorizontal,
        SunMoon,
        UserRound,
    } from '@lucide/svelte';
    import { setContext } from 'svelte';
    import { tr } from '../../lib/i18n';
    import { setThemePreference, themePreference } from '../../lib/theme';
    import lorepiaLogoMark from '../../assets/lorepia-logo-mark.png';
    import type { LorepiaAppController, LorepiaAppState } from '../../app/app-controller';
    import type {
        AuthBindingDto,
        CredentialTargetDto,
        ParameterLiteralDto,
        ProviderConnectionDto,
        ProviderProfileDto,
        ProviderTemplateDto,
    } from '../../lib/ipc/contracts';
    import PersonaPanel from '../personas/PersonaPanel.svelte';
    import type { PersonaController, PersonaState } from '../personas/persona-controller';
    import OpenSourceLicenses from '../licenses/OpenSourceLicenses.svelte';
    import DetailActionBar from '../../components/detail/DetailActionBar.svelte';
    import {
        DETAIL_SCROLL_CONTEXT,
        type DetailScrollListener,
    } from '../../components/detail/detail-scroll';
    import type { SettingsDetailPage, SettingsSection } from './settings-contracts';
    import CapabilityPanel from './CapabilityPanel.svelte';
    import CatalogPanel from './CatalogPanel.svelte';
    import DiscoveryPanel from './DiscoveryPanel.svelte';
    import ModelSyncPanel from './ModelSyncPanel.svelte';
    import ProviderCrudPanel from './ProviderCrudPanel.svelte';

    interface Props {
        appState: LorepiaAppState;
        section?: SettingsSection | null;
        onOpenSection?: (section: SettingsSection) => void;
        controller: LorepiaAppController;
        personaState?: PersonaState;
        personaController?: PersonaController;
        personaEditorMode?: 'create' | 'edit' | null;
        detailPage?: SettingsDetailPage;
        editorMode?: string | null;
        editorTitle?: string;
        onDetailScroll?: DetailScrollListener;
    }

    let {
        appState,
        controller,
        personaState,
        personaController,
        personaEditorMode = $bindable(null),
        detailPage = $bindable(null),
        editorMode = $bindable(null),
        editorTitle = $bindable(''),
        section = null,
        onOpenSection = () => undefined,
        onDetailScroll = () => undefined,
    }: Props = $props();

    setContext<DetailScrollListener>(DETAIL_SCROLL_CONTEXT, (scrollTop) => {
        onDetailScroll(scrollTop);
    });

    /* The index is also a compact status surface, so every destination names its current value. */
    const entries = $derived([
        { id: 'appearance' as const },
        { id: 'persona' as const },
        { id: 'target' as const },
        { id: 'connections' as const },
        { id: 'templates' as const },
        { id: 'discovery' as const },
        { id: 'catalog' as const },
        { id: 'advanced' as const },
        { id: 'licenses' as const },
    ]);
    let settingsMenuOpen = $state(false);
    let savingKey = $state<string | null>(null);
    let selectingProfileId = $state<string | null>(null);
    let settingsBusy = $state(false);
    let credentialDeleteConfirmationKey = $state<string | null>(null);
    let selectedRouteId = $state('');
    let selectedPresetId = $state('');
    let preservePartialGenerations = $state(false);
    let targetSelectionDirty = $state(false);
    let settingsScrollElement = $state<HTMLDivElement>();
    let syncedTargetKey = '';
    let settingsRouteKey = '';

    const retainedLegacyProfileIds = $derived(
        new Set(appState.providers.workspace.legacy_profiles.map((profile) => profile.id)),
    );
    const selectableRoutes = $derived(
        appState.providers.workspace.routes.filter(
            (route) => !retainedLegacyProfileIds.has(route.connection_id),
        ),
    );
    const selectedRoutePresets = $derived(
        appState.providers.workspace.presets.filter(
            (preset) => preset.model_route_id === selectedRouteId,
        ),
    );
    const targetDraftValid = $derived(
        (selectedRouteId === '' && selectedPresetId === '') ||
            (selectableRoutes.some((route) => route.id === selectedRouteId) &&
                selectedRoutePresets.some((preset) => preset.id === selectedPresetId)),
    );
    const targetDraftHasChanges = $derived(
        targetSelectionDirty ||
            preservePartialGenerations !==
                appState.providers.workspace.settings.preserve_partial_generations,
    );

    function handleSettingsDetailScroll(event: Event): void {
        const scroller = event.currentTarget as HTMLDivElement;
        onDetailScroll(scroller.scrollTop);
    }

    $effect(() => {
        const settings = appState.providers.workspace.settings;
        const key = `${section === 'target' ? 'target' : 'other'}:${
            settings.selected_provider_profile_id ?? ''
        }:${settings.selected_model_route_id ?? ''}:${
            settings.selected_generation_preset_id ?? ''
        }:${String(settings.preserve_partial_generations)}:${selectableRoutes
            .map((route) => route.id)
            .join(',')}:${appState.providers.workspace.presets
            .map((preset) => `${preset.id}:${preset.model_route_id}`)
            .join(',')}`;
        if (key === syncedTargetKey) return;
        syncedTargetKey = key;
        const persistedRouteId =
            settings.selected_provider_profile_id === null
                ? (settings.selected_model_route_id ?? '')
                : '';
        const persistedPresetId =
            settings.selected_provider_profile_id === null
                ? (settings.selected_generation_preset_id ?? '')
                : '';
        selectedRouteId = selectableRoutes.some((route) => route.id === persistedRouteId)
            ? persistedRouteId
            : '';
        selectedPresetId = appState.providers.workspace.presets.some(
            (preset) =>
                preset.id === persistedPresetId && preset.model_route_id === selectedRouteId,
        )
            ? persistedPresetId
            : '';
        preservePartialGenerations = settings.preserve_partial_generations;
        targetSelectionDirty = false;
    });

    $effect(() => {
        const nextKey = `${section ?? ''}:${detailPage ?? ''}:${editorMode ?? ''}`;
        if (nextKey === settingsRouteKey) return;
        settingsRouteKey = nextKey;
        queueMicrotask(() => {
            const scroller = settingsScrollElement;
            if (!scroller) return;
            scroller.scrollTop = 0;
            onDetailScroll(0);
        });
    });

    function connectionTarget(connectionId: string): CredentialTargetDto {
        return { kind: 'connection', connection_id: connectionId };
    }

    function profileTarget(profileId: string): CredentialTargetDto {
        return { kind: 'legacy_profile', provider_profile_id: profileId };
    }

    function targetKey(target: CredentialTargetDto): string {
        switch (target.kind) {
            case 'connection':
                return `connection:${target.connection_id}`;
            case 'legacy_profile':
                return `legacy_profile:${target.provider_profile_id}`;
            case 'discovery_session':
                return `discovery_session:${target.session_id}`;
        }
    }

    function statusLabel(key: string): string {
        const status = appState.providers.workspace.credential_statuses[key];
        if (status === 'available') return '자격증명 저장됨';
        if (status === 'unreadable') return '자격증명 확인 불가';
        return '자격증명 없음';
    }

    async function captureCredential(target: CredentialTargetDto): Promise<void> {
        const key = targetKey(target);
        savingKey = key;
        try {
            await controller.captureProviderCredential(target);
        } finally {
            savingKey = null;
        }
    }

    async function deleteCredential(target: CredentialTargetDto): Promise<void> {
        const key = targetKey(target);
        savingKey = key;
        try {
            await controller.deleteProviderCredential(target);
            credentialDeleteConfirmationKey = null;
        } finally {
            savingKey = null;
        }
    }

    async function selectLegacyProfile(profileId: string): Promise<void> {
        if (settingsBusy) return;
        settingsBusy = true;
        selectingProfileId = profileId;
        try {
            await controller.selectLegacyProviderProfile(profileId);
        } finally {
            selectingProfileId = null;
            settingsBusy = false;
        }
    }

    async function saveGenerationTarget(): Promise<void> {
        if (settingsBusy) return;
        if (!targetDraftHasChanges || !targetDraftValid) return;
        const routeId = selectedRouteId === '' ? null : selectedRouteId;
        const presetId = selectedPresetId === '' ? null : selectedPresetId;
        const preservePartial = preservePartialGenerations;
        const preserveChanged =
            preservePartial !== appState.providers.workspace.settings.preserve_partial_generations;
        settingsBusy = true;
        try {
            const targetSaved = targetSelectionDirty
                ? await controller.selectProviderGenerationTarget(routeId, presetId)
                : true;
            if (targetSaved && preserveChanged) {
                await controller.setPreservePartialGenerations(preservePartial);
            }
        } finally {
            settingsBusy = false;
        }
    }

    async function openTargetPreview(): Promise<void> {
        if (settingsBusy) return;
        settingsBusy = true;
        try {
            if (await controller.previewSelectedProviderRequest()) detailPage = 'preview';
        } finally {
            settingsBusy = false;
        }
    }

    function routesFor(connection: ProviderConnectionDto) {
        return appState.providers.workspace.routes.filter(
            (route) => route.connection_id === connection.id,
        );
    }

    function presetsFor(routeId: string) {
        return appState.providers.workspace.presets.filter(
            (preset) => preset.model_route_id === routeId,
        );
    }

    function profileSelected(profile: ProviderProfileDto): boolean {
        return appState.providers.workspace.settings.selected_provider_profile_id === profile.id;
    }

    function changeRoute(routeId: string): void {
        selectedRouteId = routeId;
        selectedPresetId =
            appState.providers.workspace.presets.find((preset) => preset.model_route_id === routeId)
                ?.id ?? '';
        targetSelectionDirty = true;
    }

    function clearGenerationTargetDraft(): void {
        selectedRouteId = '';
        selectedPresetId = '';
        targetSelectionDirty = true;
    }

    function openSettingsShortcut(next: SettingsSection): void {
        settingsMenuOpen = false;
        onOpenSection(next);
    }

    function openDetailPage(next: string, title = ''): void {
        editorMode = null;
        editorTitle = title;
        detailPage = next;
    }

    function selectedTemplate(): ProviderTemplateDto | undefined {
        if (!detailPage?.startsWith('template:')) return undefined;
        return appState.providers.workspace.templates.find(
            (template) => template.id === detailPage?.slice('template:'.length),
        );
    }

    function authBindingLabel(binding: AuthBindingDto): string {
        if (binding.kind === 'none') return '없음';
        if (binding.kind === 'bearer_header') return 'Bearer 인증 헤더';
        return `${binding.header_name} 헤더 API 키`;
    }

    function formatParameterLiteral(literal: ParameterLiteralDto): string {
        if (literal.type === 'string_list' || literal.type === 'stop_sequence_list') {
            return literal.value.join(', ');
        }
        return String(literal.value);
    }

    function selectedConnection(): ProviderConnectionDto | undefined {
        if (!detailPage?.startsWith('connection:')) return undefined;
        return appState.providers.workspace.connections.find(
            (connection) => connection.id === detailPage?.slice('connection:'.length),
        );
    }

    function selectedLegacyProfile(): ProviderProfileDto | undefined {
        if (!detailPage?.startsWith('legacy:')) return undefined;
        return appState.providers.workspace.legacy_profiles.find(
            (profile) => profile.id === detailPage?.slice('legacy:'.length),
        );
    }

    function settingValue(id: SettingsSection): string {
        const workspace = appState.providers.workspace;
        switch (id) {
            case 'appearance':
                return $themePreference === 'system'
                    ? '시스템 기준'
                    : $themePreference === 'light'
                      ? '라이트'
                      : '다크';
            case 'persona':
                return `${String(personaState?.personas.length ?? 0)}개`;
            case 'target': {
                const legacyProfile = workspace.legacy_profiles.find(
                    (profile) => profile.id === workspace.settings.selected_provider_profile_id,
                );
                if (legacyProfile) return legacyProfile.display_name;
                const preset = workspace.presets.find(
                    (candidate) =>
                        candidate.id === workspace.settings.selected_generation_preset_id,
                );
                if (preset) return preset.display_name;
                const route = workspace.routes.find(
                    (candidate) => candidate.id === workspace.settings.selected_model_route_id,
                );
                return route?.display_name ?? route?.model_id ?? '선택 안 함';
            }
            case 'connections': {
                const count = workspace.connections.length + workspace.legacy_profiles.length;
                return `${String(count)}개 연결`;
            }
            case 'templates':
                return `${String(workspace.templates.length)}개 템플릿`;
            case 'discovery':
                return workspace.selected_discovery_id === null
                    ? `${String(workspace.discoveries.length)}개 기록`
                    : '진행 중';
            case 'catalog':
                return workspace.catalog_status === null
                    ? '기본 카탈로그'
                    : `${String(workspace.catalog_status.active_revision)}차`;
            case 'advanced':
                return `${String(workspace.routes.length)}개 라우트`;
            case 'licenses':
                return 'ISC · MIT';
        }
    }
</script>

{#snippet tileMark(id: SettingsSection)}
    {#if id === 'appearance'}
        <SunMoon aria-hidden="true" />
    {:else if id === 'persona'}
        <UserRound aria-hidden="true" />
    {:else if id === 'target'}
        <CircleDot aria-hidden="true" />
    {:else if id === 'connections'}
        <Link2 aria-hidden="true" />
    {:else if id === 'templates'}
        <LayoutTemplate aria-hidden="true" />
    {:else if id === 'discovery'}
        <Search aria-hidden="true" />
    {:else if id === 'catalog'}
        <BriefcaseBusiness aria-hidden="true" />
    {:else if id === 'advanced'}
        <SlidersHorizontal aria-hidden="true" />
    {:else}
        <Scale aria-hidden="true" />
    {/if}
{/snippet}

{#snippet providerPhaseState()}
    {#if appState.providers.phase === 'loading'}
        <div class="provider-state" role="status">프로바이더 상태를 불러오는 중입니다.</div>
    {:else if appState.providers.phase === 'error'}
        <div class="provider-state error" role="alert">
            <strong>프로바이더 상태를 불러오지 못했습니다.</strong>
            <p>{appState.providers.error}</p>
            <button type="button" onclick={() => void controller.loadProviders()}>
                다시 시도
            </button>
        </div>
    {/if}
{/snippet}

<section
    class="provider-pane"
    aria-label={section === null
        ? $tr('app.tab.providers')
        : $tr(`settings.section.${section}.title`)}
>
    {#if section === null}
        <div class="mobile-top-frame settings-toolbar" role="toolbar" aria-label="설정 도구">
            <button
                class="icon-button ghost mobile-top-action settings-tool-button"
                type="button"
                aria-label="설정 더보기"
                aria-expanded={settingsMenuOpen}
                aria-controls="settings-shortcuts"
                onclick={() => (settingsMenuOpen = !settingsMenuOpen)}
            >
                <EllipsisVertical aria-hidden="true" />
            </button>
            {#if settingsMenuOpen}
                <div
                    id="settings-shortcuts"
                    class="settings-shortcuts"
                    role="group"
                    aria-label="설정 바로가기"
                >
                    <button type="button" onclick={() => openSettingsShortcut('appearance')}
                        >화면 모드</button
                    >
                    <button type="button" onclick={() => openSettingsShortcut('advanced')}
                        >고급 설정</button
                    >
                </div>
            {/if}
        </div>
        <div class="provider-scroll settings-home-scroll">
            <section class="settings-identity" aria-labelledby="settings-identity-title">
                <span class="settings-avatar-wrap" aria-hidden="true">
                    <span
                        class="settings-avatar brand-logo-mark"
                        style:--logo-mask={`url("${lorepiaLogoMark}")`}
                    ></span>
                    <span class="settings-avatar-badge">
                        <SlidersHorizontal />
                    </span>
                </span>
                <div class="settings-identity-copy">
                    <h2 id="settings-identity-title">LorePia</h2>
                </div>
            </section>
            <ul class="setting-list">
                {#each entries as entry (entry.id)}
                    <li>
                        <button
                            class="setting-row"
                            type="button"
                            onclick={() => {
                                settingsMenuOpen = false;
                                onOpenSection(entry.id);
                            }}
                        >
                            <span class="setting-icon" aria-hidden="true">
                                {@render tileMark(entry.id)}
                            </span>
                            <span class="setting-content">
                                <span class="setting-copy">
                                    <strong>{$tr(`settings.section.${entry.id}.title`)}</strong>
                                </span>
                                <span class="setting-trailing">
                                    <span class="setting-value">{settingValue(entry.id)}</span>
                                </span>
                            </span>
                        </button>
                    </li>
                {/each}
            </ul>
        </div>
    {:else if section === 'persona'}
        {#if personaState && personaController}
            <PersonaPanel
                {personaState}
                controller={personaController}
                bind:editorMode={personaEditorMode}
            />
        {/if}
    {:else if section !== 'appearance' && section !== 'licenses' && appState.providers.phase !== 'ready'}
        <div
            bind:this={settingsScrollElement}
            class="provider-scroll settings-detail-scroll"
            onscroll={handleSettingsDetailScroll}
        >
            {@render providerPhaseState()}
        </div>
    {:else if section === 'catalog'}
        <CatalogPanel {appState} {controller} bind:detailPage />
    {:else if section === 'discovery' && detailPage === 'provider-discovery'}
        <DiscoveryPanel
            {appState}
            {controller}
            bind:nestedPage={editorMode}
            bind:nestedTitle={editorTitle}
        />
    {:else if section === 'discovery' && detailPage === 'model-sync'}
        <ModelSyncPanel
            {appState}
            {controller}
            bind:nestedPage={editorMode}
            bind:nestedTitle={editorTitle}
        />
    {:else if section === 'advanced' && detailPage === 'capabilities'}
        <CapabilityPanel {appState} {controller} bind:detailMode={editorMode} />
    {:else if section === 'advanced' && (detailPage === 'connections' || detailPage === 'routes' || detailPage === 'presets')}
        <ProviderCrudPanel
            {appState}
            {controller}
            resourcePage={detailPage}
            bind:detailMode={editorMode}
            bind:detailTitle={editorTitle}
        />
    {:else if section === 'licenses'}
        <div
            bind:this={settingsScrollElement}
            class="provider-scroll settings-detail-scroll"
            onscroll={handleSettingsDetailScroll}
        >
            <OpenSourceLicenses />
        </div>
    {:else}
        <div
            bind:this={settingsScrollElement}
            class="provider-scroll settings-detail-scroll"
            class:detail-scroll-has-actions={(section === 'target' && detailPage === null) ||
                (section === 'connections' && detailPage !== null)}
            onscroll={handleSettingsDetailScroll}
        >
            {#if section === 'appearance'}
                <ul class="setting-list detail-choice-list" aria-label="화면 모드 선택">
                    <li>
                        <button
                            type="button"
                            class="setting-row detail-choice-row"
                            aria-pressed={$themePreference === 'system'}
                            onclick={() => setThemePreference('system')}
                        >
                            <span class="setting-content">
                                <span class="setting-copy"><strong>시스템</strong></span>
                                {#if $themePreference === 'system'}
                                    <Check class="detail-check" aria-hidden="true" />
                                {/if}
                            </span>
                        </button>
                    </li>
                    <li>
                        <button
                            type="button"
                            class="setting-row detail-choice-row"
                            aria-pressed={$themePreference === 'light'}
                            onclick={() => setThemePreference('light')}
                        >
                            <span class="setting-content">
                                <span class="setting-copy"><strong>라이트</strong></span>
                                {#if $themePreference === 'light'}
                                    <Check class="detail-check" aria-hidden="true" />
                                {/if}
                            </span>
                        </button>
                    </li>
                    <li>
                        <button
                            type="button"
                            class="setting-row detail-choice-row"
                            aria-pressed={$themePreference === 'dark'}
                            onclick={() => setThemePreference('dark')}
                        >
                            <span class="setting-content">
                                <span class="setting-copy"><strong>다크</strong></span>
                                {#if $themePreference === 'dark'}
                                    <Check class="detail-check" aria-hidden="true" />
                                {/if}
                            </span>
                        </button>
                    </li>
                </ul>
            {/if}
            {#if section !== 'appearance' && appState.providers.phase === 'loading'}
                <div class="provider-state" role="status">프로바이더 상태를 불러오는 중입니다.</div>
            {:else if section !== 'appearance' && appState.providers.phase === 'error'}
                <div class="provider-state error" role="alert">
                    <strong>프로바이더 상태를 불러오지 못했습니다.</strong>
                    <p>{appState.providers.error}</p>
                    <button type="button" onclick={() => void controller.loadProviders()}>
                        다시 시도
                    </button>
                </div>
            {:else if section !== 'appearance'}
                {@const workspace = appState.providers.workspace}
                {#if section === 'target' && detailPage === 'preview'}
                    <section class="detail-read-page" aria-label="민감값이 제거된 요청 구조">
                        {#if workspace.request_preview}
                            <dl class="detail-value-list">
                                <div>
                                    <dt>Method</dt>
                                    <dd>{workspace.request_preview.method}</dd>
                                </div>
                                <div>
                                    <dt>Origin</dt>
                                    <dd>{workspace.request_preview.origin}</dd>
                                </div>
                                <div>
                                    <dt>Path</dt>
                                    <dd>{workspace.request_preview.path}</dd>
                                </div>
                                <div>
                                    <dt>Headers</dt>
                                    <dd>
                                        {workspace.request_preview.header_names.join(', ') ||
                                            '없음'}
                                    </dd>
                                </div>
                            </dl>
                            <p class="inline-note">
                                메시지 본문과 자격증명 값은 이 미리보기에 포함되지 않습니다.
                            </p>
                        {:else}
                            <p class="inline-note">표시할 요청 구조가 없습니다.</p>
                        {/if}
                    </section>
                {:else if section === 'target'}
                    <section class="detail-form-page target-page" aria-label="기본 생성 대상 편집">
                        {#if workspace.settings.selected_provider_profile_id !== null}
                            <p class="inline-note">
                                기존 프로바이더 프로필을 기본 대상으로 사용 중입니다.
                            </p>
                        {:else if workspace.settings.selected_model_route_id === null}
                            <p class="inline-note warning">저장된 기본 생성 대상이 없습니다.</p>
                        {/if}

                        <div class="target-form detail-form">
                            <label>
                                <span>모델 라우트</span>
                                <select
                                    value={selectedRouteId}
                                    disabled={settingsBusy}
                                    onchange={(event) => changeRoute(event.currentTarget.value)}
                                >
                                    <option value="">선택 안 함</option>
                                    {#each selectableRoutes as route (route.id)}
                                        <option value={route.id}
                                            >{route.display_name ?? route.model_id}</option
                                        >
                                    {/each}
                                </select>
                            </label>
                            <label>
                                <span>생성 프리셋</span>
                                <select
                                    value={selectedPresetId}
                                    disabled={settingsBusy || selectedRouteId === ''}
                                    onchange={(event) => {
                                        selectedPresetId = event.currentTarget.value;
                                        targetSelectionDirty = true;
                                    }}
                                >
                                    <option value="">선택 안 함</option>
                                    {#each selectedRoutePresets as preset (preset.id)}
                                        <option value={preset.id}>{preset.display_name}</option>
                                    {/each}
                                </select>
                            </label>
                        </div>

                        <label class="settings-control-row">
                            <span class="settings-control-copy">
                                <strong>부분 응답 보존</strong>
                                <small>취소·오류 시 생성된 일부 응답을 보존</small>
                            </span>
                            <input
                                class="settings-switch"
                                type="checkbox"
                                role="switch"
                                aria-label="취소·오류 시 생성된 일부 응답을 보존"
                                checked={preservePartialGenerations}
                                disabled={settingsBusy}
                                onchange={(event) => {
                                    preservePartialGenerations = event.currentTarget.checked;
                                }}
                            />
                        </label>

                        <button
                            class="detail-secondary-action"
                            type="button"
                            disabled={settingsBusy ||
                                workspace.settings.selected_provider_profile_id !== null ||
                                workspace.settings.selected_model_route_id === null ||
                                selectedRouteId !== workspace.settings.selected_model_route_id ||
                                selectedPresetId !==
                                    workspace.settings.selected_generation_preset_id}
                            onclick={() => void openTargetPreview()}>요청 구조 미리보기</button
                        >
                    </section>
                {/if}
                {#if section === 'connections'}
                    {@const connection = selectedConnection()}
                    {@const legacyProfile = selectedLegacyProfile()}
                    {#if connection}
                        {@const target = connectionTarget(connection.id)}
                        {@const key = targetKey(target)}
                        <section
                            class="detail-read-page connection-detail"
                            aria-label={connection.display_name}
                        >
                            <dl class="detail-value-list">
                                <div>
                                    <dt>템플릿</dt>
                                    <dd>{connection.template_id}</dd>
                                </div>
                                <div>
                                    <dt>상태</dt>
                                    <dd>{connection.status}</dd>
                                </div>
                                <div>
                                    <dt>네트워크</dt>
                                    <dd>{connection.network_mode}</dd>
                                </div>
                                <div>
                                    <dt>시간 제한</dt>
                                    <dd>{connection.timeout_seconds}초</dd>
                                </div>
                                <div>
                                    <dt>자격증명</dt>
                                    <dd>{statusLabel(key)}</dd>
                                </div>
                            </dl>
                            {#if routesFor(connection).length > 0}
                                <section class="detail-subsection" aria-label="모델 라우트">
                                    <h3>모델 라우트</h3>
                                    <ul class="detail-plain-list">
                                        {#each routesFor(connection) as route (route.id)}
                                            <li>
                                                <strong
                                                    >{route.display_name ?? route.model_id}</strong
                                                >
                                                <span>{route.status} · {route.metadata_source}</span
                                                >
                                                <small
                                                    >{presetsFor(route.id).length === 0
                                                        ? '프리셋 없음'
                                                        : presetsFor(route.id)
                                                              .map((preset) => preset.display_name)
                                                              .join(', ')}</small
                                                >
                                            </li>
                                        {/each}
                                    </ul>
                                </section>
                            {/if}
                            {#if connection.credential_binding_required && !retainedLegacyProfileIds.has(connection.id)}
                                <p class="inline-note">
                                    자격증명은 클립보드에서 네이티브로 캡처하며 WebView에 전달되지
                                    않습니다.
                                </p>
                            {/if}
                        </section>
                    {:else if legacyProfile}
                        {@const target = profileTarget(legacyProfile.id)}
                        {@const key = targetKey(target)}
                        <section
                            class="detail-read-page connection-detail"
                            aria-label={legacyProfile.display_name}
                        >
                            <dl class="detail-value-list">
                                <div>
                                    <dt>종류</dt>
                                    <dd>기존 프로필</dd>
                                </div>
                                <div>
                                    <dt>모델</dt>
                                    <dd>{legacyProfile.model}</dd>
                                </div>
                                <div>
                                    <dt>자격증명</dt>
                                    <dd>{statusLabel(key)}</dd>
                                </div>
                            </dl>
                            <button
                                class="detail-secondary-action"
                                type="button"
                                disabled={settingsBusy || profileSelected(legacyProfile)}
                                onclick={() => void selectLegacyProfile(legacyProfile.id)}
                                >{profileSelected(legacyProfile)
                                    ? '기본 대상으로 사용 중'
                                    : selectingProfileId === legacyProfile.id
                                      ? '기본 대상으로 설정 중'
                                      : '기본 대상으로 선택'}</button
                            >
                            <p class="inline-note">
                                자격증명은 클립보드에서 네이티브로 캡처하며 WebView에 전달되지
                                않습니다.
                            </p>
                        </section>
                    {:else if workspace.connections.length === 0 && workspace.legacy_profiles.length === 0}
                        <p class="inline-note">저장된 프로바이더 연결이 없습니다.</p>
                    {:else}
                        <div class="setting-list detail-record-list" aria-label="연결 목록">
                            {#each workspace.connections as connectionItem (connectionItem.id)}
                                {@const target = connectionTarget(connectionItem.id)}
                                <button
                                    class="setting-row detail-record-row"
                                    type="button"
                                    onclick={() =>
                                        openDetailPage(`connection:${connectionItem.id}`)}
                                >
                                    <span class="setting-content">
                                        <span class="setting-copy detail-row-copy">
                                            <strong>{connectionItem.display_name}</strong>
                                            <small
                                                >{connectionItem.template_id} · {connectionItem.status}</small
                                            >
                                        </span>
                                        <span class="setting-value"
                                            >{statusLabel(targetKey(target))}</span
                                        >
                                    </span>
                                </button>
                            {/each}
                            {#each workspace.legacy_profiles as profile (profile.id)}
                                <button
                                    class="setting-row detail-record-row"
                                    type="button"
                                    onclick={() => openDetailPage(`legacy:${profile.id}`)}
                                >
                                    <span class="setting-content">
                                        <span class="setting-copy detail-row-copy">
                                            <strong>{profile.display_name}</strong>
                                            <small>기존 프로필 · {profile.model}</small>
                                        </span>
                                        <span class="setting-value"
                                            >{profileSelected(profile)
                                                ? '기본 대상'
                                                : statusLabel(
                                                      targetKey(profileTarget(profile.id)),
                                                  )}</span
                                        >
                                    </span>
                                </button>
                            {/each}
                        </div>
                    {/if}
                {/if}
                {#if section === 'templates'}
                    {@const template = selectedTemplate()}
                    {#if template}
                        <section
                            class="detail-read-page template-detail"
                            aria-label={`${template.display_name} 템플릿 정보`}
                        >
                            <dl class="detail-value-list" aria-label="템플릿 기본 정보">
                                <div>
                                    <dt>템플릿 ID</dt>
                                    <dd>{template.id}</dd>
                                </div>
                                <div>
                                    <dt>원본</dt>
                                    <dd>{template.source}</dd>
                                </div>
                                <div>
                                    <dt>API 패밀리</dt>
                                    <dd>{template.api_family}</dd>
                                </div>
                                <div>
                                    <dt>매니페스트</dt>
                                    <dd>v{template.manifest_version}</dd>
                                </div>
                                <div>
                                    <dt>기본 API Origin</dt>
                                    <dd>{template.default_api_origin ?? '사용자가 입력'}</dd>
                                </div>
                                <div>
                                    <dt>기본 네트워크</dt>
                                    <dd>{template.default_network_mode}</dd>
                                </div>
                                <div>
                                    <dt>인증 방식</dt>
                                    <dd>{authBindingLabel(template.auth_binding)}</dd>
                                </div>
                                <div>
                                    <dt>자격증명</dt>
                                    <dd>{template.credential_required ? '필요' : '필요 없음'}</dd>
                                </div>
                                <div>
                                    <dt>모델 목록</dt>
                                    <dd>
                                        {template.supports_model_listing ? '지원' : '지원 안 함'}
                                    </dd>
                                </div>
                            </dl>

                            <section
                                class="detail-subsection"
                                aria-labelledby="template-fields-title"
                            >
                                <h3 id="template-fields-title">연결 필드</h3>
                                {#if template.connection_fields.length === 0}
                                    <p class="inline-note">추가 연결 필드가 없습니다.</p>
                                {:else}
                                    <dl class="detail-value-list template-spec-list">
                                        {#each template.connection_fields as field (field.key)}
                                            <div>
                                                <dt>{field.label_key}</dt>
                                                <dd>
                                                    <span
                                                        >{field.key} · {field.value_type} · {field.required
                                                            ? '필수'
                                                            : '선택'}</span
                                                    >
                                                    {#if field.description_key}
                                                        <small>{field.description_key}</small>
                                                    {/if}
                                                </dd>
                                            </div>
                                        {/each}
                                    </dl>
                                {/if}
                            </section>

                            <section
                                class="detail-subsection"
                                aria-labelledby="template-parameters-title"
                            >
                                <h3 id="template-parameters-title">생성 파라미터</h3>
                                {#if template.parameters.length === 0}
                                    <p class="inline-note">정의된 생성 파라미터가 없습니다.</p>
                                {:else}
                                    <dl class="detail-value-list template-spec-list">
                                        {#each template.parameters as parameter (parameter.id)}
                                            <div>
                                                <dt>{parameter.label_key}</dt>
                                                <dd>
                                                    <span
                                                        >{parameter.id} · {parameter.value_type} · {parameter.level}</span
                                                    >
                                                    <small
                                                        >기본 {parameter.default_mode} · 전달 {parameter
                                                            .provider_mapping.target}:{parameter
                                                            .provider_mapping.field_name}</small
                                                    >
                                                    {#if parameter.allowed_values.length > 0}
                                                        <small
                                                            >허용값: {parameter.allowed_values
                                                                .map((choice) =>
                                                                    formatParameterLiteral(
                                                                        choice.value,
                                                                    ),
                                                                )
                                                                .join(', ')}</small
                                                        >
                                                    {/if}
                                                    {#if parameter.minimum !== null || parameter.maximum !== null || parameter.step !== null}
                                                        <small
                                                            >범위 {parameter.minimum ??
                                                                '제한 없음'}–{parameter.maximum ??
                                                                '제한 없음'} · 단계 {parameter.step ??
                                                                '기본값'}</small
                                                        >
                                                    {/if}
                                                    {#if parameter.visibility}
                                                        <small
                                                            >표시 조건: {parameter.visibility
                                                                .parameter_id}
                                                            {parameter.visibility.operator}
                                                            {formatParameterLiteral(
                                                                parameter.visibility.value,
                                                            )}</small
                                                        >
                                                    {/if}
                                                    {#if parameter.conflicts.length > 0}
                                                        <small
                                                            >충돌 규칙 {parameter.conflicts
                                                                .length}개</small
                                                        >
                                                    {/if}
                                                </dd>
                                            </div>
                                        {/each}
                                    </dl>
                                {/if}
                            </section>
                        </section>
                    {:else if detailPage?.startsWith('template:')}
                        <p class="inline-note">선택한 템플릿을 찾을 수 없습니다.</p>
                    {:else if workspace.templates.length === 0}
                        <p class="inline-note">현재 사용할 수 있는 템플릿이 없습니다.</p>
                    {:else}
                        <ul class="setting-list detail-record-list" aria-label="템플릿 목록">
                            {#each workspace.templates as template (template.id)}
                                <li>
                                    <button
                                        class="setting-row detail-record-row"
                                        type="button"
                                        onclick={() =>
                                            openDetailPage(
                                                `template:${template.id}`,
                                                template.display_name,
                                            )}
                                    >
                                        <span class="setting-content">
                                            <span class="setting-copy detail-row-copy">
                                                <strong>{template.display_name}</strong>
                                                <small
                                                    >{template.api_family} · v{template.manifest_version}</small
                                                >
                                            </span>
                                            <span class="setting-value"
                                                >필드 {template.connection_fields.length} · 파라미터 {template
                                                    .parameters.length}</span
                                            >
                                        </span>
                                    </button>
                                </li>
                            {/each}
                        </ul>
                    {/if}
                {/if}
                {#if section === 'discovery'}
                    {#if detailPage === null}
                        <div class="setting-list detail-tool-list" aria-label="검색과 동기화 도구">
                            <button
                                class="setting-row"
                                type="button"
                                onclick={() => openDetailPage('provider-discovery')}
                            >
                                <span class="setting-icon" aria-hidden="true"><Compass /></span>
                                <span class="setting-content">
                                    <span class="setting-copy detail-row-copy">
                                        <strong>프로바이더 탐색</strong>
                                        <small>연결을 찾고 검토해 추가합니다.</small>
                                    </span>
                                </span>
                            </button>
                            <button
                                class="setting-row"
                                type="button"
                                onclick={() => openDetailPage('model-sync')}
                            >
                                <span class="setting-icon" aria-hidden="true"><ListChecks /></span>
                                <span class="setting-content">
                                    <span class="setting-copy detail-row-copy">
                                        <strong>모델 동기화</strong>
                                        <small>연결에서 사용할 모델을 검토합니다.</small>
                                    </span>
                                </span>
                            </button>
                        </div>
                    {/if}
                {/if}
                {#if section === 'advanced'}
                    <div class="setting-list detail-tool-list" aria-label="고급 설정 도구">
                        <button
                            class="setting-row"
                            type="button"
                            onclick={() => openDetailPage('connections')}
                        >
                            <span class="setting-icon" aria-hidden="true"><Link2 /></span>
                            <span class="setting-content"
                                ><span class="setting-copy"><strong>연결 관리</strong></span><span
                                    class="setting-value">{workspace.connections.length}개</span
                                ></span
                            >
                        </button>
                        <button
                            class="setting-row"
                            type="button"
                            onclick={() => openDetailPage('routes')}
                        >
                            <span class="setting-icon" aria-hidden="true"><Route /></span>
                            <span class="setting-content"
                                ><span class="setting-copy"><strong>모델 라우트</strong></span><span
                                    class="setting-value">{workspace.routes.length}개</span
                                ></span
                            >
                        </button>
                        <button
                            class="setting-row"
                            type="button"
                            onclick={() => openDetailPage('presets')}
                        >
                            <span class="setting-icon" aria-hidden="true"><GitBranch /></span>
                            <span class="setting-content"
                                ><span class="setting-copy"><strong>생성 프리셋</strong></span><span
                                    class="setting-value">{workspace.presets.length}개</span
                                ></span
                            >
                        </button>
                        <button
                            class="setting-row"
                            type="button"
                            onclick={() => openDetailPage('capabilities')}
                        >
                            <span class="setting-icon" aria-hidden="true"
                                ><SlidersHorizontal /></span
                            >
                            <span class="setting-content"
                                ><span class="setting-copy"><strong>모델 기능</strong></span></span
                            >
                        </button>
                    </div>
                {/if}
            {/if}
        </div>
        {#if section === 'target' && detailPage === null && appState.providers.phase === 'ready'}
            <DetailActionBar ariaLabel="기본 생성 대상 작업">
                <button
                    class="detail-action detail-action--borderless"
                    type="button"
                    disabled={settingsBusy}
                    onclick={clearGenerationTargetDraft}>해제</button
                >
                <button
                    class="primary detail-action detail-action--grow"
                    type="button"
                    disabled={settingsBusy || !targetDraftValid || !targetDraftHasChanges}
                    onclick={() => void saveGenerationTarget()}>저장</button
                >
            </DetailActionBar>
        {/if}
        {#if section === 'connections' && appState.providers.phase === 'ready'}
            {@const connection = selectedConnection()}
            {@const legacyProfile = selectedLegacyProfile()}
            {@const credentialTarget = connection
                ? connectionTarget(connection.id)
                : legacyProfile
                  ? profileTarget(legacyProfile.id)
                  : null}
            {#if credentialTarget && (!connection || (connection.credential_binding_required && !appState.providers.workspace.legacy_profiles.some((profile) => profile.id === connection.id)))}
                {@const key = targetKey(credentialTarget)}
                <DetailActionBar ariaLabel="자격증명 작업">
                    {#if credentialDeleteConfirmationKey === key}
                        <button
                            class="danger detail-action detail-action--destructive"
                            type="button"
                            disabled={savingKey === key}
                            onclick={() => void deleteCredential(credentialTarget)}
                            >삭제 확인</button
                        >
                        <button
                            class="detail-action detail-action--grow"
                            type="button"
                            disabled={savingKey === key}
                            onclick={() => (credentialDeleteConfirmationKey = null)}>취소</button
                        >
                    {:else}
                        <button
                            class="detail-action detail-action--destructive detail-action--borderless"
                            type="button"
                            disabled={appState.providers.workspace.credential_statuses[key] ===
                                'missing' || savingKey === key}
                            onclick={() => (credentialDeleteConfirmationKey = key)}>삭제</button
                        >
                        <button
                            class="primary detail-action detail-action--grow"
                            type="button"
                            disabled={savingKey === key}
                            onclick={() => void captureCredential(credentialTarget)}
                            >자격증명 캡처</button
                        >
                    {/if}
                </DetailActionBar>
            {/if}
        {/if}
    {/if}
</section>

<style>
    .provider-pane {
        position: relative;
        display: flex;
        flex-direction: column;
        width: 100%;
        height: 100%;
        min-height: 0;
        background: var(--bg);
    }

    .provider-scroll {
        display: grid;
        height: 0;
        min-height: 0;
        flex: 1 1 0;
        gap: 16px;
        padding: 16px var(--settings-gutter) 24px;
        overflow-y: scroll;
    }

    .provider-scroll.settings-home-scroll {
        gap: 0;
        padding-top: clamp(36px, 10.297vw, 45px);
        padding-bottom: 24px;
        padding-inline: var(--settings-gutter);
    }

    .settings-detail-scroll {
        align-content: start;
        padding-bottom: 24px;
    }

    .settings-detail-scroll.detail-scroll-has-actions {
        padding-bottom: calc(var(--mobile-nav) + 36px + env(safe-area-inset-bottom));
    }

    :global(.app-shell[data-layout='mobile']) .settings-detail-scroll {
        padding-bottom: calc(clamp(10px, 5.492vw, 24px) + env(safe-area-inset-bottom));
    }

    :global(.app-shell[data-layout='mobile']) .settings-detail-scroll.detail-scroll-has-actions {
        padding-bottom: calc(
            var(--mobile-nav) + clamp(15px, 8.238vw, 36px) + env(safe-area-inset-bottom)
        );
    }

    :global(.app-shell[data-layout='mobile']) .provider-scroll.settings-home-scroll {
        display: flex;
        min-height: 0;
        flex-direction: column;
        padding-top: clamp(25px, 10.297vw, 45px);
        padding-bottom: calc(
            var(--mobile-nav) + clamp(11px, 5.95vw, 26px) + env(safe-area-inset-bottom)
        );
    }

    :global(.app-shell[data-layout='mobile']) .settings-home-scroll .setting-list {
        min-height: 0;
        flex: none;
        justify-content: flex-start;
        /* The fixed tab bar overlays the page, so keep a real scroll tail after the last row. */
        margin-bottom: calc(
            var(--mobile-nav) + clamp(11px, 5.95vw, 26px) + env(safe-area-inset-bottom)
        );
    }

    .settings-toolbar {
        position: absolute;
        inset: 0 0 auto;
        background: transparent;
        pointer-events: none;
    }

    .settings-tool-button {
        grid-column: 2;
        justify-self: end;
        color: var(--ink);
        pointer-events: auto;
    }

    .settings-shortcuts {
        position: absolute;
        z-index: 5;
        top: calc(100% - 2px);
        right: var(--mobile-top-inset);
        display: grid;
        min-width: 164px;
        padding: 6px;
        border: 1px solid var(--line);
        border-radius: var(--radius-md);
        background: var(--surface-raised);
        box-shadow: var(--shadow-2);
        pointer-events: auto;
    }

    .settings-shortcuts button {
        min-height: 42px;
        justify-content: flex-start;
        padding-inline: 12px;
        border: 0;
        background: transparent;
    }

    .settings-identity {
        display: grid;
        min-height: clamp(151px, 43.021vw, 188px);
        align-content: start;
        justify-items: center;
        padding: 0 20px 18px;
        margin-bottom: 8px;
        gap: clamp(10px, 2.975vw, 13px);
        text-align: center;
    }

    .settings-avatar-wrap {
        position: relative;
        display: block;
        width: clamp(87px, 24.714vw, 108px);
        height: clamp(87px, 24.714vw, 108px);
        flex: none;
        overflow: visible;
        border: 1px solid var(--accent-line);
        border-radius: 50%;
        background: var(--brand-logo-bg);
        box-shadow: var(--shadow-2);
    }

    .settings-avatar {
        position: absolute;
        inset: 0;
        display: block;
        width: 100%;
        height: 100%;
        border-radius: 50%;
    }

    .settings-avatar-badge {
        position: absolute;
        z-index: 1;
        right: -2px;
        bottom: -2px;
        display: grid;
        width: 36px;
        height: 36px;
        border: 4px solid var(--bg);
        border-radius: 50%;
        background: var(--surface-active);
        box-shadow: var(--shadow-1);
        color: var(--accent);
        place-items: center;
    }

    .settings-avatar-badge :global(svg) {
        width: 20px;
        height: 20px;
        fill: none;
        stroke: currentcolor;
        stroke-linecap: round;
        stroke-linejoin: round;
        stroke-width: 2;
    }

    .settings-identity-copy {
        display: grid;
        justify-items: center;
        gap: 3px;
    }

    .settings-identity-copy h2 {
        font-size: 1.5rem;
        font-weight: 650;
        line-height: 1.2;
        letter-spacing: -0.025em;
    }

    :global(.app-shell[data-layout='mobile']) .provider-scroll {
        gap: clamp(7px, 3.661vw, 16px);
        padding-inline: var(--settings-gutter);
    }

    :global(.app-shell[data-layout='mobile']) .settings-identity {
        min-height: clamp(103px, 43.021vw, 188px);
        padding: 0 clamp(8px, 4.577vw, 20px) clamp(7px, 4.119vw, 18px);
        margin-bottom: clamp(3px, 1.831vw, 8px);
        gap: clamp(7px, 2.975vw, 13px);
    }

    :global(.app-shell[data-layout='mobile']) .settings-avatar-wrap {
        width: clamp(59px, 24.714vw, 108px);
        height: clamp(59px, 24.714vw, 108px);
    }

    :global(.app-shell[data-layout='mobile']) .settings-avatar-badge {
        right: clamp(-3px, -0.458vw, -1px);
        bottom: clamp(-3px, -0.458vw, -1px);
        width: clamp(20px, 8.238vw, 36px);
        height: clamp(20px, 8.238vw, 36px);
        border-width: clamp(2px, 0.915vw, 4px);
    }

    :global(.app-shell[data-layout='mobile']) .settings-avatar-badge :global(svg) {
        width: clamp(11px, 4.577vw, 20px);
        height: clamp(11px, 4.577vw, 20px);
    }

    .detail-choice-list,
    .detail-record-list,
    .detail-tool-list {
        width: 100%;
        margin: 0;
    }

    .detail-choice-row .setting-content {
        justify-content: space-between;
    }

    :global(.detail-check) {
        width: 21px;
        height: 21px;
        flex: none;
        color: var(--accent);
    }

    .detail-form-page,
    .detail-read-page,
    .detail-subsection {
        display: grid;
        min-width: 0;
        gap: 16px;
    }

    .detail-form,
    .detail-form label {
        display: grid;
        gap: 7px;
    }

    .detail-form {
        grid-template-columns: 1fr;
        margin: 0;
    }

    .detail-form label {
        color: var(--ink-muted);
        font-size: var(--detail-support-type);
        font-weight: 700;
    }

    .detail-form :is(input, select) {
        width: 100%;
        min-height: clamp(48px, 13.73vw, 60px);
        padding: clamp(12px, 3.432vw, 15px);
        border: 1.5px solid var(--line);
        border-radius: var(--radius-md);
        background: color-mix(in srgb, var(--surface-sunken) 26%, var(--surface-raised));
        box-shadow: inset 0 1px 2px rgb(16 18 24 / 3%);
        color: var(--ink);
        font-size: var(--detail-support-type);
    }

    .detail-form :is(input, select):hover:not(:focus, :disabled) {
        border-color: var(--line);
    }

    .detail-form :is(input, select):focus {
        border-color: var(--accent);
        outline: 0;
    }

    :global(.app-shell[data-layout='mobile']) .detail-form :is(input, select) {
        min-height: clamp(33px, 13.73vw, 60px);
        padding: clamp(8px, 3.432vw, 15px);
    }

    .detail-secondary-action {
        width: 100%;
        min-height: clamp(48px, 13.73vw, 60px);
        justify-content: center;
        border-color: var(--line);
        font-size: var(--detail-support-type);
        font-weight: 700;
    }

    :global(.app-shell[data-layout='mobile']) .detail-secondary-action {
        min-height: clamp(33px, 13.73vw, 60px);
    }

    .detail-value-list {
        display: grid;
        margin: 0;
    }

    .detail-value-list > div {
        display: grid;
        grid-template-columns: minmax(100px, 0.65fr) minmax(0, 1.35fr);
        padding: 13px 2px;
        border-bottom: 1px solid var(--line);
        gap: 12px;
    }

    .template-spec-list dd {
        display: grid;
        min-width: 0;
        gap: 4px;
    }

    .template-spec-list dd small {
        color: var(--ink-muted);
        font-size: var(--detail-support-type);
        font-weight: 500;
        line-height: 1.4;
        overflow-wrap: anywhere;
    }

    .detail-row-copy {
        display: grid;
        min-width: 0;
        gap: 5px;
    }

    .detail-row-copy :is(strong, small) {
        overflow: hidden;
        font-size: var(--detail-support-type);
        line-height: 1.35;
        text-overflow: ellipsis;
    }

    .detail-row-copy small {
        display: -webkit-box;
        color: var(--ink-muted);
        font-weight: 500;
        white-space: normal;
        line-clamp: 3;
        -webkit-box-orient: vertical;
        -webkit-line-clamp: 3;
    }

    .detail-plain-list {
        display: grid;
        padding: 0;
        margin: 0;
        list-style: none;
    }

    .detail-plain-list li {
        display: grid;
        padding: 13px 2px;
        border-bottom: 1px solid var(--line);
        gap: 4px;
    }

    .detail-plain-list :is(span, small) {
        color: var(--ink-muted);
    }

    .provider-state {
        margin: auto;
        padding: 32px;
        color: var(--ink-muted);
        text-align: center;
    }

    .provider-state.error {
        color: var(--danger);
    }

    .target-form {
        display: grid;
        grid-template-columns: 1fr;
        gap: 14px;
        margin: 0;
    }

    .target-form label {
        display: grid;
        gap: 7px;
        color: var(--ink-muted);
        font-size: var(--detail-support-type);
        font-weight: 700;
    }

    .target-form select {
        width: 100%;
    }

    .settings-control-row {
        display: flex;
        min-height: 62px;
        align-items: center;
        justify-content: space-between;
        padding-top: 16px;
        border-top: 1px solid var(--line);
        margin-top: 16px;
        color: var(--ink);
        cursor: pointer;
        gap: 16px;
    }

    .settings-control-copy {
        display: grid;
        min-width: 0;
        gap: 3px;
    }

    .settings-control-copy strong {
        font-size: 0.9375rem;
        font-weight: 600;
    }

    .settings-control-copy small {
        color: var(--ink-muted);
        font-size: 0.8125rem;
        font-weight: 400;
        line-height: 1.35;
    }

    .settings-switch {
        position: relative;
        width: 50px;
        height: 30px;
        min-height: 30px;
        flex: none;
        padding: 0;
        border: 1px solid var(--line-strong);
        border-radius: var(--radius-pill);
        appearance: none;
        background: var(--surface-active);
        cursor: pointer;
        transition:
            background 140ms ease,
            border-color 140ms ease;
    }

    .settings-switch::after {
        position: absolute;
        width: 24px;
        height: 24px;
        border-radius: 50%;
        background: var(--surface-raised);
        box-shadow: var(--shadow-1);
        content: '';
        inset: 2px auto auto 2px;
        transition: transform 140ms ease;
    }

    .settings-switch:checked {
        border-color: var(--primary-bg);
        background: var(--primary-bg);
    }

    .settings-switch:checked::after {
        transform: translateX(20px);
    }

    .settings-switch:focus-visible {
        outline: 2px solid var(--accent);
        outline-offset: 2px;
    }

    .settings-switch:disabled {
        cursor: not-allowed;
        opacity: 0.5;
    }

    dt {
        color: var(--ink-muted);
        font-size: 0.72rem;
    }

    dd {
        margin: 4px 0 0;
        overflow-wrap: anywhere;
        font-weight: 700;
    }

    .inline-note {
        color: var(--ink-muted);
        line-height: 1.55;
    }

    .inline-note.warning {
        color: var(--warning);
    }

    @container view (max-width: 640px) {
        .target-form {
            grid-template-columns: 1fr;
        }
    }

    @media (max-width: 899px) {
        :global(.app-shell[data-layout='mobile']) .provider-scroll.settings-home-scroll {
            padding-top: clamp(25px, 10.297vw, 32px);
        }

        :global(.app-shell[data-layout='mobile']) .settings-identity {
            min-height: clamp(103px, 43.021vw, 150px);
            padding: 0 clamp(8px, 4.577vw, 16px) clamp(7px, 4.119vw, 14px);
            margin-bottom: clamp(3px, 1.831vw, 6px);
            gap: clamp(7px, 2.975vw, 8px);
        }

        :global(.app-shell[data-layout='mobile']) .settings-avatar-wrap {
            width: clamp(59px, 24.714vw, 88px);
            height: clamp(59px, 24.714vw, 88px);
        }

        :global(.app-shell[data-layout='mobile']) .settings-avatar-badge {
            right: clamp(-3px, -0.458vw, -2px);
            bottom: clamp(-3px, -0.458vw, -2px);
            width: clamp(20px, 8.238vw, 30px);
            height: clamp(20px, 8.238vw, 30px);
            border-width: clamp(2px, 0.915vw, 3px);
        }

        :global(.app-shell[data-layout='mobile']) .settings-avatar-badge :global(svg) {
            width: clamp(11px, 4.577vw, 16px);
            height: clamp(11px, 4.577vw, 16px);
        }
    }
</style>
