import { cleanup, fireEvent, render, screen, waitFor, within } from '@testing-library/svelte';
import { afterEach, describe, expect, it, vi } from 'vitest';

import {
    INITIAL_APP_STATE,
    LorepiaAppController,
    type LorepiaAppState,
} from '../../app/app-controller';
import type {
    GenerationPresetDto,
    LorepiaClient,
    ModelRouteDto,
    ProviderConnectionDto,
    ProviderTemplateDto,
} from '../../lib/ipc/contracts';
import CapabilityPanel from './CapabilityPanel.svelte';
import ProviderCrudPanel from './ProviderCrudPanel.svelte';

afterEach(() => {
    cleanup();
    vi.restoreAllMocks();
});

async function chooseSetting(label: string, option: string): Promise<void> {
    await fireEvent.click(screen.getByLabelText(label));
    await fireEvent.click(await screen.findByRole('option', { name: option }));
}

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

const ROUTE: ModelRouteDto = {
    id: 'route-1',
    connection_id: CONNECTION.id,
    api_family: 'open_ai_responses',
    model_id: 'model-1',
    display_name: 'Synthetic model',
    route_config: {
        deployment_id: null,
        region: null,
        endpoint_path: null,
        values: [],
    },
    status: 'available',
    miss_count: 0,
    metadata_source: 'manual',
    metadata_observed_at: null,
    first_seen_at: '2026-08-02T00:00:00Z',
    last_seen_at: null,
};

const LEGACY_PROFILE_ID = 'legacy-profile-1';
const LEGACY_ROUTE: ModelRouteDto = {
    ...ROUTE,
    id: 'legacy-route-v2',
    connection_id: LEGACY_PROFILE_ID,
    api_family: 'open_ai_chat_completions',
    model_id: 'legacy-model-v2',
    display_name: 'Legacy model v2',
    metadata_source: 'legacy',
};
const LEGACY_PRESET: GenerationPresetDto = {
    id: LEGACY_ROUTE.id,
    model_route_id: LEGACY_ROUTE.id,
    display_name: 'Legacy default',
    values: [],
    reasoning: {
        mode: 'disabled',
        effort: null,
        budget_tokens: null,
        summary: 'none',
        preserve_opaque_state: false,
    },
    prompt_cache: {
        mode: 'disabled',
        ttl_kind: 'provider_default',
        ttl_seconds: null,
        context_reference: null,
    },
    created_at: '2026-08-02T00:00:00Z',
    updated_at: '2026-08-02T00:00:00Z',
};

function configuredState(): LorepiaAppState {
    const state = structuredClone(INITIAL_APP_STATE);
    state.providers.phase = 'ready';
    state.providers.workspace.templates = [TEMPLATE];
    state.providers.workspace.connections = [CONNECTION];
    state.providers.workspace.routes = [ROUTE];
    return state;
}

