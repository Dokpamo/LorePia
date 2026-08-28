<script lang="ts">
    import { Activity, ListChecks, Plus, SlidersHorizontal, Telescope } from '@lucide/svelte';
    import { tick } from 'svelte';
    import type { LorepiaAppController, LorepiaAppState } from '../../app/app-controller';
    import ChoiceField from '../../components/ChoiceField.svelte';
    import DetailActionBar from '../../components/detail/DetailActionBar.svelte';
    import DetailPage from '../../components/detail/DetailPage.svelte';
    import {
        CAPABILITY_KEYS,
        type CapabilityKeyInput,
        type CapabilityObservationDto,
        type CapabilityOverrideStatusInput,
        type CapabilityOverrideValueInput,
        type CapabilityValueDto,
        type UpsertCapabilityOverrideInput,
    } from '../../lib/ipc/contracts';

    type CapabilityDetailMode =
        | 'effective'
        | 'overrides'
        | 'override-create'
        | 'override-edit'
        | 'override-readonly'
        | 'observations'
        | 'parameters'
        | null;

    interface Props {
        appState: LorepiaAppState;
        controller: LorepiaAppController;
        detailMode?: string | null;
    }

    type OverrideValueKind = CapabilityOverrideValueInput['type'];

    const OVERRIDE_FORM_ID = 'capability-override-editor-form';
    const CAPABILITY_LABELS: Record<CapabilityKeyInput, string> = {
        streaming: '스트리밍',
        reasoning: '추론',
        prompt_caching: '프롬프트 캐시',
        tool_calling: '도구 호출',
        parallel_tool_calling: '병렬 도구 호출',
        structured_output: '구조화 출력',
        json_mode: 'JSON 모드',
        image_input: '이미지 입력',
        audio_input: '오디오 입력',
        audio_output: '오디오 출력',
        logprobs: '로그 확률',
        seed: '시드',
        batch: '배치',
        background: '백그라운드 실행',
        context_window: '컨텍스트 창',
        max_output_tokens: '최대 출력 토큰',
    };

    let { appState, controller, detailMode = $bindable(null) }: Props = $props();
    let selectedRouteId = $state('');
    let selectedCapabilityKey = $state<CapabilityKeyInput>('streaming');
    let overrideId = $state('');
    let overrideKey = $state<CapabilityKeyInput>('streaming');
    let overrideValueKind = $state<OverrideValueKind>('boolean');
    let booleanValue = $state(true);
    let integerValue = $state(1);
    let enumValues = $state('');
    let overrideStatus = $state<CapabilityOverrideStatusInput>('verified');
    let expiresAt = $state('');
    let busy = $state(false);
    let formError = $state<string | null>(null);
    let deleteConfirmationId = $state<string | null>(null);
    let syncedRouteKey: string | null = null;

    const workspace = $derived(appState.providers.workspace);
    const selectedRoute = $derived(
        workspace.routes.find((route) => route.id === selectedRouteId) ?? null,
    );
    const routeIsLoaded = $derived(
        selectedRouteId !== '' &&
            selectedRoute !== null &&
            workspace.selected_capability_model_route_id === selectedRouteId,
    );
    const observations = $derived(routeIsLoaded ? workspace.capability_observations : []);
    const userOverrides = $derived(
        observations.filter((observation) => observation.source === 'user_override'),
    );
    const selectedUserOverride = $derived(
        userOverrides.find((observation) => observation.id === overrideId) ?? null,
    );
    const parameterSpecs = $derived(routeIsLoaded ? workspace.capability_parameter_specs : []);
    const effectiveCapability = $derived(
        routeIsLoaded && workspace.effective_capability?.selected.key === selectedCapabilityKey
            ? workspace.effective_capability
            : null,
    );
    const hasBottomAction = $derived(
        detailMode === 'effective' ||
            detailMode === 'overrides' ||
            detailMode === 'override-create' ||
            detailMode === 'override-edit' ||
            detailMode === 'override-readonly',
    );

    $effect(() => {
        const routeId = workspace.selected_capability_model_route_id;
        const routeExists =
            routeId === null || workspace.routes.some((route) => route.id === routeId);
        const routeKey = `${routeId ?? '<none>'}:${routeExists ? 'present' : 'missing'}`;
        if (routeKey === syncedRouteKey) return;
        syncedRouteKey = routeKey;
        selectedRouteId = routeId !== null && routeExists ? routeId : '';
        resetOverrideForm();
        detailMode = null;
    });

    $effect(() => {
        if (
            detailMode === 'override-edit' ||
            detailMode === 'override-create' ||
            detailMode === 'override-readonly'
        ) {
            return;
        }
        deleteConfirmationId = null;
        formError = null;
    });

    $effect(() => {
        if (
            (detailMode !== 'override-edit' && detailMode !== 'override-readonly') ||
            overrideId === '' ||
            !routeIsLoaded ||
            userOverrides.some((observation) => observation.id === overrideId)
        ) {
            return;
        }
        resetOverrideForm();
        detailMode = 'overrides';
    });

    function isCapabilityKey(value: string): value is CapabilityKeyInput {
        return (CAPABILITY_KEYS as readonly string[]).includes(value);
    }

    function isOverrideStatus(value: string): value is CapabilityOverrideStatusInput {
        return ['verified', 'unsupported', 'unknown', 'conditional'].includes(value);
    }

    function capabilityLabel(key: string): string {
        return isCapabilityKey(key) ? CAPABILITY_LABELS[key] : key;
    }

    function statusLabel(status: string): string {
        if (status === 'verified') return '검증됨';
        if (status === 'unsupported') return '지원하지 않음';
        if (status === 'conditional') return '조건부';
        return '알 수 없음';
    }

    function sourceLabel(source: string): string {
        if (source === 'user_override') return '사용자 override';
        if (source === 'catalog') return '서명 카탈로그';
        if (source === 'provider_api') return '프로바이더 API';
        if (source === 'discovery') return '탐색';
        return source;
    }

    function formatValue(value: CapabilityValueDto): string {
        if (value.type === 'boolean') return value.value ? 'true' : 'false';
        if (value.type === 'integer') return String(value.value);
        if (value.type === 'enum_values') return value.value.join(', ');
        return JSON.stringify(value.value);
    }

    function localDateTime(value: string | null): string {
        if (value === null) return '';
        const date = new Date(value);
        if (Number.isNaN(date.getTime())) return '';
        const pad = (part: number) => String(part).padStart(2, '0');
        return `${String(date.getFullYear())}-${pad(date.getMonth() + 1)}-${pad(
            date.getDate(),
        )}T${pad(date.getHours())}:${pad(date.getMinutes())}`;
    }

    function newOverrideId(): string {
        return `capability-override-${globalThis.crypto.randomUUID()}`;
    }

    function resetOverrideForm(): void {
        overrideId = '';
        overrideKey = selectedCapabilityKey;
        overrideValueKind = 'boolean';
        booleanValue = true;
        integerValue = 1;
        enumValues = '';
        overrideStatus = 'verified';
        expiresAt = '';
        formError = null;
        deleteConfirmationId = null;
    }

    function openDetail(mode: Exclude<CapabilityDetailMode, null>): void {
        if (busy || !routeIsLoaded) return;
        deleteConfirmationId = null;
        formError = null;
        detailMode = mode;
    }

    function beginCreate(): void {
        if (busy || !routeIsLoaded) return;
        resetOverrideForm();
        detailMode = 'override-create';
    }

    function editOverride(observation: CapabilityObservationDto): void {
        if (
            busy ||
            observation.source !== 'user_override' ||
            !isCapabilityKey(observation.key) ||
            observation.value.type === 'structured'
        ) {
            return;
        }
        overrideId = observation.id;
        overrideKey = observation.key;
        selectedCapabilityKey = observation.key;
        overrideValueKind = observation.value.type;
        booleanValue = true;
        integerValue = 1;
        enumValues = '';
        if (observation.value.type === 'boolean') booleanValue = observation.value.value;
        if (observation.value.type === 'integer') integerValue = observation.value.value;
        if (observation.value.type === 'enum_values') {
            enumValues = observation.value.value.join(', ');
        }
        overrideStatus = isOverrideStatus(observation.status) ? observation.status : 'unknown';
        expiresAt = localDateTime(observation.expires_at);
        formError = null;
        deleteConfirmationId = null;
        detailMode = 'override-edit';
    }

    function viewReadOnlyOverride(observation: CapabilityObservationDto): void {
        if (busy || observation.source !== 'user_override') return;
        resetOverrideForm();
        overrideId = observation.id;
        detailMode = 'override-readonly';
    }

    function overrideValue(): CapabilityOverrideValueInput | null {
        if (overrideValueKind === 'boolean') {
            return { type: 'boolean', value: booleanValue };
        }
        if (overrideValueKind === 'integer') {
            if (!Number.isInteger(integerValue)) {
                formError = '정수 값을 입력해 주세요.';
                return null;
            }
            return { type: 'integer', value: integerValue };
        }
        const values = [
            ...new Set(
                enumValues
                    .split(/[,\n]/)
                    .map((value) => value.trim())
                    .filter((value) => value.length > 0),
            ),
        ];
        if (values.length === 0) {
            formError = '열거 값은 하나 이상 입력해 주세요.';
            return null;
        }
        return { type: 'enum_values', value: values };
    }

    async function loadRoute(routeId: string): Promise<void> {
        const previousRouteId = workspace.selected_capability_model_route_id ?? '';
        selectedRouteId = routeId;
        resetOverrideForm();
        detailMode = null;
        if (routeId === '') return;
        busy = true;
        try {
            await controller.loadProviderCapabilities(routeId);
            await tick();
            if (workspace.selected_capability_model_route_id !== routeId) {
                selectedRouteId = workspace.routes.some((route) => route.id === previousRouteId)
                    ? previousRouteId
                    : '';
            }
        } finally {
            busy = false;
        }
    }

    async function inspectCapability(): Promise<void> {
        if (!routeIsLoaded) return;
        busy = true;
        try {
            await controller.inspectEffectiveProviderCapability(selectedCapabilityKey);
        } finally {
            busy = false;
        }
    }

    async function saveOverride(): Promise<void> {
        if (!routeIsLoaded) {
            formError = '먼저 모델 라우트를 선택해 주세요.';
            return;
        }
        formError = null;
        const value = overrideValue();
        if (value === null) return;

        let expiresAtValue: string | null = null;
        if (expiresAt !== '') {
            const expiry = new Date(expiresAt);
            if (Number.isNaN(expiry.getTime())) {
                formError = '만료 시각을 확인해 주세요.';
                return;
            }
            expiresAtValue = expiry.toISOString();
        }

        const input: UpsertCapabilityOverrideInput = {
            id: overrideId === '' ? newOverrideId() : overrideId,
            model_route_id: selectedRouteId,
            key: overrideKey,
            value,
            status: overrideStatus,
            expires_at: expiresAtValue,
        };

        busy = true;
        try {
            if (await controller.upsertProviderCapabilityOverride(input)) {
                selectedCapabilityKey = input.key;
                resetOverrideForm();
                detailMode = 'overrides';
            }
        } finally {
            busy = false;
        }
    }

    async function deleteEditingOverride(): Promise<void> {
        if (!routeIsLoaded || overrideId === '' || deleteConfirmationId !== overrideId) return;
        const deletingOverrideId = overrideId;
        busy = true;
        try {
            await controller.deleteProviderCapabilityOverride(deletingOverrideId);
            const stillExists = workspace.capability_observations.some(
                (observation) => observation.id === deletingOverrideId,
            );
            if (!stillExists) {
                resetOverrideForm();
                detailMode = 'overrides';
            }
        } finally {
            busy = false;
        }
    }
