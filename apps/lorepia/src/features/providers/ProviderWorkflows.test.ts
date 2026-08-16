import { cleanup, fireEvent, render, screen, waitFor } from '@testing-library/svelte';
import { afterEach, describe, expect, it, vi } from 'vitest';

import {
    INITIAL_APP_STATE,
    LorepiaAppController,
    type LorepiaAppState,
} from '../../app/app-controller';
import type {
    LorepiaClient,
    ModelRouteDto,
    ModelSyncJobDto,
    ProviderCatalogImportTicketDto,
    ProviderCatalogRollbackPlanDto,
    ProviderDiscoverySessionDto,
} from '../../lib/ipc/contracts';
import CatalogPanel from './CatalogPanel.svelte';
import DiscoveryPanel from './DiscoveryPanel.svelte';
import ModelSyncPanel from './ModelSyncPanel.svelte';

afterEach(() => {
    cleanup();
    vi.restoreAllMocks();
});

const ASSISTANT_ROUTE: ModelRouteDto = {
    id: 'route-assistant',
    connection_id: 'connection-assistant',
    api_family: 'open_ai_responses',
    model_id: 'assistant-model',
    display_name: '설정 도우미',
    route_config: {
        deployment_id: null,
        region: null,
        endpoint_path: null,
        values: [],
    },
    status: 'available',
    miss_count: 0,
    metadata_source: 'synthetic',
    metadata_observed_at: null,
    first_seen_at: '2026-08-02T00:00:00Z',
    last_seen_at: null,
};

function providerState(): LorepiaAppState {
    const state = structuredClone(INITIAL_APP_STATE);
    state.providers.phase = 'ready';
    state.providers.workspace.routes = [ASSISTANT_ROUTE];
    return state;
}

function discoverySession(
    overrides: Partial<ProviderDiscoverySessionDto> = {},
): ProviderDiscoverySessionDto {
    return {
        snapshot_schema_version: 3,
        id: 'discovery-1',
        connection_id: 'connection-1',
        display_name: 'Synthetic provider',
        site_url: 'https://provider.example',
        docs_url: null,
        credential_binding_requested: false,
        preferred_assistant: 'route-assistant',
        connection_options: {
            values: [],
            api_base_path: null,
            timeout_seconds: 30,
            network_mode: 'public',
            local_network_approval: null,
        },
        supplied_evidence_ids: [],
        state: 'awaiting_review',
        revision: 7,
        next_event_sequence: 3,
        steps: [],
        action_required: { kind: 'review', operation: null },
        active_operation_id: null,
        recovery_operation: null,
        unknown_operation: null,
        manifest_sha256: 'manifest-sha',
        commit_plan_sha256: 'plan-sha',
        commit_attempt_id: null,
        committed_connection_id: null,
        cancellation_pending: false,
        active_effect_approval: null,
        failure: null,
        has_private_draft: true,
        review: null,
        assistant_resume_boundary: null,
        created_at: '2026-08-02T00:00:00Z',
        updated_at: '2026-08-02T00:00:01Z',
        ...overrides,
    };
}

function createController(): LorepiaAppController {
    return new LorepiaAppController({} as LorepiaClient);
}

function selectDiscovery(
    appState: LorepiaAppState,
    session: ProviderDiscoverySessionDto,
    actionKind: string | null,
    credentialStatus: 'available' | 'missing' | 'unreadable',
): void {
    appState.providers.workspace.discoveries = [session];
    appState.providers.workspace.selected_discovery_id = session.id;
    appState.providers.workspace.credential_statuses[`discovery_session:${session.id}`] =
        credentialStatus;
    appState.providers.workspace.discovery_event =
        actionKind === null
            ? null
            : {
                  version: 1,
                  id: `event-${actionKind}`,
                  session_id: session.id,
                  sequence: 1,
                  session_revision: session.revision,
                  state: session.state,
                  progress: null,
                  action_required: { kind: actionKind, operation: session.recovery_operation },
                  warning: null,
                  action_id: `action-${actionKind}`,
                  failure: null,
              };
    appState.providers.workspace.discovery_approval_proposal =
        actionKind === 'approve_credential_origin' || actionKind === 'approve_probes'
            ? { id: 'approval-credential', grant: {}, grant_sha256: 'grant-sha' }
            : null;
}

