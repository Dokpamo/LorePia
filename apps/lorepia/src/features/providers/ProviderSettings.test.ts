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
    ProviderTemplateDto,
} from '../../lib/ipc/contracts';
import { setThemePreference } from '../../lib/theme';
import { INITIAL_PERSONA_STATE, PersonaController } from '../personas/persona-controller';
import '../../styles/app.css';
import appCss from '../../styles/app-css';
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
        const logo = rendered.container.querySelector<HTMLElement>(
            '.settings-avatar.brand-logo-mark',
        );
        expect(logo).toBeInTheDocument();
        expect(logo?.style.getPropertyValue('--logo-mask')).toContain('lorepia-logo-mark');
        expect(rendered.container.querySelector('.settings-avatar-badge')).toBeInTheDocument();
        expect(within(card).getAllByRole('button')).toHaveLength(9);
        expect(within(card).getByRole('button', { name: /검색과 동기화/ })).toBeInTheDocument();
        expect(within(card).getByRole('button', { name: /제공자 카탈로그/ })).toBeInTheDocument();
        expect(within(card).getByRole('button', { name: /페르소나 0개/ })).toBeInTheDocument();
        expect(
            within(card).getByRole('button', { name: /오픈소스 라이선스 ISC · MIT/ }),
        ).toBeInTheDocument();
        expect(within(card).queryByText('내 Persona')).not.toBeInTheDocument();
        expect(card.querySelectorAll('.setting-value')).toHaveLength(9);
        const settingIcons = [...card.querySelectorAll<HTMLElement>('.setting-icon')];
        expect(settingIcons).toHaveLength(9);
        expect(settingIcons.every((icon) => icon.querySelector('svg') !== null)).toBe(true);
        expect(card.querySelector('[data-tone]')).not.toBeInTheDocument();
        expect(within(card).getByText('라이트')).toBeInTheDocument();
        expect(card.querySelector('.setting-copy small')).not.toBeInTheDocument();
        expect(card.querySelector('.setting-chevron')).not.toBeInTheDocument();
        expect(within(card).queryByText('라이트·다크·시스템')).not.toBeInTheDocument();
        expect(within(card).queryByText('대화에서 나를 어떻게 부를지')).not.toBeInTheDocument();
        expect(
            identity.compareDocumentPosition(card) & Node.DOCUMENT_POSITION_FOLLOWING,
        ).toBeTruthy();
        controller.destroy();
    });

    it('bundles the complete Lucide and Feather license notices in a dedicated screen', () => {
        const appState = structuredClone(INITIAL_APP_STATE);
        appState.providers.phase = 'loading';
        const controller = new LorepiaAppController({} as LorepiaClient);
        const rendered = render(ProviderSettings, {
            appState,
            controller,
            section: 'licenses',
        });

        expect(screen.getByRole('region', { name: '오픈소스 라이선스' })).toBeInTheDocument();
        expect(screen.getByRole('heading', { name: 'Lucide Icons' })).toBeInTheDocument();
        expect(screen.getByText('상업적 사용')).toBeInTheDocument();
        expect(screen.getByText('수정')).toBeInTheDocument();
        expect(screen.getByText('재배포')).toBeInTheDocument();
        const notice = rendered.container.querySelector<HTMLElement>(
            'pre[aria-label="Lucide Icons 라이선스 전문"]',
        );
        if (notice === null) throw new Error('Lucide license notice is missing');
        expect(notice).toHaveTextContent('Copyright (c) 2026 Lucide Icons and Contributors');
        expect(notice).toHaveTextContent('Copyright (c) 2013-present Cole Bemis');
        expect(notice).toHaveTextContent(
            'Permission to use, copy, modify, and/or distribute this software',
        );
        expect(screen.queryByText('프로바이더 상태를 불러오는 중입니다.')).not.toBeInTheDocument();
        controller.destroy();
    });

    it('keeps the persona page independent from provider loading state', () => {
        const appState = structuredClone(INITIAL_APP_STATE);
        appState.providers.phase = 'loading';
        const controller = new LorepiaAppController({} as LorepiaClient);
        const personaController = new PersonaController({});
        const rendered = render(ProviderSettings, {
            appState,
            controller,
            section: 'persona',
            personaState: { ...structuredClone(INITIAL_PERSONA_STATE), phase: 'ready' },
            personaController,
        });

        const panel = rendered.container.querySelector<HTMLElement>('.persona-panel');
        if (panel === null) throw new Error('persona settings panel is missing');
        expect(panel).toHaveAccessibleName('페르소나');
        expect(within(panel).queryByRole('heading', { name: '현재 대화' })).not.toBeInTheDocument();
        const actionBar = within(panel).getByRole('toolbar', { name: '페르소나 작업' });
        expect(
            within(actionBar).getByRole('button', { name: '페르소나 추가하기' }),
        ).toBeInTheDocument();
        expect(screen.queryByText('프로바이더 상태를 불러오는 중입니다.')).not.toBeInTheDocument();
        expect(screen.queryByText('Local persona')).not.toBeInTheDocument();
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
        const choices = within(screenRegion).getByRole('list', { name: '화면 모드 선택' });
        expect(within(choices).getAllByRole('button')).toHaveLength(3);
        expect(within(choices).getByRole('button', { name: '시스템' })).toHaveClass(
            'detail-choice-row',
        );
        expect(within(choices).getByRole('button', { name: '시스템' })).toHaveAttribute(
            'aria-pressed',
            'true',
        );
        expect(within(choices).getByRole('button', { name: '라이트 모드' })).toHaveAttribute(
            'aria-pressed',
            'false',
        );
        expect(within(choices).getByRole('button', { name: '다크 모드' })).toHaveAttribute(
            'aria-pressed',
            'false',
        );
        expect(screen.queryByRole('button', { name: /화면 모드 라이트/ })).not.toBeInTheDocument();
        expect(
            within(screenRegion).queryByText(/시스템을 고르면 운영체제 설정을 따라갑니다/),
        ).not.toBeInTheDocument();

        await fireEvent.click(within(screenRegion).getByRole('button', { name: '다크 모드' }));
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
        const shortcuts = screen.getByRole('group', { name: '설정 바로가기' });
        await fireEvent.click(within(shortcuts).getByRole('button', { name: '화면 모드' }));

        expect(onOpenSection).toHaveBeenCalledWith('appearance');
        expect(screen.queryByRole('group', { name: '설정 바로가기' })).not.toBeInTheDocument();
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

    it('keeps target fields in the scroller and mutations in a Persona-style bottom toolbar', () => {
        const appState = legacyProviderState();
        const controller = new LorepiaAppController({} as LorepiaClient);

        const rendered = render(ProviderSettings, { appState, controller, section: 'target' });

        const toggle = screen.getByRole('switch', {
            name: '취소·오류 시 생성된 일부 응답을 보존',
        });
        expect(toggle).toBeChecked();
        expect(toggle).toHaveClass('toggle-switch');
        expect(toggle.closest('.settings-control-row')).toBeInTheDocument();
        const scroller = rendered.container.querySelector<HTMLElement>('.settings-detail-scroll');
        if (scroller === null) throw new Error('target settings scroller is missing');
        expect(scroller).toHaveClass('detail-scroll-has-actions');
        const toolbar = screen.getByRole('toolbar', { name: '기본 생성 대상 작업' });
        expect(within(toolbar).getByRole('button', { name: '해제' })).toBeInTheDocument();
        expect(within(toolbar).getByRole('button', { name: '저장' })).toBeInTheDocument();
        expect(scroller.nextElementSibling).toBe(toolbar);
        controller.destroy();
    });

    it.each([
        ['catalog', null],
        ['discovery', 'provider-discovery'],
        ['discovery', 'model-sync'],
        ['advanced', 'connections'],
        ['advanced', 'capabilities'],
    ] as const)(
        'gives the %s/%s pushed workflow one child-owned settings scroller',
        async (section, detailPage) => {
            const appState = structuredClone(INITIAL_APP_STATE);
            appState.providers.phase = 'ready';
            const controller = new LorepiaAppController({} as LorepiaClient);
            const onDetailScroll = vi.fn();
            const rendered = render(ProviderSettings, {
                appState,
                controller,
                section,
                detailPage,
                onDetailScroll,
            });

            const pane = rendered.container.querySelector('.provider-pane');
            const scrollers = pane?.querySelectorAll('.settings-detail-scroll');
            expect(scrollers).toHaveLength(1);
            expect(pane?.querySelector(':scope > .settings-detail-scroll')).toBeNull();
            expect(scrollers?.[0]).toHaveClass('detail-page-scroll');
            const scroller = scrollers?.[0];
            if (!scroller) throw new Error('child-owned settings scroller is missing');
            await waitFor(() => expect(onDetailScroll).toHaveBeenCalledWith(0));
            onDetailScroll.mockClear();
            Object.defineProperty(scroller, 'scrollTop', {
                configurable: true,
                value: 24,
                writable: true,
            });
            await fireEvent.scroll(scroller);
            expect(onDetailScroll).toHaveBeenLastCalledWith(24);
            controller.destroy();
        },
    );

    it('keeps pushed provider workflows behind the shared loading gate', () => {
        const appState = structuredClone(INITIAL_APP_STATE);
        appState.providers.phase = 'loading';
        const controller = new LorepiaAppController({} as LorepiaClient);
        const rendered = render(ProviderSettings, {
            appState,
            controller,
            section: 'discovery',
            detailPage: 'provider-discovery',
        });

        expect(screen.getByRole('status')).toHaveTextContent(
            '프로바이더 상태를 불러오는 중입니다.',
        );
        expect(screen.queryByRole('button', { name: '새 탐색' })).not.toBeInTheDocument();
        expect(rendered.container.querySelectorAll('.settings-detail-scroll')).toHaveLength(1);
        controller.destroy();
    });

    it('shows one retry surface instead of mounting a workflow in provider error state', async () => {
        const appState = structuredClone(INITIAL_APP_STATE);
        appState.providers.phase = 'error';
        appState.providers.error = 'synthetic provider failure';
        const controller = new LorepiaAppController({} as LorepiaClient);
        const loadProviders = vi.spyOn(controller, 'loadProviders').mockResolvedValue();

        render(ProviderSettings, {
            appState,
            controller,
            section: 'advanced',
            detailPage: 'capabilities',
        });

        const alert = screen.getByRole('alert');
        expect(alert).toHaveTextContent('synthetic provider failure');
        expect(screen.queryByRole('region', { name: '기능 호환성' })).not.toBeInTheDocument();
        await fireEvent.click(within(alert).getByRole('button', { name: '다시 시도' }));
        expect(loadProviders).toHaveBeenCalledOnce();
        controller.destroy();
    });

    it('stages the target and partial-response choice until save and discards drafts on re-entry', async () => {
        const appState = legacyProviderState();
        appState.providers.workspace.settings.preserve_partial_generations = true;
        const controller = new LorepiaAppController({} as LorepiaClient);
        const selectTarget = vi
            .spyOn(controller, 'selectProviderGenerationTarget')
            .mockResolvedValue(true);
        const savePartial = vi
            .spyOn(controller, 'setPreservePartialGenerations')
            .mockResolvedValue(true);
        const rendered = render(ProviderSettings, {
            appState,
            controller,
            section: 'target',
        });

        let toggle = screen.getByRole('switch', {
            name: '취소·오류 시 생성된 일부 응답을 보존',
        });
        await fireEvent.click(toggle);
        expect(toggle).not.toBeChecked();
        expect(selectTarget).not.toHaveBeenCalled();
        expect(savePartial).not.toHaveBeenCalled();

        await rendered.rerender({ appState, controller, section: null });
        await rendered.rerender({ appState, controller, section: 'target' });
        toggle = screen.getByRole('switch', {
            name: '취소·오류 시 생성된 일부 응답을 보존',
        });
        expect(toggle).toBeChecked();

        await fireEvent.click(toggle);
        expect(toggle).not.toBeChecked();
        await fireEvent.click(screen.getByRole('button', { name: '해제' }));
        expect(toggle).not.toBeChecked();
        expect(selectTarget).not.toHaveBeenCalled();

        await fireEvent.click(screen.getByRole('button', { name: '저장' }));
        await waitFor(() => {
            expect(selectTarget).toHaveBeenCalledWith(null, null);
            expect(savePartial).toHaveBeenCalledWith(false);
        });
        controller.destroy();
    });

    it('stays on the target editor when request preview generation fails', async () => {
        const appState = ordinaryTargetState();
        const controller = new LorepiaAppController({} as LorepiaClient);
        const preview = vi
            .spyOn(controller, 'previewSelectedProviderRequest')
            .mockResolvedValue(false);

        render(ProviderSettings, { appState, controller, section: 'target' });
        await fireEvent.click(screen.getByRole('button', { name: '요청 구조 미리보기' }));

        expect(preview).toHaveBeenCalledOnce();
        expect(screen.getByRole('region', { name: '기본 생성 대상 편집' })).toBeInTheDocument();
        expect(
            screen.queryByRole('region', { name: '민감값이 제거된 요청 구조' }),
        ).not.toBeInTheDocument();
        controller.destroy();
    });

    it('opens a connection row as a full detail page and confirms credential deletion twice', async () => {
        const appState = connectedProviderState();
        const controller = new LorepiaAppController({} as LorepiaClient);
        const deleteProviderCredential = vi
            .spyOn(controller, 'deleteProviderCredential')
            .mockResolvedValue();
        const rendered = render(ProviderSettings, {
            appState,
            controller,
            section: 'connections',
        });

        await fireEvent.click(
            screen.getByRole('button', { name: /테스트 연결 synthetic-template/ }),
        );

        const detail = screen.getByRole('region', { name: '테스트 연결' });
        expect(within(detail).getByText('자격증명 저장됨')).toBeInTheDocument();
        const scroller = rendered.container.querySelector<HTMLElement>('.settings-detail-scroll');
        if (scroller === null) throw new Error('connection detail scroller is missing');
        const toolbar = screen.getByRole('toolbar', { name: '자격증명 작업' });
        expect(scroller.nextElementSibling).toBe(toolbar);

        await fireEvent.click(within(toolbar).getByRole('button', { name: '삭제' }));
        expect(deleteProviderCredential).not.toHaveBeenCalled();
        expect(within(toolbar).getByRole('button', { name: '삭제 확인' })).toBeInTheDocument();
        expect(within(toolbar).getByRole('button', { name: '취소' })).toBeInTheDocument();

        await fireEvent.click(within(toolbar).getByRole('button', { name: '삭제 확인' }));
        await waitFor(() =>
            expect(deleteProviderCredential).toHaveBeenCalledWith({
                kind: 'connection',
                connection_id: 'connection-1',
            }),
        );
        controller.destroy();
    });

    it('opens a template row as a flat read-only detail page', async () => {
        const appState = templatedProviderState();
        const controller = new LorepiaAppController({} as LorepiaClient);
        const rendered = render(ProviderSettings, {
            appState,
            controller,
            section: 'templates',
            detailPage: null,
        });

        const list = screen.getByRole('list', { name: '템플릿 목록' });
        const row = within(list).getByRole('button', {
            name: /Synthetic API open_ai_responses · v2 필드 1 · 파라미터 1/,
        });
        expect(row).toHaveClass('detail-record-row');
        expect(row.querySelector('.setting-chevron')).not.toBeInTheDocument();

        await fireEvent.click(row);

        const detail = screen.getByRole('region', { name: 'Synthetic API 템플릿 정보' });
        expect(screen.queryByRole('list', { name: '템플릿 목록' })).not.toBeInTheDocument();
        expect(within(detail).getByText('https://api.synthetic.invalid')).toBeInTheDocument();
        expect(within(detail).getByText('X-API-Key 헤더 API 키')).toBeInTheDocument();
        expect(within(detail).getByText('public')).toBeInTheDocument();
        expect(within(detail).getByRole('heading', { name: '연결 필드' })).toBeInTheDocument();
        expect(within(detail).getByText('organization · string · 선택')).toBeInTheDocument();
        expect(within(detail).getByRole('heading', { name: '생성 파라미터' })).toBeInTheDocument();
        expect(within(detail).getByText('temperature · number · standard')).toBeInTheDocument();
        expect(within(detail).getByText('허용값: 0.7')).toBeInTheDocument();
        expect(detail.querySelector('.settings-card, .provider-card')).not.toBeInTheDocument();
        expect(rendered.container.querySelector('.settings-detail-scroll')).toContainElement(
            detail,
        );
        controller.destroy();
    });

    it('keeps the template empty state on the templates index', () => {
        const appState = structuredClone(INITIAL_APP_STATE);
        appState.providers.phase = 'ready';
        const controller = new LorepiaAppController({} as LorepiaClient);

        render(ProviderSettings, {
            appState,
            controller,
            section: 'templates',
            detailPage: null,
        });

        expect(screen.getByText('현재 사용할 수 있는 템플릿이 없습니다.')).toBeInTheDocument();
        expect(screen.queryByRole('list', { name: '템플릿 목록' })).not.toBeInTheDocument();
        controller.destroy();
    });
});