</script>

{#snippet capabilityContent()}
    {#if detailMode === null}
        <div class="route-form direct-form" aria-label="Capability 모델 라우트">
            <ChoiceField
                id="capability-model-route"
                label="모델 라우트"
                value={selectedRouteId}
                options={[
                    { value: '', label: '선택' },
                    ...workspace.routes.map((route) => ({
                        value: route.id,
                        label: route.display_name ?? route.model_id,
                    })),
                ]}
                disabled={busy}
                onSelect={(value: string) => void loadRoute(value)}
            />
        </div>

        <ul class="setting-list capability-index" aria-label="Capability 영역">
            <li>
                <button
                    class="setting-row capability-destination"
                    type="button"
                    disabled={busy || !routeIsLoaded}
                    onclick={() => openDetail('effective')}
                >
                    <span class="setting-icon" aria-hidden="true"><Activity /></span>
                    <span class="setting-content destination-content">
                        <span class="setting-copy destination-copy">
                            <strong>유효 capability</strong>
                            <small>
                                {effectiveCapability
                                    ? `${capabilityLabel(effectiveCapability.selected.key)} · ${formatValue(effectiveCapability.selected.value)}`
                                    : `${CAPABILITY_LABELS[selectedCapabilityKey]} 확인`}
                            </small>
                        </span>
                    </span>
                </button>
            </li>
            <li>
                <button
                    class="setting-row capability-destination"
                    type="button"
                    disabled={busy || !routeIsLoaded}
                    onclick={() => openDetail('overrides')}
                >
                    <span class="setting-icon" aria-hidden="true"><SlidersHorizontal /></span>
                    <span class="setting-content destination-content">
                        <span class="setting-copy destination-copy">
                            <strong>사용자 override</strong>
                            <small>{userOverrides.length}개</small>
                        </span>
                    </span>
                </button>
            </li>
            <li>
                <button
                    class="setting-row capability-destination"
                    type="button"
                    disabled={busy || !routeIsLoaded}
                    onclick={() => openDetail('observations')}
                >
                    <span class="setting-icon" aria-hidden="true"><Telescope /></span>
                    <span class="setting-content destination-content">
                        <span class="setting-copy destination-copy">
                            <strong>Capability 관측</strong>
                            <small>{observations.length}개 관측</small>
                        </span>
                    </span>
                </button>
            </li>
            <li>
                <button
                    class="setting-row capability-destination"
                    type="button"
                    disabled={busy || !routeIsLoaded}
                    onclick={() => openDetail('parameters')}
                >
                    <span class="setting-icon" aria-hidden="true"><ListChecks /></span>
                    <span class="setting-content destination-content">
                        <span class="setting-copy destination-copy">
                            <strong>유효 생성 파라미터</strong>
                            <small>{parameterSpecs.length}개 파라미터</small>
                        </span>
                    </span>
                </button>
            </li>
        </ul>

        {#if !routeIsLoaded}
            <p class="empty-note">
                {selectedRouteId === ''
                    ? '모델 라우트를 선택해 주세요.'
                    : selectedRoute === null
                      ? '선택한 모델 라우트를 찾을 수 없습니다.'
                      : 'Capability 상태를 불러오는 중입니다.'}
            </p>
        {/if}
    {:else if detailMode === 'effective'}
        <div class="direct-form effective-form">
            <ChoiceField
                id="effective-capability-key"
                label="Capability 키"
                value={selectedCapabilityKey}
                options={CAPABILITY_KEYS.map((key) => ({
                    value: key,
                    label: `${CAPABILITY_LABELS[key]} · ${key}`,
                }))}
                disabled={busy || !routeIsLoaded}
                onSelect={(value: string) => (selectedCapabilityKey = value as CapabilityKeyInput)}
            />
        </div>

        {#if effectiveCapability}
            <div class="result-stack">
                <p class="group-label">선택된 값</p>
                <dl class:warning={effectiveCapability.has_conflict} class="effective-result">
                    <div class="effective-primary-row">
                        <dt>{capabilityLabel(effectiveCapability.selected.key)}</dt>
                        <dd class="effective-value">
                            {formatValue(effectiveCapability.selected.value)}
                        </dd>
                    </div>
                    <div>
                        <dt>상태</dt>
                        <dd class="badges">
                            <span>{statusLabel(effectiveCapability.selected.status)}</span>
                            {#if effectiveCapability.selected_is_stale}
                                <span class="warning-badge">만료됨</span>
                            {/if}
                            {#if effectiveCapability.has_conflict}
                                <span class="warning-badge">출처 충돌</span>
                            {/if}
                        </dd>
                    </div>
                    <div>
                        <dt>출처</dt>
                        <dd>{sourceLabel(effectiveCapability.selected.source)}</dd>
                    </div>
                    <div>
                        <dt>신뢰도</dt>
                        <dd>{effectiveCapability.selected.confidence}</dd>
                    </div>
                    <div>
                        <dt>평가 시각</dt>
                        <dd>{effectiveCapability.evaluated_at}</dd>
                    </div>
                </dl>

                {#if effectiveCapability.alternatives.length > 0}
                    <p class="group-label">다른 관측</p>
                    <ul class="read-list compact-read-list">
                        {#each effectiveCapability.alternatives as alternative (alternative.id)}
                            <li class="read-row">
                                <div class="read-row-heading">
                                    <strong>{formatValue(alternative.value)}</strong>
                                    <span>{statusLabel(alternative.status)}</span>
                                </div>
                                <small>{sourceLabel(alternative.source)}</small>
                            </li>
                        {/each}
                    </ul>
                {/if}
            </div>
        {:else}
            <p class="empty-note">키를 선택하고 유효 값을 확인해 주세요.</p>
        {/if}
    {:else if detailMode === 'overrides'}
        {#if userOverrides.length === 0}
            <p class="empty-note">저장된 사용자 override가 없습니다.</p>
        {:else}
            <ul class="setting-list override-list" aria-label="사용자 override 목록">
                {#each userOverrides as observation (observation.id)}
                    <li>
                        {#if isCapabilityKey(observation.key) && observation.value.type !== 'structured'}
                            <button
                                class="setting-row override-row"
                                type="button"
                                aria-label="이 override 수정"
                                disabled={busy}
                                onclick={() => editOverride(observation)}
                            >
                                <span class="override-row-copy">
                                    <strong>{capabilityLabel(observation.key)}</strong>
                                    <small>
                                        {formatValue(observation.value)} · {statusLabel(
                                            observation.status,
                                        )}
                                    </small>
                                </span>
                            </button>
                        {:else}
                            <button
                                class="setting-row override-row read-only-row"
                                type="button"
                                aria-label="이 override 보기"
                                disabled={busy}
                                onclick={() => viewReadOnlyOverride(observation)}
                            >
                                <span class="override-row-copy">
                                    <strong>{capabilityLabel(observation.key)}</strong>
                                    <small>
                                        {formatValue(observation.value)} · 읽기 전용
                                    </small>
                                </span>
                            </button>
                        {/if}
                    </li>
                {/each}
            </ul>
        {/if}
    {:else if detailMode === 'override-create' || detailMode === 'override-edit'}
        <form
            id={OVERRIDE_FORM_ID}
            class="direct-form override-editor"
            aria-label={detailMode === 'override-create'
                ? '사용자 override 추가'
                : '사용자 override 수정'}
            onsubmit={(event) => {
                event.preventDefault();
                void saveOverride();
            }}
        >
            <ChoiceField
                id="override-capability-key"
                label="Capability 키"
                value={overrideKey}
                options={CAPABILITY_KEYS.map((key) => ({
                    value: key,
                    label: `${CAPABILITY_LABELS[key]} · ${key}`,
                }))}
                disabled={busy || !routeIsLoaded}
                onSelect={(value: string) => (overrideKey = value as CapabilityKeyInput)}
            />
            <ChoiceField
                id="override-value-kind"
                label="값 종류"
                value={overrideValueKind}
                options={[
                    { value: 'boolean', label: 'Boolean' },
                    { value: 'integer', label: 'Integer' },
                    { value: 'enum_values', label: 'Enum 목록' },
                ]}
                disabled={busy || !routeIsLoaded}
                onSelect={(value: string) => (overrideValueKind = value as OverrideValueKind)}
            />

            {#if overrideValueKind === 'boolean'}
                <ChoiceField
                    id="override-boolean-value"
                    label="Boolean 값"
                    value={String(booleanValue)}
                    options={[
                        { value: 'true', label: 'true' },
                        { value: 'false', label: 'false' },
                    ]}
                    disabled={busy || !routeIsLoaded}
                    onSelect={(value: string) => (booleanValue = value === 'true')}
                />
            {:else if overrideValueKind === 'integer'}
                <label>
                    <span>정수 값</span>
                    <input
                        type="number"
                        step="1"
                        bind:value={integerValue}
                        disabled={busy || !routeIsLoaded}
                    />
                </label>
            {:else}
                <label>
                    <span>열거 값</span>
                    <textarea
                        rows="3"
                        bind:value={enumValues}
                        disabled={busy || !routeIsLoaded}
                        placeholder="값을 쉼표 또는 줄바꿈으로 구분"
                        required></textarea>
                </label>
            {/if}

            <ChoiceField
                id="override-status"
                label="상태"
                value={overrideStatus}
                options={[
                    { value: 'verified', label: '검증됨' },
                    { value: 'unsupported', label: '지원하지 않음' },
                    { value: 'unknown', label: '알 수 없음' },
                    { value: 'conditional', label: '조건부' },
                ]}
                disabled={busy || !routeIsLoaded}
                onSelect={(value: string) =>
                    (overrideStatus = value as CapabilityOverrideStatusInput)}
            />
            <label>
                <span>만료 시각 (선택)</span>
                <input
                    type="text"
                    bind:value={expiresAt}
                    disabled={busy || !routeIsLoaded}
                    placeholder="2026-08-27T22:30"
                    inputmode="text"
                />
            </label>

            {#if formError}
                <p class="form-error" role="alert">{formError}</p>
            {/if}
        </form>
    {:else if detailMode === 'override-readonly'}
        {#if selectedUserOverride}
            <ul class="read-list" aria-label="읽기 전용 사용자 override">
                <li class="read-row">
                    <div class="read-row-heading">
                        <strong>{capabilityLabel(selectedUserOverride.key)}</strong>
                        <span>읽기 전용</span>
                    </div>
                    <code>{selectedUserOverride.key}</code>
                    <p class="read-only-value">{formatValue(selectedUserOverride.value)}</p>
                    <dl class="metadata-grid">
                        <div>
                            <dt>상태</dt>
                            <dd>{statusLabel(selectedUserOverride.status)}</dd>
                        </div>
                        <div>
                            <dt>출처</dt>
                            <dd>{sourceLabel(selectedUserOverride.source)}</dd>
                        </div>
                        <div>
                            <dt>신뢰도</dt>
                            <dd>{selectedUserOverride.confidence}</dd>
                        </div>
                        <div>
                            <dt>만료</dt>
                            <dd>{selectedUserOverride.expires_at ?? '없음'}</dd>
                        </div>
                    </dl>
                </li>
            </ul>
        {:else}
            <p class="empty-note">이 사용자 override를 찾을 수 없습니다.</p>
        {/if}
    {:else if detailMode === 'observations'}
        {#if observations.length === 0}
            <p class="empty-note">저장된 capability 관측이 없습니다.</p>
        {:else}
            <ul class="read-list" aria-label="Capability 관측 목록">
                {#each observations as observation (observation.id)}
                    <li class="read-row">
                        <div class="read-row-heading">
                            <strong>{capabilityLabel(observation.key)}</strong>
                            <span>{formatValue(observation.value)}</span>
                        </div>
                        <code>{observation.key}</code>
                        <dl class="metadata-grid">
                            <div>
                                <dt>상태</dt>
                                <dd>{statusLabel(observation.status)}</dd>
                            </div>
                            <div>
                                <dt>출처</dt>
                                <dd>{sourceLabel(observation.source)}</dd>
                            </div>
                            <div>
                                <dt>신뢰도</dt>
                                <dd>{observation.confidence}</dd>
                            </div>
                            <div>
                                <dt>만료</dt>
                                <dd>{observation.expires_at ?? '없음'}</dd>
                            </div>
                        </dl>
                    </li>
                {/each}
            </ul>
        {/if}
    {:else if detailMode === 'parameters'}
        {#if parameterSpecs.length === 0}
            <p class="empty-note">이 라우트에서 사용할 수 있는 파라미터가 없습니다.</p>
        {:else}
            <ul class="read-list" aria-label="유효 생성 파라미터 목록">
                {#each parameterSpecs as spec (spec.id)}
                    <li class="read-row">
                        <div class="read-row-heading">
                            <strong>{spec.label_key}</strong>
                            <span>{spec.value_type}</span>
                        </div>
                        <code>{spec.id}</code>
                        <dl class="metadata-grid">
                            <div>
                                <dt>범위</dt>
                                <dd>
                                    {spec.minimum ?? '제한 없음'} – {spec.maximum ?? '제한 없음'}
                                </dd>
                            </div>
                            <div>
                                <dt>기본 모드</dt>
                                <dd>{spec.default_mode}</dd>
                            </div>
                            <div>
                                <dt>전송 필드</dt>
                                <dd>{spec.provider_mapping.field_name}</dd>
                            </div>
                        </dl>
                        {#if spec.description_key}
                            <small>{spec.description_key}</small>
                        {/if}
                    </li>
                {/each}
            </ul>
        {/if}
    {/if}
{/snippet}

{#snippet capabilityActions()}
    {#if detailMode === 'effective'}
        <DetailActionBar ariaLabel="유효 capability 작업">
            <button
                class="primary detail-action detail-action--wide"
                type="button"
                aria-label="유효 값 확인"
                disabled={busy || !routeIsLoaded}
                onclick={() => void inspectCapability()}
            >
                확인
            </button>
        </DetailActionBar>
    {:else if detailMode === 'overrides'}
        <DetailActionBar ariaLabel="사용자 override 작업">
            <button
                class="primary detail-action detail-action--wide"
                type="button"
                disabled={busy || !routeIsLoaded}
                onclick={beginCreate}
            >
                <Plus aria-hidden="true" />
                사용자 override 추가
            </button>
        </DetailActionBar>
    {:else if detailMode === 'override-create'}
        <DetailActionBar ariaLabel="사용자 override 편집 작업">
            <button
                class="primary detail-action detail-action--wide"
                type="submit"
                form={OVERRIDE_FORM_ID}
                aria-label="사용자 override 저장"
                disabled={busy || !routeIsLoaded}
            >
                저장
            </button>
        </DetailActionBar>
    {:else if detailMode === 'override-edit'}
        <DetailActionBar ariaLabel="사용자 override 편집 작업">
            {#if deleteConfirmationId === overrideId}
                <button
                    class="danger detail-action detail-action--destructive detail-action--borderless delete-confirm"
                    type="button"
                    aria-label="사용자 override 삭제 확인"
                    disabled={busy || !routeIsLoaded}
                    onclick={() => void deleteEditingOverride()}
                >
                    정말 삭제
                </button>
                <button
                    class="detail-action detail-action--grow cancel-action"
                    type="button"
                    disabled={busy}
                    onclick={() => (deleteConfirmationId = null)}
                >
                    취소
                </button>
            {:else}
                <button
                    class="detail-action detail-action--destructive detail-action--borderless delete-action"
                    type="button"
                    aria-label="사용자 override 삭제"
                    disabled={busy || !routeIsLoaded || overrideId === ''}
                    onclick={() => (deleteConfirmationId = overrideId)}
                >
                    삭제
                </button>
                <button
                    class="primary detail-action detail-action--grow save-action"
                    type="submit"
                    form={OVERRIDE_FORM_ID}
                    aria-label="사용자 override 업데이트"
                    disabled={busy || !routeIsLoaded}
                >
                    저장
                </button>
            {/if}
        </DetailActionBar>
    {:else if detailMode === 'override-readonly'}
        <DetailActionBar ariaLabel="읽기 전용 사용자 override 작업">
            {#if deleteConfirmationId === overrideId}
                <button
                    class="danger detail-action detail-action--destructive detail-action--borderless delete-confirm"
                    type="button"
                    aria-label="사용자 override 삭제 확인"
                    disabled={busy || !routeIsLoaded || selectedUserOverride === null}
                    onclick={() => void deleteEditingOverride()}
                >
                    정말 삭제
                </button>
                <button
                    class="detail-action detail-action--grow cancel-action"
                    type="button"
                    disabled={busy}
                    onclick={() => (deleteConfirmationId = null)}
                >
                    취소
                </button>
            {:else}
                <button
                    class="detail-action detail-action--destructive detail-action--borderless detail-action--wide delete-action"
                    type="button"
                    aria-label="사용자 override 삭제"
                    disabled={busy || !routeIsLoaded || selectedUserOverride === null}
                    onclick={() => (deleteConfirmationId = overrideId)}
                >
                    삭제
                </button>
            {/if}
        </DetailActionBar>
    {/if}
{/snippet}

<DetailPage
    ariaLabel="모델 capability"
    className="capability-panel"
    scrollClassName="provider-scroll settings-detail-scroll capability-scroll"
    resetKey={detailMode ?? 'index'}
    hasActions={hasBottomAction}
    content={capabilityContent}
    actions={capabilityActions}
/>

<style>
    :global(.capability-scroll) {
        overscroll-behavior: contain;
    }

    .direct-form {
        display: grid;
        gap: 14px;
    }

    .direct-form label {
        display: grid;
        min-width: 0;
        gap: 7px;
        color: var(--ink-muted);
        font-size: var(--detail-support-type);
        font-weight: 700;
    }

    .direct-form :is(input, textarea) {
        width: 100%;
        min-width: 0;
        min-height: clamp(48px, 13.73vw, 60px);
        box-sizing: border-box;
        padding: clamp(12px, 3.432vw, 15px);
        border: 1.5px solid var(--line);
        border-radius: var(--radius-md);
        appearance: none;
        background: color-mix(in srgb, var(--surface-sunken) 26%, var(--surface-raised));
        box-shadow: var(--control-inset-shadow);
        caret-color: var(--accent);
        color: var(--ink);
        font: inherit;
        line-height: 1.5;
        transition:
            background-color 140ms ease,
            box-shadow 140ms ease;
    }

    .direct-form textarea {
        min-height: clamp(112px, 32.037vw, 140px);
        resize: vertical;
    }

    .direct-form :is(input, textarea):hover:not(:focus, :disabled) {
        border-color: var(--line);
    }

    .direct-form :is(input, textarea):focus {
        border-color: var(--accent);
        outline: none;
    }

    .direct-form :is(input, textarea):disabled {
        cursor: not-allowed;
        opacity: var(--disabled-opacity);
    }

    .route-form {
        width: 100%;
    }

    .detail-action :global(svg) {
        width: 19px;
        height: 19px;
        flex: none;
        fill: none;
        stroke: currentcolor;
        stroke-linecap: round;
        stroke-linejoin: round;
        stroke-width: 1.8;
    }

    .capability-index,
    .override-list {
        width: 100%;
        margin: 0;
    }

    .capability-destination,
    .override-row {
        min-height: clamp(60px, 17.162vw, 75px);
    }

    .destination-content,
    .destination-copy,
    .override-row-copy {
        display: grid;
        min-width: 0;
        flex: 1;
        gap: 5px;
    }

    .destination-copy strong,
    .destination-copy small,
    .override-row-copy strong,
    .override-row-copy small {
        overflow: hidden;
        font-size: var(--detail-support-type);
        line-height: 1.35;
        text-overflow: ellipsis;
    }

    .destination-copy strong,
    .override-row-copy strong {
        color: var(--ink);
        font-weight: 600;
    }

    .destination-copy small,
    .override-row-copy small {
        color: var(--ink-muted);
        font-weight: 500;
        white-space: normal;
    }

    .read-only-row {
        cursor: default;
        opacity: var(--read-only-opacity);
    }

    .empty-note {
        padding: 12px 0;
        border-radius: 0;
        margin: 0;
        background: transparent;
        color: var(--ink-muted);
        font-size: var(--detail-support-type);
        line-height: 1.5;
    }

    .result-stack {
        display: grid;
        gap: 10px;
    }

    .group-label {
        margin: 4px 0 0;
        color: var(--ink-muted);
        font-size: var(--detail-support-type);
        font-weight: 700;
    }

    .effective-result {
        display: grid;
        padding: 0;
        border-block: 1px solid var(--line);
        margin: 0;
        background: transparent;
    }

    .effective-result.warning {
        border-color: var(--status-warning-border);
        background: var(--status-warning-bg);
    }

    .effective-result > div {
        display: grid;
        min-width: 0;
        grid-template-columns: minmax(0, 1fr) minmax(0, 1.6fr);
        align-items: baseline;
        padding: clamp(12px, 3.432vw, 15px) 0;
        border-bottom: 1px solid var(--line);
        gap: 14px;
    }

    .effective-result > div:last-child {
        border-bottom: 0;
    }

    .effective-result dt {
        color: var(--ink-muted);
        font-size: var(--detail-support-type);
        font-weight: 650;
    }

    .effective-result dd {
        min-width: 0;
        margin: 0;
        overflow-wrap: anywhere;
        color: var(--ink);
        font-size: var(--detail-support-type);
        font-weight: 650;
    }

    .read-row-heading {
        display: flex;
        min-width: 0;
        align-items: baseline;
        justify-content: space-between;
        gap: 12px;
    }

    .read-row-heading strong {
        min-width: 0;
        overflow-wrap: anywhere;
        color: var(--ink);
        font-size: var(--detail-support-type);
        font-weight: 650;
    }

    .effective-value {
        overflow-wrap: anywhere;
        font-size: clamp(18px, 4.8vw, 22px) !important;
        font-weight: 750 !important;
    }

    .badges {
        display: flex;
        flex-wrap: wrap;
        justify-content: flex-end;
        gap: 5px;
    }

    .badges span {
        padding: 0;
        border-radius: 0;
        background: transparent;
        color: var(--accent);
        font-size: 0.72rem;
        font-weight: 750;
    }

    .badges .warning-badge {
        background: transparent;
        color: var(--status-warning-fg);
    }

    .read-list {
        display: flex;
        flex-direction: column;
        padding: 0;
        border-top: 1px solid var(--line);
        border-radius: 0;
        margin: 0;
        background: transparent;
        box-shadow: none;
        gap: 0;
        list-style: none;
        overflow: visible;
    }

    .read-row {
        display: grid;
        min-width: 0;
        padding: clamp(14px, 4vw, 18px) 0;
        border-bottom: 1px solid var(--line);
        border-radius: 0;
        background: transparent;
        gap: 8px;
    }

    .read-row-heading span,
    .read-row > small,
    .read-row > code {
        overflow-wrap: anywhere;
        color: var(--ink-muted);
        font-size: 0.78rem;
    }

    .read-only-value {
        margin: 0;
        overflow-wrap: anywhere;
        color: var(--ink);
        font-size: var(--detail-support-type);
        line-height: 1.5;
    }

    .compact-read-list .read-row {
        min-height: 52px;
    }

    .metadata-grid {
        display: grid;
        grid-template-columns: repeat(3, minmax(0, 1fr));
        gap: 10px;
        margin: 0;
    }

    .metadata-grid > div {
        min-width: 0;
    }

    .metadata-grid dt {
        color: var(--ink-muted);
        font-size: 0.7rem;
    }

    .metadata-grid dd {
        margin: 3px 0 0;
        overflow-wrap: anywhere;
        color: var(--ink);
        font-size: 0.78rem;
        font-weight: 650;
    }

    .form-error {
        padding: 10px 12px;
        border: 1px solid var(--status-error-border);
        border-radius: var(--radius-sm);
        margin: 0;
        color: var(--status-error-fg);
        background: var(--status-error-bg);
        font-size: var(--detail-support-type);
        font-weight: 700;
    }

    @container view (max-width: 520px) {
        .metadata-grid {
            grid-template-columns: repeat(2, minmax(0, 1fr));
        }
    }

    :global(.app-shell[data-layout='mobile'] .capability-scroll) {
        scrollbar-width: none;
    }

    :global(.app-shell[data-layout='mobile'] .capability-scroll::-webkit-scrollbar) {
        display: none;
    }
</style>
