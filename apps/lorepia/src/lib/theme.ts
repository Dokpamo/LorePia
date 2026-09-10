import { isTauri } from '@tauri-apps/api/core';
import { writable } from 'svelte/store';

import { syncNativeSystemBarStyle } from './ipc/client';

/**
 * `system` follows the operating system. An explicit choice stamps
 * `data-theme` on the document root, which the stylesheet lets win over its
 * `prefers-color-scheme` query.
 */
export type ThemePreference = 'system' | 'light' | 'dark';

export const THEME_PREFERENCES: readonly ThemePreference[] = ['system', 'light', 'dark'];

const STORAGE_KEY = 'lorepia.theme';
const DEFAULT_THEME_PREFERENCE: ThemePreference = 'light';
const SYSTEM_DARK_QUERY = '(prefers-color-scheme: dark)';

let currentPreference: ThemePreference = readStoredPreference();
let systemThemeQuery: MediaQueryList | null = null;
let systemThemeListenerInstalled = false;

function isThemePreference(value: unknown): value is ThemePreference {
    return value === 'system' || value === 'light' || value === 'dark';
}

function readStoredPreference(): ThemePreference {
    try {
        const stored = localStorage.getItem(STORAGE_KEY);
        return isThemePreference(stored) ? stored : DEFAULT_THEME_PREFERENCE;
    } catch {
        // A blocked or absent store starts with LorePia's daylight identity.
        return DEFAULT_THEME_PREFERENCE;
    }
}

function getSystemThemeQuery(): MediaQueryList | null {
    if (systemThemeQuery !== null) return systemThemeQuery;
    if (typeof window === 'undefined' || typeof window.matchMedia !== 'function') return null;
    systemThemeQuery = window.matchMedia(SYSTEM_DARK_QUERY);
    return systemThemeQuery;
}

function resolvesToDark(preference: ThemePreference): boolean {
    return (
        preference === 'dark' ||
        (preference === 'system' && getSystemThemeQuery()?.matches === true)
    );
}

function syncNativeAppearance(preference: ThemePreference): void {
    if (!isTauri()) return;
    void syncNativeSystemBarStyle(resolvesToDark(preference)).catch(() => {
        // Native chrome is best-effort; a platform failure must not block the renderer theme.
    });
}

function applyPreference(preference: ThemePreference): void {
    const root = document.documentElement;
    if (preference === 'system') {
        root.removeAttribute('data-theme');
    } else {
        root.setAttribute('data-theme', preference);
    }
    syncNativeAppearance(preference);
}

function observeSystemTheme(): void {
    const query = getSystemThemeQuery();
    if (query === null || systemThemeListenerInstalled) return;
    query.addEventListener('change', () => {
        if (currentPreference === 'system') syncNativeAppearance('system');
    });
    systemThemeListenerInstalled = true;
}

export const themePreference = writable<ThemePreference>(currentPreference);

export function setThemePreference(preference: ThemePreference): void {
    currentPreference = preference;
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
    currentPreference = readStoredPreference();
    themePreference.set(currentPreference);
    observeSystemTheme();
    applyPreference(currentPreference);
}
