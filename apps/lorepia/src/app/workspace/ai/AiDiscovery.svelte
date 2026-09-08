<script lang="ts">
    import AiDiscoveryAssistant from './AiDiscoveryAssistant.svelte';
    import AiDiscoveryGuidance from './AiDiscoveryGuidance.svelte';
    import AiDiscoveryActions from './AiDiscoveryActions.svelte';
    import { discoveryConnectionOptions as options } from '../../../features/providers/discovery-connection-options';
    import { tick } from 'svelte';
    import { tr } from '../../../lib/i18n';
    import {
        discoveryCredentialTarget,
        type LorepiaAppController,
        type LorepiaAppState,
    } from '../../app-controller';
    import type {
        BeginProviderDiscoveryCurlInput,
        BeginProviderDiscoveryInput,
        ContinueProviderDiscoveryActionInput,
        DiscoveryCandidateSummaryDto,
    } from '../../../lib/ipc/contracts';

    interface Props {
        appState: LorepiaAppState;
        controller: LorepiaAppController;
        onclose: () => void;
        nestedPage?: string | null;
        nestedTitle?: string;
    }

    let {
        appState,
        controller,
        onclose,
        nestedPage = $bindable(null),
        nestedTitle = $bindable(''),
    }: Props = $props();
    let sourceMode = $state<'site' | 'known_provider' | 'curl'>('site');
    let connectionId = $state('');
    let displayName = $state('');
    let siteUrl = $state('');
    let docsUrl = $state('');
    let templateId = $state('');
    let preferredAssistantId = $state('');
    let credentialRequested = $state(false);
    let documentEvidenceUrl = $state('');
    let unknownResolution = $state<
        'confirmed_no_effect' | 'confirmed_compensated' | 'manually_reconciled_as_failed'
    >('confirmed_no_effect');
    let busy = $state(false);
    let originalDraft = $state('');
    function draftSnapshot() {
        return JSON.stringify([
            sourceMode,
            connectionId,
            displayName,
            siteUrl,
            docsUrl,
            templateId,
            preferredAssistantId,
            credentialRequested,
        ]);
    }
    const dirty = $derived(nestedPage === 'create' && draftSnapshot() !== originalDraft);

    const workspace = $derived(appState.providers.workspace);
    const routedSessionId = $derived(
        nestedPage?.startsWith('session:') ? nestedPage.slice('session:'.length) : null,
    );
    const selectedSession = $derived(
        workspace.discoveries.find((session) => session.id === routedSessionId) ?? null,
    );
    const latestEvent = $derived(
        workspace.discovery_event?.session_id === selectedSession?.id
            ? workspace.discovery_event
            : null,
    );
    const actionKind = $derived(selectedSession?.action_required?.kind ?? null);
    const selectedCredentialTarget = $derived(
        selectedSession === null ? null : discoveryCredentialTarget(selectedSession),
    );
    const selectedCredentialStatus = $derived(
        selectedCredentialTarget === null
            ? null
            : (workspace.credential_statuses[
                  `discovery_session:${selectedCredentialTarget.session_id}`
              ] ?? 'missing'),
    );

    async function run(action: () => Promise<unknown>): Promise<void> {
        if (busy) return;
        busy = true;
        try {
            await action();
        } finally {
            busy = false;
        }
    }

    async function startDiscovery(): Promise<void> {
        if (busy || connectionId.trim() === '' || displayName.trim() === '') return;
        const previousSessionId = workspace.selected_discovery_id;
        busy = true;
        try {
            let started: boolean;
            if (sourceMode === 'curl') {
                const input: BeginProviderDiscoveryCurlInput = {
                    connection_id: connectionId.trim(),
                    display_name: displayName.trim(),
                    docs_url: docsUrl.trim() === '' ? null : docsUrl.trim(),
                    credential_binding_requested: credentialRequested,
                    preferred_assistant: preferredAssistantId === '' ? null : preferredAssistantId,
                    connection_options: options(),
                    supplied_evidence_ids: [],
                };
                started = await controller.beginProviderDiscovery({
                    kind: 'curl',
                    input,
                });
            } else {
                if (
                    siteUrl.trim() === '' ||
                    (sourceMode === 'known_provider' && templateId === '')
                ) {
                    return;
                }
                const input: BeginProviderDiscoveryInput = {
                    connection_id: connectionId.trim(),
                    display_name: displayName.trim(),
                    site_url: siteUrl.trim(),
                    docs_url: docsUrl.trim() === '' ? null : docsUrl.trim(),
                    credential_binding_requested: credentialRequested,
                    preferred_assistant: preferredAssistantId === '' ? null : preferredAssistantId,
                    connection_options: options(),
                    supplied_evidence_ids: [],
                    source:
                        sourceMode === 'known_provider'
                            ? { kind: 'known_provider', template_id: templateId }
                            : { kind: 'site' },
                };
                started = await controller.beginProviderDiscovery({ kind: 'site', input });
            }
            if (!started) return;
            await tick();
            const sessionId = workspace.selected_discovery_id;
            const session = workspace.discoveries.find((candidate) => candidate.id === sessionId);
            if (sessionId !== null && sessionId !== previousSessionId && session) {
                nestedTitle = session.display_name;
                nestedPage = `session:${sessionId}`;
            }
        } finally {
            busy = false;
        }
    }

    function beginCreate(): void {
        sourceMode = 'site';
        connectionId = '';
        displayName = '';
        siteUrl = '';
        docsUrl = '';
        templateId = '';
        preferredAssistantId = '';
        credentialRequested = false;
        nestedTitle = $tr('settings.page.discovery.create');
        nestedPage = 'create';
        originalDraft = draftSnapshot();
    }

    async function continueWith(action: ContinueProviderDiscoveryActionInput): Promise<void> {
        if (busy) return;
        busy = true;
        try {
            await controller.continueProviderDiscovery(action);
        } finally {
            busy = false;
        }
    }

    async function submitDocumentEvidence(): Promise<void> {
        if (busy) return;
        busy = true;
        try {
            if (await controller.supplyProviderDiscoveryDocumentEvidence(documentEvidenceUrl)) {
                documentEvidenceUrl = '';
            }
        } finally {
            busy = false;
        }
    }

    async function submitCurlEvidence(): Promise<void> {
        if (busy) return;
        busy = true;
        try {
            await controller.supplyProviderDiscoveryCurlEvidence();
        } finally {
            busy = false;
        }
    }

    async function commitDiscovery(): Promise<void> {
        if (busy) return;
        busy = true;
        try {
            await controller.commitProviderDiscovery();
        } finally {
            busy = false;
        }
    }

    async function captureDiscoveryCredential(): Promise<void> {
        const target = selectedCredentialTarget;
        if (busy || target === null) return;
        busy = true;
        try {
            await controller.captureProviderCredential(target);
        } finally {
            busy = false;
        }
    }

    function candidateLabel(summary: DiscoveryCandidateSummaryDto): string {
        switch (summary.kind) {
            case 'provider_template':
                return `${summary.template_id} v${String(summary.template_version)}`;
            case 'api_origin':
                return summary.origin;
            case 'official_document':
                return summary.url;
            case 'model_route':
                return summary.model_id;
            case 'manifest_draft':
                return `manifest v${String(summary.schema_version)}`;
        }
    }

    async function openSession(sessionId: string, title: string): Promise<void> {
        if (busy) return;
        busy = true;
        try {
            await controller.refreshProviderDiscovery(sessionId);
            if (nestedTitle !== title) nestedTitle = title;
            nestedPage = `session:${sessionId}`;
        } finally {
            busy = false;
        }
    }

    $effect(() => {
        if (nestedPage === 'create') {
            const title = $tr('settings.page.discovery.create');
            if (nestedTitle !== title) nestedTitle = title;
            return;
        }
        if (nestedPage === null) {
            if (nestedTitle !== '') nestedTitle = '';
            return;
        }
        if (routedSessionId === null) return;
        const session = workspace.discoveries.find((candidate) => candidate.id === routedSessionId);
        if (session) {
            if (nestedTitle !== session.display_name) nestedTitle = session.display_name;
            return;
        }
        nestedPage = null;
        nestedTitle = '';
    });

    import SettingsPanel from './AiPanel.svelte';
    import EditField from '../../../ui/workspace/EditField.svelte';
    import AiChoice from './AiChoice.svelte';
    import AiAction from './AiAction.svelte';
    import AiLink from './AiLink.svelte';
    import AiReview from './AiReview.svelte';
    import { onMount } from 'svelte';
    onMount(() => {
        void controller.loadProviderDiagnostics('discovery');
    });
