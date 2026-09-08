<script lang="ts">
    import type { LorepiaAppState } from '../../app-controller';
    import type { DiscoveryAssistantFailureKindInput } from '../../../lib/ipc/contracts';
    import { tr } from '../../../lib/i18n';
    import AiReview from './AiReview.svelte';
    import AiAction from './AiAction.svelte';
    import AiChoice from './AiChoice.svelte';
    import type { LorepiaAppController } from '../../app-controller';
    let {
        appState,
        controller,
        busy,
        run,
    }: {
        appState: LorepiaAppState;
        controller: LorepiaAppController;
        busy: boolean;
        run: (action: () => Promise<unknown>) => Promise<void>;
    } = $props();
    const assistantBoundary = $derived(
        appState.providers.workspace.discovery_assistant_resume_boundary,
    );
    let assistantFailureKind = $state<DiscoveryAssistantFailureKindInput>('transport');
    let assistantFailureRetryable = $state(true);
</script>

{#if assistantBoundary}<AiReview label={$tr('workspaceAi.text94')} value={assistantBoundary} />
    {#if assistantBoundary.action === 'run_assistant'}<p class="ui-field-error">
            {$tr('workspaceAi.assistantUnavailable')}
        </p>
    {:else if assistantBoundary.action === 'review_draft'}<AiAction
            label={$tr('workspaceAi.text95')}
            disabled={busy}
            onclick={() => void run(() => controller.requestProviderDiscoveryAssistantRevision())}
        />{:else if assistantBoundary.action === 'wait_for_assistant_outcome'}<AiAction
            label={$tr('workspaceAi.text96')}
            disabled={busy}
            onclick={() =>
                void run(() =>
                    controller.interruptProviderDiscoveryAssistant('confirmed_no_external_effect'),
                )}
        /><AiAction
            label={$tr('workspaceAi.text97')}
            disabled={busy}
            onclick={() =>
                void run(() =>
                    controller.interruptProviderDiscoveryAssistant('external_outcome_unknown'),
                )}
        />{/if}<AiChoice
        label={$tr('workspaceAi.text98')}
        value={assistantFailureKind}
        options={[
            'transport',
            'timeout',
            'rate_limited',
            'invalid_structured_output',
            'draft_revision_required',
            'provider_rejected',
            'internal',
        ].map((value) => ({ value, label: value }))}
        onselect={(v: string) => (assistantFailureKind = v as DiscoveryAssistantFailureKindInput)}
    />
    <AiChoice
        label={$tr('workspaceAi.text99')}
        value={String(assistantFailureRetryable)}
        options={[
            { value: 'true', label: $tr('workspaceAi.yes') },
            { value: 'false', label: $tr('workspaceAi.no') },
        ]}
        onselect={(v: string) => (assistantFailureRetryable = v === 'true')}
    /><AiAction
        label={$tr('workspaceAi.text100')}
        disabled={busy}
        onclick={() =>
            void run(() =>
                controller.recordProviderDiscoveryAssistantFailure(
                    assistantFailureKind,
                    assistantFailureRetryable,
                ),
            )}
    />{/if}
