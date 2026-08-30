import { cleanup, fireEvent, render, screen, within } from '@testing-library/svelte';
import { afterEach, describe, expect, it, vi } from 'vitest';

import OrchestrationStudio from './OrchestrationStudio.svelte';
import { appState, controller, orchestrationState } from './tests/fixtures';

afterEach(() => {
    cleanup();
    vi.restoreAllMocks();
});

describe('OrchestrationStudio', () => {
    it('pushes interaction proposals into a bounded review page with fixed decisions', async () => {
        const state = orchestrationState();
        state.workspace.interaction_state = [
            {
                id: 'mood',
                label: '기분',
                value: '차분함',
                scope: 'conversation',
            },
        ];
        state.workspace.interaction_proposals = [
            {
                conversation_id: 'conversation-1',
                branch_id: 'branch-1',
                state_revision: 11,
                proposal_revision: 4,
                proposal: {
                    id: 'proposal-1',
                    title: '합성 상태 변경',
                    body: '현재 상태를 바꾸는 제안입니다.',
                    status: 'pending',
                    source_interaction_state_revision: 10,
                    requested_at_epoch_seconds: 1,
                    expires_at_epoch_seconds: 60,
                    decided_at_epoch_seconds: null,
                },
            },
        ];
        const studioController = controller();
        const decideProposal = vi.spyOn(studioController, 'decideProposal').mockResolvedValue(true);
        render(OrchestrationStudio, {
            section: 'memory',
            detailPage: 'interactions',
            appState: appState(),
            orchestrationState: state,
            controller: studioController,
        });

        expect(screen.getByText('기분')).toBeInTheDocument();
        const proposals = screen.getByRole('list', { name: '사용자 승인 제안 목록' });
        expect(
            within(proposals).queryByText('현재 상태를 바꾸는 제안입니다.'),
        ).not.toBeInTheDocument();
        await fireEvent.click(within(proposals).getByRole('button', { name: /합성 상태 변경/ }));

        expect(screen.getByRole('region', { name: '상호작용 검토' })).toHaveTextContent(
            '현재 상태를 바꾸는 제안입니다.',
        );
        const reviewActions = screen.getByRole('toolbar', {
            name: '상호작용 제안 검토 작업',
        });
        expect(reviewActions).toHaveClass('fixed');
        expect(within(reviewActions).getByRole('button', { name: '거절' })).toBeEnabled();
        expect(within(reviewActions).getByRole('button', { name: '승인' })).toBeEnabled();
        await fireEvent.click(within(reviewActions).getByRole('button', { name: '승인' }));
        expect(decideProposal).toHaveBeenCalledWith('proposal-1', true);
        expect(screen.getByRole('list', { name: '사용자 승인 제안 목록' })).toBeInTheDocument();
    });

    it('never projects unsafe stored interaction text and only allows rejection', () => {
        const state = orchestrationState();
        state.workspace.interaction_proposals = [
            {
                conversation_id: 'conversation-1',
                branch_id: 'branch-1',
                state_revision: 11,
                proposal_revision: 4,
                proposal: {
                    id: 'proposal-redacted',
                    title: 'unsafe title sentinel',
                    body: 'unsafe body sentinel',
                    projection_rejection_reason: 'unsafe_native_text',
                    status: 'pending',
                    source_interaction_state_revision: 10,
                    requested_at_epoch_seconds: 1,
                    expires_at_epoch_seconds: null,
                    decided_at_epoch_seconds: null,
                },
            },
        ];
        render(OrchestrationStudio, {
            section: 'memory',
            detailPage: 'interactions/review/proposal-redacted',
            appState: appState(),
            orchestrationState: state,
            controller: controller(),
        });

        expect(screen.getByText('저장 제안 내용을 표시할 수 없음')).toBeInTheDocument();
        expect(screen.queryByText('unsafe title sentinel')).not.toBeInTheDocument();
        expect(screen.queryByText('unsafe body sentinel')).not.toBeInTheDocument();
        expect(screen.getByRole('button', { name: '거절' })).toBeEnabled();
        expect(screen.getByRole('button', { name: '승인' })).toBeDisabled();
    });
});
