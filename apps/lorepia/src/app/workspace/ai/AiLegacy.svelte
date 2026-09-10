<script lang="ts">
    import type { LorepiaAppState, LorepiaAppController } from '../../app-controller';
    import { tr } from '../../../lib/i18n';
    import SettingsPanel from './AiPanel.svelte';
    import AiAction from './AiAction.svelte';
    import AiLink from './AiLink.svelte';
    import AiReview from './AiReview.svelte';
    let {
        appState,
        controller,
        onclose,
    }: { appState: LorepiaAppState; controller: LorepiaAppController; onclose: () => void } =
        $props();
    let id = $state('');
    let busy = $state(false);
    let deleting = $state(false);
    const profile = $derived(appState.providers.workspace.legacy_profiles.find((p) => p.id === id));
    async function run(action: () => Promise<unknown>) {
        if (busy) return;
        busy = true;
        try {
            await action();
        } finally {
            busy = false;
        }
    }
</script>

<SettingsPanel
    {appState}
    {controller}
    title={$tr('workspaceAi.legacy')}
    disabled={busy}
    {onclose}
    covered={id !== ''}
>
    <section class="ui-settings-group" inert={busy}>
        {#each appState.providers.workspace.legacy_profiles as p (p.id)}<AiLink
                label={p.display_name}
                onclick={() => {
                    id = p.id;
                    deleting = false;
                }}
            />{/each}
    </section>
</SettingsPanel>
{#if id !== ''}
    <SettingsPanel
        {appState}
        {controller}
        title={$tr('workspaceAi.legacy')}
        disabled={busy}
        onclose={() => (id = '')}
    >
        <section class="ui-settings-group" inert={busy}>
            {#if profile}
                <AiReview label={$tr('workspaceAi.text177')} value={profile} />
                <AiAction
                    label={$tr('workspaceAi.text178')}
                    onclick={() =>
                        void run(() => controller.selectLegacyProviderProfile(profile.id))}
                />
                <AiAction
                    label={$tr('workspaceAi.text25')}
                    onclick={() =>
                        void run(() =>
                            controller.captureProviderCredential({
                                kind: 'legacy_profile',
                                provider_profile_id: profile.id,
                            }),
                        )}
                />
                <AiAction label={$tr('workspaceAi.text26')} onclick={() => (deleting = true)} />
                {#if deleting}<AiAction
                        label={$tr('workspaceAi.text27')}
                        onclick={() =>
                            void run(async () => {
                                await controller.deleteProviderCredential({
                                    kind: 'legacy_profile',
                                    provider_profile_id: profile.id,
                                });
                                deleting = false;
                            })}
                    />{/if}
            {/if}
        </section>
    </SettingsPanel>
{/if}
