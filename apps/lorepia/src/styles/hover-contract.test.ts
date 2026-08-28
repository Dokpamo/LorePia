import { describe, expect, it } from 'vitest';

import appSource from '../app/App.svelte?raw';
import choiceFieldSource from '../components/ChoiceField.svelte?raw';
import choicePopoverSource from '../components/ChoicePopover.svelte?raw';
import segmentedControlSource from '../components/SegmentedControl.svelte?raw';
import toggleSwitchSource from '../components/ToggleSwitch.svelte?raw';
import detailActionBarSource from '../components/detail/DetailActionBar.svelte?raw';
import detailPageSource from '../components/detail/DetailPage.svelte?raw';
import chatPaneSource from '../features/chat/ChatPane.svelte?raw';
import conversationPaneSource from '../features/conversations/ConversationPane.svelte?raw';
import libraryPaneSource from '../features/library/LibraryPane.svelte?raw';
import orchestrationQuickDrawerSource from '../features/orchestration/OrchestrationQuickDrawer.svelte?raw';
import orchestrationStudioSource from '../features/orchestration/OrchestrationStudio.svelte?raw';
import personaPanelSource from '../features/personas/PersonaPanel.svelte?raw';
import capabilityPanelSource from '../features/providers/CapabilityPanel.svelte?raw';
import discoveryPanelSource from '../features/providers/DiscoveryPanel.svelte?raw';
import modelSyncPanelSource from '../features/providers/ModelSyncPanel.svelte?raw';
import providerCrudPanelSource from '../features/providers/ProviderCrudPanel.svelte?raw';
import desktopSettingsSidebarSource from '../features/providers/DesktopSettingsSidebar.svelte?raw';
import providerSettingsSource from '../features/providers/ProviderSettings.svelte?raw';
import themeSource from '../lib/theme.ts?raw';
import appCss from './app.css?raw';

const FINE_POINTER_MEDIA = '@media (hover: hover) and (pointer: fine)';

function blockRanges(source: string, marker: string): [number, number][] {
    const ranges: [number, number][] = [];
    let cursor = 0;

    while (cursor < source.length) {
        const markerIndex = source.indexOf(marker, cursor);
        if (markerIndex === -1) break;
        const openIndex = source.indexOf('{', markerIndex + marker.length);
        if (openIndex === -1) break;

        let depth = 0;
        let closeIndex = source.length;
        for (let index = openIndex; index < source.length; index += 1) {
            if (source[index] === '{') depth += 1;
            if (source[index] !== '}') continue;
            depth -= 1;
            if (depth === 0) {
                closeIndex = index;
                break;
            }
        }

        ranges.push([openIndex, closeIndex]);
        cursor = closeIndex + 1;
    }

    return ranges;
}

function hoverIndexes(source: string): number[] {
    return Array.from(source.matchAll(/:hover\b/g), (match) => match.index);
}

