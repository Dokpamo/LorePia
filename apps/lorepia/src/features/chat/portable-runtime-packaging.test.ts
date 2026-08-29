import { describe, expect, it } from 'vitest';

import baseConfig from '../../../src-tauri/tauri.conf.json';
import devCapability from '../../../src-tauri/capabilities/main-development.json';
import releaseCapability from '../../../src-tauri/capabilities/main-release.json';
import devConfig from '../../../src-tauri/tauri.dev.conf.json';
import kernelSource from './portable-runtime-kernel.ts?raw';
import portableMessageSource from './PortableMessage.svelte?raw';
import runtimeSource from './portable-runtime.ts?raw';
import workerSource from './portable-runtime.worker.ts?raw';
import workerClientSource from './portable-runtime-worker-client.ts?raw';

describe('portable runtime packaging', () => {
    it('allows the bundled Lua WebAssembly runtime to load from the app origin', () => {
        expect(baseConfig.app.security.csp['script-src'].split(/\s+/)).toContain(
            "'wasm-unsafe-eval'",
        );
        expect(baseConfig.app.security.csp['worker-src'].split(/\s+/)).toContain("'self'");

        for (const config of [baseConfig, devConfig]) {
            expect(config.app.security.csp['connect-src'].split(/\s+/)).toContain("'self'");
        }
    });

    it('keeps Wasmoon and imported Lua execution out of the privileged renderer', () => {
        expect(runtimeSource).not.toContain("from 'wasmoon'");
        expect(runtimeSource).not.toContain('glue.wasm');
        expect(runtimeSource).not.toContain('thread.run(');
        expect(workerClientSource).toContain("new URL('./portable-runtime.worker.ts'");
        expect(workerClientSource).toContain("name: 'lorepia-portable-runtime'");
        expect(workerSource).toContain("from './portable-runtime-kernel'");
        expect(kernelSource).toContain("from 'wasmoon'");
        expect(kernelSource).toContain('thread.run(');
        expect(kernelSource).not.toContain("from '../../lib/ipc/client'");
        expect(kernelSource).not.toContain('LorepiaClient');
    });

    it('allows the chat runtime to subscribe and unsubscribe from native events', () => {
        for (const capability of [devCapability, releaseCapability]) {
            expect(capability.permissions).toContain('core:event:allow-listen');
            expect(capability.permissions).toContain('core:event:allow-unlisten');
        }
    });

    it('packages portable markup only in an opaque, capability-free iframe', () => {
        expect(baseConfig.app.security.csp['frame-src'].split(/\s+/)).toContain("'self'");
        expect(portableMessageSource).toContain('sandbox="allow-scripts"');
        expect(portableMessageSource).not.toContain('allow-same-origin');
        expect(portableMessageSource).toContain("event.origin !== 'null'");
        expect(portableMessageSource).toContain('event.source !== target.contentWindow');
        expect(portableMessageSource).toContain("connect-src 'none'");
        expect(portableMessageSource).toContain("form-action 'none'");
    });
});
