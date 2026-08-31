import { describe, expect, it } from 'vitest';

import choicePopoverSource from '../components/ChoicePopover.svelte?raw';
import segmentedControlSource from '../components/SegmentedControl.svelte?raw';
import toggleSwitchSource from '../components/ToggleSwitch.svelte?raw';
import trustedAssetSource from '../features/assets/TrustedAsset.svelte?raw';
import chatErrorSource from '../features/chat/ChatErrorRegion.svelte?raw';
import chatPaneSource from '../features/chat/ChatPane.svelte?raw';
import orchestrationStudioSource from '../features/orchestration/OrchestrationStudio.svelte?raw';
import orchestrationMemorySource from '../features/orchestration/studio/MemorySection.svelte?raw';
import orchestrationStudioStylesA from '../features/orchestration/studio/styles/studio-a.css?raw';
import orchestrationStudioStylesB from '../features/orchestration/studio/styles/studio-b.css?raw';
import capabilityPanelSource from '../features/providers/CapabilityPanel.svelte?raw';
import personaPanelSource from '../features/personas/PersonaPanel.svelte?raw';
import appCss from './app.css?raw';

const orchestrationStudioContractSource = [
    orchestrationStudioSource,
    orchestrationMemorySource,
    orchestrationStudioStylesA,
    orchestrationStudioStylesB,
].join('\n');

const svelteSources = import.meta.glob<string>('../**/*.svelte', {
    eager: true,
    query: '?raw',
    import: 'default',
});

function channel(value: string): number {
    return Number.parseInt(value, 16) / 255;
}

function relativeLuminance(hex: string): number {
    const values = [hex.slice(1, 3), hex.slice(3, 5), hex.slice(5, 7)].map(channel);
    const linear = values.map((value) =>
        value <= 0.04045 ? value / 12.92 : ((value + 0.055) / 1.055) ** 2.4,
    );
    const [red = 0, green = 0, blue = 0] = linear;
    return 0.2126 * red + 0.7152 * green + 0.0722 * blue;
}

function contrast(foreground: string, background: string): number {
    const light = Math.max(relativeLuminance(foreground), relativeLuminance(background));
    const dark = Math.min(relativeLuminance(foreground), relativeLuminance(background));
    return (light + 0.05) / (dark + 0.05);
}

describe('semantic state styling', () => {
    it('defines complete light and dark feedback palettes and a neutral modal scrim', () => {
        for (const token of [
            'status-error-fg',
            'status-error-bg',
            'status-error-border',
            'status-warning-fg',
            'status-warning-bg',
            'status-warning-border',
            'status-success-fg',
            'status-success-bg',
            'status-success-border',
            'status-info-fg',
            'status-info-bg',
            'status-info-border',
            'overlay-scrim',
            'disabled-opacity',
            'control-inset-shadow',
            'popover-shadow',
        ]) {
            expect(appCss.match(new RegExp(`--${token}:`, 'g'))).toHaveLength(3);
        }

        expect(appCss).toMatch(/\.modal-backdrop\s*\{[^}]*background:\s*var\(--overlay-scrim\);/s);
        expect(appCss).toMatch(
            /\.modal-backdrop::backdrop\s*\{[^}]*background:\s*var\(--overlay-scrim\);/s,
        );
    });

    it('keeps hidden choices inside the LorePia popover system', () => {
        for (const [path, source] of Object.entries(svelteSources)) {
            expect(source, path).not.toMatch(/<select\b/u);
            expect(source, path).not.toMatch(/<option\b/u);
        }
    });

    it('does not regress to platform colors or raw destructive red', () => {
        for (const [path, source] of Object.entries(svelteSources)) {
            expect(source, path).not.toMatch(/#ff0000\b/iu);
            expect(source, path).not.toMatch(/\bCanvas\b/u);
        }
        expect(trustedAssetSource).toContain('var(--status-error-bg)');
        expect(trustedAssetSource).toContain('var(--status-info-bg)');
    });

    it('keeps warning states distinct from errors', () => {
        expect(personaPanelSource).toContain('class="persona-feedback error"');
        expect(personaPanelSource).toMatch(
            /\.persona-feedback\.warning\s*\{[^}]*var\(--status-warning-border\);[^}]*var\(--status-warning-fg\);[^}]*var\(--status-warning-bg\);/s,
        );
        expect(capabilityPanelSource).toMatch(
            /\.effective-result\.warning\s*\{[^}]*var\(--status-warning-border\);[^}]*var\(--status-warning-bg\);/s,
        );
        expect(capabilityPanelSource).toMatch(
            /\.warning-badge\s*\{[^}]*color:\s*var\(--status-warning-fg\);/s,
        );
    });

    it('renders delayed memory failures as errors instead of neutral notices', () => {
        expect(orchestrationStudioContractSource).toContain(
            "class:error={appState.memory_supervisor.status.phase === 'failed'}",
        );
        expect(orchestrationStudioContractSource).toMatch(
            /memory_supervisor\.error[^]*class="bounded-note error" role="alert"/u,
        );
        expect(orchestrationStudioContractSource).toMatch(/\.bounded-note\.error,/u);
    });

    it('keeps chat errors above the transcript instead of stretching them along the bottom edge', () => {
        expect(chatErrorSource).toMatch(
            /\.chat-error-region\s*\{[^}]*position:\s*absolute;[^}]*z-index:\s*24;/s,
        );
        expect(chatErrorSource).toMatch(
            /app-shell\[data-layout='desktop'\][^{}]*\.chat-error-region\s*\{[^}]*top:\s*72px;/s,
        );
        expect(chatErrorSource).toMatch(
            /\.chat-error-notice\s*\{[^}]*width:\s*min\(100%, 680px\);[^}]*var\(--status-error-bg\);[^}]*var\(--popover-shadow\);/s,
        );
        expect(chatErrorSource.match(/class="chat-error-dismiss"/g)).toHaveLength(3);
        expect(chatPaneSource).not.toContain('class="state-panel error portable-runtime-status"');
        expect(chatPaneSource).toContain('copyNotice !== portableRuntimeLifecycle.error');
    });

    it('uses the same disabled contract and moving control thumbs', () => {
        expect(choicePopoverSource).toMatch(
            /\.choice-option:disabled\s*\{[^}]*opacity:\s*var\(--disabled-opacity\);/s,
        );
        expect(toggleSwitchSource).toMatch(
            /\.toggle-switch:disabled\s*\{[^}]*opacity:\s*var\(--disabled-opacity\);/s,
        );
        expect(segmentedControlSource).toMatch(
            /\.segmented-control button:disabled\s*\{[^}]*opacity:\s*var\(--disabled-opacity\);/s,
        );
        expect(toggleSwitchSource).toContain('transform: translate3d(16px, 0, 0);');
        expect(segmentedControlSource).toContain(
            'transform: translate3d(calc(var(--segment-index) * 100%), 0, 0);',
        );
    });

    it('keeps tiny dark-theme secondary text above normal-text contrast', () => {
        expect(contrast('#858585', '#1b1b1b')).toBeGreaterThanOrEqual(4.5);
        expect(contrast('#858585', '#151515')).toBeGreaterThanOrEqual(4.5);
    });
});
