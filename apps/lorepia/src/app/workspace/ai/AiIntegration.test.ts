import { cleanup, fireEvent, render, screen } from '@testing-library/svelte';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import {
    INITIAL_APP_STATE,
    LorepiaAppController,
    type LorepiaAppState,
} from '../../app-controller';
import { t } from '../../../lib/i18n';
import type {
    LorepiaClient,
    ModelSyncJobDto,
    ProviderConnectionDto,
} from '../../../lib/ipc/contracts';
import type { ChoiceRequest } from '../../../ui/workspace/settings-choice';
import WorkspaceAiSettings from './WorkspaceAiSettings.svelte';
import AiSync from './AiSync.svelte';
import AiDiscovery from './AiDiscovery.svelte';
import AiReview from './AiReview.svelte';
const mocks = vi.hoisted(() => ({ choice: vi.fn() }));
beforeEach(() => {
    vi.spyOn(LorepiaAppController.prototype, 'loadProviderDiagnostics').mockResolvedValue(null);
});
vi.mock('../../../ui/workspace/choice-sheet.svelte', () => ({
    useChoiceSheet: () => ({ open: mocks.choice }),
}));
afterEach(() => {
    cleanup();
    vi.restoreAllMocks();
    mocks.choice.mockReset();
});

function state(): LorepiaAppState {
    return {
        ...structuredClone(INITIAL_APP_STATE),
        providers: { ...structuredClone(INITIAL_APP_STATE.providers), phase: 'ready' },
    };
}

it.each([
    ['settings.page.discovery.provider', 'discovery'],
    ['settings.section.catalog.title', 'catalog'],
    ['settings.page.discovery.sync_job', 'sync'],
] as const)(
    'opens %s from AI settings and loads its controller-owned data',
    async (label, section) => {
        const controller = new LorepiaAppController({} as LorepiaClient);
        const load = vi.spyOn(controller, 'loadProviderDiagnostics').mockResolvedValue(null);
        render(WorkspaceAiSettings, { appState: state(), controller, onclose: vi.fn() });
        await fireEvent.click(screen.getByRole('button', { name: t(label) }));
        expect(screen.getByRole('heading', { name: t(label) })).toBeInTheDocument();
        expect(load).toHaveBeenCalledWith(section);
    },
);

const oldJob: ModelSyncJobDto = {
    id: 'previous-job',
    connection_id: 'connection',
    state: 'diff-ready-awaiting-review',
    revision: 1,
    review: null,
    failure: null,
    created_at: '2026-09-08T00:00:00Z',
    updated_at: '2026-09-08T00:00:00Z',
};
function syncState() {
    const appState = state();
    appState.providers.workspace.connections = [
        { id: 'connection', display_name: 'Connection' } as ProviderConnectionDto,
    ];
    appState.providers.workspace.model_sync_jobs = [oldJob];
    appState.providers.workspace.selected_model_sync_job_id = oldJob.id;
    return appState;
}
async function selectConnection() {
    await fireEvent.click(screen.getByRole('button', { name: t('model_sync.connection') }));
    const request = mocks.choice.mock.calls.at(-1)?.[0] as ChoiceRequest;
    request.onselect('connection');
}

it('does not open an earlier sync review when starting a new sync fails', async () => {
    const controller = new LorepiaAppController({} as LorepiaClient);
    const start = vi.spyOn(controller, 'startProviderModelSync').mockResolvedValue();
    render(AiSync, { appState: syncState(), controller, onclose: vi.fn() });
    await selectConnection();
    await fireEvent.click(
        screen.getByRole('button', { name: t('settings.page.discovery.sync_create') }),
    );
    expect(start).toHaveBeenCalledWith('connection');
    expect(
        screen.getByRole('button', { name: t('settings.page.discovery.sync_create') }),
    ).toBeInTheDocument();
    expect(screen.queryByRole('button', { name: t('uiPreview.cancel') })).not.toBeInTheDocument();
});

