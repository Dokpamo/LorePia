import { cleanup, fireEvent, render, screen } from '@testing-library/svelte';
import { afterEach, describe, expect, it, vi } from 'vitest';
import {
    INITIAL_APP_STATE,
    LorepiaAppController,
    type LorepiaAppState,
} from '../../app-controller';
import type {
    LorepiaClient,
    ProviderTemplateDto,
    ProviderConnectionDto,
} from '../../../lib/ipc/contracts';
import type { ChoiceRequest } from '../../../ui/workspace/settings-choice';
import type { TextEditRequest } from '../../../ui/workspace/text-editor.svelte';
import { t } from '../../../lib/i18n';
import AiConnections from './AiConnections.svelte';
import WorkspaceAiSettings from './WorkspaceAiSettings.svelte';
const mocks = vi.hoisted(() => ({ choice: vi.fn(), editor: vi.fn() }));
vi.mock('../../../ui/workspace/choice-sheet.svelte', () => ({
    useChoiceSheet: () => ({ open: mocks.choice }),
}));
vi.mock('../../../ui/workspace/text-editor.svelte', () => ({
    useTextEditor: () => ({ open: mocks.editor }),
}));
afterEach(() => {
    cleanup();
    vi.restoreAllMocks();
    mocks.choice.mockReset();
    mocks.editor.mockReset();
});
const TEMPLATE: ProviderTemplateDto = {
    id: 'template-1',
    display_name: 'Synthetic API',
    manifest_version: 2,
    source: 'bundled',
    api_family: 'open_ai_responses',
    connection_fields: [],
    default_network_mode: 'public',
    default_api_origin: 'https://api.example',
    credential_required: true,
    supports_model_listing: true,
    auth_binding: { kind: 'bearer_header' },
    parameters: [],
};

const CONNECTION: ProviderConnectionDto = {
    id: 'connection-1',
    template_id: TEMPLATE.id,
    template_version: TEMPLATE.manifest_version,
    display_name: 'Synthetic connection',
    api_origin: 'https://api.example',
    api_base_path: null,
    network_mode: 'public',
    local_network_approval: null,
    config_values: [],
    credential_binding_required: true,
    credential_scope: {
        allowed_origins: ['https://api.example'],
        auth_binding: { kind: 'bearer_header' },
        redirect_policy: 'same_origin',
    },
    approved_credential_origins: ['https://api.example'],
    timeout_seconds: 30,
    status: 'active',
    created_at: '2026-08-02T00:00:00Z',
    updated_at: '2026-08-02T00:00:00Z',
};

function state(): LorepiaAppState {
    return {
        ...INITIAL_APP_STATE,
        providers: {
            phase: 'ready',
            error: null,
            workspace: {
                ...INITIAL_APP_STATE.providers.workspace,
                templates: [TEMPLATE],
                connections: [CONNECTION],
            },
        },
    };
}
async function edit(label: string, value: string) {
    await fireEvent.click(screen.getByRole('button', { name: label }));
    const request = mocks.editor.mock.calls.at(-1)?.[0] as TextEditRequest;
    await request.onchange(value);
}
async function choose(label: string, value: string) {
    await fireEvent.click(screen.getByRole('button', { name: label }));
    const request = mocks.choice.mock.calls.at(-1)?.[0] as ChoiceRequest;
    request.onselect(value);
}
describe('Mockup AI connection workflows', () => {
    it('creates through fullscreen editor and choice requests, never inline inputs', async () => {
        const controller = new LorepiaAppController({} as LorepiaClient);
        const create = vi.spyOn(controller, 'createProviderConnection').mockResolvedValue(true);
        const { container } = render(AiConnections, {
            appState: state(),
            controller,
            onclose: vi.fn(),
        });
        await fireEvent.click(screen.getByRole('button', { name: t('workspaceAi.text13') }));
        expect(container.querySelector('input,select,textarea')).toBeNull();
        await choose(t('workspaceAi.text14'), TEMPLATE.id);
        await edit(t('workspaceAi.text15'), 'new-connection');
        await edit(t('workspaceAi.text16'), 'New API');
        await fireEvent.click(screen.getByRole('button', { name: t('workspaceAi.save') }));
        expect(create).toHaveBeenCalledWith(
            expect.objectContaining({
                id: 'new-connection',
                display_name: 'New API',
                template_id: TEMPLATE.id,
                api_origin: TEMPLATE.default_api_origin,
                values: [],
            }),
        );
    });
    it('captures only a native credential target and requires confirmation before deleting it', async () => {
        const controller = new LorepiaAppController({} as LorepiaClient);
        const capture = vi.spyOn(controller, 'captureProviderCredential').mockResolvedValue(true);
        const remove = vi.spyOn(controller, 'deleteProviderCredential').mockResolvedValue();
        render(AiConnections, { appState: state(), controller, onclose: vi.fn() });
        await fireEvent.click(screen.getByRole('button', { name: CONNECTION.display_name }));
        await fireEvent.click(screen.getByRole('button', { name: t('workspaceAi.text25') }));
        expect(capture).toHaveBeenCalledWith({ kind: 'connection', connection_id: CONNECTION.id });
        await fireEvent.click(screen.getByRole('button', { name: t('workspaceAi.text26') }));
        expect(remove).not.toHaveBeenCalled();
        await fireEvent.click(screen.getByRole('button', { name: t('workspaceAi.text27') }));
        expect(remove).toHaveBeenCalledWith({ kind: 'connection', connection_id: CONNECTION.id });
    });
    it('rejects credential fields supplied in connection JSON', async () => {
        const controller = new LorepiaAppController({} as LorepiaClient);
        const create = vi.spyOn(controller, 'createProviderConnection').mockResolvedValue(true);
        const appState = state();
        appState.providers.workspace.templates[0] = {
            ...TEMPLATE,
            connection_fields: [
                {
                    key: 'token',
                    label_key: 'token',
                    description_key: null,
                    value_type: 'credential',
                    required: true,
                },
            ],
        };
        render(AiConnections, { appState, controller, onclose: vi.fn() });
        await fireEvent.click(screen.getByRole('button', { name: t('workspaceAi.text13') }));
        await choose(t('workspaceAi.text14'), TEMPLATE.id);
        await edit(
            t('workspaceAi.text24'),
            '[{"key":"token","value":{"type":"text","value":"not-a-real-secret"}}]',
        );
        await fireEvent.click(screen.getByRole('button', { name: t('workspaceAi.save') }));
        expect(create).not.toHaveBeenCalled();
        expect(screen.getByRole('alert')).toHaveTextContent(t('workspaceAi.text36'));
    });
});

