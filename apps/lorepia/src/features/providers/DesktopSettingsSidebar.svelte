<script lang="ts">
    import {
        ArrowLeft,
        BriefcaseBusiness,
        CircleDot,
        Compass,
        LayoutTemplate,
        Link2,
        Scale,
        Search,
        Settings,
        SlidersHorizontal,
        SunMoon,
        UserRound,
    } from '@lucide/svelte';
    import { tr, type MessageKey } from '../../lib/i18n';
    import type { SettingsSection } from './settings-contracts';

    type SettingsDestination = 'general' | SettingsSection;

    interface SettingsNavigationGroup {
        label: MessageKey;
        destinations: SettingsDestination[];
    }

    interface Props {
        selected?: SettingsSection | null;
        titlebarOverlay?: boolean;
        onSelect?: (section: SettingsSection | null) => void;
        onReturn?: () => void;
    }

    let {
        selected = null,
        titlebarOverlay = false,
        onSelect = () => undefined,
        onReturn = () => undefined,
    }: Props = $props();
    let query = $state('');

    const groups: SettingsNavigationGroup[] = [
        {
            label: 'settings.desktop.group.personal',
            destinations: ['general', 'appearance', 'persona'],
        },
        {
            label: 'settings.desktop.group.generation',
            destinations: ['target', 'connections', 'templates'],
        },
        {
            label: 'settings.desktop.group.knowledge',
            destinations: ['discovery', 'catalog'],
        },
        {
            label: 'settings.desktop.group.advanced',
            destinations: ['advanced', 'licenses'],
        },
    ];

    function destinationLabel(destination: SettingsDestination): string {
        return destination === 'general'
            ? $tr('settings.desktop.general')
            : $tr(`settings.section.${destination}.title`);
    }

    function destinationVisible(destination: SettingsDestination): boolean {
        const normalizedQuery = query.trim().toLocaleLowerCase('ko');
        if (normalizedQuery === '') return true;
        return destinationLabel(destination).toLocaleLowerCase('ko').includes(normalizedQuery);
    }

    function groupVisible(group: SettingsNavigationGroup): boolean {
        return group.destinations.some(destinationVisible);
    }

    function isSelected(destination: SettingsDestination): boolean {
        return destination === 'general' ? selected === null : selected === destination;
    }

    function selectDestination(destination: SettingsDestination): void {
        onSelect(destination === 'general' ? null : destination);
    }
</script>

