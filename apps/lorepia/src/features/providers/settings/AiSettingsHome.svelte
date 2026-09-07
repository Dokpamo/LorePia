<script lang="ts">
    import { ChevronRight } from '@lucide/svelte';
    import { tr } from '../../../lib/i18n';
    import type { SettingsSection } from '../settings-contracts';
    let { onSelect }: { onSelect: (section: SettingsSection) => void } = $props();
    const steps = ['connections', 'target', 'advanced'] as const;
</script>

<div class="settings-workspace">
    <p class="settings-lead">{$tr('settingsLive.aiIntro')}</p>
    <div class="settings-purpose-card">
        {#each steps as section, index (section)}
            <button class="settings-purpose-row" onclick={() => onSelect(section)}>
                <span class="settings-step" aria-hidden="true">{index + 1}</span>
                <span><strong>{$tr(`settings.section.${section}.title`)}</strong><small>{$tr(`settingsLive.${section}Hint`)}</small></span>
                <ChevronRight aria-hidden="true" />
            </button>
        {/each}
    </div>
    <h2>{$tr('settingsLive.connectionTools')}</h2>
    <div class="settings-purpose-card">
        {#each ['discovery', 'templates', 'catalog'] as section (section)}
            <button class="settings-purpose-row" onclick={() => onSelect(section as SettingsSection)}><span><strong>{$tr(`settings.section.${section}.title` as import('../../../lib/i18n').MessageKey)}</strong></span><ChevronRight aria-hidden="true" /></button>
        {/each}
    </div>
</div>