it('keeps the selected legacy profile when only partial-response preference changes', async () => {
    const controller = new LorepiaAppController({} as LorepiaClient);
    const target = vi.spyOn(controller, 'selectProviderGenerationTarget').mockResolvedValue(true);
    const preserve = vi.spyOn(controller, 'setPreservePartialGenerations').mockResolvedValue(true);
    const appState = state();
    appState.providers.workspace.settings = {
        ...appState.providers.workspace.settings,
        selected_provider_profile_id: 'legacy-profile',
        preserve_partial_generations: false,
    };
    render(WorkspaceAiSettings, { appState, controller, onclose: vi.fn() });
    await fireEvent.click(screen.getByRole('switch', { name: t('workspaceAi.partial') }));
    await fireEvent.click(screen.getByRole('button', { name: t('workspaceAi.save') }));
    expect(preserve).toHaveBeenCalledWith(true);
    expect(target).not.toHaveBeenCalled();
});

it('keeps an edited connection draft behind the original discard confirmation', async () => {
    const controller = new LorepiaAppController({} as LorepiaClient);
    const onclose = vi.fn();
    render(AiConnections, { appState: state(), controller, onclose });
    await fireEvent.click(screen.getByRole('button', { name: CONNECTION.display_name }));
    await edit(t('workspaceAi.text16'), 'Changed connection');
    await fireEvent.click(screen.getByRole('button', { name: t('uiPreview.back') }));
    expect(screen.getByRole('alertdialog')).toBeInTheDocument();
    expect(onclose).not.toHaveBeenCalled();
    await fireEvent.click(screen.getByRole('button', { name: t('uiPreview.keepEditing') }));
    expect(screen.getByText('Changed connection')).toBeInTheDocument();
    await fireEvent.click(screen.getByRole('button', { name: t('uiPreview.back') }));
    await fireEvent.click(screen.getByRole('button', { name: t('uiPreview.discardChanges') }));
    expect(screen.getByRole('button', { name: CONNECTION.display_name })).toBeInTheDocument();
});

it('shows backend announcements visibly inside the active connection panel', async () => {
    const controller = new LorepiaAppController({} as LorepiaClient);
    const appState = state();
    const view = render(AiConnections, { appState, controller, onclose: vi.fn() });
    await view.rerender({
        appState: { ...appState, announcement: 'The server rejected the connection settings.' },
        controller,
        onclose: vi.fn(),
    });
    expect(screen.getByRole('status')).toHaveTextContent(
        'The server rejected the connection settings.',
    );
});

it('retains the connection list beneath a separate editor and restores it on back', async () => {
    const controller = new LorepiaAppController({} as LorepiaClient);
    const onclose = vi.fn();
    render(AiConnections, { appState: state(), controller, onclose });
    const list = screen.getByRole('dialog');
    await fireEvent.click(screen.getByRole('button', { name: CONNECTION.display_name }));
    const editor = screen.getByRole('dialog');
    expect(editor).not.toBe(list);
    expect(list).toHaveAttribute('aria-hidden', 'true');
    await fireEvent.click(screen.getByRole('button', { name: t('uiPreview.back') }));
    expect(screen.getByRole('dialog')).toBe(list);
    expect(onclose).not.toHaveBeenCalled();
    await fireEvent.click(screen.getByRole('button', { name: CONNECTION.display_name }));
    expect(screen.getByRole('dialog')).not.toBe(editor);
    await fireEvent.click(screen.getByRole('button', { name: t('uiPreview.back') }));
    await fireEvent.click(screen.getByRole('button', { name: t('uiPreview.back') }));
    expect(onclose).toHaveBeenCalledOnce();
});
