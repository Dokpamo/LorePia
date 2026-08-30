import { get } from 'svelte/store';
import { describe, expect, it, vi } from 'vitest';

import type {
    CredentialTargetDto,
    DiscoveryOutboxEventDto,
    LorepiaClient,
    ProviderDiscoverySessionDto,
} from '../../lib/ipc/contracts';
import { LorepiaAppController } from '../app-controller';
import { createAppControllerProviderFixture } from './app-controller-provider-test-support';

const {
    deferred,
    providerClient,
    discoverySession,
    discoveryEvent,
    discoverySessionFor,
    discoveryClient,
} = createAppControllerProviderFixture();

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
