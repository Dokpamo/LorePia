import { cleanup, fireEvent, render, screen, waitFor, within } from '@testing-library/svelte';
import { afterEach, describe, expect, it, vi } from 'vitest';

import {
    INITIAL_APP_STATE,
    LorepiaAppController,
    type LorepiaAppState,
} from '../../app/app-controller';
import type { LorepiaClient } from '../../lib/ipc/contracts';
import ProviderSettings from './ProviderSettings.svelte';

afterEach(() => {
    cleanup();
    vi.restoreAllMocks();
});

function legacyProviderState(): LorepiaAppState {
    const state = structuredClone(INITIAL_APP_STATE);
    state.providers.phase = 'ready';
    state.providers.workspace.legacy_profiles = [
        {
            id: 'legacy-profile-1',
            display_name: '보존된 레거시 프로필',
            base_url: 'https://synthetic.invalid/v1',
            model: 'synthetic-model',
            timeout_seconds: 30,
        },
    ];
    state.providers.workspace.credential_statuses['legacy_profile:legacy-profile-1'] = 'available';
    state.providers.workspace.settings.selected_provider_profile_id = null;
    state.providers.workspace.settings.selected_model_route_id = null;
    state.providers.workspace.settings.selected_generation_preset_id = null;
    return state;
}

function deferred<T>(): { promise: Promise<T>; resolve: (value: T) => void } {
    let resolve!: (value: T) => void;
    const promise = new Promise<T>((nextResolve) => {
        resolve = nextResolve;
    });
    return { promise, resolve };
}

