<script lang="ts">
    import type { LorepiaAppState } from '../../app/app-controller';
    import type { StudioSection } from '../../features/orchestration/studio-contracts';
    import type { SettingsSection } from '../../features/providers/settings-contracts';
    import { tr } from '../../lib/i18n';
    import { themePreference } from '../../lib/theme';
    import MobileMenuRow from './MobileMenuRow.svelte';

    interface Props {
        appState: LorepiaAppState;
        onOpenSettings: (section: SettingsSection) => void;
        onOpenStudio: (section: StudioSection) => void;
        titlebarOverlay?: boolean;
    }
    let { appState, onOpenSettings, onOpenStudio, titlebarOverlay = false }: Props = $props();
    const workspace = $derived(appState.providers.workspace);
    const selectedRoute = $derived(
        workspace.routes.find((route) => route.id === workspace.settings.selected_model_route_id),
    );
    const selectedModel = $derived(
        workspace.legacy_profiles.find(
            (profile) => profile.id === workspace.settings.selected_provider_profile_id,
        )?.display_name ??
            selectedRoute?.display_name ??
            selectedRoute?.model_id,
    );
    const creationSections = ['prompt', 'memory', 'content'] as const;
    const modelSections = ['discovery', 'templates', 'catalog', 'advanced'] as const;
</script>

<section class="mobile-screen mobile-all" aria-label={$tr('mobile.nav.all')}>
    <header class="mobile-page-header" data-tauri-drag-region={titlebarOverlay ? '' : undefined}>
        <h1 data-tauri-drag-region={titlebarOverlay ? '' : undefined}>{$tr('mobile.nav.all')}</h1>
    </header>
    <div class="mobile-page-scroll">
        <section class="mobile-card mobile-menu-card" aria-label={$tr('mobile.all.conversation')}>
            <h2>{$tr('mobile.all.conversation')}</h2>
            <MobileMenuRow
                label={$tr('mobile.settings.target')}
                value={selectedModel ?? $tr('mobile.all.choose_model')}
                accent={!selectedModel}
                onSelect={() => onOpenSettings('target')}
            />
            <MobileMenuRow
                label={$tr('mobile.settings.connections')}
                onSelect={() => onOpenSettings('connections')}
            />
            <MobileMenuRow
                label={$tr('mobile.settings.persona')}
                onSelect={() => onOpenSettings('persona')}
            />
        </section>
        <section class="mobile-card mobile-menu-card" aria-label={$tr('mobile.all.create')}>
            <h2>{$tr('mobile.all.create')}</h2>
            {#each creationSections as section (section)}
                <MobileMenuRow
                    label={$tr(`mobile.studio.${section}`)}
                    onSelect={() => onOpenStudio(section)}
                />
            {/each}
        </section>
        <section class="mobile-card mobile-menu-card" aria-label={$tr('mobile.all.app')}>
            <h2>{$tr('mobile.all.app')}</h2>
            <MobileMenuRow
                label={$tr('mobile.settings.appearance')}
                value={$tr(`mobile.theme.${$themePreference}`)}
                onSelect={() => onOpenSettings('appearance')}
            />
            <MobileMenuRow
                label={$tr('mobile.settings.licenses')}
                onSelect={() => onOpenSettings('licenses')}
            />
        </section>
        <section class="mobile-card mobile-menu-card" aria-label={$tr('mobile.all.tools')}>
            <h2>{$tr('mobile.all.tools')}</h2>
            {#each modelSections as section (section)}
                <MobileMenuRow
                    label={$tr(`mobile.settings.${section}`)}
                    onSelect={() => onOpenSettings(section)}
                />
            {/each}
            <MobileMenuRow
                label={$tr('mobile.studio.diagnostics')}
                onSelect={() => onOpenStudio('diagnostics')}
            />
        </section>
        {#if appState.bootstrap.value?.app_version}
            <p class="mobile-version">LorePia {appState.bootstrap.value.app_version}</p>
        {/if}
    </div>
</section>
