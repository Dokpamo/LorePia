import { describe, expect, it } from 'vitest';

import appSource from '../app/App.svelte?raw';
import choicePopoverSource from '../components/ChoicePopover.svelte?raw';
import detailActionBarSource from '../components/detail/DetailActionBar.svelte?raw';
import detailPageSource from '../components/detail/DetailPage.svelte?raw';
import chatPaneSource from '../features/chat/ChatPane.svelte?raw';
import conversationPaneSource from '../features/conversations/ConversationPane.svelte?raw';
import libraryPaneSource from '../features/library/LibraryPane.svelte?raw';
import orchestrationQuickDrawerSource from '../features/orchestration/OrchestrationQuickDrawer.svelte?raw';
import orchestrationStudioSource from '../features/orchestration/OrchestrationStudio.svelte?raw';
import personaPanelSource from '../features/personas/PersonaPanel.svelte?raw';
import discoveryPanelSource from '../features/providers/DiscoveryPanel.svelte?raw';
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
        expect(appCss.match(/--surface-sunken:\s*#121212;/g)).toHaveLength(2);
        expect(appCss.match(/--surface-raised:\s*#1d1d1d;/g)).toHaveLength(2);
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
        expect(appSource).toContain('lorepia-logo-mark.png');
        expect(appSource).toContain('class="brand-logo-mark"');
        expect(appSource).not.toContain('lorepia-logo-light.png');
        expect(appSource).not.toContain('lorepia-logo-dark.png');
        expect(appCss).toContain('-webkit-mask-image: var(--logo-mask);');
        expect(appCss).toContain('mask-image: var(--logo-mask);');
        expect(themeSource).toContain("const DEFAULT_THEME_PREFERENCE: ThemePreference = 'light';");
        expect(providerSettingsSource).toMatch(
            /\.settings-avatar-wrap\s*\{[^}]*background:\s*var\(--brand-logo-bg\);[^}]*box-shadow:\s*var\(--shadow-2\);/s,
        );
        expect(providerSettingsSource).toMatch(
            /\.settings-avatar\s*\{[^}]*position:\s*absolute;[^}]*border-radius:\s*50%;/s,
        );
        expect(providerSettingsSource).not.toContain('data-tone=');
        expect(appCss).not.toContain('.setting-tile[data-tone=');
        expect(appCss).toMatch(
            /\.setting-icon\s*\{[^}]*color:\s*var\(--ink\);[^}]*place-items:\s*center;/s,
        );
        expect(appCss).toMatch(
            /\.segmented button\.active\s*\{[^}]*background:\s*var\(--accent-soft\);[^}]*color:\s*var\(--accent\);/s,
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
            /\.app-shell\[data-layout='mobile'\] \.sub-header\s*\{[^}]*position:\s*absolute;[^}]*z-index:\s*10;[^}]*border:\s*0;[^}]*background:\s*transparent;[^}]*inset:\s*0 0 auto;[^}]*pointer-events:\s*none;/s,
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
            /\.app-shell\[data-layout='mobile'\] \.sub-header h1\s*\{[^}]*height:\s*var\(--mobile-top-action\);[^}]*display:\s*flex;[^}]*align-self:\s*center;[^}]*border-radius:\s*var\(--radius-pill\);[^}]*margin-top:\s*0;[^}]*background:\s*color-mix\(in srgb,\s*var\(--surface-raised\) 94%,\s*transparent\);[^}]*box-shadow:\s*var\(--shadow-1\);/s,
        );
        expect(appCss).toMatch(
            /\.app-shell\[data-layout='desktop'\] \.sub-header\s*\{[^}]*position:\s*relative;[^}]*border-bottom:\s*1px solid color-mix\(in srgb,\s*var\(--line\) 72%,\s*transparent\);[^}]*background:\s*var\(--bg\);/s,
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

    it('hides the mobile tab bar for every pushed settings screen', () => {
        expect(appSource).toMatch(
            /\{#if\s+!isDesktop\s+&&\s+studioSection === null\s+&&\s+!\(view === 'chat' && chatThreadOpen\)\s+&&\s+!\(view === 'settings' && settingsSection !== null\)\s*\}\s*<nav class="tab-bar"/s,
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
            /\.detail-page-scroll\s*\{[^}]*display:\s*grid;[^}]*height:\s*0;[^}]*min-height:\s*0;[^}]*flex:\s*1 1 0;[^}]*align-content:\s*start;[^}]*padding:\s*16px var\(--settings-gutter\)\s*calc\(24px \+ env\(safe-area-inset-bottom\)\);[^}]*overflow-y:\s*scroll;/s,
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
            /\.detail-action-bar :global\(\.detail-action--destructive\)\s*\{[^}]*color:\s*#ff0000;/s,
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
        expect(providerSettingsSource).toMatch(
            /\.detail-form :is\(input, select\):hover:not\(:focus, :disabled\)\s*\{[^}]*border-color:\s*var\(--line\);/s,
        );
        expect(providerSettingsSource).toMatch(
            /\.detail-form :is\(input, select\):focus\s*\{[^}]*border-color:\s*var\(--accent\);[^}]*outline:\s*0;/s,
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
            /\.provider-scroll\s*\{[^}]*height:\s*0;[^}]*min-height:\s*0;[^}]*flex:\s*1 1 0;[^}]*overflow-y:\s*scroll;/s,
        );
        expect(detailPageSource).toMatch(
            /\.detail-page-scroll\s*\{[^}]*height:\s*0;[^}]*min-height:\s*0;[^}]*flex:\s*1 1 0;[^}]*overflow-y:\s*scroll;/s,
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
        expect(appCss).not.toContain('scrollbar-width: thin');
        expect(appCss).not.toContain('::-webkit-scrollbar-thumb');
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
        expect(libraryPaneSource).toMatch(
            /\.library-pane\.root-view \.entity-list\s*\{[^}]*padding:\s*0 8px calc\(/s,
        );
        expect(conversationPaneSource).toMatch(
            /\.conversation-pane\.root-view \.entity-list\s*\{[^}]*padding:\s*8px 8px calc\(/s,
        );
        expect(appCss).toMatch(/\.message-scroll\s*\{[^}]*padding:\s*0 var\(--gutter\);/s);
        expect(providerSettingsSource).toMatch(
            /:global\(\.app-shell\[data-layout='mobile'\]\) \.provider-scroll\.settings-home-scroll\s*\{[^}]*display:\s*flex;[^}]*flex-direction:\s*column;[^}]*padding-bottom:/s,
        );
        expect(providerSettingsSource).toMatch(
            /:global\(\.app-shell\[data-layout='mobile'\]\) \.settings-home-scroll \.setting-list\s*\{[^}]*flex:\s*none;[^}]*margin-bottom:/s,
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
            /\.app-shell\[data-layout='mobile'\]\[data-view='chat'\] \.composer-field\s*\{[^}]*min-height:\s*var\(--mobile-top-action\);/s,
        );
        expect(appCss).toMatch(
            /\.composer-field\s*\{[^}]*--composer-expanded-size:\s*clamp\(96px,\s*18dvh,\s*140px\);[^}]*border-radius:\s*var\(--composer-corner\);[^}]*transition:[^}]*min-height 260ms/s,
        );
        expect(appCss).toMatch(
            /\.composer-field:focus-within\s*\{[^}]*min-height:\s*var\(--composer-expanded-size\);[^}]*border-radius:\s*var\(--radius-lg\);/s,
        );
        expect(appCss).toMatch(
            /\.composer-field:focus-within textarea\s*\{[^}]*min-height:\s*calc\(var\(--composer-expanded-size\) - 10px\);[^}]*padding-block:\s*12px;/s,
        );
        expect(appCss).toMatch(
            /\.app-shell\[data-layout='mobile'\]\[data-view='chat'\] \.composer-field:focus-within\s*\{[^}]*min-height:\s*var\(--composer-expanded-size\);/s,
        );
        expect(appCss).toMatch(
            /\.app-shell\[data-layout='mobile'\]\[data-view='chat'\] \.composer-field:focus-within textarea\s*\{[^}]*min-height:\s*calc\(var\(--composer-expanded-size\) - 10px\);[^}]*padding-block:\s*12px;/s,
        );
        expect(chatPaneSource).toContain('<Plus aria-hidden="true" />');
        expect(chatPaneSource).toContain('<ArrowUp class="chat-send-icon" aria-hidden="true" />');
        const removedPlaceholder = `placeholder="${String.fromCodePoint(47700, 49884, 51648)}"`;
        expect(chatPaneSource).not.toContain(removedPlaceholder);
        expect(chatPaneSource).not.toContain('message-action-reveal');
        expect(appCss).toMatch(/\.sub-header h1\s*\{[^}]*font-size:\s*var\(--detail-ui-type\);/s);
        expect(appCss).toMatch(
            /\.app-shell\[data-layout='mobile'\] \.sub-header h1\s*\{[^}]*height:\s*var\(--mobile-top-action\);[^}]*display:\s*flex;[^}]*align-self:\s*center;[^}]*align-items:\s*center;[^}]*justify-content:\s*center;[^}]*border-radius:\s*var\(--radius-pill\);[^}]*margin-top:\s*0;/s,
        );
        expect(chatPaneSource).toContain('{#snippet roomControls(');
        expect(chatPaneSource).toMatch(/<OrchestrationQuickDrawer[\s\S]*\{roomControls\}/);
        expect(chatPaneSource).not.toContain('class="chat-toolbar"');
        expect(appCss).not.toContain('.chat-toolbar-new-operation');
        expect(appCss).not.toContain('.chat-pane .chat-toolbar');
        expect(appCss).toMatch(
            /\.app-shell\[data-layout='mobile'\]\[data-view='chat'\] \.chat-pane \.chat-header\s*\{[^}]*margin:\s*0 auto;[^}]*inset:\s*0 0 auto;/s,
        );
        expect(orchestrationQuickDrawerSource).toContain('class="quick-drawer-backdrop"');
        expect(orchestrationQuickDrawerSource).toContain('aria-hidden={!open}');
        expect(orchestrationQuickDrawerSource).toContain('inert={!open}');
        expect(orchestrationQuickDrawerSource).not.toContain('in:fly');
        expect(orchestrationQuickDrawerSource).not.toContain('out:fly');
        expect(orchestrationQuickDrawerSource).toMatch(
            /\.quick-drawer-backdrop\s*\{[^}]*opacity:\s*0;[^}]*visibility:\s*hidden;/s,
        );
        expect(orchestrationQuickDrawerSource).toMatch(
            /\.quick-drawer-backdrop\.open\s*\{[^}]*opacity:\s*1;[^}]*visibility:\s*visible;/s,
        );
        expect(choicePopoverSource).toContain('role="menu"');
        expect(choicePopoverSource).toContain('role="menuitemradio"');
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
        expect(orchestrationQuickDrawerSource).toContain('class="switch-button"');
        expect(orchestrationQuickDrawerSource).toContain('role="switch"');
        expect(orchestrationQuickDrawerSource).toContain(
            'aria-checked={roomConfig.memory_enabled}',
        );
        expect(orchestrationQuickDrawerSource).toContain(
            'background-color: rgba(0, 0, 0, 0.14) !important;',
        );
        expect(orchestrationQuickDrawerSource).toContain('backdrop-filter: none !important;');
        expect(orchestrationQuickDrawerSource).toContain('onpointerdown={handleSheetPointerDown}');
        expect(orchestrationQuickDrawerSource).toMatch(
            /\.quick-drawer\s*\{[^}]*position:\s*fixed;[^}]*bottom:\s*calc\(max\(-680px, -70dvh\) - 24px\);[^}]*grid-template-rows:\s*auto auto minmax\(0, 1fr\);[^}]*height:\s*min\(70dvh, 680px\);/s,
        );
        expect(orchestrationQuickDrawerSource).toMatch(
            /\.quick-drawer\.open\s*\{[^}]*bottom:\s*calc\(12px - var\(--sheet-drag-y, 0px\)\);[^}]*visibility:\s*visible;/s,
        );
        expect(orchestrationQuickDrawerSource).not.toContain('translate3d');
        expect(orchestrationQuickDrawerSource).not.toContain('will-change');
        expect(orchestrationQuickDrawerSource).toMatch(
            /@container view \(max-width:\s*640px\)[\s\S]*?\.quick-drawer\s*\{[^}]*bottom:\s*calc\(-70dvh - 24px\);[^}]*width:\s*100%;[^}]*height:\s*70dvh;[^}]*border-radius:\s*24px 24px 0 0;/s,
        );
        expect(orchestrationQuickDrawerSource).toMatch(
            /\.drawer-body\s*\{[^}]*overflow-y:\s*auto;/s,
        );
        expect(appCss).toMatch(
            /\.app-shell\[data-layout='mobile'\]\[data-view='chat'\] \.chat-pane::before\s*\{[^}]*position:\s*absolute;[^}]*z-index:\s*10;[^}]*background:\s*linear-gradient\(/s,
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
            /\.detail-action-bar :global\(\.detail-action--destructive\)\s*\{[^}]*color:\s*#ff0000;/s,
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
        expect(appCss).toMatch(
            /\.tab\[aria-current='page'\]::before\s*\{[^}]*background:\s*var\(--accent-soft\);/s,
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
