import { svelte } from '@sveltejs/vite-plugin-svelte';
import { defineConfig } from 'vitest/config';

export default defineConfig({
    plugins: [svelte()],
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
});