describe('direct provider configuration', () => {
    it('creates connection metadata without WebView credential ingress and gates deletion', async () => {
        const appState = configuredState();
        const controller = new LorepiaAppController({} as LorepiaClient);
        const create = vi.spyOn(controller, 'createProviderConnection').mockResolvedValue(true);
        const remove = vi.spyOn(controller, 'deleteProviderConnection').mockResolvedValue(true);
        render(ProviderCrudPanel, { appState, controller });

        await fireEvent.click(screen.getByRole('button', { name: '프로바이더 연결 1개 연결' }));
        expect(screen.getByRole('toolbar', { name: '프로바이더 연결 작업' })).toBeInTheDocument();
        await fireEvent.click(screen.getByRole('button', { name: '연결 추가하기' }));
        const createForm = screen.getByRole('form', { name: '프로바이더 연결 만들기' });
        await chooseSetting('템플릿', 'Synthetic API · v2');
        await fireEvent.input(within(createForm).getByLabelText('연결 ID'), {
            target: { value: 'connection-new' },
        });
        await fireEvent.input(within(createForm).getByLabelText('표시 이름'), {
            target: { value: '새 연결' },
        });
        expect(createForm.querySelector('input[type="password"]')).toBeNull();
        expect(createForm).toHaveTextContent(
            '자격증명은 이 화면이나 WebView 메모리에 들어오지 않습니다.',
        );
        await fireEvent.click(screen.getByRole('button', { name: '연결 만들기' }));

        await waitFor(() => {
            expect(create).toHaveBeenCalledWith(
                expect.objectContaining({
                    id: 'connection-new',
                    template_id: TEMPLATE.id,
                    api_origin: 'https://api.example',
                }),
            );
        });

        await fireEvent.click(
            screen.getByRole('button', {
                name: 'Synthetic connection https://api.example',
            }),
        );
        expect(
            screen.getByRole('toolbar', { name: '프로바이더 연결 편집 작업' }),
        ).toBeInTheDocument();
        expect(
            screen.getByRole('form', {
                name: '프로바이더 연결 수정 또는 삭제',
            }),
        ).toBeInTheDocument();
        await fireEvent.click(screen.getByRole('button', { name: '삭제' }));
        expect(remove).not.toHaveBeenCalled();
        expect(screen.getByRole('button', { name: '삭제 확인' })).toBeEnabled();
        await fireEvent.click(screen.getByRole('button', { name: '삭제 확인' }));
        expect(remove).toHaveBeenCalledWith(CONNECTION.id);
        controller.destroy();
    });

    it('builds one preset candidate for save, validation and redacted preview controls', async () => {
        const appState = configuredState();
        const controller = new LorepiaAppController({} as LorepiaClient);
        const save = vi.spyOn(controller, 'upsertProviderGenerationPreset').mockResolvedValue(true);
        const validate = vi
            .spyOn(controller, 'validateProviderGenerationPresetCandidate')
            .mockResolvedValue(true);
        const preview = vi.spyOn(controller, 'previewProviderRequestCandidate').mockResolvedValue();
        render(ProviderCrudPanel, { appState, controller });

        await fireEvent.click(screen.getByRole('button', { name: '생성 프리셋 0개 프리셋' }));
        expect(screen.getByRole('toolbar', { name: '생성 프리셋 작업' })).toBeInTheDocument();
        await fireEvent.click(screen.getByRole('button', { name: '프리셋 추가하기' }));
        const form = screen.getByRole('form', { name: '생성 프리셋 만들기 또는 수정' });
        await chooseSetting('모델 라우트', 'Synthetic model');
        await fireEvent.input(within(form).getByLabelText('프리셋 ID'), {
            target: { value: 'preset-new' },
        });
        await fireEvent.input(within(form).getByLabelText('표시 이름'), {
            target: { value: 'Creative' },
        });
        await fireEvent.input(within(form).getByLabelText(/파라미터 JSON/u), {
            target: {
                value: JSON.stringify([
                    {
                        parameter_id: 'temperature',
                        state: {
                            state: 'explicit',
                            value: { type: 'number', value: 0.7 },
                        },
                    },
                ]),
            },
        });
        const reasoning = within(form).getByRole('group', { name: 'Reasoning' });
        await fireEvent.input(within(reasoning).getByLabelText('Mode'), {
            target: { value: 'effort' },
        });
        await fireEvent.input(within(reasoning).getByLabelText('Effort (선택)'), {
            target: { value: 'medium' },
        });
        const cache = within(form).getByRole('group', { name: 'Prompt cache' });
        await fireEvent.input(within(cache).getByLabelText('Mode'), {
            target: { value: 'automatic' },
        });
        await fireEvent.input(within(cache).getByLabelText('TTL kind'), {
            target: { value: 'short' },
        });

        await fireEvent.click(within(form).getByRole('button', { name: '후보 검증' }));
        await waitFor(() => expect(validate).toHaveBeenCalledOnce());
        const candidate = validate.mock.calls[0]?.[0];
        expect(candidate).toMatchObject({
            id: 'preset-new',
            model_route_id: ROUTE.id,
            display_name: 'Creative',
            reasoning: { mode: 'effort', effort: 'medium' },
            prompt_cache: { mode: 'automatic', ttl_kind: 'short' },
        });

        await fireEvent.click(within(form).getByRole('button', { name: '요청 구조 미리보기' }));
        await waitFor(() => expect(preview).toHaveBeenCalledWith(candidate));
        await fireEvent.click(screen.getByRole('button', { name: '프리셋 만들기' }));
        await waitFor(() => expect(save).toHaveBeenCalledWith(candidate));
        controller.destroy();
    });

    it('does not expose independent delete actions for the retained legacy sibling graph', async () => {
        const appState = configuredState();
        appState.providers.workspace.connections = [
            { ...CONNECTION, id: LEGACY_PROFILE_ID, display_name: 'Legacy connection' },
            CONNECTION,
        ];
        appState.providers.workspace.legacy_profiles = [
            {
                id: LEGACY_PROFILE_ID,
                display_name: 'Retained legacy profile',
                base_url: 'https://synthetic.invalid/v1',
                model: LEGACY_ROUTE.model_id,
                timeout_seconds: 30,
            },
        ];
        appState.providers.workspace.routes = [LEGACY_ROUTE, ROUTE];
        appState.providers.workspace.presets = [LEGACY_PRESET];
        appState.providers.workspace.settings.selected_provider_profile_id = null;
        appState.providers.workspace.settings.selected_model_route_id = null;
        appState.providers.workspace.settings.selected_generation_preset_id = null;
        const controller = new LorepiaAppController({} as LorepiaClient);
        const deleteRoute = vi
            .spyOn(controller, 'deleteProviderModelRoute')
            .mockResolvedValue(true);
        const deletePreset = vi
            .spyOn(controller, 'deleteProviderGenerationPreset')
            .mockResolvedValue(true);
        render(ProviderCrudPanel, { appState, controller });

        await fireEvent.click(screen.getByRole('button', { name: '모델 라우트 1개 라우트' }));
        expect(screen.queryByRole('button', { name: /Legacy model v2/u })).not.toBeInTheDocument();
        await fireEvent.click(
            screen.getByRole('button', {
                name: 'Synthetic model open_ai_responses · model-1',
            }),
        );
        expect(
            screen.getByRole('form', { name: '모델 라우트 수정 또는 삭제' }),
        ).toBeInTheDocument();
        await fireEvent.click(screen.getByRole('button', { name: '삭제' }));
        expect(deleteRoute).not.toHaveBeenCalled();
        await fireEvent.click(screen.getByRole('button', { name: '삭제 확인' }));
        expect(deleteRoute).toHaveBeenCalledWith(ROUTE.id);

        cleanup();
        render(ProviderCrudPanel, { appState, controller });
        await fireEvent.click(screen.getByRole('button', { name: '생성 프리셋 0개 프리셋' }));
        expect(screen.queryByRole('button', { name: /Legacy default/u })).not.toBeInTheDocument();
        await fireEvent.click(screen.getByRole('button', { name: '프리셋 추가하기' }));
        const presetForm = screen.getByRole('form', {
            name: '생성 프리셋 만들기 또는 수정',
        });
        expect(
            within(presetForm).queryByRole('option', { name: LEGACY_ROUTE.display_name ?? '' }),
        ).not.toBeInTheDocument();
        expect(deletePreset).not.toHaveBeenCalled();
        controller.destroy();
    });
});

