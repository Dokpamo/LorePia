<script lang="ts">
    import { tr } from '../../../lib/i18n';
    import type { LorepiaAppController, LorepiaAppState } from '../../app-controller';
    import { discoveryCredentialTarget } from '../../app-controller';
    import type { ContinueProviderDiscoveryActionInput } from '../../../lib/ipc/contracts';
    let {
        appState,
        controller,
        sessionId,
        busy,
        unknownResolution,
        run,
        continueWith,
        commitDiscovery,
    }: {
        appState: LorepiaAppState;
        controller: LorepiaAppController;
        sessionId: string | null;
        busy: boolean;
        unknownResolution:
            'confirmed_no_effect' | 'confirmed_compensated' | 'manually_reconciled_as_failed';
        run: (action: () => Promise<unknown>) => Promise<void>;
        continueWith: (action: ContinueProviderDiscoveryActionInput) => Promise<void>;
        commitDiscovery: () => Promise<void>;
    } = $props();
    const workspace = $derived(appState.providers.workspace);
    const routedSessionId = $derived(sessionId);
    const selectedSession = $derived(
        workspace.discoveries.find((session) => session.id === routedSessionId) ?? null,
    );
    const actionKind = $derived(selectedSession?.action_required?.kind ?? null);
    const assistantBoundary = $derived(workspace.discovery_assistant_resume_boundary);
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
    const actionNeedsCredential = $derived(
        selectedSession !== null &&
            selectedCredentialTarget !== null &&
            ((selectedSession.state === 'awaiting_credential_origin_approval' &&
                actionKind === 'approve_credential_origin') ||
                (selectedSession.state === 'awaiting_probe_consent' &&
                    actionKind === 'approve_probes') ||
                (selectedSession.state === 'interrupted' &&
                    actionKind === 'restart_interrupted' &&
                    (selectedSession.recovery_operation === 'list_models' ||
                        selectedSession.recovery_operation === 'probe_capabilities'))),
    );

    function terminalState(state: string) {
        return ['completed', 'cancelled', 'failed'].includes(state);
    }
</script>

