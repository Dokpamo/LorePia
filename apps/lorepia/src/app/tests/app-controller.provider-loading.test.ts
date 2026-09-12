import { get } from 'svelte/store';
import { describe, expect, it, vi } from 'vitest';
import { LorepiaAppController } from '../app-controller';
import {
    DEMO_PROVIDER_OVERVIEW,
    DEMO_CATALOG_HISTORY,
    DEMO_CATALOG_STATUS,
} from '../../preview/demo-data';
import type {
    CredentialTargetDto,
    LorepiaClient,
    ProviderOverviewDto,
} from '../../lib/ipc/contracts';
import { createAppControllerProviderFixture } from './app-controller-provider-test-support';

const { deferred, providerClient, discoveryClient, discoverySessionFor } =
    createAppControllerProviderFixture();

function fixture(overrides: Partial<LorepiaClient> = {}) {
    const overview = structuredClone(DEMO_PROVIDER_OVERVIEW);
    const client = {
        ...providerClient(vi.fn()),
        getProviderOverview: vi.fn().mockResolvedValue(overview),
        listModelRoutes: vi.fn(),
        listGenerationPresets: vi.fn(),
        listProviderDiscoveries: vi.fn(),
        providerCatalogStatus: vi.fn(),
        providerCatalogHistory: vi.fn(),
        listProviderModelSyncs: vi.fn(),
        credentialStatus: vi.fn().mockResolvedValue({ status: 'available' }),
        ...overrides,
    };
    const route = overview.routes[0];
    const connection = overview.connections[0];
    if (!route || !connection) throw new Error('Expected demo route and connection fixtures');
    return { overview, client, route, connection, controller: new LorepiaAppController(client) };
}