describe('provider discovery workflow', () => {
    it('recovers an awaiting action from the durable session when no outbox event remains', async () => {
        const appState = providerState();
        const session = discoverySession({
            state: 'awaiting_template_selection',
            action_required: { kind: 'select_template', operation: null },
        });
        appState.providers.workspace.discoveries = [session];
        appState.providers.workspace.selected_discovery_id = session.id;
        appState.providers.workspace.discovery_event = null;
        const controller = createController();
        const continueDiscovery = vi
            .spyOn(controller, 'continueProviderDiscovery')
            .mockResolvedValue(true);
        render(DiscoveryPanel, { appState, controller });

        await fireEvent.click(screen.getByRole('button', { name: '템플릿 없이 계속' }));

        expect(continueDiscovery).toHaveBeenCalledWith({ kind: 'continue_without_template' });
        expect(screen.getByText('아직 확인하지 않은 탐색 이벤트가 없습니다.')).toBeInTheDocument();
        controller.destroy();
    });

    it('starts a durable site discovery with the explicitly selected assistant route', async () => {
        const appState = providerState();
        const controller = createController();
        const begin = vi.spyOn(controller, 'beginProviderDiscovery').mockResolvedValue(true);
        render(DiscoveryPanel, { appState, controller });

        await fireEvent.input(screen.getByLabelText('연결 ID'), {
            target: { value: 'connection-new' },
        });
        await fireEvent.input(screen.getByLabelText('표시 이름'), {
            target: { value: '새 프로바이더' },
        });
        await fireEvent.input(screen.getByLabelText('사이트 URL'), {
            target: { value: 'https://new.example' },
        });
        await fireEvent.change(screen.getByLabelText('설정 도우미 모델 (선택)'), {
            target: { value: 'route-assistant' },
        });
        await fireEvent.click(screen.getByRole('button', { name: '탐색 시작' }));

        await waitFor(() => expect(begin).toHaveBeenCalledOnce());
        const request = begin.mock.calls[0]?.[0];
        expect(request?.kind).toBe('site');
        if (request?.kind !== 'site') throw new Error('site request was not captured');
        expect(request.input.connection_id).toBe('connection-new');
        expect(request.input.preferred_assistant).toBe('route-assistant');
        expect(request.input.source).toEqual({ kind: 'site' });
        controller.destroy();
    });

    it('keeps deterministic discovery available without a remote assistant', async () => {
        const appState = providerState();
        const controller = createController();
        const begin = vi.spyOn(controller, 'beginProviderDiscovery').mockResolvedValue(true);
        render(DiscoveryPanel, { appState, controller });

        await fireEvent.input(screen.getByLabelText('연결 ID'), {
            target: { value: 'connection-deterministic' },
        });
        await fireEvent.input(screen.getByLabelText('표시 이름'), {
            target: { value: '결정론적 탐색' },
        });
        await fireEvent.input(screen.getByLabelText('사이트 URL'), {
            target: { value: 'https://deterministic.example' },
        });
        await fireEvent.click(screen.getByRole('button', { name: '탐색 시작' }));

        await waitFor(() => expect(begin).toHaveBeenCalledOnce());
        const request = begin.mock.calls[0]?.[0];
        expect(request?.kind).toBe('site');
        if (request?.kind !== 'site') throw new Error('site request was not captured');
        expect(request.input.preferred_assistant).toBeNull();
        expect(request.input.source).toEqual({ kind: 'site' });
        controller.destroy();
    });

    it.each([
        {
            label: 'origin approval',
            state: 'awaiting_credential_origin_approval',
            recoveryOperation: null,
            actionKind: 'approve_credential_origin',
            dependentButton: '표시된 origin 승인',
            expectedAction: {
                kind: 'approve_credential_origin',
                approval_id: 'approval-credential',
            },
        },
        {
            label: 'capability probes',
            state: 'awaiting_probe_consent',
            recoveryOperation: null,
            actionKind: 'approve_probes',
            dependentButton: '표시된 probe만 승인',
            expectedAction: {
                kind: 'approve_probes',
                approval_id: 'approval-credential',
                approval_grant_sha256: 'grant-sha',
            },
        },
        {
            label: 'credential-bound restart',
            state: 'interrupted',
            recoveryOperation: 'list_models',
            actionKind: 'restart_interrupted',
            dependentButton: '중단 작업 명시적으로 재개',
            expectedAction: { kind: 'restart_interrupted' },
        },
    ])('captures a credential before $label and enables only an available lease', async (flow) => {
        const missingState = providerState();
        const missingSession = discoverySession({
            credential_binding_requested: true,
            state: flow.state,
            recovery_operation: flow.recoveryOperation,
            action_required: { kind: flow.actionKind, operation: flow.recoveryOperation },
        });
        selectDiscovery(missingState, missingSession, flow.actionKind, 'missing');
        const missingController = createController();
        const capture = vi
            .spyOn(missingController, 'captureProviderCredential')
            .mockResolvedValue(true);
        render(DiscoveryPanel, { appState: missingState, controller: missingController });

        const dependent = screen.getByRole('button', { name: flow.dependentButton });
        expect(dependent).toBeDisabled();
        if (flow.actionKind === 'approve_probes') {
            expect(screen.getByRole('button', { name: 'Probe 건너뛰기' })).toBeEnabled();
        }
        await fireEvent.click(screen.getByRole('button', { name: '자격증명 네이티브 캡처' }));
        expect(capture).toHaveBeenCalledWith({
            kind: 'discovery_session',
            session_id: missingSession.id,
            expected_revision: missingSession.revision,
        });
        missingController.destroy();

        cleanup();
        const availableState = providerState();
        const availableSession = discoverySession({
            credential_binding_requested: true,
            state: flow.state,
            recovery_operation: flow.recoveryOperation,
            action_required: { kind: flow.actionKind, operation: flow.recoveryOperation },
        });
        selectDiscovery(availableState, availableSession, flow.actionKind, 'available');
        const availableController = createController();
        const continueDiscovery = vi
            .spyOn(availableController, 'continueProviderDiscovery')
            .mockResolvedValue(true);
        render(DiscoveryPanel, { appState: availableState, controller: availableController });

        const enabled = screen.getByRole('button', { name: flow.dependentButton });
        expect(enabled).toBeEnabled();
        await fireEvent.click(enabled);
        expect(continueDiscovery).toHaveBeenCalledWith(flow.expectedAction);
        availableController.destroy();
    });

    it('offers precommit recapture during review when the lease binding is unreadable', async () => {
        const appState = providerState();
        const session = discoverySession({
            credential_binding_requested: true,
            state: 'awaiting_review',
            action_required: { kind: 'review', operation: null },
        });
        selectDiscovery(appState, session, 'review', 'unreadable');
        const controller = createController();
        const capture = vi.spyOn(controller, 'captureProviderCredential').mockResolvedValue(true);
        render(DiscoveryPanel, { appState, controller });

        await fireEvent.click(screen.getByRole('button', { name: '자격증명 네이티브 캡처' }));
        expect(capture).toHaveBeenCalledWith({
            kind: 'discovery_session',
            session_id: session.id,
            expected_revision: session.revision,
        });
        controller.destroy();
    });

    it('keeps a committing final-vault capture fallback and blocks commit until available', async () => {
        const appState = providerState();
        const session = discoverySession({
            credential_binding_requested: true,
            state: 'committing',
            action_required: null,
            review: {
                sha256: 'review-sha',
                graph_sha256: 'graph-sha',
                changes: [],
                unresolved_question_count: 0,
                warning_count: 0,
            },
            commit_attempt_id: 'attempt-1',
            commit_plan_sha256: 'plan-sha',
        });
        selectDiscovery(appState, session, null, 'missing');
        const controller = createController();
        const capture = vi.spyOn(controller, 'captureProviderCredential').mockResolvedValue(true);
        render(DiscoveryPanel, { appState, controller });

        expect(screen.getByRole('button', { name: '승인된 연결 적용' })).toBeDisabled();
        await fireEvent.click(screen.getByRole('button', { name: '자격증명 네이티브 캡처' }));
        expect(capture).toHaveBeenCalledWith({
            kind: 'discovery_session',
            session_id: session.id,
            expected_revision: session.revision,
        });
        controller.destroy();

        cleanup();
        const availableState = providerState();
        selectDiscovery(availableState, session, null, 'available');
        const availableController = createController();
        const commit = vi
            .spyOn(availableController, 'commitProviderDiscovery')
            .mockResolvedValue(true);
        render(DiscoveryPanel, { appState: availableState, controller: availableController });

        await fireEvent.click(screen.getByRole('button', { name: '승인된 연결 적용' }));
        expect(commit).toHaveBeenCalledOnce();
        availableController.destroy();
    });

    it('echoes the exact reviewed plan values and can cancel the durable session', async () => {
        const appState = providerState();
        const session = discoverySession();
        appState.providers.workspace.discoveries = [session];
        appState.providers.workspace.selected_discovery_id = session.id;
        appState.providers.workspace.discovery_event = {
            version: 1,
            id: 'event-1',
            session_id: session.id,
            sequence: 2,
            session_revision: session.revision,
            state: session.state,
            progress: null,
            action_required: { kind: 'review', operation: null },
            warning: null,
            action_id: 'action-review',
            failure: null,
        };
        appState.providers.workspace.discovery_review_proposal = {
            review: {
                sha256: 'review-sha',
                graph_sha256: 'graph-sha',
                changes: [
                    {
                        kind: 'add',
                        target_kind: 'connection',
                        target_id: 'connection-1',
                        summary_key: 'connection.add',
                        evidence_ids: ['evidence-1'],
                    },
                ],
                unresolved_question_count: 0,
                warning_count: 0,
            },
            approval: {
                id: 'approval-1',
                grant: {},
                grant_sha256: 'grant-sha',
            },
            commit_attempt_id: 'attempt-1',
            commit_plan_sha256: 'plan-sha',
            request_preview: null,
        };
        const controller = createController();
        const continueDiscovery = vi
            .spyOn(controller, 'continueProviderDiscovery')
            .mockResolvedValue(true);
        const cancel = vi.spyOn(controller, 'cancelProviderDiscovery').mockResolvedValue();
        render(DiscoveryPanel, { appState, controller });

        await fireEvent.click(screen.getByRole('button', { name: '검토한 정확한 계획 승인' }));
        expect(continueDiscovery).toHaveBeenCalledWith({
            kind: 'approve_review',
            approval_id: 'approval-1',
            commit_attempt_id: 'attempt-1',
            commit_plan_sha256: 'plan-sha',
            graph_sha256: 'graph-sha',
        });

        await fireEvent.click(screen.getByRole('button', { name: '탐색 취소' }));
        expect(cancel).toHaveBeenCalledOnce();
        controller.destroy();
    });

    it('requires explicit assistant restart and disables untrusted remote turn pricing', async () => {
        const appState = providerState();
        const resumeBoundary = {
            checkpoint: 'ready',
            action: 'restart_interrupted' as const,
            questions: [],
            draft_review: null,
        };
        const session = discoverySession({
            state: 'interrupted',
            commit_attempt_id: 'attempt-1',
            action_required: null,
            assistant_resume_boundary: resumeBoundary,
        });
        appState.providers.workspace.discoveries = [session];
        appState.providers.workspace.selected_discovery_id = session.id;
        appState.providers.workspace.discovery_assistant_resume_boundary = resumeBoundary;
        appState.providers.workspace.discovery_compensation_steps = [
            {
                id: 'compensation-1',
                commit_attempt_id: 'attempt-1',
                ordinal: 1,
                action_id: 'compensate-1',
                kind: 'delete_connection',
                status: 'pending',
                attempt_count: 0,
                last_failure: null,
                created_at: '2026-08-02T00:00:00Z',
                updated_at: '2026-08-02T00:00:00Z',
                completed_at: null,
            },
        ];
        const controller = createController();
        const restart = vi
            .spyOn(controller, 'restartProviderDiscoveryAssistantAfterInterruption')
            .mockResolvedValue();
        const resume = vi
            .spyOn(controller, 'continueProviderDiscoveryCompensation')
            .mockResolvedValue();
        render(DiscoveryPanel, { appState, controller });

        await fireEvent.click(
            screen.getByRole('button', {
                name: '도우미 중단 지점에서 명시적 재시작',
            }),
        );
        expect(restart).toHaveBeenCalledOnce();
        await fireEvent.click(screen.getByRole('button', { name: '보상 작업 재개' }));
        expect(resume).toHaveBeenCalledWith(true);
        controller.destroy();

        cleanup();
        const runState = providerState();
        const runBoundary = {
            checkpoint: 'ready',
            action: 'run_assistant' as const,
            questions: [],
            draft_review: null,
        };
        const runSession = discoverySession({
            state: 'building_assistant_manifest_draft',
            action_required: null,
            assistant_resume_boundary: runBoundary,
        });
        runState.providers.workspace.discoveries = [runSession];
        runState.providers.workspace.selected_discovery_id = runSession.id;
        runState.providers.workspace.discovery_assistant_resume_boundary = runBoundary;
        const runController = createController();
        const runAssistant = vi.spyOn(runController, 'runProviderDiscoveryAssistant');
        render(DiscoveryPanel, { appState: runState, controller: runController });

        expect(screen.getByText(/원격 설정 도우미는 Rust가 정확한 요청을/)).toBeInTheDocument();
        expect(screen.queryByRole('button', { name: /도우미 실행/ })).not.toBeInTheDocument();
        expect(screen.queryByLabelText(/예상 입력 토큰/)).not.toBeInTheDocument();
        expect(screen.queryByLabelText(/최대 비용/)).not.toBeInTheDocument();
        expect(runAssistant).not.toHaveBeenCalled();
        runController.destroy();
    });
});

