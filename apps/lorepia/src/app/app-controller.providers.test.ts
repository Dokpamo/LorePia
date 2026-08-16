import { get } from 'svelte/store';
import { describe, expect, it, vi } from 'vitest';

import type {
    AppSettingsDto,
    CredentialTargetDto,
    DiscoveryOutboxEventDto,
    LorepiaClient,
    ProviderConnectionDto,
    ProviderDiscoveryEventDto,
    ProviderDiscoverySessionDto,
    ProviderProfileDto,
} from '../lib/ipc/contracts';
import { LorepiaAppController } from './app-controller';

function deferred<T>(): {
    promise: Promise<T>;
    resolve: (value: T) => void;
    reject: (reason?: unknown) => void;
} {
    let resolve!: (value: T) => void;
    let reject!: (reason?: unknown) => void;
    const promise = new Promise<T>((resolvePromise, rejectPromise) => {
        resolve = resolvePromise;
        reject = rejectPromise;
    });
    return { promise, resolve, reject };
}

const legacyProfile: ProviderProfileDto = {
    id: 'legacy-profile-1',
    display_name: '보존된 레거시 프로필',
    base_url: 'https://synthetic.invalid/v1',
    model: 'synthetic-model',
    timeout_seconds: 30,
};

const modernSettings: AppSettingsDto = {
    preserve_partial_generations: true,
    selected_provider_profile_id: null,
    selected_model_route_id: 'modern-route',
    selected_generation_preset_id: 'modern-preset',
};

const normalizedLegacySettings: AppSettingsDto = {
    preserve_partial_generations: true,
    selected_provider_profile_id: legacyProfile.id,
    selected_model_route_id: legacyProfile.id,
    selected_generation_preset_id: legacyProfile.id,
};

const normalizedLegacyConnection: ProviderConnectionDto = {
    id: legacyProfile.id,
    template_id: 'legacy-openai-compatible',
    template_version: 1,
    display_name: '정규화된 레거시 연결',
    api_origin: 'https://synthetic.invalid',
    api_base_path: '/v1',
    network_mode: 'public',
    local_network_approval: null,
    config_values: [],
    credential_binding_required: true,
    credential_scope: null,
    approved_credential_origins: [],
    timeout_seconds: 30,
    status: 'active',
    created_at: '2026-08-11T00:00:00Z',
    updated_at: '2026-08-11T00:00:00Z',
};