{#if selectedSession && !terminalState(selectedSession.state)}
    <section>
        <button
            type="button"
            disabled={busy}
            onclick={() => void run(() => controller.cancelProviderDiscovery())}
            class="ui-submit ui-pressable"
            ><span class="ui-press-visual">{$tr('workspaceAi.text106')}</span></button
        >

        {#if actionKind === 'approve_assistant' && workspace.discovery_approval_proposal}
            {@const proposal = workspace.discovery_approval_proposal}
            <button
                type="button"
                disabled={busy}
                onclick={() =>
                    void continueWith({
                        kind: 'approve_assistant',
                        approval_id: proposal.id,
                        approval_grant_sha256: proposal.grant_sha256,
                    })}
                class="ui-submit ui-pressable"
                ><span class="ui-press-visual">{$tr('workspaceAi.text107')}</span></button
            >
        {:else if actionKind === 'approve_credential_origin' && workspace.discovery_approval_proposal}
            {@const proposal = workspace.discovery_approval_proposal}
            <button
                type="button"
                disabled={busy ||
                    (actionNeedsCredential && selectedCredentialStatus !== 'available')}
                onclick={() =>
                    void continueWith({
                        kind: 'approve_credential_origin',
                        approval_id: proposal.id,
                    })}
                class="ui-submit ui-pressable"
                ><span class="ui-press-visual">{$tr('workspaceAi.text108')}</span></button
            >
        {:else if actionKind === 'approve_probes' && workspace.discovery_approval_proposal}
            {@const proposal = workspace.discovery_approval_proposal}
            <button
                type="button"
                disabled={busy ||
                    (actionNeedsCredential && selectedCredentialStatus !== 'available')}
                onclick={() =>
                    void continueWith({
                        kind: 'approve_probes',
                        approval_id: proposal.id,
                        approval_grant_sha256: proposal.grant_sha256,
                    })}
                class="ui-submit ui-pressable"
                ><span class="ui-press-visual">{$tr('workspaceAi.text109')}</span></button
            >
        {:else if actionKind === 'review' && workspace.discovery_review_proposal}
            {@const proposal = workspace.discovery_review_proposal}
            <button
                type="button"
                disabled={busy || proposal.review.unresolved_question_count > 0}
                onclick={() =>
                    void continueWith({
                        kind: 'approve_review',
                        approval_id: proposal.approval.id,
                        commit_attempt_id: proposal.commit_attempt_id,
                        commit_plan_sha256: proposal.commit_plan_sha256,
                        graph_sha256: proposal.review.graph_sha256,
                    })}
                class="ui-submit ui-pressable"
                ><span class="ui-press-visual">{$tr('workspaceAi.text110')}</span></button
            >
        {:else if actionKind === 'restart_interrupted'}
            <button
                type="button"
                disabled={busy ||
                    (actionNeedsCredential && selectedCredentialStatus !== 'available')}
                onclick={() => void continueWith({ kind: 'restart_interrupted' })}
                class="ui-submit ui-pressable"
                ><span class="ui-press-visual">{$tr('workspaceAi.text111')}</span></button
            >
        {:else if actionKind === 'reconcile_unknown_outcome' && workspace.discovery_approval_proposal}
            {@const proposal = workspace.discovery_approval_proposal}
            <button
                type="button"
                disabled={busy}
                onclick={() =>
                    void continueWith({
                        kind: 'resolve_unknown_outcome',
                        approval_id: proposal.id,
                        resolution: { resolution: unknownResolution },
                    })}
                class="ui-submit ui-pressable"
                ><span class="ui-press-visual">{$tr('workspaceAi.text112')}</span></button
            >
        {:else if selectedSession.review !== null && selectedSession.committed_connection_id === null && actionKind === null}
            <button
                type="button"
                disabled={busy ||
                    (selectedCredentialTarget !== null && selectedCredentialStatus !== 'available')}
                onclick={() => void commitDiscovery()}
                class="ui-submit ui-pressable"
                ><span class="ui-press-visual">{$tr('workspaceAi.text113')}</span></button
            >
        {:else if assistantBoundary?.action === 'resume_core_host_action'}
            <button
                type="button"
                disabled={busy}
                onclick={() =>
                    void run(() => controller.resumeProviderDiscoveryAssistantCoreHostAction())}
                class="ui-submit ui-pressable"
                ><span class="ui-press-visual">{$tr('workspaceAi.text114')}</span></button
            >
        {:else if assistantBoundary?.action === 'approve_retry'}
            <button
                type="button"
                disabled={busy}
                onclick={() => void run(() => controller.approveProviderDiscoveryAssistantRetry())}
                class="ui-submit ui-pressable"
                ><span class="ui-press-visual">{$tr('workspaceAi.text115')}</span></button
            >
        {:else if assistantBoundary?.action === 'review_draft' && assistantBoundary.draft_review}
            <button
                type="button"
                disabled={busy ||
                    assistantBoundary.draft_review.unresolved_conflicts.length > 0 ||
                    assistantBoundary.draft_review.draft.unresolved_questions.length > 0}
                onclick={() => void run(() => controller.acceptProviderDiscoveryAssistantDraft())}
                class="ui-submit ui-pressable"
                ><span class="ui-press-visual">{$tr('workspaceAi.text116')}</span></button
            >
        {:else if assistantBoundary?.action === 'restart_interrupted'}
            <button
                type="button"
                disabled={busy}
                onclick={() =>
                    void run(() => controller.restartProviderDiscoveryAssistantAfterInterruption())}
                class="ui-submit ui-pressable"
                ><span class="ui-press-visual">{$tr('workspaceAi.text117')}</span></button
            >
        {:else}
            <button
                type="button"
                disabled={busy}
                onclick={() => void run(() => controller.pollSelectedProviderDiscoveryEvents())}
                class="ui-submit ui-pressable"
                ><span class="ui-press-visual">{$tr('workspaceAi.text118')}</span></button
            >
        {/if}
    </section>
{/if}
