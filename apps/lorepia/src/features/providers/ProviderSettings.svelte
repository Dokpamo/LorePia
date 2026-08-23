<script lang="ts">
    import { tr } from '../../lib/i18n';
    import { setThemePreference, themePreference } from '../../lib/theme';
    import type { LorepiaAppController, LorepiaAppState } from '../../app/app-controller';
    import type {
        CredentialTargetDto,
        ProviderConnectionDto,
        ProviderProfileDto,
    } from '../../lib/ipc/contracts';
    import PersonaPanel from '../personas/PersonaPanel.svelte';
    import type { PersonaController, PersonaState } from '../personas/persona-controller';
    import type { SettingsSection } from './settings-contracts';
    import CapabilityPanel from './CapabilityPanel.svelte';
    import CatalogPanel from './CatalogPanel.svelte';
    import DiscoveryPanel from './DiscoveryPanel.svelte';
    import ModelSyncPanel from './ModelSyncPanel.svelte';
    import ProviderCrudPanel from './ProviderCrudPanel.svelte';

    const MOBILE_TOP_FADE_DISTANCE_PX = 48;

    interface Props {
        appState: LorepiaAppState;
        section?: SettingsSection | null;
        onOpenSection?: (section: SettingsSection) => void;
        controller: LorepiaAppController;
        personaState?: PersonaState;
        personaController?: PersonaController;
    }

    let {
        appState,
        controller,
        personaState,
        personaController,
        section = null,
        onOpenSection = () => undefined,
    }: Props = $props();

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
    ]);
    let settingsMenuOpen = $state(false);
    let savingKey = $state<string | null>(null);
    let selectingProfileId = $state<string | null>(null);
    let settingsBusy = $state(false);
    let selectedRouteId = $state('');
    let selectedPresetId = $state('');
    let syncedTargetKey = '';

    const selectedRoutePresets = $derived(
        appState.providers.workspace.presets.filter(
            (preset) => preset.model_route_id === selectedRouteId,
        ),
    );

    function handleSettingsDetailScroll(event: Event): void {
        const scroller = event.currentTarget as HTMLDivElement;
        const alpha = Math.min(
            1,
            Math.max(0, 1 - scroller.scrollTop / MOBILE_TOP_FADE_DISTANCE_PX),
        );
        scroller.style.setProperty('--mobile-top-mask-alpha', String(alpha));
    }

    $effect(() => {
        const settings = appState.providers.workspace.settings;
        const key = `${settings.selected_model_route_id ?? ''}:${
            settings.selected_generation_preset_id ?? ''
        }`;
        if (key === syncedTargetKey) return;
        syncedTargetKey = key;
        selectedRouteId = settings.selected_model_route_id ?? '';
        selectedPresetId = settings.selected_generation_preset_id ?? '';
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

    async function selectGenerationTarget(
        modelRouteId: string | null,
        generationPresetId: string | null,
    ): Promise<void> {
        if (settingsBusy) return;
        settingsBusy = true;
        try {
            await controller.selectProviderGenerationTarget(modelRouteId, generationPresetId);
        } finally {
            settingsBusy = false;
        }
    }

    async function setPreservePartialGenerations(preserve: boolean): Promise<void> {
        if (settingsBusy) return;
        settingsBusy = true;
        try {
            await controller.setPreservePartialGenerations(preserve);
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
    }

    function openSettingsShortcut(next: SettingsSection): void {
        settingsMenuOpen = false;
        onOpenSection(next);
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
                return personaState?.selection?.selected_persona?.value.name ?? '선택 안 함';
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
        }
    }
</script>

{#snippet tileMark(id: SettingsSection)}
    <svg viewBox="0 0 24 24" aria-hidden="true">
        {#if id === 'appearance'}
            <circle cx="12" cy="12" r="8" />
            <path d="M12 4v16" />
        {:else if id === 'persona'}
            <circle cx="12" cy="8.5" r="3.5" />
            <path d="M5 19.5a7 7 0 0 1 14 0" />
        {:else if id === 'target'}
            <circle cx="12" cy="12" r="8" />
            <circle cx="12" cy="12" r="3" />
        {:else if id === 'connections'}
            <path
                d="M9.5 14.5 6.8 17.2a3.8 3.8 0 0 1-5.4-5.4l2.7-2.7M14.5 9.5l2.7-2.7a3.8 3.8 0 0 1 5.4 5.4l-2.7 2.7M9 15l6-6"
            />
        {:else if id === 'templates'}
            <path d="M4 6h16v12H4z" />
            <path d="M4 10h16" />
        {:else if id === 'discovery'}
            <circle cx="10.5" cy="10.5" r="6.5" />
            <path d="m15.5 15.5 4.5 4.5" />
        {:else if id === 'catalog'}
            <path d="M4 7h16v12H4z" />
            <path d="M8 4h8v3M8 11h8M8 15h5" />
        {:else}
            <path d="M5 8h9M18 8h1M5 16h1M10 16h9" />
            <circle cx="16" cy="8" r="2" />
            <circle cx="8" cy="16" r="2" />
        {/if}
    </svg>
{/snippet}

<section
    class="provider-pane"
    aria-label={section === null
        ? $tr('app.tab.providers')
        : $tr(`settings.section.${section}.title`)}
>
    {#if section === null}
        <div class="mobile-top-bar settings-toolbar" role="toolbar" aria-label="설정 도구">
            <button
                class="icon-button ghost mobile-top-action mobile-top-action-right settings-tool-button"
                type="button"
                aria-label="설정 더보기"
                aria-expanded={settingsMenuOpen}
                onclick={() => (settingsMenuOpen = !settingsMenuOpen)}
            >
                <svg viewBox="0 0 24 24" aria-hidden="true">
                    <circle cx="12" cy="5" r="1.5" class="filled-mark" />
                    <circle cx="12" cy="12" r="1.5" class="filled-mark" />
                    <circle cx="12" cy="19" r="1.5" class="filled-mark" />
                </svg>
            </button>
            {#if settingsMenuOpen}
                <div class="settings-shortcuts" role="menu" aria-label="설정 바로가기">
                    <button
                        type="button"
                        role="menuitem"
                        onclick={() => openSettingsShortcut('appearance')}>화면 모드</button
                    >
                    <button
                        type="button"
                        role="menuitem"
                        onclick={() => openSettingsShortcut('advanced')}>고급 설정</button
                    >
                </div>
            {/if}
        </div>
        <div class="provider-scroll settings-home-scroll">
            <section class="settings-identity" aria-labelledby="settings-identity-title">
                <span class="settings-avatar-wrap" aria-hidden="true">
                    <span class="settings-avatar">L</span>
                    <span class="settings-avatar-badge">
                        <svg viewBox="0 0 24 24">
                            <path d="M4 7h10M18 7h2M4 17h2M10 17h10" />
                            <circle cx="16" cy="7" r="2.4" />
                            <circle cx="8" cy="17" r="2.4" />
                        </svg>
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
                                    <svg
                                        class="setting-chevron"
                                        viewBox="0 0 20 20"
                                        aria-hidden="true"
                                    >
                                        <path d="m7.5 4.5 5 5-5 5" />
                                    </svg>
                                </span>
                            </span>
                        </button>
                    </li>
                {/each}
            </ul>
        </div>
    {:else}
        <div class="provider-scroll settings-detail-scroll" onscroll={handleSettingsDetailScroll}>
            {#if section === 'appearance'}
                <section class="settings-section appearance-settings" aria-label="화면 모드 값">
                    <div class="segmented theme-picker" role="group" aria-label="화면 모드 선택">
                        <button
                            type="button"
                            class:active={$themePreference === 'system'}
                            aria-pressed={$themePreference === 'system'}
                            onclick={() => setThemePreference('system')}
                        >
                            시스템
                        </button>
                        <button
                            type="button"
                            class:active={$themePreference === 'light'}
                            aria-pressed={$themePreference === 'light'}
                            onclick={() => setThemePreference('light')}
                        >
                            라이트
                        </button>
                        <button
                            type="button"
                            class:active={$themePreference === 'dark'}
                            aria-pressed={$themePreference === 'dark'}
                            onclick={() => setThemePreference('dark')}
                        >
                            다크
                        </button>
                    </div>
                </section>
            {/if}
            {#if section === 'persona'}
                {#if personaState && personaController}
                    <PersonaPanel
                        {personaState}
                        controller={personaController}
                        conversationTitle={appState.selected_conversation?.title ?? null}
                    />
                {/if}
            {/if}
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
            {:else}
                {@const workspace = appState.providers.workspace}
                {@const retainedLegacyProfileIds = new Set(
                    workspace.legacy_profiles.map((profile) => profile.id),
                )}
                {#if section === 'target'}
                    <section class="settings-section" aria-labelledby="default-target-title">
                        <div class="section-heading">
                            <div>
                                <p class="eyebrow">Generation target</p>
                                <h2 id="default-target-title">저장된 기본 생성 대상</h2>
                            </div>
                            <button
                                type="button"
                                disabled={workspace.settings.selected_model_route_id === null}
                                onclick={() => void controller.previewSelectedProviderRequest()}
                            >
                                요청 구조 미리보기
                            </button>
                        </div>

                        {#if workspace.settings.selected_provider_profile_id !== null}
                            <p class="inline-note">
                                기존 프로바이더 프로필을 기본 대상으로 사용 중입니다.
                            </p>
                        {:else if workspace.settings.selected_model_route_id !== null && workspace.settings.selected_generation_preset_id !== null}
                            {@const selectedRoute = workspace.routes.find(
                                (route) => route.id === workspace.settings.selected_model_route_id,
                            )}
                            {@const selectedPreset = workspace.presets.find(
                                (preset) =>
                                    preset.id === workspace.settings.selected_generation_preset_id,
                            )}
                            <dl class="summary-grid">
                                <div>
                                    <dt>모델</dt>
                                    <dd>
                                        {selectedRoute?.display_name ??
                                            selectedRoute?.model_id ??
                                            '알 수 없음'}
                                    </dd>
                                </div>
                                <div>
                                    <dt>프리셋</dt>
                                    <dd>{selectedPreset?.display_name ?? '알 수 없음'}</dd>
                                </div>
                            </dl>
                        {:else}
                            <p class="inline-note warning">
                                Core에 저장된 기본 생성 대상이 없습니다.
                            </p>
                        {/if}

                        <div class="target-form">
                            <label>
                                <span>모델 라우트</span>
                                <select
                                    value={selectedRouteId}
                                    disabled={settingsBusy}
                                    onchange={(event) => changeRoute(event.currentTarget.value)}
                                >
                                    <option value="">선택 안 함</option>
                                    {#each workspace.routes.filter((route) => !retainedLegacyProfileIds.has(route.connection_id)) as route (route.id)}
                                        <option value={route.id}>
                                            {route.display_name ?? route.model_id}
                                        </option>
                                    {/each}
                                </select>
                            </label>
                            <label>
                                <span>생성 프리셋</span>
                                <select
                                    bind:value={selectedPresetId}
                                    disabled={settingsBusy || selectedRouteId === ''}
                                >
                                    <option value="">선택 안 함</option>
                                    {#each selectedRoutePresets as preset (preset.id)}
                                        <option value={preset.id}>{preset.display_name}</option>
                                    {/each}
                                </select>
                            </label>
                            <div class="target-actions">
                                <button
                                    class="primary"
                                    type="button"
                                    disabled={settingsBusy ||
                                        selectedRouteId === '' ||
                                        selectedPresetId === ''}
                                    onclick={() =>
                                        void selectGenerationTarget(
                                            selectedRouteId,
                                            selectedPresetId,
                                        )}
                                >
                                    기본 대상으로 저장
                                </button>
                                <button
                                    type="button"
                                    disabled={settingsBusy}
                                    onclick={() => {
                                        selectedRouteId = '';
                                        selectedPresetId = '';
                                        void selectGenerationTarget(null, null);
                                    }}
                                >
                                    기본 대상 해제
                                </button>
                            </div>
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
                                checked={workspace.settings.preserve_partial_generations}
                                disabled={settingsBusy}
                                onchange={(event) =>
                                    void setPreservePartialGenerations(event.currentTarget.checked)}
                            />
                        </label>

                        {#if workspace.request_preview}
                            <article class="preview-card" aria-labelledby="request-preview-title">
                                <h3 id="request-preview-title">민감값이 제거된 요청 구조</h3>
                                <dl>
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
                                <p>메시지 본문과 자격증명 값은 이 미리보기에 포함되지 않습니다.</p>
                            </article>
                        {/if}
                    </section>
                {/if}
                {#if section === 'connections'}
                    <section class="settings-section" aria-labelledby="connections-title">
                        <div class="section-heading">
                            <div>
                                <p class="eyebrow">Connections</p>
                                <h2 id="connections-title">연결과 자격증명</h2>
                            </div>
                            <span class="count-badge">{workspace.connections.length}개</span>
                        </div>

                        {#if workspace.connections.length === 0 && workspace.legacy_profiles.length === 0}
                            <p class="inline-note">Core에 저장된 프로바이더 연결이 없습니다.</p>
                        {:else}
                            <div class="card-grid">
                                {#each workspace.connections as connection (connection.id)}
                                    {@const target = connectionTarget(connection.id)}
                                    {@const key = targetKey(target)}
                                    <article class="provider-card">
                                        <header>
                                            <div>
                                                <h3>{connection.display_name}</h3>
                                                <p>
                                                    {connection.template_id} · {connection.status}
                                                </p>
                                            </div>
                                            {#if !retainedLegacyProfileIds.has(connection.id)}
                                                <span class="status-pill">{statusLabel(key)}</span>
                                            {/if}
                                        </header>
                                        <dl class="compact-list">
                                            <div>
                                                <dt>Template</dt>
                                                <dd>{connection.template_id}</dd>
                                            </div>
                                            <div>
                                                <dt>Network</dt>
                                                <dd>{connection.network_mode}</dd>
                                            </div>
                                            <div>
                                                <dt>Timeout</dt>
                                                <dd>{connection.timeout_seconds}초</dd>
                                            </div>
                                        </dl>

                                        {#if connection.credential_binding_required && !retainedLegacyProfileIds.has(connection.id)}
                                            <div
                                                class="credential-form"
                                                aria-label={`${connection.display_name} 자격증명`}
                                            >
                                                <p>
                                                    자격증명을 클립보드에 복사한 뒤 네이티브 캡처를
                                                    누르세요. 값은 WebView에 전달되지 않습니다.
                                                </p>
                                                <div>
                                                    <button
                                                        class="primary"
                                                        type="button"
                                                        disabled={savingKey === key}
                                                        onclick={() =>
                                                            void captureCredential(target)}
                                                    >
                                                        클립보드에서 안전하게 캡처
                                                    </button>
                                                    <button
                                                        class="danger"
                                                        type="button"
                                                        disabled={workspace.credential_statuses[
                                                            key
                                                        ] === 'missing'}
                                                        onclick={() =>
                                                            void controller.deleteProviderCredential(
                                                                target,
                                                            )}
                                                    >
                                                        삭제
                                                    </button>
                                                </div>
                                            </div>
                                        {/if}

                                        {#if routesFor(connection).length > 0}
                                            <details>
                                                <summary
                                                    >모델 라우트 {routesFor(connection)
                                                        .length}개</summary
                                                >
                                                <ul class="route-list">
                                                    {#each routesFor(connection) as route (route.id)}
                                                        <li>
                                                            <strong
                                                                >{route.display_name ??
                                                                    route.model_id}</strong
                                                            >
                                                            <span
                                                                >{route.status} · {route.metadata_source}</span
                                                            >
                                                            <small>
                                                                {presetsFor(route.id).length === 0
                                                                    ? '프리셋 없음'
                                                                    : `프리셋: ${presetsFor(
                                                                          route.id,
                                                                      )
                                                                          .map(
                                                                              (preset) =>
                                                                                  preset.display_name,
                                                                          )
                                                                          .join(', ')}`}
                                                            </small>
                                                        </li>
                                                    {/each}
                                                </ul>
                                            </details>
                                        {/if}
                                    </article>
                                {/each}

                                {#each workspace.legacy_profiles as profile (profile.id)}
                                    {@const target = profileTarget(profile.id)}
                                    {@const key = targetKey(target)}
                                    <article class="provider-card legacy">
                                        <header>
                                            <div>
                                                <h3>{profile.display_name}</h3>
                                                <p>기존 프로필 · {profile.model}</p>
                                            </div>
                                            {#if profileSelected(profile)}
                                                <span class="status-pill selected">기본 대상</span>
                                            {/if}
                                        </header>
                                        <div class="legacy-actions">
                                            <button
                                                class="primary"
                                                type="button"
                                                disabled={settingsBusy || profileSelected(profile)}
                                                onclick={() => void selectLegacyProfile(profile.id)}
                                            >
                                                {profileSelected(profile)
                                                    ? '기본 대상으로 사용 중'
                                                    : selectingProfileId === profile.id
                                                      ? '기본 대상으로 설정 중'
                                                      : '기본 대상으로 선택'}
                                            </button>
                                        </div>
                                        <div
                                            class="credential-form"
                                            aria-label={`${profile.display_name} 자격증명`}
                                        >
                                            <p>
                                                자격증명을 클립보드에 복사한 뒤 네이티브 캡처를
                                                누르세요. 값은 WebView에 전달되지 않습니다.
                                            </p>
                                            <div>
                                                <button
                                                    class="primary"
                                                    type="button"
                                                    disabled={savingKey === key}
                                                    onclick={() => void captureCredential(target)}
                                                >
                                                    클립보드에서 안전하게 캡처
                                                </button>
                                                <button
                                                    class="danger"
                                                    type="button"
                                                    disabled={workspace.credential_statuses[key] ===
                                                        'missing'}
                                                    onclick={() =>
                                                        void controller.deleteProviderCredential(
                                                            target,
                                                        )}
                                                >
                                                    삭제
                                                </button>
                                            </div>
                                        </div>
                                    </article>
                                {/each}
                            </div>
                        {/if}
                    </section>
                {/if}
                {#if section === 'templates'}
                    <section class="settings-section" aria-labelledby="templates-title">
                        <div class="section-heading">
                            <div>
                                <p class="eyebrow">Catalog projection</p>
                                <h2 id="templates-title">사용 가능한 템플릿</h2>
                            </div>
                            <span class="count-badge">{workspace.templates.length}개</span>
                        </div>
                        {#if workspace.templates.length === 0}
                            <p class="inline-note">현재 사용할 수 있는 템플릿이 없습니다.</p>
                        {:else}
                            <ul class="template-list">
                                {#each workspace.templates as template (template.id)}
                                    <li>
                                        <strong>{template.display_name}</strong>
                                        <span
                                            >{template.api_family} · v{template.manifest_version}</span
                                        >
                                    </li>
                                {/each}
                            </ul>
                        {/if}
                    </section>
                {/if}
                {#if section === 'discovery'}
                    <DiscoveryPanel {appState} {controller} />
                    <ModelSyncPanel {appState} {controller} />
                {/if}
                {#if section === 'catalog'}
                    <CatalogPanel {appState} {controller} />
                {/if}
                {#if section === 'advanced'}
                    <ProviderCrudPanel {appState} {controller} />
                    <CapabilityPanel {appState} {controller} />
                {/if}
            {/if}
        </div>
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

    :global(.app-shell[data-layout='mobile']) .settings-detail-scroll {
        padding-bottom: calc(var(--mobile-nav) + 28px + env(safe-area-inset-bottom));
    }

    :global(.app-shell[data-layout='mobile']) .provider-scroll.settings-home-scroll {
        padding-bottom: calc(var(--mobile-nav) + 26px + env(safe-area-inset-bottom));
    }

    .settings-toolbar {
        position: absolute;
        inset: 0 0 auto;
        background: transparent;
        pointer-events: none;
    }

    .settings-tool-button {
        color: var(--ink);
        pointer-events: auto;
    }

    .settings-tool-button .filled-mark {
        fill: currentcolor;
        stroke: none;
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
    }

    .settings-avatar {
        display: grid;
        width: 100%;
        height: 100%;
        border: 1px solid var(--accent-line);
        border-radius: 50%;
        background: var(--primary-bg);
        box-shadow: var(--shadow-2);
        color: var(--primary-ink);
        font-size: 2.125rem;
        font-weight: 750;
        place-items: center;
    }

    .settings-avatar-badge {
        position: absolute;
        right: -2px;
        bottom: -2px;
        display: grid;
        width: 36px;
        height: 36px;
        border: 4px solid var(--bg);
        border-radius: 50%;
        background: var(--surface-raised);
        color: var(--accent);
        place-items: center;
    }

    .settings-avatar-badge svg {
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

    @container view (max-width: 719px) {
        .provider-scroll.settings-home-scroll {
            display: flex;
            min-height: 0;
            flex-direction: column;
            padding-bottom: calc(var(--mobile-nav) + 26px + env(safe-area-inset-bottom));
        }

        .settings-home-scroll .setting-list {
            min-height: 0;
            flex: none;
            justify-content: flex-start;
        }
    }

    .section-heading {
        margin-bottom: 16px;
    }

    .section-heading h2 {
        font-size: 1.08rem;
    }

    .theme-picker {
        margin-top: 12px;
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

    .summary-grid,
    .preview-card dl,
    .compact-list {
        display: grid;
        grid-template-columns: repeat(2, minmax(0, 1fr));
        gap: 10px;
        margin: 0;
    }

    .summary-grid > div,
    .preview-card,
    .compact-list > div {
        padding: 12px;
        border-radius: 12px;
        background: var(--surface-sunken);
    }

    .target-form {
        display: grid;
        grid-template-columns: repeat(2, minmax(0, 1fr));
        gap: 12px;
        margin-top: 14px;
    }

    .target-form label {
        display: grid;
        gap: 7px;
        color: var(--ink-muted);
        font-size: 0.8rem;
        font-weight: 700;
    }

    .target-form select {
        width: 100%;
    }

    .target-actions {
        display: flex;
        grid-column: 1 / -1;
        gap: 8px;
        flex-wrap: wrap;
    }

    .legacy-actions {
        display: flex;
        margin-top: 14px;
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

    .preview-card {
        margin-top: 14px;
    }

    .preview-card h3 {
        margin: 0 0 12px;
    }

    .preview-card p,
    .inline-note {
        color: var(--ink-muted);
        line-height: 1.55;
    }

    .inline-note.warning {
        color: var(--warning);
    }

    .card-grid {
        display: grid;
        grid-template-columns: repeat(auto-fit, minmax(min(300px, 100%), 1fr));
        gap: 12px;
    }

    .provider-card {
        padding: 16px;
        border: 1px solid var(--line);
        border-radius: 14px;
    }

    .provider-card h3 {
        font-size: 0.98rem;
    }

    .provider-card header p {
        font-size: 0.78rem;
    }

    .compact-list {
        margin-top: 14px;
    }

    .credential-form {
        display: grid;
        gap: 10px;
        margin-top: 14px;
    }

    .credential-form > div {
        display: flex;
        gap: 8px;
    }

    .status-pill,
    .count-badge {
        padding: 5px 9px;
        border-radius: 999px;
        color: var(--ink-muted);
        background: var(--surface-sunken);
        font-size: 0.7rem;
        font-weight: 800;
    }

    .status-pill.selected {
        color: var(--accent);
        background: var(--accent-soft);
    }

    details {
        margin-top: 14px;
        color: var(--ink-muted);
    }

    .route-list,
    .template-list {
        display: grid;
        gap: 8px;
        margin: 10px 0 0;
        padding: 0;
        list-style: none;
    }

    .route-list li,
    .template-list li {
        display: grid;
        gap: 3px;
        padding: 10px;
        border-radius: 10px;
        background: var(--surface-sunken);
    }

    .route-list span,
    .route-list small,
    .template-list span {
        color: var(--ink-muted);
        font-size: 0.74rem;
    }

    @container view (max-width: 640px) {
        .target-form,
        .summary-grid,
        .preview-card dl,
        .compact-list {
            grid-template-columns: 1fr;
        }
    }
</style>
