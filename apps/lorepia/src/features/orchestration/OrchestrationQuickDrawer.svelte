<script lang="ts">
    import { tick } from 'svelte';

    import type { LorepiaAppState } from '../../app/app-controller';
    import type {
        CreatorControlDto,
        CreatorControlValue,
        RoomOrchestrationConfigDto,
    } from '../../lib/ipc/contracts';
    import type { OrchestrationController, OrchestrationState } from './orchestration-controller';

    interface Props {
        appState: LorepiaAppState;
        orchestrationState: OrchestrationState;
        controller: OrchestrationController;
        onOpen?: () => void;
        onOpenStudio?: () => void;
    }

    let {
        appState,
        orchestrationState,
        controller,
        onOpen = () => undefined,
        onOpenStudio = () => undefined,
    }: Props = $props();
    let open = $state(false);
    let closeButton = $state<HTMLButtonElement | null>(null);
    let toggleButton = $state<HTMLButtonElement | null>(null);

    const roomConfig = $derived(orchestrationState.workspace.room_config);
    const generationPresets = $derived(appState.providers.workspace.presets.slice(0, 200));
    const modelRoutes = $derived(appState.providers.workspace.routes.slice(0, 200));
    const selectedGenerationPreset = $derived(
        generationPresets.find((preset) => preset.id === roomConfig.generation_preset_id) ?? null,
    );
    const selectedModelRouteId = $derived(
        selectedGenerationPreset?.model_route_id ??
            orchestrationState.workspace.generation_target?.model_route_id ??
            appState.providers.workspace.settings.selected_model_route_id,
    );
    const visibleGenerationPresets = $derived(
        selectedModelRouteId === null
            ? generationPresets
            : generationPresets.filter((preset) => preset.model_route_id === selectedModelRouteId),
    );

    async function setOpen(next: boolean): Promise<void> {
        open = next;
        if (next) {
            onOpen();
            await tick();
            closeButton?.focus();
        } else {
            await tick();
            toggleButton?.focus();
        }
    }

    function handleWindowKeydown(event: KeyboardEvent): void {
        if (open && event.key === 'Escape') {
            event.preventDefault();
            void setOpen(false);
        }
    }

    function controlValue(control: CreatorControlDto): CreatorControlValue {
        return roomConfig.creator_values[control.id] ?? control.value;
    }

    function selectedValues(control: CreatorControlDto): string[] {
        const value = controlValue(control);
        return Array.isArray(value) ? value : [];
    }

    function toggleMultiChoice(control: CreatorControlDto, choice: string, checked: boolean): void {
        const values = selectedValues(control);
        const nextValues = checked
            ? values.includes(choice)
                ? values
                : [...values, choice]
            : values.filter((value) => value !== choice);
        controller.stageCreatorControl(control.id, nextValues);
    }

    function selectModelRoute(modelRouteId: string): void {
        if (modelRouteId === '') {
            controller.stageRoomConfig({ generation_preset_id: null });
            return;
        }
        const currentPreset = generationPresets.find(
            (preset) =>
                preset.id === roomConfig.generation_preset_id &&
                preset.model_route_id === modelRouteId,
        );
        const nextPreset =
            currentPreset ??
            generationPresets.find((preset) => preset.model_route_id === modelRouteId);
        if (nextPreset !== undefined) {
            controller.stageRoomConfig({ generation_preset_id: nextPreset.id });
        }
    }
</script>

<svelte:window onkeydown={handleWindowKeydown} />