describe('model sync workflow', () => {
    it('applies the reviewed digest and exposes explicit cancellation', async () => {
        const appState = providerState();
        const job = {
            id: 'sync-1',
            connection_id: 'connection-1',
            state: 'diff-ready-awaiting-review',
            revision: 4,
            review: {
                sha256: 'review-digest',
                diff: {
                    newly_seen_model_route_ids: ['route-new'],
                    missing_model_route_ids: [],
                    initial_presets: [],
                    routes_requiring_preset_configuration: ['route-new'],
                    provenance: {
                        source: 'provider_api',
                        endpoint_path: '/models',
                        pages_fetched: 1,
                    },
                },
            },
            failure: null,
            created_at: '2026-08-02T00:00:00Z',
            updated_at: '2026-08-02T00:00:01Z',
        } as unknown as ModelSyncJobDto;
        appState.providers.workspace.model_sync_jobs = [job];
        appState.providers.workspace.selected_model_sync_job_id = job.id;
        const controller = createController();
        const approve = vi.spyOn(controller, 'approveProviderModelSync').mockResolvedValue();
        const cancel = vi.spyOn(controller, 'cancelProviderModelSync').mockResolvedValue();
        render(ModelSyncPanel, { appState, controller });

        await fireEvent.click(screen.getByRole('button', { name: '검토한 정확한 diff 적용' }));
        expect(approve).toHaveBeenCalledWith('sync-1');
        await fireEvent.click(screen.getByRole('button', { name: '동기화 취소' }));
        expect(cancel).toHaveBeenCalledWith('sync-1');
        controller.destroy();
    });
});