</script>

<SettingsPanel
    {appState}
    {controller}
    title={$tr('settings.page.discovery.provider')}
    {onclose}
    covered={nestedPage !== null}
    disabled={busy}
    ><section class="ui-settings-group" inert={busy}>
        {#each workspace.discoveries as session (session.id)}<AiLink
                label={session.display_name}
                onclick={() => void openSession(session.id, session.display_name)}
            />{/each}<AiAction
            label={$tr('workspaceAi.text104')}
            disabled={busy}
            onclick={() => void run(() => controller.recoverProviderDiscoveries())}
        /><AiAction
            label={$tr('workspaceAi.text105')}
            disabled={busy}
            onclick={() => beginCreate()}
        />
    </section>
</SettingsPanel>

{#if nestedPage !== null}
    <SettingsPanel
        {appState}
        {controller}
        {dirty}
        title={nestedTitle || $tr('settings.page.discovery.provider')}
        onclose={() => {
            if (nestedPage) nestedPage = null;
            else onclose();
        }}
        disabled={busy}
        ><section class="ui-settings-group" inert={busy}>
            {#if nestedPage === 'create'}<EditField
                    label={$tr('workspaceAi.text15')}
                    value={connectionId}
                    maxlength={2048}
                    onchange={(v: string) => (connectionId = v)}
                /><EditField
                    label={$tr('workspaceAi.text16')}
                    value={displayName}
                    maxlength={2048}
                    onchange={(v: string) => (displayName = v)}
                /><AiChoice
                    label={$tr('workspaceAi.text76')}
                    value={sourceMode}
                    options={['site', 'known_provider', 'curl'].map((value) => ({
                        value,
                        label: value,
                    }))}
                    onselect={(v: string) => (sourceMode = v as typeof sourceMode)}
                />
                {#if sourceMode !== 'curl'}<EditField
                        label={$tr('workspaceAi.text77')}
                        value={siteUrl}
                        maxlength={2048}
                        onchange={(v: string) => (siteUrl = v)}
                    />{/if}
                {#if sourceMode === 'known_provider'}<AiChoice
                        label={$tr('workspaceAi.text14')}
                        value={templateId}
                        options={workspace.templates.map((t) => ({
                            value: t.id,
                            label: t.display_name,
                        }))}
                        onselect={(v: string) => (templateId = v)}
                    />{/if}<EditField
                    label={$tr('workspaceAi.text78')}
                    value={docsUrl}
                    maxlength={2048}
                    onchange={(v: string) => (docsUrl = v)}
                /><EditField
                    label={$tr('workspaceAi.text79')}
                    value={preferredAssistantId}
                    maxlength={2048}
                    onchange={(v: string) => (preferredAssistantId = v)}
                /><AiChoice
                    label={$tr('workspaceAi.text80')}
                    value={String(credentialRequested)}
                    options={[
                        { value: 'true', label: $tr('workspaceAi.yes') },
                        { value: 'false', label: $tr('workspaceAi.no') },
                    ]}
                    onselect={(v: string) => (credentialRequested = v === 'true')}
                /><AiAction
                    label={$tr('workspaceAi.text81')}
                    disabled={busy}
                    onclick={() => void startDiscovery()}
                />{:else if selectedSession}<AiReview
                    label={$tr('workspaceAi.text82')}
                    value={selectedSession}
                />{#if latestEvent}<AiReview
                        label={$tr('workspaceAi.text83')}
                        value={latestEvent}
                    />{/if}{#if actionKind === 'select_template'}{#each workspace.discovery_candidates as candidate (candidate.id)}<AiLink
                            label={candidateLabel(candidate.summary)}
                            onclick={() =>
                                void continueWith({
                                    kind: 'select_template',
                                    candidate_id: candidate.id,
                                })}
                        />{/each}<AiAction
                        label={$tr('workspaceAi.text84')}
                        disabled={busy}
                        onclick={() => void continueWith({ kind: 'continue_without_template' })}
                    />{/if}{#if actionKind === 'supply_more_evidence'}<EditField
                        label={$tr('workspaceAi.text78')}
                        value={documentEvidenceUrl}
                        maxlength={2048}
                        onchange={(v: string) => (documentEvidenceUrl = v)}
                    /><AiAction
                        label={$tr('workspaceAi.text85')}
                        disabled={busy}
                        onclick={() => void submitDocumentEvidence()}
                    /><AiAction
                        label={$tr('workspaceAi.text86')}
                        disabled={busy}
                        onclick={() => void submitCurlEvidence()}
                    /><AiAction
                        label={$tr('workspaceAi.text87')}
                        disabled={busy || selectedSession.preferred_assistant === null}
                        onclick={() => void continueWith({ kind: 'request_assistant' })}
                    />{/if}{#if workspace.discovery_approval_proposal}<AiReview
                        label={$tr('workspaceAi.text88')}
                        value={workspace.discovery_approval_proposal.grant}
                    />{/if}
                {#if workspace.discovery_review_proposal}<AiReview
                        label={$tr('workspaceAi.text89')}
                        value={workspace.discovery_review_proposal}
                    />{/if}
                {#if selectedCredentialTarget !== null && selectedSession.state !== 'committing' && selectedCredentialStatus !== 'available'}<AiAction
                        label={$tr('workspaceAi.text90')}
                        disabled={busy}
                        onclick={() => void captureDiscoveryCredential()}
                    />{/if}{#if actionKind === 'approve_assistant'}<AiAction
                        label={$tr('workspaceAi.text91')}
                        disabled={busy}
                        onclick={() => void continueWith({ kind: 'decline_assistant' })}
                    />{:else if actionKind === 'approve_probes'}<AiAction
                        label={$tr('workspaceAi.text92')}
                        disabled={busy}
                        onclick={() => void continueWith({ kind: 'skip_probes' })}
                    />{/if}{#if actionKind === 'reconcile_unknown_outcome'}<AiChoice
                        label={$tr('workspaceAi.text93')}
                        value={unknownResolution}
                        options={[
                            'confirmed_no_effect',
                            'confirmed_compensated',
                            'manually_reconciled_as_failed',
                        ].map((value) => ({
                            value,
                            label:
                                value === 'confirmed_no_effect'
                                    ? $tr('workspaceReview.noEffect')
                                    : value === 'confirmed_compensated'
                                      ? $tr('workspaceReview.compensated')
                                      : $tr('workspaceReview.failed'),
                        }))}
                        onselect={(v: string) =>
                            (unknownResolution = v as typeof unknownResolution)}
                    />{/if}
                <AiDiscoveryAssistant
                    {appState}
                    {controller}
                    {busy}
                    {run}
                />{#if selectedSession.commit_attempt_id !== null && workspace.discovery_compensation_steps.length > 0}<AiReview
                        label={$tr('workspaceAi.text101')}
                        value={workspace.discovery_compensation_steps}
                    /><AiAction
                        label={$tr('workspaceAi.text102')}
                        disabled={busy}
                        onclick={() =>
                            void run(() => controller.continueProviderDiscoveryCompensation(false))}
                    /><AiAction
                        label={$tr('workspaceAi.text103')}
                        disabled={busy}
                        onclick={() =>
                            void run(() => controller.continueProviderDiscoveryCompensation(true))}
                    />{/if}<AiAction
                    label={$tr('workspaceAi.reload')}
                    disabled={busy}
                    onclick={() =>
                        void run(() => controller.refreshProviderDiscovery(selectedSession.id))}
                />{/if}
        </section>
        <AiDiscoveryGuidance {appState} sessionId={routedSessionId} />
        <AiDiscoveryActions
            {appState}
            {controller}
            sessionId={routedSessionId}
            {busy}
            {unknownResolution}
            {run}
            {continueWith}
            {commitDiscovery}
        />
    </SettingsPanel>
{/if}
