<script lang="ts">
    import { onMount, untrack } from 'svelte';
    import {
        Monitor,
        Type,
        Languages,
        Bot,
        Database,
        UserRound,
        NotebookPen,
        Brain,
        Puzzle,
    } from '@lucide/svelte';
    import type { LorepiaAppController, LorepiaAppState } from '../app-controller';
    import type { LorepiaClient } from '../../lib/ipc/contracts';
    import type { PersonaClientApi } from '../../features/personas/persona-contracts';
    import { locale, setLocale, t, tr } from '../../lib/i18n';
    import { themePreference, setThemePreference } from '../../lib/theme';
    import { chatTextSize, setChatTextSize } from '../../lib/display';
    import { PersonaController } from '../../features/personas/persona-controller';
    import { OrchestrationController } from '../../features/orchestration/orchestration-controller';
    import { ContentPackageController } from '../../features/orchestration/content-package-controller';
    import SettingsPanel from '../../ui/workspace/SettingsPanel.svelte';
    import SettingsRow from '../../ui/workspace/SettingsRow.svelte';
    import ChoiceField from '../../ui/workspace/ChoiceField.svelte';
    import { useChoiceSheet } from '../../ui/workspace/choice-sheet.svelte';
    import WorkspaceAiSettings from './ai/WorkspaceAiSettings.svelte';
    import WorkspaceDataSettings from './data/WorkspaceDataSettings.svelte';

    type Section = 'ai' | 'persona' | 'prompt' | 'memory' | 'plugins' | 'storage';
    let {
        client,
        appState,
        controller,
        mode,
        onclose,
        root = false,
        initialSection,
        ondetail,
    }: {
        client: LorepiaClient;
        appState: LorepiaAppState;
        controller: LorepiaAppController;
        mode: 'app-settings' | 'providers' | 'studio';
        onclose: () => void;
        root?: boolean;
        initialSection?: Section;
        ondetail?: (active: boolean) => void;
    } = $props();
    let section = $state<Section | null>(
        untrack(
            () =>
                initialSection ??
                (mode === 'providers' ? 'ai' : mode === 'studio' ? 'prompt' : null),
        ),
    );
    const choices = useChoiceSheet();
    const chatSections = [
        { value: 'persona', icon: UserRound },
        { value: 'prompt', icon: NotebookPen },
        { value: 'memory', icon: Brain },
        { value: 'plugins', icon: Puzzle },
    ] as const;
    $effect(() => ondetail?.(section !== null));
    const personas = untrack(
        () => new PersonaController(client as LorepiaClient & Partial<PersonaClientApi>),
    );
    const orchestration = untrack(() => new OrchestrationController(client));
    const packages = untrack(() => new ContentPackageController(client));
    const personaStore = personas.state;
    const orchestrationStore = orchestration.state;
    const packageStore = packages.state;
    function backToSettings() {
        if (mode === 'app-settings') section = null;
        else onclose();
    }
    const ready = $derived(appState.bootstrap.phase === 'ready');
    const conversationId = $derived(appState.selected_conversation?.id ?? null);
    const branchId = $derived(appState.conversation_state?.active_branch_id ?? null);
    const services = $derived({
        client,
        appState,
        appController: controller,
        orchestrationState: $orchestrationStore,
        orchestrationController: orchestration,
        contentPackageState: $packageStore,
        contentPackageController: packages,
    });
    $effect(() => {
        if (ready) {
            const id = conversationId;
            untrack(() => void personas.loadContext(id));
        }
    });
    $effect(() => {
        if (ready) {
            const conversation = conversationId;
            const branch = branchId;
            untrack(() => void orchestration.loadContext(conversation, branch));
        }
    });
    onMount(() => {
        return () => {
            personas.destroy();
            orchestration.destroy();
            packages.destroy();
        };
    });
</script>

