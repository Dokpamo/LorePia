<script lang="ts">
    import { t, tr } from '../../../lib/i18n';
    import type { LorepiaAppController, LorepiaAppState } from '../../app-controller';
    import type {
        CreateProviderConnectionInput,
        ProviderConfigEntryDto,
        ProviderNetworkModeInput,
        UpdateProviderConnectionInput,
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

    let credentialDelete = $state(false);
    async function captureCredential() {
        connectionBusy = true;
        try {
            await controller.captureProviderCredential({
                kind: 'connection',
                connection_id: selectedConnectionId,
            });
        } finally {
            connectionBusy = false;
        }
    }
    async function removeCredential() {
        connectionBusy = true;
        try {
            await controller.deleteProviderCredential({
                kind: 'connection',
                connection_id: selectedConnectionId,
            });
            credentialDelete = false;
        } finally {
            connectionBusy = false;
        }
    }
    const selectedConnectionIsRetainedLegacy = $derived(
        retainedLegacyProfileIds.has(selectedConnectionId),
    );
    const selectedTemplate = $derived(
        workspace.templates.find((t) => t.id === connectionTemplateId),
    );
    function resetConfirmations() {
        confirmConnectionDelete = false;
        credentialDelete = false;
    }

    let originalDraft = $state('');
    function draftSnapshot() {
        return JSON.stringify(
            detailMode === 'create'
                ? [
                      connectionTemplateId,
                      connectionId,
                      connectionDisplayName,
                      connectionOrigin,
                      connectionBasePath,
                      connectionNetworkMode,
                      connectionLocalOrigin,
                      connectionLocalAddresses,
                      connectionValuesJson,
                      connectionApprovedCredentialOrigin,
                      connectionTimeout,
                  ]
                : [selectedConnectionId, updateConnectionDisplayName, updateConnectionTimeout],
        );
    }
    const dirty = $derived(detailMode !== null && draftSnapshot() !== originalDraft);

    function optionalText(value: string): string | null {
        const normalized = value.trim();
        return normalized === '' ? null : normalized;
    }

    function positiveInteger(value: string, label: string): number | null {
        const parsed = Number(value);
        if (!Number.isInteger(parsed) || parsed <= 0) {
            throw new Error(t('workspaceAi.text38', { label }));
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

    function openConnectionCreate(): void {
        resetConnectionCreateForm();
        connectionError = '';
        resetConfirmations();
        detailMode = 'create';
        originalDraft = draftSnapshot();
    }

    function openConnectionEdit(id: string): void {
        connectionError = '';
        selectConnection(id);
        detailMode = 'edit';
        originalDraft = draftSnapshot();
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
                throw new Error(t('workspaceAi.text32'));
            }
            const timeoutSeconds = positiveInteger(connectionTimeout, t('workspaceAi.text33'));
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
                throw new Error(t('workspaceAi.text34'));
            }
            const values = parseConfigValues(connectionValuesJson, t('workspaceAi.text35'));
            const credentialKeys = new Set(
                selectedTemplate.connection_fields
                    .filter((field) => field.value_type === 'credential')
                    .map((field) => field.key),
            );
            if (values.some((entry) => credentialKeys.has(entry.key))) {
                throw new Error(t('workspaceAi.text36'));
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
            connectionError = error instanceof Error ? error.message : t('workspaceAi.text37');
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
            const timeoutSeconds = positiveInteger(
                updateConnectionTimeout,
                t('workspaceAi.text33'),
            );
            if (timeoutSeconds === null) return;
            const input: UpdateProviderConnectionInput = {
                id: selectedConnectionId,
                display_name: updateConnectionDisplayName.trim(),
                timeout_seconds: timeoutSeconds,
            };
            if (await controller.updateProviderConnection(input)) detailMode = null;
        } catch (error: unknown) {
            connectionError = error instanceof Error ? error.message : t('workspaceAi.text37');
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

    function back() {
        if (detailMode) detailMode = null;
        else onclose();
    }
</script>

<SettingsPanel
    {appState}
    {controller}
    title={$tr('workspaceAi.connections')}
    {onclose}
    covered={detailMode !== null}
    disabled={connectionBusy}
    ><div inert={connectionBusy}>
        <section class="ui-settings-group">
            {#each ordinaryConnections as c (c.id)}<AiLink
                    label={c.display_name}
                    onclick={() => {
                        openConnectionEdit(c.id);
                    }}
                />
            {/each}
        </section>
        <AiAction
            label={$tr('workspaceAi.text13')}
            onclick={() => {
                openConnectionCreate();
            }}
        />
    </div>
</SettingsPanel>

{#if detailMode !== null}
    <SettingsPanel
        {appState}
        {controller}
        {dirty}
        title={$tr('workspaceAi.connections')}
        onclose={back}
        disabled={connectionBusy}
        ><div inert={connectionBusy}>
            <section class="ui-settings-group">
                {#if detailMode === 'create'}<AiChoice
                        label={$tr('workspaceAi.text14')}
                        value={connectionTemplateId}
                        options={workspace.templates.map((t) => ({
                            value: t.id,
                            label: t.display_name,
                        }))}
                        onselect={selectTemplate}
                    />
                    <EditField
                        label={$tr('workspaceAi.text15')}
                        value={connectionId}
                        maxlength={65536}
                        onchange={(v: string) => (connectionId = v)}
                    />
                    <EditField
                        label={$tr('workspaceAi.text16')}
                        value={connectionDisplayName}
                        maxlength={65536}
                        onchange={(v: string) => (connectionDisplayName = v)}
                    />
                    <EditField
                        label={$tr('workspaceAi.text17')}
                        value={connectionOrigin}
                        maxlength={65536}
                        onchange={(v: string) => (connectionOrigin = v)}
                    />
                    <EditField
                        label={$tr('workspaceAi.text18')}
                        value={connectionBasePath}
                        maxlength={65536}
                        onchange={(v: string) => (connectionBasePath = v)}
                    />
                    <EditField
                        label={$tr('workspaceAi.text19')}
                        value={connectionTimeout}
                        maxlength={65536}
                        onchange={(v: string) => (connectionTimeout = v)}
                    />
                    <EditField
                        label={$tr('workspaceAi.text20')}
                        value={connectionApprovedCredentialOrigin}
                        maxlength={65536}
                        onchange={(v: string) => (connectionApprovedCredentialOrigin = v)}
                    />
                    <AiChoice
                        label={$tr('workspaceAi.text21')}
                        value={connectionNetworkMode}
                        options={['public', 'local_loopback', 'approved_local_network'].map(
                            (value) => ({
                                value,
                                label: value,
                            }),
                        )}
                        onselect={(v: string) =>
                            (connectionNetworkMode = v as ProviderNetworkModeInput)}
                    />
                    {#if connectionNetworkMode === 'approved_local_network'}<EditField
                            label={$tr('workspaceAi.text22')}
                            value={connectionLocalOrigin}
                            maxlength={65536}
                            onchange={(v: string) => (connectionLocalOrigin = v)}
                        />
                        <EditField
                            label={$tr('workspaceAi.text23')}
                            value={connectionLocalAddresses}
                            maxlength={65536}
                            onchange={(v: string) => (connectionLocalAddresses = v)}
                        />
                    {/if}<EditField
                        label={$tr('workspaceAi.text24')}
                        value={connectionValuesJson}
                        maxlength={65536}
                        onchange={(v: string) => (connectionValuesJson = v)}
                    />
                {:else}<EditField
                        label={$tr('workspaceAi.text16')}
                        value={updateConnectionDisplayName}
                        maxlength={65536}
                        onchange={(v: string) => (updateConnectionDisplayName = v)}
                    />
                    <EditField
                        label={$tr('workspaceAi.text19')}
                        value={updateConnectionTimeout}
                        maxlength={65536}
                        onchange={(v: string) => (updateConnectionTimeout = v)}
                    />
                {/if}
            </section>
            <AiAction
                label={$tr('workspaceAi.save')}
                onclick={() => {
                    void (detailMode === 'create' ? createConnection() : updateConnection());
                }}
            />
            {#if detailMode === 'edit'}<AiAction
                    label={$tr('workspaceAi.text25')}
                    onclick={() => {
                        void captureCredential();
                    }}
                />
                <AiAction
                    label={$tr('workspaceAi.text26')}
                    onclick={() => {
                        credentialDelete = true;
                    }}
                />
                {#if credentialDelete}<AiAction
                        label={$tr('workspaceAi.text27')}
                        onclick={() => {
                            void removeCredential();
                        }}
                    />
                {/if}<AiAction
                    label={$tr('workspaceAi.text28')}
                    onclick={() => {
                        confirmConnectionDelete = true;
                    }}
                />
                {#if confirmConnectionDelete}<AiAction
                        label={$tr('workspaceAi.text29')}
                        onclick={() => {
                            void deleteConnection();
                        }}
                    />
                {/if}{/if}
        </div>
        {#if connectionError}<p class="ui-field-error" role="alert">
                {connectionError}
            </p>{/if}</SettingsPanel
    >
{/if}
