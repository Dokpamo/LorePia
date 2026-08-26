<script lang="ts">
    import { Menu, X } from '@lucide/svelte';
    import { tr } from '../../lib/i18n';
    import { tick, type Snippet } from 'svelte';

    import ChoicePopover from '../../components/ChoicePopover.svelte';
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
        roomControls?: Snippet<[closeSettings: () => Promise<void>]>;
    }

    let {
        appState,
        orchestrationState,
        controller,
        onOpen = () => undefined,
        roomControls,
    }: Props = $props();
    let open = $state(false);
    let toggleButton = $state<HTMLButtonElement | null>(null);
    let drawerElement = $state<HTMLDivElement | null>(null);
    let dragOffset = $state(0);
    let dragging = $state(false);
    let settling = $state(false);
    let handleDragged = $state(false);
    let dragStartY = 0;
    let dragStartTime = 0;
    let dragLastY = 0;
    let dragLastTime = 0;

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

    async function setOpen(next: boolean, restoreToggleFocus = true): Promise<void> {
        if (next) {
            dragOffset = 0;
            dragging = false;
            settling = false;
            handleDragged = false;
        } else if (orchestrationState.dirty_room_config && !orchestrationState.saving) {
            void controller.saveRoomConfig();
        }
        open = next;
        if (next) {
            onOpen();
            await tick();
            drawerElement?.focus();
        } else {
            await tick();
            dragOffset = 0;
            dragging = false;
            settling = false;
            if (restoreToggleFocus) toggleButton?.focus();
        }
    }

    function handleSheetPointerDown(event: PointerEvent): void {
        if (event.button !== 0 || drawerElement === null) return;
        event.preventDefault();
        if (
            event.currentTarget instanceof HTMLElement &&
            typeof event.currentTarget.setPointerCapture === 'function'
        ) {
            event.currentTarget.setPointerCapture(event.pointerId);
        }
        dragging = true;
        settling = false;
        handleDragged = false;
        dragStartY = event.clientY;
        dragStartTime = performance.now();
        dragLastY = event.clientY;
        dragLastTime = dragStartTime;
    }

    function handleSheetPointerMove(event: PointerEvent): void {
        if (!dragging) return;
        const nextOffset = Math.max(0, event.clientY - dragStartY);
        dragOffset = nextOffset;
        handleDragged ||= nextOffset > 5;
        dragLastY = event.clientY;
        dragLastTime = performance.now();
    }

    async function dismissFromDrag(): Promise<void> {
        dragging = false;
        settling = true;
        dragOffset = (drawerElement?.offsetHeight ?? window.innerHeight * 0.7) + 24;
        await new Promise((resolve) => window.setTimeout(resolve, 260));
        await setOpen(false, false);
    }

    function settleAfterDrag(): void {
        dragging = false;
        settling = true;
        dragOffset = 0;
        window.setTimeout(() => {
            settling = false;
        }, 300);
    }

    function handleSheetPointerUp(event: PointerEvent): void {
        if (!dragging) return;
        const target = event.currentTarget;
        if (
            target instanceof HTMLElement &&
            typeof target.hasPointerCapture === 'function' &&
            target.hasPointerCapture(event.pointerId)
        ) {
            target.releasePointerCapture(event.pointerId);
        }
        const now = performance.now();
        const recentElapsed = Math.max(1, now - dragLastTime);
        const recentVelocity = (event.clientY - dragLastY) / recentElapsed;
        const totalElapsed = Math.max(1, now - dragStartTime);
        const totalVelocity = (event.clientY - dragStartY) / totalElapsed;
        const closeThreshold = Math.max(72, (drawerElement?.offsetHeight ?? 0) * 0.18);
        if (dragOffset >= closeThreshold || Math.max(recentVelocity, totalVelocity) > 0.55) {
            void dismissFromDrag();
        } else {
            settleAfterDrag();
        }
    }

    function handleSheetPointerCancel(): void {
        if (dragging) settleAfterDrag();
    }

    function handleSheetHandleClick(event: MouseEvent): void {
        if (handleDragged) {
            handleDragged = false;
            return;
        }
        void setOpen(false, event.detail === 0);
    }

    function handleWindowKeydown(event: KeyboardEvent): void {
        if (!event.defaultPrevented && open && event.key === 'Escape') {
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
        class="orchestration-toggle mobile-top-action mobile-top-action-right"
        type="button"
        bind:this={toggleButton}
        aria-label={$tr('quick.toggle')}
        aria-expanded={open}
        aria-controls="orchestration-quick-drawer"
        onclick={() => void setOpen(!open)}
    >
        <Menu class="orchestration-toggle-icon" aria-hidden="true" />
        {#if orchestrationState.dirty_room_config}
            <span class="dirty-dot" aria-label={$tr('quick.dirty')}></span>
        {/if}
    </button>

    <button
        class="quick-drawer-backdrop"
        class:open
        type="button"
        aria-label="대화 설정 바깥 영역을 눌러 닫기"
        aria-hidden={!open}
        tabindex={open ? 0 : -1}
        disabled={!open}
        onclick={() => void setOpen(false, false)}
    ></button>
    <div
        id="orchestration-quick-drawer"
        class="quick-drawer"
        class:open
        class:dragging
        class:settling
        bind:this={drawerElement}
        style:--sheet-drag-y={`${String(dragOffset)}px`}
        tabindex="-1"
        role="dialog"
        aria-modal="true"
        aria-hidden={!open}
        inert={!open}
        aria-labelledby="orchestration-quick-title"
    >
        <button
            class="sheet-handle"
            type="button"
            aria-label={$tr('quick.drag_close')}
            onpointerdown={handleSheetPointerDown}
            onpointermove={handleSheetPointerMove}
            onpointerup={handleSheetPointerUp}
            onpointercancel={handleSheetPointerCancel}
            onclick={handleSheetHandleClick}><span aria-hidden="true"></span></button
        >
        <header>
            <div>
                <p class="eyebrow">{$tr('quick.eyebrow')}</p>
                <h3 id="orchestration-quick-title">{$tr('quick.title')}</h3>
            </div>
            <button
                class="icon-button"
                type="button"
                aria-label={$tr('quick.close')}
                onclick={(event) => void setOpen(false, event.detail === 0)}
            >
                <X class="quick-drawer-close-icon" aria-hidden="true" />
            </button>
        </header>

        <div class="drawer-body">
            <div class="quick-drawer-context">
                {#if orchestrationState.phase === 'loading'}
                    <p class="drawer-status" role="status">{$tr('quick.loading')}</p>
                {:else if orchestrationState.phase === 'unavailable'}
                    <p class="drawer-status warning" role="note">{orchestrationState.error}</p>
                {:else if orchestrationState.error !== null}
                    <p class="drawer-status error" role="alert">{orchestrationState.error}</p>
                {/if}

                {#if roomControls}
                    <section class="drawer-room-controls" aria-labelledby="room-controls-title">
                        <h4 id="room-controls-title">대화</h4>
                        {@render roomControls(() => setOpen(false, false))}
                    </section>
                {/if}
            </div>

            <h4 id="generation-settings-title" class="drawer-section-title">
                {$tr('quick.legend')}
            </h4>
            <fieldset
                class="drawer-scroll drawer-fields"
                aria-labelledby="generation-settings-title"
                disabled={orchestrationState.phase !== 'ready'}
            >
                <legend class="sr-only">{$tr('quick.legend')}</legend>
                <div class="drawer-setting-row">
                    <ChoicePopover
                        id="orchestration-prompt-preset"
                        label={$tr('quick.preset')}
                        value={roomConfig.prompt_preset_id ?? ''}
                        disabled={orchestrationState.workspace.prompt_presets.length === 0}
                        options={[
                            { value: '', label: $tr('quick.preset.default') },
                            ...orchestrationState.workspace.prompt_presets
                                .slice(0, 100)
                                .map((preset) => ({ value: preset.id, label: preset.name })),
                        ]}
                        onSelect={(value: string) =>
                            controller.stageRoomConfig({
                                prompt_preset_id: value.length === 0 ? null : value,
                            })}
                    />
                </div>

                <div class="drawer-setting-row">
                    <ChoicePopover
                        id="orchestration-model-route"
                        label={$tr('quick.model')}
                        value={selectedModelRouteId ?? ''}
                        disabled={modelRoutes.length === 0}
                        options={[
                            { value: '', label: $tr('quick.model.auto') },
                            ...modelRoutes.map((route) => ({
                                value: route.id,
                                label: `${route.display_name ?? route.model_id} · ${route.status}`,
                                disabled: !generationPresets.some(
                                    (preset) => preset.model_route_id === route.id,
                                ),
                            })),
                        ]}
                        onSelect={selectModelRoute}
                    />
                </div>

                <div class="drawer-setting-row">
                    <ChoicePopover
                        id="orchestration-generation-preset"
                        label={$tr('quick.generation_preset')}
                        value={roomConfig.generation_preset_id ?? ''}
                        options={[
                            { value: '', label: $tr('quick.generation_preset.default') },
                            ...visibleGenerationPresets.map((preset) => ({
                                value: preset.id,
                                label: preset.display_name,
                            })),
                        ]}
                        onSelect={(value: string) =>
                            controller.stageRoomConfig({
                                generation_preset_id: value.length === 0 ? null : value,
                            })}
                    />
                </div>

                <fieldset>
                    <legend>{$tr('quick.length')}</legend>
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
                                        ? $tr('quick.length.short')
                                        : length === 'balanced'
                                          ? $tr('quick.length.balanced')
                                          : $tr('quick.length.long')}
                                </span>
                            </label>
                        {/each}
                    </div>
                </fieldset>

                <label>
                    <span
                        >{$tr('quick.creativity')}
                        <output>{roomConfig.creativity}</output></span
                    >
                    <input
                        type="range"
                        aria-label={$tr('quick.creativity')}
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

                <div class="drawer-setting-row">
                    <ChoicePopover
                        id="orchestration-reasoning-effort"
                        label={$tr('quick.reasoning')}
                        value={roomConfig.reasoning_effort}
                        disabled={!roomConfig.supported_fields.reasoning_effort}
                        options={[
                            {
                                value: 'provider_default',
                                label: $tr('quick.reasoning.provider_default'),
                            },
                            { value: 'minimal', label: $tr('quick.reasoning.minimal') },
                            { value: 'low', label: $tr('quick.reasoning.low') },
                            { value: 'medium', label: $tr('quick.reasoning.medium') },
                            { value: 'high', label: $tr('quick.reasoning.high') },
                            { value: 'extra_high', label: $tr('quick.reasoning.extra_high') },
                            { value: 'maximum', label: $tr('quick.reasoning.maximum') },
                        ]}
                        onSelect={(value: string) =>
                            controller.stageRoomConfig({
                                reasoning_effort:
                                    value as RoomOrchestrationConfigDto['reasoning_effort'],
                            })}
                    />
                </div>

                <fieldset>
                    <legend>{$tr('quick.enrichment')}</legend>
                    <button
                        class="switch-button"
                        type="button"
                        role="switch"
                        aria-label={$tr('quick.memory')}
                        aria-checked={roomConfig.memory_enabled}
                        disabled={!roomConfig.supported_fields.memory_enabled}
                        onclick={() =>
                            controller.stageRoomConfig({
                                memory_enabled: !roomConfig.memory_enabled,
                            })}
                    >
                        <span>{$tr('quick.memory')}</span>
                        <span class="switch-track" aria-hidden="true">
                            <span class="switch-thumb"></span>
                        </span>
                    </button>
                    <button
                        class="switch-button"
                        type="button"
                        role="switch"
                        aria-label={$tr('quick.knowledge')}
                        aria-checked={roomConfig.knowledge_enabled}
                        disabled={!roomConfig.supported_fields.knowledge_enabled}
                        onclick={() =>
                            controller.stageRoomConfig({
                                knowledge_enabled: !roomConfig.knowledge_enabled,
                            })}
                    >
                        <span>{$tr('quick.knowledge')}</span>
                        <span class="switch-track" aria-hidden="true">
                            <span class="switch-thumb"></span>
                        </span>
                    </button>
                </fieldset>

                {#if orchestrationState.workspace.creator_controls.length > 0}
                    <fieldset>
                        <legend>{$tr('quick.creator_controls')}</legend>
                        <div class="creator-controls">
                            {#each orchestrationState.workspace.creator_controls.slice(0, 80) as control (control.id)}
                                {#if control.kind === 'toggle'}
                                    <button
                                        class="switch-button"
                                        type="button"
                                        role="switch"
                                        aria-label={control.label}
                                        aria-checked={Boolean(controlValue(control))}
                                        onclick={() =>
                                            controller.stageCreatorControl(
                                                control.id,
                                                !controlValue(control),
                                            )}
                                    >
                                        <span>{control.label}</span>
                                        <span class="switch-track" aria-hidden="true">
                                            <span class="switch-thumb"></span>
                                        </span>
                                    </button>
                                {:else if control.kind === 'select'}
                                    <div class="creator-choice-row">
                                        <ChoicePopover
                                            id={`creator-control-${control.id}`}
                                            label={control.label}
                                            value={String(controlValue(control))}
                                            options={control.choices
                                                .slice(0, 100)
                                                .map((choice) => ({
                                                    value: choice,
                                                    label: choice,
                                                }))}
                                            onSelect={(value: string) =>
                                                controller.stageCreatorControl(control.id, value)}
                                        />
                                    </div>
                                {:else if control.kind === 'multi_select'}
                                    <fieldset class="nested-fieldset">
                                        <legend>{control.label}</legend>
                                        {#each control.choices.slice(0, 40) as choice (choice)}
                                            <button
                                                class="switch-button"
                                                type="button"
                                                role="switch"
                                                aria-label={choice}
                                                aria-checked={selectedValues(control).includes(
                                                    choice,
                                                )}
                                                onclick={() =>
                                                    toggleMultiChoice(
                                                        control,
                                                        choice,
                                                        !selectedValues(control).includes(choice),
                                                    )}
                                            >
                                                <span>{choice}</span>
                                                <span class="switch-track" aria-hidden="true">
                                                    <span class="switch-thumb"></span>
                                                </span>
                                            </button>
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
        </div>
    </div>
</div>

<style>
    .quick-orchestration {
        position: relative;
    }

    .orchestration-toggle {
        position: relative;
        display: grid;
        width: var(--mobile-top-action);
        height: var(--mobile-top-action);
        min-width: var(--mobile-top-action);
        min-height: var(--mobile-top-action);
        padding: 0;
        border: 0;
        border-radius: 50%;
        background: var(--surface-raised);
        box-shadow: var(--shadow-1);
        place-items: center;
    }

    .orchestration-toggle :global(.orchestration-toggle-icon) {
        width: 24px;
        height: 24px;
    }

    .dirty-dot {
        position: absolute;
        top: 6px;
        right: 6px;
        width: 7px;
        height: 7px;
        border-radius: 999px;
        background: var(--brand-coral);
    }

    .quick-drawer-backdrop {
        position: fixed;
        z-index: 40;
        padding: 0;
        border: 0;
        border-radius: 0;
        -webkit-backdrop-filter: none !important;
        backdrop-filter: none !important;
        background-color: rgba(0, 0, 0, 0.14) !important;
        background-image: none !important;
        box-shadow: none;
        filter: none;
        opacity: 0;
        pointer-events: none;
        transition: none;
        visibility: hidden;
        inset: 0;
    }

    .quick-drawer-backdrop.open {
        opacity: 1;
        pointer-events: auto;
        visibility: visible;
    }

    .quick-drawer-backdrop:disabled {
        opacity: 0;
    }

    .quick-drawer {
        position: fixed;
        z-index: 41;
        right: auto;
        bottom: calc(max(-680px, -70dvh) - 24px);
        left: var(--detail-action-center, 50%);
        display: grid;
        grid-template-rows: auto auto minmax(0, 1fr);
        width: min(calc(var(--detail-action-workspace-width, 100vw) - 32px), 620px);
        height: min(70dvh, 680px);
        max-height: min(70dvh, 680px);
        overflow: hidden;
        border: 1px solid var(--line);
        border-radius: 24px;
        background: var(--bg);
        box-shadow: var(--shadow-3);
        pointer-events: none;
        transform: translateX(-50%);
        transition:
            bottom var(--panel-close-duration) var(--panel-close-easing),
            visibility 0s linear var(--panel-close-duration);
        visibility: hidden;
    }

    .quick-drawer.open {
        bottom: calc(12px - var(--sheet-drag-y, 0px));
        pointer-events: auto;
        transition:
            bottom var(--panel-open-duration) var(--panel-open-easing),
            visibility 0s;
        visibility: visible;
    }

    .quick-drawer:focus {
        outline: none;
    }

    .quick-drawer.settling {
        transition: bottom 260ms cubic-bezier(0.22, 1, 0.36, 1);
    }

    .quick-drawer.dragging {
        cursor: grabbing;
        transition: none;
        user-select: none;
    }

    .sheet-handle {
        display: flex;
        width: 100%;
        height: 22px;
        min-height: 22px;
        align-items: end;
        justify-content: center;
        padding: 0 0 4px;
        border: 0;
        border-radius: 0;
        background: transparent;
        box-shadow: none;
        cursor: grab;
        touch-action: none;
    }

    .sheet-handle span {
        width: 38px;
        height: 4px;
        border-radius: var(--radius-pill);
        background: var(--line-strong);
    }

    .quick-drawer .sheet-handle:hover:not(:disabled),
    .quick-drawer .sheet-handle:active:not(:disabled) {
        background: transparent;
    }

    .quick-drawer > header {
        display: flex;
        gap: 12px;
        align-items: center;
        justify-content: space-between;
        padding: 12px 18px;
        border-bottom: 0;
    }

    .drawer-body {
        min-height: 0;
        padding-bottom: calc(14px + env(safe-area-inset-bottom));
        overflow-y: auto;
        overscroll-behavior: contain;
    }

    .quick-drawer h3,
    .quick-drawer h4,
    .quick-drawer p {
        margin: 0;
    }

    .quick-drawer h3 {
        font-size: 1.0625rem;
    }

    .quick-drawer .eyebrow,
    .drawer-section-title,
    .drawer-room-controls h4 {
        color: var(--ink-muted);
        font-size: 0.75rem;
        font-weight: 650;
    }

    .icon-button {
        display: grid;
        width: 36px;
        height: 36px;
        min-width: 36px;
        padding: 6px;
        border: 0;
        border-radius: 50%;
        background: transparent;
        box-shadow: none;
        place-items: center;
    }

    .quick-drawer .icon-button:hover:not(:disabled) {
        background: transparent;
    }

    .icon-button :global(.quick-drawer-close-icon) {
        width: 20px;
        height: 20px;
    }

    .drawer-scroll {
        display: grid;
        gap: 3px;
        padding: 0 16px;
    }

    .drawer-fields {
        min-width: 0;
        margin: 0;
        border: 0;
    }

    .drawer-section-title {
        padding: 0 18px 8px;
    }

    .drawer-fields:disabled {
        opacity: 0.68;
    }

    .drawer-scroll > label,
    .creator-controls > label {
        display: grid;
        gap: 6px;
    }

    .drawer-scroll > label,
    .drawer-scroll > fieldset,
    .drawer-setting-row {
        padding: 12px 14px;
        border: 0;
        border-radius: 4px;
        background: var(--surface-raised);
    }

    .drawer-setting-row {
        position: relative;
        min-width: 0;
        padding: 0;
    }

    .drawer-scroll > :first-child,
    .drawer-scroll > .drawer-setting-row:first-child :global(.choice-trigger) {
        border-radius: var(--radius-lg) var(--radius-lg) 4px 4px;
    }

    .drawer-scroll > :last-child,
    .drawer-scroll > .drawer-setting-row:last-child :global(.choice-trigger) {
        border-radius: 4px 4px var(--radius-lg) var(--radius-lg);
    }

    .drawer-scroll input[type='text'],
    .drawer-scroll input[type='number'] {
        height: 38px;
        min-height: 38px;
        padding: 0 34px 0 12px;
        border: 1px solid var(--line);
        border-radius: var(--radius-md);
        background: var(--bg);
        color: var(--ink);
    }

    .drawer-scroll fieldset {
        display: grid;
        gap: 9px;
        min-width: 0;
        margin: 0;
        padding: 12px;
        border: 0;
        border-radius: var(--radius-md);
    }

    .nested-fieldset {
        background: var(--surface-sunken);
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
        background: var(--surface-sunken);
    }

    .switch-button {
        display: flex;
        width: 100%;
        min-height: 44px;
        align-items: center;
        justify-content: space-between;
        padding: 0 12px;
        border: 0;
        border-radius: 10px;
        background: transparent;
        color: var(--ink);
        font-weight: 650;
        text-align: left;
    }

    .switch-track {
        position: relative;
        width: 42px;
        height: 24px;
        flex: 0 0 42px;
        border-radius: var(--radius-pill);
        background: var(--line-strong);
        transition: background-color 160ms ease;
    }

    .switch-thumb {
        position: absolute;
        top: 3px;
        left: 3px;
        width: 18px;
        height: 18px;
        border-radius: 50%;
        background: var(--bg);
        box-shadow: var(--shadow-1);
        transform: translateX(0);
        transition: transform 180ms cubic-bezier(0.22, 1, 0.36, 1);
    }

    .switch-button[aria-checked='true'] .switch-track {
        background: var(--primary-bg);
    }

    .switch-button[aria-checked='true'] .switch-thumb {
        transform: translateX(18px);
    }

    .creator-controls {
        display: grid;
        gap: 10px;
    }

    .creator-choice-row {
        min-width: 0;
        border-radius: var(--radius-md);
        background: var(--surface-sunken);
    }

    .creator-controls small,
    .drawer-status {
        color: var(--ink-muted);
    }

    .quick-drawer-context {
        display: grid;
        min-height: 0;
    }

    .drawer-room-controls {
        display: grid;
        padding: 12px 14px;
        border-radius: var(--radius-lg);
        margin: 0 16px 14px;
        background: var(--surface-sunken);
        box-shadow: inset 0 0 0 1px color-mix(in srgb, var(--line) 72%, transparent);
        gap: 10px;
    }

    .drawer-status {
        padding: 10px 16px;
        background: var(--surface-sunken);
    }

    .drawer-status.warning {
        color: var(--warning);
    }

    .drawer-status.error {
        color: var(--danger);
    }

    @container view (max-width: 640px) {
        .quick-drawer {
            right: 0;
            bottom: calc(-70dvh - 24px);
            left: 0;
            width: 100%;
            height: 70dvh;
            max-height: 70dvh;
            border-right: 0;
            border-bottom: 0;
            border-left: 0;
            border-radius: 24px 24px 0 0;
            transform: none;
        }

        .quick-drawer.open {
            bottom: calc(0px - var(--sheet-drag-y, 0px));
        }
    }
</style>
