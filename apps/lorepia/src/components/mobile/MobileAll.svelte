<script lang="ts">
    import type { LorepiaAppState } from '../../app/app-controller';
    import type { StudioSection } from '../../features/orchestration/studio-contracts';
    import type { SettingsSection } from '../../features/providers/settings-contracts';
    import SettingsOverview from '../../features/providers/settings/SettingsOverview.svelte';
    import { tr } from '../../lib/i18n';
    let { appState, onOpenSettings, titlebarOverlay = false }: {
        appState: LorepiaAppState;
        onOpenSettings: (section: SettingsSection) => void;
        onOpenStudio: (section: StudioSection) => void;
        titlebarOverlay?: boolean;
    } = $props();
</script>

<section class="mobile-screen mobile-all" aria-label={$tr('app.tab.providers')}>
    <header class="mobile-page-header" data-tauri-drag-region={titlebarOverlay ? '' : undefined}>
        <h1 data-tauri-drag-region={titlebarOverlay ? '' : undefined}>{$tr('app.tab.providers')}</h1>
    </header>
    <div class="mobile-page-scroll"><SettingsOverview {appState} onSelectSection={onOpenSettings} /></div>
</section>
