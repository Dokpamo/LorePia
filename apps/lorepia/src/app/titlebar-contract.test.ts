import { describe, expect, it } from 'vitest';

import baseConfig from '../../src-tauri/tauri.conf.json';
import developmentCapability from '../../src-tauri/capabilities/main-development.json';
import releaseCapability from '../../src-tauri/capabilities/main-release.json';
import demoConfig from '../../src-tauri/tauri.demo.conf.json';
import devConfig from '../../src-tauri/tauri.dev.conf.json';
import releaseConfig from '../../src-tauri/tauri.release.conf.json';
import chatPaneSource from '../features/chat/ChatPane.svelte?raw';
import orchestrationStudioSource from '../features/orchestration/OrchestrationStudio.svelte?raw';
import providerSettingsSource from '../features/providers/ProviderSettings.svelte?raw';
import appCss from '../styles/app.css?raw';
import appSource from './App.svelte?raw';

const windowConfigs = [baseConfig, devConfig, demoConfig, releaseConfig].map(
    (config) => config.app.windows[0],
);

describe('macOS title bar integration', () => {
    it('uses the overlay title bar in every Tauri launch profile', () => {
        for (const windowConfig of windowConfigs) {
            expect(windowConfig).toMatchObject({
                decorations: true,
                titleBarStyle: 'Overlay',
                hiddenTitle: true,
                trafficLightPosition: { x: 15, y: 25 },
            });
        }
    });

    it('retains the native drag commands in development and release builds', () => {
        for (const capability of [developmentCapability, releaseCapability]) {
            expect(capability.permissions).toEqual(
                expect.arrayContaining([
                    'core:window:allow-start-dragging',
                    'core:window:allow-internal-toggle-maximize',
                ]),
            );
        }
    });

    it('only enables native title bar geometry inside a macOS Tauri window', () => {
        expect(appSource).toContain("import { isTauri } from '@tauri-apps/api/core';");
        expect(appSource).toContain("if (!isTauri() || typeof window === 'undefined')");
        expect(appSource).toContain(
            "window.navigator.platform.startsWith('Mac') && window.navigator.maxTouchPoints === 0",
        );
        expect(appSource).toContain(
            "data-titlebar-overlay={nativeMacosTitlebarOverlay ? 'true' : 'false'}",
        );
    });

    it('turns each visible first-row surface into native draggable chrome', () => {
        expect(appSource).toContain('class="sidebar-head"');
        expect(appSource).toContain('titlebarOverlay={nativeMacosTitlebarOverlay}');
        expect(chatPaneSource).toContain('titlebarOverlay?: boolean;');
        expect(providerSettingsSource).toContain('class:titlebar-overlay={titlebarOverlay}');
        expect(orchestrationStudioSource).toContain('titlebarOverlay?: boolean;');

        for (const source of [
            appSource,
            chatPaneSource,
            providerSettingsSource,
            orchestrationStudioSource,
        ]) {
            expect(source).toContain('data-tauri-drag-region=');
        }
    });

    it('clears the traffic lights without changing browser preview geometry', () => {
        expect(appCss).toMatch(
            /\.app-shell\[data-titlebar-overlay='true'\]\s*\{[^}]*--native-traffic-light-clearance:\s*82px;/s,
        );
        expect(appCss).toMatch(
            /\.app-shell\[data-layout='desktop'\]\[data-titlebar-overlay='true'\] \.sidebar-head\s*\{[^}]*padding-left:\s*var\(--native-traffic-light-clearance\);/s,
        );
        expect(appCss).toMatch(
            /\.app-shell\[data-layout='mobile'\]\[data-titlebar-overlay='true'\]\s*\{[^}]*padding-top:\s*var\(--native-mobile-titlebar-inset\);/s,
        );
        expect(appCss).not.toContain(".app-shell[data-titlebar-overlay='false']");
    });
});
