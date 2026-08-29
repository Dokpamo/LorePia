import type {
    AppSettingsDto,
    BootstrapDto,
    CharacterDto,
    CharacterGreetingCatalogDto,
    ConversationBranchDto,
    ConversationDto,
    ConversationStateDto,
    ImportInspectionDto,
    InterruptedMemoryJobDto,
    LoadingPhase,
    MemoryQueryEmbeddingRetryCandidateDto,
    MemorySupervisorStatusDto,
    MessageDto,
    ProviderWorkspaceDto,
} from '../lib/ipc/contracts';

export interface SectionState {
    phase: LoadingPhase;
    error: string | null;
}

export interface ImportFlowState extends SectionState {
    inspection: ImportInspectionDto | null;
}

export interface ChatState extends SectionState {
    active_generation_id: string | null;
    live_assistant_message_id: string | null;
    streaming_text: string;
    reasoning_text: string;
    reconcile_notice: string | null;
    usage_label: string | null;
}

export interface GreetingCatalogState extends SectionState {
    value: CharacterGreetingCatalogDto | null;
    selected_greeting_id: string | null;
}

export interface MemoryQueryRetryState extends SectionState {
    candidates: MemoryQueryEmbeddingRetryCandidateDto[];
    interrupted_jobs: InterruptedMemoryJobDto[];
    busy_id: string | null;
    notice: string | null;
}

export interface LorepiaAppState {
    bootstrap: SectionState & { value: BootstrapDto | null };
    memory_supervisor: SectionState & { status: MemorySupervisorStatusDto | null };
    library: SectionState & { characters: CharacterDto[] };
    import_flow: ImportFlowState;
    selected_character: CharacterDto | null;
    conversations: SectionState & { items: ConversationDto[] };
    greeting_catalog: GreetingCatalogState;
    selected_conversation: ConversationDto | null;
    conversation_state: ConversationStateDto | null;
    branches: ConversationBranchDto[];
    messages: SectionState & { items: MessageDto[] };
    memory_query_retries: MemoryQueryRetryState;
    chat: ChatState;
    providers: SectionState & { workspace: ProviderWorkspaceDto };
    announcement: string;
}

const EMPTY_SETTINGS: AppSettingsDto = {
    preserve_partial_generations: true,
    selected_provider_profile_id: null,
    selected_model_route_id: null,
    selected_generation_preset_id: null,
};

const EMPTY_PROVIDER_WORKSPACE: ProviderWorkspaceDto = {
    templates: [],
    connections: [],
    legacy_profiles: [],
    routes: [],
    presets: [],
    settings: EMPTY_SETTINGS,
    credential_statuses: {},
    request_preview: null,
    selected_capability_model_route_id: null,
    capability_observations: [],
    capability_parameter_specs: [],
    effective_capability: null,
    model_sync_jobs: [],
    selected_model_sync_job_id: null,
    model_sync_event: null,
    discoveries: [],
    selected_discovery_id: null,
    discovery_candidates: [],
    discovery_evidence: [],
    discovery_approvals: [],
    discovery_review: null,
    discovery_approval_proposal: null,
    discovery_review_proposal: null,
    discovery_assistant_resume_boundary: null,
    discovery_assistant_host_action: null,
    discovery_event: null,
    discovery_compensation_steps: [],
    discovery_recovery_results: [],
    catalog_status: null,
    catalog_history: null,
    pending_catalog_import: null,
    pending_catalog_rollback: null,
    catalog_diff: null,
};

export const INITIAL_APP_STATE: LorepiaAppState = {
    bootstrap: { phase: 'idle', error: null, value: null },
    memory_supervisor: { phase: 'idle', error: null, status: null },
    library: { phase: 'idle', error: null, characters: [] },
    import_flow: { phase: 'idle', error: null, inspection: null },
    selected_character: null,
    conversations: { phase: 'idle', error: null, items: [] },
    greeting_catalog: {
        phase: 'idle',
        error: null,
        value: null,
        selected_greeting_id: null,
    },
    selected_conversation: null,
    conversation_state: null,
    branches: [],
    messages: { phase: 'idle', error: null, items: [] },
    memory_query_retries: {
        phase: 'idle',
        error: null,
        candidates: [],
        interrupted_jobs: [],
        busy_id: null,
        notice: null,
    },
    chat: {
        phase: 'idle',
        error: null,
        active_generation_id: null,
        live_assistant_message_id: null,
        streaming_text: '',
        reasoning_text: '',
        reconcile_notice: null,
        usage_label: null,
    },
    providers: {
        phase: 'idle',
        error: null,
        workspace: EMPTY_PROVIDER_WORKSPACE,
    },
    announcement: '',
};
