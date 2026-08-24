import { svelte } from '@sveltejs/vite-plugin-svelte';
import type { Plugin, ViteDevServer } from 'vite';
import { defineConfig } from 'vitest/config';

function demoEntryPlugin(): Plugin {
    return {
        name: 'lorepia-demo-entry',
        configureServer(server: ViteDevServer) {
            server.middlewares.use((request, _response, next) => {
                const requestUrl: unknown = Reflect.get(request, 'url');
                if (
                    typeof requestUrl === 'string' &&
                    (requestUrl === '/' || requestUrl.startsWith('/?'))
                ) {
                    Reflect.set(request, 'url', `/preview.html${requestUrl.slice(1)}`);
                }
                next();
            });
        },
    };
}

export default defineConfig(({ mode }) => ({
    plugins: [mode === 'demo' && demoEntryPlugin(), svelte()],
    clearScreen: false,
    envPrefix: ['VITE_', 'TAURI_'],
    resolve: {
        conditions: ['browser'],
    },
    server: {
        strictPort: true,
    },
    test: {
        environment: 'jsdom',
        setupFiles: ['./src/tests/setup.ts'],
        css: true,
    },
}));
