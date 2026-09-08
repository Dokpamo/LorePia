<script lang="ts">
    import { tick } from 'svelte';
    import type { LorepiaAppController, LorepiaAppState } from '../../app-controller';
    import {
        CAPABILITY_KEYS,
        type CapabilityKeyInput,
        type CapabilityObservationDto,
        type CapabilityOverrideStatusInput,
        type CapabilityOverrideValueInput,
        type UpsertCapabilityOverrideInput,
    } from '../../../lib/ipc/contracts';

    interface Props {
        onclose: () => void;
        appState: LorepiaAppState;
        controller: LorepiaAppController;
        detailMode?: string | null;
    }

    type OverrideValueKind = CapabilityOverrideValueInput['type'];

    const CAPABILITY_LABELS: Record<CapabilityKeyInput, string> = {
        streaming: t('workspaceAi.text148'),
        reasoning: t('workspaceAi.text149'),
        prompt_caching: t('workspaceAi.text150'),
        tool_calling: t('workspaceAi.text151'),
        parallel_tool_calling: t('workspaceAi.text152'),
        structured_output: t('workspaceAi.text153'),
        json_mode: t('workspaceAi.text154'),
        image_input: t('workspaceAi.text155'),
        audio_input: t('workspaceAi.text156'),
        audio_output: t('workspaceAi.text157'),
        logprobs: t('workspaceAi.text158'),
        seed: t('workspaceAi.text159'),
        batch: t('workspaceAi.text160'),
        background: t('workspaceAi.text161'),
        context_window: t('workspaceAi.text162'),
        max_output_tokens: t('workspaceAi.text163'),
    };

    let { appState, controller, onclose, detailMode = $bindable(null) }: Props = $props();
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

    let originalDraft = $state('');
    function draftSnapshot() {
        return JSON.stringify([
            overrideId,
            overrideKey,
            overrideValueKind,
            booleanValue,
            integerValue,
            enumValues,
            overrideStatus,
            expiresAt,
        ]);
    }
    const dirty = $derived(
        (detailMode === 'override-create' || detailMode === 'override-edit') &&
            draftSnapshot() !== originalDraft,
    );

    function isCapabilityKey(value: string): value is CapabilityKeyInput {
        return (CAPABILITY_KEYS as readonly string[]).includes(value);
    }

    function isOverrideStatus(value: string): value is CapabilityOverrideStatusInput {
        return ['verified', 'unsupported', 'unknown', 'conditional'].includes(value);
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

    function beginCreate(): void {
        if (busy || !routeIsLoaded) return;
        resetOverrideForm();
        detailMode = 'override-create';
        originalDraft = draftSnapshot();
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
        originalDraft = draftSnapshot();
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
                formError = t('workspaceAi.text172');
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
            formError = t('workspaceAi.text173');
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
            formError = t('workspaceAi.text174');
            return;
        }
        formError = null;
        const value = overrideValue();
        if (value === null) return;

        let expiresAtValue: string | null = null;
        if (expiresAt !== '') {
            const expiry = new Date(expiresAt);
            if (Number.isNaN(expiry.getTime())) {
                formError = t('workspaceAi.text175');
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

    import { t, tr } from '../../../lib/i18n';
    import SettingsPanel from './AiPanel.svelte';
    import EditField from '../../../ui/workspace/EditField.svelte';
    import AiChoice from './AiChoice.svelte';
    import AiAction from './AiAction.svelte';
    import AiLink from './AiLink.svelte';
    import AiReview from './AiReview.svelte';
</script>

<SettingsPanel
    {appState}
    {controller}
    title={$tr('workspaceAi.capabilities')}
    {onclose}
    covered={detailMode === 'override-create' ||
        detailMode === 'override-edit' ||
        detailMode === 'override-readonly'}
    disabled={busy}
>
    <section class="ui-settings-group" inert={busy}>
        <AiChoice
            label={$tr('workspaceAi.model')}
            value={selectedRouteId}
            options={workspace.routes.map((r) => ({
                value: r.id,
                label: r.display_name ?? r.model_id,
            }))}
            onselect={(v: string) => void loadRoute(v)}
        />
        <AiChoice
            label={$tr('workspaceAi.text132')}
            value={selectedCapabilityKey}
            options={CAPABILITY_KEYS.map((value) => ({ value, label: CAPABILITY_LABELS[value] }))}
            onselect={(v: string) => {
                if (isCapabilityKey(v)) selectedCapabilityKey = v;
            }}
        />
        <AiAction
            label={$tr('workspaceAi.text133')}
            disabled={!routeIsLoaded}
            onclick={() => void inspectCapability()}
        />
        {#if effectiveCapability}<AiReview
                label={$tr('workspaceAi.text134')}
                value={effectiveCapability}
            />{/if}
        {#if routeIsLoaded}<AiReview
                label={$tr('workspaceAi.text135')}
                value={observations}
            /><AiReview label={$tr('workspaceAi.text136')} value={parameterSpecs} />
            <AiAction label={$tr('workspaceAi.text137')} onclick={beginCreate} />
            {#each userOverrides as observation (observation.id)}<AiLink
                    label={observation.key}
                    onclick={() =>
                        observation.value.type === 'structured'
                            ? viewReadOnlyOverride(observation)
                            : editOverride(observation)}
                />{/each}
        {/if}
    </section>
</SettingsPanel>

{#if detailMode === 'override-create' || detailMode === 'override-edit' || detailMode === 'override-readonly'}
    <SettingsPanel
        {appState}
        {controller}
        {dirty}
        title={$tr('workspaceAi.capabilities')}
        onclose={() => (detailMode = 'overrides')}
        disabled={busy}
    >
        <section class="ui-settings-group" inert={busy}>
            {#if detailMode === 'override-create' || detailMode === 'override-edit'}
                <AiChoice
                    label={$tr('workspaceAi.text138')}
                    value={overrideKey}
                    options={CAPABILITY_KEYS.map((value) => ({
                        value,
                        label: CAPABILITY_LABELS[value],
                    }))}
                    onselect={(v: string) => {
                        if (isCapabilityKey(v)) overrideKey = v;
                    }}
                />
                <AiChoice
                    label={$tr('workspaceAi.text139')}
                    value={overrideValueKind}
                    options={['boolean', 'integer', 'enum_values'].map((value) => ({
                        value,
                        label: value,
                    }))}
                    onselect={(v: string) => (overrideValueKind = v as OverrideValueKind)}
                />
                {#if overrideValueKind === 'boolean'}<AiChoice
                        label={$tr('workspaceAi.text140')}
                        value={String(booleanValue)}
                        options={[
                            { value: 'true', label: $tr('workspaceAi.yes') },
                            { value: 'false', label: $tr('workspaceAi.no') },
                        ]}
                        onselect={(v: string) => (booleanValue = v === 'true')}
                    />
                {:else if overrideValueKind === 'integer'}<EditField
                        label={$tr('workspaceAi.text141')}
                        value={String(integerValue)}
                        maxlength={20}
                        onchange={(v: string) => (integerValue = Number(v))}
                    />
                {:else}<EditField
                        label={$tr('workspaceAi.text142')}
                        value={enumValues}
                        maxlength={4096}
                        onchange={(v: string) => (enumValues = v)}
                    />{/if}
                <AiChoice
                    label={$tr('workspaceAi.text143')}
                    value={overrideStatus}
                    options={['verified', 'unsupported', 'unknown', 'conditional'].map((value) => ({
                        value,
                        label: value,
                    }))}
                    onselect={(v: string) => {
                        if (isOverrideStatus(v)) overrideStatus = v;
                    }}
                />
                <EditField
                    label={$tr('workspaceAi.text144')}
                    value={expiresAt}
                    maxlength={40}
                    onchange={(v: string) => (expiresAt = v)}
                />
                <AiAction label={$tr('workspaceAi.save')} onclick={() => void saveOverride()} />
            {/if}
            {#if detailMode === 'override-readonly' && selectedUserOverride}<AiReview
                    label={$tr('workspaceAi.text145')}
                    value={selectedUserOverride}
                />{/if}
            {#if overrideId}
                <AiAction
                    label={$tr('workspaceAi.text146')}
                    onclick={() => (deleteConfirmationId = overrideId)}
                />
                {#if deleteConfirmationId === overrideId}<AiAction
                        label={$tr('workspaceAi.text147')}
                        onclick={() => void deleteEditingOverride()}
                    />{/if}
            {/if}
        </section>
        {#if formError}<p class="ui-field-error" role="alert">{formError}</p>{/if}
    </SettingsPanel>
{/if}