const TEMPLATE: ProviderTemplateDto = {
    id: 'synthetic-template',
    display_name: 'Synthetic API',
    manifest_version: 2,
    source: 'bundled',
    api_family: 'open_ai_responses',
    connection_fields: [
        {
            key: 'organization',
            label_key: '조직',
            description_key: '요청에 사용할 조직 ID',
            value_type: 'string',
            required: false,
        },
    ],
    default_network_mode: 'public',
    default_api_origin: 'https://api.synthetic.invalid',
    credential_required: true,
    supports_model_listing: true,
    auth_binding: { kind: 'header_api_key', header_name: 'X-API-Key' },
    parameters: [
        {
            id: 'temperature',
            label_key: '온도',
            description_key: '응답의 무작위성',
            value_type: 'number',
            allowed_values: [{ value: { type: 'number', value: 0.7 }, label_key: '균형' }],
            minimum: 0,
            maximum: 2,
            step: 0.1,
            default_mode: 'provider_default',
            visibility: null,
            conflicts: [],
            provider_mapping: { target: 'json_body', field_name: 'temperature' },
            level: 'standard',
        },
    ],
};

function templatedProviderState(): LorepiaAppState {
    const state = structuredClone(INITIAL_APP_STATE);
    state.providers.phase = 'ready';
    state.providers.workspace.templates = [TEMPLATE];
    return state;
}

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