describe('pointer interaction styling', () => {
    it('keeps settings choices inside the LorePia popover layer', () => {
        const settingsSurfaces = [
            providerSettingsSource,
            providerCrudPanelSource,
            capabilityPanelSource,
            discoveryPanelSource,
            modelSyncPanelSource,
        ];

        for (const source of settingsSurfaces) {
            expect(source).not.toMatch(/<select\b/u);
            expect(source).not.toContain('datetime-local');
        }

        expect(choiceFieldSource).toContain('variant="field"');
        expect(choicePopoverSource).toContain("variant?: 'row' | 'field'");
        expect(choicePopoverSource).toContain('popover="manual"');
        expect(choicePopoverSource).toMatch(
            /\.choice-menu\.field-menu\s*\{[^}]*border:\s*1px solid var\(--line\);[^}]*background:\s*var\(--surface-raised\);/s,
        );
        expect(choicePopoverSource).toContain("getAttribute('data-layout') === 'desktop'");
        expect(choicePopoverSource).toContain(
            '? Math.min(triggerRect.width, viewportWidth - edge * 2)',
        );
        expect(choicePopoverSource).toContain(
            "variant === 'field' ? triggerRect.left : triggerRect.right - desiredWidth",
        );
        expect(choicePopoverSource).toContain(
            'const calculatedContentHeight = options.length * optionHeight + menuPadding + 2;',
        );
        expect(choicePopoverSource).toContain("variant === 'field'");
        expect(choicePopoverSource).toContain('style:height={`${String(menuHeight)}px`}');
        expect(choicePopoverSource).toMatch(
            /\.choice-menu\s*\{[^}]*box-sizing:\s*border-box;[^}]*align-content:\s*start;[^}]*grid-auto-rows:\s*max-content;/s,
        );
        expect(choicePopoverSource).toMatch(
            /\.choice-menu\.field-menu\.desktop-field-menu\s*\{[^}]*border-radius:\s*12px;[^}]*background:\s*var\(--surface-raised\);[^}]*transform-origin:\s*top left;/s,
        );
        expect(choicePopoverSource).toMatch(
            /\.field-menu\.desktop-field-menu \.choice-option\s*\{[^}]*height:\s*40px;[^}]*min-height:\s*40px;/s,
        );
        expect(choicePopoverSource).toMatch(
            /\.choice-option:focus-visible\s*\{[^}]*outline:\s*none;[^}]*background:\s*var\(--surface-hover\);/s,
        );
        expect(choicePopoverSource).not.toMatch(
            /\.field-menu\.desktop-field-menu :global\(\.choice-check\)\s*\{[^}]*display:\s*none;/s,
        );
    });

    it('uses one inverse Paper and Ink palette across light and dark modes', () => {
        const palette = {
            paper: '#f2f0ea',
            'paper-bright': '#fbfcfa',
            ink: '#2b2a28',
            'night-navy': '#151515',
            'night-indigo': '#2c2c2c',
            moon: '#edede9',
        } as const;

        for (const [name, value] of Object.entries(palette)) {
            expect(appCss).toContain(`--brand-${name}: ${value};`);
        }
        expect(appCss).toContain('--brand-summer-aqua: var(--brand-ink);');
        expect(appCss).toContain('--brand-summer-blue: var(--brand-ink);');
        expect(appCss).toContain('--brand-night-cyan: var(--brand-moon);');
        expect(appCss).toContain('--brand-logo-bg: var(--brand-paper);');
        expect(appCss).toContain('--brand-logo-ink: var(--brand-ink);');
        expect(appCss).toContain('--bg: #fbfcfa;');
        expect(appCss).toContain('--surface-sunken: #fafaf8;');
        expect(appCss).toContain('--desktop-sidebar-bg: #f5f5f5;');
        expect(appCss).toContain('--desktop-workspace-bg: #ffffff;');
        expect(appCss).toContain('--desktop-panel-bg: #f3f3f3;');
        expect(appCss).toContain('--desktop-selection-bg: #e8e8e8;');
        expect(appCss).toContain('--desktop-hover-bg: #eeeeee;');
        expect(appCss).toContain('--desktop-divider: #e3e3e3;');
        expect(appCss).toContain('--desktop-user-bubble-bg: #f1f1f1;');
        expect(appCss).toContain('--desktop-composer-bg: #f7f7f7;');
        expect(appCss).toContain('--desktop-composer-line: #e3e3e3;');
        expect(appCss).toContain('--desktop-ink: #242424;');
        expect(appCss.match(/--desktop-sidebar-bg:\s*#1b1b1b;/g)).toHaveLength(2);
        expect(appCss.match(/--desktop-workspace-bg:\s*#1f1f1f;/g)).toHaveLength(2);
        expect(appCss.match(/--desktop-panel-bg:\s*#282828;/g)).toHaveLength(2);
        expect(appCss.match(/--desktop-composer-bg:\s*#282828;/g)).toHaveLength(2);
        expect(appCss.match(/--surface-sunken:\s*#181818;/g)).toHaveLength(2);
        expect(appCss.match(/--surface-raised:\s*#262626;/g)).toHaveLength(2);
        expect(appCss.match(/--primary-bg:\s*var\(--brand-summer-gradient\);/g)).toHaveLength(1);
        expect(appCss.match(/--primary-bg:\s*var\(--brand-night-action-gradient\);/g)).toHaveLength(
            2,
        );
        expect(appCss).not.toContain('#59d8f4');
        expect(appCss).not.toContain('#101b3d');
        expect(appCss).not.toContain('#d97757');
        expect(appCss).not.toContain('--brand-orange');
        expect(appCss).not.toContain('--brand-yellow');
        expect(appCss).not.toContain('--brand-tangerine-orange');
        expect(appCss).not.toContain('#0e9384');
        expect(providerSettingsSource).toContain('lorepia-logo-mark.png');
        expect(providerSettingsSource).toContain('class="settings-avatar brand-logo-mark"');
        expect(appSource).not.toContain('lorepia-logo-mark.png');
        expect(appSource).not.toContain('class="sidebar-logo"');
        expect(appSource).not.toContain('lorepia-logo-light.png');
        expect(appSource).not.toContain('lorepia-logo-dark.png');
        expect(appCss).toContain('-webkit-mask-image: var(--logo-mask);');
        expect(appCss).toContain('mask-image: var(--logo-mask);');
        expect(themeSource).toContain("const DEFAULT_THEME_PREFERENCE: ThemePreference = 'light';");
        expect(providerSettingsSource).toMatch(
            /\.settings-avatar-wrap\s*\{[^}]*overflow:\s*visible;[^}]*background:\s*var\(--brand-logo-bg\);[^}]*box-shadow:\s*var\(--shadow-2\);/s,
        );
        expect(providerSettingsSource).toMatch(
            /\.settings-avatar-badge\s*\{[^}]*z-index:\s*1;[^}]*background:\s*var\(--surface-active\);[^}]*box-shadow:\s*var\(--shadow-1\);/s,
        );
        expect(providerSettingsSource).toMatch(
            /\.settings-avatar\s*\{[^}]*position:\s*absolute;[^}]*border-radius:\s*50%;/s,
        );
        expect(providerSettingsSource).not.toContain('data-tone=');
        expect(appCss).not.toContain('.setting-tile[data-tone=');
        expect(appCss).toMatch(
            /\.setting-icon\s*\{[^}]*color:\s*var\(--ink\);[^}]*place-items:\s*center;/s,
        );
        expect(segmentedControlSource).toMatch(
            /\.segmented-control-thumb\s*\{[^}]*position:\s*absolute;[^}]*transform:\s*translate3d\(calc\(var\(--segment-index\) \* 100%\), 0, 0\);[^}]*transition:\s*transform 220ms cubic-bezier\(0\.22, 1, 0\.36, 1\);/s,
        );
        expect(segmentedControlSource).not.toContain('font-weight: 400');
    });

    it('keeps warm daylight surfaces while separating neutral dark chat and settings layers', () => {
        expect(appCss).toContain('--desktop-chat-workspace-bg: #fdfcfb;');
        expect(
            appCss.match(/--desktop-chat-sidebar-bg:\s*var\(--desktop-chat-workspace-bg\);/g),
        ).toHaveLength(1);
        expect(appCss).toContain('--desktop-chat-panel-bg: #f5f3ef;');
        expect(appCss).toContain('--desktop-chat-selection-bg: #ebe9e5;');
        expect(appCss).toContain('--desktop-chat-bubble-bg: #f2f1f0;');
        expect(appCss).toContain('--desktop-chat-composer-bg: #ffffff;');
        expect(appCss).toContain('--desktop-chat-composer-line: #d8d6d2;');
        expect(appCss).toContain('--desktop-chat-composer-shadow: 0 5px 18px rgb(61 61 58 / 8%);');
        expect(appCss).toContain('--desktop-chat-ink: #3d3d3a;');
        expect(appCss).toContain('--desktop-chat-action-ink: #a7a6a2;');
        expect(appCss.match(/--desktop-chat-workspace-bg:\s*#151515;/g)).toHaveLength(2);
        expect(appCss.match(/--desktop-chat-sidebar-bg:\s*#111111;/g)).toHaveLength(2);
        expect(appCss.match(/--desktop-chat-panel-bg:\s*#202020;/g)).toHaveLength(2);
        expect(appCss.match(/--desktop-chat-selection-bg:\s*#2c2c2c;/g)).toHaveLength(2);
        expect(appCss.match(/--desktop-chat-bubble-bg:\s*#262626;/g)).toHaveLength(2);
        expect(appCss.match(/--desktop-chat-composer-bg:\s*#202020;/g)).toHaveLength(2);
        expect(appCss.match(/--desktop-chat-composer-line:\s*#353535;/g)).toHaveLength(2);
        expect(appCss.match(/--desktop-settings-sidebar-bg:\s*#1b1b1b;/g)).toHaveLength(2);
        expect(appCss.match(/--desktop-settings-workspace-bg:\s*#1f1f1f;/g)).toHaveLength(2);
        expect(appCss.match(/--desktop-settings-panel-bg:\s*#282828;/g)).toHaveLength(2);
        expect(appCss.match(/--desktop-settings-selection-bg:\s*#3a3a3a;/g)).toHaveLength(2);
        expect(appCss.match(/--desktop-settings-divider:\s*#333333;/g)).toHaveLength(2);
        expect(appCss.match(/--desktop-settings-control-line:\s*#414141;/g)).toHaveLength(2);
        for (const staleWarmDark of [
            '#20201e',
            '#302f2c',
            '#262624',
            '#343431',
            '#292927',
            '#dededb',
            '#aaa9a4',
        ]) {
            expect(appCss).not.toContain(staleWarmDark);
        }
        expect(appCss).toMatch(
            /\.app-shell\[data-layout='desktop'\]\[data-view='chat'\]\s*\{[^}]*--desktop-sidebar-bg:\s*var\(--desktop-chat-sidebar-bg\);[^}]*--desktop-workspace-bg:\s*var\(--desktop-chat-workspace-bg\);[^}]*--desktop-panel-bg:\s*var\(--desktop-chat-panel-bg\);[^}]*--desktop-selection-bg:\s*var\(--desktop-chat-selection-bg\);[^}]*--desktop-user-bubble-bg:\s*var\(--desktop-chat-bubble-bg\);[^}]*--desktop-composer-line:\s*var\(--desktop-chat-composer-line\);[^}]*--desktop-composer-shadow:\s*var\(--desktop-chat-composer-shadow\);[^}]*--desktop-ink:\s*var\(--desktop-chat-ink\);[^}]*--surface-sunken:\s*var\(--desktop-chat-workspace-bg\);[^}]*--surface-active:\s*var\(--desktop-chat-selection-bg\);/s,
        );
        expect(appCss).toMatch(
            /\.app-shell\[data-layout='desktop'\]\[data-view='settings'\]\s*\{[^}]*--desktop-sidebar-bg:\s*var\(--desktop-settings-sidebar-bg\);[^}]*--desktop-workspace-bg:\s*var\(--desktop-settings-workspace-bg\);[^}]*--desktop-panel-bg:\s*var\(--desktop-settings-panel-bg\);[^}]*--desktop-selection-bg:\s*var\(--desktop-settings-selection-bg\);[^}]*--desktop-composer-line:\s*var\(--desktop-settings-control-line\);[^}]*--desktop-ink:\s*var\(--desktop-settings-ink\);[^}]*--surface-sunken:\s*var\(--desktop-settings-sidebar-bg\);[^}]*--surface-raised:\s*var\(--desktop-settings-panel-bg\);[^}]*--surface-active:\s*var\(--desktop-settings-selection-bg\);/s,
        );
        expect(appCss).not.toContain(".app-shell[data-layout='desktop'][data-view='chat'] > .main");
        expect(appCss).toMatch(
            /\.app-shell\[data-layout='desktop'\]\[data-view='chat'\] \.composer-field\s*\{[^}]*box-shadow:\s*0 0 0 0\.5px var\(--desktop-composer-line\),\s*var\(--desktop-composer-shadow\);/s,
        );
        expect(appCss).toMatch(
            /\.app-shell\[data-layout='desktop'\] \.message-actions button\s*\{[^}]*color:\s*var\(--desktop-chat-action-ink\);/s,
        );
        expect(appCss).toMatch(
            /\.app-shell\[data-layout='desktop'\] \.message-actions button:hover:not\(:disabled\)\s*\{[^}]*background:\s*transparent;[^}]*color:\s*var\(--desktop-chat-action-hover-ink\);/s,
        );
        expect(providerSettingsSource).toMatch(
            /\.desktop-settings-card\s*\{[^}]*background:\s*var\(--desktop-workspace-bg\);/s,
        );
        expect(appCss).toMatch(
            /\.app-shell\[data-layout='desktop'\]\[data-view='settings'\] \.setting-list\s*\{[^}]*background:\s*var\(--desktop-workspace-bg\);/s,
        );
        expect(providerSettingsSource).toMatch(
            /\.theme-preview-dark \.theme-preview-canvas\s*\{[^}]*border-color:\s*#464646;[^}]*background:\s*#1f1f1f;/s,
        );
        expect(providerSettingsSource).toMatch(
            /\.theme-preview-dark \.theme-preview-sidebar\s*\{[^}]*background:\s*#1b1b1b;/s,
        );
        expect(providerSettingsSource).toMatch(
            /\.theme-preview-dark \.theme-preview-composer\s*\{[^}]*border-color:\s*#414141;[^}]*background:\s*#282828;/s,
        );
    });

    it('puts every pushed-screen action in the same measured toolbar slot', () => {
        expect(appSource).toMatch(/class="mobile-top-frame mobile-root-header"/);
        expect(
            appSource.match(/class="mobile-top-frame mobile-top-frame-leading sub-header"/g),
        ).toHaveLength(2);
        expect(appSource).toContain('class:studio-detail-scroll={studioSection !== null}');
        expect(appSource).toContain('onscroll={handleStudioDetailScroll}');
        expect(appSource).toContain('onDetailScroll={handlePushedDetailScroll}');
        expect(
            appSource.match(/mobile-top-action mobile-top-action-left back-button/g),
        ).toHaveLength(2);
        expect(conversationPaneSource).toMatch(
            /class="mobile-top-frame mobile-root-header conversation-root-header"/,
        );
        expect(providerSettingsSource).toMatch(/class="mobile-top-frame settings-toolbar"/);
        expect(providerSettingsSource).toMatch(/mobile-top-action settings-tool-button/);
        expect(providerSettingsSource).toMatch(
            /\.settings-toolbar\s*\{[^}]*position:\s*absolute;[^}]*inset:\s*0 0 auto;[^}]*background:\s*transparent;[^}]*pointer-events:\s*none;/s,
        );
        expect(providerSettingsSource).toMatch(
            /\.settings-tool-button\s*\{[^}]*pointer-events:\s*auto;/s,
        );
        expect(providerSettingsSource).toMatch(
            /\.provider-scroll\.settings-home-scroll\s*\{[^}]*padding-top:\s*clamp\(36px,\s*10\.297vw,\s*45px\);[^}]*padding-inline:\s*var\(--settings-gutter\);/s,
        );
        expect(appSource).not.toContain('mobile-detail-title');
        expect(providerSettingsSource).not.toContain('mobile-detail-title');
        expect(providerSettingsSource).not.toContain('showDetailTitle');
        expect(providerSettingsSource).toContain('onscroll={handleSettingsDetailScroll}');
        expect(providerSettingsSource).not.toMatch(/class="settings-dialog"/);
        expect(chatPaneSource).toMatch(
            /class="mobile-top-frame mobile-top-frame-leading chat-header"/,
        );
        expect(chatPaneSource).toMatch(/mobile-top-action mobile-top-action-left back-button/);
        expect(appCss).toMatch(
            /\.mobile-top-frame\s*\{[^}]*height:\s*calc\(var\(--mobile-root-header\) \+ env\(safe-area-inset-top\)\);[^}]*grid-template-columns:\s*minmax\(0,\s*1fr\) auto;[^}]*padding-top:\s*env\(safe-area-inset-top\);[^}]*padding-inline-start:\s*max\(var\(--mobile-top-inset\),\s*env\(safe-area-inset-left\)\);[^}]*padding-inline-end:\s*max\(var\(--mobile-top-inset\),\s*env\(safe-area-inset-right\)\);/s,
        );
        expect(appCss).toMatch(
            /\.mobile-top-frame-leading\s*\{[^}]*grid-template-columns:\s*auto minmax\(0,\s*1fr\) auto;/s,
        );
        expect(appCss).toMatch(
            /\.mobile-top-action\s*\{[^}]*width:\s*var\(--mobile-top-action\);[^}]*height:\s*var\(--mobile-top-action\);[^}]*border-radius:\s*50%;[^}]*background:\s*var\(--surface-raised\);[^}]*box-shadow:\s*var\(--shadow-1\);/s,
        );
        expect(appCss).toMatch(
            /\.app-shell\[data-layout='mobile'\] \.sub-header\s*\{[^}]*position:\s*absolute;[^}]*z-index:\s*10;[^}]*grid-template-columns:\s*var\(--mobile-top-action\) minmax\(0,\s*1fr\) var\(--mobile-top-action\);[^}]*border:\s*0;[^}]*background:\s*transparent;[^}]*inset:\s*0 0 auto;[^}]*pointer-events:\s*none;/s,
        );
        expect(appCss).toMatch(
            /\.app-shell\[data-layout='mobile'\] \.sub-header > \.mobile-top-action\s*\{[^}]*pointer-events:\s*auto;/s,
        );
        expect(appCss).toMatch(
            /\.app-shell\[data-layout='mobile'\] \.sub-header::after\s*\{[^}]*position:\s*absolute;[^}]*z-index:\s*-1;[^}]*height:\s*var\(--mobile-top-fade\);[^}]*background:\s*linear-gradient\(to bottom,\s*var\(--bg\) 0,\s*transparent 100%\);[^}]*opacity:\s*var\(--mobile-top-fade-progress,\s*0\);[^}]*pointer-events:\s*none;/s,
        );
        expect(appCss).not.toMatch(
            /\.app-shell\[data-layout='mobile'\] \.sub-header::after\s*\{[^}]*(?:-webkit-)?mask-(?:image|repeat):/s,
        );
        expect(appCss).toMatch(
            /\.app-shell\[data-layout='mobile'\] :is\(\.studio-detail-scroll, \.settings-detail-scroll\)\s*\{[^}]*padding-top:\s*calc\(\s*env\(safe-area-inset-top\) \+ var\(--mobile-top-offset\) \+ var\(--mobile-top-action\) \+\s*clamp\(7px,\s*3\.661vw,\s*16px\)\s*\);[^}]*scroll-padding-top:/s,
        );
        expect(appCss).toMatch(
            /\.sub-header h1\s*\{[^}]*grid-column:\s*2;[^}]*padding-inline:\s*8px;[^}]*text-align:\s*center;/s,
        );
        expect(appCss).toMatch(
            /\.app-shell\[data-layout='mobile'\] \.sub-header h1\s*\{[^}]*height:\s*var\(--mobile-top-action\);[^}]*display:\s*flex;[^}]*align-self:\s*center;[^}]*justify-content:\s*center;[^}]*padding-inline:\s*0;[^}]*margin:\s*0;[^}]*background:\s*transparent;[^}]*box-shadow:\s*none;/s,
        );
        expect(appCss).toMatch(
            /\.app-shell\[data-layout='mobile'\]\[data-view='chat'\] \.chat-pane \.chat-identity\s*\{[^}]*border-radius:\s*var\(--radius-pill\);[^}]*background:\s*color-mix\(in srgb,\s*var\(--surface-raised\) 94%,\s*transparent\);[^}]*box-shadow:\s*var\(--shadow-1\);/s,
        );
        expect(appCss).toMatch(
            /\.app-shell\[data-layout='desktop'\] \.sub-header\s*\{[^}]*position:\s*relative;[^}]*height:\s*112px;[^}]*align-items:\s*flex-end;[^}]*padding:\s*0 var\(--settings-gutter\) 24px;[^}]*border-bottom:\s*0;[^}]*background:\s*var\(--desktop-workspace-bg\);/s,
        );
    });

    it('keeps the interactive-back dimmer off the desktop workspace', () => {
        expect(appCss).toMatch(
            /\.app-shell\[data-layout='mobile'\] > \.back-swipe-underlay::after\s*\{[^}]*opacity:\s*var\(--back-swipe-underlay-dim\);/s,
        );
        expect(appCss).toMatch(
            /\.app-shell\[data-layout='desktop'\] > \.back-swipe-underlay\s*\{[^}]*display:\s*none;/s,
        );
        expect(appCss).not.toMatch(/(?:^|\n)\.back-swipe-underlay::after\s*\{/);
    });

    it('measures desktop scrollbars for aligned fixed actions and chat fields', () => {
        expect(appCss).toMatch(
            /\.app-shell\[data-layout='desktop'\] :is\(\.view-scroll, \.provider-scroll\)\s*\{[^}]*scrollbar-gutter:\s*auto;/s,
        );
        expect(appSource).toContain('use:syncDetailActionViewport');
        expect(appSource).toContain("'--detail-action-center'");
        expect(appSource).toContain("'--detail-action-workspace-width'");
        expect(chatPaneSource).toContain('use:syncMessageScrollbarInset');
        expect(chatPaneSource).toContain("'--message-scrollbar-width'");
        expect(appCss).toMatch(
            /\.app-shell\[data-layout='desktop'\]\[data-view='chat'\] \.composer\s*\{[^}]*padding-right:\s*var\(--chat-side-inset\);/s,
        );
    });

    it.each([
        ['global app styles', appCss],
        ['conversation pane styles', conversationPaneSource],
    ])('keeps every %s hover rule on devices with a fine pointer', (_, source) => {
        const ranges = blockRanges(source, FINE_POINTER_MEDIA);
        const indexes = hoverIndexes(source);

        expect(indexes.length).toBeGreaterThan(0);
        expect(
            indexes.every((index) => ranges.some(([start, end]) => index > start && index < end)),
        ).toBe(true);
    });

    it('routes create destinations through the shared settings-row hover treatment', () => {
        expect(orchestrationStudioSource).toContain('class="setting-row studio-destination-row"');
        expect(appCss).toMatch(
            /\.setting-row:hover:not\(:disabled\)\s*\{[^}]*background:\s*var\(--bg\);/s,
        );
    });

    it('keeps message actions icon-only and gives timestamps a legible hierarchy', () => {
        expect(chatPaneSource).toContain('<Copy aria-hidden="true" />');
        expect(chatPaneSource).toContain('<GitBranch aria-hidden="true" />');
        expect(chatPaneSource).toContain('<Pencil aria-hidden="true" />');
        expect(chatPaneSource).toContain('<RefreshCw aria-hidden="true" />');
        expect(chatPaneSource).toContain('<Trash2 aria-hidden="true" />');
        expect(appCss).toMatch(
            /\.message-actions button\s*\{[^}]*width:\s*30px;[^}]*height:\s*30px;[^}]*place-items:\s*center;/s,
        );
        expect(appCss).toMatch(
            /\.message-actions button svg\s*\{[^}]*width:\s*15px;[^}]*height:\s*15px;[^}]*stroke-width:\s*1\.8;/s,
        );
        expect(appCss).toMatch(
            /\.app-shell\[data-layout='desktop'\] \.message-actions\s*\{[^}]*position:\s*static;[^}]*min-height:\s*30px;[^}]*padding:\s*0;[^}]*border:\s*0;[^}]*border-radius:\s*0;[^}]*background:\s*transparent;[^}]*box-shadow:\s*none;/s,
        );
        expect(chatPaneSource).toContain(
            'class:actions-hovered={hoveredMessageActionId === message.id}',
        );
        expect(chatPaneSource).toContain('onmouseenter={() => hoverMessageActions(message.id)}');
        expect(chatPaneSource).toContain('onmouseleave={() => unhoverMessageActions(message.id)}');
        expect(appCss).toMatch(
            /\.app-shell\[data-layout='desktop'\] \.message-item\.actions-hovered \.message-actions\s*\{[^}]*opacity:\s*1;[^}]*pointer-events:\s*auto;/s,
        );
        expect(appCss).not.toMatch(/(?:^|\n)\.message-item:focus-within \.message-actions\s*\{/);
        expect(appCss).toMatch(
            /\.message-date-chip\s*\{[^}]*font-size:\s*0\.75rem;[^}]*font-weight:\s*650;/s,
        );
        expect(appCss).toMatch(
            /\.message-time\s*\{[^}]*font-size:\s*0\.6875rem;[^}]*font-weight:\s*500;/s,
        );
        expect(appCss).toMatch(
            /\.app-shell\[data-layout='mobile'\]\[data-view='chat'\] \.message-date-chip\s*\{[^}]*font-size:\s*clamp\(10px,\s*3\.204vw,\s*12px\);/s,
        );
        expect(chatPaneSource).not.toContain('message-date-follower');
        expect(appCss).toMatch(
            /\.message-date-divider\s*\{[^}]*position:\s*sticky;[^}]*z-index:\s*11;[^}]*top:\s*0;[^}]*display:\s*flex;/s,
        );
        expect(appCss).toMatch(
            /\.app-shell\[data-layout='mobile'\]\[data-view='chat'\] \.message-date-divider\s*\{[^}]*top:\s*calc\(0px - clamp\(10px,\s*4\.119vw,\s*18px\)\);[^}]*padding:\s*0 0 clamp\(3px,\s*1\.831vw,\s*8px\);/s,
        );
        expect(appCss).not.toContain('.message-date-follower');
        expect(appCss).toMatch(
            /\.app-shell\[data-layout='mobile'\]\[data-view='chat'\] \.message-time\s*\{[^}]*font-size:\s*clamp\(9px,\s*2\.975vw,\s*11px\);/s,
        );
    });

    it('floats the white composer without changing the message bubble treatment', () => {
        expect(appCss).not.toContain('--bubble-shadow');
        expect(appCss).toMatch(
            /\.composer-field\s*\{[^}]*background:\s*var\(--surface-raised\);[^}]*box-shadow:\s*var\(--shadow-2\);/s,
        );
        expect(appCss).toMatch(
            /\.app-shell\[data-layout='mobile'\]\[data-view='chat'\] \.message-item \.message-body\s*\{[^}]*background:\s*var\(--bubble-char-bg\);[^}]*box-shadow:\s*var\(--shadow-1\);/s,
        );
    });

    it('spaces transcript turns at 2.5x and reveals mobile message actions as one motion', () => {
        const mobileMessageActions =
            /\.app-shell\[data-layout='mobile'\]\[data-view='chat'\] \.message-item \.message-actions\s*\{(?<body>[^}]*)\}/s.exec(
                appCss,
            )?.groups?.body ?? '';

        expect(appCss).toMatch(
            /\.message-item\s*\{[^}]*--message-turn-spacing:\s*15px;[^}]*padding:\s*var\(--message-turn-spacing\) 0;/s,
        );
        expect(appCss).toMatch(
            /\.app-shell\[data-layout='mobile'\]\[data-view='chat'\] \.message-item\s*\{[^}]*--message-turn-spacing:\s*clamp\(5px,\s*2\.86vw,\s*12\.5px\);/s,
        );
        expect(appCss).toMatch(/\.message-scroll\s*\{[^}]*overflow-anchor:\s*none;/s);
        expect(mobileMessageActions).toContain('overflow-anchor: none;');
        expect(mobileMessageActions).toContain(
            '--message-action-size: clamp(26px, 8.696vw, 32px);',
        );
        expect(mobileMessageActions).toContain('--message-actions-duration: 360ms;');
        expect(mobileMessageActions).toContain(
            '--message-actions-easing: cubic-bezier(0.16, 1, 0.3, 1);',
        );
        expect(mobileMessageActions).not.toContain('position: absolute;');
        expect(mobileMessageActions).not.toMatch(/(?:^|\n)\s*top:/);
        expect(mobileMessageActions).toContain(
            'max-height var(--message-actions-duration) var(--message-actions-easing)',
        );
        expect(mobileMessageActions).toContain(
            'padding-top var(--message-actions-duration) var(--message-actions-easing)',
        );
        expect(mobileMessageActions).not.toContain('clip-path');
        expect(mobileMessageActions).not.toContain('transform:');
        expect(appCss).toMatch(
            /\.app-shell\[data-layout='mobile'\]\[data-view='chat'\][\s\S]*?\.message-item:is\(\.actions-open,\s*:focus-within\)[\s\S]*?\.message-actions\s*\{[^}]*max-height:\s*var\(--message-action-size\);[^}]*padding-top:\s*4px;[^}]*opacity:\s*1;/s,
        );
    });

    it('omits decorative right arrows from every destination row', () => {
        for (const source of [
            providerSettingsSource,
            personaPanelSource,
            orchestrationStudioSource,
        ]) {
            expect(source).not.toContain('setting-chevron');
        }
        expect(conversationPaneSource).not.toContain('<span aria-hidden="true">›</span>');
        expect(appCss).not.toContain('.setting-chevron');
    });

    it('pins the mobile tab bar over page content instead of reserving a layout row', () => {
        expect(appCss).toMatch(
            /\.tab-bar\s*\{[^}]*position:\s*absolute;[^}]*bottom:\s*calc\(clamp\(4px,\s*1\.831vw,\s*8px\) \+ env\(safe-area-inset-bottom\)\);[^}]*left:\s*50%;[^}]*width:\s*min\(calc\(100% - var\(--gutter\) - var\(--gutter\)\),\s*560px\);[^}]*transform:\s*translateX\(-50%\);/s,
        );
        expect(appCss).toMatch(/\.tab-bar\s*\{[^}]*margin:\s*0;/s);
    });

    it('hides the mobile tab bar only for the active pushed screen', () => {
        expect(appSource).toMatch(
            /\{#if\s+!isDesktop\s+&&\s+!\(view === 'create' && studioSection !== null\)\s+&&\s+!\(view === 'chat' && chatThreadOpen\)\s+&&\s+!\(view === 'settings' && settingsSection !== null\)\s*\}\s*<nav class="tab-bar"/s,
        );
        expect(appSource).not.toMatch(
            /\{#if[^}]*settingsSection === 'persona'[^}]*\}\s*<nav class="tab-bar"/s,
        );
    });

    it('uses one full-height scrolling detail shell with the shared fade contract', () => {
        expect(detailPageSource).toContain('getContext<DetailScrollListener | undefined>');
        expect(detailPageSource).toContain('inheritedOnScroll?.(scrollTop)');
        expect(detailPageSource).not.toContain('mask');
        expect(providerSettingsSource).toContain('onDetailScroll(scroller.scrollTop)');
        expect(detailPageSource).toMatch(
            /<section class=\{`detail-page \$\{className\}`\.trim\(\)\} aria-label=\{ariaLabel\}>[\s\S]*?<div[\s\S]*?class=\{`detail-page-scroll \$\{scrollClassName\}`\.trim\(\)\}[\s\S]*?onscroll=\{handleScroll\}[\s\S]*?>/s,
        );
        expect(detailPageSource).toMatch(
            /\.detail-page\s*\{[^}]*position:\s*relative;[^}]*display:\s*flex;[^}]*height:\s*100%;[^}]*min-height:\s*0;[^}]*flex-direction:\s*column;/s,
        );
        expect(detailPageSource).toMatch(
            /\.detail-page-scroll\s*\{[^}]*display:\s*grid;[^}]*height:\s*0;[^}]*min-height:\s*0;[^}]*flex:\s*1 1 0;[^}]*align-content:\s*start;[^}]*padding:\s*16px var\(--settings-gutter\)\s*calc\(24px \+ env\(safe-area-inset-bottom\)\);[^}]*overflow-y:\s*auto;/s,
        );
        expect(detailPageSource).toMatch(
            /\.detail-page-scroll\.detail-page-has-actions\s*\{[^}]*padding-bottom:\s*calc\(var\(--mobile-nav\) \+ 36px \+ env\(safe-area-inset-bottom\)\);/s,
        );
        expect(providerSettingsSource).toMatch(
            /:global\(\.app-shell\[data-layout='mobile'\]\)\s*\.settings-detail-scroll\.detail-scroll-has-actions\s*\{[^}]*padding-bottom:\s*calc\(\s*var\(--mobile-nav\) \+ clamp\(15px,\s*8\.238vw,\s*36px\) \+ env\(safe-area-inset-bottom\)\s*\);/s,
        );
        expect(
            providerSettingsSource.indexOf(
                '.settings-detail-scroll.detail-scroll-has-actions {',
                providerSettingsSource.indexOf(":global(.app-shell[data-layout='mobile'])"),
            ),
        ).toBeGreaterThan(
            providerSettingsSource.indexOf(
                ":global(.app-shell[data-layout='mobile']) .settings-detail-scroll {",
            ),
        );
    });

    it('keeps discovery evidence inside the single page scroller', () => {
        expect(discoveryPanelSource).toMatch(
            /pre,\s*code\s*\{[^}]*max-height:\s*none;[^}]*overflow-x:\s*auto;[^}]*overflow-y:\s*visible;/s,
        );
        expect(discoveryPanelSource).not.toMatch(/pre,\s*code\s*\{[^}]*overflow:\s*auto;/s);
    });

    it('keeps detail actions in the floating Persona proportions and source order', () => {
        expect(detailActionBarSource).toMatch(
            /\.detail-action-bar\s*\{[^}]*position:\s*absolute;[^}]*bottom:\s*calc\(8px \+ env\(safe-area-inset-bottom\)\);[^}]*left:\s*50%;[^}]*width:\s*min\(calc\(100% - var\(--gutter\) - var\(--gutter\)\),\s*560px\);[^}]*height:\s*var\(--mobile-nav\);[^}]*min-height:\s*var\(--mobile-nav\);[^}]*background:\s*transparent;[^}]*gap:\s*clamp\(4px,\s*1\.144vw,\s*6px\);[^}]*transform:\s*translateX\(-50%\);/s,
        );
        expect(detailActionBarSource).toMatch(
            /\.detail-action-bar :global\(\.detail-action\)\s*\{[^}]*height:\s*100%;[^}]*min-height:\s*0;[^}]*flex:\s*1;[^}]*padding:\s*0 clamp\(12px,\s*3\.661vw,\s*16px\);[^}]*border-radius:\s*var\(--radius-pill\);[^}]*font-size:\s*var\(--detail-support-type\);[^}]*font-weight:\s*700;/s,
        );
        expect(detailActionBarSource).toMatch(
            /\.detail-action-bar :global\(\.detail-action--grow\)\s*\{[^}]*flex:\s*2;/s,
        );
        expect(detailActionBarSource).toMatch(
            /\.detail-action-bar :global\(\.detail-action--wide\)\s*\{[^}]*flex:\s*1;/s,
        );
        expect(detailActionBarSource).toMatch(
            /\.detail-action-bar :global\(\.detail-action--destructive\)\s*\{[^}]*color:\s*var\(--status-error-fg\);/s,
        );
        expect(detailActionBarSource).toMatch(
            /\.detail-action-bar :global\(\.detail-action--borderless\)\s*\{[^}]*border:\s*0;/s,
        );
        expect(detailActionBarSource).toContain('{@render children()}');
        expect(detailActionBarSource).not.toMatch(/flex-direction:\s*row-reverse/);
        expect(personaPanelSource.indexOf('persona-delete-button')).toBeLessThan(
            personaPanelSource.indexOf('persona-save-action'),
        );
    });

    it('keeps pushed-screen fields neutral on hover and brand-accented only on focus', () => {
        expect(personaPanelSource).toMatch(
            /\.persona-form :is\(input, textarea\):hover:not\(:focus, :disabled\)\s*\{[^}]*border-color:\s*var\(--line\);/s,
        );
        expect(personaPanelSource).toMatch(
            /\.persona-form :is\(input, textarea\):focus\s*\{[^}]*border-color:\s*var\(--accent\);[^}]*outline:\s*none;/s,
        );
        expect(choicePopoverSource).toMatch(
            /\.choice-trigger\.field\s*\{[^}]*border:\s*1\.5px solid var\(--line\);/s,
        );
        expect(choicePopoverSource).toMatch(
            /\.choice-trigger\.field:focus-visible\s*\{[^}]*border-color:\s*var\(--accent\);/s,
        );
    });

    it('keeps wide handhelds fluid and animates the desktop sidebar without stale grid tracks', () => {
        expect(appCss).toMatch(
            /\.app-shell\[data-layout='mobile'\]\s*\{[^}]*width:\s*min\(100%,\s*899px\);[^}]*margin-inline:\s*auto;/s,
        );
        expect(appCss).toMatch(
            /\.app-shell\s*\{[^}]*display:\s*flex;[^}]*overflow:\s*clip;[^}]*contain:\s*layout;/s,
        );
        expect(appCss).toMatch(
            /\.app-shell\[data-layout='desktop'\] \.sidebar-rail\s*\{[^}]*width:\s*var\(--sidebar\);[^}]*flex-basis:\s*var\(--sidebar\);[^}]*transition-duration:\s*300ms;[^}]*transition-timing-function:\s*cubic-bezier\(0\.22,\s*0\.61,\s*0\.36,\s*1\);/s,
        );
        expect(appCss).toMatch(
            /\.sidebar-rail\s*\{[^}]*width:\s*0;[^}]*flex:\s*0 0 0;[^}]*overflow:\s*hidden;[^}]*transition:\s*width 240ms[^;]*,\s*flex-basis 240ms/s,
        );
        expect(appCss).toMatch(
            /\.sidebar-view-switcher\s*\{[^}]*display:\s*grid;[^}]*grid-template-columns:\s*repeat\(2,\s*minmax\(0,\s*1fr\)\);/s,
        );
        expect(appCss).toMatch(
            /\.sidebar-view-thumb\s*\{[^}]*position:\s*absolute;[^}]*width:\s*calc\(\(100% - 4px\) \/ 2\);[^}]*background:\s*var\(--surface-raised\);[^}]*transform:\s*translate3d\(0,\s*0,\s*0\);[^}]*transition:\s*transform 260ms cubic-bezier\(0\.22,\s*1,\s*0\.36,\s*1\);/s,
        );
        expect(appCss).toMatch(
            /\.sidebar-view-switcher\[data-section='conversations'\] \.sidebar-view-thumb\s*\{[^}]*transform:\s*translate3d\(100%,\s*0,\s*0\);/s,
        );
        expect(appCss).not.toMatch(/\.sidebar-view-switcher::before\s*\{/);
        expect(appCss).toMatch(
            /\.sidebar-view-panels\s*\{[^}]*display:\s*grid;[^}]*min-height:\s*0;[^}]*flex:\s*1;[^}]*overflow:\s*hidden;/s,
        );
        expect(appCss).toMatch(
            /\.sidebar-view-panel\s*\{[^}]*grid-area:\s*1 \/ 1;[^}]*opacity:\s*0;[^}]*visibility:\s*hidden;[^}]*pointer-events:\s*none;[^}]*transition:\s*opacity 160ms ease,[^;]*transform 240ms cubic-bezier\(0\.22,\s*1,\s*0\.36,\s*1\),[^;]*visibility 0s linear 240ms;/s,
        );
        expect(appCss).toMatch(
            /\.sidebar-view-panel\[aria-hidden='false'\]\s*\{[^}]*opacity:\s*1;[^}]*visibility:\s*visible;[^}]*pointer-events:\s*auto;[^}]*transform:\s*translate3d\(0,\s*0,\s*0\);/s,
        );
        expect(appCss).toMatch(
            /\.sidebar-view-option\[aria-pressed='true'\]\s*\{[^}]*background:\s*transparent;[^}]*color:\s*var\(--ink\);/s,
        );
        expect(appCss).toMatch(
            /@media \(min-width:\s*900px\)\s*\{\s*:root\s*\{[^}]*font-size:\s*13px;/s,
        );
        expect(appCss).toMatch(
            /\.app-shell\[data-layout='desktop'\]\s*\{[^}]*--sidebar:\s*280px;[^}]*--touch:\s*32px;[^}]*--detail-ui-type:\s*13px;[^}]*--detail-support-type:\s*12px;/s,
        );
        expect(appCss).toMatch(
            /\.app-shell\[data-layout='desktop'\]\s*\{[^}]*--reading:\s*736px;[^}]*--settings:\s*768px;[^}]*--gutter:\s*max\(16px,\s*calc\(\(100% - var\(--reading\)\) \/ 2\)\);/s,
        );
        expect(appCss).toMatch(/\.sidebar-head\s*\{[^}]*justify-content:\s*flex-start;/s);
        expect(libraryPaneSource).toMatch(
            /:global\(\.app-shell\[data-layout='desktop'\]\) \.library-search\s*\{[^}]*min-height:\s*30px;[^}]*box-shadow:\s*none;/s,
        );
        expect(appCss).toMatch(
            /\.app-shell\[data-layout='desktop'\] \.setting-row\s*\{[^}]*position:\s*relative;[^}]*min-height:\s*52px;[^}]*padding:\s*9px 16px;[^}]*border-bottom:\s*0;[^}]*gap:\s*14px;/s,
        );
        expect(appCss).toMatch(
            /\.app-shell\[data-layout='desktop'\] \.setting-list > li \+ li > \.setting-row::before,[\s\S]*?\.app-shell\[data-layout='desktop'\] \.setting-list > \.setting-row \+ li > \.setting-row::before\s*\{[^}]*right:\s*16px;[^}]*left:\s*16px;[^}]*height:\s*1px;[^}]*background:\s*var\(--desktop-divider\);/s,
        );
        expect(appSource).toContain("aria-label={$tr('app.sidebar.switcher')}");
        expect(appSource).toContain('data-section={homeSection}');
        expect(appSource).toContain('id="sidebar-character-list"');
        expect(appSource).toContain('id="sidebar-chat-list"');
        expect(appCss).toMatch(/\.main\s*\{[^}]*display:\s*flex;[^}]*width:\s*0;[^}]*flex:\s*1;/s);
        expect(appSource).toContain("const DESKTOP_LAYOUT = '(min-width: 900px)'");
        expect(appSource).toContain("data-layout={isDesktop ? 'desktop' : 'mobile'}");
        expect(appSource).toContain('let sidebarContentMounted = $state(false)');
        expect(appSource).toContain('const SIDEBAR_EXIT_SETTLE_MS = 260');
        expect(appSource).toContain('sidebarUnmountTimer = setTimeout');
        expect(appSource).not.toContain("from 'svelte/transition'");
        expect(appSource).toContain("const REDUCED_MOTION = '(prefers-reduced-motion: reduce)'");
    });

    it('caps mobile density continuously through Fold and tablet widths', () => {
        expect(appCss).toMatch(
            /@media \(max-width:\s*899px\)\s*\{[\s\S]*?:root\s*\{[^}]*font-size:\s*clamp\(9px,\s*3\.661vw,\s*15px\);[\s\S]*?\.app-shell\[data-layout='mobile'\]\s*\{[^}]*--mobile-root-header:\s*clamp\(40px,\s*16\.476vw,\s*60px\);[^}]*--mobile-top-action:\s*clamp\(30px,\s*12\.18vw,\s*44px\);[^}]*--mobile-pill-control:\s*clamp\(26px,\s*10\.526vw,\s*36px\);[^}]*--mobile-nav:\s*clamp\(37px,\s*15\.103vw,\s*60px\);[^}]*--reading:\s*560px;[^}]*--settings:\s*560px;/s,
        );
        expect(appCss).toMatch(
            /@media \(max-width:\s*899px\)[\s\S]*?\.mobile-root-header h1\s*\{[^}]*font-size:\s*clamp\(14px,\s*5\.72vw,\s*22px\);/s,
        );
        for (const source of [libraryPaneSource, conversationPaneSource]) {
            expect(source).toMatch(
                /@media \(max-width:\s*899px\)[\s\S]*?\.mobile-root-row\s*\{[^}]*min-height:\s*clamp\(46px,\s*19\.222vw,\s*68px\);[^}]*padding:\s*clamp\(4px,\s*1\.831vw,\s*6px\) clamp\(10px,\s*4\.119vw,\s*16px\);/s,
            );
            expect(source).toMatch(
                /@media \(max-width:\s*899px\)[\s\S]*?\.mobile-root-row[\s\S]*?\.avatar\s*\{[^}]*width:\s*clamp\(35px,\s*14\.645vw,\s*52px\);[^}]*height:\s*clamp\(35px,\s*14\.645vw,\s*52px\);/s,
            );
        }
        expect(conversationPaneSource).toMatch(
            /@media \(max-width:\s*899px\)[\s\S]*?\.conversation-filter-strip\s*\{[^}]*min-height:\s*clamp\(37px,\s*15\.561vw,\s*52px\);/s,
        );
        expect(providerSettingsSource).toMatch(
            /@media \(max-width:\s*899px\)[\s\S]*?\.settings-avatar-wrap\s*\{[^}]*width:\s*clamp\(59px,\s*24\.714vw,\s*88px\);[^}]*height:\s*clamp\(59px,\s*24\.714vw,\s*88px\);/s,
        );
        expect(appCss).not.toContain('@media (min-width: 600px) and (max-width: 899px)');
    });

    it('hides mobile scrollbars without disabling native wheel or touch scrolling', () => {
        expect(providerSettingsSource).toMatch(
            /\.provider-scroll\s*\{[^}]*height:\s*0;[^}]*min-height:\s*0;[^}]*flex:\s*1 1 0;[^}]*overflow-y:\s*auto;/s,
        );
        expect(detailPageSource).toMatch(
            /\.detail-page-scroll\s*\{[^}]*height:\s*0;[^}]*min-height:\s*0;[^}]*flex:\s*1 1 0;[^}]*overflow-y:\s*auto;/s,
        );
        expect(appCss).toMatch(
            /\.app-shell\[data-layout='mobile'\],\s*\.app-shell\[data-layout='mobile'\] \*\s*\{[^}]*scrollbar-width:\s*none;/s,
        );
        expect(appCss).toMatch(
            /\.app-shell\[data-layout='mobile'\]::-webkit-scrollbar,\s*\.app-shell\[data-layout='mobile'\] \*::-webkit-scrollbar\s*\{[^}]*display:\s*none;[^}]*width:\s*0;[^}]*height:\s*0;/s,
        );
        expect(appCss).toMatch(
            /\.app-shell\[data-layout='mobile'\][^{}]*:where\(\.navigator,\s*\.view-scroll,\s*\.provider-scroll,\s*\.entity-list,\s*\.message-scroll,\s*\.modal-card\)\s*\{[^}]*-webkit-overflow-scrolling:\s*touch;[^}]*touch-action:\s*pan-y;/s,
        );
        expect(appCss).toMatch(
            /\.app-shell\[data-layout='desktop'\][^{}]*:is\(\.message-scroll, \.quick-drawer\.desktop \.drawer-body\)\s*\{[^}]*scrollbar-gutter:\s*stable;[^}]*scrollbar-width:\s*thin;/s,
        );
        expect(appCss).toMatch(
            /\.app-shell\[data-layout='desktop'\][^{}]*:is\(\.message-scroll, \.quick-drawer\.desktop \.drawer-body\)::-webkit-scrollbar-thumb\s*\{[^}]*background-clip:\s*padding-box;/s,
        );
    });

    it('keeps symmetric page gutters after scrollbar chrome is hidden', () => {
        expect(providerSettingsSource).toMatch(
            /\.provider-scroll\.settings-home-scroll\s*\{[^}]*padding-inline:\s*var\(--settings-gutter\);/s,
        );
        expect(providerSettingsSource).not.toMatch(
            /\.provider-scroll\.settings-home-scroll\s*\{[^}]*padding-inline:\s*var\(--settings-gutter\)\s+0;/s,
        );
        expect(appCss).toMatch(
            /\.view-scroll\s*\{[^}]*padding:\s*16px var\(--settings-gutter\) 24px;/s,
        );
        expect(appCss).toMatch(
            /\.app-shell\[data-layout='desktop'\]\[data-view='create'\]\s*\.view-scroll:not\(\.studio-detail-scroll\)\s*\{[^}]*padding-top:\s*clamp\(52px,\s*7\.4vh,\s*68px\);/s,
        );
        expect(libraryPaneSource).toMatch(
            /\.library-pane\.root-view \.entity-list\s*\{[^}]*padding:\s*0 8px calc\(/s,
        );
        expect(conversationPaneSource).toMatch(
            /\.conversation-pane\.root-view \.entity-list\s*\{[^}]*padding:\s*8px 8px calc\(/s,
        );
        expect(appCss).toMatch(
            /\.message-scroll\s*\{[^}]*padding:\s*0 var\(--chat-side-inset\)\s*calc\(var\(--composer-overlay-height\) \+ var\(--chat-side-inset\)\);/s,
        );
        expect(providerSettingsSource).toMatch(
            /\.provider-scroll\.settings-home-scroll\s*\{[^}]*display:\s*flex;[^}]*flex-direction:\s*column;[^}]*padding-inline:\s*var\(--settings-gutter\);/s,
        );
        expect(providerSettingsSource).toMatch(
            /\.settings-home-scroll \.setting-list\s*\{[^}]*flex:\s*none;[^}]*justify-content:\s*flex-start;/s,
        );
        expect(providerSettingsSource).toMatch(
            /:global\(\.app-shell\[data-layout='mobile'\]\) \.provider-scroll\.settings-home-scroll\s*\{[^}]*padding-bottom:/s,
        );
        expect(providerSettingsSource).toMatch(
            /:global\(\.app-shell\[data-layout='mobile'\]\) \.settings-home-scroll \.setting-list\s*\{[^}]*margin-bottom:/s,
        );
    });

    it('keeps the desktop settings title at the top-left without the profile mark', () => {
        expect(providerSettingsSource).toMatch(
            /:global\(\.app-shell\[data-layout='desktop'\]\) \.provider-scroll\.settings-home-scroll\s*\{[^}]*padding-top:\s*clamp\(52px,\s*7\.4vh,\s*68px\);/s,
        );
        expect(providerSettingsSource).toMatch(
            /:global\(\.app-shell\[data-layout='desktop'\]\) \.settings-identity\s*\{[^}]*min-height:\s*0;[^}]*justify-items:\s*start;[^}]*text-align:\s*left;/s,
        );
        expect(providerSettingsSource).toMatch(
            /:global\(\.app-shell\[data-layout='desktop'\]\) \.settings-avatar-wrap\s*\{[^}]*display:\s*none;/s,
        );
        expect(providerSettingsSource).toMatch(
            /:global\(\.app-shell\[data-layout='desktop'\]\) \.settings-identity-copy\s*\{[^}]*justify-items:\s*start;/s,
        );
        expect(providerSettingsSource).toMatch(
            /:global\(\.app-shell\[data-layout='desktop'\]\) \.settings-identity-copy h2\s*\{[^}]*font-size:\s*28px;[^}]*font-weight:\s*600;/s,
        );
    });

    it('uses a quiet desktop settings selection and inset separators', () => {
        expect(desktopSettingsSidebarSource).toMatch(
            /\.settings-destination-row\[aria-current='page'\]\s*\{[^}]*background:\s*var\(--surface-active\);[^}]*color:\s*var\(--ink\);/s,
        );
        expect(desktopSettingsSidebarSource).not.toMatch(
            /\.settings-destination-row\[aria-current='page'\]\s*\{[^}]*box-shadow:/s,
        );
        expect(desktopSettingsSidebarSource).not.toContain(
            ".settings-destination-row[aria-current='page']::before",
        );
        expect(desktopSettingsSidebarSource).toMatch(
            /\.settings-destination-row:focus-visible\s*\{[^}]*outline:\s*none;/s,
        );
        expect(desktopSettingsSidebarSource).not.toContain('.settings-destination-row:hover');
        expect(providerSettingsSource).not.toContain('.desktop-settings-summary-row:hover');
        expect(appCss).toContain(
            ".app-shell:not([data-layout='desktop'][data-view='settings']) button:hover:not(:disabled)",
        );
        expect(choicePopoverSource).toContain(
            ":global(.app-shell:not([data-layout='desktop'][data-view='settings']))",
        );
        expect(providerSettingsSource).toMatch(
            /\.desktop-settings-summary-row \+ \.desktop-settings-summary-row,[\s\S]*?\.desktop-settings-static-row \+ \.desktop-settings-summary-row\s*\{[^}]*border-top:\s*0;/s,
        );
        expect(providerSettingsSource).toMatch(
            /\.desktop-settings-summary-row \+ \.desktop-settings-summary-row::before,[\s\S]*?\.desktop-settings-static-row \+ \.desktop-settings-summary-row::before\s*\{[^}]*right:\s*14px;[^}]*left:\s*14px;[^}]*height:\s*1px;[^}]*background:\s*var\(--line\);/s,
        );
    });

    it('uses shared root-screen geometry for home and chat', () => {
        expect(libraryPaneSource).toMatch(/class="library-search"/);
        expect(libraryPaneSource).not.toContain('class:mobile-root-search={rootView}');
        expect(libraryPaneSource).toMatch(
            /class="mobile-top-frame mobile-root-header library-root-header"/,
        );
        expect(conversationPaneSource).toMatch(
            /class="mobile-top-frame mobile-root-header conversation-root-header"/,
        );
        expect(conversationPaneSource).toMatch(
            /class="conversation-search conversation-top-search"[\s\S]*?role="search"/,
        );
        expect(conversationPaneSource).not.toMatch(
            /class="conversation-search mobile-root-search"/,
        );
        expect(conversationPaneSource).toMatch(/@keyframes conversation-search-expand/);
        expect(conversationPaneSource).toMatch(/@keyframes conversation-search-collapse/);
        expect(libraryPaneSource).toMatch(/class="library-top-search"[\s\S]*?role="search"/);
        expect(libraryPaneSource).toMatch(/@keyframes library-search-expand/);
        expect(libraryPaneSource).toMatch(/@keyframes library-search-collapse/);
        expect(conversationPaneSource).toMatch(
            /\.conversation-filter-pill\s*\{[^}]*height:\s*var\(--mobile-pill-control\);[^}]*min-height:\s*var\(--mobile-pill-control\);/s,
        );
        expect(libraryPaneSource).toMatch(
            /\.library-filter-pill\s*\{[^}]*height:\s*var\(--mobile-pill-control\);[^}]*min-height:\s*var\(--mobile-pill-control\);/s,
        );
        expect(conversationPaneSource).toContain(
            '.conversation-filter-pill:hover:not(:disabled):not(.active)',
        );
        expect(libraryPaneSource).toContain(
            '.library-filter-pill:hover:not(:disabled):not(.active)',
        );
        expect(conversationPaneSource).not.toMatch(
            /\.conversation-filter-pill(?:\.active)?:hover[^{}]*\{[^}]*translateY/s,
        );
        expect(libraryPaneSource).not.toMatch(
            /\.library-filter-pill(?:\.active)?:hover[^{}]*\{[^}]*translateY/s,
        );
        expect(libraryPaneSource).toMatch(
            /\.library-pane\.root-view\s*\.mobile-root-row\.active:hover\s*\{[^}]*background:\s*var\(--surface-hover\);/s,
        );
        expect(conversationPaneSource).toMatch(
            /\.conversation-pane\.root-view\s*\.mobile-root-row\.active:hover\s*\{[^}]*background:\s*var\(--surface-hover\);/s,
        );
        expect(conversationPaneSource).toMatch(
            /\.conversation-preview\s*\{[^}]*padding-right:\s*clamp\(14px,\s*5\.492vw,\s*24px\);/s,
        );
        expect(libraryPaneSource).toMatch(/class="mobile-root-actions"/);
        expect(conversationPaneSource).toMatch(/class="mobile-root-actions"/);
        expect(libraryPaneSource).not.toContain('mobile-root-fab');
        expect(conversationPaneSource).not.toContain('mobile-root-fab');
        expect(libraryPaneSource).toMatch(/class:mobile-root-row=\{rootView\}/);
        expect(conversationPaneSource).toMatch(/class:mobile-root-row=\{rootView\}/);
        expect(appCss).toMatch(/\.chat-list-view\s*\{[^}]*background:\s*var\(--bg\);/s);
        expect(appCss).toMatch(
            /\.app-shell\[data-layout='mobile'\] \.mobile-root-search\s*\{[^}]*min-height:\s*var\(--mobile-search\);[^}]*border:\s*1px solid var\(--line\);[^}]*width:\s*min\([\s\S]*?var\(--reading\)[\s\S]*?\);[^}]*margin:\s*0 auto clamp\(4px,\s*1\.831vw,\s*8px\);[^}]*background:\s*var\(--surface-raised\);[^}]*box-shadow:\s*var\(--shadow-1\);/s,
        );
        expect(libraryPaneSource).not.toContain('mobile-root-empty');
        expect(conversationPaneSource).not.toContain('mobile-root-empty');
        expect(appCss).not.toContain('.mobile-root-empty');
        expect(appCss).not.toContain('.mobile-root-contact-action');
        expect(appCss).not.toContain('.mobile-root-contact-button');
        expect(appCss).not.toContain('.mobile-root-fab');
        expect(appCss).toMatch(
            /\.mobile-root-actions\s*\{[^}]*display:\s*flex;[^}]*grid-column:\s*2;[^}]*align-self:\s*center;[^}]*margin-top:\s*0;/s,
        );
        expect(appCss).toMatch(
            /\.mobile-root-header \.mobile-top-action,\s*\.settings-toolbar \.mobile-top-action\s*\{[^}]*width:\s*var\(--mobile-top-action\);[^}]*height:\s*var\(--mobile-top-action\);[^}]*min-width:\s*var\(--mobile-top-action\);[^}]*min-height:\s*var\(--mobile-top-action\);/s,
        );
        expect(appCss).toMatch(/\.mobile-top-add-action\s*\{[^}]*color:\s*var\(--ink\);/s);
        expect(appCss).toMatch(
            /\.mobile-root-header h1\s*\{[^}]*grid-column:\s*1;[^}]*align-self:\s*center;/s,
        );
        expect(appCss).toMatch(
            /\.app-shell\[data-layout='mobile'\] \.mobile-root-row\s*\{[^}]*min-height:\s*var\(--mobile-row\);[^}]*border-radius:\s*var\(--radius-md\);/s,
        );
    });

    it('matches the Telegram pushed-header proportions at every mobile width', () => {
        expect(appCss).toMatch(
            /--mobile-root-header:\s*clamp\(40px,\s*16\.476vw,\s*72px\);[^}]*--mobile-top-action:\s*clamp\(30px,\s*12\.18vw,\s*53px\);[^}]*--mobile-top-inset:\s*clamp\(9px,\s*3\.89vw,\s*17px\);[^}]*--mobile-top-offset:\s*clamp\(10px,\s*4\.06vw,\s*18px\);[^}]*--mobile-root-title-inset:\s*clamp\(14px,\s*6\.095vw,\s*26px\);/s,
        );
        expect(appCss).toMatch(/--mobile-pill-control:\s*clamp\(26px,\s*10\.526vw,\s*46px\);/s);
        expect(appCss).toMatch(
            /\.mobile-top-action\s*\{[^}]*width:\s*var\(--mobile-top-action\);[^}]*height:\s*var\(--mobile-top-action\);[^}]*min-width:\s*var\(--mobile-top-action\);[^}]*min-height:\s*var\(--mobile-top-action\);/s,
        );
        expect(appCss).toMatch(/--detail-ui-type:\s*clamp\(10px,\s*4\.119vw,\s*18px\);/s);
        expect(appCss).toMatch(/--detail-support-type:\s*clamp\(9px,\s*3\.55vw,\s*16px\);/s);
        expect(appCss).toMatch(
            /\.app-shell\[data-layout='mobile'\]\s*\{[^}]*--reading:\s*591px;[^}]*--settings:\s*591px;/s,
        );
        for (const source of [libraryPaneSource, conversationPaneSource]) {
            expect(source).toMatch(
                /\.mobile-root-row\s*\{[^}]*min-height:\s*clamp\(46px,\s*19\.222vw,\s*84px\);[^}]*padding:\s*clamp\(4px,\s*1\.831vw,\s*8px\) clamp\(10px,\s*4\.119vw,\s*18px\);/s,
            );
            expect(source).toMatch(
                /\.mobile-root-row[\s\S]*?\.avatar\s*\{[^}]*width:\s*clamp\(35px,\s*14\.645vw,\s*64px\);[^}]*height:\s*clamp\(35px,\s*14\.645vw,\s*64px\);/s,
            );
        }
        expect(appCss).toMatch(
            /\.app-shell\[data-layout='mobile'\] \.setting-row\s*\{[^}]*min-height:\s*clamp\(37px,\s*15\.561vw,\s*68px\);/s,
        );
        expect(appCss).toMatch(
            /\.app-shell\[data-layout='mobile'\]\[data-view='chat'\] \.composer-field\s*\{[^}]*--composer-control-size:\s*clamp\(28px,\s*11\.35vw,\s*42px\);[^}]*--composer-button-size:\s*var\(--composer-control-size\);[^}]*--composer-collapsed-size:\s*clamp\(44px,\s*17\.98vw,\s*66px\);[^}]*--composer-corner:\s*calc\(var\(--composer-collapsed-size\) \/ 2\);[^}]*min-height:\s*var\(--composer-collapsed-size\);[^}]*background:\s*var\(--surface-raised\);[^}]*box-shadow:\s*var\(--shadow-2\);/s,
        );
        expect(appCss).toMatch(
            /\.app-shell\[data-layout='mobile'\]\[data-view='chat'\] \.composer\s*\{[^}]*--composer-dock-top-inset:\s*clamp\(3px,\s*1\.373vw,\s*6px\);[^}]*padding:\s*var\(--composer-dock-top-inset\)\s*var\(--chat-side-inset\)\s*calc\([^}]*env\(safe-area-inset-bottom\)\);[^}]*background:\s*none;[^}]*box-shadow:\s*none;/s,
        );
        expect(appCss).toMatch(
            /\.composer\s*\{[^}]*--composer-dock-top-inset:\s*8px;[^}]*position:\s*absolute;[^}]*z-index:\s*6;[^}]*right:\s*0;[^}]*bottom:\s*0;[^}]*left:\s*0;[^}]*background:\s*none;[^}]*box-shadow:\s*none;[^}]*pointer-events:\s*none;/s,
        );
        expect(appCss).not.toContain('.composer::before');
        expect(appCss).not.toContain('.composer::after');
        expect(appCss).not.toContain('--composer-dock-fade-size');
        expect(appCss).toMatch(
            /\.app-shell\[data-layout='mobile'\]\[data-view='chat'\] \.composer-text-region textarea\s*\{[^}]*font-size:\s*clamp\(10px,\s*4\.36vw,\s*16px\);/s,
        );
        expect(appCss).toMatch(
            /\.app-shell\[data-layout='mobile'\]\[data-view='chat'\][\s\S]*?\.composer-field:is\(\.expanded,\s*\.measuring\)[\s\S]*?\.composer-text-region[\s\S]*?textarea\s*\{[^}]*padding:\s*clamp\(5px,\s*2\.288vw,\s*10px\) clamp\(5px,\s*2\.18vw,\s*10px\) 0;/s,
        );
        expect(appCss).toMatch(
            /\.composer-field\s*\{[^}]*--composer-collapsed-size:\s*54px;[^}]*--composer-text-action-gap:\s*6px;[^}]*--composer-expanded-min:\s*clamp\(82px,\s*11dvh,\s*96px\);[^}]*--composer-motion-duration:\s*360ms;[^}]*display:\s*block;[^}]*height:\s*var\(--composer-collapsed-size\);[^}]*overflow:\s*hidden;[^}]*border:\s*0;[^}]*background:\s*var\(--surface-raised\);[^}]*box-shadow:\s*var\(--shadow-2\);[^}]*pointer-events:\s*auto;[^}]*transition:[^}]*height var\(--composer-motion-duration\) var\(--composer-motion-easing\),[^}]*border-radius var\(--composer-motion-duration\) var\(--composer-motion-easing\);[^}]*will-change:\s*height,\s*border-radius;/s,
        );
        expect(appCss).toMatch(
            /\.composer-field\.expanded\s*\{[^}]*height:\s*clamp\([^}]*var\(--composer-text-size\)[^}]*var\(--composer-text-action-gap\)[^}]*var\(--composer-max-size\)[^}]*border-radius:\s*var\(--composer-expanded-corner\);/s,
        );
        expect(appCss).not.toMatch(/\.composer-field:focus-within\s*\{[^}]*height:/s);
        expect(appCss).toMatch(
            /\.composer-text-region\s*\{[^}]*position:\s*absolute;[^}]*top:\s*var\(--composer-field-inset\);[^}]*bottom:\s*var\(--composer-field-inset\);[^}]*height:\s*auto;[^}]*max-height:\s*calc\([^}]*var\(--composer-text-action-gap\)[^}]*transition:[^}]*right var\(--composer-motion-duration\)[^}]*left var\(--composer-motion-duration\)[^}]*;/s,
        );
        expect(appCss).not.toMatch(
            /\.composer-text-region\s*\{[^}]*transition:[^}]*bottom var\(--composer-motion-duration\)/s,
        );
        expect(appCss).not.toMatch(/\.composer-text-region\s*\{[^}]*will-change:/s);
        expect(appCss).toMatch(
            /\.composer-field:is\(\.expanded,\s*\.measuring\) \.composer-text-region\s*\{[^}]*bottom:\s*calc\([^}]*var\(--composer-text-action-gap\)[^}]*left:\s*var\(--composer-field-inset\);/s,
        );
        expect(appCss).toMatch(
            /\.composer-field\.measuring \.composer-text-region\s*\{[^}]*transition:\s*none;/s,
        );
        expect(appCss).not.toMatch(
            /\.composer-text-region\s*\{[^}]*height var\(--composer-motion-duration\)/s,
        );
        expect(appCss).toMatch(
            /\.composer-text-region textarea\s*\{[^}]*height:\s*100%;[^}]*min-height:\s*0;[^}]*overflow:\s*hidden;[^}]*white-space:\s*nowrap;[^}]*transition:\s*none;/s,
        );
        expect(appCss).toMatch(
            /\.composer-field:is\(\.expanded,\s*\.measuring\) \.composer-text-region textarea\s*\{[^}]*overflow-y:\s*hidden;[^}]*white-space:\s*pre-wrap;/s,
        );
        expect(appCss).toMatch(
            /\.composer-field\.overflows \.composer-text-region textarea\s*\{[^}]*overflow-y:\s*auto;/s,
        );
        expect(appCss).not.toMatch(/field-sizing:\s*content;/);
        expect(appCss).toMatch(
            /\.composer-action-row\s*\{[^}]*display:\s*flex;[^}]*position:\s*absolute;[^}]*bottom:\s*var\(--composer-action-bottom\);[^}]*height:\s*var\(--composer-control-size\);[^}]*pointer-events:\s*none;[^}]*transition:\s*none;/s,
        );
        expect(appCss).not.toMatch(/\.composer-field\.expanded \.composer-action-row\s*\{/s);
        expect(appCss).toMatch(
            /\.composer-field\.has-draft:not\(\.expanded\) \.composer-text-region\s*\{[^}]*right:\s*calc\([^}]*var\(--composer-button-size\)[^}]*var\(--composer-inline-action-gap\)[^}]*\);/s,
        );
        expect(appCss).toMatch(
            /\.composer-field\.has-draft\.can-fullscreen:not\(\.expanded\) \.composer-text-region\s*\{[^}]*right:\s*calc\([^}]*var\(--composer-button-size\)[^}]*var\(--composer-action-gap\)[^}]*var\(--composer-inline-action-gap\)[^}]*\);/s,
        );
        expect(appCss).toMatch(
            /\.composer-action-row > button:not\(\.composer-expand-action\)\s*\{[^}]*pointer-events:\s*auto;/s,
        );
        expect(appCss).toMatch(
            /\.composer-leading-action\s*\{[^}]*width:\s*var\(--composer-button-size\);[^}]*border:\s*0;[^}]*background:\s*transparent;[^}]*place-items:\s*center;/s,
        );
        expect(appCss).toMatch(
            /\.composer-expand-action\s*\{[^}]*height:\s*var\(--composer-button-size\);[^}]*border:\s*0;[^}]*background:\s*transparent;[^}]*opacity:\s*0;/s,
        );
        expect(appCss).toMatch(
            /\.app-shell\[data-layout='mobile'\]\[data-view='chat'\] \.composer-action-row\s*\{[^}]*gap:\s*var\(--composer-action-gap\);/s,
        );
        expect(appCss).toMatch(
            /\.app-shell\[data-layout='mobile'\]\[data-view='chat'\] \.composer-field\s*\{[^}]*--composer-field-inset:\s*clamp\(5px,\s*2\.18vw,\s*8px\);[^}]*--composer-inline-action-gap:\s*clamp\(10px,\s*4\.36vw,\s*16px\);[^}]*--composer-text-action-gap:\s*clamp\(7px,\s*2\.99vw,\s*11px\);[^}]*--composer-action-gap:\s*clamp\(4px,\s*1\.63vw,\s*6px\);[^}]*--composer-expanded-min:\s*clamp\(68px,\s*27\.7vw,\s*102px\);[^}]*--composer-expanded-corner:\s*clamp\(14px,\s*5\.72vw,\s*22px\);[^}]*background:\s*var\(--surface-raised\);[^}]*box-shadow:\s*var\(--shadow-2\);/s,
        );
        expect(appCss).toMatch(
            /\.composer-expand-action\s*\{[^}]*width:\s*0;[^}]*height:\s*var\(--composer-button-size\);[^}]*min-width:\s*0;[^}]*min-height:\s*var\(--composer-button-size\);[^}]*opacity:\s*0;[^}]*transform:\s*scale\(0\.82\);[^}]*transition:[^}]*width var\(--composer-motion-duration\)[^}]*opacity calc\(var\(--composer-motion-duration\) \* 0\.72\)[^}]*transform var\(--composer-motion-duration\)/s,
        );
        expect(appCss).toMatch(
            /\.composer-expand-action\.available\s*\{[^}]*width:\s*var\(--composer-button-size\);[^}]*min-width:\s*var\(--composer-button-size\);[^}]*opacity:\s*1;[^}]*pointer-events:\s*auto;[^}]*transform:\s*scale\(1\);/s,
        );
        expect(appCss).toMatch(
            /\.composer-expand-action:not\(\.available\)\s*\{[^}]*opacity:\s*0;[^}]*pointer-events:\s*none;/s,
        );
        expect(appCss).toContain(
            ':is(.composer-leading-action, .composer-expand-action.available, .send-button)',
        );
        expect(appCss).toMatch(
            /\.app-shell\[data-layout='mobile'\]\[data-view='chat'\][\s\S]*?:is\(\.composer-leading-action,\s*\.composer-expand-action\)[\s\S]*?svg\s*\{[^}]*width:\s*clamp\(16px,\s*6\.865vw,\s*24px\);[^}]*height:\s*clamp\(16px,\s*6\.865vw,\s*24px\);/s,
        );
        expect(appCss).toMatch(
            /\.app-shell\[data-layout='mobile'\]\[data-view='chat'\] \.send-button svg\s*\{[^}]*width:\s*clamp\(14px,\s*5\.95vw,\s*21px\);[^}]*height:\s*clamp\(14px,\s*5\.95vw,\s*21px\);/s,
        );
        expect(appCss).toMatch(
            /\.app-shell\[data-layout='mobile'\]\[data-view='chat'\] \.composer-field\s*\{[^}]*--composer-button-size:\s*var\(--composer-control-size\);[^}]*--composer-collapsed-size:\s*clamp\(44px,\s*17\.98vw,\s*66px\);/s,
        );
        expect(appCss).toMatch(
            /\.composer-fullscreen\s*\{[^}]*--composer-origin-top:\s*calc\([^}]*var\(--composer-field-height\)[^}]*var\(--composer-field-bottom-inset\)[^}]*\);[^}]*--composer-origin-right:\s*var\(--chat-side-inset\);[^}]*--composer-origin-bottom:\s*var\(--composer-field-bottom-inset\);[^}]*--composer-origin-left:\s*var\(--chat-side-inset\);[^}]*--composer-origin-radius:\s*calc\(var\(--composer-field-height\) \/ 2\);[^}]*position:\s*absolute;[^}]*bottom:\s*0;[^}]*background:\s*var\(--surface-raised\);[^}]*filter:\s*var\(--composer-morph-filter\);[^}]*overflow:\s*hidden;[^}]*pointer-events:\s*auto;[^}]*clip-path:\s*inset\([^}]*var\(--composer-origin-top\)[^}]*var\(--composer-origin-right\)[^}]*var\(--composer-origin-bottom\)[^}]*var\(--composer-origin-left\)[^}]*round var\(--composer-origin-radius\)[^}]*\);[^}]*visibility:\s*hidden;/s,
        );
        expect(appCss).toMatch(
            /\.composer-fullscreen\s*\{[^}]*transition:[^}]*clip-path var\(--composer-fullscreen-close-duration\)[^}]*var\(--composer-fullscreen-close-easing\),[^}]*visibility 0s linear var\(--composer-fullscreen-close-duration\);/s,
        );
        expect(appCss).toMatch(
            /\.composer-fullscreen\.open\s*\{[^}]*clip-path:\s*inset\(0 round var\(--radius-lg\) var\(--radius-lg\) 0 0\);[^}]*pointer-events:\s*auto;[^}]*visibility:\s*visible;[^}]*transition:[^}]*clip-path var\(--composer-fullscreen-open-duration\)[^}]*var\(--composer-fullscreen-open-easing\),[^}]*visibility 0s;/s,
        );
        expect(chatPaneSource).toContain('target?.focus({ preventScroll: true });');
        expect(appCss.match(/--composer-morph-filter:/g)).toHaveLength(3);
        expect(appCss).not.toContain(".composer[aria-hidden='true'] .composer-field");
        expect(appCss).not.toMatch(/\.composer-fullscreen\s*\{[^}]*will-change:/s);
        expect(appCss).not.toContain('.composer-fullscreen::before');
        expect(appCss).not.toContain(
            '.composer-fullscreen :is(.composer-fullscreen-header, textarea)',
        );
        expect(appCss).toMatch(
            /\.composer-fullscreen-header\s*\{[^}]*--composer-control-size:\s*44px;[^}]*--composer-button-size:\s*var\(--composer-control-size\);/s,
        );
        expect(appCss).toMatch(
            /\.composer-fullscreen-close\s*\{[^}]*width:\s*var\(--composer-control-size,\s*44px\);[^}]*border:\s*0;[^}]*background:\s*transparent;[^}]*place-items:\s*center;/s,
        );
        expect(appCss).toMatch(
            /\.composer-fullscreen-close svg\s*\{[^}]*width:\s*24px;[^}]*height:\s*24px;/s,
        );
        expect(appCss).toMatch(
            /\.composer-fullscreen-close\s*\{[^}]*width:\s*var\(--composer-control-size,\s*44px\);[^}]*height:\s*var\(--composer-control-size,\s*44px\);/s,
        );
        expect(appCss).toMatch(
            /\.composer-fullscreen-header\s+:is\(\.composer-fullscreen-close,\s*\.send-button\)\s*\{[^}]*transform:\s*translate3d\([^}]*var\(--composer-control-origin-x,\s*0px\)[^}]*var\(--composer-control-origin-y,\s*0px\)[^}]*\);[^}]*transition:\s*transform var\(--composer-fullscreen-close-duration\)[^}]*var\(--composer-fullscreen-close-easing\);[^}]*will-change:\s*transform;/s,
        );
        expect(appCss).toMatch(
            /\.composer-fullscreen\.open\s+\.composer-fullscreen-header\s+:is\(\.composer-fullscreen-close,\s*\.send-button\)\s*\{[^}]*transform:\s*translate3d\(0,\s*0,\s*0\);[^}]*transition:\s*transform var\(--composer-fullscreen-open-duration\)[^}]*var\(--composer-fullscreen-open-easing\);/s,
        );
        expect(appCss).toMatch(
            /\.composer-fullscreen-text-region\s*\{[^}]*--composer-fullscreen-text-size:\s*1rem;[^}]*--composer-fullscreen-text-line-height:\s*1\.6;[^}]*transform:\s*translate3d\([^}]*var\(--composer-text-origin-x,\s*0px\)[^}]*var\(--composer-text-origin-y,\s*0px\)[^}]*\);[^}]*transform-origin:\s*0 0;[^}]*transition:\s*transform var\(--composer-fullscreen-close-duration\)[^}]*var\(--composer-fullscreen-close-easing\);[^}]*will-change:\s*transform;/s,
        );
        expect(appCss).toMatch(
            /\.composer-fullscreen\.open \.composer-fullscreen-text-region\s*\{[^}]*transform:\s*translate3d\(0,\s*0,\s*0\);[^}]*transition:\s*transform var\(--composer-fullscreen-open-duration\)[^}]*var\(--composer-fullscreen-open-easing\);/s,
        );
        expect(appCss).toMatch(
            /\.composer-fullscreen textarea\s*\{[^}]*display:\s*block;[^}]*height:\s*100%;[^}]*font-size:\s*var\(--composer-text-origin-font-size,\s*var\(--composer-fullscreen-text-size\)\);[^}]*line-height:\s*var\([^}]*--composer-text-origin-line-height,[^}]*var\(--composer-fullscreen-text-line-height\)[^}]*\);[^}]*transition:[^}]*font-size var\(--composer-fullscreen-close-duration\)[^}]*var\(--composer-fullscreen-close-easing\),[^}]*line-height var\(--composer-fullscreen-close-duration\)[^}]*var\(--composer-fullscreen-close-easing\);/s,
        );
        expect(appCss).toMatch(
            /\.composer-fullscreen\.open textarea\s*\{[^}]*font-size:\s*var\(--composer-fullscreen-text-size\);[^}]*line-height:\s*var\(--composer-fullscreen-text-line-height\);[^}]*transition:[^}]*font-size var\(--composer-fullscreen-open-duration\)[^}]*var\(--composer-fullscreen-open-easing\),[^}]*line-height var\(--composer-fullscreen-open-duration\)[^}]*var\(--composer-fullscreen-open-easing\);/s,
        );
        expect(appCss).toMatch(
            /\.app-shell\[data-layout='mobile'\]\[data-view='chat'\] \.composer-fullscreen textarea\s*\{[^}]*--composer-fullscreen-text-size:\s*var\(--detail-ui-type\);/s,
        );
        expect(appCss).toMatch(
            /\.app-shell\[data-layout='mobile'\]\[data-view='chat'\] \.composer-fullscreen-header\s*\{[^}]*--composer-control-size:\s*clamp\(28px,\s*11\.35vw,\s*42px\);[^}]*--composer-button-size:\s*var\(--composer-control-size\);/s,
        );
        expect(appCss).not.toContain('opacity 220ms ease 110ms');
        expect(appCss).not.toContain('transform 360ms cubic-bezier(0.22, 1, 0.36, 1) 80ms');
        expect(appCss).not.toContain('opacity 120ms ease 240ms');
        expect(chatPaneSource).toContain('<Plus aria-hidden="true" />');
        expect(chatPaneSource).toContain('<Maximize2 aria-hidden="true" />');
        expect(chatPaneSource).toContain('<Minimize2 aria-hidden="true" />');
        expect(chatPaneSource).toContain('<ArrowUp class="chat-send-icon" aria-hidden="true" />');
        expect(chatPaneSource).toContain('class="composer-text-region"');
        expect(chatPaneSource).toContain('class="composer-action-row"');
        expect(chatPaneSource).toContain('class:can-fullscreen={composerCanFullscreen}');
        expect(chatPaneSource).toContain('class:expanded={composerExpanded}');
        expect(chatPaneSource).toContain('class:available={composerCanFullscreen}');
        expect(chatPaneSource).toContain('aria-hidden={!composerCanFullscreen}');
        expect(chatPaneSource).toContain('class:overflows={composerOverflows}');
        expect(chatPaneSource).toContain('class="composer-fullscreen"');
        expect(chatPaneSource).toContain('syncFullscreenControlOrigins();');
        expect(chatPaneSource).toContain('syncFullscreenTextOrigin();');
        expect(chatPaneSource).toContain("'--composer-control-origin-x'");
        expect(chatPaneSource).toContain("'--composer-control-origin-y'");
        expect(chatPaneSource).toContain("'--composer-text-origin-x'");
        expect(chatPaneSource).toContain("'--composer-text-origin-y'");
        expect(chatPaneSource).toContain('bind:this={composerLeadingAction}');
        expect(chatPaneSource).toContain('bind:this={composerSendButton}');
        expect(chatPaneSource).toContain('bind:this={composerField}');
        expect(chatPaneSource).toContain('bind:this={fullscreenComposerSurface}');
        expect(chatPaneSource).toContain('bind:this={fullscreenCloseButton}');
        expect(chatPaneSource).toContain('bind:this={fullscreenSendButton}');
        expect(chatPaneSource).toContain('bind:this={fullscreenTextRegion}');
        expect(appCss).toMatch(
            /:root\s*\{[^}]*--panel-open-duration:\s*420ms;[^}]*--panel-close-duration:\s*360ms;[^}]*--panel-open-easing:\s*cubic-bezier\(0\.22,\s*1,\s*0\.36,\s*1\);[^}]*--panel-close-easing:\s*cubic-bezier\(0\.65,\s*0,\s*0\.35,\s*1\);/s,
        );
        expect(appCss).toMatch(
            /\.chat-pane\s*\{[^}]*--chat-side-inset:\s*var\(--gutter\);[^}]*--composer-overlay-height:\s*78px;[^}]*--composer-field-height:\s*54px;[^}]*--composer-field-bottom-inset:\s*8px;[^}]*--composer-fullscreen-open-duration:\s*var\(--panel-open-duration\);[^}]*--composer-fullscreen-close-duration:\s*var\(--panel-close-duration\);[^}]*--composer-fullscreen-open-easing:\s*var\(--panel-open-easing\);[^}]*--composer-fullscreen-close-easing:\s*var\(--panel-close-easing\);/s,
        );
        expect(orchestrationQuickDrawerSource).toMatch(
            /\.quick-drawer\s*\{[^}]*transition:[^}]*transform var\(--panel-close-duration\) var\(--panel-close-easing\),[^}]*visibility 0s linear var\(--panel-close-duration\);/s,
        );
        expect(orchestrationQuickDrawerSource).toMatch(
            /\.quick-drawer\.open\s*\{[^}]*transition:[^}]*transform var\(--panel-open-duration\) var\(--panel-open-easing\),[^}]*visibility 0s;/s,
        );
        expect(appCss).toMatch(
            /\.app-shell\[data-layout='mobile'\]\[data-view='chat'\] \.chat-pane\s*\{[^}]*--chat-side-inset:\s*max\([^}]*var\(--reading\)[^}]*\);/s,
        );
        expect(appCss).toMatch(
            /\.app-shell\[data-layout='mobile'\]\[data-view='chat'\] \.message-scroll\s*\{[^}]*padding-inline:\s*var\(--chat-side-inset\);[^}]*background:\s*transparent;/s,
        );
        expect(appCss).toMatch(
            /\.app-shell\[data-layout='mobile'\]\[data-view='chat'\] \.chat-pane::after\s*\{[^}]*z-index:\s*5;[^}]*height:\s*calc\(var\(--composer-overlay-height\) \+ clamp\(12px,\s*4\.577vw,\s*20px\)\);[^}]*background:\s*linear-gradient\([^}]*var\(--bg\)[^}]*\);[^}]*pointer-events:\s*none;/s,
        );
        expect(appCss).not.toMatch(
            /\.app-shell\[data-layout='mobile'\]\[data-view='chat'\] \.message-scroll\s*\{[^}]*(?:-webkit-)?mask-image:/s,
        );
        expect(chatPaneSource).toContain("chatPane.style.setProperty('--composer-overlay-height'");
        expect(chatPaneSource).toContain("'--composer-field-height'");
        expect(chatPaneSource).toContain("'--composer-field-bottom-inset'");
        expect(chatPaneSource).toContain('observer.observe(field)');
        const composerMaximumRead = chatPaneSource.indexOf(
            'textRegion ? getComputedStyle(textRegion).maxHeight',
        );
        const composerHeightWrite = chatPaneSource.indexOf(
            "field?.style.setProperty('--composer-text-size'",
        );
        expect(composerMaximumRead).toBeGreaterThan(-1);
        expect(composerMaximumRead).toBeLessThan(composerHeightWrite);
        const composerFinalMeasurement = chatPaneSource.indexOf('update(true);');
        const composerExpansionStart = chatPaneSource.indexOf(
            'composerExpanded = true;',
            composerFinalMeasurement,
        );
        expect(composerFinalMeasurement).toBeGreaterThan(-1);
        expect(composerExpansionStart).toBeGreaterThan(composerFinalMeasurement);
        expect(chatPaneSource).toMatch(
            /const handleFocusOut[\s\S]*?node\.value\.trim\(\)\.length > 0[\s\S]*?composerExpanded = false;/,
        );
        expect(chatPaneSource).not.toContain('--composer-line-offset');
        expect(chatPaneSource).toContain('aria-hidden={!composerFullscreen}');
        expect(chatPaneSource).toContain('inert={!composerFullscreen}');
        const removedPlaceholder = `placeholder="${String.fromCodePoint(47700, 49884, 51648)}"`;
        expect(chatPaneSource).not.toContain(removedPlaceholder);
        expect(chatPaneSource).not.toContain('message-action-reveal');
        expect(appCss).toMatch(/\.sub-header h1\s*\{[^}]*font-size:\s*var\(--detail-ui-type\);/s);
        expect(appCss).toMatch(
            /\.app-shell\[data-layout='mobile'\] \.sub-header h1\s*\{[^}]*height:\s*var\(--mobile-top-action\);[^}]*display:\s*flex;[^}]*align-self:\s*center;[^}]*align-items:\s*center;[^}]*justify-content:\s*center;[^}]*padding-inline:\s*0;[^}]*margin:\s*0;/s,
        );
        expect(chatPaneSource).toContain('{#snippet roomControls(');
        expect(chatPaneSource).toMatch(/<OrchestrationQuickDrawer[\s\S]*\{roomControls\}/);
        expect(appSource.match(/<ArrowLeft aria-hidden="true" \/>/g)).toHaveLength(2);
        expect(chatPaneSource).toContain('<ArrowLeft class="chat-back-icon" aria-hidden="true" />');
        for (const icon of [
            'ArrowLeft',
            'ChevronRight',
            'Menu',
            'PanelRightClose',
            'PanelRightOpen',
            'X',
        ]) {
            expect(orchestrationQuickDrawerSource).toContain(`${icon},`);
        }
        expect(orchestrationQuickDrawerSource).toContain(
            '<Menu class="orchestration-toggle-icon" aria-hidden="true" />',
        );
        expect(orchestrationQuickDrawerSource).toContain('SlidersHorizontal,');
        expect(orchestrationQuickDrawerSource).not.toContain('EllipsisVertical');
        expect(chatPaneSource).not.toContain('class="chat-toolbar"');
        expect(appCss).not.toContain('.chat-toolbar-new-operation');
        expect(appCss).not.toContain('.chat-pane .chat-toolbar');
        expect(appCss).toMatch(
            /\.app-shell\[data-layout='mobile'\]\[data-view='chat'\] \.chat-pane \.chat-header\s*\{[^}]*margin:\s*0 auto;[^}]*inset:\s*0 0 auto;/s,
        );
        expect(orchestrationQuickDrawerSource).not.toContain('quick-drawer-backdrop');
        expect(orchestrationQuickDrawerSource).toContain('aria-hidden={!open}');
        expect(orchestrationQuickDrawerSource).toContain('inert={!open}');
        expect(orchestrationQuickDrawerSource).toContain('data-view={view}');
        expect(chatPaneSource).toContain('bind:view={utilityView}');
        expect(chatPaneSource).toContain("utilityView = 'tools';");
        expect(orchestrationQuickDrawerSource).not.toContain('in:fly');
        expect(orchestrationQuickDrawerSource).not.toContain('out:fly');
        expect(orchestrationQuickDrawerSource).toContain(
            '<PanelRightOpen class="orchestration-toggle-icon" aria-hidden="true" />',
        );
        expect(orchestrationQuickDrawerSource).toContain(
            '<PanelRightClose class="orchestration-toggle-icon" aria-hidden="true" />',
        );
        expect(choicePopoverSource).toContain('role="combobox"');
        expect(choicePopoverSource).toContain('role="listbox"');
        expect(choicePopoverSource).toContain('role="option"');
        expect(choicePopoverSource).toContain('class="choice-check"');
        expect(choicePopoverSource).toContain('popover="manual"');
        expect(choicePopoverSource).toMatch(
            /\.choice-menu\s*\{[^}]*position:\s*fixed;[^}]*inset:\s*auto;/s,
        );
        expect(choicePopoverSource).toMatch(
            /\.choice-menu::backdrop\s*\{[^}]*background:\s*transparent;/s,
        );
        expect(orchestrationQuickDrawerSource).not.toContain('<footer>');
        expect(orchestrationQuickDrawerSource).not.toContain('type="checkbox"');
        expect(orchestrationQuickDrawerSource).toContain('<ToggleSwitch');
        expect(orchestrationQuickDrawerSource).toContain('checked={roomConfig.memory_enabled}');
        expect(toggleSwitchSource).toContain('role="switch"');
        expect(toggleSwitchSource).toMatch(
            /\.toggle-switch\[aria-checked='true'\] \.toggle-switch-thumb\s*\{[^}]*transform:\s*translate3d\(16px, 0, 0\);/s,
        );
        expect(orchestrationQuickDrawerSource).toContain('onpointerdown={handlePanelPointerDown}');
        expect(orchestrationQuickDrawerSource).toMatch(
            /\.quick-drawer\s*\{[^}]*position:\s*fixed;[^}]*top:\s*0;[^}]*right:\s*0;[^}]*bottom:\s*0;[^}]*grid-template-rows:\s*auto minmax\(0, 1fr\);[^}]*width:\s*min\(100%, 390px\);[^}]*height:\s*100dvh;/s,
        );
        expect(orchestrationQuickDrawerSource).toMatch(
            /\.quick-drawer\.open\s*\{[^}]*transform:\s*translate3d\(0, 0, 0\);[^}]*visibility:\s*visible;/s,
        );
        expect(orchestrationQuickDrawerSource).toContain(
            'transform: translate3d(var(--utility-drag-x, 0px), 0, 0);',
        );
        expect(orchestrationQuickDrawerSource).toContain('will-change: transform;');
        expect(orchestrationQuickDrawerSource).toMatch(
            /\.quick-drawer\.desktop\s*\{[^}]*top:\s*58px;[^}]*right:\s*var\(--chat-utility-edge-inset,\s*12px\);[^}]*width:\s*var\(--chat-utility-width,[^}]*border-radius:\s*18px;[^}]*background:\s*var\(--desktop-panel-bg,\s*var\(--desktop-sidebar-bg\)\);/s,
        );
        expect(orchestrationQuickDrawerSource).toMatch(
            /@container view \(max-width:\s*640px\)[\s\S]*?\.quick-drawer\s*\{[^}]*width:\s*100%;[^}]*height:\s*100dvh;[^}]*max-height:\s*100dvh;/s,
        );
        expect(orchestrationQuickDrawerSource).toMatch(
            /\.drawer-body\s*\{[^}]*overflow-y:\s*auto;/s,
        );
        expect(appCss).toMatch(
            /\.app-shell\[data-layout='mobile'\]\[data-view='chat'\] \.chat-pane::before\s*\{[^}]*position:\s*absolute;[^}]*z-index:\s*10;[^}]*background:\s*linear-gradient\(/s,
        );
        expect(appCss).toMatch(
            /\.app-shell\[data-layout='desktop'\] \.sidebar-rail::after\s*\{[^}]*position:\s*absolute;[^}]*right:\s*0;[^}]*width:\s*1px;[^}]*background:\s*var\(--desktop-divider\);/s,
        );
        expect(appCss).toMatch(
            /\.app-shell\[data-layout='desktop'\] \.sidebar-head\s*\{[^}]*height:\s*46px;[^}]*min-height:\s*46px;[^}]*justify-content:\s*flex-start;[^}]*padding:\s*6px 12px;/s,
        );
        expect(appCss).toMatch(
            /\.app-shell\[data-layout='desktop'\] \.sidebar-view-switcher\s*\{[^}]*width:\s*100%;[^}]*height:\s*28px;[^}]*flex:\s*0 0 28px;/s,
        );
        expect(appCss).toMatch(
            /\.app-shell\[data-layout='desktop'\] \.chat-header\s*\{[^}]*height:\s*46px;[^}]*min-height:\s*46px;[^}]*padding:\s*6px 14px 6px 18px;[^}]*border-bottom:\s*1px solid var\(--desktop-divider\);/s,
        );
        expect(chatPaneSource).toContain(
            "data-conversation-mode={appState.conversation_state?.selected_mode ?? 'chat'}",
        );
        expect(appCss).toMatch(
            /\.app-shell\[data-layout='desktop'\][\s\S]*?\.chat-pane\[data-conversation-mode='chat'\][\s\S]*?\.message-item:not\(\.from-user\)\s+\.message-body\s*\{[^}]*width:\s*fit-content;[^}]*max-width:\s*76%;[^}]*padding:\s*9px 13px;[^}]*border-radius:\s*14px;[^}]*background:\s*var\(--desktop-user-bubble-bg\);/s,
        );
        expect(appCss).toMatch(
            /\.app-shell\[data-layout='desktop'\][\s\S]*?\.chat-pane\[data-conversation-mode='story'\][\s\S]*?\.message-item:not\(\.from-user\)\s+\.message-body\s*\{[^}]*width:\s*100%;[^}]*padding:\s*0;[^}]*background:\s*transparent;[^}]*color:\s*var\(--desktop-ink\);[^}]*font-size:\s*13px;/s,
        );
        expect(appCss).toMatch(
            /\.app-shell\[data-layout='desktop'\]\[data-view='chat'\] \.composer-field\s*\{[^}]*--composer-control-size:\s*28px;[^}]*--composer-collapsed-size:\s*96px;[^}]*--composer-corner:\s*16px;[^}]*--composer-action-gap:\s*6px;[^}]*--composer-expanded-min:\s*96px;[^}]*border:\s*0;[^}]*border-radius:\s*var\(--composer-corner\);[^}]*background:\s*var\(--desktop-composer-bg\);[^}]*backdrop-filter:\s*blur\(12px\);/s,
        );
        expect(appCss).toMatch(
            /\.app-shell\[data-layout='desktop'\]\[data-view='chat'\] \.send-button\s*\{[^}]*width:\s*var\(--composer-button-size\);[^}]*height:\s*var\(--composer-button-size\);[^}]*border-radius:\s*50%;[^}]*margin-right:\s*0;[^}]*transform:\s*none;/s,
        );
        expect(appCss).toMatch(
            /\.app-shell\[data-layout='desktop'\]\[data-view='chat'\] \.composer-text-region textarea\s*\{[^}]*font-family:\s*-apple-system,[^}]*font-size:\s*14px;/s,
        );
        expect(appCss).toMatch(
            /\.app-shell\[data-layout='desktop'\]\[data-view='chat'\] \.composer\s*\{[^}]*padding-right:\s*var\(--chat-side-inset\);[^}]*padding-bottom:\s*18px;/s,
        );
        expect(appCss).toMatch(
            /\.app-shell\[data-layout='desktop'\] \.message-scroll\s*\{[^}]*padding-bottom:\s*calc\(var\(--composer-overlay-height\) \+ 16px\);[^}]*padding-right:\s*max\([^}]*calc\(var\(--chat-side-inset\) - var\(--message-scrollbar-width, 0px\)\)[^}]*scrollbar-gutter:\s*auto;/s,
        );
        expect(appCss).toMatch(
            /\.app-shell\[data-layout='desktop'\]\[data-view='chat'\] \.chat-live-status\s*\{[^}]*position:\s*absolute;[^}]*bottom:\s*calc\(18px \+ var\(--composer-field-height\) \+ 8px\);[^}]*justify-content:\s*center;/s,
        );
        expect(appCss).toMatch(
            /\.app-shell\[data-layout='desktop'\] \.chat-pane\.utility-open\s*\{[^}]*padding-right:\s*var\(--chat-utility-reserved-width\);/s,
        );
        expect(appCss).toContain('--chat-utility-edge-inset: 12px;');
        expect(appCss).toContain('--chat-utility-gap: 12px;');
        expect(chatPaneSource).not.toContain('chat-disclaimer');
        expect(chatPaneSource).not.toMatch(/<label[^>]*for="chat-draft"/);
        expect(chatPaneSource).toMatch(/id="chat-draft"\s+aria-label=/);
        expect(appCss).toMatch(
            /\.app-shell\[data-layout='desktop'\]\[data-view='chat'\] \.chat-pane\.utility-open > \.composer\s*\{[^}]*padding-left:\s*var\(--chat-utility-side-inset\);[^}]*padding-right:\s*var\(--chat-utility-side-inset\);/s,
        );
        expect(appCss).toMatch(
            /\.app-shell\[data-layout='mobile'\]\[data-view='chat'\] \.chat-pane \.message-scroll\s*\{[^}]*padding-top:\s*calc\([^}]*var\(--mobile-top-action\)[^}]*clamp\(10px,/s,
        );
        expect(appCss).not.toMatch(
            /\.app-shell\[data-layout='mobile'\]\[data-view='chat'\] \.chat-pane \.message-scroll\s*\{[^}]*padding-top:\s*calc\([^}]*var\(--mobile-pill-control\)/s,
        );
        expect(appCss).not.toMatch(
            /\.app-shell\[data-layout='mobile'\]\[data-view='chat'\] \.chat-pane \.message-scroll\s*\{[^}]*(?:-webkit-)?mask-image:/s,
        );
        expect(personaPanelSource).toMatch(
            /\.persona-form label\s*\{[^}]*font-size:\s*var\(--detail-support-type\);/s,
        );
        expect(detailActionBarSource).toMatch(
            /\.detail-action-bar :global\(\.detail-action\)\s*\{[^}]*font-size:\s*var\(--detail-support-type\);/s,
        );
        expect(detailActionBarSource).toMatch(
            /\.detail-action-bar :global\(\.detail-action--destructive\)\s*\{[^}]*color:\s*var\(--status-error-fg\);/s,
        );
        expect(detailActionBarSource).toMatch(
            /\.detail-action-bar :global\(\.detail-action--borderless\)\s*\{[^}]*border:\s*0;/s,
        );
        expect(personaPanelSource).toMatch(
            /\.persona-row-name,\s*\.persona-row-description\s*\{[^}]*font-size:\s*var\(--detail-support-type\);[^}]*font-weight:\s*550;[^}]*line-height:\s*1\.35;/s,
        );
        expect(personaPanelSource).toMatch(
            /\.persona-row-description\s*\{[^}]*color:\s*var\(--ink-muted\);[^}]*white-space:\s*normal;[^}]*line-clamp:\s*3;[^}]*-webkit-box-orient:\s*vertical;[^}]*-webkit-line-clamp:\s*3;/s,
        );
        expect(appCss).toMatch(
            /\.mobile-top-frame > \.mobile-top-action\s*\{[^}]*align-self:\s*center;[^}]*margin-top:\s*0;/s,
        );
    });

    it('scales controls to the 437px logical reference without growing on wider hosts', () => {
        expect(appCss).toMatch(/--mobile-search:\s*clamp\(27px,\s*10\.984vw,\s*48px\);/);
        expect(appCss).toMatch(/--mobile-nav:\s*clamp\(37px,\s*15\.103vw,\s*66px\);/);
        expect(appCss).toMatch(
            /\.tab-bar\s*\{[^}]*padding:\s*clamp\(2px,\s*0\.686vw,\s*3px\);[^}]*gap:\s*clamp\(1px,\s*0\.458vw,\s*2px\);/s,
        );
        expect(appCss).toMatch(/\.tab\s*\{[^}]*height:\s*100%;[^}]*min-height:\s*0;/s);
        expect(appCss).toMatch(/\.tab\s*\{[^}]*margin-inline:\s*clamp\(1px,\s*0\.458vw,\s*2px\);/s);
        expect(appCss).toMatch(
            /\.tab > \.nav-icon,\s*\.tab > \.tab-label\s*\{[^}]*transform:\s*translateY\(-2px\);/s,
        );
        expect(appCss).toMatch(
            /\.tab::before\s*\{[^}]*background:\s*transparent;[^}]*inset:\s*clamp\(1px,\s*0\.458vw,\s*2px\) clamp\(2px,\s*0\.686vw,\s*3px\);/s,
        );
        expect(appCss).toMatch(/\.tab:first-child::before\s*\{[^}]*left:\s*0;/s);
        expect(appCss).toMatch(/\.tab:last-child::before\s*\{[^}]*right:\s*0;/s);
        expect(appCss).toMatch(/\.tab\[aria-current='page'\]\s*\{[^}]*color:\s*var\(--accent\);/s);
        expect(appCss).toMatch(
            /\.tab\[aria-current='page'\]::before\s*\{[^}]*background:\s*var\(--accent-soft\);/s,
        );
        expect(appSource).toContain('<House class="nav-icon-home-fill-layer" />');
        expect(appSource).toContain('<House class="nav-icon-home-stroke-layer" />');
        expect(appSource).toContain('class="nav-icon nav-icon-chat"');
        expect(appSource).toContain('class="nav-icon nav-icon-create"');
        expect(appSource).toContain('class="nav-icon nav-icon-settings"');
        expect(appSource).toContain(
            '<CirclePlus class="nav-icon nav-icon-create" aria-hidden="true" />',
        );
        expect(appSource).not.toContain('<svg');
        expect(appCss).toMatch(/\.nav-icon-home-fill-layer\s*\{[^}]*stroke:\s*none;/s);
        expect(appCss).toMatch(/\.nav-icon-home-stroke-layer\s*\{[^}]*fill:\s*none;/s);
        expect(appCss).toMatch(
            /\.nav-icon-home-fill-layer > path:last-child\s*\{[^}]*fill:\s*currentcolor;/s,
        );
        expect(appCss).toMatch(
            /\.nav-icon-home::after\s*\{[^}]*bottom:\s*12\.5%;[^}]*left:\s*37\.5%;[^}]*background:\s*var\(--accent-soft\);/s,
        );
        expect(appCss).toMatch(
            /\.nav-icon-chat > path:first-child\s*\{[^}]*fill:\s*currentcolor;/s,
        );
        expect(appCss).toMatch(
            /\.nav-icon-chat > path:not\(:first-child\)\s*\{[^}]*stroke:\s*var\(--accent-soft\);/s,
        );
        expect(appCss).toMatch(/\.nav-icon-create > circle\s*\{[^}]*fill:\s*currentcolor;/s);
        expect(appCss).toMatch(
            /\.nav-icon-create > path\s*\{[^}]*stroke:\s*var\(--accent-soft\);/s,
        );
        expect(appCss).toMatch(/\.nav-icon-settings > path\s*\{[^}]*fill:\s*currentcolor;/s);
        expect(appCss).toMatch(
            /\.nav-icon-settings > circle\s*\{[^}]*fill:\s*var\(--accent-soft\);/s,
        );
        expect(appCss).toMatch(
            /\.tab:hover:not\(:disabled\)::before\s*\{[^}]*background:\s*var\(--surface-hover\);/s,
        );
        expect(appCss).toMatch(
            /\.tab\[aria-current='page'\]:hover:not\(:disabled\)::before\s*\{[^}]*background:\s*var\(--accent-soft\);/s,
        );
        expect(appCss).toMatch(
            /\.setting-row:hover:not\(:disabled\)\s*\{[^}]*background:\s*var\(--bg\);/s,
        );
        expect(appCss).toMatch(
            /\.tab \.nav-icon\s*\{[^}]*width:\s*clamp\(14px,\s*5\.95vw,\s*26px\);[^}]*height:\s*clamp\(14px,\s*5\.95vw,\s*26px\);/s,
        );
        expect(appCss).toMatch(
            /\.tab-label\s*\{[^}]*font-size:\s*clamp\(8px,\s*3\.204vw,\s*14px\);[^}]*font-weight:\s*700;/s,
        );
        expect(appCss).toMatch(
            /\.mobile-root-header h1\s*\{[^}]*grid-column:\s*1;[^}]*align-self:\s*center;[^}]*padding-left:\s*calc\(var\(--mobile-root-title-inset\) - var\(--mobile-top-inset\)\);/s,
        );
        expect(appCss).toMatch(
            /\.app-shell\[data-layout='mobile'\] \.mobile-root-search input\s*\{[^}]*font-size:\s*clamp\(8px,\s*3\.432vw,\s*15px\);/s,
        );
    });
});
