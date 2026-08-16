import { cleanup, fireEvent, render, screen, waitFor } from '@testing-library/svelte';
import { afterEach, describe, expect, it, vi } from 'vitest';

import type { BootstrapDto, CharacterDto, LorepiaClient } from '../lib/ipc/contracts';
import App from './App.svelte';

const BOOTSTRAP: BootstrapDto = {
    shell_api_version: 2,
    core_api_version: 9,
    chat_event_version: 4,
    health: {
        core_version: '0.1.0',
        database_open: true,
        schema_version: 1,
        data_root_writable: true,
        staging_writable: true,
        recovery_pending: false,
        active_jobs: 0,
    },
};

function deferred<Value>(): {
    promise: Promise<Value>;
    resolve: (value: Value) => void;
} {
    let resolvePromise!: (value: Value) => void;
    const promise = new Promise<Value>((resolve) => {
        resolvePromise = resolve;
    });
    return { promise, resolve: resolvePromise };
}

function appClient(
    bootstrapSnapshot: LorepiaClient['bootstrapSnapshot'],
    listPendingContentPackageImports = vi.fn().mockResolvedValue([]),
    listCompletedContentPackageExports = vi.fn().mockResolvedValue([]),
    listCharacters: LorepiaClient['listCharacters'] = vi.fn().mockResolvedValue([]),
): LorepiaClient {
    return {
        bootstrapSnapshot,
        listCharacters,
        getProviderOverview: vi.fn().mockResolvedValue({
            templates: [],
            connections: [],
            legacy_profiles: [],
            settings: {
                preserve_partial_generations: true,
                selected_provider_profile_id: null,
                selected_model_route_id: null,
                selected_generation_preset_id: null,
            },
        }),
        listProviderDiscoveries: vi.fn().mockResolvedValue([]),
        providerCatalogStatus: vi.fn().mockResolvedValue({
            status_schema_version: 1,
            state_version: 1,
            active_revision: 1,
            active_snapshot_sha256: 'synthetic-active',
            bundled_baseline_sha256: 'synthetic-baseline',
            snapshot_count: 1,
            signed_update_count: 0,
            highest_accepted_revision: 1,
            latest_issued_at: null,
            active_signed_revisions: [],
        }),
        providerCatalogHistory: vi.fn().mockResolvedValue({
            history_schema_version: 1,
            active_revision: 1,
            revisions: [],
            activations: [],
            next_before_revision: null,
            next_before_state_version: null,
        }),
        subscribeMemorySupervisorStatus: vi.fn().mockResolvedValue(() => undefined),
        getMemorySupervisorStatus: vi.fn().mockResolvedValue({
            sequence: 1,
            phase: 'running',
            recovered_interrupted_jobs: 0,
            completed_jobs: 0,
        }),
        listPendingContentPackageImports,
        listCompletedContentPackageExports,
    } as unknown as LorepiaClient;
}

afterEach(() => {
    cleanup();
    vi.restoreAllMocks();
});

describe('App bootstrap content-package recovery', () => {
    it('restores as soon as bootstrap succeeds while unrelated startup work is still pending', async () => {
        const bootstrap = deferred<BootstrapDto>();
        const library = deferred<CharacterDto[]>();
        const bootstrapSnapshot = vi.fn(() => bootstrap.promise);
        const listCharacters = vi.fn(() => library.promise);
        const listPendingContentPackageImports = vi.fn().mockResolvedValue([]);
        const listCompletedContentPackageExports = vi.fn().mockResolvedValue([]);
        render(App, {
            client: appClient(
                bootstrapSnapshot,
                listPendingContentPackageImports,
                listCompletedContentPackageExports,
                listCharacters,
            ),
        });

        await waitFor(() => expect(bootstrapSnapshot).toHaveBeenCalledOnce());
        bootstrap.resolve(BOOTSTRAP);

        await screen.findByText('로컬 Core');
        expect(listCharacters).toHaveBeenCalledOnce();
        await waitFor(() => {
            expect(listPendingContentPackageImports).toHaveBeenCalledOnce();
            expect(listCompletedContentPackageExports).toHaveBeenCalledOnce();
        });

        library.resolve([]);
    });

    it('waits for successful bootstrap before restoring pending imports and completed exports', async () => {
        const bootstrap = deferred<BootstrapDto>();
        const bootstrapSnapshot = vi.fn(() => bootstrap.promise);
        const listPendingContentPackageImports = vi.fn().mockResolvedValue([]);
        const listCompletedContentPackageExports = vi.fn().mockResolvedValue([]);
        render(App, {
            client: appClient(
                bootstrapSnapshot,
                listPendingContentPackageImports,
                listCompletedContentPackageExports,
            ),
        });

        await waitFor(() => expect(bootstrapSnapshot).toHaveBeenCalledOnce());
        expect(listPendingContentPackageImports).not.toHaveBeenCalled();
        expect(listCompletedContentPackageExports).not.toHaveBeenCalled();

        bootstrap.resolve(BOOTSTRAP);

        await waitFor(() => {
            expect(listPendingContentPackageImports).toHaveBeenCalledOnce();
            expect(listCompletedContentPackageExports).toHaveBeenCalledOnce();
        });
        expect(listPendingContentPackageImports).toHaveBeenCalledWith({ limit: 100 });
        expect(listCompletedContentPackageExports).toHaveBeenCalledWith({ limit: 100 });
    });

    it('restores only after a failed bootstrap is retried successfully', async () => {
        const bootstrapSnapshot = vi
            .fn<LorepiaClient['bootstrapSnapshot']>()
            .mockRejectedValueOnce(new Error('synthetic cold-start failure'))
            .mockResolvedValueOnce(BOOTSTRAP);
        const listPendingContentPackageImports = vi.fn().mockResolvedValue([]);
        const listCompletedContentPackageExports = vi.fn().mockResolvedValue([]);
        render(App, {
            client: appClient(
                bootstrapSnapshot,
                listPendingContentPackageImports,
                listCompletedContentPackageExports,
            ),
        });

        const retry = await screen.findByRole('button', { name: '다시 시도' });
        expect(listPendingContentPackageImports).not.toHaveBeenCalled();
        expect(listCompletedContentPackageExports).not.toHaveBeenCalled();

        await fireEvent.click(retry);

        await waitFor(() => {
            expect(bootstrapSnapshot).toHaveBeenCalledTimes(2);
            expect(listPendingContentPackageImports).toHaveBeenCalledOnce();
            expect(listCompletedContentPackageExports).toHaveBeenCalledOnce();
        });
    });
});