describe('ProviderSettings retained legacy profiles', () => {
    it('does not expose retained legacy alias routes as ordinary generation targets', () => {
        const appState = legacyProviderState();
        appState.providers.workspace.routes = [
            {
                id: 'legacy-route-v2',
                connection_id: 'legacy-profile-1',
                api_family: 'open_ai_chat_completions',
                model_id: 'synthetic-model',
                display_name: 'Legacy alias model',
                route_config: {
                    deployment_id: null,
                    region: null,
                    endpoint_path: null,
                    values: [],
                },
                status: 'available',
                miss_count: 0,
                metadata_source: 'legacy',
                metadata_observed_at: null,
                first_seen_at: '2026-08-02T00:00:00Z',
                last_seen_at: null,
            },
            {
                id: 'ordinary-route',
                connection_id: 'ordinary-connection',
                api_family: 'open_ai_responses',
                model_id: 'ordinary-model',
                display_name: 'Ordinary model',
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
            },
        ];
        const controller = new LorepiaAppController({} as LorepiaClient);

        render(ProviderSettings, { appState, controller });

        const heading = screen.getByRole('heading', { name: '저장된 기본 생성 대상' });
        const targetSection = heading.closest('section');
        if (targetSection === null) throw new Error('generation target section is missing');
        const routeSelect = within(targetSection).getByLabelText('모델 라우트');
        expect(
            within(routeSelect).queryByRole('option', { name: 'Legacy alias model' }),
        ).not.toBeInTheDocument();
        expect(
            within(routeSelect).getByRole('option', { name: 'Ordinary model' }),
        ).toBeInTheDocument();
        controller.destroy();
    });

    it('lets an unselected retained legacy profile become the default target again', async () => {
        const controller = new LorepiaAppController({} as LorepiaClient);
        const selectLegacyProviderProfile = vi.fn().mockResolvedValue(true);
        Object.assign(controller, { selectLegacyProviderProfile });

        render(ProviderSettings, {
            appState: legacyProviderState(),
            controller,
        });

        const profileHeading = screen.getByRole('heading', {
            name: '보존된 레거시 프로필',
        });
        const profileCard = profileHeading.closest('article');
        if (profileCard === null) {
            throw new Error('legacy provider profile card was not rendered');
        }

        await fireEvent.click(
            within(profileCard).getByRole('button', { name: '기본 대상으로 선택' }),
        );

        expect(selectLegacyProviderProfile).toHaveBeenCalledOnce();
        expect(selectLegacyProviderProfile).toHaveBeenCalledWith('legacy-profile-1');
    });

    it('disables every settings mutation while a retained legacy selection is pending', async () => {
        const controller = new LorepiaAppController({} as LorepiaClient);
        const pendingSelection = deferred<boolean>();
        vi.spyOn(controller, 'selectLegacyProviderProfile').mockReturnValue(
            pendingSelection.promise,
        );

        render(ProviderSettings, {
            appState: legacyProviderState(),
            controller,
        });

        await fireEvent.click(screen.getByRole('button', { name: '기본 대상으로 선택' }));

        expect(screen.getByLabelText('취소·오류 시 생성된 일부 응답을 보존')).toBeDisabled();
        expect(screen.getByRole('button', { name: '기본 대상 해제' })).toBeDisabled();
        const targetSection = screen
            .getByRole('heading', { name: '저장된 기본 생성 대상' })
            .closest('section');
        if (targetSection === null) throw new Error('generation target section is missing');
        expect(within(targetSection).getByLabelText('모델 라우트')).toBeDisabled();

        pendingSelection.resolve(true);
        await waitFor(() =>
            expect(screen.getByLabelText('취소·오류 시 생성된 일부 응답을 보존')).toBeEnabled(),
        );
        controller.destroy();
    });

    it('summarizes a normalized retained legacy selection as legacy', () => {
        const appState = legacyProviderState();
        appState.providers.workspace.settings.selected_provider_profile_id = 'legacy-profile-1';
        appState.providers.workspace.settings.selected_model_route_id = 'legacy-profile-1';
        appState.providers.workspace.settings.selected_generation_preset_id = 'legacy-profile-1';
        const controller = new LorepiaAppController({} as LorepiaClient);

        render(ProviderSettings, { appState, controller });

        const summary = screen
            .getByRole('heading', { name: '저장된 기본 생성 대상' })
            .closest('section');
        if (summary === null) throw new Error('default generation target summary was not rendered');
        expect(
            within(summary).getByText('기존 프로바이더 프로필을 기본 대상으로 사용 중입니다.'),
        ).toBeInTheDocument();
        controller.destroy();
    });

    it('shows only the retained legacy credential actions for a dual-written same-ID connection', async () => {
        const appState = legacyProviderState();
        appState.providers.workspace.settings.selected_provider_profile_id = 'legacy-profile-1';
        appState.providers.workspace.settings.selected_model_route_id = 'legacy-profile-1';
        appState.providers.workspace.settings.selected_generation_preset_id = 'legacy-profile-1';
        appState.providers.workspace.connections = [
            {
                id: 'legacy-profile-1',
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
            },
        ];
        appState.providers.workspace.credential_statuses['connection:legacy-profile-1'] =
            'available';
        const controller = new LorepiaAppController({} as LorepiaClient);
        const captureProviderCredential = vi
            .spyOn(controller, 'captureProviderCredential')
            .mockResolvedValue(true);
        const deleteProviderCredential = vi
            .spyOn(controller, 'deleteProviderCredential')
            .mockResolvedValue();

        render(ProviderSettings, { appState, controller });

        expect(screen.queryByLabelText('정규화된 레거시 연결 자격증명')).not.toBeInTheDocument();
        const ordinaryConnectionCard = screen
            .getByRole('heading', { name: '정규화된 레거시 연결' })
            .closest('article');
        if (ordinaryConnectionCard === null)
            throw new Error('ordinary connection card was not rendered');
        expect(within(ordinaryConnectionCard).queryByText('자격증명 없음')).not.toBeInTheDocument();
        const legacyCredentialActions = screen.getByLabelText('보존된 레거시 프로필 자격증명');
        await fireEvent.click(
            within(legacyCredentialActions).getByRole('button', {
                name: '클립보드에서 안전하게 캡처',
            }),
        );
        await fireEvent.click(
            within(legacyCredentialActions).getByRole('button', { name: '삭제' }),
        );

        expect(captureProviderCredential).toHaveBeenCalledWith({
            kind: 'legacy_profile',
            provider_profile_id: 'legacy-profile-1',
        });
        expect(deleteProviderCredential).toHaveBeenCalledWith({
            kind: 'legacy_profile',
            provider_profile_id: 'legacy-profile-1',
        });
        const summary = screen
            .getByRole('heading', { name: '저장된 기본 생성 대상' })
            .closest('section');
        if (summary === null) throw new Error('default generation target summary was not rendered');
        expect(
            within(summary).getByText('기존 프로바이더 프로필을 기본 대상으로 사용 중입니다.'),
        ).toBeInTheDocument();
        controller.destroy();
    });
});
