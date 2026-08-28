import { describe, expect, it } from 'vitest';

import baseConfig from '../../../src-tauri/tauri.conf.json';
import devCapability from '../../../src-tauri/capabilities/main-development.json';
import releaseCapability from '../../../src-tauri/capabilities/main-release.json';
import devConfig from '../../../src-tauri/tauri.dev.conf.json';

describe('portable runtime packaging', () => {
    it('allows the bundled Lua WebAssembly runtime to load from the app origin', () => {
        expect(baseConfig.app.security.csp['script-src'].split(/\s+/)).toContain(
            "'wasm-unsafe-eval'",
        );

        for (const config of [baseConfig, devConfig]) {
            expect(config.app.security.csp['connect-src'].split(/\s+/)).toContain("'self'");
        }
    });

    it('allows the chat runtime to subscribe and unsubscribe from native events', () => {
        for (const capability of [devCapability, releaseCapability]) {
            expect(capability.permissions).toContain('core:event:allow-listen');
            expect(capability.permissions).toContain('core:event:allow-unlisten');
        }
    });
});
