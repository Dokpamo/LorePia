import { cleanup, fireEvent, render, screen, waitFor, within } from '@testing-library/svelte';
import { afterEach, describe, expect, it, vi } from 'vitest';

import {
    INITIAL_APP_STATE,
    LorepiaAppController,
    type LorepiaAppState,
} from '../../app/app-controller';
import type { LorepiaClient } from '../../lib/ipc/contracts';
import { setThemePreference } from '../../lib/theme';
import '../../styles/app.css';
import appCss from '../../styles/app.css?raw';
import ProviderSettings from './ProviderSettings.svelte';

afterEach(() => {
    cleanup();
    setThemePreference('system');
    delete document.documentElement.dataset.theme;
    document.documentElement.removeAttribute('style');
    vi.restoreAllMocks();
});

describe('ProviderSettings mobile settings language', () => {
    it('shows concise current values without the old explanatory subtitles', () => {
        const appState = structuredClone(INITIAL_APP_STATE);
        appState.providers.phase = 'ready';
        const controller = new LorepiaAppController({} as LorepiaClient);
        setThemePreference('light');

        const rendered = render(ProviderSettings, { appState, controller, section: null });
        const identity = rendered.container.querySelector<HTMLElement>('.settings-identity');
        const card = rendered.container.querySelector<HTMLElement>('.setting-list');
        if (identity === null || card === null) throw new Error('settings hierarchy is missing');

        expect(screen.getByRole('region', { name: '설정' })).toBeInTheDocument();
        expect(screen.queryByRole('heading', { name: '설정' })).not.toBeInTheDocument();
        expect(within(identity).getByRole('heading', { name: 'LorePia' })).toBeInTheDocument();
        expect(within(identity).queryByText('로컬 Core')).not.toBeInTheDocument();
        expect(
            within(identity).queryByText('설정과 자격증명은 이 기기에 보관됩니다.'),
        ).not.toBeInTheDocument();
        expect(screen.queryByRole('button', { name: '새로고침' })).not.toBeInTheDocument();
        expect(screen.getByRole('button', { name: '설정 더보기' })).toBeInTheDocument();
        expect(rendered.container.querySelector('.settings-avatar-badge')).toBeInTheDocument();
        expect(within(card).getAllByRole('button')).toHaveLength(8);
        expect(within(card).getByRole('button', { name: /검색과 동기화/ })).toBeInTheDocument();
        expect(within(card).getByRole('button', { name: /제공자 카탈로그/ })).toBeInTheDocument();
        expect(card.querySelectorAll('.setting-value')).toHaveLength(8);
        const settingIcons = [...card.querySelectorAll<HTMLElement>('.setting-icon')];
        expect(settingIcons).toHaveLength(8);
        expect(settingIcons.every((icon) => icon.querySelector('svg') !== null)).toBe(true);
        expect(card.querySelector('[data-tone]')).not.toBeInTheDocument();
        expect(within(card).getByText('라이트')).toBeInTheDocument();
        expect(card.querySelector('.setting-copy small')).not.toBeInTheDocument();
        expect(within(card).queryByText('라이트·다크·시스템')).not.toBeInTheDocument();
        expect(within(card).queryByText('대화에서 나를 어떻게 부를지')).not.toBeInTheDocument();
        expect(
            identity.compareDocumentPosition(card) & Node.DOCUMENT_POSITION_FOLLOWING,
        ).toBeTruthy();
        controller.destroy();
    });

    it('enters a dedicated value screen instead of opening a mini dialog', async () => {
        const appState = structuredClone(INITIAL_APP_STATE);
        appState.providers.phase = 'ready';
        const controller = new LorepiaAppController({} as LorepiaClient);
        const onOpenSection = vi.fn();
        const rendered = render(ProviderSettings, {
            appState,
            controller,
            section: null,
            onOpenSection,
        });

        const appearanceRow = screen.getByRole('button', { name: /화면 모드/ });
        await fireEvent.click(appearanceRow);
        expect(onOpenSection).toHaveBeenCalledWith('appearance');

        await rendered.rerender({
            appState,
            controller,
            section: 'appearance',
            onOpenSection,
        });

        expect(screen.queryByRole('dialog')).not.toBeInTheDocument();
        const screenRegion = screen.getByRole('region', { name: '화면 모드' });
        const detailScroll =
            rendered.container.querySelector<HTMLElement>('.settings-detail-scroll');
        if (detailScroll === null) throw new Error('settings detail scroll is missing');
        expect(getComputedStyle(detailScroll).alignContent).toBe('start');
        expect(screen.queryByRole('button', { name: /화면 모드 라이트/ })).not.toBeInTheDocument();
        expect(
            within(screenRegion).queryByText(/시스템을 고르면 운영체제 설정을 따라갑니다/),
        ).not.toBeInTheDocument();

        await fireEvent.click(within(screenRegion).getByRole('button', { name: '다크' }));
        await rendered.rerender({ appState, controller, section: null, onOpenSection });

        expect(screen.getByRole('button', { name: /화면 모드 다크/ })).toBeInTheDocument();
        controller.destroy();
    });

    it('keeps the settings root toolbar to the overflow action', () => {
        const appState = structuredClone(INITIAL_APP_STATE);
        appState.providers.phase = 'ready';
        const controller = new LorepiaAppController({} as LorepiaClient);

        const rendered = render(ProviderSettings, { appState, controller, section: null });

        const toolbar = screen.getByRole('toolbar', { name: '설정 도구' });
        const scrollRegion = rendered.container.querySelector('.settings-home-scroll');
        if (scrollRegion === null) throw new Error('settings home scroll is missing');
        expect(
            within(toolbar).queryByRole('button', { name: '설정 검색' }),
        ).not.toBeInTheDocument();
        expect(within(toolbar).queryByRole('searchbox')).not.toBeInTheDocument();
        expect(within(toolbar).getByRole('button', { name: '설정 더보기' })).toBeVisible();
        expect(toolbar.parentElement).toBe(scrollRegion.parentElement);
        expect(toolbar.nextElementSibling).toBe(scrollRegion);
        controller.destroy();
    });

    it('uses the overflow action for real settings shortcuts', async () => {
        const appState = structuredClone(INITIAL_APP_STATE);
        appState.providers.phase = 'ready';
        const controller = new LorepiaAppController({} as LorepiaClient);
        const onOpenSection = vi.fn();

        render(ProviderSettings, { appState, controller, section: null, onOpenSection });

        await fireEvent.click(screen.getByRole('button', { name: '설정 더보기' }));
        const menu = screen.getByRole('menu', { name: '설정 바로가기' });
        await fireEvent.click(within(menu).getByRole('menuitem', { name: '화면 모드' }));

        expect(onOpenSection).toHaveBeenCalledWith('appearance');
        expect(screen.queryByRole('menu', { name: '설정 바로가기' })).not.toBeInTheDocument();
        controller.destroy();
    });

    it('groups destination buttons into one rounded settings panel', () => {
        expect(appCss).toMatch(/\.setting-list\s*\{[^}]*padding:\s*0;/s);
        expect(appCss).toMatch(
            /\.setting-list\s*\{[^}]*border-radius:\s*clamp\(20px,\s*6\.59vw,\s*24px\);[^}]*margin:\s*0 clamp\(3px,\s*0\.686vw,\s*3px\);[^}]*background:\s*var\(--bg\);[^}]*box-shadow:\s*var\(--shadow-1\);[^}]*gap:\s*clamp\(2px,\s*0\.686vw,\s*3px\);[^}]*overflow:\s*hidden;/s,
        );
        expect(appCss).toMatch(
            /\.setting-row\s*\{[^}]*min-height:\s*clamp\(54px,\s*15\.561vw,\s*68px\);[^}]*border:\s*0;[^}]*border-radius:\s*clamp\(3px,\s*0\.915vw,\s*4px\);[^}]*background:\s*var\(--surface-raised\);[^}]*box-shadow:\s*none;[^}]*gap:\s*clamp\(19px,\s*5\.492vw,\s*24px\);/s,
        );
        expect(appCss).toMatch(/\.setting-copy\s*\{[^}]*flex:\s*1;/s);
    });

    it('keeps the shared panel and its inset rows visually distinct', () => {
        const appState = structuredClone(INITIAL_APP_STATE);
        appState.providers.phase = 'ready';
        const controller = new LorepiaAppController({} as LorepiaClient);

        const rendered = render(ProviderSettings, { appState, controller, section: null });
        const pane = rendered.container.querySelector<HTMLElement>('.provider-pane');
        const list = rendered.container.querySelector<HTMLElement>('.setting-list');
        const button = rendered.container.querySelector<HTMLElement>('.setting-row');
        if (pane === null || list === null || button === null) {
            throw new Error('settings surfaces are missing');
        }

        expect(pane).toHaveClass('provider-pane');
        expect(list).toHaveClass('setting-list');
        expect(appCss).toMatch(/\.provider-pane\s*\{[^}]*background:\s*var\(--bg\)/s);
        expect(appCss).toMatch(/\.setting-list\s*\{[^}]*background:\s*var\(--bg\)/s);
        expect(appCss).toMatch(/\.setting-row\s*\{[^}]*background:\s*var\(--surface-raised\)/s);
        controller.destroy();
    });

    it('presents the partial-generation preference as a trailing mobile switch', () => {
        const appState = legacyProviderState();
        const controller = new LorepiaAppController({} as LorepiaClient);

        render(ProviderSettings, { appState, controller, section: 'target' });

        const toggle = screen.getByRole('switch', {
            name: '취소·오류 시 생성된 일부 응답을 보존',
        });
        expect(toggle).toBeChecked();
        expect(toggle).toHaveClass('settings-switch');
        expect(toggle.closest('label')).toHaveClass('settings-control-row');
        controller.destroy();
    });
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

        render(ProviderSettings, { appState, controller, section: 'target' });

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
            section: 'connections',
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

        const rendered = render(ProviderSettings, {
            appState: legacyProviderState(),
            controller,
            section: 'connections',
        });

        await fireEvent.click(screen.getByRole('button', { name: '기본 대상으로 선택' }));

        await rendered.rerender({
            appState: legacyProviderState(),
            controller,
            section: 'target',
        });
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

        render(ProviderSettings, { appState, controller, section: 'target' });

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

        const rendered = render(ProviderSettings, { appState, controller, section: 'connections' });

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
        await rendered.rerender({ appState, controller, section: 'target' });
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
