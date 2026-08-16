<script lang="ts">
    import type { LorepiaAppController, LorepiaAppState } from '../../app/app-controller';
    import {
        CAPABILITY_KEYS,
        type CapabilityKeyInput,
        type CapabilityObservationDto,
        type CapabilityOverrideStatusInput,
        type CapabilityOverrideValueInput,
        type CapabilityValueDto,
        type UpsertCapabilityOverrideInput,
    } from '../../lib/ipc/contracts';

    interface Props {
        appState: LorepiaAppState;
        controller: LorepiaAppController;
    }

    type OverrideValueKind = CapabilityOverrideValueInput['type'];

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

    let { appState, controller }: Props = $props();
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
    let syncedRouteId: string | null = null;

    const workspace = $derived(appState.providers.workspace);
    const effectiveCapability = $derived(
        workspace.effective_capability?.selected.key === selectedCapabilityKey
            ? workspace.effective_capability
            : null,
    );

    $effect(() => {
        const routeId = workspace.selected_capability_model_route_id;
        if (routeId === syncedRouteId) return;
        syncedRouteId = routeId;
        selectedRouteId = routeId ?? '';
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
    }

    function editOverride(observation: CapabilityObservationDto): void {
        if (
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
        if (observation.value.type === 'boolean') booleanValue = observation.value.value;
        if (observation.value.type === 'integer') integerValue = observation.value.value;
        if (observation.value.type === 'enum_values') {
            enumValues = observation.value.value.join(', ');
        }
        overrideStatus = isOverrideStatus(observation.status) ? observation.status : 'unknown';
        expiresAt = localDateTime(observation.expires_at);
        formError = null;
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
        selectedRouteId = routeId;
        resetOverrideForm();
        if (routeId === '') return;
        busy = true;
        try {
            await controller.loadProviderCapabilities(routeId);
        } finally {
            busy = false;
        }
    }

    async function inspectCapability(): Promise<void> {
        if (selectedRouteId === '') return;
        busy = true;
        try {
            await controller.inspectEffectiveProviderCapability(selectedCapabilityKey);
        } finally {
            busy = false;
        }
    }

    async function saveOverride(): Promise<void> {
        if (selectedRouteId === '') {
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
            }
        } finally {
            busy = false;
        }
    }

    async function deleteOverride(observation: CapabilityObservationDto): Promise<void> {
        if (observation.source !== 'user_override') return;
        busy = true;
        try {
            await controller.deleteProviderCapabilityOverride(observation.id);
        } finally {
            busy = false;
        }
    }
</script>

<section class="capability-panel" aria-labelledby="capability-title">
    <header class="panel-heading">
        <div>
            <p class="eyebrow">Effective provider contract</p>
            <h2 id="capability-title">모델 capability</h2>
            <p>
                관측 출처와 충돌을 확인하고, 필요한 경우에만 만료 가능한 사용자 override를
                저장합니다.
            </p>
        </div>
    </header>

    <div class="route-row">
        <label>
            <span>모델 라우트</span>
            <select
                value={selectedRouteId}
                disabled={busy}
                onchange={(event) => void loadRoute(event.currentTarget.value)}
            >
                <option value="">선택</option>
                {#each workspace.routes as route (route.id)}
                    <option value={route.id}>{route.display_name ?? route.model_id}</option>
                {/each}
            </select>
        </label>
        <button
            type="button"
            disabled={busy || selectedRouteId === ''}
            onclick={() => void loadRoute(selectedRouteId)}
        >
            capability 새로고침
        </button>
    </div>

    {#if workspace.selected_capability_model_route_id}
        <div class="content-grid">
            <section class="inspection-card" aria-labelledby="effective-capability-title">
                <h3 id="effective-capability-title">유효 capability 확인</h3>
                <div class="inspect-row">
                    <label>
                        <span>Capability 키</span>
                        <select bind:value={selectedCapabilityKey}>
                            {#each CAPABILITY_KEYS as key (key)}
                                <option value={key}>{CAPABILITY_LABELS[key]} · {key}</option>
                            {/each}
                        </select>
                    </label>
                    <button
                        class="primary"
                        type="button"
                        disabled={busy}
                        onclick={() => void inspectCapability()}
                    >
                        유효 값 확인
                    </button>
                </div>

                {#if effectiveCapability}
                    <article class:warning={effectiveCapability.has_conflict}>
                        <div class="effective-heading">
                            <strong>{capabilityLabel(effectiveCapability.selected.key)}</strong>
                            <div class="badges">
                                <span>{statusLabel(effectiveCapability.selected.status)}</span>
                                {#if effectiveCapability.selected_is_stale}
                                    <span class="warning-badge">만료됨</span>
                                {/if}
                                {#if effectiveCapability.has_conflict}
                                    <span class="warning-badge">출처 충돌</span>
                                {/if}
                            </div>
                        </div>
                        <p class="effective-value">
                            {formatValue(effectiveCapability.selected.value)}
                        </p>
                        <p>
                            선택 출처: {sourceLabel(effectiveCapability.selected.source)} · 신뢰도
                            {effectiveCapability.selected.confidence}
                        </p>
                        {#if effectiveCapability.alternatives.length > 0}
                            <details>
                                <summary>
                                    다른 관측 {effectiveCapability.alternatives.length}개
                                </summary>
                                <ul class="compact-list">
                                    {#each effectiveCapability.alternatives as alternative (alternative.id)}
                                        <li>
                                            <strong>{formatValue(alternative.value)}</strong>
                                            <span>
                                                {sourceLabel(alternative.source)} ·
                                                {statusLabel(alternative.status)}
                                            </span>
                                        </li>
                                    {/each}
                                </ul>
                            </details>
                        {/if}
                        <small>평가 시각 {effectiveCapability.evaluated_at}</small>
                    </article>
                {:else}
                    <p class="empty-note">키를 선택하고 유효 값을 확인해 주세요.</p>
                {/if}
            </section>

            <section class="override-card" aria-labelledby="capability-override-title">
                <header>
                    <div>
                        <h3 id="capability-override-title">
                            {overrideId === '' ? '사용자 override 추가' : '사용자 override 수정'}
                        </h3>
                        <p>구조화 값은 override할 수 없으며 Core가 저장 값을 다시 검증합니다.</p>
                    </div>
                    {#if overrideId !== ''}
                        <button type="button" disabled={busy} onclick={resetOverrideForm}>
                            수정 취소
                        </button>
                    {/if}
                </header>

                <form
                    onsubmit={(event) => {
                        event.preventDefault();
                        void saveOverride();
                    }}
                >
                    <div class="form-grid">
                        <label>
                            <span>Capability 키</span>
                            <select bind:value={overrideKey} disabled={busy}>
                                {#each CAPABILITY_KEYS as key (key)}
                                    <option value={key}>{CAPABILITY_LABELS[key]} · {key}</option>
                                {/each}
                            </select>
                        </label>
                        <label>
                            <span>값 종류</span>
                            <select bind:value={overrideValueKind} disabled={busy}>
                                <option value="boolean">Boolean</option>
                                <option value="integer">Integer</option>
                                <option value="enum_values">Enum 목록</option>
                            </select>
                        </label>

                        {#if overrideValueKind === 'boolean'}
                            <label>
                                <span>Boolean 값</span>
                                <select bind:value={booleanValue} disabled={busy}>
                                    <option value={true}>true</option>
                                    <option value={false}>false</option>
                                </select>
                            </label>
                        {:else if overrideValueKind === 'integer'}
                            <label>
                                <span>정수 값</span>
                                <input
                                    type="number"
                                    step="1"
                                    bind:value={integerValue}
                                    disabled={busy}
                                />
                            </label>
                        {:else}
                            <label>
                                <span>열거 값</span>
                                <textarea
                                    rows="3"
                                    bind:value={enumValues}
                                    disabled={busy}
                                    placeholder="값을 쉼표 또는 줄바꿈으로 구분"
                                    required></textarea>
                            </label>
                        {/if}

                        <label>
                            <span>상태</span>
                            <select bind:value={overrideStatus} disabled={busy}>
                                <option value="verified">검증됨</option>
                                <option value="unsupported">지원하지 않음</option>
                                <option value="unknown">알 수 없음</option>
                                <option value="conditional">조건부</option>
                            </select>
                        </label>
                        <label>
                            <span>만료 시각 (선택)</span>
                            <input type="datetime-local" bind:value={expiresAt} disabled={busy} />
                        </label>
                    </div>

                    {#if formError}
                        <p class="form-error" role="alert">{formError}</p>
                    {/if}

                    <button class="primary" type="submit" disabled={busy}>
                        {overrideId === '' ? '사용자 override 저장' : '사용자 override 업데이트'}
                    </button>
                </form>
            </section>
        </div>

        <section class="data-card" aria-labelledby="capability-observations-title">
            <header>
                <h3 id="capability-observations-title">Capability 관측</h3>
                <span>{workspace.capability_observations.length}개</span>
            </header>
            {#if workspace.capability_observations.length === 0}
                <p class="empty-note">저장된 capability 관측이 없습니다.</p>
            {:else}
                <ul class="observation-list">
                    {#each workspace.capability_observations as observation (observation.id)}
                        <li>
                            <div class="observation-main">
                                <strong>{capabilityLabel(observation.key)}</strong>
                                <code>{observation.key}</code>
                                <span class="observation-value">
                                    {formatValue(observation.value)}
                                </span>
                            </div>
                            <dl>
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
                            {#if observation.source === 'user_override'}
                                <div class="actions">
                                    {#if isCapabilityKey(observation.key) && observation.value.type !== 'structured'}
                                        <button
                                            type="button"
                                            disabled={busy}
                                            onclick={() => editOverride(observation)}
                                        >
                                            이 override 수정
                                        </button>
                                    {/if}
                                    <button
                                        class="danger"
                                        type="button"
                                        disabled={busy}
                                        onclick={() => void deleteOverride(observation)}
                                    >
                                        사용자 override 삭제
                                    </button>
                                </div>
                            {/if}
                        </li>
                    {/each}
                </ul>
            {/if}
        </section>

        <section class="data-card" aria-labelledby="effective-parameters-title">
            <header>
                <h3 id="effective-parameters-title">유효 생성 파라미터</h3>
                <span>{workspace.capability_parameter_specs.length}개</span>
            </header>
            {#if workspace.capability_parameter_specs.length === 0}
                <p class="empty-note">이 라우트에서 사용할 수 있는 파라미터가 없습니다.</p>
            {:else}
                <ul class="parameter-list">
                    {#each workspace.capability_parameter_specs as spec (spec.id)}
                        <li>
                            <div>
                                <strong>{spec.label_key}</strong>
                                <code>{spec.id}</code>
                            </div>
                            <dl>
                                <div>
                                    <dt>값 형식</dt>
                                    <dd>{spec.value_type}</dd>
                                </div>
                                <div>
                                    <dt>범위</dt>
                                    <dd>
                                        {spec.minimum ?? '제한 없음'} – {spec.maximum ??
                                            '제한 없음'}
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
        </section>
    {:else}
        <p class="empty-note route-empty">모델 라우트를 선택하면 capability 상태를 불러옵니다.</p>
    {/if}
</section>

<style>
    .capability-panel {
        padding: 20px;
        border: 1px solid var(--line);
        border-radius: var(--radius-md);
        background: var(--surface-raised);
    }

    .panel-heading h2,
    h3 {
        margin: 3px 0;
    }

    .panel-heading p:last-child,
    .override-card header p,
    .empty-note {
        margin: 5px 0 0;
        color: var(--ink-muted);
        line-height: 1.45;
    }

    .route-row,
    .inspect-row,
    .override-card > header,
    .data-card > header,
    .actions,
    .effective-heading {
        display: flex;
        gap: 10px;
        align-items: end;
        justify-content: space-between;
    }

    .route-row {
        margin-top: 16px;
    }

    label {
        display: grid;
        flex: 1;
        gap: 6px;
        color: var(--ink-muted);
        font-size: 0.78rem;
        font-weight: 700;
    }

    .content-grid {
        display: grid;
        grid-template-columns: repeat(2, minmax(0, 1fr));
        gap: 14px;
        margin-top: 16px;
    }

    .inspection-card,
    .override-card,
    .data-card {
        padding: 15px;
        border: 1px solid var(--line);
        border-radius: 14px;
        background: var(--surface);
    }

    .inspect-row {
        margin-top: 12px;
    }

    .inspection-card article {
        margin-top: 12px;
        padding: 12px;
        border-radius: 11px;
        background: var(--surface-muted);
    }

    .inspection-card article.warning {
        border: 1px solid color-mix(in srgb, var(--danger), transparent 55%);
    }

    .effective-value {
        margin: 10px 0 4px;
        overflow-wrap: anywhere;
        font-size: 1.1rem;
        font-weight: 850;
    }

    .inspection-card article > p:not(.effective-value),
    .inspection-card small {
        color: var(--ink-muted);
        font-size: 0.75rem;
    }

    .badges,
    .actions {
        flex-wrap: wrap;
        justify-content: flex-start;
    }

    .badges span {
        padding: 3px 7px;
        border-radius: 999px;
        color: var(--accent);
        background: var(--accent-soft);
        font-size: 0.68rem;
        font-weight: 800;
    }

    .badges .warning-badge {
        color: var(--danger);
        background: color-mix(in srgb, var(--danger), transparent 88%);
    }

    details {
        margin: 10px 0;
    }

    .compact-list,
    .observation-list,
    .parameter-list {
        display: grid;
        gap: 8px;
        margin: 10px 0 0;
        padding: 0;
        list-style: none;
    }

    .compact-list li {
        display: flex;
        gap: 8px;
        justify-content: space-between;
    }

    .compact-list span {
        color: var(--ink-muted);
        font-size: 0.72rem;
    }

    .override-card form {
        display: grid;
        gap: 12px;
        margin-top: 12px;
    }

    .form-grid {
        display: grid;
        grid-template-columns: repeat(2, minmax(0, 1fr));
        gap: 10px;
    }

    .form-error {
        margin: 0;
        color: var(--danger);
        font-size: 0.78rem;
        font-weight: 750;
    }

    .data-card {
        margin-top: 14px;
    }

    .data-card > header {
        align-items: center;
    }

    .data-card > header span {
        color: var(--ink-muted);
        font-size: 0.75rem;
    }

    .observation-list > li,
    .parameter-list > li {
        display: grid;
        gap: 10px;
        padding: 11px;
        border-radius: 11px;
        background: var(--surface-muted);
    }

    .observation-main,
    .parameter-list > li > div:first-child {
        display: flex;
        gap: 8px;
        align-items: baseline;
        flex-wrap: wrap;
    }

    code,
    .observation-value {
        overflow-wrap: anywhere;
        font-size: 0.72rem;
    }

    .observation-value {
        margin-left: auto;
        font-weight: 800;
    }

    dl {
        display: grid;
        grid-template-columns: repeat(4, minmax(0, 1fr));
        gap: 8px;
        margin: 0;
    }

    dl > div {
        min-width: 0;
    }

    dt {
        color: var(--ink-muted);
        font-size: 0.67rem;
    }

    dd {
        margin: 3px 0 0;
        overflow-wrap: anywhere;
        font-size: 0.76rem;
        font-weight: 750;
    }

    .parameter-list small {
        color: var(--ink-muted);
    }

    .route-empty {
        margin-top: 16px;
        padding: 12px;
        border-radius: 11px;
        background: var(--surface-muted);
    }

    @media (max-width: 760px) {
        .content-grid,
        .form-grid {
            grid-template-columns: 1fr;
        }

        .route-row,
        .inspect-row,
        .override-card > header {
            align-items: stretch;
            flex-direction: column;
        }

        dl {
            grid-template-columns: repeat(2, minmax(0, 1fr));
        }
    }
</style>
