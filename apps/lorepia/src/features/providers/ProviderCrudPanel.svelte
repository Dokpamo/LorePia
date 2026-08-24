<script lang="ts">
    import { Plus } from '@lucide/svelte';
    import DetailActionBar from '../../components/detail/DetailActionBar.svelte';
    import DetailPage from '../../components/detail/DetailPage.svelte';
    import type { LorepiaAppController, LorepiaAppState } from '../../app/app-controller';
    import type {
        ApiFamilyInput,
        CreateProviderConnectionInput,
        GenerationParameterDto,
        GenerationPresetInput,
        ModelAvailabilityInput,
        ProviderConfigEntryDto,
        ProviderNetworkModeInput,
        UpdateProviderConnectionInput,
        UpsertModelRouteInput,
    } from '../../lib/ipc/contracts';

    interface Props {
        appState: LorepiaAppState;
        controller: LorepiaAppController;
        resourcePage?: 'connections' | 'routes' | 'presets' | null;
        detailMode?: string | null;
        detailTitle?: string;
    }

    type ResourcePage = 'connections' | 'routes' | 'presets';
    let {
        appState,
        controller,
        resourcePage = $bindable<ResourcePage | null>(null),
        detailMode = $bindable<string | null>(null),
        detailTitle = $bindable(''),
    }: Props = $props();
    let lastResourcePage: ResourcePage | null = resourcePage;

    let connectionBusy = $state(false);
    let connectionError = $state('');
    let connectionTemplateId = $state('');
    let connectionId = $state('');
    let connectionDisplayName = $state('');
    let connectionOrigin = $state('');
    let connectionBasePath = $state('');
    let connectionNetworkMode = $state<ProviderNetworkModeInput>('public');
    let connectionLocalOrigin = $state('');
    let connectionLocalAddresses = $state('');
    let connectionValuesJson = $state('[]');
    let connectionApprovedCredentialOrigin = $state('');
    let connectionTimeout = $state('30');
    let selectedConnectionId = $state('');
    let updateConnectionDisplayName = $state('');
    let updateConnectionTimeout = $state('30');
    let confirmConnectionDelete = $state(false);

    let routeBusy = $state(false);
    let routeError = $state('');
    let routeConnectionId = $state('');
    let routeId = $state('');
    let routeApiFamily = $state<ApiFamilyInput>('open_ai_responses');
    let routeModelId = $state('');
    let routeDisplayName = $state('');
    let routeDeploymentId = $state('');
    let routeRegion = $state('');
    let routeEndpointPath = $state('');
    let routeValuesJson = $state('[]');
    let routeStatus = $state<ModelAvailabilityInput>('available');
    let selectedRouteId = $state('');
    let updateRouteDisplayName = $state('');
    let updateRouteStatus = $state<ModelAvailabilityInput>('available');
    let confirmRouteDelete = $state(false);

    let presetBusy = $state(false);
    let presetError = $state('');
    let presetRouteId = $state('');
    let selectedPresetId = $state('');
    let presetId = $state('');
    let presetDisplayName = $state('');
    let presetValuesJson = $state('[]');
    let reasoningMode = $state('disabled');
    let reasoningEffort = $state('');
    let reasoningBudgetTokens = $state('');
    let reasoningSummary = $state('none');
    let reasoningPreserveOpaqueState = $state(false);
    let promptCacheMode = $state('disabled');
    let promptCacheTtlKind = $state('provider_default');
    let promptCacheTtlSeconds = $state('');
    let promptCacheContextReference = $state('');
    let confirmPresetDelete = $state(false);

    const workspace = $derived(appState.providers.workspace);
    const retainedLegacyProfileIds = $derived(
        new Set(workspace.legacy_profiles.map((profile) => profile.id)),
    );
    const ordinaryConnections = $derived(
        workspace.connections.filter((connection) => !retainedLegacyProfileIds.has(connection.id)),
    );
    const ordinaryRoutes = $derived(
        workspace.routes.filter((route) => !retainedLegacyProfileIds.has(route.connection_id)),
    );
    const ordinaryRouteIds = $derived(new Set(ordinaryRoutes.map((route) => route.id)));
    const ordinaryPresets = $derived(
        workspace.presets.filter((preset) => ordinaryRouteIds.has(preset.model_route_id)),
    );
    const selectedConnectionIsRetainedLegacy = $derived(
        retainedLegacyProfileIds.has(selectedConnectionId),
    );
    const selectedTemplate = $derived(
        workspace.templates.find((template) => template.id === connectionTemplateId) ?? null,
    );
    function pageTitle(page: ResourcePage | null, mode: string | null): string {
        if (page === 'connections') {
            if (mode === 'create') return '새 프로바이더 연결';
            if (mode === 'edit') return '프로바이더 연결 편집';
            return '프로바이더 연결';
        }
        if (page === 'routes') {
            if (mode === 'create') return '새 모델 라우트';
            if (mode === 'edit') return '모델 라우트 편집';
            return '모델 라우트';
        }
        if (page === 'presets') {
            if (mode === 'create') return '새 생성 프리셋';
            if (mode === 'edit') return '생성 프리셋 편집';
            return '생성 프리셋';
        }
        return '고급';
    }

    function resetConfirmations(): void {
        confirmConnectionDelete = false;
        confirmRouteDelete = false;
        confirmPresetDelete = false;
    }

    function openResource(page: ResourcePage): void {
        resetConfirmations();
        resourcePage = page;
        detailMode = null;
    }

    function openConnectionCreate(): void {
        resetConnectionCreateForm();
        connectionError = '';
        resetConfirmations();
        detailMode = 'create';
    }

    function openConnectionEdit(id: string): void {
        connectionError = '';
        selectConnection(id);
        detailMode = 'edit';
    }

    function openRouteCreate(): void {
        routeError = '';
        routeConnectionId = '';
        routeId = '';
        routeApiFamily = 'open_ai_responses';
        routeModelId = '';
        routeDisplayName = '';
        routeDeploymentId = '';
        routeRegion = '';
        routeEndpointPath = '';
        routeValuesJson = '[]';
        routeStatus = 'available';
        resetConfirmations();
        detailMode = 'create';
    }

    function openRouteEdit(id: string): void {
        routeError = '';
        selectRoute(id);
        detailMode = 'edit';
    }

    function openPresetCreate(): void {
        presetError = '';
        presetRouteId = '';
        clearPresetForm();
        detailMode = 'create';
    }

    function openPresetEdit(id: string): void {
        presetError = '';
        const preset = workspace.presets.find((candidate) => candidate.id === id);
        if (!preset) return;
        presetRouteId = preset.model_route_id;
        selectPreset(id);
        detailMode = 'edit';
    }

    $effect(() => {
        const page = resourcePage;
        if (page !== lastResourcePage) {
            lastResourcePage = page;
            detailMode = null;
            resetConfirmations();
        }
        const nextTitle = pageTitle(page, detailMode);
        if (detailTitle !== nextTitle) detailTitle = nextTitle;
    });

    function optionalText(value: string): string | null {
        const normalized = value.trim();
        return normalized === '' ? null : normalized;
    }

    function positiveInteger(value: string, label: string): number | null {
        const parsed = Number(value);
        if (!Number.isInteger(parsed) || parsed <= 0) {
            throw new Error(`${label}은(는) 1 이상의 정수여야 합니다.`);
        }
        return parsed;
    }

    function optionalNonNegativeInteger(value: unknown, label: string): number | null {
        if (value === null || value === undefined) return null;
        if (typeof value !== 'string' && typeof value !== 'number') {
            throw new Error(`${label}은(는) 숫자여야 합니다.`);
        }
        const normalized = typeof value === 'string' ? value.trim() : value;
        if (normalized === '') return null;
        const parsed = Number(normalized);
        if (!Number.isInteger(parsed) || parsed < 0) {
            throw new Error(`${label}은(는) 0 이상의 정수여야 합니다.`);
        }
        return parsed;
    }

    function isRecord(value: unknown): value is Record<string, unknown> {
        return typeof value === 'object' && value !== null && !Array.isArray(value);
    }

    function parseJsonArray(text: string, label: string): unknown[] {
        if (text.trim() === '') return [];
        let parsed: unknown;
        try {
            parsed = JSON.parse(text) as unknown;
        } catch {
            throw new Error(`${label} JSON 문법을 확인해 주세요.`);
        }
        if (!Array.isArray(parsed)) {
            throw new Error(`${label}은(는) JSON 배열이어야 합니다.`);
        }
        return parsed;
    }

    function parseConfigValues(text: string, label: string): ProviderConfigEntryDto[] {
        const entries = parseJsonArray(text, label);
        const valid = entries.every((entry) => {
            if (!isRecord(entry) || typeof entry.key !== 'string' || !isRecord(entry.value)) {
                return false;
            }
            switch (entry.value.type) {
                case 'text':
                    return typeof entry.value.value === 'string';
                case 'integer':
                    return (
                        typeof entry.value.value === 'number' && Number.isInteger(entry.value.value)
                    );
                case 'boolean':
                    return typeof entry.value.value === 'boolean';
                default:
                    return false;
            }
        });
        if (!valid) {
            throw new Error(
                `${label} 항목은 key와 text·integer·boolean 형식의 value를 가져야 합니다.`,
            );
        }
        return entries as ProviderConfigEntryDto[];
    }

    function parsePresetValues(text: string): GenerationParameterDto[] {
        const entries = parseJsonArray(text, '파라미터');
        const valid = entries.every(
            (entry) =>
                isRecord(entry) &&
                typeof entry.parameter_id === 'string' &&
                isRecord(entry.state) &&
                (entry.state.state === 'inherit_provider_default' ||
                    (entry.state.state === 'explicit' && isRecord(entry.state.value))),
        );
        if (!valid) {
            throw new Error(
                '파라미터 항목은 parameter_id와 inherit_provider_default 또는 explicit state를 가져야 합니다.',
            );
        }
        return entries as GenerationParameterDto[];
    }

    function selectTemplate(templateId: string): void {
        connectionTemplateId = templateId;
        const template = workspace.templates.find((candidate) => candidate.id === templateId);
        if (!template) return;
        connectionOrigin = template.default_api_origin ?? '';
        connectionNetworkMode = template.default_network_mode as ProviderNetworkModeInput;
        connectionValuesJson = JSON.stringify(
            template.connection_fields
                .filter((field) => field.required && field.value_type !== 'credential')
                .map((field) => ({
                    key: field.key,
                    value:
                        field.value_type === 'integer'
                            ? { type: 'integer', value: 0 }
                            : field.value_type === 'boolean'
                              ? { type: 'boolean', value: false }
                              : { type: 'text', value: '' },
                })),
            null,
            2,
        );
    }

    function resetConnectionCreateForm(): void {
        connectionTemplateId = '';
        connectionId = '';
        connectionDisplayName = '';
        connectionOrigin = '';
        connectionBasePath = '';
        connectionNetworkMode = 'public';
        connectionLocalOrigin = '';
        connectionLocalAddresses = '';
        connectionValuesJson = '[]';
        connectionApprovedCredentialOrigin = '';
        connectionTimeout = '30';
    }

    async function createConnection(): Promise<void> {
        connectionError = '';
        connectionBusy = true;
        try {
            if (!selectedTemplate) {
                throw new Error('프로바이더 템플릿을 선택해 주세요.');
            }
            const timeoutSeconds = positiveInteger(connectionTimeout, '타임아웃');
            if (timeoutSeconds === null) return;
            const localNetworkApproval =
                connectionNetworkMode === 'approved_local_network'
                    ? {
                          origin: connectionLocalOrigin.trim(),
                          addresses: connectionLocalAddresses
                              .split(/[\n,]/u)
                              .map((address) => address.trim())
                              .filter(Boolean),
                      }
                    : null;
            if (
                localNetworkApproval !== null &&
                (localNetworkApproval.origin === '' || localNetworkApproval.addresses.length === 0)
            ) {
                throw new Error('승인된 로컬 네트워크의 origin과 주소를 모두 입력해 주세요.');
            }
            const values = parseConfigValues(connectionValuesJson, '연결 값');
            const credentialKeys = new Set(
                selectedTemplate.connection_fields
                    .filter((field) => field.value_type === 'credential')
                    .map((field) => field.key),
            );
            if (values.some((entry) => credentialKeys.has(entry.key))) {
                throw new Error(
                    '자격증명 값은 이 화면에 입력할 수 없습니다. 연결을 만든 뒤 네이티브 캡처를 사용해 주세요.',
                );
            }

            const input: CreateProviderConnectionInput = {
                id: connectionId.trim(),
                template_id: selectedTemplate.id,
                template_version: selectedTemplate.manifest_version,
                display_name: connectionDisplayName.trim(),
                api_origin: connectionOrigin.trim(),
                api_base_path: optionalText(connectionBasePath),
                network_mode: connectionNetworkMode,
                local_network_approval: localNetworkApproval,
                values,
                approved_credential_origin: optionalText(connectionApprovedCredentialOrigin),
                timeout_seconds: timeoutSeconds,
            };
            const created = await controller.createProviderConnection(input);
            if (created) {
                resetConnectionCreateForm();
                detailMode = null;
            }
        } catch (error: unknown) {
            connectionError = error instanceof Error ? error.message : '연결 입력을 확인해 주세요.';
        } finally {
            connectionBusy = false;
        }
    }

    function selectConnection(connectionId: string): void {
        selectedConnectionId = connectionId;
        confirmConnectionDelete = false;
        const connection = workspace.connections.find((candidate) => candidate.id === connectionId);
        updateConnectionDisplayName = connection?.display_name ?? '';
        updateConnectionTimeout = connection ? String(connection.timeout_seconds) : '30';
    }

    async function updateConnection(): Promise<void> {
        if (selectedConnectionId === '' || selectedConnectionIsRetainedLegacy) return;
        connectionError = '';
        connectionBusy = true;
        try {
            const timeoutSeconds = positiveInteger(updateConnectionTimeout, '타임아웃');
            if (timeoutSeconds === null) return;
            const input: UpdateProviderConnectionInput = {
                id: selectedConnectionId,
                display_name: updateConnectionDisplayName.trim(),
                timeout_seconds: timeoutSeconds,
            };
            if (await controller.updateProviderConnection(input)) detailMode = null;
        } catch (error: unknown) {
            connectionError = error instanceof Error ? error.message : '연결 입력을 확인해 주세요.';
        } finally {
            connectionBusy = false;
        }
    }

    async function deleteConnection(): Promise<void> {
        if (
            selectedConnectionId === '' ||
            !confirmConnectionDelete ||
            selectedConnectionIsRetainedLegacy
        )
            return;
        connectionBusy = true;
        try {
            if (await controller.deleteProviderConnection(selectedConnectionId)) {
                selectConnection('');
                detailMode = null;
            }
        } finally {
            connectionBusy = false;
        }
    }

    async function createRoute(): Promise<void> {
        routeError = '';
        routeBusy = true;
        try {
            const input: UpsertModelRouteInput = {
                kind: 'create',
                id: routeId.trim(),
                connection_id: routeConnectionId,
                api_family: routeApiFamily,
                model_id: routeModelId.trim(),
                display_name: optionalText(routeDisplayName),
                route_config: {
                    deployment_id: optionalText(routeDeploymentId),
                    region: optionalText(routeRegion),
                    endpoint_path: optionalText(routeEndpointPath),
                    values: parseConfigValues(routeValuesJson, '라우트 값'),
                },
                status: routeStatus,
            };
            const saved = await controller.upsertProviderModelRoute(input);
            if (saved) {
                routeId = '';
                routeModelId = '';
                routeDisplayName = '';
                routeDeploymentId = '';
                routeRegion = '';
                routeEndpointPath = '';
                routeValuesJson = '[]';
                detailMode = null;
            }
        } catch (error: unknown) {
            routeError = error instanceof Error ? error.message : '라우트 입력을 확인해 주세요.';
        } finally {
            routeBusy = false;
        }
    }

    function selectRoute(routeId: string): void {
        selectedRouteId = routeId;
        confirmRouteDelete = false;
        const route = workspace.routes.find((candidate) => candidate.id === routeId);
        updateRouteDisplayName = route?.display_name ?? '';
        updateRouteStatus = (route?.status as ModelAvailabilityInput | undefined) ?? 'available';
    }

    function protectsRetainedLegacyRoute(routeId: string): boolean {
        const route = workspace.routes.find((candidate) => candidate.id === routeId);
        if (route?.metadata_source !== 'legacy') return false;
        const profile = workspace.legacy_profiles.find(
            (candidate) => candidate.id === route.connection_id,
        );
        return (
            profile?.model === route.model_id &&
            route.route_config.deployment_id === null &&
            route.route_config.region === null &&
            route.route_config.endpoint_path === null &&
            route.route_config.values.length === 0
        );
    }

    function protectsRetainedLegacyPreset(presetId: string): boolean {
        const preset = workspace.presets.find((candidate) => candidate.id === presetId);
        return (
            preset !== undefined &&
            preset.id === preset.model_route_id &&
            protectsRetainedLegacyRoute(preset.model_route_id)
        );
    }

    async function updateRoute(): Promise<void> {
        if (selectedRouteId === '') return;
        routeBusy = true;
        try {
            const input: UpsertModelRouteInput = {
                kind: 'update',
                id: selectedRouteId,
                display_name: optionalText(updateRouteDisplayName),
                status: updateRouteStatus,
            };
            if (await controller.upsertProviderModelRoute(input)) detailMode = null;
        } finally {
            routeBusy = false;
        }
    }

    async function deleteRoute(): Promise<void> {
        if (
            selectedRouteId === '' ||
            !confirmRouteDelete ||
            protectsRetainedLegacyRoute(selectedRouteId)
        )
            return;
        routeBusy = true;
        try {
            if (await controller.deleteProviderModelRoute(selectedRouteId)) {
                selectRoute('');
                detailMode = null;
            }
        } finally {
            routeBusy = false;
        }
    }

    function clearPresetForm(): void {
        selectedPresetId = '';
        presetId = '';
        presetDisplayName = '';
        presetValuesJson = '[]';
        reasoningMode = 'disabled';
        reasoningEffort = '';
        reasoningBudgetTokens = '';
        reasoningSummary = 'none';
        reasoningPreserveOpaqueState = false;
        promptCacheMode = 'disabled';
        promptCacheTtlKind = 'provider_default';
        promptCacheTtlSeconds = '';
        promptCacheContextReference = '';
        confirmPresetDelete = false;
    }

    function selectPresetRoute(routeId: string): void {
        presetRouteId = routeId;
        clearPresetForm();
    }

    function selectPreset(presetIdToSelect: string): void {
        clearPresetForm();
        selectedPresetId = presetIdToSelect;
        const preset = workspace.presets.find((candidate) => candidate.id === presetIdToSelect);
        if (!preset) return;
        presetId = preset.id;
        presetDisplayName = preset.display_name;
        presetValuesJson = JSON.stringify(preset.values, null, 2);
        reasoningMode = preset.reasoning.mode;
        reasoningEffort = preset.reasoning.effort ?? '';
        reasoningBudgetTokens =
            preset.reasoning.budget_tokens === null ? '' : String(preset.reasoning.budget_tokens);
        reasoningSummary = preset.reasoning.summary;
        reasoningPreserveOpaqueState = preset.reasoning.preserve_opaque_state;
        promptCacheMode = preset.prompt_cache.mode;
        promptCacheTtlKind = preset.prompt_cache.ttl_kind;
        promptCacheTtlSeconds =
            preset.prompt_cache.ttl_seconds === null ? '' : String(preset.prompt_cache.ttl_seconds);
        promptCacheContextReference = preset.prompt_cache.context_reference ?? '';
    }

    function buildPresetCandidate(): GenerationPresetInput | null {
        presetError = '';
        try {
            if (presetRouteId === '') throw new Error('모델 라우트를 선택해 주세요.');
            return {
                id: presetId.trim(),
                model_route_id: presetRouteId,
                display_name: presetDisplayName.trim(),
                values: parsePresetValues(presetValuesJson),
                reasoning: {
                    mode: reasoningMode.trim(),
                    effort: optionalText(reasoningEffort),
                    budget_tokens: optionalNonNegativeInteger(
                        reasoningBudgetTokens,
                        'Reasoning token budget',
                    ),
                    summary: reasoningSummary.trim(),
                    preserve_opaque_state: reasoningPreserveOpaqueState,
                },
                prompt_cache: {
                    mode: promptCacheMode.trim(),
                    ttl_kind: promptCacheTtlKind.trim(),
                    ttl_seconds: optionalNonNegativeInteger(
                        promptCacheTtlSeconds,
                        'Prompt cache TTL',
                    ),
                    context_reference: optionalText(promptCacheContextReference),
                },
            };
        } catch (error: unknown) {
            presetError = error instanceof Error ? error.message : '프리셋 입력을 확인해 주세요.';
            return null;
        }
    }

    async function savePreset(): Promise<void> {
        const candidate = buildPresetCandidate();
        if (candidate === null) return;
        presetBusy = true;
        try {
            if (await controller.upsertProviderGenerationPreset(candidate)) detailMode = null;
        } finally {
            presetBusy = false;
        }
    }

    async function validatePreset(): Promise<void> {
        const candidate = buildPresetCandidate();
        if (candidate === null) return;
        presetBusy = true;
        try {
            await controller.validateProviderGenerationPresetCandidate(candidate);
        } finally {
            presetBusy = false;
        }
    }

    async function previewPreset(): Promise<void> {
        const candidate = buildPresetCandidate();
        if (candidate === null) return;
        presetBusy = true;
        try {
            await controller.previewProviderRequestCandidate(candidate);
        } finally {
            presetBusy = false;
        }
    }

    async function deletePreset(): Promise<void> {
        if (
            selectedPresetId === '' ||
            !confirmPresetDelete ||
            protectsRetainedLegacyPreset(selectedPresetId)
        )
            return;
        presetBusy = true;
        try {
            if (await controller.deleteProviderGenerationPreset(selectedPresetId)) {
                clearPresetForm();
                detailMode = null;
            }
        } finally {
            presetBusy = false;
        }
    }
