<script lang="ts">
    import {
        ArrowLeft,
        Brain,
        ChevronRight,
        Database,
        Menu,
        MessagesSquare,
        PanelRightClose,
        PanelRightOpen,
        SlidersHorizontal,
        X,
    } from '@lucide/svelte';
    import { tr } from '../../lib/i18n';
    import { onDestroy, tick, type Snippet } from 'svelte';

    import ChoicePopover from '../../components/ChoicePopover.svelte';
    import SegmentedControl from '../../components/SegmentedControl.svelte';
    import ToggleSwitch from '../../components/ToggleSwitch.svelte';
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
        desktop?: boolean;
        open?: boolean;
        view?: 'tools' | 'settings';
        onOpen?: () => void;
        roomControls?: Snippet<[closeSettings: () => Promise<void>]>;
    }

    let {
        appState,
        orchestrationState,
        controller,
        desktop = false,
        open = $bindable(false),
        view = $bindable('tools'),
        onOpen = () => undefined,
        roomControls,
    }: Props = $props();
    let settingsButton = $state<HTMLButtonElement | null>(null);
    let panelToggleButton = $state<HTMLButtonElement | null>(null);
    let drawerElement = $state<HTMLDivElement | null>(null);
    let panelGesture = $state<'idle' | 'tracking' | 'dragging' | 'settling'>('idle');
    let panelDragX = $state(0);
    let panelPointer: {
        pointerId: number;
        startX: number;
        startY: number;
        lastX: number;
        lastTime: number;
        velocityX: number;
        viewportWidth: number;
    } | null = null;
    let panelSettleTimer: ReturnType<typeof setTimeout> | undefined;
    let restoreToggleFocusOnClose = true;
    let observedOpen = open;
    let observedView = view;
    let lastTrigger: 'settings' | 'panel' = 'settings';
    let suppressPanelClickUntil = 0;

    const PANEL_SWIPE_AXIS_LOCK_PX = 8;
    const PANEL_SWIPE_COMMIT_MIN_PX = 64;
    const PANEL_SWIPE_COMMIT_MAX_PX = 120;
    const PANEL_SWIPE_COMMIT_RATIO = 0.22;
    const PANEL_SWIPE_FLING_MIN_PX = 32;
    const PANEL_SWIPE_FLING_VELOCITY = 0.55;
    const PANEL_SWIPE_SETTLE_MS = 260;

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
    const selectedModelRoute = $derived(
        modelRoutes.find((route) => route.id === selectedModelRouteId) ?? null,
    );
    const selectedPromptPreset = $derived(
        orchestrationState.workspace.prompt_presets.find(
            (preset) => preset.id === roomConfig.prompt_preset_id,
        ) ?? null,
    );
    const visibleGenerationPresets = $derived(
        selectedModelRouteId === null
            ? generationPresets
            : generationPresets.filter((preset) => preset.model_route_id === selectedModelRouteId),
    );

    function formatReasoningEffort(effort: RoomOrchestrationConfigDto['reasoning_effort']): string {
        return $tr(`quick.reasoning.${effort}`);
    }

    function clearPanelSettleTimer(): void {
        if (panelSettleTimer === undefined) return;
        clearTimeout(panelSettleTimer);
        panelSettleTimer = undefined;
    }

    function resetPanelGesture(): void {
        clearPanelSettleTimer();
        panelPointer = null;
        panelGesture = 'idle';
        panelDragX = 0;
    }

    $effect(() => {
        const nextOpen = open;
        const nextView = view;
        const didOpen = nextOpen && !observedOpen;
        const didClose = !nextOpen && observedOpen;
        const enteredSettings =
            nextOpen && nextView === 'settings' && (!observedOpen || observedView !== 'settings');
        const leftSettings =
            observedOpen && observedView === 'settings' && (!nextOpen || nextView !== 'settings');
        if (!didOpen && !didClose && nextView === observedView) return;
        observedOpen = nextOpen;
        observedView = nextView;
        if (didOpen || didClose) resetPanelGesture();
        if (enteredSettings) onOpen();
        if (didOpen) {
            void tick().then(() => {
                if (open) drawerElement?.focus();
            });
        }
        if (leftSettings && orchestrationState.dirty_room_config && !orchestrationState.saving) {
            void controller.saveRoomConfig();
        }
        if (didClose) {
            void tick().then(() => {
                if (restoreToggleFocusOnClose) {
                    const target =
                        lastTrigger === 'panel'
                            ? (panelToggleButton ?? settingsButton)
                            : settingsButton;
                    target?.focus();
                }
                restoreToggleFocusOnClose = true;
            });
        }
    });

    async function setOpen(
        next: boolean,
        restoreToggleFocus = true,
        trigger?: 'settings' | 'panel',
    ): Promise<void> {
        if (trigger !== undefined) lastTrigger = trigger;
        restoreToggleFocusOnClose = restoreToggleFocus;
        open = next;
        await tick();
    }

    async function showSettings(trigger?: 'settings' | 'panel'): Promise<void> {
        if (trigger !== undefined) lastTrigger = trigger;
        view = 'settings';
        open = true;
        await tick();
    }

    async function showTools(): Promise<void> {
        view = 'tools';
        await tick();
        drawerElement?.focus();
    }

    function panelSwipeCommitDistance(viewportWidth: number): number {
        return Math.min(
            PANEL_SWIPE_COMMIT_MAX_PX,
            Math.max(PANEL_SWIPE_COMMIT_MIN_PX, viewportWidth * PANEL_SWIPE_COMMIT_RATIO),
        );
    }

    function handlePanelPointerDown(event: PointerEvent): void {
        if (desktop || !open || event.button !== 0 || !event.isPrimary) return;
        const target = event.currentTarget as HTMLElement;
        const boundsWidth = target.getBoundingClientRect().width;
        panelPointer = {
            pointerId: Number.isFinite(event.pointerId) ? event.pointerId : 1,
            startX: event.clientX,
            startY: event.clientY,
            lastX: event.clientX,
            lastTime: event.timeStamp,
            velocityX: 0,
            viewportWidth: Math.max(1, boundsWidth || target.clientWidth || window.innerWidth),
        };
        panelGesture = 'tracking';
    }

    function handlePanelPointerMove(event: PointerEvent): void {
        const pointer = panelPointer;
        if (event.pointerId !== pointer?.pointerId) return;
        const deltaX = event.clientX - pointer.startX;
        const deltaY = event.clientY - pointer.startY;
        const absoluteX = Math.abs(deltaX);
        const absoluteY = Math.abs(deltaY);
        if (panelGesture === 'tracking') {
            if (absoluteX < PANEL_SWIPE_AXIS_LOCK_PX && absoluteY < PANEL_SWIPE_AXIS_LOCK_PX)
                return;
            if (deltaX <= 0 || absoluteY >= absoluteX) {
                resetPanelGesture();
                return;
            }
            if (absoluteX < absoluteY * 1.2) return;
            panelGesture = 'dragging';
            const target = event.currentTarget as HTMLElement;
            if (typeof target.setPointerCapture === 'function') {
                target.setPointerCapture(pointer.pointerId);
            }
        }
        if (panelGesture !== 'dragging') return;
        event.preventDefault();
        const elapsed = event.timeStamp - pointer.lastTime;
        if (elapsed > 0) {
            pointer.velocityX = Math.max(0, (event.clientX - pointer.lastX) / elapsed);
        }
        pointer.lastX = event.clientX;
        pointer.lastTime = event.timeStamp;
        panelDragX = Math.min(pointer.viewportWidth, Math.max(0, deltaX));
    }

    function releasePanelPointer(event: PointerEvent): void {
        const target = event.currentTarget as HTMLElement;
        if (
            typeof target.hasPointerCapture === 'function' &&
            typeof target.releasePointerCapture === 'function' &&
            target.hasPointerCapture(event.pointerId)
        ) {
            target.releasePointerCapture(event.pointerId);
        }
    }

    function settlePanelGesture(targetX: number, closes: boolean): void {
        panelPointer = null;
        panelGesture = 'settling';
        panelDragX = targetX;
        clearPanelSettleTimer();
        panelSettleTimer = setTimeout(() => {
            panelSettleTimer = undefined;
            if (closes) {
                void setOpen(false, false);
                suppressPanelClickUntil = Date.now() + 120;
            }
            resetPanelGesture();
        }, PANEL_SWIPE_SETTLE_MS);
    }

    function handlePanelPointerUp(event: PointerEvent): void {
        const pointer = panelPointer;
        if (event.pointerId !== pointer?.pointerId) return;
        releasePanelPointer(event);
        if (panelGesture !== 'dragging') {
            resetPanelGesture();
            return;
        }
        event.preventDefault();
        const distance = Math.max(0, event.clientX - pointer.startX);
        const commits =
            distance >= panelSwipeCommitDistance(pointer.viewportWidth) ||
            (distance >= PANEL_SWIPE_FLING_MIN_PX &&
                pointer.velocityX >= PANEL_SWIPE_FLING_VELOCITY);
        settlePanelGesture(commits ? pointer.viewportWidth : 0, commits);
    }

    function handlePanelPointerCancel(event: PointerEvent): void {
        if (event.pointerId !== panelPointer?.pointerId) return;
        releasePanelPointer(event);
        if (panelGesture === 'dragging') {
            settlePanelGesture(0, false);
            return;
        }
        resetPanelGesture();
    }

    function handlePanelClickCapture(event: MouseEvent): void {
        if (Date.now() > suppressPanelClickUntil) return;
        suppressPanelClickUntil = 0;
        event.preventDefault();
        event.stopPropagation();
    }

    function handleWindowKeydown(event: KeyboardEvent): void {
        if (!event.defaultPrevented && open && event.key === 'Escape') {
            event.preventDefault();
            void setOpen(false);
        }
    }

    onDestroy(clearPanelSettleTimer);

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

