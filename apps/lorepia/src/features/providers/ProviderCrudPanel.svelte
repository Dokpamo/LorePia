<script lang="ts">
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
    }

    let { appState, controller }: Props = $props();

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
    const selectedConnectionIsRetainedLegacy = $derived(
        retainedLegacyProfileIds.has(selectedConnectionId),
    );
    const selectedTemplate = $derived(
        workspace.templates.find((template) => template.id === connectionTemplateId) ?? null,
    );
    const presetsForSelectedRoute = $derived(
        workspace.presets.filter((preset) => preset.model_route_id === presetRouteId),
    );

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

    function optionalNonNegativeInteger(value: string, label: string): number | null {
        if (value.trim() === '') return null;
        const parsed = Number(value);
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
                .filter((field) => field.required)
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
        connectionId = '';
        connectionDisplayName = '';
        connectionBasePath = '';
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

            const input: CreateProviderConnectionInput = {
                id: connectionId.trim(),
                template_id: selectedTemplate.id,
                template_version: selectedTemplate.manifest_version,
                display_name: connectionDisplayName.trim(),
                api_origin: connectionOrigin.trim(),
                api_base_path: optionalText(connectionBasePath),
                network_mode: connectionNetworkMode,
                local_network_approval: localNetworkApproval,
                values: parseConfigValues(connectionValuesJson, '연결 값'),
                approved_credential_origin: optionalText(connectionApprovedCredentialOrigin),
                timeout_seconds: timeoutSeconds,
            };
            const created = await controller.createProviderConnection(input);
            if (created) resetConnectionCreateForm();
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
            await controller.updateProviderConnection(input);
        } catch (error: unknown) {
            connectionError = error instanceof Error ? error.message : '연결 입력을 확인해 주세요.';
        } finally {
            connectionBusy = false;
        }
    }

    async function deleteConnection(): Promise<void> {
        if (selectedConnectionId === '' || !confirmConnectionDelete) return;
        connectionBusy = true;
        try {
            if (await controller.deleteProviderConnection(selectedConnectionId)) {
                selectConnection('');
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
            await controller.upsertProviderModelRoute(input);
        } finally {
            routeBusy = false;
        }
    }

    async function deleteRoute(): Promise<void> {
        if (selectedRouteId === '' || !confirmRouteDelete) return;
        routeBusy = true;
        try {
            if (await controller.deleteProviderModelRoute(selectedRouteId)) {
                selectRoute('');
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
            await controller.upsertProviderGenerationPreset(candidate);
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
        if (selectedPresetId === '' || !confirmPresetDelete) return;
        presetBusy = true;
        try {
            if (await controller.deleteProviderGenerationPreset(selectedPresetId)) {
                clearPresetForm();
            }
        } finally {
            presetBusy = false;
        }
    }
</script>

<section class="crud-panel" aria-labelledby="provider-crud-title">
    <header class="panel-heading">
        <div>
            <p class="eyebrow">Direct Core configuration</p>
            <h2 id="provider-crud-title">연결·라우트·프리셋 직접 관리</h2>
            <p>
                입력은 고수준 Tauri 명령으로만 전달됩니다. 자격증명은 이 화면에 보관하지 않습니다.
            </p>
        </div>
    </header>

    <details open>
        <summary>프로바이더 연결</summary>
        <div class="detail-body">
            <form
                class="form-grid"
                aria-label="프로바이더 연결 만들기"
                onsubmit={(event) => {
                    event.preventDefault();
                    void createConnection();
                }}
            >
                <h3 class="wide">새 연결</h3>
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
                    <label class="wide">
                        <span>승인 주소 (줄바꿈 또는 쉼표 구분)</span>
                        <textarea
                            bind:value={connectionLocalAddresses}
                            rows="2"
                            required
                            spellcheck="false"></textarea>
                    </label>
                {/if}
                <label class="wide">
                    <span>연결 값 JSON</span>
                    <textarea
                        bind:value={connectionValuesJson}
                        rows="5"
                        spellcheck="false"
                        aria-describedby="connection-values-help"></textarea>
                    <small id="connection-values-help">
                        [{`{"key":"organization","value":{"type":"text","value":"..."}}`}]
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
                <p class="wide">
                    연결을 만든 뒤 자격증명 카드에서 운영체제 네이티브 캡처를 사용하세요. 자격증명은
                    이 화면이나 WebView 메모리에 들어오지 않습니다.
                </p>
                <div class="wide actions">
                    <button
                        class="primary"
                        type="submit"
                        disabled={connectionBusy || connectionTemplateId === ''}
                    >
                        연결 만들기
                    </button>
                </div>
            </form>

            <form
                class="form-grid divided"
                aria-label="프로바이더 연결 수정 또는 삭제"
                onsubmit={(event) => {
                    event.preventDefault();
                    void updateConnection();
                }}
            >
                <h3 class="wide">기존 연결 수정</h3>
                <label class="wide">
                    <span>연결 선택</span>
                    <select
                        value={selectedConnectionId}
                        onchange={(event) => selectConnection(event.currentTarget.value)}
                    >
                        <option value="">선택</option>
                        {#each workspace.connections as connection (connection.id)}
                            <option value={connection.id}>
                                {connection.display_name} · {connection.id}
                            </option>
                        {/each}
                    </select>
                </label>
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
                <div class="wide actions">
                    <button
                        type="submit"
                        disabled={connectionBusy ||
                            selectedConnectionId === '' ||
                            selectedConnectionIsRetainedLegacy}
                    >
                        연결 수정
                    </button>
                </div>
                {#if selectedConnectionIsRetainedLegacy}
                    <p class="wide inline-note">
                        기존 프로필 연결의 메타데이터는 기존 프로필 경로에서 관리됩니다.
                    </p>
                {/if}
                <label class="wide confirm-row">
                    <input
                        type="checkbox"
                        bind:checked={confirmConnectionDelete}
                        disabled={selectedConnectionId === ''}
                    />
                    <span>선택한 연결과 그 종속 설정의 삭제를 확인합니다.</span>
                </label>
                <div class="wide actions">
                    <button
                        class="danger"
                        type="button"
                        disabled={connectionBusy ||
                            selectedConnectionId === '' ||
                            !confirmConnectionDelete}
                        onclick={() => void deleteConnection()}
                    >
                        선택한 연결 삭제
                    </button>
                </div>
            </form>
            {#if connectionError}
                <p class="form-error" role="alert">{connectionError}</p>
            {/if}
        </div>
    </details>

    <details>
        <summary>모델 라우트</summary>
        <div class="detail-body">
            <form
                class="form-grid"
                aria-label="모델 라우트 만들기"
                onsubmit={(event) => {
                    event.preventDefault();
                    void createRoute();
                }}
            >
                <h3 class="wide">새 모델 라우트</h3>
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
                <label class="wide">
                    <span>Endpoint path (선택)</span>
                    <input bind:value={routeEndpointPath} autocomplete="off" />
                </label>
                <label class="wide">
                    <span>라우트 값 JSON</span>
                    <textarea bind:value={routeValuesJson} rows="4" spellcheck="false"></textarea>
                </label>
                <div class="wide actions">
                    <button
                        class="primary"
                        type="submit"
                        disabled={routeBusy || routeConnectionId === ''}
                    >
                        라우트 만들기
                    </button>
                </div>
            </form>

            <form
                class="form-grid divided"
                aria-label="모델 라우트 수정 또는 삭제"
                onsubmit={(event) => {
                    event.preventDefault();
                    void updateRoute();
                }}
            >
                <h3 class="wide">기존 라우트 수정</h3>
                <label class="wide">
                    <span>라우트 선택</span>
                    <select
                        value={selectedRouteId}
                        onchange={(event) => selectRoute(event.currentTarget.value)}
                    >
                        <option value="">선택</option>
                        {#each ordinaryRoutes as route (route.id)}
                            <option value={route.id}>
                                {route.display_name ?? route.model_id} · {route.id}
                            </option>
                        {/each}
                    </select>
                </label>
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
                <div class="wide actions">
                    <button type="submit" disabled={routeBusy || selectedRouteId === ''}>
                        라우트 수정
                    </button>
                </div>
                {#if protectsRetainedLegacyRoute(selectedRouteId)}
                    <p class="wide inline-note">
                        현재 기존 프로필의 모델 라우트는 프로필 연결과 함께 관리됩니다.
                    </p>
                {:else}
                    <label class="wide confirm-row">
                        <input
                            type="checkbox"
                            bind:checked={confirmRouteDelete}
                            disabled={selectedRouteId === ''}
                        />
                        <span>선택한 라우트와 그 종속 프리셋의 삭제를 확인합니다.</span>
                    </label>
                    <div class="wide actions">
                        <button
                            class="danger"
                            type="button"
                            disabled={routeBusy || selectedRouteId === '' || !confirmRouteDelete}
                            onclick={() => void deleteRoute()}
                        >
                            선택한 라우트 삭제
                        </button>
                    </div>
                {/if}
            </form>
            {#if routeError}
                <p class="form-error" role="alert">{routeError}</p>
            {/if}
        </div>
    </details>

    <details>
        <summary>생성 프리셋</summary>
        <div class="detail-body">
            <form
                class="form-grid"
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
                        onchange={(event) => selectPresetRoute(event.currentTarget.value)}
                    >
                        <option value="">선택</option>
                        {#each ordinaryRoutes as route (route.id)}
                            <option value={route.id}>{route.display_name ?? route.model_id}</option>
                        {/each}
                    </select>
                </label>
                <label>
                    <span>기존 프리셋 (선택)</span>
                    <select
                        value={selectedPresetId}
                        disabled={presetRouteId === ''}
                        onchange={(event) => selectPreset(event.currentTarget.value)}
                    >
                        <option value="">새 프리셋</option>
                        {#each presetsForSelectedRoute as preset (preset.id)}
                            <option value={preset.id}>{preset.display_name}</option>
                        {/each}
                    </select>
                </label>
                <label>
                    <span>프리셋 ID</span>
                    <input
                        bind:value={presetId}
                        required
                        readonly={selectedPresetId !== ''}
                        autocomplete="off"
                    />
                </label>
                <label>
                    <span>표시 이름</span>
                    <input bind:value={presetDisplayName} required autocomplete="off" />
                </label>
                <label class="wide">
                    <span>파라미터 JSON</span>
                    <textarea
                        bind:value={presetValuesJson}
                        rows="8"
                        spellcheck="false"
                        aria-describedby="preset-values-help"></textarea>
                    <small id="preset-values-help">
                        [{`{"parameter_id":"temperature","state":{"state":"explicit","value":{"type":"number","value":0.7}}}`}]
                    </small>
                </label>

                <fieldset class="wide nested-grid">
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
                    <label class="wide confirm-row">
                        <input type="checkbox" bind:checked={reasoningPreserveOpaqueState} />
                        <span>프로바이더의 opaque reasoning 상태 보존</span>
                    </label>
                </fieldset>

                <fieldset class="wide nested-grid">
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

                <div class="wide actions">
                    <button class="primary" type="submit" disabled={presetBusy}>
                        {selectedPresetId === '' ? '프리셋 만들기' : '프리셋 수정'}
                    </button>
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

                {#if selectedPresetId !== ''}
                    {#if protectsRetainedLegacyPreset(selectedPresetId)}
                        <p class="wide inline-note">
                            현재 기존 프로필의 기본 프리셋은 프로필 연결과 함께 관리됩니다.
                        </p>
                    {:else}
                        <label class="wide confirm-row">
                            <input type="checkbox" bind:checked={confirmPresetDelete} />
                            <span>선택한 생성 프리셋의 삭제를 확인합니다.</span>
                        </label>
                        <div class="wide actions">
                            <button
                                class="danger"
                                type="button"
                                disabled={presetBusy || !confirmPresetDelete}
                                onclick={() => void deletePreset()}
                            >
                                선택한 프리셋 삭제
                            </button>
                        </div>
                    {/if}
                {/if}
            </form>
            {#if presetError}
                <p class="form-error" role="alert">{presetError}</p>
            {/if}

            {#if workspace.request_preview}
                <article class="preview-card" aria-labelledby="candidate-preview-title">
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
                            <dd>{workspace.request_preview.header_names.join(', ') || '없음'}</dd>
                        </div>
                    </dl>
                    <p>메시지 본문과 자격증명 값은 표시하지 않습니다.</p>
                </article>
            {/if}
        </div>
    </details>
</section>

<style>
    .crud-panel {
        display: grid;
        gap: 12px;
        padding: 20px;
        border: 1px solid var(--line);
        border-radius: var(--radius-md);
        background: var(--surface-raised);
    }

    .panel-heading h2,
    .form-grid h3,
    .preview-card h3 {
        margin: 3px 0;
    }

    .panel-heading p:last-child,
    .preview-card p {
        margin: 5px 0 0;
        color: var(--ink-muted);
        line-height: 1.45;
    }

    details {
        margin: 0;
        border: 1px solid var(--line);
        border-radius: 14px;
        background: var(--surface);
    }

    summary {
        min-height: var(--touch);
        padding: 13px 15px;
        cursor: pointer;
        font-weight: 800;
    }

    .detail-body {
        padding: 0 15px 15px;
    }

    .form-grid,
    .nested-grid {
        display: grid;
        grid-template-columns: repeat(2, minmax(0, 1fr));
        gap: 12px;
    }

    .divided {
        margin-top: 20px;
        padding-top: 20px;
        border-top: 1px solid var(--line);
    }

    .wide {
        grid-column: 1 / -1;
    }

    label {
        display: grid;
        gap: 6px;
        color: var(--ink-muted);
        font-size: 0.78rem;
        font-weight: 700;
    }

    input,
    select,
    textarea {
        width: 100%;
        min-height: var(--touch);
        padding: 9px 11px;
        border: 1px solid var(--line);
        border-radius: 10px;
        color: var(--ink);
        background: var(--surface-raised);
    }

    textarea {
        resize: vertical;
        font-family: ui-monospace, SFMono-Regular, Menlo, monospace;
        font-size: 0.78rem;
    }

    small {
        color: var(--ink-muted);
        overflow-wrap: anywhere;
        font-weight: 500;
    }

    fieldset {
        min-width: 0;
        margin: 0;
        padding: 14px;
        border: 1px solid var(--line);
        border-radius: 12px;
    }

    legend {
        padding: 0 5px;
        font-weight: 800;
    }

    .confirm-row {
        display: flex;
        gap: 9px;
        align-items: center;
        min-height: var(--touch);
        padding: 8px 10px;
        border-radius: 10px;
        background: var(--surface-sunken);
    }

    .confirm-row input {
        width: 18px;
        min-height: 18px;
        margin: 0;
    }

    .actions {
        display: flex;
        flex-wrap: wrap;
        gap: 8px;
    }

    .actions button {
        padding-inline: 14px;
    }

    .form-error {
        margin: 12px 0 0;
        padding: 10px 12px;
        border-radius: 10px;
        color: var(--danger);
        background: var(--danger-soft);
    }

    .preview-card {
        margin-top: 16px;
        padding: 14px;
        border: 1px solid var(--line);
        border-radius: 12px;
        background: var(--surface-sunken);
    }

    .preview-card dl {
        display: grid;
        grid-template-columns: repeat(2, minmax(0, 1fr));
        gap: 8px;
        margin: 12px 0 0;
    }

    .preview-card dl > div {
        min-width: 0;
        padding: 9px;
        border-radius: 9px;
        background: var(--surface-raised);
    }

    dt {
        color: var(--ink-muted);
        font-size: 0.7rem;
    }

    dd {
        margin: 3px 0 0;
        overflow-wrap: anywhere;
        font-weight: 800;
    }

    @media (max-width: 700px) {
        .form-grid,
        .nested-grid,
        .preview-card dl {
            grid-template-columns: 1fr;
        }

        .wide {
            grid-column: auto;
        }
    }
</style>
