<script lang="ts">
    import { tick, type Snippet } from 'svelte';
    import type { LorepiaAppState, LorepiaAppController } from '../../app-controller';
    import { tr } from '../../../lib/i18n';
    import { workspaceFeedback } from '../workspace-feedback';
    import SettingsPanel from '../../../ui/workspace/SettingsPanel.svelte';
    import DiscardChanges from '../../../ui/workspace/DiscardChanges.svelte';
    import type { BackDecision } from '../../../ui/workspace/edge-back';
    import AiAction from './AiAction.svelte';
    let {
        title,
        appState,
        controller,
        onclose,
        disabled = false,
        covered = false,
        dirty = false,
        children,
    }: {
        title: string;
        appState: LorepiaAppState;
        controller: LorepiaAppController;
        onclose: () => void;
        disabled?: boolean;
        covered?: boolean;
        dirty?: boolean;
        children: Snippet;
    } = $props();
    let confirming = $state(false);
    let resumeBack: (() => Promise<void>) | undefined;
    let previousFocus: HTMLElement | null = null;
    function beforeback(): BackDecision {
        if (confirming || disabled) return false;
        if (!dirty) return true;
        previousFocus =
            document.activeElement instanceof HTMLElement ? document.activeElement : null;
        return {
            confirm: (resume) => {
                resumeBack = resume;
                confirming = true;
            },
        };
    }
    async function keepEditing() {
        confirming = false;
        await tick();
        await resumeBack?.();
        resumeBack = undefined;
        previousFocus?.focus({ preventScroll: true });
    }
</script>

<SettingsPanel {title} {onclose} {disabled} {beforeback} covered={covered || confirming}>
    {#if appState.providers.error}<p class="ui-field-error" role="alert">
            {workspaceFeedback(appState.providers.error)}
        </p>
        <AiAction
            label={$tr('workspaceAi.reload')}
            {disabled}
            onclick={() => void controller.loadProviders()}
        />
    {/if}
    {#if appState.announcement}
        <p class="ui-live-hint" role="status" aria-live="polite">
            {$tr('workspaceReview.latestNotice')} · {workspaceFeedback(appState.announcement)}
        </p>
    {/if}
    {@render children()}
</SettingsPanel>
{#if confirming}<DiscardChanges
        onkeep={() => void keepEditing()}
        ondiscard={() => {
            confirming = false;
            onclose();
        }}
    />{/if}
