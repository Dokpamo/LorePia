import { describe, expect, it } from 'vitest';

import appSource from '../app/App.svelte?raw';
import chatPaneSource from '../features/chat/ChatPane.svelte?raw';
import conversationPaneSource from '../features/conversations/ConversationPane.svelte?raw';
import libraryPaneSource from '../features/library/LibraryPane.svelte?raw';
import orchestrationStudioSource from '../features/orchestration/OrchestrationStudio.svelte?raw';
import providerSettingsSource from '../features/providers/ProviderSettings.svelte?raw';
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
    it('uses the downloaded bright sibling palette as the application color system', () => {
        const palette = {
            original: '#f1fb06',
            'acid-lime': '#b8ff00',
            'apple-green': '#63e85b',
            'emerald-mint': '#42e6a4',
            'electric-cyan': '#31dff3',
            'bright-sky-blue': '#62b5ff',
            'blue-lavender': '#8b8cff',
            'electric-violet': '#b56bff',
            'neon-pink': '#ff5aa5',
            'pop-coral': '#ff6d62',
            'tangerine-orange': '#ffaa24',
        } as const;

        for (const [name, value] of Object.entries(palette)) {
            expect(appCss).toContain(`--brand-${name}: ${value};`);
        }
        expect(appCss.match(/--primary-bg:\s*var\(--brand-tangerine-orange\);/g)).toHaveLength(3);
        expect(appCss).not.toMatch(/--primary-bg:\s*var\(--brand-original\);/);
        expect(appCss).not.toMatch(/--primary-bg-hover:\s*var\(--brand-acid-lime\);/);
        expect(appCss).not.toContain('#0e9384');
        expect(providerSettingsSource).toMatch(
            /\.settings-avatar\s*\{[^}]*background:\s*var\(--primary-bg\);/s,
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
        expect(appSource).toMatch(/class="mobile-top-bar mobile-root-header"/);
        expect(appSource.match(/class="mobile-top-bar sub-header"/g)).toHaveLength(2);
        expect(appSource).toContain('class:studio-detail-scroll={studioSection !== null}');
        expect(appSource).toContain('onscroll={handleStudioDetailScroll}');
        expect(
            appSource.match(/mobile-top-action mobile-top-action-left back-button/g),
        ).toHaveLength(2);
        expect(conversationPaneSource).toMatch(
            /class="mobile-top-bar mobile-root-header conversation-root-header"/,
        );
        expect(providerSettingsSource).toMatch(/class="mobile-top-bar settings-toolbar"/);
        expect(providerSettingsSource).toMatch(
            /mobile-top-action mobile-top-action-right settings-tool-button/,
        );
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
        expect(chatPaneSource).toMatch(/class="mobile-top-bar chat-header"/);
        expect(chatPaneSource).toMatch(/mobile-top-action mobile-top-action-left back-button/);
        expect(appCss).toMatch(
            /\.mobile-top-bar\s*\{[^}]*height:\s*calc\(var\(--mobile-root-header\) \+ env\(safe-area-inset-top\)\);[^}]*grid-template-columns:\s*var\(--mobile-top-action\) minmax\(0,\s*1fr\) var\(--mobile-top-action\);[^}]*padding-top:\s*env\(safe-area-inset-top\);[^}]*padding-inline-start:\s*max\(var\(--mobile-top-inset\),\s*env\(safe-area-inset-left\)\);[^}]*padding-inline-end:\s*max\(var\(--mobile-top-inset\),\s*env\(safe-area-inset-right\)\);/s,
        );
        expect(appCss).toMatch(
            /\.mobile-top-action\s*\{[^}]*width:\s*var\(--mobile-top-action\);[^}]*height:\s*var\(--mobile-top-action\);[^}]*border-radius:\s*50%;[^}]*background:\s*var\(--surface-raised\);[^}]*box-shadow:\s*var\(--shadow-1\);/s,
        );
        expect(appCss).toMatch(
            /\.app-shell\[data-layout='mobile'\] \.sub-header\s*\{[^}]*position:\s*absolute;[^}]*z-index:\s*10;[^}]*border:\s*0;[^}]*background:\s*transparent;[^}]*inset:\s*0 0 auto;[^}]*pointer-events:\s*none;/s,
        );
        expect(appCss).toMatch(
            /\.app-shell\[data-layout='mobile'\] \.sub-header > \.mobile-top-action\s*\{[^}]*z-index:\s*2;[^}]*pointer-events:\s*auto;/s,
        );
        expect(appCss).toMatch(
            /\.app-shell\[data-layout='mobile'\] :is\(\.studio-detail-scroll, \.settings-detail-scroll\)\s*\{[^}]*padding-top:\s*calc\(\s*env\(safe-area-inset-top\) \+ var\(--mobile-top-offset\) \+ var\(--mobile-top-action\) \+ 16px\s*\);[^}]*-webkit-mask-image:\s*linear-gradient\([\s\S]*?var\(--mobile-top-fade\)\s*\);[^}]*mask-image:\s*linear-gradient\([\s\S]*?var\(--mobile-top-fade\)\s*\);/s,
        );
        expect(appCss).toContain('rgb(0 0 0 / var(--mobile-top-mask-alpha, 1)) 0');
        expect(appCss).toMatch(
            /\.sub-header h1\s*\{[^}]*grid-column:\s*2;[^}]*padding-inline:\s*8px;[^}]*text-align:\s*center;/s,
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

    it('pins the mobile tab bar over page content instead of reserving a layout row', () => {
        expect(appCss).toMatch(
            /\.tab-bar\s*\{[^}]*position:\s*absolute;[^}]*bottom:\s*calc\(8px \+ env\(safe-area-inset-bottom\)\);[^}]*left:\s*50%;[^}]*width:\s*min\(calc\(100% - var\(--gutter\) - var\(--gutter\)\),\s*560px\);[^}]*transform:\s*translateX\(-50%\);/s,
        );
        expect(appCss).toMatch(/\.tab-bar\s*\{[^}]*margin:\s*0;/s);
    });

    it('keeps wide handhelds fluid and restores the desktop sidebar grid', () => {
        expect(appCss).toMatch(
            /\.app-shell\[data-layout='mobile'\]\s*\{[^}]*width:\s*min\(100%,\s*899px\);[^}]*margin-inline:\s*auto;[^}]*grid-template-columns:\s*0 minmax\(0,\s*1fr\);/s,
        );
        expect(appCss).toMatch(
            /\.app-shell\s*\{[^}]*display:\s*grid;[^}]*grid-template-columns:\s*0 minmax\(0,\s*1fr\);[^}]*transition:\s*grid-template-columns 240ms cubic-bezier\(0\.4,\s*0,\s*0\.2,\s*1\);/s,
        );
        expect(appCss).toMatch(
            /\.app-shell\[data-layout='desktop'\]\s*\{[^}]*width:\s*100%;[^}]*grid-template-columns:\s*var\(--sidebar\) minmax\(0,\s*1fr\);[^}]*transition-duration:\s*300ms;[^}]*transition-timing-function:\s*cubic-bezier\(0\.22,\s*0\.61,\s*0\.36,\s*1\);/s,
        );
        expect(appCss).toMatch(
            /\.sidebar-rail\s*\{[^}]*overflow:\s*hidden;[^}]*grid-column:\s*1;[^}]*grid-row:\s*1;/s,
        );
        expect(appCss).toMatch(/\.main\s*\{[^}]*grid-column:\s*2;[^}]*grid-row:\s*1;/s);
        expect(appSource).toContain("const DESKTOP_LAYOUT = '(min-width: 900px)'");
        expect(appSource).toContain("data-layout={isDesktop ? 'desktop' : 'mobile'}");
        expect(appSource).toContain('let sidebarContentMounted = $state(false)');
        expect(appSource).toContain('const SIDEBAR_EXIT_SETTLE_MS = 260');
        expect(appSource).toContain('sidebarUnmountTimer = setTimeout');
        expect(appSource).not.toContain("from 'svelte/transition'");
        expect(appSource).toContain("const REDUCED_MOTION = '(prefers-reduced-motion: reduce)'");
    });

    it('hides mobile scrollbars without disabling native wheel or touch scrolling', () => {
        expect(providerSettingsSource).toMatch(
            /\.provider-scroll\s*\{[^}]*height:\s*0;[^}]*min-height:\s*0;[^}]*flex:\s*1 1 0;[^}]*overflow-y:\s*scroll;/s,
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
            /\.conversation-pane\.root-view \.entity-list\s*\{[^}]*padding:\s*0 8px calc\(/s,
        );
        expect(appCss).toMatch(/\.message-scroll\s*\{[^}]*padding:\s*0 var\(--gutter\);/s);
    });

    it('uses shared root-screen geometry for home and chat', () => {
        expect(libraryPaneSource).toMatch(
            /class="library-search"\s+class:mobile-root-search=\{rootView\}/,
        );
        expect(conversationPaneSource).toMatch(
            /class="mobile-top-bar mobile-root-header conversation-root-header"/,
        );
        expect(conversationPaneSource).toMatch(/class="conversation-search mobile-root-search"/);
        expect(libraryPaneSource).toMatch(/class:mobile-root-fab=\{rootView\}/);
        expect(conversationPaneSource).toMatch(/class:mobile-root-fab=\{rootView\}/);
        expect(libraryPaneSource).toMatch(/class:mobile-root-row=\{rootView\}/);
        expect(conversationPaneSource).toMatch(/class:mobile-root-row=\{rootView\}/);
        expect(appCss).toMatch(/\.chat-list-view\s*\{[^}]*background:\s*var\(--bg\);/s);
        expect(appCss).toMatch(
            /\.app-shell\[data-layout='mobile'\] \.mobile-root-search\s*\{[^}]*min-height:\s*var\(--mobile-search\);[^}]*border:\s*1px solid var\(--line\);[^}]*width:\s*min\([\s\S]*?var\(--reading\)[\s\S]*?\);[^}]*margin:\s*0 auto 8px;[^}]*background:\s*var\(--surface-raised\);[^}]*box-shadow:\s*var\(--shadow-1\);/s,
        );
        expect(libraryPaneSource).not.toContain('mobile-root-empty');
        expect(conversationPaneSource).not.toContain('mobile-root-empty');
        expect(appCss).not.toContain('.mobile-root-empty');
        expect(appCss).toMatch(
            /\.app-shell\[data-layout='mobile'\] \.mobile-root-contact-action\s*\{[^}]*top:\s*calc\(65% - 10px\);[^}]*left:\s*50%;[^}]*transform:\s*translateX\(-50%\);/s,
        );
        expect(appCss).toMatch(
            /\.app-shell\[data-layout='mobile'\] \.mobile-root-contact-button\s*\{[^}]*width:\s*clamp\(155px,\s*45\.35vw,\s*198px\);[^}]*min-height:\s*clamp\(43px,\s*12\.52vw,\s*55px\);[^}]*border-radius:\s*var\(--radius-pill\);/s,
        );
        expect(appCss).toMatch(
            /\.app-shell\[data-layout='mobile'\] \.mobile-root-fab\s*\{[^}]*width:\s*56px;[^}]*height:\s*56px;[^}]*background:\s*var\(--primary-bg\);[^}]*box-shadow:\s*var\(--shadow-2\);/s,
        );
        expect(appCss).toMatch(
            /\.app-shell\[data-layout='mobile'\] \.mobile-root-row\s*\{[^}]*min-height:\s*var\(--mobile-row\);[^}]*border-radius:\s*var\(--radius-md\);/s,
        );
    });

    it('matches the Telegram pushed-header proportions at every mobile width', () => {
        expect(appCss).toMatch(
            /--mobile-root-header:\s*clamp\(56px,\s*16\.476vw,\s*72px\);[^}]*--mobile-top-action:\s*clamp\(42px,\s*12\.18vw,\s*53px\);[^}]*--mobile-top-inset:\s*clamp\(13px,\s*3\.89vw,\s*17px\);[^}]*--mobile-top-offset:\s*clamp\(14px,\s*4\.06vw,\s*18px\);[^}]*--mobile-root-title-inset:\s*clamp\(20px,\s*6\.095vw,\s*26px\);/s,
        );
        expect(appCss).toMatch(
            /\.mobile-top-action\s*\{[^}]*width:\s*var\(--mobile-top-action\);[^}]*height:\s*var\(--mobile-top-action\);[^}]*min-width:\s*var\(--mobile-top-action\);[^}]*min-height:\s*var\(--mobile-top-action\);/s,
        );
        expect(appCss).toMatch(
            /\.sub-header h1\s*\{[^}]*font-size:\s*clamp\(18px,\s*4\.577vw,\s*20px\);/s,
        );
        expect(appCss).toMatch(
            /\.mobile-top-bar > \.mobile-top-action\s*\{[^}]*margin-top:\s*var\(--mobile-top-offset\);/s,
        );
    });

    it('scales the search rail and floating tab bar from the 437px reference', () => {
        expect(appCss).toMatch(/--mobile-search:\s*clamp\(39px,\s*10\.984vw,\s*48px\);/);
        expect(appCss).toMatch(/--mobile-nav:\s*clamp\(53px,\s*15\.103vw,\s*66px\);/);
        expect(appCss).toMatch(
            /\.tab-bar\s*\{[^}]*padding:\s*clamp\(2px,\s*0\.686vw,\s*3px\);[^}]*gap:\s*2px;/s,
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
            /\.tab \.nav-icon\s*\{[^}]*width:\s*clamp\(22px,\s*5\.95vw,\s*26px\);[^}]*height:\s*clamp\(22px,\s*5\.95vw,\s*26px\);/s,
        );
        expect(appCss).toMatch(
            /\.tab-label\s*\{[^}]*font-size:\s*clamp\(11px,\s*3\.204vw,\s*14px\);[^}]*font-weight:\s*700;/s,
        );
        expect(appCss).toMatch(
            /\.mobile-root-header h1\s*\{[^}]*grid-column:\s*1 \/ 3;[^}]*align-self:\s*center;[^}]*padding-left:\s*calc\(var\(--mobile-root-title-inset\) - var\(--mobile-top-inset\)\);/s,
        );
        expect(appCss).toMatch(
            /\.app-shell\[data-layout='mobile'\] \.mobile-root-search input\s*\{[^}]*font-size:\s*clamp\(13px,\s*3\.432vw,\s*15px\);/s,
        );
    });
});
