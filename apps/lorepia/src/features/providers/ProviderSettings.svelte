<script lang="ts">
    import { setThemePreference, themePreference } from '../../lib/theme';
    import type { LorepiaAppController, LorepiaAppState } from '../../app/app-controller';
    import type {
        CredentialTargetDto,
        LorepiaClient,
        MemoryRecordSourceNavigationDto,
        PromptPresetHistoryClientApi,
        ProviderConnectionDto,
        ProviderProfileDto,
    } from '../../lib/ipc/contracts';
    import OrchestrationStudio from '../orchestration/OrchestrationStudio.svelte';
    import PersonaPanel from '../personas/PersonaPanel.svelte';
    import type { PersonaController, PersonaState } from '../personas/persona-controller';
    import type {
        ContentPackageController,
        ContentPackageState,
    } from '../orchestration/content-package-controller';
    import type {
        OrchestrationController,
        OrchestrationState,
    } from '../orchestration/orchestration-controller';
    import CapabilityPanel from './CapabilityPanel.svelte';
    import CatalogPanel from './CatalogPanel.svelte';
    import DiscoveryPanel from './DiscoveryPanel.svelte';
    import ModelSyncPanel from './ModelSyncPanel.svelte';
    import ProviderCrudPanel from './ProviderCrudPanel.svelte';

    interface Props {
        client?: LorepiaClient & Partial<PromptPresetHistoryClientApi>;
        appState: LorepiaAppState;
        controller: LorepiaAppController;
        orchestrationState?: OrchestrationState;
        orchestrationController?: OrchestrationController;
        contentPackageState?: ContentPackageState;
        contentPackageController?: ContentPackageController;
        personaState?: PersonaState;
        personaController?: PersonaController;
        onNavigateToMemorySource?: (source: MemoryRecordSourceNavigationDto) => void;
    }

    let {
        client,
        appState,
        controller,
        orchestrationState,
        orchestrationController,
        contentPackageState,
        contentPackageController,
        personaState,
        personaController,
        onNavigateToMemorySource = () => undefined,
    }: Props = $props();
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
</script>