{#snippet destinationIcon(destination: SettingsDestination)}
    {#if destination === 'general'}
        <Settings aria-hidden="true" />
    {:else if destination === 'appearance'}
        <SunMoon aria-hidden="true" />
    {:else if destination === 'persona'}
        <UserRound aria-hidden="true" />
    {:else if destination === 'target'}
        <CircleDot aria-hidden="true" />
    {:else if destination === 'connections'}
        <Link2 aria-hidden="true" />
    {:else if destination === 'templates'}
        <LayoutTemplate aria-hidden="true" />
    {:else if destination === 'discovery'}
        <Compass aria-hidden="true" />
    {:else if destination === 'catalog'}
        <BriefcaseBusiness aria-hidden="true" />
    {:else if destination === 'advanced'}
        <SlidersHorizontal aria-hidden="true" />
    {:else}
        <Scale aria-hidden="true" />
    {/if}
{/snippet}

<div class="desktop-settings-sidebar">
    <div
        class="settings-native-titlebar"
        class:titlebar-overlay={titlebarOverlay}
        data-tauri-drag-region={titlebarOverlay ? '' : undefined}
        aria-hidden="true"
    ></div>

    <div class="settings-sidebar-controls">
        <button class="settings-return-button" type="button" onclick={onReturn}>
            <ArrowLeft aria-hidden="true" />
            <span>{$tr('settings.desktop.return')}</span>
        </button>

        <label class="settings-search-field">
            <span class="sr-only">{$tr('settings.desktop.search')}</span>
            <Search aria-hidden="true" />
            <input
                bind:value={query}
                type="search"
                placeholder={$tr('settings.desktop.search.placeholder')}
                autocomplete="off"
            />
        </label>
    </div>

    <nav class="settings-destination-scroll" aria-label={$tr('settings.desktop.navigation')}>
        {#each groups as group (group.label)}
            {#if groupVisible(group)}
                <section
                    class="settings-navigation-group"
                    aria-labelledby={`settings-group-${group.label}`}
                >
                    <h2 id={`settings-group-${group.label}`}>{$tr(group.label)}</h2>
                    <div class="settings-navigation-items">
                        {#each group.destinations as destination (destination)}
                            {#if destinationVisible(destination)}
                                <button
                                    class="settings-destination-row"
                                    type="button"
                                    aria-current={isSelected(destination) ? 'page' : undefined}
                                    onclick={() => selectDestination(destination)}
                                >
                                    <span class="settings-destination-icon">
                                        {@render destinationIcon(destination)}
                                    </span>
                                    <span>{destinationLabel(destination)}</span>
                                </button>
                            {/if}
                        {/each}
                    </div>
                </section>
            {/if}
        {/each}
        {#if !groups.some(groupVisible)}
            <p class="settings-search-empty">일치하는 설정이 없습니다.</p>
        {/if}
    </nav>
</div>

<style>
    .desktop-settings-sidebar {
        display: grid;
        height: 100%;
        min-height: 0;
        grid-template-rows: 46px auto minmax(0, 1fr);
    }

    .settings-native-titlebar {
        min-height: 46px;
    }

    .settings-native-titlebar.titlebar-overlay {
        -webkit-app-region: drag;
    }

    .settings-sidebar-controls {
        display: grid;
        padding: 2px 10px 12px;
        gap: 9px;
    }

    .settings-return-button {
        display: flex;
        min-height: 30px;
        align-items: center;
        justify-content: flex-start;
        padding: 0 7px;
        border: 0;
        border-radius: var(--radius-sm);
        background: transparent;
        color: var(--ink-muted);
        font-size: 13px;
        gap: 8px;
    }

    .settings-return-button :global(svg) {
        width: 15px;
        height: 15px;
        stroke-width: 1.8;
    }

    .settings-search-field {
        display: grid;
        min-height: 36px;
        align-items: center;
        border: 1px solid transparent;
        border-radius: 10px;
        background: var(--surface-active);
        color: var(--ink-subtle);
        grid-template-columns: 18px minmax(0, 1fr);
        padding-inline: 10px;
        gap: 7px;
    }

    .settings-search-field:focus-within {
        border-color: var(--accent-line);
    }

    .settings-search-field :global(svg) {
        width: 17px;
        height: 17px;
        stroke-width: 1.8;
    }

    .settings-search-field input {
        width: 100%;
        min-width: 0;
        height: 34px;
        padding: 0;
        border: 0;
        outline: 0;
        background: transparent;
        color: var(--ink);
        font: inherit;
        font-size: 13px;
    }

    .settings-search-field input::placeholder {
        color: var(--ink-subtle);
    }

    .settings-search-field input::-webkit-search-cancel-button {
        opacity: 0.62;
    }

    .settings-destination-scroll {
        min-height: 0;
        padding: 2px 10px 18px;
        overflow-x: hidden;
        overflow-y: auto;
        overscroll-behavior: contain;
    }

    .settings-navigation-group + .settings-navigation-group {
        margin-top: 18px;
    }

    .settings-navigation-group h2 {
        padding: 0 7px 7px;
        margin: 0;
        color: var(--ink-subtle);
        font-size: 12px;
        font-weight: 600;
        letter-spacing: 0.01em;
    }

    .settings-navigation-items {
        display: grid;
        gap: 2px;
    }

    .settings-destination-row {
        position: relative;
        display: grid;
        width: 100%;
        min-height: 33px;
        align-items: center;
        justify-content: stretch;
        padding: 0 9px;
        border: 0;
        border-radius: 8px;
        background: transparent;
        color: var(--ink-muted);
        font-size: 13px;
        font-weight: 500;
        grid-template-columns: 18px minmax(0, 1fr);
        gap: 8px;
        text-align: left;
    }

    .settings-destination-row[aria-current='page'] {
        background: var(--surface-active);
        color: var(--ink);
    }

    .settings-destination-row:focus-visible {
        outline: none;
    }

    .settings-destination-row:focus-visible:not([aria-current='page']) {
        background: var(--surface-active);
        color: var(--ink);
    }

    .settings-destination-icon {
        display: grid;
        place-items: center;
    }

    .settings-destination-icon :global(svg) {
        width: 17px;
        height: 17px;
        fill: none;
        stroke: currentcolor;
        stroke-width: 1.8;
    }

    .settings-search-empty {
        padding: 12px 8px;
        margin: 0;
        color: var(--ink-subtle);
        font-size: 11px;
    }
</style>
