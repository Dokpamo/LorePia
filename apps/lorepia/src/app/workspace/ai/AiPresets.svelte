<script lang="ts">
    import { t, tr } from '../../../lib/i18n';
    import type { LorepiaAppController, LorepiaAppState } from '../../app-controller';
    import type { GenerationParameterDto, GenerationPresetInput } from '../../../lib/ipc/contracts';
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
    const ordinaryRoutes = $derived(
        workspace.routes.filter((r) => !retainedLegacyProfileIds.has(r.connection_id)),
    );
    const ordinaryPresets = $derived(
        workspace.presets.filter((p) => ordinaryRoutes.some((r) => r.id === p.model_route_id)),
    );
    import AiPreview from './AiPreview.svelte';
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

    let originalDraft = $state('');
    function draftSnapshot() {
        return JSON.stringify([
            presetRouteId,
            selectedPresetId,
            presetId,
            presetDisplayName,
            presetValuesJson,
            reasoningMode,
            reasoningEffort,
            reasoningBudgetTokens,
            reasoningSummary,
            reasoningPreserveOpaqueState,
            promptCacheMode,
            promptCacheTtlKind,
            promptCacheTtlSeconds,
            promptCacheContextReference,
        ]);
    }
    const dirty = $derived(detailMode !== null && draftSnapshot() !== originalDraft);

    function optionalText(value: string): string | null {
        const normalized = value.trim();
        return normalized === '' ? null : normalized;
    }

    function optionalNonNegativeInteger(value: unknown, label: string): number | null {
        if (value === null || value === undefined) return null;
        if (typeof value !== 'string' && typeof value !== 'number') {
            throw new Error(t('workspaceAi.text39', { label }));
        }
        const normalized = typeof value === 'string' ? value.trim() : value;
        if (normalized === '') return null;
        const parsed = Number(normalized);
        if (!Number.isInteger(parsed) || parsed < 0) {
            throw new Error(t('workspaceAi.text40', { label }));
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
            throw new Error(t('workspaceAi.text41', { label }));
        }
        if (!Array.isArray(parsed)) {
            throw new Error(t('workspaceAi.text42', { label }));
        }
        return parsed;
    }

    function parsePresetValues(text: string): GenerationParameterDto[] {
        const entries = parseJsonArray(text, t('workspaceAi.text30'));
        const valid = entries.every(
            (entry) =>
                isRecord(entry) &&
                typeof entry.parameter_id === 'string' &&
                isRecord(entry.state) &&
                (entry.state.state === 'inherit_provider_default' ||
                    (entry.state.state === 'explicit' && isRecord(entry.state.value))),
        );
        if (!valid) {
            throw new Error(t('workspaceAi.text31'));
        }
        return entries as GenerationParameterDto[];
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

    function openPresetCreate(): void {
        presetError = '';
        presetRouteId = '';
        clearPresetForm();
        detailMode = 'create';
        originalDraft = draftSnapshot();
    }

    function openPresetEdit(id: string): void {
        presetError = '';
        const preset = workspace.presets.find((candidate) => candidate.id === id);
        if (!preset) return;
        presetRouteId = preset.model_route_id;
        selectPreset(id);
        detailMode = 'edit';
        originalDraft = draftSnapshot();
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
            if (presetRouteId === '') throw new Error(t('workspaceAi.text73'));
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
            presetError = error instanceof Error ? error.message : t('workspaceAi.text74');
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
    function back() {
        if (detailMode) detailMode = null;
        else onclose();
    }
</script>

<SettingsPanel
    {appState}
    {controller}
    title={$tr('workspaceAi.preset')}
    {onclose}
    covered={detailMode !== null}
    disabled={presetBusy}
    ><div inert={presetBusy}>
        <section class="ui-settings-group">
            {#each ordinaryPresets as p (p.id)}<AiLink
                    label={p.display_name}
                    onclick={() => {
                        openPresetEdit(p.id);
                    }}
                />
            {/each}
        </section>
        <AiAction
            label={$tr('workspaceAi.text58')}
            onclick={() => {
                openPresetCreate();
            }}
        />
    </div>
</SettingsPanel>

{#if detailMode !== null}
    <SettingsPanel
        {appState}
        {controller}
        {dirty}
        title={$tr('workspaceAi.preset')}
        onclose={back}
        disabled={presetBusy}
        ><div inert={presetBusy}>
            <section class="ui-settings-group">
                {#if detailMode === 'create'}<AiChoice
                        label={$tr('workspaceAi.model')}
                        value={presetRouteId}
                        options={ordinaryRoutes.map((r) => ({
                            value: r.id,
                            label: r.display_name ?? r.model_id,
                        }))}
                        onselect={selectPresetRoute}
                    />
                    <EditField
                        label={$tr('workspaceAi.text59')}
                        value={presetId}
                        maxlength={65536}
                        onchange={(v: string) => (presetId = v)}
                    />
                {/if}<EditField
                    label={$tr('workspaceAi.text16')}
                    value={presetDisplayName}
                    maxlength={65536}
                    onchange={(v: string) => (presetDisplayName = v)}
                />
                <EditField
                    label={$tr('workspaceAi.text60')}
                    value={presetValuesJson}
                    maxlength={65536}
                    onchange={(v: string) => (presetValuesJson = v)}
                />
                <EditField
                    label={$tr('workspaceAi.text61')}
                    value={reasoningMode}
                    maxlength={65536}
                    onchange={(v: string) => (reasoningMode = v)}
                />
                <EditField
                    label={$tr('workspaceAi.text62')}
                    value={reasoningEffort}
                    maxlength={65536}
                    onchange={(v: string) => (reasoningEffort = v)}
                />
                <EditField
                    label={$tr('workspaceAi.text63')}
                    value={reasoningBudgetTokens}
                    maxlength={65536}
                    onchange={(v: string) => (reasoningBudgetTokens = v)}
                />
                <EditField
                    label={$tr('workspaceAi.text64')}
                    value={reasoningSummary}
                    maxlength={65536}
                    onchange={(v: string) => (reasoningSummary = v)}
                />
                <EditField
                    label={$tr('workspaceAi.text65')}
                    value={promptCacheMode}
                    maxlength={65536}
                    onchange={(v: string) => (promptCacheMode = v)}
                />
                <EditField
                    label={$tr('workspaceAi.text66')}
                    value={promptCacheTtlKind}
                    maxlength={65536}
                    onchange={(v: string) => (promptCacheTtlKind = v)}
                />
                <EditField
                    label={$tr('workspaceAi.text67')}
                    value={promptCacheTtlSeconds}
                    maxlength={65536}
                    onchange={(v: string) => (promptCacheTtlSeconds = v)}
                />
                <EditField
                    label={$tr('workspaceAi.text68')}
                    value={promptCacheContextReference}
                    maxlength={65536}
                    onchange={(v: string) => (promptCacheContextReference = v)}
                />
                <AiChoice
                    label={$tr('workspaceAi.text69')}
                    value={String(reasoningPreserveOpaqueState)}
                    options={[
                        { value: 'true', label: t('workspaceAi.yes') },
                        { value: 'false', label: t('workspaceAi.no') },
                    ]}
                    onselect={(v: string) => (reasoningPreserveOpaqueState = v === 'true')}
                />
            </section>
            <AiAction
                label={$tr('workspaceAi.text70')}
                onclick={() => {
                    void validatePreset();
                }}
            />
            <AiAction
                label={$tr('workspaceAi.preview')}
                onclick={() => {
                    void previewPreset();
                }}
            />
            <AiAction
                label={$tr('workspaceAi.save')}
                onclick={() => {
                    void savePreset();
                }}
            />
            {#if detailMode === 'edit'}<AiAction
                    label={$tr('workspaceAi.text71')}
                    onclick={() => {
                        confirmPresetDelete = true;
                    }}
                />
                {#if confirmPresetDelete}<AiAction
                        label={$tr('workspaceAi.text72')}
                        onclick={() => {
                            void deletePreset();
                        }}
                    />
                {/if}{/if}<AiPreview {appState} />
        </div>
        {#if presetError}<p class="ui-field-error" role="alert">
                {presetError}
            </p>{/if}</SettingsPanel
    >
{/if}
