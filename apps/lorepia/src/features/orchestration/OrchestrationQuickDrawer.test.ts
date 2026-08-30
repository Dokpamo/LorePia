import { cleanup, fireEvent, render, screen, waitFor, within } from '@testing-library/svelte';
import { afterEach, describe, expect, it, vi } from 'vitest';

import OrchestrationQuickDrawer from './OrchestrationQuickDrawer.svelte';
import OrchestrationStudio from './OrchestrationStudio.svelte';
import {
    appState,
    controller,
    generationPreset,
    modelRoute,
    orchestrationState,
} from './tests/fixtures';

afterEach(() => {
    cleanup();
    vi.restoreAllMocks();
});

const GENERATION_PRESET = generationPreset();
const MODEL_ROUTE = modelRoute();

describe('OrchestrationQuickDrawer', () => {
    it('provides accessible quick controls and saves staged changes when it closes', async () => {
        const orchestrationController = controller();
        const stage = vi.spyOn(orchestrationController, 'stageRoomConfig');
        const save = vi.spyOn(orchestrationController, 'saveRoomConfig').mockResolvedValue(true);
        const readyAppState = appState();
        readyAppState.providers.workspace.routes.push({
            ...MODEL_ROUTE,
            id: 'route-2',
            model_id: 'synthetic-model-2',
            display_name: '합성 모델 2',
        });
        readyAppState.providers.workspace.presets.push({
            ...GENERATION_PRESET,
            id: 'generation-2',
            model_route_id: 'route-2',
            display_name: '합성 모델 2 균형 생성',
        });
        const rendered = render(OrchestrationQuickDrawer, {
            appState: readyAppState,
            orchestrationState: { ...orchestrationState(), dirty_room_config: true },
            controller: orchestrationController,
        });
        const persistentDrawer = rendered.container.querySelector('.quick-drawer');
        expect(persistentDrawer).toHaveAttribute('aria-hidden', 'true');
        expect(persistentDrawer).toHaveProperty('inert', true);

        const toggle = screen.getByRole('button', { name: '대화 설정' });
        await fireEvent.click(toggle);
        const drawer = screen.getByRole('dialog', { name: '대화 설정' });
        expect(drawer).toBe(persistentDrawer);
        expect(drawer).toHaveAttribute('aria-hidden', 'false');
        expect(drawer).toHaveProperty('inert', false);

        const promptChoice = within(drawer).getByRole('combobox', {
            name: /^프롬프트 프리셋:/,
        });
        const promptRect = vi
            .spyOn(promptChoice, 'getBoundingClientRect')
            .mockReturnValue(new DOMRect(90, 700, 320, 40));
        const viewportHeight = vi.spyOn(window, 'innerHeight', 'get').mockReturnValue(768);
        await fireEvent.click(promptChoice);
        expect(within(drawer).getByRole('listbox', { name: '프롬프트 프리셋 선택' })).toHaveClass(
            'above',
        );
        await fireEvent.keyDown(window, { key: 'Escape' });
        promptRect.mockRestore();
        viewportHeight.mockRestore();
        expect(screen.getByRole('dialog', { name: '대화 설정' })).toBeInTheDocument();
        expect(within(drawer).queryByRole('listbox')).not.toBeInTheDocument();

        const modelChoice = within(drawer).getByRole('combobox', { name: /^모델:/ });
        expect(modelChoice).toHaveAttribute('aria-expanded', 'false');
        await fireEvent.click(modelChoice);
        await fireEvent.click(within(drawer).getByRole('option', { name: /합성 모델 2/ }));
        await fireEvent.click(within(drawer).getByRole('combobox', { name: /^생성 프리셋:/ }));
        await fireEvent.click(within(drawer).getByRole('option', { name: '균형 생성' }));
        await fireEvent.click(within(drawer).getByRole('radio', { name: '길게' }));
        await fireEvent.input(within(drawer).getByRole('slider', { name: /창의성/ }), {
            target: { value: '73' },
        });
        await fireEvent.click(within(drawer).getByRole('combobox', { name: /^추론 강도:/ }));
        await fireEvent.click(within(drawer).getByRole('option', { name: '매우 높음' }));
        const memoryToggle = within(drawer).getByRole('switch', { name: '장기기억 사용' });
        const knowledgeToggle = within(drawer).getByRole('switch', {
            name: '세계관 지식 사용',
        });
        expect(memoryToggle).toHaveAttribute('aria-checked', 'true');
        expect(knowledgeToggle).toHaveAttribute('aria-checked', 'true');
        expect(within(drawer).queryByRole('checkbox')).not.toBeInTheDocument();
        await fireEvent.click(memoryToggle);
        await fireEvent.click(knowledgeToggle);
        expect(stage).toHaveBeenCalledWith({ generation_preset_id: 'generation-2' });
        expect(stage).toHaveBeenCalledWith({ generation_preset_id: 'generation-1' });
        expect(stage).toHaveBeenCalledWith({ response_length: 'long' });
        expect(stage).toHaveBeenCalledWith({ creativity: 73 });
        expect(stage).toHaveBeenCalledWith({ reasoning_effort: 'extra_high' });
        expect(memoryToggle).toBeEnabled();
        expect(stage).toHaveBeenCalledWith({ memory_enabled: false });
        expect(stage).toHaveBeenCalledWith({ knowledge_enabled: false });

        expect(within(drawer).queryByRole('button', { name: '고급 설정' })).not.toBeInTheDocument();
        expect(within(drawer).queryByRole('button', { name: '저장' })).not.toBeInTheDocument();

        vi.spyOn(drawer, 'getBoundingClientRect').mockReturnValue(new DOMRect(0, 0, 393, 852));
        await fireEvent.pointerDown(drawer, {
            button: 0,
            clientX: 80,
            clientY: 240,
            isPrimary: true,
            pointerId: 1,
        });
        await fireEvent.pointerMove(drawer, {
            buttons: 1,
            clientX: 250,
            clientY: 244,
            isPrimary: true,
            pointerId: 1,
        });
        await fireEvent.pointerUp(drawer, {
            button: 0,
            clientX: 250,
            clientY: 244,
            isPrimary: true,
            pointerId: 1,
        });
        expect(drawer).toHaveClass('utility-settling');
        await waitFor(() => expect(screen.queryByRole('dialog')).not.toBeInTheDocument());
        expect(rendered.container.querySelector('.quick-drawer')).toBe(persistentDrawer);
        expect(persistentDrawer).toHaveAttribute('aria-hidden', 'true');
        expect(persistentDrawer).toHaveProperty('inert', true);
        expect(save).toHaveBeenCalledOnce();
    });

    it('separates conversation settings from the persistent desktop tool dock', async () => {
        const rendered = render(OrchestrationQuickDrawer, {
            appState: appState(),
            orchestrationState: orchestrationState(),
            controller: controller(),
            desktop: true,
        });

        const settingsToggle = screen.getByRole('button', { name: '대화 설정' });
        const panelToggle = screen.getByRole('button', { name: '오른쪽 도구 패널 열기' });
        expect(settingsToggle.querySelector('.lucide-menu')).not.toBeNull();
        expect(panelToggle.querySelector('.lucide-panel-right-open')).not.toBeNull();
        await fireEvent.click(panelToggle);

        const dock = screen.getByRole('complementary', { name: '도구 패널' });
        expect(dock).toHaveClass('desktop', 'open');
        expect(dock).toHaveAttribute('data-view', 'tools');
        expect(dock).toHaveAttribute('aria-hidden', 'false');
        expect(within(dock).getByRole('button', { name: '대화 설정 열기' })).toBeVisible();
        expect(
            within(dock).queryByRole('button', { name: /^프롬프트 프리셋:/ }),
        ).not.toBeInTheDocument();
        expect(rendered.container.querySelector('.quick-drawer-backdrop')).not.toBeInTheDocument();
        expect(
            screen
                .getByRole('button', { name: '오른쪽 도구 패널 닫기' })
                .querySelector('.lucide-panel-right-close'),
        ).not.toBeNull();

        await fireEvent.click(settingsToggle);
        const settings = screen.getByRole('complementary', { name: '대화 설정' });
        expect(settings).toBe(dock);
        expect(settings).toHaveAttribute('data-view', 'settings');
        expect(within(settings).getByRole('button', { name: '도구 목록으로' })).toBeVisible();
        expect(within(settings).getByRole('combobox', { name: /^프롬프트 프리셋:/ })).toBeVisible();

        await fireEvent.click(screen.getByRole('button', { name: '오른쪽 도구 패널 닫기' }));
        await waitFor(() =>
            expect(
                screen.queryByRole('complementary', { name: '대화 설정' }),
            ).not.toBeInTheDocument(),
        );
    });

    it('filters large block sets and navigates their zone minimap', async () => {
        const state = orchestrationState();
        const recentBlock = state.workspace.prompt_blocks[1];
        if (recentBlock === undefined) throw new Error('recent block fixture is missing');
        recentBlock.enabled = false;
        render(OrchestrationStudio, {
            section: 'prompt',
            detailPage: 'blocks',
            appState: appState(),
            orchestrationState: state,
            controller: controller(),
        });
        const blockCard = screen.getByRole('region', { name: '프롬프트 블록' });
        const blockUi = within(blockCard);
        const minimap = blockUi.getByRole('navigation', { name: '프롬프트 블록 미니맵' });
        expect(minimap).toBeInTheDocument();
        expect(
            within(minimap).getByRole('button', { name: 'A. 앱 정책 구역으로 이동' }),
        ).toHaveAttribute('title', 'A. 앱 정책: 전체 1개, 사용 1개');
        expect(
            within(minimap).getByRole('button', { name: 'G. 최근 대화 구역으로 이동' }),
        ).toHaveAttribute('title', 'G. 최근 대화: 전체 1개, 사용 0개');

        await fireEvent.click(
            within(minimap).getByRole('button', { name: 'G. 최근 대화 구역으로 이동' }),
        );
        expect(blockUi.getByRole('combobox', { name: /블록 구역 필터/ })).toHaveAttribute(
            'data-value',
            'G. 최근 대화',
        );
        expect(blockUi.getByText('최근 대화')).toBeInTheDocument();
        expect(blockUi.queryByText('안전 정책')).not.toBeInTheDocument();

        await fireEvent.click(blockUi.getByLabelText('블록 활성 상태 필터'));
        await fireEvent.click(screen.getByRole('option', { name: '사용 중' }));
        expect(blockUi.getByText('표시할 프롬프트 블록이 없습니다.')).toBeInTheDocument();
        await fireEvent.click(blockUi.getByRole('button', { name: '블록 필터 초기화' }));
        await fireEvent.click(
            within(minimap).getByRole('button', { name: 'A. 앱 정책 구역으로 이동' }),
        );
        const zoneHeading = blockUi.getByRole('heading', { name: 'A. 앱 정책' });
        expect(zoneHeading).toHaveFocus();
        expect(blockUi.getByText('안전 정책')).toBeInTheDocument();
        expect(blockUi.queryByText('최근 대화')).not.toBeInTheDocument();
    });
});
