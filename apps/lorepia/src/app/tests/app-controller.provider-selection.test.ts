import { get } from 'svelte/store';
import { describe, expect, it, vi } from 'vitest';

import type { AppSettingsDto, LorepiaClient } from '../../lib/ipc/contracts';
import { LorepiaAppController } from '../app-controller';
import { createAppControllerProviderFixture } from './app-controller-provider-test-support';

const {
    deferred,
    legacyProfile,
    modernSettings,
    normalizedLegacySettings,
    normalizedLegacyConnection,
    providerClient,
} = createAppControllerProviderFixture();

describe('LorepiaAppController retained legacy profile selection', () => {
    it('reselects a retained profile without retaining a conflicting modern target', async () => {
        const updateSettings = vi.fn(() => Promise.resolve(normalizedLegacySettings));
        const controller = new LorepiaAppController(providerClient(updateSettings));
        await controller.loadProviders();

        await expect(controller.selectLegacyProviderProfile(legacyProfile.id)).resolves.toBe(true);

        await vi.waitFor(() => expect(updateSettings).toHaveBeenCalledOnce());
        expect(updateSettings).toHaveBeenCalledWith({
            ...modernSettings,
            selected_provider_profile_id: legacyProfile.id,
            selected_model_route_id: null,
            selected_generation_preset_id: null,
        });
        expect(get(controller.state).providers.workspace.settings).toEqual(
            normalizedLegacySettings,
        );
        expect(get(controller.state).announcement).toBe(
            '기존 프로바이더를 기본 대상으로 저장했습니다.',
        );
        controller.destroy();
    });

    it('serializes settings mutations and rebases a preserve toggle on the normalized legacy selection', async () => {
        const firstUpdate = deferred<AppSettingsDto>();
        const updateSettings = vi.fn((settings: AppSettingsDto) =>
            updateSettings.mock.calls.length === 1
                ? firstUpdate.promise
                : Promise.resolve(settings),
        );
        const controller = new LorepiaAppController(providerClient(updateSettings));
        await controller.loadProviders();

        const select = controller.selectLegacyProviderProfile(legacyProfile.id);
        const preserve = controller.setPreservePartialGenerations(false);

        await vi.waitFor(() => expect(updateSettings).toHaveBeenCalledOnce());
        firstUpdate.resolve(normalizedLegacySettings);
        await expect(select).resolves.toBe(true);
        await expect(preserve).resolves.toBe(true);

        expect(updateSettings).toHaveBeenCalledTimes(2);
        expect(updateSettings).toHaveBeenNthCalledWith(2, {
            ...normalizedLegacySettings,
            preserve_partial_generations: false,
        });
        expect(get(controller.state).providers.workspace.settings).toEqual({
            ...normalizedLegacySettings,
            preserve_partial_generations: false,
        });
        controller.destroy();
    });

    it('keeps a completed legacy selection when an older provider refresh finishes later', async () => {
        const updateSettings = vi.fn(() => Promise.resolve(normalizedLegacySettings));
        const client = providerClient(updateSettings);
        const providerCatalogHistory = client.providerCatalogHistory.bind(client);
        const lateHistory = deferred<Awaited<ReturnType<typeof client.providerCatalogHistory>>>();
        let historyReads = 0;
        Object.assign(client, {
            providerCatalogHistory: (
                limit: number,
                beforeRevision: number | null,
                beforeStateVersion: number | null,
            ) => {
                historyReads += 1;
                return historyReads === 1
                    ? providerCatalogHistory(limit, beforeRevision, beforeStateVersion)
                    : lateHistory.promise;
            },
        });
        const controller = new LorepiaAppController(client);
        await controller.loadProviders();

        const staleRefresh = controller.loadProviders();
        await vi.waitFor(() => expect(historyReads).toBe(2));
        await expect(controller.selectLegacyProviderProfile(legacyProfile.id)).resolves.toBe(true);

        lateHistory.resolve(await providerCatalogHistory(50, null, null));
        await staleRefresh;

        expect(get(controller.state).providers.workspace.settings).toEqual(
            normalizedLegacySettings,
        );
        controller.destroy();
    });

    it('does not expose the dual-written same-ID connection as a second credential authority', async () => {
        const credentialStatus = vi.fn(() => Promise.resolve({ status: 'available' as const }));
        const captureCredential = vi.fn(() =>
            Promise.resolve({ clipboard_cleanup: 'cleared' as const }),
        );
        const deleteCredential = vi.fn(() => Promise.resolve());
        const client = {
            ...providerClient(vi.fn()),
            getProviderOverview: () =>
                Promise.resolve({
                    templates: [],
                    connections: [normalizedLegacyConnection],
                    legacy_profiles: [legacyProfile],
                    settings: normalizedLegacySettings,
                }),
            listModelRoutes: () => Promise.resolve([]),
            listProviderModelSyncs: () => Promise.resolve([]),
            credentialStatus,
            captureCredential,
            deleteCredential,
        } as unknown as LorepiaClient;
        const controller = new LorepiaAppController(client);

        await controller.loadProviders();
        await expect(
            controller.captureProviderCredential({
                kind: 'connection',
                connection_id: legacyProfile.id,
            }),
        ).resolves.toBe(false);
        await controller.deleteProviderCredential({
            kind: 'connection',
            connection_id: legacyProfile.id,
        });

        expect(credentialStatus).toHaveBeenCalledOnce();
        expect(credentialStatus).toHaveBeenCalledWith({
            kind: 'legacy_profile',
            provider_profile_id: legacyProfile.id,
        });
        expect(captureCredential).not.toHaveBeenCalled();
        expect(deleteCredential).not.toHaveBeenCalled();
        controller.destroy();
    });
});