<div class="quick-orchestration">
    <button
        class="orchestration-toggle"
        type="button"
        bind:this={toggleButton}
        aria-expanded={open}
        aria-controls="orchestration-quick-drawer"
        onclick={() => void setOpen(!open)}
    >
        생성 설정
        {#if orchestrationState.dirty_room_config}
            <span class="dirty-dot" aria-label="저장하지 않은 변경 있음"></span>
        {/if}
    </button>

    {#if open}
        <aside
            id="orchestration-quick-drawer"
            class="quick-drawer"
            aria-labelledby="orchestration-quick-title"
        >
            <header>
                <div>
                    <p class="eyebrow">이번 대화</p>
                    <h3 id="orchestration-quick-title">프롬프트와 생성</h3>
                </div>
                <button
                    class="icon-button"
                    type="button"
                    bind:this={closeButton}
                    aria-label="생성 설정 닫기"
                    onclick={() => void setOpen(false)}
                >
                    ×
                </button>
            </header>

            {#if orchestrationState.phase === 'loading'}
                <p class="drawer-status" role="status">방 설정을 불러오는 중입니다.</p>
            {:else if orchestrationState.phase === 'unavailable'}
                <p class="drawer-status warning" role="note">{orchestrationState.error}</p>
            {:else if orchestrationState.error !== null}
                <p class="drawer-status error" role="alert">{orchestrationState.error}</p>
            {/if}

            <fieldset
                class="drawer-scroll drawer-fields"
                disabled={orchestrationState.phase !== 'ready'}
            >
                <legend class="sr-only">이 방의 프롬프트 및 생성 설정</legend>
                <label>
                    <span>프롬프트 프리셋</span>
                    <select
                        value={roomConfig.prompt_preset_id ?? ''}
                        disabled={orchestrationState.workspace.prompt_presets.length === 0}
                        onchange={(event) =>
                            controller.stageRoomConfig({
                                prompt_preset_id: event.currentTarget.value || null,
                            })}
                    >
                        <option value="">기본 프롬프트</option>
                        {#each orchestrationState.workspace.prompt_presets.slice(0, 100) as preset (preset.id)}
                            <option value={preset.id}>{preset.name}</option>
                        {/each}
                    </select>
                </label>

                <label>
                    <span>모델</span>
                    <select
                        aria-label="모델"
                        aria-describedby="orchestration-model-route-help"
                        value={selectedModelRouteId ?? ''}
                        disabled={modelRoutes.length === 0}
                        onchange={(event) => selectModelRoute(event.currentTarget.value)}
                    >
                        <option value="">모델 자동 선택</option>
                        {#each modelRoutes as route (route.id)}
                            <option
                                value={route.id}
                                disabled={!generationPresets.some(
                                    (preset) => preset.model_route_id === route.id,
                                )}
                            >
                                {route.display_name ?? route.model_id} · {route.status}
                            </option>
                        {/each}
                    </select>
                    <small id="orchestration-model-route-help">
                        모델을 바꾸면 그 모델에 속한 첫 생성 프리셋을 함께 선택합니다.
                    </small>
                </label>

                <label>
                    <span>생성 프리셋</span>
                    <select
                        value={roomConfig.generation_preset_id ?? ''}
                        onchange={(event) =>
                            controller.stageRoomConfig({
                                generation_preset_id: event.currentTarget.value || null,
                            })}
                    >
                        <option value="">프로바이더 기본 설정</option>
                        {#each visibleGenerationPresets as preset (preset.id)}
                            <option value={preset.id}>{preset.display_name}</option>
                        {/each}
                    </select>
                </label>

                <fieldset>
                    <legend>응답 길이</legend>
                    <div class="choice-row">
                        {#each ['short', 'balanced', 'long'] as length (length)}
                            <label>
                                <input
                                    type="radio"
                                    name="response-length"
                                    value={length}
                                    checked={roomConfig.response_length === length}
                                    disabled={!roomConfig.supported_fields.response_length}
                                    onchange={() =>
                                        controller.stageRoomConfig({
                                            response_length:
                                                length as RoomOrchestrationConfigDto['response_length'],
                                        })}
                                />
                                <span>
                                    {length === 'short'
                                        ? '짧게'
                                        : length === 'balanced'
                                          ? '균형'
                                          : '길게'}
                                </span>
                            </label>
                        {/each}
                    </div>
                </fieldset>

                <label>
                    <span>창의성 <output>{roomConfig.creativity}</output></span>
                    <input
                        type="range"
                        aria-label="창의성"
                        min="0"
                        max="100"
                        step="1"
                        value={roomConfig.creativity}
                        disabled={!roomConfig.supported_fields.creativity}
                        oninput={(event) =>
                            controller.stageRoomConfig({
                                creativity: Number(event.currentTarget.value),
                            })}
                    />
                </label>

                <label>
                    <span>추론 강도</span>
                    <select
                        value={roomConfig.reasoning_effort}
                        disabled={!roomConfig.supported_fields.reasoning_effort}
                        onchange={(event) =>
                            controller.stageRoomConfig({
                                reasoning_effort: event.currentTarget
                                    .value as RoomOrchestrationConfigDto['reasoning_effort'],
                            })}
                    >
                        <option value="provider_default">모델 기본값</option>
                        <option value="minimal">최소</option>
                        <option value="low">낮음</option>
                        <option value="medium">중간</option>
                        <option value="high">높음</option>
                        <option value="extra_high">매우 높음</option>
                        <option value="maximum">최대</option>
                    </select>
                </label>

                <fieldset>
                    <legend>대화 보강</legend>
                    <label class="toggle-row">
                        <input
                            type="checkbox"
                            checked={roomConfig.memory_enabled}
                            disabled={!roomConfig.supported_fields.memory_enabled}
                            onchange={(event) =>
                                controller.stageRoomConfig({
                                    memory_enabled: event.currentTarget.checked,
                                })}
                        />
                        <span>장기기억 사용</span>
                    </label>
                    <label class="toggle-row">
                        <input
                            type="checkbox"
                            checked={roomConfig.knowledge_enabled}
                            disabled={!roomConfig.supported_fields.knowledge_enabled}
                            onchange={(event) =>
                                controller.stageRoomConfig({
                                    knowledge_enabled: event.currentTarget.checked,
                                })}
                        />
                        <span>세계관 지식 사용</span>
                    </label>
                </fieldset>

                {#if orchestrationState.workspace.creator_controls.length > 0}
                    <fieldset>
                        <legend>제작자 조절 항목</legend>
                        <div class="creator-controls">
                            {#each orchestrationState.workspace.creator_controls.slice(0, 80) as control (control.id)}
                                {#if control.kind === 'toggle'}
                                    <label class="toggle-row">
                                        <input
                                            type="checkbox"
                                            checked={Boolean(controlValue(control))}
                                            onchange={(event) =>
                                                controller.stageCreatorControl(
                                                    control.id,
                                                    event.currentTarget.checked,
                                                )}
                                        />
                                        <span>{control.label}</span>
                                    </label>
                                {:else if control.kind === 'select'}
                                    <label>
                                        <span>{control.label}</span>
                                        <select
                                            value={String(controlValue(control))}
                                            onchange={(event) =>
                                                controller.stageCreatorControl(
                                                    control.id,
                                                    event.currentTarget.value,
                                                )}
                                        >
                                            {#each control.choices.slice(0, 100) as choice (choice)}
                                                <option value={choice}>{choice}</option>
                                            {/each}
                                        </select>
                                    </label>
                                {:else if control.kind === 'multi_select'}
                                    <fieldset class="nested-fieldset">
                                        <legend>{control.label}</legend>
                                        {#each control.choices.slice(0, 40) as choice (choice)}
                                            <label class="toggle-row">
                                                <input
                                                    type="checkbox"
                                                    checked={selectedValues(control).includes(
                                                        choice,
                                                    )}
                                                    onchange={(event) =>
                                                        toggleMultiChoice(
                                                            control,
                                                            choice,
                                                            event.currentTarget.checked,
                                                        )}
                                                />
                                                <span>{choice}</span>
                                            </label>
                                        {/each}
                                    </fieldset>
                                {:else if control.kind === 'number' || control.kind === 'slider'}
                                    <label>
                                        <span>{control.label}</span>
                                        <input
                                            type={control.kind === 'slider' ? 'range' : 'number'}
                                            min={control.minimum ?? undefined}
                                            max={control.maximum ?? undefined}
                                            step={control.step ?? 1}
                                            value={Number(controlValue(control))}
                                            oninput={(event) =>
                                                controller.stageCreatorControl(
                                                    control.id,
                                                    Number(event.currentTarget.value),
                                                )}
                                        />
                                    </label>
                                {:else}
                                    <label>
                                        <span>{control.label}</span>
                                        <input
                                            type="text"
                                            maxlength="4096"
                                            value={String(controlValue(control))}
                                            oninput={(event) =>
                                                controller.stageCreatorControl(
                                                    control.id,
                                                    event.currentTarget.value,
                                                )}
                                        />
                                    </label>
                                {/if}
                                {#if control.description}
                                    <small>{control.description}</small>
                                {/if}
                            {/each}
                        </div>
                    </fieldset>
                {/if}
            </fieldset>

            <footer>
                <button type="button" onclick={onOpenStudio}>고급 설정</button>
                <button
                    class="primary"
                    type="button"
                    disabled={!orchestrationState.dirty_room_config || orchestrationState.saving}
                    onclick={() => void controller.saveRoomConfig()}
                >
                    {orchestrationState.saving ? '저장 중…' : '방 설정 저장'}
                </button>
            </footer>
        </aside>
    {/if}
</div>

<style>
    .quick-orchestration {
        position: relative;
    }

    .orchestration-toggle {
        display: inline-flex;
        gap: 7px;
        align-items: center;
        white-space: nowrap;
    }

    .dirty-dot {
        width: 7px;
        height: 7px;
        border-radius: 999px;
        background: var(--accent);
    }

    .quick-drawer {
        position: absolute;
        z-index: 20;
        top: calc(100% + 10px);
        right: 0;
        display: grid;
        grid-template-rows: auto minmax(0, 1fr) auto;
        width: min(390px, calc(100vw - 28px));
        max-height: min(680px, calc(100vh - 130px));
        overflow: hidden;
        border: 1px solid var(--line);
        border-radius: var(--radius-md);
        background: var(--surface-raised);
        box-shadow: 0 18px 52px rgb(18 25 38 / 20%);
    }

    .quick-drawer > header,
    .quick-drawer > footer {
        display: flex;
        gap: 12px;
        align-items: center;
        justify-content: space-between;
        padding: 14px 16px;
    }

    .quick-drawer > header {
        border-bottom: 1px solid var(--line);
    }

    .quick-drawer > footer {
        border-top: 1px solid var(--line);
    }

    .quick-drawer h3,
    .quick-drawer p {
        margin: 0;
    }

    .icon-button {
        width: 36px;
        min-width: 36px;
        padding: 6px;
        font-size: 1.35rem;
    }

    .drawer-scroll {
        display: grid;
        gap: 15px;
        padding: 16px;
        overflow-y: auto;
    }

    .drawer-fields {
        min-width: 0;
        margin: 0;
        border: 0;
    }

    .drawer-fields:disabled {
        opacity: 0.68;
    }

    .drawer-scroll > label,
    .creator-controls > label {
        display: grid;
        gap: 6px;
    }

    .drawer-scroll fieldset {
        display: grid;
        gap: 9px;
        min-width: 0;
        margin: 0;
        padding: 12px;
        border: 1px solid var(--line);
        border-radius: 11px;
    }

    .nested-fieldset {
        background: var(--surface-muted);
    }

    .choice-row {
        display: grid;
        grid-template-columns: repeat(3, 1fr);
        gap: 6px;
    }

    .choice-row label {
        display: flex;
        gap: 5px;
        align-items: center;
        justify-content: center;
        padding: 8px;
        border-radius: 9px;
        background: var(--surface-muted);
    }

    .toggle-row {
        display: flex !important;
        gap: 8px;
        align-items: center;
    }

    .creator-controls {
        display: grid;
        gap: 10px;
    }

    .creator-controls small,
    .drawer-status {
        color: var(--ink-muted);
    }

    .drawer-status {
        padding: 10px 16px;
        background: var(--surface-muted);
    }

    .drawer-status.warning {
        color: var(--warning-ink, #7a4b00);
    }

    .drawer-status.error {
        color: var(--danger);
    }

    @media (max-width: 640px) {
        .quick-drawer {
            position: fixed;
            inset: auto 8px 76px;
            width: auto;
            max-height: calc(100vh - 150px);
        }
    }
</style>
