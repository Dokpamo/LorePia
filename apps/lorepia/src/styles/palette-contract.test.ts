import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { get } from 'svelte/store';

const { syncNative } = vi.hoisted(() => ({ syncNative: vi.fn().mockResolvedValue(undefined) }));
vi.mock('@tauri-apps/api/core', () => ({ isTauri: () => true }));
vi.mock('../lib/ipc/client', () => ({ syncNativeSystemBarStyle: syncNative }));

describe('renderer and native theme behavior', () => {
    let systemDark = false;
    let onSystemChange: (() => void) | undefined;
    beforeEach(() => {
        vi.resetModules();
        syncNative.mockClear();
        localStorage.clear();
        systemDark = false;
        onSystemChange = undefined;
        vi.stubGlobal('matchMedia', () => ({
            get matches() {
                return systemDark;
            },
            addEventListener: (_event: string, callback: () => void) => {
                onSystemChange = callback;
            },
        }));
    });
    afterEach(() => {
        vi.unstubAllGlobals();
        document.documentElement.removeAttribute('data-theme');
        localStorage.clear();
    });

    it('applies and persists an explicit palette to the renderer, store and native chrome', async () => {
        const theme = await import('../lib/theme');
        theme.initTheme();
        theme.setThemePreference('dark');
        expect(document.documentElement).toHaveAttribute('data-theme', 'dark');
        expect(get(theme.themePreference)).toBe('dark');
        expect(localStorage.getItem('lorepia.theme')).toBe('dark');
        expect(syncNative).toHaveBeenLastCalledWith(true);
        systemDark = true;
        onSystemChange?.();
        theme.setThemePreference('light');
        expect(document.documentElement).toHaveAttribute('data-theme', 'light');
        expect(syncNative).toHaveBeenLastCalledWith(false);
    });

    it('follows OS changes only while system mode is selected', async () => {
        const theme = await import('../lib/theme');
        theme.initTheme();
        theme.setThemePreference('system');
        expect(document.documentElement).not.toHaveAttribute('data-theme');
        expect(syncNative).toHaveBeenLastCalledWith(false);
        systemDark = true;
        onSystemChange?.();
        expect(syncNative).toHaveBeenLastCalledWith(true);
        theme.setThemePreference('light');
        syncNative.mockClear();
        systemDark = false;
        onSystemChange?.();
        expect(syncNative).not.toHaveBeenCalled();
        expect(document.documentElement).toHaveAttribute('data-theme', 'light');
    });
});
