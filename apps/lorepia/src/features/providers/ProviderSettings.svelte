<script lang="ts">
    import { EllipsisVertical } from '@lucide/svelte';
    import { setContext } from 'svelte';

    import type { LorepiaAppController, LorepiaAppState } from '../../app/app-controller';
    import DetailActionBar from '../../components/detail/DetailActionBar.svelte';
    import {
        DETAIL_SCROLL_CONTEXT,
        type DetailScrollListener,
    } from '../../components/detail/detail-scroll';
    import { tr } from '../../lib/i18n';
    import type {
        CredentialTargetDto,
        ProviderConnectionDto,
        ProviderProfileDto,
    } from '../../lib/ipc/contracts';
    import OpenSourceLicenses from '../licenses/OpenSourceLicenses.svelte';
    import PersonaPanel from '../personas/PersonaPanel.svelte';
    import type { PersonaController, PersonaState } from '../personas/persona-controller';
    import CapabilityPanel from './CapabilityPanel.svelte';
    import CatalogPanel from './CatalogPanel.svelte';
    import DiscoveryPanel from './DiscoveryPanel.svelte';
    import ModelSyncPanel from './ModelSyncPanel.svelte';
    import ProviderCrudPanel from './ProviderCrudPanel.svelte';
    import type { SettingsDetailPage, SettingsSection } from './settings-contracts';
    import AppearanceSection from './settings/AppearanceSection.svelte';
    import ConnectionSection from './settings/ConnectionSection.svelte';
    import CredentialSection from './settings/CredentialSection.svelte';
    import ModelRouteSection from './settings/ModelRouteSection.svelte';
    import SettingsOverview from './settings/SettingsOverview.svelte';
    import SettingsToolsSection from './settings/SettingsToolsSection.svelte';
    import TemplateSection from './settings/TemplateSection.svelte';
    import './settings/styles/provider-settings-a.css';
    import './settings/styles/provider-settings-b.css';

    interface Props {
        appState: LorepiaAppState;
        desktop?: boolean;
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
        titlebarOverlay?: boolean;
    }

    let {
        appState,
        controller,
        desktop = false,
        personaState,
        personaController,
        personaEditorMode = $bindable(null),
        detailPage = $bindable(null),
        editorMode = $bindable(null),
        editorTitle = $bindable(''),
        section = null,
        onOpenSection = () => undefined,
        onDetailScroll = () => undefined,
        titlebarOverlay = false,
    }: Props = $props();

    setContext<DetailScrollListener>(DETAIL_SCROLL_CONTEXT, (scrollTop) => {
        onDetailScroll(scrollTop);
    });

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

    function settingsPageTitle(): string {
        return section === null ? '일반' : $tr(`settings.section.${section}.title`);
    }

    const desktopNestedRoute = $derived(
        desktop &&
            section !== null &&
            (detailPage !== null || editorMode !== null || personaEditorMode !== null),
    );
</script>

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
    {#if desktop && !desktopNestedRoute}
        <header
            class="desktop-settings-page-heading"
            data-tauri-drag-region={titlebarOverlay ? '' : undefined}
        >
            <h1 data-tauri-drag-region={titlebarOverlay ? '' : undefined}>
                {settingsPageTitle()}
            </h1>
        </header>
    {/if}

    {#if section === null && desktop}
        <div
            bind:this={settingsScrollElement}
            class="provider-scroll desktop-settings-overview"
            onscroll={handleSettingsDetailScroll}
        >
            <SettingsOverview
                {appState}
                {desktop}
                {personaState}
                {titlebarOverlay}
                onSelectSection={openSettingsShortcut}
            />
        </div>
    {:else if section === null}
        <div
            class="mobile-top-frame settings-toolbar"
            class:titlebar-overlay={titlebarOverlay}
            role="toolbar"
            aria-label="설정 도구"
            data-tauri-drag-region={titlebarOverlay ? '' : undefined}
        >
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
            <SettingsOverview
                {appState}
                {desktop}
                {personaState}
                {titlebarOverlay}
                onSelectSection={openSettingsShortcut}
            />
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
            data-provider-owned-definition=""
            data-provider-owned-note=""
            onscroll={handleSettingsDetailScroll}
        >
            {#if section === 'appearance'}
                <AppearanceSection {desktop} />
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
                {#if section === 'target'}
                    <ModelRouteSection
                        {appState}
                        {settingsBusy}
                        {selectedRouteId}
                        {selectedPresetId}
                        {preservePartialGenerations}
                        {selectableRoutes}
                        {selectedRoutePresets}
                        preview={detailPage === 'preview'}
                        onChangeRoute={changeRoute}
                        onSelectPreset={(value: string) => {
                            selectedPresetId = value;
                            targetSelectionDirty = true;
                        }}
                        onPreservePartialChange={(checked: boolean) => {
                            preservePartialGenerations = checked;
                        }}
                        {openTargetPreview}
                    />
                {/if}
                {#if section === 'connections'}
                    <ConnectionSection
                        {appState}
                        connection={selectedConnection()}
                        legacyProfile={selectedLegacyProfile()}
                        {retainedLegacyProfileIds}
                        {settingsBusy}
                        {selectingProfileId}
                        onOpenDetailPage={(page: string) => openDetailPage(page)}
                        onSelectLegacyProfile={(profileId: string) =>
                            void selectLegacyProfile(profileId)}
                    />
                {/if}
                {#if section === 'templates'}
                    <TemplateSection
                        {appState}
                        {detailPage}
                        onOpenDetailPage={(page: string, title = '') => openDetailPage(page, title)}
                    />
                {/if}
                {#if section === 'discovery' && detailPage === null}
                    <SettingsToolsSection
                        {section}
                        {workspace}
                        onOpenDetailPage={(page: string) => openDetailPage(page)}
                    />
                {/if}
                {#if section === 'advanced'}
                    <SettingsToolsSection
                        {section}
                        {workspace}
                        onOpenDetailPage={(page: string) => openDetailPage(page)}
                    />
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
            <CredentialSection
                {credentialTarget}
                showActions={credentialTarget !== null &&
                    (!connection ||
                        (connection.credential_binding_required &&
                            !appState.providers.workspace.legacy_profiles.some(
                                (profile) => profile.id === connection.id,
                            )))}
                credentialStatuses={appState.providers.workspace.credential_statuses}
                {savingKey}
                bind:credentialDeleteConfirmationKey
                {deleteCredential}
                {captureCredential}
            />
        {/if}
    {/if}
</section>
