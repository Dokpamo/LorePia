<script lang="ts">
    import { t, tr } from '../../../lib/i18n';
    import type { LorepiaAppController, LorepiaAppState } from '../../app-controller';
    import type {
        ApiFamilyInput,
        ModelAvailabilityInput,
        ProviderConfigEntryDto,
        UpsertModelRouteInput,
    } from '../../../lib/ipc/contracts';
    import EditField from '../../../ui/workspace/EditField.svelte';
    import AiChoice from './AiChoice.svelte';
    import AiAction from './AiAction.svelte';
    import AiLink from './AiLink.svelte';
    import SettingsPanel from './AiPanel.svelte';
    let {
        appState,
        controller,
        onclose,
    }: { appState: LorepiaAppState; controller: LorepiaAppController; onclose: () => void } =
        $props();
    let detailMode = $state<string | null>(null);
    const workspace = $derived(appState.providers.workspace);
    const retainedLegacyProfileIds = $derived(new Set(workspace.legacy_profiles.map((p) => p.id)));
    const ordinaryConnections = $derived(
        workspace.connections.filter((c) => !retainedLegacyProfileIds.has(c.id)),
    );
    const ordinaryRoutes = $derived(
        workspace.routes.filter((r) => !retainedLegacyProfileIds.has(r.connection_id)),
    );
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

    let originalDraft = $state('');
    function draftSnapshot() {
        return JSON.stringify(
            detailMode === 'create'
                ? [
                      routeConnectionId,
                      routeId,
                      routeApiFamily,
                      routeModelId,
                      routeDisplayName,
                      routeDeploymentId,
                      routeRegion,
                      routeEndpointPath,
                      routeValuesJson,
                      routeStatus,
                  ]
                : [selectedRouteId, updateRouteDisplayName, updateRouteStatus],
        );
    }
    const dirty = $derived(detailMode !== null && draftSnapshot() !== originalDraft);

    function optionalText(value: string): string | null {
        const normalized = value.trim();
        return normalized === '' ? null : normalized;
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
            throw new Error(t('workspaceAi.text41', { label }));
        }
        if (!Array.isArray(parsed)) {
            throw new Error(t('workspaceAi.text42', { label }));
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
            throw new Error(t('workspaceAi.text43', { label }));
        }
        return entries as ProviderConfigEntryDto[];
    }

    function resetConfirmations() {
        confirmRouteDelete = false;
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
        originalDraft = draftSnapshot();
    }

    function openRouteEdit(id: string): void {
        routeError = '';
        selectRoute(id);
        detailMode = 'edit';
        originalDraft = draftSnapshot();
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
                    values: parseConfigValues(routeValuesJson, t('workspaceAi.text56')),
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
            routeError = error instanceof Error ? error.message : t('workspaceAi.text57');
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

    function back() {
        if (detailMode) detailMode = null;
        else onclose();
    }
</script>

<SettingsPanel
    {appState}
    {controller}
    title={$tr('workspaceAi.model')}
    {onclose}
    covered={detailMode !== null}
    disabled={routeBusy}
    ><div inert={routeBusy}>
        <section class="ui-settings-group">
            {#each ordinaryRoutes as r (r.id)}<AiLink
                    label={r.display_name ?? r.model_id}
                    onclick={() => {
                        openRouteEdit(r.id);
                    }}
                />
            {/each}
        </section>
        <AiAction
            label={$tr('workspaceAi.text44')}
            onclick={() => {
                openRouteCreate();
            }}
        />
    </div>
</SettingsPanel>

{#if detailMode !== null}
    <SettingsPanel
        {appState}
        {controller}
        {dirty}
        title={$tr('workspaceAi.model')}
        onclose={back}
        disabled={routeBusy}
        ><div inert={routeBusy}>
            <section class="ui-settings-group">
                {#if detailMode === 'create'}<AiChoice
                        label={$tr('workspaceAi.text45')}
                        value={routeConnectionId}
                        options={ordinaryConnections.map((c) => ({
                            value: c.id,
                            label: c.display_name,
                        }))}
                        onselect={(v: string) => (routeConnectionId = v)}
                    />
                    <AiChoice
                        label={$tr('workspaceAi.text46')}
                        value={routeApiFamily}
                        options={[
                            'open_ai_responses',
                            'open_ai_chat_completions',
                            'anthropic_messages',
                            'gemini_generate_content',
                            'ollama_native',
                        ].map((value) => ({ value, label: value }))}
                        onselect={(v: string) => (routeApiFamily = v as ApiFamilyInput)}
                    />
                    <EditField
                        label={$tr('workspaceAi.text47')}
                        value={routeId}
                        maxlength={65536}
                        onchange={(v: string) => (routeId = v)}
                    />
                    <EditField
                        label={$tr('workspaceAi.text48')}
                        value={routeModelId}
                        maxlength={65536}
                        onchange={(v: string) => (routeModelId = v)}
                    />
                    <EditField
                        label={$tr('workspaceAi.text16')}
                        value={routeDisplayName}
                        maxlength={65536}
                        onchange={(v: string) => (routeDisplayName = v)}
                    />
                    <EditField
                        label={$tr('workspaceAi.text49')}
                        value={routeDeploymentId}
                        maxlength={65536}
                        onchange={(v: string) => (routeDeploymentId = v)}
                    />
                    <EditField
                        label={$tr('workspaceAi.text50')}
                        value={routeRegion}
                        maxlength={65536}
                        onchange={(v: string) => (routeRegion = v)}
                    />
                    <EditField
                        label={$tr('workspaceAi.text51')}
                        value={routeEndpointPath}
                        maxlength={65536}
                        onchange={(v: string) => (routeEndpointPath = v)}
                    />
                    <EditField
                        label={$tr('workspaceAi.text52')}
                        value={routeValuesJson}
                        maxlength={65536}
                        onchange={(v: string) => (routeValuesJson = v)}
                    />
                {:else}<EditField
                        label={$tr('workspaceAi.text16')}
                        value={updateRouteDisplayName}
                        maxlength={65536}
                        onchange={(v: string) => (updateRouteDisplayName = v)}
                    />
                {/if}<AiChoice
                    label={$tr('workspaceAi.text53')}
                    value={detailMode === 'create' ? routeStatus : updateRouteStatus}
                    options={[
                        'available',
                        'missing_temporarily',
                        'documented_only',
                        'access_denied',
                        'deprecated',
                        'retired',
                        'unknown',
                    ].map((value) => ({ value, label: value }))}
                    onselect={(v: string) => {
                        if (detailMode === 'create') routeStatus = v as ModelAvailabilityInput;
                        else updateRouteStatus = v as ModelAvailabilityInput;
                    }}
                />
            </section>
            <AiAction
                label={$tr('workspaceAi.save')}
                onclick={() => {
                    void (detailMode === 'create' ? createRoute() : updateRoute());
                }}
            />
            {#if detailMode === 'edit'}<AiAction
                    label={$tr('workspaceAi.text54')}
                    onclick={() => {
                        confirmRouteDelete = true;
                    }}
                />
                {#if confirmRouteDelete}<AiAction
                        label={$tr('workspaceAi.text55')}
                        onclick={() => {
                            void deleteRoute();
                        }}
                    />
                {/if}{/if}
        </div>
        {#if routeError}<p class="ui-field-error" role="alert">{routeError}</p>{/if}</SettingsPanel
    >
{/if}