describe('capability overrides', () => {
    it('restores the loaded route when a different route fails to load', async () => {
        const appState = configuredState();
        const secondRoute: ModelRouteDto = {
            ...ROUTE,
            id: 'route-2',
            model_id: 'model-2',
            display_name: 'Second model',
        };
        appState.providers.workspace.routes = [ROUTE, secondRoute];
        appState.providers.workspace.selected_capability_model_route_id = ROUTE.id;
        const controller = new LorepiaAppController({} as LorepiaClient);
        const load = vi.spyOn(controller, 'loadProviderCapabilities').mockResolvedValue();

        render(CapabilityPanel, { appState, controller });
        const routeSelect = screen.getByLabelText('모델 라우트');
        expect(routeSelect).toHaveAttribute('data-value', ROUTE.id);
        await chooseSetting('모델 라우트', 'Second model');

        expect(load).toHaveBeenCalledWith(secondRoute.id);
        await waitFor(() => expect(routeSelect).toHaveAttribute('data-value', ROUTE.id));
        controller.destroy();
    });

    it('loads effective state and only edits/deletes an explicit user override', async () => {
        const appState = configuredState();
        appState.providers.workspace.selected_capability_model_route_id = ROUTE.id;
        const override = {
            id: 'override-1',
            model_route_id: ROUTE.id,
            key: 'streaming',
            value: { type: 'boolean' as const, value: false },
            status: 'verified',
            source: 'user_override',
            confidence: 'high',
            observed_at: '2026-08-02T00:00:00Z',
            expires_at: null,
            evidence_ref: null,
        };
        appState.providers.workspace.capability_observations = [override];
        appState.providers.workspace.effective_capability = {
            selected: override,
            alternatives: [],
            evaluated_at: '2026-08-02T00:00:00Z',
            selected_is_stale: false,
            has_conflict: false,
        };
        const controller = new LorepiaAppController({} as LorepiaClient);
        const load = vi.spyOn(controller, 'loadProviderCapabilities').mockResolvedValue();
        const inspect = vi
            .spyOn(controller, 'inspectEffectiveProviderCapability')
            .mockResolvedValue();
        const save = vi
            .spyOn(controller, 'upsertProviderCapabilityOverride')
            .mockResolvedValue(true);
        const remove = vi.spyOn(controller, 'deleteProviderCapabilityOverride').mockResolvedValue();
        const rendered = render(CapabilityPanel, { appState, controller });

        expect(
            screen.queryByRole('button', { name: 'capability 새로고침' }),
        ).not.toBeInTheDocument();
        await chooseSetting('모델 라우트', '선택');
        expect(load).not.toHaveBeenCalled();
        await chooseSetting('모델 라우트', 'Synthetic model');
        expect(load).toHaveBeenCalledWith(ROUTE.id);
        await fireEvent.click(screen.getByRole('button', { name: /유효 capability 스트리밍/ }));
        expect(screen.getByRole('toolbar', { name: '유효 capability 작업' })).toBeInTheDocument();
        expect(rendered.container.querySelector('dl.effective-result')).toBeInTheDocument();
        expect(rendered.container.querySelector('.effective-result .metadata-grid')).toBeNull();
        await fireEvent.click(screen.getByRole('button', { name: '유효 값 확인' }));
        expect(inspect).toHaveBeenCalledWith('streaming');

        cleanup();
        render(CapabilityPanel, { appState, controller });
        await fireEvent.click(screen.getByRole('button', { name: '사용자 override 1개' }));
        await fireEvent.click(screen.getByRole('button', { name: '이 override 수정' }));
        expect(
            screen.getByRole('toolbar', { name: '사용자 override 편집 작업' }),
        ).toBeInTheDocument();
        await fireEvent.click(screen.getByRole('button', { name: '사용자 override 업데이트' }));
        await waitFor(() => {
            expect(save).toHaveBeenCalledWith({
                id: 'override-1',
                model_route_id: ROUTE.id,
                key: 'streaming',
                value: { type: 'boolean', value: false },
                status: 'verified',
                expires_at: null,
            });
        });
        await fireEvent.click(screen.getByRole('button', { name: '이 override 수정' }));
        await fireEvent.click(screen.getByRole('button', { name: '사용자 override 삭제' }));
        expect(remove).not.toHaveBeenCalled();
        await fireEvent.click(screen.getByRole('button', { name: '사용자 override 삭제 확인' }));
        expect(remove).toHaveBeenCalledWith('override-1');
        controller.destroy();
    });

    it('returns to the override list if the edited override disappears', async () => {
        const appState = configuredState();
        appState.providers.workspace.selected_capability_model_route_id = ROUTE.id;
        appState.providers.workspace.capability_observations = [
            {
                id: 'override-stale',
                model_route_id: ROUTE.id,
                key: 'streaming',
                value: { type: 'boolean', value: true },
                status: 'verified',
                source: 'user_override',
                confidence: 'high',
                observed_at: '2026-08-02T00:00:00Z',
                expires_at: null,
                evidence_ref: null,
            },
        ];
        const controller = new LorepiaAppController({} as LorepiaClient);
        const rendered = render(CapabilityPanel, { appState, controller });

        await fireEvent.click(screen.getByRole('button', { name: '사용자 override 1개' }));
        await fireEvent.click(screen.getByRole('button', { name: '이 override 수정' }));
        expect(
            screen.getByRole('toolbar', { name: '사용자 override 편집 작업' }),
        ).toBeInTheDocument();

        const updatedState = structuredClone(appState);
        updatedState.providers.workspace.capability_observations = [];
        await rendered.rerender({ appState: updatedState, controller });

        await waitFor(() =>
            expect(
                screen.getByRole('toolbar', { name: '사용자 override 작업' }),
            ).toBeInTheDocument(),
        );
        expect(screen.queryByRole('button', { name: '사용자 override 업데이트' })).toBeNull();
        controller.destroy();
    });
});