<div class="quick-orchestration" class:desktop class:open>
    <button
        class="orchestration-toggle settings-toggle mobile-top-action mobile-top-action-right"
        class:active={open && view === 'settings'}
        type="button"
        bind:this={settingsButton}
        aria-label={$tr('quick.toggle')}
        aria-expanded={open && view === 'settings'}
        aria-pressed={open && view === 'settings'}
        aria-controls="orchestration-quick-drawer"
        onclick={() => void showSettings('settings')}
    >
        <Menu class="orchestration-toggle-icon" aria-hidden="true" />
        {#if orchestrationState.dirty_room_config}
            <span class="dirty-dot" aria-label={$tr('quick.dirty')}></span>
        {/if}
    </button>

    {#if desktop}
        <button
            class="orchestration-toggle panel-toggle mobile-top-action mobile-top-action-right"
            type="button"
            bind:this={panelToggleButton}
            aria-label={open ? $tr('quick.panel.close') : $tr('quick.panel.open')}
            aria-expanded={open}
            aria-controls="orchestration-quick-drawer"
            onclick={() => void setOpen(!open, true, 'panel')}
        >
            {#if open}
                <PanelRightClose class="orchestration-toggle-icon" aria-hidden="true" />
            {:else}
                <PanelRightOpen class="orchestration-toggle-icon" aria-hidden="true" />
            {/if}
        </button>
    {/if}

    <div
        id="orchestration-quick-drawer"
        class="quick-drawer"
        class:open
        class:desktop
        data-view={view}
        class:utility-dragging={panelGesture === 'dragging'}
        class:utility-settling={panelGesture === 'settling'}
        bind:this={drawerElement}
        style:--utility-drag-x={`${String(panelDragX)}px`}
        tabindex="-1"
        role={desktop ? 'complementary' : 'dialog'}
        aria-modal={!desktop}
        aria-hidden={!open}
        inert={!open}
        aria-labelledby="orchestration-quick-title"
        onpointerdown={handlePanelPointerDown}
        onpointermove={handlePanelPointerMove}
        onpointerup={handlePanelPointerUp}
        onpointercancel={handlePanelPointerCancel}
        onclickcapture={handlePanelClickCapture}
    >
        <header>
            <div class="quick-drawer-heading">
                {#if view === 'settings'}
                    <button
                        class="icon-button drawer-back-button"
                        type="button"
                        aria-label={$tr('quick.panel.back')}
                        onclick={() => void showTools()}
                    >
                        <ArrowLeft class="quick-drawer-back-icon" aria-hidden="true" />
                    </button>
                {/if}
                <div>
                    <p class="eyebrow">
                        {view === 'settings' ? $tr('quick.eyebrow') : $tr('quick.panel.eyebrow')}
                    </p>
                    <h3 id="orchestration-quick-title">
                        {view === 'settings' ? $tr('quick.title') : $tr('quick.panel.title')}
                    </h3>
                </div>
            </div>
            <button
                class="icon-button drawer-dismiss-button"
                type="button"
                aria-label={$tr('quick.panel.dismiss')}
                onclick={(event) => void setOpen(false, event.detail === 0)}
            >
                <X class="quick-drawer-close-icon" aria-hidden="true" />
            </button>
        </header>

        <div class="drawer-body">
            {#if view === 'tools'}
                <div class="utility-panel-home">
                    <p>{$tr('quick.panel.description')}</p>

                    <section
                        class="utility-summary-section"
                        aria-labelledby="utility-summary-title"
                    >
                        <h4 id="utility-summary-title">{$tr('quick.panel.summary.title')}</h4>
                        <dl class="utility-summary-card">
                            <div>
                                <dt>
                                    <MessagesSquare aria-hidden="true" />{$tr(
                                        'quick.panel.summary.mode',
                                    )}
                                </dt>
                                <dd>
                                    {appState.conversation_state?.selected_mode === 'story'
                                        ? $tr('quick.panel.summary.mode.story')
                                        : $tr('quick.panel.summary.mode.chat')}
                                </dd>
                            </div>
                            <div>
                                <dt>
                                    <SlidersHorizontal aria-hidden="true" />{$tr('quick.model')}
                                </dt>
                                <dd>
                                    {selectedModelRoute?.display_name ??
                                        selectedModelRoute?.model_id ??
                                        $tr('quick.panel.summary.auto')}
                                </dd>
                            </div>
                            <div>
                                <dt><Brain aria-hidden="true" />{$tr('quick.reasoning')}</dt>
                                <dd>{formatReasoningEffort(roomConfig.reasoning_effort)}</dd>
                            </div>
                            <div>
                                <dt>
                                    <Database aria-hidden="true" />{$tr(
                                        'quick.panel.summary.context',
                                    )}
                                </dt>
                                <dd>
                                    {roomConfig.memory_enabled
                                        ? $tr('quick.panel.summary.memory_on')
                                        : $tr('quick.panel.summary.memory_off')} · {roomConfig.knowledge_enabled
                                        ? $tr('quick.panel.summary.knowledge_on')
                                        : $tr('quick.panel.summary.knowledge_off')}
                                </dd>
                            </div>
                        </dl>
                    </section>

                    <section class="utility-tool-section" aria-labelledby="utility-tools-title">
                        <h4 id="utility-tools-title">{$tr('quick.panel.tools')}</h4>
                        <button
                            class="utility-tool-card"
                            type="button"
                            aria-label={$tr('quick.panel.settings.open')}
                            onclick={() => void showSettings()}
                        >
                            <span class="utility-tool-icon" aria-hidden="true"><Menu /></span>
                            <span>
                                <strong>{$tr('quick.title')}</strong>
                                <small>{$tr('quick.panel.settings.description')}</small>
                            </span>
                            <ChevronRight aria-hidden="true" />
                        </button>
                    </section>

                    {#if selectedPromptPreset !== null}
                        <p class="utility-active-preset">
                            프롬프트 프리셋 <strong>{selectedPromptPreset.name}</strong> 사용 중
                        </p>
                    {/if}

                    {#if roomControls}
                        <section class="utility-room-section" aria-labelledby="utility-room-title">
                            <h4 id="utility-room-title">대화</h4>
                            {@render roomControls(() => setOpen(false, false))}
                        </section>
                    {/if}
                </div>
            {:else}
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
                        <SegmentedControl
                            id="response-length"
                            label={$tr('quick.length')}
                            value={roomConfig.response_length}
                            disabled={!roomConfig.supported_fields.response_length}
                            options={[
                                { value: 'short', label: $tr('quick.length.short') },
                                { value: 'balanced', label: $tr('quick.length.balanced') },
                                { value: 'long', label: $tr('quick.length.long') },
                            ]}
                            onSelect={(value: string) =>
                                controller.stageRoomConfig({
                                    response_length:
                                        value as RoomOrchestrationConfigDto['response_length'],
                                })}
                        />
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
                        <ToggleSwitch
                            label={$tr('quick.memory')}
                            checked={roomConfig.memory_enabled}
                            disabled={!roomConfig.supported_fields.memory_enabled}
                            showLabel
                            onChange={(checked: boolean) =>
                                controller.stageRoomConfig({
                                    memory_enabled: checked,
                                })}
                        />
                        <ToggleSwitch
                            label={$tr('quick.knowledge')}
                            checked={roomConfig.knowledge_enabled}
                            disabled={!roomConfig.supported_fields.knowledge_enabled}
                            showLabel
                            onChange={(checked: boolean) =>
                                controller.stageRoomConfig({
                                    knowledge_enabled: checked,
                                })}
                        />
                    </fieldset>

                    {#if orchestrationState.workspace.creator_controls.length > 0}
                        <fieldset>
                            <legend>{$tr('quick.creator_controls')}</legend>
                            <div class="creator-controls">
                                {#each orchestrationState.workspace.creator_controls.slice(0, 80) as control (control.id)}
                                    {#if control.kind === 'toggle'}
                                        <ToggleSwitch
                                            label={control.label}
                                            checked={Boolean(controlValue(control))}
                                            showLabel
                                            onChange={(checked: boolean) =>
                                                controller.stageCreatorControl(control.id, checked)}
                                        />
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
                                                    controller.stageCreatorControl(
                                                        control.id,
                                                        value,
                                                    )}
                                            />
                                        </div>
                                    {:else if control.kind === 'multi_select'}
                                        <fieldset class="nested-fieldset">
                                            <legend>{control.label}</legend>
                                            {#each control.choices.slice(0, 40) as choice (choice)}
                                                <ToggleSwitch
                                                    label={choice}
                                                    checked={selectedValues(control).includes(
                                                        choice,
                                                    )}
                                                    showLabel
                                                    onChange={(checked: boolean) =>
                                                        toggleMultiChoice(control, choice, checked)}
                                                />
                                            {/each}
                                        </fieldset>
                                    {:else if control.kind === 'number' || control.kind === 'slider'}
                                        <label>
                                            <span>{control.label}</span>
                                            <input
                                                type={control.kind === 'slider'
                                                    ? 'range'
                                                    : 'number'}
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
            {/if}
        </div>
    </div>
</div>

<style>
    .quick-orchestration {
        position: relative;
    }

    .quick-orchestration.desktop {
        display: flex;
        align-items: center;
        gap: 2px;
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

    .quick-orchestration.desktop .orchestration-toggle {
        width: 32px;
        height: 32px;
        min-width: 32px;
        min-height: 32px;
        border-radius: var(--radius-sm);
        background: transparent;
        box-shadow: none;
    }

    .quick-orchestration.desktop .orchestration-toggle:hover:not(:disabled),
    .quick-orchestration.desktop .orchestration-toggle.active,
    .quick-orchestration.desktop .panel-toggle[aria-expanded='true'] {
        background: var(--desktop-hover-bg);
    }

    .quick-orchestration.desktop .orchestration-toggle :global(.orchestration-toggle-icon) {
        width: 18px;
        height: 18px;
        stroke-width: 1.8;
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

    .quick-drawer {
        position: fixed;
        z-index: 41;
        top: 0;
        right: 0;
        bottom: 0;
        left: auto;
        display: grid;
        grid-template-rows: auto minmax(0, 1fr);
        width: min(100%, 390px);
        height: 100dvh;
        max-height: 100dvh;
        overflow: hidden;
        border: 0;
        border-radius: 0;
        background: var(--bg);
        box-shadow: -12px 0 36px color-mix(in srgb, var(--brand-ink) 12%, transparent);
        pointer-events: none;
        transform: translate3d(100%, 0, 0);
        transition:
            transform var(--panel-close-duration) var(--panel-close-easing),
            visibility 0s linear var(--panel-close-duration);
        visibility: hidden;
        will-change: transform;
    }

    .quick-drawer.open {
        pointer-events: auto;
        transform: translate3d(0, 0, 0);
        transition:
            transform var(--panel-open-duration) var(--panel-open-easing),
            visibility 0s;
        visibility: visible;
    }

    .quick-drawer.desktop {
        top: 58px;
        right: var(--chat-utility-edge-inset, 12px);
        bottom: auto;
        width: var(--chat-utility-width, clamp(292px, 27vw, 320px));
        height: calc(100dvh - 70px);
        max-height: calc(100dvh - 70px);
        border: 1px solid var(--desktop-divider);
        border-radius: 18px;
        background: var(--desktop-panel-bg, var(--desktop-sidebar-bg));
        box-shadow: 0 12px 36px rgb(0 0 0 / 16%);
    }

    .quick-drawer:focus {
        outline: none;
    }

    .quick-drawer.utility-settling {
        pointer-events: none;
        transform: translate3d(var(--utility-drag-x, 0px), 0, 0);
        transition: transform 260ms cubic-bezier(0.22, 0.61, 0.36, 1);
        visibility: visible;
    }

    .quick-drawer.utility-dragging {
        cursor: grabbing;
        transform: translate3d(var(--utility-drag-x, 0px), 0, 0);
        transition: none;
        user-select: none;
        visibility: visible;
    }

    .quick-drawer > header {
        display: flex;
        gap: 12px;
        align-items: center;
        justify-content: space-between;
        min-height: 52px;
        padding: calc(8px + env(safe-area-inset-top)) 14px 8px;
        border-bottom: 0;
    }

    .quick-drawer-heading {
        display: flex;
        min-width: 0;
        align-items: center;
        gap: 4px;
    }

    .quick-drawer.desktop > header {
        min-height: 46px;
        padding: 6px 10px 6px 14px;
        border-bottom: 1px solid var(--desktop-divider);
    }

    .quick-drawer.desktop .drawer-dismiss-button {
        display: none;
    }

    .drawer-body {
        min-height: 0;
        padding-bottom: calc(14px + env(safe-area-inset-bottom));
        overflow-y: auto;
        overscroll-behavior: contain;
    }

    .quick-drawer.desktop .drawer-body {
        padding-bottom: 14px;
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

    .icon-button :global(.quick-drawer-close-icon),
    .icon-button :global(.quick-drawer-back-icon) {
        width: 20px;
        height: 20px;
    }

    .drawer-back-button {
        width: 32px;
        height: 32px;
        min-width: 32px;
        padding: 4px;
    }

    .utility-panel-home {
        display: grid;
        padding: 8px 14px 18px;
        align-content: start;
        gap: 18px;
    }

    .utility-panel-home > p {
        padding: 0 2px;
        color: var(--ink-muted);
        font-size: 0.8125rem;
        line-height: 1.45;
    }

    .utility-summary-section,
    .utility-tool-section,
    .utility-room-section {
        display: grid;
        gap: 8px;
    }

    .utility-summary-section > h4,
    .utility-tool-section > h4,
    .utility-room-section > h4 {
        padding-inline: 2px;
        color: var(--ink-muted);
        font-size: 0.75rem;
        font-weight: 650;
    }

    .utility-summary-card {
        overflow: hidden;
        padding: 0;
        border: 1px solid var(--line);
        border-radius: var(--radius-md);
        margin: 0;
        background: var(--surface-raised);
    }

    .utility-summary-card > div {
        display: grid;
        min-height: 45px;
        align-items: center;
        padding: 7px 10px;
        grid-template-columns: minmax(0, 1fr) minmax(0, 1.15fr);
        gap: 10px;
    }

    .utility-summary-card > div + div {
        border-top: 1px solid var(--line);
    }

    .utility-summary-card :is(dt, dd) {
        min-width: 0;
        margin: 0;
        font-size: 0.75rem;
    }

    .utility-summary-card dt {
        display: flex;
        align-items: center;
        color: var(--ink-muted);
        gap: 7px;
    }

    .utility-summary-card dt :global(svg) {
        width: 15px;
        height: 15px;
        flex: none;
        stroke-width: 1.8;
    }

    .utility-summary-card dd {
        overflow: hidden;
        color: var(--ink);
        font-weight: 550;
        text-align: right;
        text-overflow: ellipsis;
        white-space: nowrap;
    }

    .utility-active-preset {
        padding: 9px 10px;
        border: 1px solid var(--line);
        border-radius: var(--radius-md);
        color: var(--ink-muted);
        font-size: 0.75rem;
        line-height: 1.4;
    }

    .utility-active-preset strong {
        color: var(--ink);
        font-weight: 600;
    }

    .utility-room-section :global(.chat-room-controls) {
        gap: 8px;
    }

    .utility-tool-card {
        display: grid;
        width: 100%;
        min-height: 68px;
        grid-template-columns: 36px minmax(0, 1fr) 18px;
        align-items: center;
        padding: 10px 12px;
        border: 0;
        border-radius: var(--radius-md);
        background: var(--surface-raised);
        color: var(--ink);
        box-shadow: var(--shadow-1);
        gap: 10px;
        text-align: left;
    }

    .utility-tool-card > span:nth-child(2) {
        display: grid;
        min-width: 0;
        gap: 2px;
    }

    .utility-tool-card small {
        color: var(--ink-muted);
        font-size: 0.75rem;
        font-weight: 500;
    }

    .utility-tool-card > :global(svg) {
        width: 18px;
        height: 18px;
        color: var(--ink-muted);
    }

    .utility-tool-icon {
        display: grid;
        width: 36px;
        height: 36px;
        border-radius: var(--radius-sm);
        background: var(--surface-sunken);
        place-items: center;
    }

    .utility-tool-icon :global(svg) {
        width: 19px;
        height: 19px;
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
        opacity: var(--disabled-opacity);
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
        border: 1px solid var(--status-info-border);
        border-radius: var(--radius-sm);
        background: var(--status-info-bg);
    }

    .drawer-status.warning {
        border-color: var(--status-warning-border);
        color: var(--status-warning-fg);
        background: var(--status-warning-bg);
    }

    .drawer-status.error {
        border-color: var(--status-error-border);
        color: var(--status-error-fg);
        background: var(--status-error-bg);
    }

    @container view (max-width: 640px) {
        .quick-drawer {
            width: 100%;
            height: 100dvh;
            max-height: 100dvh;
        }
    }
</style>