<section class="provider-pane" aria-labelledby="provider-title">
    <header class="provider-header">
        <div>
            <p class="eyebrow">Local provider control</p>
            <h1 id="provider-title">프로바이더 설정</h1>
            <p>연결 정보는 Core에서, 자격증명은 운영체제 저장소에서 관리합니다.</p>
        </div>
        <button type="button" onclick={() => void controller.loadProviders()}> 새로고침 </button>
    </header>

    <div class="provider-scroll">
        <section class="settings-section" aria-labelledby="appearance-title">
            <div class="section-heading">
                <div>
                    <p class="eyebrow">Appearance</p>
                    <h2 id="appearance-title">화면 모드</h2>
                </div>
            </div>
            <p class="inline-note">
                시스템을 고르면 운영체제 설정을 따라갑니다. 나머지는 이 앱에만 고정되며 다시 열어도
                유지됩니다.
            </p>
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
        {#if personaState && personaController}
            <PersonaPanel
                {personaState}
                controller={personaController}
                conversationTitle={appState.selected_conversation?.title ?? null}
            />
        {/if}
        {#if orchestrationState && orchestrationController}
            <OrchestrationStudio
                {client}
                {appState}
                {orchestrationState}
                controller={orchestrationController}
                appController={controller}
                {contentPackageState}
                {contentPackageController}
                {onNavigateToMemorySource}
            />
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
                    <p class="inline-note">기존 프로바이더 프로필을 기본 대상으로 사용 중입니다.</p>
                {:else if workspace.settings.selected_model_route_id !== null && workspace.settings.selected_generation_preset_id !== null}
                    {@const selectedRoute = workspace.routes.find(
                        (route) => route.id === workspace.settings.selected_model_route_id,
                    )}
                    {@const selectedPreset = workspace.presets.find(
                        (preset) => preset.id === workspace.settings.selected_generation_preset_id,
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
                    <p class="inline-note warning">Core에 저장된 기본 생성 대상이 없습니다.</p>
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
                                void selectGenerationTarget(selectedRouteId, selectedPresetId)}
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
                <label class="toggle-row">
                    <input
                        type="checkbox"
                        checked={workspace.settings.preserve_partial_generations}
                        disabled={settingsBusy}
                        onchange={(event) =>
                            void setPreservePartialGenerations(event.currentTarget.checked)}
                    />
                    <span>취소·오류 시 생성된 일부 응답을 보존</span>
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
                                    {workspace.request_preview.header_names.join(', ') || '없음'}
                                </dd>
                            </div>
                        </dl>
                        <p>메시지 본문과 자격증명 값은 이 미리보기에 포함되지 않습니다.</p>
                    </article>
                {/if}
            </section>

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
                                        <p>{connection.template_id} · {connection.status}</p>
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
                                {/if}

                                {#if routesFor(connection).length > 0}
                                    <details>
                                        <summary
                                            >모델 라우트 {routesFor(connection).length}개</summary
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
                                                            : `프리셋: ${presetsFor(route.id)
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
                                        자격증명을 클립보드에 복사한 뒤 네이티브 캡처를 누르세요.
                                        값은 WebView에 전달되지 않습니다.
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
                                                void controller.deleteProviderCredential(target)}
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

            <section class="settings-section" aria-labelledby="templates-title">
                <div class="section-heading">
                    <div>
                        <p class="eyebrow">Catalog projection</p>
                        <h2 id="templates-title">사용 가능한 템플릿</h2>
                    </div>
                    <span class="count-badge">{workspace.templates.length}개</span>
                </div>
                {#if workspace.templates.length === 0}
                    <p class="inline-note">현재 Core가 노출한 템플릿이 없습니다.</p>
                {:else}
                    <ul class="template-list">
                        {#each workspace.templates as template (template.id)}
                            <li>
                                <strong>{template.display_name}</strong>
                                <span>{template.api_family} · v{template.manifest_version}</span>
                            </li>
                        {/each}
                    </ul>
                {/if}
            </section>

            <ProviderCrudPanel {appState} {controller} />
            <CapabilityPanel {appState} {controller} />
            <DiscoveryPanel {appState} {controller} />
            <ModelSyncPanel {appState} {controller} />
            <CatalogPanel {appState} {controller} />
        {/if}
    </div>
</section>

<style>
    .provider-pane {
        display: flex;
        flex-direction: column;
        width: 100%;
        height: 100%;
        min-height: 0;
        background: var(--surface);
    }

    .provider-header,
    .section-heading,
    .provider-card > header {
        display: flex;
        gap: 16px;
        align-items: center;
        justify-content: space-between;
    }

    .provider-header {
        padding: 22px;
        border-bottom: 1px solid var(--line);
    }

    .provider-header h1,
    .section-heading h2,
    .provider-card h3 {
        margin: 3px 0 0;
    }

    .provider-header > div > p:last-child,
    .provider-card header p {
        margin: 5px 0 0;
        color: var(--ink-muted);
    }

    .provider-scroll {
        display: grid;
        gap: 18px;
        padding: 20px;
        overflow-y: auto;
    }

    .settings-section {
        padding: 20px;
        border: 1px solid var(--line);
        border-radius: var(--radius-md);
        background: var(--surface-raised);
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

    .target-form label,
    .toggle-row {
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

    .toggle-row {
        display: flex;
        align-items: center;
        margin-top: 16px;
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
        grid-template-columns: repeat(auto-fit, minmax(300px, 1fr));
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

    @media (max-width: 640px) {
        .provider-header,
        .section-heading {
            align-items: flex-start;
        }

        .provider-scroll {
            padding: 12px;
        }

        .settings-section {
            padding: 15px;
        }

        .target-form,
        .summary-grid,
        .preview-card dl,
        .compact-list {
            grid-template-columns: 1fr;
        }
    }
</style>