function providerClient(
    updateSettings: (settings: AppSettingsDto) => Promise<AppSettingsDto>,
): LorepiaClient {
    return {
        getProviderOverview: () =>
            Promise.resolve({
                templates: [],
                connections: [],
                legacy_profiles: [legacyProfile],
                settings: modernSettings,
            }),
        listProviderDiscoveries: () => Promise.resolve([]),
        providerCatalogStatus: () =>
            Promise.resolve({
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
        providerCatalogHistory: () =>
            Promise.resolve({
                history_schema_version: 1,
                active_revision: 1,
                revisions: [],
                activations: [],
                next_before_revision: null,
                next_before_state_version: null,
            }),
        credentialStatus: () => Promise.resolve({ status: 'available' }),
        updateSettings,
    } as unknown as LorepiaClient;
}

const discoverySession: ProviderDiscoverySessionDto = {
    snapshot_schema_version: 1,
    id: 'selected-discovery-session',
    connection_id: 'synthetic-connection',
    display_name: '합성 탐색',
    site_url: 'https://synthetic.invalid',
    docs_url: null,
    credential_binding_requested: false,
    preferred_assistant: null,
    connection_options: {
        values: [],
        api_base_path: null,
        timeout_seconds: 30,
        network_mode: 'public',
        local_network_approval: null,
    },
    supplied_evidence_ids: [],
    state: 'awaiting_action',
    revision: 1,
    next_event_sequence: 2,
    steps: [],
    action_required: { kind: 'select_template', operation: null },
    active_operation_id: null,
    recovery_operation: null,
    unknown_operation: null,
    manifest_sha256: null,
    commit_plan_sha256: null,
    commit_attempt_id: null,
    committed_connection_id: null,
    cancellation_pending: false,
    active_effect_approval: null,
    failure: null,
    has_private_draft: false,
    review: null,
    assistant_resume_boundary: null,
    created_at: '2026-08-11T00:00:00Z',
    updated_at: '2026-08-11T00:00:00Z',
};

const discoveryEvent: ProviderDiscoveryEventDto = {
    version: 1,
    id: 'selected-discovery-event',
    session_id: discoverySession.id,
    sequence: 1,
    session_revision: discoverySession.revision,
    state: discoverySession.state,
    progress: null,
    action_required: discoverySession.action_required,
    warning: null,
    action_id: 'selected-discovery-action',
    failure: null,
};

function discoverySessionFor(
    id: string,
    overrides: Partial<ProviderDiscoverySessionDto> = {},
): ProviderDiscoverySessionDto {
    return {
        ...structuredClone(discoverySession),
        id,
        display_name: `합성 탐색 ${id}`,
        ...overrides,
    };
}

function discoveryClient(
    pollForSession: (sessionId: string, limit: number) => Promise<DiscoveryOutboxEventDto[]>,
    pollGlobal: (limit: number) => Promise<DiscoveryOutboxEventDto[]>,
    acknowledge: (eventId: string) => Promise<boolean>,
): LorepiaClient {
    return {
        getProviderDiscovery: () => Promise.resolve(discoverySession),
        listProviderDiscoveryCandidates: () => Promise.resolve([]),
        listProviderDiscoveryEvidence: () => Promise.resolve([]),
        listProviderDiscoveryApprovals: () => Promise.resolve([]),
        getProviderDiscoveryReview: () => Promise.resolve(null),
        getProviderDiscoveryApprovalProposal: () => Promise.resolve(null),
        getProviderDiscoveryReviewProposal: () => Promise.resolve(null),
        getProviderDiscoveryAssistantResumeBoundary: () => Promise.resolve(null),
        pollProviderDiscoveryEventsForSession: pollForSession,
        pollProviderDiscoveryEvents: pollGlobal,
        ackProviderDiscoveryEvent: acknowledge,
    } as unknown as LorepiaClient;
}

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

describe('LorepiaAppController discovery credential status', () => {
    it('loads the exact precommit and committing credential targets without renderer authority fields', async () => {
        const discoveries = [
            discoverySessionFor('origin', {
                credential_binding_requested: true,
                state: 'awaiting_credential_origin_approval',
                revision: 2,
            }),
            discoverySessionFor('probes', {
                credential_binding_requested: true,
                state: 'awaiting_probe_consent',
                revision: 3,
            }),
            discoverySessionFor('review', {
                credential_binding_requested: true,
                state: 'awaiting_review',
                revision: 4,
            }),
            discoverySessionFor('restart-models', {
                credential_binding_requested: true,
                state: 'interrupted',
                recovery_operation: 'list_models',
                revision: 5,
            }),
            discoverySessionFor('restart-probes', {
                credential_binding_requested: true,
                state: 'interrupted',
                recovery_operation: 'probe_capabilities',
                revision: 6,
            }),
            discoverySessionFor('committing', {
                credential_binding_requested: true,
                state: 'committing',
                revision: 7,
                commit_attempt_id: 'attempt-1',
                commit_plan_sha256: 'plan-1',
            }),
            discoverySessionFor('interrupted-unrelated', {
                credential_binding_requested: true,
                state: 'interrupted',
                recovery_operation: 'fetch_document',
                revision: 8,
            }),
            discoverySessionFor('review-without-binding', {
                credential_binding_requested: false,
                state: 'awaiting_review',
                revision: 9,
            }),
        ];
        const credentialStatus = vi.fn((target: CredentialTargetDto) =>
            Promise.resolve({
                status:
                    target.kind === 'discovery_session' && target.session_id === 'review'
                        ? ('unreadable' as const)
                        : target.kind === 'discovery_session' && target.session_id === 'probes'
                          ? ('missing' as const)
                          : ('available' as const),
            }),
        );
        const client = {
            ...providerClient(vi.fn()),
            listProviderDiscoveries: () => Promise.resolve(discoveries),
            credentialStatus,
        } as unknown as LorepiaClient;
        const controller = new LorepiaAppController(client);

        await controller.loadProviders();

        const discoveryTargets = credentialStatus.mock.calls
            .map(([target]) => target)
            .filter((target) => target.kind === 'discovery_session');
        expect(discoveryTargets).toEqual(
            discoveries.slice(0, 6).map((session) => ({
                kind: 'discovery_session',
                session_id: session.id,
                expected_revision: session.revision,
            })),
        );
        expect(get(controller.state).providers.workspace.credential_statuses).toMatchObject({
            'discovery_session:origin': 'available',
            'discovery_session:probes': 'missing',
            'discovery_session:review': 'unreadable',
            'discovery_session:restart-models': 'available',
            'discovery_session:restart-probes': 'available',
            'discovery_session:committing': 'available',
        });
        controller.destroy();
    });

    it('rechecks the new revision and replaces a precommit lease with final-vault status', async () => {
        let session = discoverySessionFor('credential-transition', {
            credential_binding_requested: true,
            state: 'awaiting_review',
            revision: 4,
        });
        const credentialStatus = vi.fn((target: CredentialTargetDto) =>
            Promise.resolve({
                status:
                    target.kind === 'discovery_session' && target.expected_revision === 4
                        ? ('available' as const)
                        : ('missing' as const),
            }),
        );
        const client = {
            ...discoveryClient(
                vi.fn(() => Promise.resolve([])),
                vi.fn(() => Promise.resolve([])),
                vi.fn(),
            ),
            getProviderDiscovery: () => Promise.resolve(session),
            credentialStatus,
            listProviderDiscoveryCompensationSteps: () => Promise.resolve([]),
        } as unknown as LorepiaClient;
        const controller = new LorepiaAppController(client);

        await controller.refreshProviderDiscovery(session.id);
        expect(
            get(controller.state).providers.workspace.credential_statuses[
                `discovery_session:${session.id}`
            ],
        ).toBe('available');

        session = {
            ...session,
            state: 'committing',
            revision: 5,
            commit_attempt_id: 'attempt-5',
            commit_plan_sha256: 'plan-5',
        };
        await controller.refreshProviderDiscovery(session.id);

        expect(credentialStatus.mock.calls.map(([target]) => target)).toEqual([
            {
                kind: 'discovery_session',
                session_id: session.id,
                expected_revision: 4,
            },
            {
                kind: 'discovery_session',
                session_id: session.id,
                expected_revision: 5,
            },
        ]);
        expect(
            get(controller.state).providers.workspace.credential_statuses[
                `discovery_session:${session.id}`
            ],
        ).toBe('missing');

        session = { ...session, state: 'completed', revision: 6 };
        await controller.refreshProviderDiscovery(session.id);
        expect(get(controller.state).providers.workspace.credential_statuses).not.toHaveProperty(
            `discovery_session:${session.id}`,
        );
        expect(credentialStatus).toHaveBeenCalledTimes(2);
        controller.destroy();
    });
});

describe('LorepiaAppController selected discovery event polling', () => {
    it('polls the selected session directly instead of filtering a bounded global page', async () => {
        const selectedOutboxEvent: DiscoveryOutboxEventDto = {
            event: discoveryEvent,
            delivery_attempts: 1,
            available_at: '2026-08-11T00:00:00Z',
            created_at: '2026-08-11T00:00:00Z',
        };
        const pollForSession = vi
            .fn<() => Promise<DiscoveryOutboxEventDto[]>>()
            .mockResolvedValueOnce([selectedOutboxEvent])
            .mockResolvedValue([]);
        const pollGlobal = vi.fn(() => Promise.resolve([]));
        const acknowledge = vi.fn<(eventId: string) => Promise<boolean>>().mockResolvedValue(true);
        const controller = new LorepiaAppController(
            discoveryClient(pollForSession, pollGlobal, acknowledge),
        );
        await controller.refreshProviderDiscovery(discoverySession.id);

        await controller.pollSelectedProviderDiscoveryEvents();

        expect(pollForSession).toHaveBeenCalledTimes(2);
        expect(pollForSession).toHaveBeenNthCalledWith(1, discoverySession.id, 100);
        expect(pollForSession).toHaveBeenNthCalledWith(2, discoverySession.id, 99);
        expect(pollGlobal).not.toHaveBeenCalled();
        expect(acknowledge).toHaveBeenCalledWith(discoveryEvent.id);
        expect(get(controller.state).providers.workspace.discovery_event).toEqual(discoveryEvent);
        controller.destroy();
    });

    it('drains one selected-session FIFO through acknowledgement until the poll is empty', async () => {
        const firstEvent: DiscoveryOutboxEventDto = {
            event: { ...discoveryEvent, id: 'event-1', sequence: 1, action_id: 'receipt-1' },
            delivery_attempts: 1,
            available_at: '2026-08-11T00:00:00Z',
            created_at: '2026-08-11T00:00:00Z',
        };
        const secondEvent: DiscoveryOutboxEventDto = {
            ...firstEvent,
            event: {
                ...discoveryEvent,
                id: 'event-2',
                sequence: 2,
                session_revision: 2,
                action_id: 'receipt-2',
            },
        };
        const pollForSession = vi
            .fn<() => Promise<DiscoveryOutboxEventDto[]>>()
            .mockResolvedValueOnce([firstEvent])
            .mockResolvedValueOnce([secondEvent])
            .mockResolvedValue([]);
        const pollGlobal = vi.fn(() => Promise.resolve([]));
        const acknowledge = vi.fn<(eventId: string) => Promise<boolean>>().mockResolvedValue(true);
        const controller = new LorepiaAppController(
            discoveryClient(pollForSession, pollGlobal, acknowledge),
        );
        await controller.refreshProviderDiscovery(discoverySession.id);

        await controller.pollSelectedProviderDiscoveryEvents();

        expect(pollForSession).toHaveBeenCalledTimes(3);
        expect(acknowledge.mock.calls.map(([eventId]) => eventId)).toEqual([
            firstEvent.event.id,
            secondEvent.event.id,
        ]);
        expect(pollGlobal).not.toHaveBeenCalled();
        expect(get(controller.state).providers.workspace.discovery_event).toEqual(
            secondEvent.event,
        );
        controller.destroy();
    });

    it('bounds a selected-session drain that never becomes empty', async () => {
        let sequence = 0;
        const pollForSession = vi.fn(() => {
            sequence += 1;
            return Promise.resolve([
                {
                    event: {
                        ...discoveryEvent,
                        id: `event-${String(sequence)}`,
                        sequence,
                        action_id: `receipt-${String(sequence)}`,
                    },
                    delivery_attempts: 1,
                    available_at: '2026-08-11T00:00:00Z',
                    created_at: '2026-08-11T00:00:00Z',
                },
            ]);
        });
        const acknowledge = vi.fn(() => Promise.resolve(true));
        const controller = new LorepiaAppController(
            discoveryClient(
                pollForSession,
                vi.fn(() => Promise.resolve([])),
                acknowledge,
            ),
        );
        await controller.refreshProviderDiscovery(discoverySession.id);

        await controller.pollSelectedProviderDiscoveryEvents();

        expect(pollForSession).toHaveBeenCalledTimes(100);
        expect(acknowledge).toHaveBeenCalledTimes(100);
        expect(get(controller.state).announcement).toBe(
            '탐색 이벤트가 너무 많이 쌓여 일부만 확인했습니다. 다시 확인해 주세요.',
        );
        controller.destroy();
    });

    it('uses a fresh idempotency ID and the durable session action after restart', async () => {
        let currentSession = discoverySession;
        const continueDiscovery = vi.fn(() => {
            currentSession = {
                ...currentSession,
                state: 'awaiting_more_evidence',
                revision: 2,
                action_required: { kind: 'supply_more_evidence', operation: null },
            };
            return Promise.resolve(currentSession);
        });
        const client = {
            ...discoveryClient(
                vi.fn(() => Promise.resolve([])),
                vi.fn(() => Promise.resolve([])),
                vi.fn(),
            ),
            getProviderDiscovery: () => Promise.resolve(currentSession),
            continueProviderDiscovery: continueDiscovery,
        };
        const randomUuid = vi
            .spyOn(globalThis.crypto, 'randomUUID')
            .mockReturnValue('00000000-0000-4000-8000-000000000777');
        const controller = new LorepiaAppController(client);
        await controller.refreshProviderDiscovery(discoverySession.id);
        expect(get(controller.state).providers.workspace.discovery_event).toBeNull();

        await expect(
            controller.continueProviderDiscovery({
                kind: 'select_template',
                candidate_id: 'candidate-1',
            }),
        ).resolves.toBe(true);

        expect(continueDiscovery).toHaveBeenCalledWith({
            session_id: discoverySession.id,
            action_id: '00000000-0000-4000-8000-000000000777',
            expected_revision: discoverySession.revision,
            action: { kind: 'select_template', candidate_id: 'candidate-1' },
        });
        randomUuid.mockRestore();
        controller.destroy();
    });

    it('keeps the later B selection when refresh A resolves last', async () => {
        const sessionA = discoverySessionFor('discovery-a');
        const sessionB = discoverySessionFor('discovery-b');
        const responseA = deferred<ProviderDiscoverySessionDto>();
        const responseB = deferred<ProviderDiscoverySessionDto>();
        const pollForSession = vi.fn(() => Promise.resolve([]));
        const client = {
            ...discoveryClient(
                pollForSession,
                vi.fn(() => Promise.resolve([])),
                vi.fn(),
            ),
            getProviderDiscovery: vi.fn((sessionId: string) =>
                sessionId === sessionA.id ? responseA.promise : responseB.promise,
            ),
        } as unknown as LorepiaClient;
        const controller = new LorepiaAppController(client);

        const refreshA = controller.refreshProviderDiscovery(sessionA.id);
        const refreshB = controller.refreshProviderDiscovery(sessionB.id);
        responseB.resolve(sessionB);
        await refreshB;
        responseA.resolve(sessionA);
        await refreshA;

        const workspace = get(controller.state).providers.workspace;
        expect(workspace.selected_discovery_id).toBe(sessionB.id);
        expect(workspace.discoveries[0]).toEqual(sessionB);
        controller.destroy();
    });

    it('does not acknowledge or reselect A when its poll resolves after B is selected', async () => {
        const sessionA = discoverySessionFor('discovery-a');
        const sessionB = discoverySessionFor('discovery-b');
        const pollA = deferred<DiscoveryOutboxEventDto[]>();
        const responseB = deferred<ProviderDiscoverySessionDto>();
        const eventA: DiscoveryOutboxEventDto = {
            event: { ...discoveryEvent, id: 'event-a', session_id: sessionA.id },
            delivery_attempts: 1,
            available_at: '2026-08-11T00:00:00Z',
            created_at: '2026-08-11T00:00:00Z',
        };
        const pollForSession = vi.fn((sessionId: string) =>
            sessionId === sessionA.id ? pollA.promise : Promise.resolve([]),
        );
        const acknowledge = vi.fn(() => Promise.resolve(true));
        const getProviderDiscovery = vi.fn((sessionId: string) =>
            sessionId === sessionB.id ? responseB.promise : Promise.resolve(sessionA),
        );
        const client = {
            ...discoveryClient(
                pollForSession,
                vi.fn(() => Promise.resolve([])),
                acknowledge,
            ),
            getProviderDiscovery,
        } as unknown as LorepiaClient;
        const controller = new LorepiaAppController(client);
        await controller.refreshProviderDiscovery(sessionA.id);

        const stalePoll = controller.pollSelectedProviderDiscoveryEvents();
        const refreshB = controller.refreshProviderDiscovery(sessionB.id);
        responseB.resolve(sessionB);
        await refreshB;
        pollA.resolve([eventA]);
        await stalePoll;

        const workspace = get(controller.state).providers.workspace;
        expect(workspace.selected_discovery_id).toBe(sessionB.id);
        expect(workspace.discovery_event).not.toEqual(eventA.event);
        expect(acknowledge).not.toHaveBeenCalledWith(eventA.event.id);
        expect(getProviderDiscovery.mock.calls.filter(([id]) => id === sessionA.id)).toHaveLength(
            1,
        );
        controller.destroy();
    });

    it('stops an A drain when B is selected while the first A acknowledgement is pending', async () => {
        const sessionA = discoverySessionFor('discovery-a');
        const sessionB = discoverySessionFor('discovery-b');
        const pendingAcknowledgement = deferred<boolean>();
        const firstAEvent: DiscoveryOutboxEventDto = {
            event: { ...discoveryEvent, id: 'event-a-1', session_id: sessionA.id },
            delivery_attempts: 1,
            available_at: '2026-08-11T00:00:00Z',
            created_at: '2026-08-11T00:00:00Z',
        };
        const pollForSession = vi.fn((sessionId: string) =>
            Promise.resolve(sessionId === sessionA.id ? [firstAEvent] : []),
        );
        const acknowledge = vi.fn(() => pendingAcknowledgement.promise);
        const client = {
            ...discoveryClient(
                pollForSession,
                vi.fn(() => Promise.resolve([])),
                acknowledge,
            ),
            getProviderDiscovery: (sessionId: string) =>
                Promise.resolve(sessionId === sessionA.id ? sessionA : sessionB),
        };
        const controller = new LorepiaAppController(client);
        await controller.refreshProviderDiscovery(sessionA.id);

        const staleDrain = controller.pollSelectedProviderDiscoveryEvents();
        await vi.waitFor(() => expect(acknowledge).toHaveBeenCalledOnce());
        await controller.refreshProviderDiscovery(sessionB.id);
        pendingAcknowledgement.resolve(true);
        await staleDrain;

        expect(pollForSession.mock.calls.filter(([id]) => id === sessionA.id)).toHaveLength(1);
        expect(get(controller.state).providers.workspace.selected_discovery_id).toBe(sessionB.id);
        expect(get(controller.state).providers.workspace.discovery_event).not.toEqual(
            firstAEvent.event,
        );
        controller.destroy();
    });
});