{#if mode === 'app-settings'}
    <SettingsPanel
        title={$tr(root ? 'navigation.settings' : 'uiPreview.appSettings')}
        {onclose}
        {root}
        covered={section !== null}
    >
        <section class="ui-settings-group" aria-label={$tr('uiPreview.appearance')}>
            <h2>{$tr('uiPreview.appearance')}</h2>
            <ChoiceField
                label={$tr('uiPreview.theme')}
                value={$tr(
                    $themePreference === 'dark'
                        ? 'uiPreview.dark'
                        : $themePreference === 'light'
                          ? 'uiPreview.light'
                          : 'uiPreview.system',
                )}
                onopen={(opener: HTMLButtonElement) =>
                    choices.open(
                        {
                            label: t('uiPreview.theme'),
                            value: $themePreference,
                            options: [
                                { value: 'system', label: t('uiPreview.system') },
                                { value: 'light', label: t('uiPreview.light') },
                                { value: 'dark', label: t('uiPreview.dark') },
                            ],
                            onselect: (value) => {
                                if (value === 'system' || value === 'light' || value === 'dark')
                                    setThemePreference(value);
                            },
                        },
                        opener,
                    )}
            >
                {#snippet prefix()}<Monitor />{/snippet}
            </ChoiceField>
            <ChoiceField
                label={$tr('uiPreview.textSize')}
                value={$tr(
                    $chatTextSize === 'large' ? 'uiPreview.textLarge' : 'uiPreview.textDefault',
                )}
                onopen={(opener: HTMLButtonElement) =>
                    choices.open(
                        {
                            label: t('uiPreview.textSize'),
                            value: $chatTextSize,
                            options: [
                                { value: 'normal', label: t('uiPreview.textDefault') },
                                { value: 'large', label: t('uiPreview.textLarge') },
                            ],
                            onselect: (value) => {
                                if (value === 'normal' || value === 'large') setChatTextSize(value);
                            },
                        },
                        opener,
                    )}
            >
                {#snippet prefix()}<Type />{/snippet}
            </ChoiceField>
            <ChoiceField
                label={$tr('settingsUi.screenLanguage')}
                value={$tr($locale === 'ko' ? 'settingsUi.korean' : 'settingsUi.english')}
                onopen={(opener: HTMLButtonElement) =>
                    choices.open(
                        {
                            label: t('settingsUi.screenLanguage'),
                            value: $locale,
                            options: [
                                { value: 'ko', label: t('settingsUi.korean') },
                                { value: 'en', label: t('settingsUi.english') },
                            ],
                            onselect: (value) => {
                                if (value === 'ko' || value === 'en') setLocale(value);
                            },
                        },
                        opener,
                    )}
            >
                {#snippet prefix()}<Languages />{/snippet}
            </ChoiceField>
        </section>
        <section class="ui-settings-group" aria-label={$tr('uiPreview.connections')}>
            <h2>{$tr('uiPreview.connections')}</h2>
            <SettingsRow label={$tr('uiPreview.aiConnection')} onclick={() => (section = 'ai')}>
                {#snippet prefix()}<Bot />{/snippet}
            </SettingsRow>
            <SettingsRow label={$tr('uiPreview.data')} onclick={() => (section = 'storage')}>
                {#snippet prefix()}<Database />{/snippet}
            </SettingsRow>
        </section>
        <section class="ui-settings-group" aria-label={$tr('uiPreview.chat')}>
            <h2>{$tr('uiPreview.chat')}</h2>
            {#each chatSections as item (item.value)}
                <SettingsRow
                    label={$tr(`settings.section.${item.value}.title`)}
                    onclick={() => (section = item.value)}
                >
                    {#snippet prefix()}<item.icon />{/snippet}
                </SettingsRow>
            {/each}
        </section>
    </SettingsPanel>
{/if}
{#if section === 'ai'}
    <WorkspaceAiSettings {appState} {controller} onclose={backToSettings} />
{:else if section}
    <WorkspaceDataSettings
        {section}
        {services}
        {personas}
        personaState={$personaStore}
        onclose={backToSettings}
    />
{/if}
