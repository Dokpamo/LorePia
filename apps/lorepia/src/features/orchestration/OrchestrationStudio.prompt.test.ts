import { cleanup, fireEvent, render, screen, within } from '@testing-library/svelte';
import { afterEach, describe, expect, it, vi } from 'vitest';

import OrchestrationStudio from './OrchestrationStudio.svelte';
import { appState, controller, orchestrationState } from './tests/fixtures';

afterEach(() => {
    cleanup();
    vi.restoreAllMocks();
});

describe('OrchestrationStudio', () => {
    it('renders imported markup only as text and keeps Core policy blocks read-only', async () => {
        const orchestrationController = controller();
        const move = vi.spyOn(orchestrationController, 'movePromptBlock').mockResolvedValue(true);
        render(OrchestrationStudio, {
            section: 'prompt',
            detailPage: 'blocks',
            appState: appState(),
            orchestrationState: orchestrationState(),
            controller: orchestrationController,
        });

        await fireEvent.input(screen.getByRole('searchbox', { name: '블록 검색' }), {
            target: { value: '안전 정책' },
        });
        expect(screen.getByText('안전 정책')).toBeInTheDocument();
        expect(screen.queryByText('최근 대화')).not.toBeInTheDocument();

        const movePolicy = screen.getByRole('button', { name: '안전 정책 블록 아래로 이동' });
        expect(movePolicy).toBeDisabled();
        expect(move).not.toHaveBeenCalled();

        await fireEvent.click(screen.getByRole('button', { name: /안전 정책 static_instruction/ }));
        expect(screen.getByRole('region', { name: '프롬프트 블록 편집' })).toBeInTheDocument();
        expect(screen.getByText('<img src=x onerror=alert(1)>')).toBeInTheDocument();
        expect(document.querySelector('img')).toBeNull();
        expect(screen.queryByRole('group', { name: '구조화된 블록 편집' })).not.toBeInTheDocument();
        const blockActions = screen.getByRole('toolbar', { name: '프롬프트 블록 작업' });
        expect(blockActions).toHaveClass('fixed');
        expect(within(blockActions).getByRole('button', { name: '다시 불러오기' })).toBeEnabled();
        expect(within(blockActions).getByRole('button', { name: '저장' })).toBeDisabled();
    });

    it('stages bounded room prompt sources and saves them explicitly', async () => {
        const readyState = orchestrationState();
        readyState.dirty_room_config = true;
        readyState.workspace.room_config.user_name_override = '별이';
        readyState.workspace.room_config.author_note = '차분한 장면을 유지한다.';
        readyState.workspace.room_config.group_context = '별이와 달이가 함께 대화한다.';
        readyState.workspace.room_config.template_slots = [{ name: 'tone', value: '차분하게' }];
        const orchestrationController = controller();
        const stage = vi.spyOn(orchestrationController, 'stageRoomConfig');
        const save = vi.spyOn(orchestrationController, 'saveRoomConfig').mockResolvedValue(true);
        render(OrchestrationStudio, {
            section: 'prompt',
            detailPage: 'room',
            appState: appState(),
            orchestrationState: readyState,
            controller: orchestrationController,
        });

        const userName = screen.getByLabelText('사용자 표시 이름');
        const authorNote = screen.getByLabelText('작가 메모');
        const groupContext = screen.getByLabelText('그룹 문맥');
        const slotName = screen.getByLabelText('템플릿 슬롯 1 이름');
        const slotValue = screen.getByLabelText('템플릿 슬롯 1 값');
        expect(userName).toHaveAttribute('maxlength', '128');
        expect(authorNote).toHaveAttribute('maxlength', '32768');
        expect(groupContext).toHaveAttribute('maxlength', '32768');
        expect(slotName).toHaveAttribute('maxlength', '128');
        expect(slotValue).toHaveAttribute('maxlength', '32768');

        await fireEvent.input(userName, { target: { value: '새 별칭' } });
        await fireEvent.input(authorNote, { target: { value: '새 작가 메모' } });
        await fireEvent.input(groupContext, { target: { value: '새 그룹 문맥' } });
        await fireEvent.input(slotName, { target: { value: 'voice' } });
        await fireEvent.input(slotValue, { target: { value: '명랑하게' } });
        expect(stage).toHaveBeenCalledWith({ user_name_override: '새 별칭' });
        expect(stage).toHaveBeenCalledWith({ author_note: '새 작가 메모' });
        expect(stage).toHaveBeenCalledWith({ group_context: '새 그룹 문맥' });
        expect(stage).toHaveBeenCalledWith({
            template_slots: [{ name: 'voice', value: '차분하게' }],
        });
        expect(stage).toHaveBeenCalledWith({
            template_slots: [{ name: 'tone', value: '명랑하게' }],
        });

        await fireEvent.click(screen.getByRole('button', { name: '슬롯 추가' }));
        expect(stage).toHaveBeenCalledWith({
            template_slots: [
                { name: 'tone', value: '차분하게' },
                { name: '', value: '' },
            ],
        });
        const actions = screen.getByRole('toolbar', { name: '방별 프롬프트 소스 작업' });
        expect(actions).toHaveClass('fixed');
        await fireEvent.click(within(actions).getByRole('button', { name: '저장' }));
        expect(save).toHaveBeenCalledOnce();
    });

    it('blocks duplicate template-slot names and caps the slot editor at 128 entries', () => {
        const duplicateState = orchestrationState();
        duplicateState.dirty_room_config = true;
        duplicateState.workspace.room_config.template_slots = [
            { name: 'tone', value: '차분하게' },
            { name: 'tone', value: '명랑하게' },
        ];
        render(OrchestrationStudio, {
            section: 'prompt',
            detailPage: 'room',
            appState: appState(),
            orchestrationState: duplicateState,
            controller: controller(),
        });

        expect(screen.getByRole('alert')).toHaveTextContent('중복되었습니다');
        expect(
            within(screen.getByRole('toolbar', { name: '방별 프롬프트 소스 작업' })).getByRole(
                'button',
                { name: '저장' },
            ),
        ).toBeDisabled();

        cleanup();
        const cappedState = orchestrationState();
        cappedState.workspace.room_config.template_slots = Array.from(
            { length: 128 },
            (_, index) => ({ name: `slot_${String(index)}`, value: '' }),
        );
        render(OrchestrationStudio, {
            section: 'prompt',
            detailPage: 'room',
            appState: appState(),
            orchestrationState: cappedState,
            controller: controller(),
        });

        expect(screen.getByLabelText('템플릿 슬롯 128 이름')).toBeInTheDocument();
        expect(screen.queryByLabelText('템플릿 슬롯 129 이름')).not.toBeInTheDocument();
        expect(screen.getByRole('button', { name: '슬롯 추가' })).toBeDisabled();
    });
});
