import { describe, expect, it } from 'vitest';

import androidActivitySource from '../../src-tauri/gen/android/app/src/main/java/dev/lorepia/app/MainActivity.kt?raw';
import appSource from './App.svelte?raw';
import { dismissMobileBackLayer, resolveMobileBackAction } from './android-back-navigation';

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

    it('lets the focused top layer consume Back as Escape before routing', () => {
        const button = document.createElement('button');
        button.addEventListener('keydown', (event) => {
            if (event.key === 'Escape') event.preventDefault();
        });
        document.body.append(button);
        button.focus();

        expect(dismissMobileBackLayer()).toBe(true);
        button.remove();
    });

    it('dispatches cancel to an open modal dialog before routing', () => {
        const dialog = document.createElement('dialog');
        dialog.open = true;
        dialog.setAttribute('aria-modal', 'true');
        dialog.addEventListener('cancel', (event) => event.preventDefault());
        document.body.append(dialog);

        expect(dismissMobileBackLayer()).toBe(true);
        dialog.remove();
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

    it('promotes the app callback after Tauri registers its WebView fallback', () => {
        expect(androidActivitySource).toMatch(
            /onWebViewCreate\(webView: WebView\)[\s\S]*webView\.post\s*\{\s*promoteBackCallback\(\)\s*\}/,
        );
        expect(androidActivitySource).toMatch(
            /promoteBackCallback\(\)[\s\S]*appBackCallback\.remove\(\)[\s\S]*onBackPressedDispatcher\.addCallback/,
        );
    });
});
