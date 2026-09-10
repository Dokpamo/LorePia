<script lang="ts">
    import { tr } from '../../../lib/i18n';
    import { discoveryCredentialTarget, type LorepiaAppState } from '../../app-controller';
    let { appState, sessionId }: { appState: LorepiaAppState; sessionId: string | null } = $props();
    const session = $derived(
        appState.providers.workspace.discoveries.find((item) => item.id === sessionId),
    );
    const target = $derived(session ? discoveryCredentialTarget(session) : null);
    const credentialStatus = $derived(
        target
            ? appState.providers.workspace.credential_statuses[
                  `discovery_session:${target.session_id}`
              ]
            : null,
    );
</script>

{#if session?.action_required?.kind === 'approve_probes'}
    <p class="ui-live-hint">{$tr('workspaceReview.probeHint')}</p>
{/if}
{#if session?.action_required?.kind === 'restart_interrupted'}
    <p class="ui-live-hint">{$tr('workspaceReview.interruptedHint')}</p>
{/if}
{#if target && session?.state !== 'committing' && credentialStatus !== 'available'}
    <p class="ui-live-hint">
        {$tr(
            credentialStatus === 'unreadable'
                ? 'workspaceReview.unreadableHint'
                : 'workspaceReview.captureHint',
        )}
    </p>
{/if}
