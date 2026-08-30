import type {
    AppSettingsDto,
    DiscoveryOutboxEventDto,
    LorepiaClient,
    ProviderConnectionDto,
    ProviderDiscoveryEventDto,
    ProviderDiscoverySessionDto,
    ProviderProfileDto,
} from '../../lib/ipc/contracts';

export function createAppControllerProviderFixture() {
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

    return {
        deferred,
        legacyProfile,
        modernSettings,
        normalizedLegacySettings,
        normalizedLegacyConnection,
        providerClient,
        discoverySession,
        discoveryEvent,
        discoverySessionFor,
        discoveryClient,
    };
}
