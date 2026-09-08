import { afterEach, describe, expect, it, vi } from 'vitest';
import { cleanup, fireEvent, render, screen } from '@testing-library/svelte';
import AppDetailHeader from './AppDetailHeader.svelte';
afterEach(cleanup);

import baseConfig from '../../src-tauri/tauri.conf.json';
import uiConfig from '../../src-tauri/tauri.ui.conf.json';
import developmentCapability from '../../src-tauri/capabilities/main-development.json';
import releaseCapability from '../../src-tauri/capabilities/main-release.json';
import demoConfig from '../../src-tauri/tauri.demo.conf.json';
import devConfig from '../../src-tauri/tauri.dev.conf.json';
import releaseConfig from '../../src-tauri/tauri.release.conf.json';

const windowConfigs = [baseConfig, devConfig, releaseConfig, uiConfig].map(
    (config) => config.app.windows[0],
);

describe('macOS title bar integration', () => {
    it('keeps native window controls above the shared workspace in every live profile', () => {
        for (const windowConfig of windowConfigs) {
            expect(windowConfig).toMatchObject({
                decorations: true,
                titleBarStyle: 'Visible',
                hiddenTitle: false,
                minWidth: 320,
                minHeight: 552,
            });
        }
    });

    it('keeps the earlier demo shell on its own overlay title bar', () => {
        expect(demoConfig.app.windows[0]).toMatchObject({
            titleBarStyle: 'Overlay',
            hiddenTitle: true,
        });
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

    it.each([false, true])(
        'keeps the back action accessible with native overlay %s',
        async (titlebarOverlay) => {
            const onBack = vi.fn();
            render(AppDetailHeader, {
                title: 'Settings',
                desktop: true,
                titlebarOverlay,
                fadeProgress: 0,
                onBack,
            });
            const heading = screen.getByRole('heading', { name: 'Settings' });
            expect(heading.hasAttribute('data-tauri-drag-region')).toBe(titlebarOverlay);
            const button = screen.getByRole('button');
            expect(button).toHaveAccessibleName();
            expect(button).not.toHaveAttribute('data-tauri-drag-region');
            await fireEvent.click(button);
            expect(onBack).toHaveBeenCalledOnce();
        },
    );
});