describe('signed catalog workflow', () => {
    it('supports exact import apply/discard and explicit rollback review/apply', async () => {
        const appState = providerState();
        const emptyDiff = {
            diff_schema_version: 1,
            from_revision: 3,
            to_revision: 4,
            manifest_changes: [],
            model_changes: [],
        };
        const importTicket = {
            ticket_id: 'ticket-1',
            plan: {
                review: {
                    plan_schema_version: 1,
                    action_id: 'import-action',
                    expected_state_version: 2,
                    expected_active_revision: 3,
                    expected_active_snapshot_sha256: 'active-sha',
                    expected_highest_accepted_revision: 3,
                    envelope_byte_count: 100,
                    envelope_sha256: 'envelope-sha',
                    signing_key_id: 'key-1',
                    payload_sha256: 'payload-sha',
                    signed_catalog_revision: 4,
                    candidate_revision: 4,
                    candidate_snapshot_sha256: 'candidate-sha',
                    prepared_at: '2026-08-02T00:00:00Z',
                    expires_at: '2026-08-02T01:00:00Z',
                    diff: emptyDiff,
                },
                plan_sha256: 'import-plan-sha',
            },
        } satisfies ProviderCatalogImportTicketDto;
        const rollbackPlan = {
            plan_schema_version: 1,
            action_id: 'rollback-action',
            expected_state_version: 2,
            plan_sha256: 'rollback-plan-sha',
            catalog_plan: {
                rollback_plan_version: 1,
                from_revision: 3,
                to_revision: 2,
                expected_active_sha256: 'active-sha',
                target_sha256: 'target-sha',
                created_at: '2026-08-02T00:00:00Z',
                expires_at: '2026-08-02T01:00:00Z',
                diff: { ...emptyDiff, from_revision: 3, to_revision: 2 },
            },
        } satisfies ProviderCatalogRollbackPlanDto;
        appState.providers.workspace.catalog_status = {
            status_schema_version: 1,
            state_version: 2,
            active_revision: 3,
            active_snapshot_sha256: 'active-sha',
            bundled_baseline_sha256: 'baseline-sha',
            snapshot_count: 3,
            signed_update_count: 2,
            highest_accepted_revision: 4,
            latest_issued_at: '2026-08-02T00:00:00Z',
            active_signed_revisions: [3],
        };
        appState.providers.workspace.catalog_history = {
            history_schema_version: 1,
            active_revision: 3,
            revisions: [
                {
                    revision: 2,
                    captured_at: '2026-08-01T00:00:00Z',
                    snapshot_sha256: 'revision-2-sha',
                    signed_revisions: [2],
                    active: false,
                },
            ],
            activations: [],
            next_before_revision: null,
            next_before_state_version: null,
        };
        appState.providers.workspace.pending_catalog_import = importTicket;
        appState.providers.workspace.pending_catalog_rollback = rollbackPlan;
        const controller = createController();
        const activateImport = vi
            .spyOn(controller, 'activateProviderCatalogImport')
            .mockResolvedValue();
        const discardImport = vi
            .spyOn(controller, 'discardProviderCatalogImport')
            .mockResolvedValue();
        const prepareRollback = vi
            .spyOn(controller, 'prepareProviderCatalogRollback')
            .mockResolvedValue();
        const activateRollback = vi
            .spyOn(controller, 'activateProviderCatalogRollback')
            .mockResolvedValue();
        render(CatalogPanel, { appState, controller });

        await fireEvent.click(
            screen.getByRole('button', {
                name: '검토한 정확한 가져오기 계획 적용',
            }),
        );
        expect(activateImport).toHaveBeenCalledOnce();
        await waitFor(() =>
            expect(screen.getByRole('button', { name: '가져오기 계획 폐기' })).toBeEnabled(),
        );
        await fireEvent.click(screen.getByRole('button', { name: '가져오기 계획 폐기' }));
        expect(discardImport).toHaveBeenCalledOnce();
        await waitFor(() =>
            expect(screen.getByRole('button', { name: '이 리비전으로 롤백 준비' })).toBeEnabled(),
        );
        await fireEvent.click(screen.getByRole('button', { name: '이 리비전으로 롤백 준비' }));
        expect(prepareRollback).toHaveBeenCalledWith(2);
        await waitFor(() =>
            expect(
                screen.getByRole('button', {
                    name: '검토한 정확한 롤백 계획 적용',
                }),
            ).toBeEnabled(),
        );
        await fireEvent.click(
            screen.getByRole('button', {
                name: '검토한 정확한 롤백 계획 적용',
            }),
        );
        expect(activateRollback).toHaveBeenCalledWith(rollbackPlan);
        controller.destroy();
    });
});