it('opens only the new sync job delivered by the controller after start', async () => {
    const controller = new LorepiaAppController({} as LorepiaClient);
    const appState = syncState();
    const onclose = vi.fn();
    const view = render(AiSync, { appState, controller, onclose });
    const newJob = { ...oldJob, id: 'new-job', state: 'queued' };
    vi.spyOn(controller, 'startProviderModelSync').mockImplementation(async () => {
        await view.rerender({
            appState: {
                ...appState,
                providers: {
                    ...appState.providers,
                    workspace: {
                        ...appState.providers.workspace,
                        model_sync_jobs: [oldJob, newJob],
                        selected_model_sync_job_id: newJob.id,
                    },
                },
            },
            controller,
            onclose,
        });
    });
    const cancel = vi.spyOn(controller, 'cancelProviderModelSync').mockResolvedValue();
    await selectConnection();
    await fireEvent.click(
        screen.getByRole('button', { name: t('settings.page.discovery.sync_create') }),
    );
    await fireEvent.click(await screen.findByRole('button', { name: t('uiPreview.cancel') }));
    expect(cancel).toHaveBeenCalledWith('new-job');
});

it('keeps all consent values and future fields in flat review rows', () => {
    const { container } = render(AiReview, {
        label: 'Consent',
        value: {
            allowed_document_origins: ['https://docs.example'],
            max_calls: 2,
            max_cost_micro_units: 0,
            budget: { max_requests: 3, max_total_tokens_per_request: 512 },
            before: null,
            after: {},
            evidence_ids: [],
            future_consent_scope: 'PRESERVE THIS VALUE',
        },
    });
    expect(screen.getByText('https://docs.example')).toBeInTheDocument();
    expect(screen.getByText('PRESERVE THIS VALUE')).toBeInTheDocument();
    expect(screen.getByText('0')).toBeInTheDocument();
    expect(screen.getByText('512')).toBeInTheDocument();
    expect(screen.getAllByText(t('workspaceReview.empty'))).toHaveLength(3);
    expect(container.querySelectorAll('dd')).toHaveLength(9);
    expect(container.querySelector('details')).toBeNull();
    expect(screen.queryByText('max_total_tokens_per_request')).not.toBeInTheDocument();
});

it('retains sync selection beneath the job and returns to the same selection panel', async () => {
    const controller = new LorepiaAppController({} as LorepiaClient);
    vi.spyOn(controller, 'refreshProviderModelSync').mockResolvedValue();
    const onclose = vi.fn();
    render(AiSync, { appState: syncState(), controller, onclose });
    const selection = screen.getByRole('dialog');
    await fireEvent.click(screen.getByRole('button', { name: oldJob.id + ' · ' + oldJob.state }));
    expect(screen.getByRole('dialog')).not.toBe(selection);
    expect(selection).toHaveAttribute('aria-hidden', 'true');
    await fireEvent.click(screen.getByRole('button', { name: t('uiPreview.back') }));
    expect(screen.getByRole('dialog')).toBe(selection);
    expect(onclose).not.toHaveBeenCalled();
});

it('retains discovery history beneath creation and returns to it without closing discovery', async () => {
    const controller = new LorepiaAppController({} as LorepiaClient);
    vi.spyOn(controller, 'loadProviderDiagnostics').mockResolvedValue(null);
    const onclose = vi.fn();
    render(AiDiscovery, { appState: state(), controller, onclose });
    const history = screen.getByRole('dialog');
    await fireEvent.click(screen.getByRole('button', { name: t('workspaceAi.text105') }));
    expect(screen.getByRole('dialog')).not.toBe(history);
    expect(history).toHaveAttribute('aria-hidden', 'true');
    await fireEvent.click(screen.getByRole('button', { name: t('uiPreview.back') }));
    expect(screen.getByRole('dialog')).toBe(history);
    expect(onclose).not.toHaveBeenCalled();
});
