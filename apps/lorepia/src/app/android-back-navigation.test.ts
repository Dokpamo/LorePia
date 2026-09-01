import { describe, expect, it } from 'vitest';

import androidActivitySource from '../../src-tauri/gen/android/app/src/main/java/dev/lorepia/app/MainActivity.kt?raw';
import appSource from './App.svelte?raw';
import { resolveMobileBackAction } from './android-back-navigation';

describe('Android system Back routing', () => {
    it('pops detail routes before leaving their main destination', () => {
        for (const view of ['chat', 'create', 'settings'] as const) {
            expect(resolveMobileBackAction({ view, pushed: true })).toBe('pop');
        }
    });

    it('returns top-level destinations to home and exits only from home', () => {
        for (const view of ['chat', 'create', 'settings'] as const) {
            expect(resolveMobileBackAction({ view, pushed: false })).toBe('home');
        }
        expect(resolveMobileBackAction({ view: 'home', pushed: false })).toBe('exit');
    });

    it('installs the handler with the same route descriptor as back swipe', () => {
        expect(appSource).toMatch(
            /installAndroidBack\(\s*mobileRouteDescriptor,\s*performBackSwipeNavigation,\s*showHome,\s*\)/s,
        );
    });

    it('bridges the Android dispatcher and backgrounds only from the root route', () => {
        expect(androidActivitySource).toContain('OnBackPressedCallback');
        expect(androidActivitySource).toContain('window.__LOREPIA_ANDROID_BACK__');
        expect(androidActivitySource).toMatch(
            /result == "\\"exit\\""[\s\S]*moveTaskToBack\(true\)/,
        );
    });
});
