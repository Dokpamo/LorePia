import { describe, expect, it } from 'vitest';

import drawerSource from '../features/orchestration/OrchestrationQuickDrawer.svelte?raw';
import settingsSource from '../features/providers/settings/styles/provider-settings-a.css?raw';
import themeSource from '../lib/theme.ts?raw';
import foundationSource from './shared/foundation.css?raw';

describe('shared palette contract', () => {
    it('keeps fixed theme specimens centralized and component styles semantic', () => {
        for (const token of [
            '--theme-preview-light-canvas: #f5f5f5;',
            '--theme-preview-light-sidebar: #dedede;',
            '--theme-preview-light-main: #fafafa;',
            '--theme-preview-dark-frame: #464646;',
            '--theme-preview-dark-canvas: #1f1f1f;',
            '--theme-preview-dark-sidebar: #1b1b1b;',
        ]) {
            expect(foundationSource).toContain(token);
        }
        expect(settingsSource).not.toMatch(/#[\da-f]{3,8}\b|\brgba?\(/i);
        expect(settingsSource).toContain('background: var(--theme-preview-light-canvas);');
        expect(settingsSource).toContain('background: var(--theme-preview-dark-canvas);');
        expect(settingsSource).toContain('background: var(--theme-preview-system-overlay);');
    });

    it('aliases equal dark roles and shares floating-panel elevation', () => {
        const roleAliases = [
            ['sidebar-bg', 'sidebar-bg'],
            ['workspace-bg', 'workspace-bg'],
            ['panel-bg', 'panel-bg'],
            ['selection-bg', 'selection-bg'],
            ['hover-bg', 'hover-bg'],
            ['divider', 'divider'],
            ['control-line', 'composer-line'],
            ['ink', 'ink'],
            ['muted-ink', 'ink-muted'],
            ['subtle-ink', 'ink-subtle'],
        ] as const;
        for (const [settingsRole, sharedRole] of roleAliases) {
            const alias = `--desktop-settings-${settingsRole}: var(--desktop-${sharedRole});`;
            expect(foundationSource.split(alias)).toHaveLength(3);
        }
        expect(drawerSource).toMatch(
            /\.quick-drawer\.desktop\s*\{[^}]*box-shadow:\s*var\(--popover-shadow\);/s,
        );
    });

    it('keeps native edge-to-edge chrome in the selected or system palette', () => {
        expect(themeSource).toContain('syncNativeSystemBarStyle(resolvesToDark(preference))');
        expect(themeSource).toContain("query.addEventListener('change'");
    });
});