describe('provider workspace loading', () => {
    it('loads hundreds of routes through one snapshot and leaves optional diagnostics lazy', async () => {
        const { overview, client, route, controller } = fixture();
        overview.routes = Array.from({ length: 500 }, (_, i) => ({
            ...route,
            id: `route-${String(i)}`,
        }));
        await controller.loadProviders();
        expect(get(controller.state).providers.workspace.routes).toHaveLength(500);
        expect(client.getProviderOverview).toHaveBeenCalledOnce();
        for (const call of [
            client.listModelRoutes,
            client.listGenerationPresets,
            client.listProviderDiscoveries,
            client.providerCatalogStatus,
            client.providerCatalogHistory,
            client.listProviderModelSyncs,
        ])
            expect(call).not.toHaveBeenCalled();
        controller.destroy();
    });

    it('isolates optional catalog and credential failures from provider selection', async () => {
        const { overview, controller } = fixture({
            credentialStatus: vi.fn().mockRejectedValue(new Error('locked')),
            providerCatalogHistory: vi.fn().mockRejectedValue(new Error('history failed')),
            providerCatalogStatus: vi.fn().mockResolvedValue(null),
        });
        await controller.loadProviders();
        expect(await controller.loadProviderDiagnostics('catalog')).not.toBeNull();
        const state = get(controller.state).providers;
        expect(state.phase).toBe('ready');
        expect(state.workspace.routes).toEqual(overview.routes);
        expect(state.workspace.presets).toEqual(overview.presets);
        expect(Object.values(state.workspace.credential_statuses)).not.toContain('available');
        expect(Object.values(state.workspace.credential_statuses)).toContain('unreadable');
        controller.destroy();
    });

    it('keeps a newer snapshot when an older reload finishes last', async () => {
        const older = deferred<ProviderOverviewDto>();
        const newer = deferred<ProviderOverviewDto>();
        const { overview, controller } = fixture({
            getProviderOverview: vi
                .fn()
                .mockReturnValueOnce(older.promise)
                .mockReturnValueOnce(newer.promise),
        });
        const first = controller.loadProviders();
        const second = controller.loadProviders();
        newer.resolve({ ...overview, routes: [] });
        await second;
        older.resolve(overview);
        await first;
        expect(get(controller.state).providers.workspace.routes).toEqual([]);
        controller.destroy();
    });

    it('cannot restore a deleted credential from an older status read', async () => {
        const status = deferred<{ status: 'available' }>();
        const { connection, controller } = fixture({
            credentialStatus: vi.fn().mockReturnValue(status.promise),
            deleteCredential: vi.fn().mockResolvedValue(undefined),
        });
        const loading = controller.loadProviders();
        await vi.waitFor(() => expect(get(controller.state).providers.phase).toBe('ready'));
        const target: CredentialTargetDto = {
            kind: 'connection',
            connection_id: connection.id,
        };
        await controller.deleteProviderCredential(target);
        status.resolve({ status: 'available' });
        await loading;
        expect(
            get(controller.state).providers.workspace.credential_statuses[
                `connection:${target.connection_id}`
            ],
        ).toBe('missing');
        controller.destroy();
    });

    it('refreshes changed catalog data without re-reading every credential or diagnostics history', async () => {
        const { client, controller } = fixture({ upsertModelRoute: vi.fn().mockResolvedValue({}) });
        await controller.loadProviders();
        vi.mocked(client.credentialStatus).mockClear();
        await controller.upsertProviderModelRoute({
            kind: 'update',
            id: 'route',
            display_name: 'Renamed',
            status: 'available',
        });
        expect(client.getProviderOverview).toHaveBeenCalledTimes(2);
        expect(client.credentialStatus).not.toHaveBeenCalled();
        expect(client.listGenerationPresets).not.toHaveBeenCalled();
        expect(client.providerCatalogHistory).not.toHaveBeenCalled();
        controller.destroy();
    });

    it.each([{ routes: undefined }, { routes: {} }, { presets: null }])(
        'rejects missing or malformed catalog fields: %j',
        async (invalid) => {
            const { overview, controller } = fixture({
                getProviderOverview: vi
                    .fn()
                    .mockResolvedValue({ ...DEMO_PROVIDER_OVERVIEW, ...invalid }),
            });
            await controller.loadProviders();
            expect(get(controller.state).providers.phase).toBe('error');
            expect(get(controller.state).providers.workspace.routes).not.toEqual(overview.routes);
            controller.destroy();
        },
    );

    it('refreshes an already mounted catalog after activation and rejects its older pending read', async () => {
        const oldHistory = deferred<typeof DEMO_CATALOG_HISTORY>();
        const newStatus = { ...DEMO_CATALOG_STATUS, active_revision: 5 };
        const newHistory = { ...DEMO_CATALOG_HISTORY, active_revision: 5 };
        const { client, controller } = fixture({
            providerCatalogStatus: vi
                .fn()
                .mockResolvedValueOnce(DEMO_CATALOG_STATUS)
                .mockResolvedValue(newStatus),
            providerCatalogHistory: vi
                .fn()
                .mockReturnValueOnce(oldHistory.promise)
                .mockResolvedValue(newHistory),
            pickProviderCatalogImport: vi.fn().mockResolvedValue({ ticket_id: 'ticket' }),
            activateProviderCatalogImport: vi
                .fn()
                .mockResolvedValue({ status: newStatus, diff: null }),
        });
        await controller.loadProviders();
        const pending = controller.loadProviderDiagnostics('catalog');
        await controller.pickProviderCatalogImport();
        await controller.activateProviderCatalogImport();
        oldHistory.resolve(DEMO_CATALOG_HISTORY);
        await pending;
        const state = get(controller.state).providers;
        expect(client.providerCatalogHistory).toHaveBeenCalledTimes(2);
        expect(state.workspace.catalog_history?.active_revision).toBe(5);
        expect(state.workspace.catalog_status?.active_revision).toBe(5);
        expect(state.workspace.pending_catalog_import).toBeNull();
        expect(state.phase).toBe('ready');
        controller.destroy();
    });

    it('keeps committed catalog rollback success when optional history refresh fails', async () => {
        const status = { ...DEMO_CATALOG_STATUS, active_revision: 3 };
        const { controller } = fixture({
            prepareProviderCatalogRollback: vi
                .fn()
                .mockResolvedValue({ catalog_plan: { diff: null } }),
            activateProviderCatalogRollback: vi.fn().mockResolvedValue({ status }),
            providerCatalogStatus: vi.fn().mockResolvedValue(status),
            providerCatalogHistory: vi.fn().mockRejectedValue(new Error('history unavailable')),
        });
        await controller.loadProviders();
        await controller.prepareProviderCatalogRollback(3);
        await controller.activateProviderCatalogRollback();
        const state = get(controller.state);
        expect(state.providers.phase).toBe('ready');
        expect(state.providers.workspace.catalog_status?.active_revision).toBe(3);
        expect(state.providers.workspace.pending_catalog_rollback).toBeNull();
        expect(state.announcement).not.toBe('');
        controller.destroy();
    });

    it('refreshes only the modified connection credential and retains unrelated cached statuses', async () => {
        const { overview, client, connection, controller } = fixture({
            upsertProviderConnection: vi.fn().mockResolvedValue({}),
        });
        await controller.loadProviders();
        vi.mocked(client.credentialStatus).mockClear();
        const changed = { ...connection, display_name: 'Changed connection' };
        vi.mocked(client.getProviderOverview).mockResolvedValue({
            ...overview,
            connections: overview.connections.map((item) =>
                item.id === changed.id ? changed : item,
            ),
        });
        vi.mocked(client.credentialStatus).mockRejectedValue(new Error('vault locked'));
        await controller.updateProviderConnection({
            id: changed.id,
            display_name: changed.display_name,
            timeout_seconds: changed.timeout_seconds,
        });
        expect(client.credentialStatus).toHaveBeenCalledExactlyOnceWith({
            kind: 'connection',
            connection_id: changed.id,
        });
        expect(
            get(controller.state).providers.workspace.credential_statuses[
                `connection:${changed.id}`
            ],
        ).toBe('unreadable');
        expect(get(controller.state).providers.phase).toBe('ready');
        controller.destroy();
    });

    it('preserves a newer discovery revision and its credential status during an older list reload', async () => {
        const olderSession = discoverySessionFor('session', {
            credential_binding_requested: true,
            state: 'awaiting_review',
        });
        const newerSession = { ...olderSession, revision: olderSession.revision + 1 };
        const listing = deferred<(typeof olderSession)[]>();
        const { controller } = fixture({
            ...discoveryClient(
                () => Promise.resolve([]),
                () => Promise.resolve([]),
                () => Promise.resolve(true),
            ),
            listProviderDiscoveries: vi.fn().mockReturnValue(listing.promise),
            getProviderDiscovery: vi.fn().mockResolvedValue(newerSession),
            deleteCredential: vi.fn().mockResolvedValue(undefined),
        });
        await controller.loadProviders();
        const loading = controller.loadProviderDiagnostics('discovery');
        await controller.refreshProviderDiscovery(newerSession.id);
        await controller.deleteProviderCredential({
            kind: 'discovery_session',
            session_id: newerSession.id,
            expected_revision: newerSession.revision,
        });
        listing.resolve([olderSession]);
        await loading;
        const workspace = get(controller.state).providers.workspace;
        expect(
            workspace.discoveries.find((session) => session.id === newerSession.id)?.revision,
        ).toBe(newerSession.revision);
        expect(workspace.credential_statuses[`discovery_session:${newerSession.id}`]).toBe(
            'missing',
        );
        controller.destroy();
    });
});

it('refreshes credentials without serializing unused previous snapshot comparisons', async () => {
    const { overview, client, controller } = fixture();
    await controller.loadProviders();
    const previous = get(controller.state).providers.workspace;
    const stringify = vi.spyOn(JSON, 'stringify');
    vi.mocked(client.credentialStatus).mockClear();
    try {
        await controller.loadProviders();
        const compared = stringify.mock.calls.map(([value]): unknown => value);
        for (const value of [...previous.connections, ...previous.legacy_profiles])
            expect(compared).not.toContain(value);
        expect(client.credentialStatus).toHaveBeenCalled();
        expect(get(controller.state).providers.workspace.connections).toEqual(overview.connections);
    } finally {
        stringify.mockRestore();
        controller.destroy();
    }
});