</script>

{#snippet detailContent()}
    {#if resourcePage === null}
        <div class="setting-list resource-index" aria-label="고급 설정 항목">
            <button
                class="setting-row resource-row"
                type="button"
                onclick={() => openResource('connections')}
            >
                <span class="setting-content">
                    <span class="setting-copy resource-copy">
                        <strong>프로바이더 연결</strong>
                        <small>{workspace.connections.length}개 연결</small>
                    </span>
                </span>
            </button>
            <button
                class="setting-row resource-row"
                type="button"
                onclick={() => openResource('routes')}
            >
                <span class="setting-content">
                    <span class="setting-copy resource-copy">
                        <strong>모델 라우트</strong>
                        <small>{ordinaryRoutes.length}개 라우트</small>
                    </span>
                </span>
            </button>
            <button
                class="setting-row resource-row"
                type="button"
                onclick={() => openResource('presets')}
            >
                <span class="setting-content">
                    <span class="setting-copy resource-copy">
                        <strong>생성 프리셋</strong>
                        <small>{ordinaryPresets.length}개 프리셋</small>
                    </span>
                </span>
            </button>
        </div>
    {:else if resourcePage === 'connections'}
        {#if detailMode === null}
            <div class="setting-list resource-list" aria-label="프로바이더 연결 목록">
                {#if workspace.connections.length === 0}
                    <p class="resource-empty">아직 등록된 프로바이더 연결이 없습니다.</p>
                {/if}
                {#each workspace.connections as connection (connection.id)}
                    <button
                        class="setting-row resource-row"
                        type="button"
                        disabled={connectionBusy}
                        onclick={() => openConnectionEdit(connection.id)}
                    >
                        <span class="setting-content">
                            <span class="setting-copy resource-copy">
                                <strong>{connection.display_name}</strong>
                                <small>{connection.api_origin}</small>
                            </span>
                        </span>
                    </button>
                {/each}
            </div>
            <p class="security-note">
                입력은 고수준 Tauri 명령으로만 전달됩니다. 자격증명은 이 화면에 보관하지 않습니다.
            </p>
        {:else if detailMode === 'create'}
            <form
                id="connection-editor-form"
                class="resource-form"
                aria-label="프로바이더 연결 만들기"
                onsubmit={(event) => {
                    event.preventDefault();
                    void createConnection();
                }}
            >
                <label>
                    <span>템플릿</span>
                    <select
                        value={connectionTemplateId}
                        required
                        onchange={(event) => selectTemplate(event.currentTarget.value)}
                    >
                        <option value="">선택</option>
                        {#each workspace.templates as template (template.id)}
                            <option value={template.id}>
                                {template.display_name} · v{template.manifest_version}
                            </option>
                        {/each}
                    </select>
                </label>
                <label>
                    <span>연결 ID</span>
                    <input bind:value={connectionId} required autocomplete="off" />
                </label>
                <label>
                    <span>표시 이름</span>
                    <input bind:value={connectionDisplayName} required autocomplete="off" />
                </label>
                <label>
                    <span>API origin</span>
                    <input bind:value={connectionOrigin} type="url" required autocomplete="url" />
                </label>
                <label>
                    <span>API base path (선택)</span>
                    <input bind:value={connectionBasePath} autocomplete="off" />
                </label>
                <label>
                    <span>네트워크 모드</span>
                    <select bind:value={connectionNetworkMode}>
                        <option value="public">공개 네트워크</option>
                        <option value="local_loopback">로컬 루프백</option>
                        <option value="approved_local_network">승인된 로컬 네트워크</option>
                    </select>
                </label>
                {#if connectionNetworkMode === 'approved_local_network'}
                    <label>
                        <span>승인 origin</span>
                        <input bind:value={connectionLocalOrigin} type="url" required />
                    </label>
                    <label>
                        <span>승인 주소 (줄바꿈 또는 쉼표 구분)</span>
                        <textarea
                            bind:value={connectionLocalAddresses}
                            rows="2"
                            required
                            spellcheck="false"></textarea>
                    </label>
                {/if}
                <label>
                    <span>연결 값 JSON</span>
                    <textarea
                        class="code-field"
                        bind:value={connectionValuesJson}
                        rows="5"
                        spellcheck="false"
                        aria-describedby="connection-values-help"></textarea>
                    <small id="connection-values-help">
                        자격증명 키는 제외하세요. [{`{"key":"organization","value":{"type":"text","value":"..."}}`}]
                    </small>
                </label>
                <label>
                    <span>승인된 자격증명 origin (선택)</span>
                    <input
                        bind:value={connectionApprovedCredentialOrigin}
                        type="url"
                        autocomplete="off"
                    />
                </label>
                <label>
                    <span>타임아웃 (초)</span>
                    <input bind:value={connectionTimeout} type="number" min="1" required />
                </label>
                <p class="security-note">
                    연결을 만든 뒤 자격증명 카드에서 운영체제 네이티브 캡처를 사용하세요. 자격증명은
                    이 화면이나 WebView 메모리에 들어오지 않습니다.
                </p>
                {#if connectionError}
                    <p class="form-error" role="alert">{connectionError}</p>
                {/if}
            </form>
        {:else}
            <form
                id="connection-editor-form"
                class="resource-form"
                aria-label="프로바이더 연결 수정 또는 삭제"
                onsubmit={(event) => {
                    event.preventDefault();
                    void updateConnection();
                }}
            >
                <label>
                    <span>표시 이름</span>
                    <input
                        bind:value={updateConnectionDisplayName}
                        required
                        disabled={selectedConnectionId === '' || selectedConnectionIsRetainedLegacy}
                    />
                </label>
                <label>
                    <span>타임아웃 (초)</span>
                    <input
                        bind:value={updateConnectionTimeout}
                        type="number"
                        min="1"
                        required
                        disabled={selectedConnectionId === '' || selectedConnectionIsRetainedLegacy}
                    />
                </label>
                {#if selectedConnectionIsRetainedLegacy}
                    <p class="security-note">
                        기존 프로필 연결의 메타데이터와 삭제는 기존 프로필 경로에서 관리됩니다.
                    </p>
                {:else if confirmConnectionDelete}
                    <p class="security-note" role="status">
                        이 연결과 연결에 종속된 설정이 함께 삭제됩니다. 하단에서 한 번 더 확인해
                        주세요.
                    </p>
                {/if}
                {#if connectionError}
                    <p class="form-error" role="alert">{connectionError}</p>
                {/if}
            </form>
        {/if}
    {:else if resourcePage === 'routes'}
        {#if detailMode === null}
            <div class="setting-list resource-list" aria-label="모델 라우트 목록">
                {#if ordinaryRoutes.length === 0}
                    <p class="resource-empty">아직 등록된 모델 라우트가 없습니다.</p>
                {/if}
                {#each ordinaryRoutes as route (route.id)}
                    <button
                        class="setting-row resource-row"
                        type="button"
                        disabled={routeBusy}
                        onclick={() => openRouteEdit(route.id)}
                    >
                        <span class="setting-content">
                            <span class="setting-copy resource-copy">
                                <strong>{route.display_name ?? route.model_id}</strong>
                                <small>{route.api_family} · {route.model_id}</small>
                            </span>
                        </span>
                    </button>
                {/each}
            </div>
        {:else if detailMode === 'create'}
            <form
                id="route-editor-form"
                class="resource-form"
                aria-label="모델 라우트 만들기"
                onsubmit={(event) => {
                    event.preventDefault();
                    void createRoute();
                }}
            >
                <label>
                    <span>연결</span>
                    <select bind:value={routeConnectionId} required>
                        <option value="">선택</option>
                        {#each ordinaryConnections as connection (connection.id)}
                            <option value={connection.id}>{connection.display_name}</option>
                        {/each}
                    </select>
                </label>
                <label>
                    <span>라우트 ID</span>
                    <input bind:value={routeId} required autocomplete="off" />
                </label>
                <label>
                    <span>API family</span>
                    <select bind:value={routeApiFamily}>
                        <option value="open_ai_responses">OpenAI Responses</option>
                        <option value="open_ai_chat_completions">OpenAI Chat Completions</option>
                        <option value="anthropic_messages">Anthropic Messages</option>
                        <option value="gemini_generate_content">Gemini Generate Content</option>
                        <option value="ollama_native">Ollama Native</option>
                    </select>
                </label>
                <label>
                    <span>모델 ID</span>
                    <input bind:value={routeModelId} required autocomplete="off" />
                </label>
                <label>
                    <span>표시 이름 (선택)</span>
                    <input bind:value={routeDisplayName} autocomplete="off" />
                </label>
                <label>
                    <span>상태</span>
                    <select bind:value={routeStatus}>
                        <option value="available">사용 가능</option>
                        <option value="missing_temporarily">일시 누락</option>
                        <option value="documented_only">문서에서만 확인</option>
                        <option value="access_denied">접근 거부</option>
                        <option value="deprecated">사용 중단 예정</option>
                        <option value="retired">지원 종료</option>
                        <option value="unknown">알 수 없음</option>
                    </select>
                </label>
                <label>
                    <span>Deployment ID (선택)</span>
                    <input bind:value={routeDeploymentId} autocomplete="off" />
                </label>
                <label>
                    <span>Region (선택)</span>
                    <input bind:value={routeRegion} autocomplete="off" />
                </label>
                <label>
                    <span>Endpoint path (선택)</span>
                    <input bind:value={routeEndpointPath} autocomplete="off" />
                </label>
                <label>
                    <span>라우트 값 JSON</span>
                    <textarea
                        class="code-field"
                        bind:value={routeValuesJson}
                        rows="4"
                        spellcheck="false"></textarea>
                </label>
                {#if routeError}
                    <p class="form-error" role="alert">{routeError}</p>
                {/if}
            </form>
        {:else}
            <form
                id="route-editor-form"
                class="resource-form"
                aria-label="모델 라우트 수정 또는 삭제"
                onsubmit={(event) => {
                    event.preventDefault();
                    void updateRoute();
                }}
            >
                <label>
                    <span>표시 이름 (선택)</span>
                    <input bind:value={updateRouteDisplayName} disabled={selectedRouteId === ''} />
                </label>
                <label>
                    <span>상태</span>
                    <select bind:value={updateRouteStatus} disabled={selectedRouteId === ''}>
                        <option value="available">사용 가능</option>
                        <option value="missing_temporarily">일시 누락</option>
                        <option value="documented_only">문서에서만 확인</option>
                        <option value="access_denied">접근 거부</option>
                        <option value="deprecated">사용 중단 예정</option>
                        <option value="retired">지원 종료</option>
                        <option value="unknown">알 수 없음</option>
                    </select>
                </label>
                {#if protectsRetainedLegacyRoute(selectedRouteId)}
                    <p class="security-note">
                        현재 기존 프로필의 모델 라우트는 프로필 연결과 함께 관리됩니다.
                    </p>
                {:else if confirmRouteDelete}
                    <p class="security-note" role="status">
                        이 라우트와 라우트에 종속된 프리셋이 함께 삭제됩니다. 하단에서 한 번 더
                        확인해 주세요.
                    </p>
                {/if}
                {#if routeError}
                    <p class="form-error" role="alert">{routeError}</p>
                {/if}
            </form>
        {/if}
    {:else}
        {#if detailMode === null}
            <div class="setting-list resource-list" aria-label="생성 프리셋 목록">
                {#if ordinaryPresets.length === 0}
                    <p class="resource-empty">아직 등록된 생성 프리셋이 없습니다.</p>
                {/if}
                {#each ordinaryPresets as preset (preset.id)}
                    <button
                        class="setting-row resource-row"
                        type="button"
                        disabled={presetBusy}
                        onclick={() => openPresetEdit(preset.id)}
                    >
                        <span class="setting-content">
                            <span class="setting-copy resource-copy">
                                <strong>{preset.display_name}</strong>
                                <small>
                                    {ordinaryRoutes.find(
                                        (route) => route.id === preset.model_route_id,
                                    )?.display_name ?? preset.model_route_id}
                                </small>
                            </span>
                        </span>
                    </button>
                {/each}
            </div>
        {:else}
            <form
                id="preset-editor-form"
                class="resource-form"
                aria-label="생성 프리셋 만들기 또는 수정"
                onsubmit={(event) => {
                    event.preventDefault();
                    void savePreset();
                }}
            >
                <label>
                    <span>모델 라우트</span>
                    <select
                        value={presetRouteId}
                        required
                        disabled={detailMode === 'edit'}
                        onchange={(event) => selectPresetRoute(event.currentTarget.value)}
                    >
                        <option value="">선택</option>
                        {#each ordinaryRoutes as route (route.id)}
                            <option value={route.id}>{route.display_name ?? route.model_id}</option>
                        {/each}
                    </select>
                </label>
                <label>
                    <span>프리셋 ID</span>
                    <input
                        bind:value={presetId}
                        required
                        readonly={detailMode === 'edit'}
                        autocomplete="off"
                    />
                </label>
                <label>
                    <span>표시 이름</span>
                    <input bind:value={presetDisplayName} required autocomplete="off" />
                </label>
                <label>
                    <span>파라미터 JSON</span>
                    <textarea
                        class="code-field code-field-tall"
                        bind:value={presetValuesJson}
                        rows="8"
                        spellcheck="false"
                        aria-describedby="preset-values-help"></textarea>
                    <small id="preset-values-help">
                        [{`{"parameter_id":"temperature","state":{"state":"explicit","value":{"type":"number","value":0.7}}}`}]
                    </small>
                </label>

                <fieldset class="field-group">
                    <legend>Reasoning</legend>
                    <label>
                        <span>Mode</span>
                        <input bind:value={reasoningMode} required autocomplete="off" />
                    </label>
                    <label>
                        <span>Effort (선택)</span>
                        <input bind:value={reasoningEffort} autocomplete="off" />
                    </label>
                    <label>
                        <span>Budget tokens (선택)</span>
                        <input bind:value={reasoningBudgetTokens} type="number" min="0" />
                    </label>
                    <label>
                        <span>Summary</span>
                        <input bind:value={reasoningSummary} required autocomplete="off" />
                    </label>
                    <label class="toggle-row">
                        <input type="checkbox" bind:checked={reasoningPreserveOpaqueState} />
                        <span>프로바이더의 opaque reasoning 상태 보존</span>
                    </label>
                </fieldset>

                <fieldset class="field-group">
                    <legend>Prompt cache</legend>
                    <label>
                        <span>Mode</span>
                        <input bind:value={promptCacheMode} required autocomplete="off" />
                    </label>
                    <label>
                        <span>TTL kind</span>
                        <input bind:value={promptCacheTtlKind} required autocomplete="off" />
                    </label>
                    <label>
                        <span>TTL seconds (선택)</span>
                        <input bind:value={promptCacheTtlSeconds} type="number" min="0" />
                    </label>
                    <label>
                        <span>Context reference (선택)</span>
                        <input bind:value={promptCacheContextReference} autocomplete="off" />
                    </label>
                </fieldset>

                <div class="editor-utilities" aria-label="프리셋 검사">
                    <button
                        type="button"
                        disabled={presetBusy}
                        onclick={() => void validatePreset()}
                    >
                        후보 검증
                    </button>
                    <button
                        type="button"
                        disabled={presetBusy}
                        onclick={() => void previewPreset()}
                    >
                        요청 구조 미리보기
                    </button>
                </div>

                {#if protectsRetainedLegacyPreset(selectedPresetId)}
                    <p class="security-note">
                        현재 기존 프로필의 기본 프리셋은 프로필 연결과 함께 관리됩니다.
                    </p>
                {:else if confirmPresetDelete}
                    <p class="security-note" role="status">
                        이 생성 프리셋이 삭제됩니다. 하단에서 한 번 더 확인해 주세요.
                    </p>
                {/if}
                {#if presetError}
                    <p class="form-error" role="alert">{presetError}</p>
                {/if}

                {#if workspace.request_preview}
                    <section class="request-preview" aria-labelledby="candidate-preview-title">
                        <h3 id="candidate-preview-title">민감값이 제거된 요청 구조</h3>
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
                        <p>메시지 본문과 자격증명 값은 표시하지 않습니다.</p>
                    </section>
                {/if}
            </form>
        {/if}
    {/if}
{/snippet}

{#snippet detailActions()}
    {#if resourcePage !== null}
        <DetailActionBar className="resource-action-bar" ariaLabel={`${detailTitle} 작업`}>
            {#if detailMode === null}
                <button
                    class="primary detail-action detail-action--wide"
                    type="button"
                    onclick={() => {
                        if (resourcePage === 'connections') openConnectionCreate();
                        else if (resourcePage === 'routes') openRouteCreate();
                        else openPresetCreate();
                    }}
                >
                    <Plus class="resource-add-icon" aria-hidden="true" />
                    {resourcePage === 'connections'
                        ? '연결 추가하기'
                        : resourcePage === 'routes'
                          ? '라우트 추가하기'
                          : '프리셋 추가하기'}
                </button>
            {:else if resourcePage === 'connections'}
                {#if detailMode === 'edit' && !selectedConnectionIsRetainedLegacy && confirmConnectionDelete}
                    <button
                        class="danger detail-action detail-action--destructive"
                        type="button"
                        disabled={connectionBusy}
                        onclick={() => void deleteConnection()}>삭제 확인</button
                    >
                    <button
                        class="detail-action detail-action--grow"
                        type="button"
                        disabled={connectionBusy}
                        onclick={() => (confirmConnectionDelete = false)}>취소</button
                    >
                {:else}
                    {#if detailMode === 'edit' && !selectedConnectionIsRetainedLegacy}
                        <button
                            class="detail-action detail-action--destructive detail-action--borderless"
                            type="button"
                            disabled={connectionBusy}
                            onclick={() => (confirmConnectionDelete = true)}>삭제</button
                        >
                    {/if}
                    <button
                        class="primary detail-action detail-action--grow"
                        type="submit"
                        form="connection-editor-form"
                        disabled={connectionBusy ||
                            (connectionTemplateId === '' && detailMode === 'create') ||
                            selectedConnectionIsRetainedLegacy}
                        >{detailMode === 'create' ? '연결 만들기' : '저장'}</button
                    >
                {/if}
            {:else if resourcePage === 'routes'}
                {#if detailMode === 'edit' && confirmRouteDelete}
                    <button
                        class="danger detail-action detail-action--destructive"
                        type="button"
                        disabled={routeBusy}
                        onclick={() => void deleteRoute()}>삭제 확인</button
                    >
                    <button
                        class="detail-action detail-action--grow"
                        type="button"
                        disabled={routeBusy}
                        onclick={() => (confirmRouteDelete = false)}>취소</button
                    >
                {:else}
                    {#if detailMode === 'edit'}
                        <button
                            class="detail-action detail-action--destructive detail-action--borderless"
                            type="button"
                            disabled={routeBusy}
                            onclick={() => (confirmRouteDelete = true)}>삭제</button
                        >
                    {/if}
                    <button
                        class="primary detail-action detail-action--grow"
                        type="submit"
                        form="route-editor-form"
                        disabled={routeBusy ||
                            (routeConnectionId === '' && detailMode === 'create')}
                        >{detailMode === 'create' ? '라우트 만들기' : '저장'}</button
                    >
                {/if}
            {:else}
                {#if detailMode === 'edit' && confirmPresetDelete}
                    <button
                        class="danger detail-action detail-action--destructive"
                        type="button"
                        disabled={presetBusy}
                        onclick={() => void deletePreset()}>삭제 확인</button
                    >
                    <button
                        class="detail-action detail-action--grow"
                        type="button"
                        disabled={presetBusy}
                        onclick={() => (confirmPresetDelete = false)}>취소</button
                    >
                {:else}
                    {#if detailMode === 'edit'}
                        <button
                            class="detail-action detail-action--destructive detail-action--borderless"
                            type="button"
                            disabled={presetBusy}
                            onclick={() => (confirmPresetDelete = true)}>삭제</button
                        >
                    {/if}
                    <button
                        class="primary detail-action detail-action--grow"
                        type="submit"
                        form="preset-editor-form"
                        disabled={presetBusy || presetRouteId === ''}
                        >{detailMode === 'create' ? '프리셋 만들기' : '저장'}</button
                    >
                {/if}
            {/if}
        </DetailActionBar>
    {/if}
{/snippet}

<DetailPage
    className="crud-panel"
    scrollClassName="provider-scroll settings-detail-scroll resource-scroll"
    ariaLabel={detailTitle}
    resetKey={`${resourcePage ?? 'index'}:${detailMode ?? 'list'}`}
    content={detailContent}
    actions={detailActions}
/>

<style>
    .resource-index,
    .resource-list {
        width: 100%;
        margin: 0;
    }

    .resource-copy {
        display: grid;
        min-width: 0;
        gap: 5px;
    }

    :global(.resource-add-icon) {
        width: 20px;
        height: 20px;
        flex: none;
        fill: none;
        stroke: currentcolor;
        stroke-linecap: round;
        stroke-linejoin: round;
        stroke-width: 1.8;
    }

    .resource-copy strong,
    .resource-copy small {
        overflow: hidden;
        font-size: var(--detail-support-type);
        font-weight: 550;
        line-height: 1.35;
        text-overflow: ellipsis;
    }

    .resource-copy strong {
        color: var(--ink);
        white-space: nowrap;
    }

    .resource-copy small {
        display: -webkit-box;
        color: var(--ink-muted);
        overflow-wrap: anywhere;
        white-space: normal;
        line-clamp: 3;
        -webkit-box-orient: vertical;
        -webkit-line-clamp: 3;
    }

    .resource-empty,
    .security-note,
    .form-error {
        padding: 12px;
        border-radius: var(--radius-md);
        margin: 0;
        color: var(--ink-muted);
        background: var(--surface-sunken);
        font-size: var(--detail-support-type);
        line-height: 1.5;
    }

    .resource-empty {
        margin: 0;
    }

    .form-error {
        color: var(--danger);
        background: var(--danger-soft);
    }

    .resource-form,
    .field-group {
        display: grid;
        gap: 14px;
    }

    .resource-form label {
        display: grid;
        gap: 7px;
        color: var(--ink-muted);
        font-size: var(--detail-support-type);
        font-weight: 700;
    }

    .resource-form :is(input, select, textarea) {
        width: 100%;
        min-width: 0;
        box-sizing: border-box;
        padding: clamp(12px, 3.432vw, 15px);
        border: 1.5px solid var(--line);
        border-radius: var(--radius-md);
        -webkit-appearance: none;
        appearance: none;
        background: color-mix(in srgb, var(--surface-sunken) 26%, var(--surface-raised));
        box-shadow: inset 0 1px 2px rgb(16 18 24 / 3%);
        caret-color: var(--accent);
        color: var(--ink);
        font: inherit;
        font-size: var(--detail-support-type);
        line-height: 1.5;
        transition:
            background-color 140ms ease,
            box-shadow 140ms ease;
    }

    .resource-form :is(input, select) {
        min-height: clamp(48px, 13.73vw, 60px);
    }

    .resource-form select {
        padding-right: 38px;
        background-image:
            linear-gradient(45deg, transparent 50%, var(--ink-muted) 50%),
            linear-gradient(135deg, var(--ink-muted) 50%, transparent 50%);
        background-position:
            calc(100% - 19px) 50%,
            calc(100% - 14px) 50%;
        background-repeat: no-repeat;
        background-size:
            5px 5px,
            5px 5px;
    }

    .resource-form textarea {
        min-height: clamp(112px, 32.037vw, 140px);
        resize: vertical;
    }

    .resource-form .code-field {
        font-family: ui-monospace, SFMono-Regular, Menlo, monospace;
        font-size: 0.8rem;
    }

    .resource-form .code-field-tall {
        min-height: 200px;
    }

    .resource-form :is(input, select, textarea):hover:not(:focus, :disabled) {
        border-color: var(--line);
    }

    .resource-form :is(input, select, textarea):focus {
        border-color: var(--accent);
        outline: none;
    }

    .resource-form :is(input, select, textarea):disabled {
        cursor: not-allowed;
        opacity: 0.55;
    }

    .resource-form small {
        color: var(--ink-muted);
        overflow-wrap: anywhere;
        font-size: 0.8em;
        font-weight: 500;
    }

    .field-group {
        min-width: 0;
        padding: 18px 0 0;
        border: 0;
        border-top: 1px solid var(--line);
        margin: 4px 0 0;
    }

    .field-group legend {
        padding: 0 0 8px;
        color: var(--ink);
        font-size: var(--detail-support-type);
        font-weight: 700;
    }

    .resource-form .toggle-row {
        display: flex;
        min-height: var(--touch);
        align-items: center;
        gap: 10px;
    }

    .resource-form .toggle-row input {
        width: 20px;
        min-height: 20px;
        flex: none;
        padding: 0;
        appearance: auto;
    }

    .editor-utilities {
        display: grid;
        grid-template-columns: repeat(2, minmax(0, 1fr));
        gap: 8px;
    }

    .editor-utilities button {
        min-height: var(--touch);
        padding: 9px 12px;
        border-radius: var(--radius-pill);
        font-size: var(--detail-support-type);
        font-weight: 700;
    }

    .request-preview {
        display: grid;
        padding-top: 18px;
        border-top: 1px solid var(--line);
        gap: 12px;
    }

    .request-preview h3,
    .request-preview p,
    .request-preview dl {
        margin: 0;
    }

    .request-preview h3 {
        font-size: var(--detail-support-type);
    }

    .request-preview dl {
        display: grid;
        gap: 10px;
    }

    .request-preview dl > div {
        display: grid;
        grid-template-columns: minmax(72px, 0.3fr) minmax(0, 1fr);
        padding-bottom: 10px;
        border-bottom: 1px solid var(--line);
        gap: 12px;
    }

    .request-preview dt,
    .request-preview p {
        color: var(--ink-muted);
    }

    .request-preview dd {
        margin: 0;
        overflow-wrap: anywhere;
        font-weight: 700;
    }

    @container view (min-width: 701px) {
        .resource-form {
            grid-template-columns: repeat(2, minmax(0, 1fr));
        }

        .resource-form
            > :is(.security-note, .form-error, .field-group, .editor-utilities, .request-preview),
        .resource-form > label:has(textarea) {
            grid-column: 1 / -1;
        }
    }
</style>
