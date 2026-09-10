<script lang="ts">
    import { ChevronRight } from '@lucide/svelte';
    import type { LorepiaAppState } from '../../../app/app-controller';
    import { tr } from '../../../lib/i18n';
    import { themePreference } from '../../../lib/theme';
    import type { PersonaState } from '../../personas/persona-controller';
    import { PRIMARY_SETTINGS_SECTIONS, type SettingsSection } from '../settings-contracts';
    import SettingsNavigationIcon from './SettingsNavigationIcon.svelte';
    import './styles/settings-workspace.css';
    let {
        appState,
        personaState,
        onSelectSection,
    }: {
        appState: LorepiaAppState;
        personaState?: PersonaState;
        onSelectSection: (section: SettingsSection) => void;
    } = $props();
    const model = $derived(
        appState.providers.workspace.routes.find(
            (item) => item.id === appState.providers.workspace.settings.selected_model_route_id,
        ),
    );
</script>

<div class="settings-workspace settings-home">
    <p class="settings-lead">{$tr('settingsUi.intro')}</p>
    {#each [PRIMARY_SETTINGS_SECTIONS.slice(0, 4), PRIMARY_SETTINGS_SECTIONS.slice(4)] as group, index (index)}
        <section aria-label={$tr(index === 0 ? 'settingsUi.play' : 'settingsUi.app')}>
            <h2>{$tr(index === 0 ? 'settingsUi.play' : 'settingsUi.app')}</h2>
            <div class="settings-purpose-card ui-settings-group">
                {#each group as section (section)}
                    <button
                        class="settings-purpose-row ui-pressable"
                        aria-label={$tr(`settings.section.${section}.title`)}
                        onclick={() => onSelectSection(section)}
                    >
                        <span class="settings-purpose-icon"
                            ><SettingsNavigationIcon {section} /></span
                        >
                        <span
                            ><strong>{$tr(`settings.section.${section}.title`)}</strong><small>
                                {#if section === 'ai'}{model?.display_name ??
                                        model?.model_id ??
                                        $tr('settingsLive.aiIntro')}
                                {:else if section === 'appearance'}{$tr(
                                        `mobile.theme.${$themePreference}`,
                                    )}
                                {:else if section === 'persona' && personaState}{$tr(
                                        'settingsLive.personaCount',
                                        { count: personaState.personas.length },
                                    )}
                                {:else}{$tr(`settings.section.${section}.hint`)}{/if}
                            </small></span
                        >
                        <ChevronRight aria-hidden="true" />
                    </button>
                {/each}
            </div>
        </section>
    {/each}
    <button class="settings-info-link" onclick={() => onSelectSection('licenses')}
        >{$tr('settings.section.licenses.title')}</button
    >
</div>
