import { writable } from 'svelte/store';

/**
 * `system` follows the operating system. An explicit choice stamps
 * `data-theme` on the document root, which the stylesheet lets win over its
 * `prefers-color-scheme` query.
 */
export type ThemePreference = 'system' | 'light' | 'dark';

export const THEME_PREFERENCES: readonly ThemePreference[] = ['system', 'light', 'dark'];

const STORAGE_KEY = 'lorepia.theme';

function isThemePreference(value: unknown): value is ThemePreference {
    return value === 'system' || value === 'light' || value === 'dark';
}

function readStoredPreference(): ThemePreference {
    try {
        const stored = localStorage.getItem(STORAGE_KEY);
        return isThemePreference(stored) ? stored : 'system';
    } catch {
        // A blocked or absent store just means the system preference wins.
        return 'system';
    }
}

function applyPreference(preference: ThemePreference): void {
    const root = document.documentElement;
    if (preference === 'system') {
        root.removeAttribute('data-theme');
        return;
    }
    root.setAttribute('data-theme', preference);
}

export const themePreference = writable<ThemePreference>(readStoredPreference());

export function setThemePreference(preference: ThemePreference): void {
    themePreference.set(preference);
    applyPreference(preference);
    try {
        localStorage.setItem(STORAGE_KEY, preference);
    } catch {
        // Losing persistence must not lose the switch itself.
    }
}

/** Called once at startup, before the app mounts, so there is no flash. */
export function initTheme(): void {
    applyPreference(readStoredPreference());
}
