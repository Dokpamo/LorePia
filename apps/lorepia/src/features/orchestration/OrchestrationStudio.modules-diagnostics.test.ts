import { cleanup, fireEvent, render, screen, within } from '@testing-library/svelte';
import { afterEach, describe, expect, it, vi } from 'vitest';

import OrchestrationStudio from './OrchestrationStudio.svelte';
import { appState, controller, orchestrationState } from './tests/fixtures';

afterEach(() => {
    cleanup();
    vi.restoreAllMocks();
});

describe('OrchestrationStudio', () => {
    it('shows the safe module lifecycle boundary and a bounded, escaped final plan preview in expert mode', async () => {
        const orchestrationController = controller();
        const expertProps = {
            section: 'content' as const,
            detailPage: 'modules',
            appState: appState(),
            orchestrationState: orchestrationState(),
            controller: orchestrationController,
        };
        const rendered = render(OrchestrationStudio, expertProps);

        expect(
            screen.getByRole('heading', { name: '콘텐츠 모듈 활성화·롤백' }),
        ).toBeInTheDocument();
        expect(
            screen.getAllByText(/해시로 고정된 콘텐츠 모듈 활성화·롤백 API/).length,
        ).toBeGreaterThan(0);

        await rendered.rerender({
            ...expertProps,
            section: 'diagnostics',
            detailPage: 'plan',
        });
        expect(screen.getByText('sha256:synthetic-plan')).toBeInTheDocument();
        expect(screen.queryByText('<script>never execute</script>')).not.toBeInTheDocument();
        expect(document.querySelector('script:not([src])')).toBeNull();
        expect(screen.getByText(/사용자가 요청할 때만 실제 생성/)).toBeInTheDocument();
        expect(screen.queryByText('private policy body')).not.toBeInTheDocument();
        expect(screen.getAllByText(/developer → system/).length).toBeGreaterThan(0);
        expect(screen.getByText(/제공자 계열/)).toHaveTextContent('anthropic_messages');
        expect(screen.getByText(/매핑 anthropic_inline_breakpoint/)).toBeInTheDocument();
        expect(screen.getByText(/lorepia\.application-policy\.v1/)).toBeInTheDocument();
        expect(screen.getByText('8 / 8')).toBeInTheDocument();
        expect(screen.getByText(/비공개 프롬프트 본문과 원시 제공자 요청/)).toBeInTheDocument();

        const messageSummary = screen.getByText('최종 메시지 구조 (2개)');
        const messageDetails = messageSummary.closest('details');
        if (messageDetails === null) throw new Error('message structure details are missing');
        expect(messageDetails).not.toHaveAttribute('open');
        await fireEvent.click(messageSummary);
        expect(within(messageDetails).getByText(/순서 0 · 블록 block-1/)).toBeInTheDocument();
        expect(
            screen.queryByText('project-owned synthetic effective instruction'),
        ).not.toBeInTheDocument();
        await fireEvent.click(screen.getByText('제공자 변환 구조 (2개)'));
        expect(screen.queryByText(/"model": "synthetic-model"/)).not.toBeInTheDocument();
        expect(screen.getByText('temperature')).toBeInTheDocument();
        expect(screen.getByText(/developer → system → system · message/)).toBeInTheDocument();

        await fireEvent.click(screen.getByLabelText('표시 필터'));
        await fireEvent.click(screen.getByRole('option', { name: '적용 파라미터' }));
        expect(screen.queryByText('최종 메시지 구조 (2개)')).not.toBeInTheDocument();
        expect(document.body.textContent.toLocaleLowerCase()).not.toContain('authorization');
        expect(document.body.textContent.toLocaleLowerCase()).not.toContain('api_key');
        expect(document.body.textContent).not.toContain('/Users/');
    });

    it('shows only content-free, hash-verified DisplayOnly diagnostics after message reopen', () => {
        const reopenedAppState = appState();
        reopenedAppState.messages = {
            phase: 'ready',
            error: null,
            items: [
                {
                    id: 'message-user-1',
                    conversation_id: 'conversation-1',
                    parent_id: null,
                    role: 'user',
                    content: 'reopened user message',
                    status: 'complete',
                    generation_id: null,
                    created_at: '2026-08-03T00:00:00Z',
                },
                {
                    id: 'message-assistant-1',
                    conversation_id: 'conversation-1',
                    parent_id: 'message-user-1',
                    role: 'assistant',
                    content: 'DISPLAY_CONTENT_CANARY_MUST_NOT_ENTER_DIAGNOSTICS',
                    status: 'complete',
                    generation_id: 'generation-1',
                    created_at: '2026-08-03T00:00:01Z',
                    display_projection: {
                        canonical_content_sha256: 'c'.repeat(64),
                        display_content_sha256: 'd'.repeat(64),
                        diagnostics_sha256: 'e'.repeat(64),
                        diagnostics: [
                            {
                                set_revision_id: 'transform-set-revision-7',
                                rule_id: 'display-rule-1',
                                stage: 'display_only',
                                disposition: 'applied',
                                code: null,
                                before_sha256: 'a'.repeat(64),
                                after_sha256: 'b'.repeat(64),
                                recorded_at: '2026-08-03T00:00:01Z',
                            },
                        ],
                    },
                },
            ],
        };
        render(OrchestrationStudio, {
            section: 'diagnostics',
            detailPage: 'display',
            appState: reopenedAppState,
            orchestrationState: orchestrationState(),
            controller: controller(),
        });
        const diagnosticsCard = screen.getByRole('region', {
            name: '메시지 표시 변환 진단',
        });
        const card = within(diagnosticsCard);
        expect(card.getByText('display_only · applied')).toBeInTheDocument();
        expect(card.getByText('transform-set-revision-7')).toBeInTheDocument();
        expect(card.getByText('display-rule-1')).toBeInTheDocument();
        expect(card.getByText('c'.repeat(64))).toBeInTheDocument();
        expect(card.getByText('d'.repeat(64))).toBeInTheDocument();
        expect(card.getByText('e'.repeat(64))).toBeInTheDocument();
        expect(diagnosticsCard).not.toHaveTextContent(
            'DISPLAY_CONTENT_CANARY_MUST_NOT_ENTER_DIAGNOSTICS',
        );
        expect(diagnosticsCard).toHaveTextContent('2026-08-03T00:00:01Z');
    });
});
