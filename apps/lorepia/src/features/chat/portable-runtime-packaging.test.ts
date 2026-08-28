import { describe, expect, it } from 'vitest';

import baseConfig from '../../../src-tauri/tauri.conf.json';
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
});