function ordinaryTargetState(): LorepiaAppState {
    const state = structuredClone(INITIAL_APP_STATE);
    state.providers.phase = 'ready';
    const route = {
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
    } satisfies ModelRouteDto;
    const preset = {
        id: 'ordinary-preset',
        model_route_id: route.id,
        display_name: 'Ordinary preset',
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
    } satisfies GenerationPresetDto;
    state.providers.workspace.routes = [route];
    state.providers.workspace.presets = [preset];
    state.providers.workspace.settings.selected_model_route_id = route.id;
    state.providers.workspace.settings.selected_generation_preset_id = preset.id;
    return state;
}

function connectedProviderState(): LorepiaAppState {
    const state = structuredClone(INITIAL_APP_STATE);
    state.providers.phase = 'ready';
    state.providers.workspace.connections = [
        {
            id: 'connection-1',
            template_id: 'synthetic-template',
            template_version: 1,
            display_name: '테스트 연결',
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
            created_at: '2026-08-23T00:00:00Z',
            updated_at: '2026-08-23T00:00:00Z',
        },
    ];
    state.providers.workspace.credential_statuses['connection:connection-1'] = 'available';
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
    it('does not expose retained legacy alias routes as ordinary generation targets', async () => {
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

        const targetSection = screen.getByRole('region', { name: '기본 생성 대상 편집' });
        const routeSelect = within(targetSection).getByLabelText('모델 라우트');
        await fireEvent.click(routeSelect);
        expect(
            within(targetSection).queryByRole('option', { name: 'Legacy alias model' }),
        ).not.toBeInTheDocument();
        expect(
            within(targetSection).getByRole('option', { name: 'Ordinary model' }),
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

        await fireEvent.click(
            screen.getByRole('button', { name: /보존된 레거시 프로필 기존 프로필/ }),
        );
        const profileDetail = screen.getByRole('region', { name: '보존된 레거시 프로필' });

        await fireEvent.click(
            within(profileDetail).getByRole('button', { name: '기본 대상으로 선택' }),
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

        await fireEvent.click(
            screen.getByRole('button', { name: /보존된 레거시 프로필 기존 프로필/ }),
        );
        await fireEvent.click(screen.getByRole('button', { name: '기본 대상으로 선택' }));

        await rendered.rerender({
            appState: legacyProviderState(),
            controller,
            section: 'target',
            detailPage: null,
        });
        expect(screen.getByLabelText('취소·오류 시 생성된 일부 응답을 보존')).toBeDisabled();
        expect(screen.getByRole('button', { name: '해제' })).toBeDisabled();
        expect(screen.getByRole('button', { name: '저장' })).toBeDisabled();
        const targetSection = screen.getByRole('region', { name: '기본 생성 대상 편집' });
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

        const summary = screen.getByRole('region', { name: '기본 생성 대상 편집' });
        expect(
            within(summary).getByText('기존 프로바이더 프로필을 기본 대상으로 사용 중입니다.'),
        ).toBeInTheDocument();
        controller.destroy();
    });

    it('preserves a retained legacy target when only the partial-response setting changes', async () => {
        const appState = legacyProviderState();
        appState.providers.workspace.settings.selected_provider_profile_id = 'legacy-profile-1';
        appState.providers.workspace.settings.selected_model_route_id = 'legacy-profile-1';
        appState.providers.workspace.settings.selected_generation_preset_id = 'legacy-profile-1';
        appState.providers.workspace.settings.preserve_partial_generations = true;
        const controller = new LorepiaAppController({} as LorepiaClient);
        const selectTarget = vi
            .spyOn(controller, 'selectProviderGenerationTarget')
            .mockResolvedValue(true);
        const savePartial = vi
            .spyOn(controller, 'setPreservePartialGenerations')
            .mockResolvedValue(true);

        render(ProviderSettings, { appState, controller, section: 'target' });

        const targetSection = screen.getByRole('region', { name: '기본 생성 대상 편집' });
        expect(within(targetSection).getByLabelText('모델 라우트')).toHaveAttribute(
            'data-value',
            '',
        );
        expect(screen.getByRole('button', { name: '저장' })).toBeDisabled();
        await fireEvent.click(
            screen.getByRole('switch', {
                name: '취소·오류 시 생성된 일부 응답을 보존',
            }),
        );
        await fireEvent.click(screen.getByRole('button', { name: '저장' }));

        await waitFor(() => expect(savePartial).toHaveBeenCalledWith(false));
        expect(selectTarget).not.toHaveBeenCalled();
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

        const rendered = render(ProviderSettings, {
            appState,
            controller,
            section: 'connections',
            detailPage: null,
        });

        await fireEvent.click(
            screen.getByRole('button', { name: /정규화된 레거시 연결 legacy-openai-compatible/ }),
        );
        const ordinaryConnectionDetail = screen.getByRole('region', {
            name: '정규화된 레거시 연결',
        });
        expect(within(ordinaryConnectionDetail).getByText('자격증명 저장됨')).toBeInTheDocument();
        expect(screen.queryByRole('toolbar', { name: '자격증명 작업' })).not.toBeInTheDocument();

        await rendered.rerender({
            appState,
            controller,
            section: 'connections',
            detailPage: null,
        });
        await fireEvent.click(
            screen.getByRole('button', { name: /보존된 레거시 프로필 기존 프로필/ }),
        );
        const legacyCredentialActions = screen.getByRole('toolbar', {
            name: '자격증명 작업',
        });
        await fireEvent.click(
            within(legacyCredentialActions).getByRole('button', {
                name: '자격증명 캡처',
            }),
        );
        await fireEvent.click(
            within(legacyCredentialActions).getByRole('button', { name: '삭제' }),
        );
        expect(deleteProviderCredential).not.toHaveBeenCalled();
        await fireEvent.click(
            within(legacyCredentialActions).getByRole('button', { name: '삭제 확인' }),
        );

        expect(captureProviderCredential).toHaveBeenCalledWith({
            kind: 'legacy_profile',
            provider_profile_id: 'legacy-profile-1',
        });
        expect(deleteProviderCredential).toHaveBeenCalledWith({
            kind: 'legacy_profile',
            provider_profile_id: 'legacy-profile-1',
        });
        await rendered.rerender({ appState, controller, section: 'target', detailPage: null });
        const summary = screen.getByRole('region', { name: '기본 생성 대상 편집' });
        expect(
            within(summary).getByText('기존 프로바이더 프로필을 기본 대상으로 사용 중입니다.'),
        ).toBeInTheDocument();
        controller.destroy();
    });
});
