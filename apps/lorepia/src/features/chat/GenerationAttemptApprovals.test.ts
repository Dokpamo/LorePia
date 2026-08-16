import { cleanup, fireEvent, render, screen, waitFor } from '@testing-library/svelte';
import { afterEach, describe, expect, it, vi } from 'vitest';

import type {
    GenerationAttemptProposalDecisionReceiptDto,
    GenerationAttemptProposalListItemDto,
} from '../../lib/ipc/contracts';
import { LiveLorepiaClient, type LorepiaTransport } from '../../lib/ipc/client';
import GenerationAttemptApprovals from './GenerationAttemptApprovals.svelte';
import type { GenerationAttemptApprovalCapableClient } from './generation-attempt-approval-controller';

afterEach(cleanup);

function proposal(): GenerationAttemptProposalListItemDto {
    return {
        conversation_id: 'conversation-1',
        source_branch_id: 'branch-1',
        proposed_branch_id: 'branch-1',
        generation_id: 'generation-1',
        aggregate_revision: '9',
        interaction_state_revision: '7',
        pending_proposal_count: 1,
        proposal_revision: '3',
        proposal: {
            id: 'proposal-1',
            title: '비밀 문 열기',
            body: '이번 생성 시도에서 문을 여는 상태 변경입니다.',
            status: 'pending',
            source_interaction_state_revision: '7',
            requested_at_epoch_seconds: 1,
            expires_at_epoch_seconds: null,
            decided_at_epoch_seconds: null,
        },
    };
}

function approvedReceipt(): GenerationAttemptProposalDecisionReceiptDto {
    return {
        ...proposal(),
        aggregate_revision: '10',
        interaction_state_revision: '8',
        pending_proposal_count: 0,
        proposal_revision: '4',
        proposal: {
            ...proposal().proposal,
            status: 'approved',
            decided_at_epoch_seconds: 2,
        },
        approval_evidence_sha256: 'a'.repeat(64),
        exact_replay: false,
    };
}

function expiredReceipt(
    generationId: string,
    proposalId: string,
): GenerationAttemptProposalDecisionReceiptDto {
    return {
        ...approvedReceipt(),
        generation_id: generationId,
        proposal: {
            ...approvedReceipt().proposal,
            id: proposalId,
            status: 'expired',
            expires_at_epoch_seconds: 2,
            decided_at_epoch_seconds: 2,
        },
    };
}

describe('GenerationAttemptApprovals', () => {
    it('recreates through the live command adapter and retries only the clicked projected attempt', async () => {
        const invoke = vi.fn((commandName: string): Promise<unknown> => {
            switch (commandName) {
                case 'expire_generation_attempt_proposals':
                    return Promise.resolve({
                        conversation_id: 'conversation-1',
                        source_branch_id: 'branch-1',
                        decisions: [],
                        has_more_due: false,
                    });
                case 'list_generation_attempt_proposals':
                    return Promise.resolve([]);
                case 'list_retryable_generation_attempts':
                    return Promise.resolve([
                        {
                            generation_id: 'generation-projected',
                            status: 'dispatch_ready',
                            created_at: '2026-08-10T00:00:00Z',
                            updated_at: '2026-08-10T00:00:01Z',
                        },
                    ]);
                default:
                    return Promise.reject(new Error(`unexpected command: ${commandName}`));
            }
        });
        const transport: LorepiaTransport = {
            invoke,
            createChatChannel: vi.fn(),
            listen: vi.fn().mockResolvedValue(() => undefined),
        };
        const client = new LiveLorepiaClient(transport);
        const onRetry = vi.fn();
        const props = {
            client,
            conversationId: 'conversation-1',
            sourceBranchId: 'branch-1',
            onRetry,
            retryLabel: '정확한 시도 재개',
        };

        const first = render(GenerationAttemptApprovals, { props });
        expect(
            await screen.findByRole('button', {
                name: '정확한 시도 재개: 생성 시도 generation-projected',
            }),
        ).toBeInTheDocument();
        expect(onRetry).not.toHaveBeenCalled();
        first.unmount();

        render(GenerationAttemptApprovals, { props });
        const retry = await screen.findByRole('button', {
            name: '정확한 시도 재개: 생성 시도 generation-projected',
        });
        const expectedLoadCalls = [
            [
                'expire_generation_attempt_proposals',
                {
                    request: {
                        conversation_id: 'conversation-1',
                        source_branch_id: 'branch-1',
                        limit: 100,
                    },
                },
            ],
            [
                'list_generation_attempt_proposals',
                {
                    request: {
                        conversation_id: 'conversation-1',
                        source_branch_id: 'branch-1',
                        status: 'pending',
                        limit: 100,
                    },
                },
            ],
            [
                'list_retryable_generation_attempts',
                {
                    request: {
                        conversation_id: 'conversation-1',
                        source_branch_id: 'branch-1',
                        limit: 100,
                    },
                },
            ],
        ];
        expect(invoke.mock.calls).toEqual([...expectedLoadCalls, ...expectedLoadCalls]);
        expect(invoke.mock.calls.some(([commandName]) => commandName.includes('send'))).toBe(false);
        expect(onRetry).not.toHaveBeenCalled();

        await fireEvent.click(retry);
        expect(onRetry).toHaveBeenCalledOnce();
        expect(onRetry).toHaveBeenCalledWith('generation-projected');
        expect(invoke.mock.calls).toEqual([...expectedLoadCalls, ...expectedLoadCalls]);
    });

    it('renders accessible reviewed cards, submits exact approval, and requires an explicit retry', async () => {
        const item = proposal();
        Object.assign(item.proposal, { action_payload: 'payload-must-never-be-rendered' });
        const decide = vi.fn().mockResolvedValue(approvedReceipt());
        const onRetry = vi.fn();
        const expire = vi.fn().mockResolvedValue({
            conversation_id: 'conversation-1',
            source_branch_id: 'branch-1',
            decisions: [],
            has_more_due: false,
        });
        const client: GenerationAttemptApprovalCapableClient = {
            expireGenerationAttemptProposals: expire,
            listGenerationAttemptProposals: vi.fn().mockResolvedValue([item]),
            listRetryableGenerationAttempts: vi.fn().mockResolvedValue([]),
            decideGenerationAttemptProposal: decide,
        };
        const rendered = render(GenerationAttemptApprovals, {
            props: {
                client,
                conversationId: 'conversation-1',
                sourceBranchId: 'branch-1',
                onRetry,
                retryLabel: '보낼 메시지 확인',
            },
        });

        expect(await screen.findByRole('heading', { name: '비밀 문 열기' })).toBeInTheDocument();
        expect(
            screen.getByText('이번 생성 시도에서 문을 여는 상태 변경입니다.'),
        ).toBeInTheDocument();
        expect(screen.queryByText('payload-must-never-be-rendered')).not.toBeInTheDocument();
        expect(onRetry).not.toHaveBeenCalled();

        await rendered.rerender({
            client,
            conversationId: 'conversation-1',
            sourceBranchId: 'branch-1',
            refreshEpoch: 1,
            onRetry,
            retryLabel: '보낼 메시지 확인',
        });
        await waitFor(() => expect(expire).toHaveBeenCalledTimes(2));
        expect(onRetry).not.toHaveBeenCalled();

        await fireEvent.click(screen.getByRole('button', { name: '제안 1 승인' }));
        await waitFor(() => {
            expect(decide).toHaveBeenCalledWith({
                conversation_id: 'conversation-1',
                source_branch_id: 'branch-1',
                generation_id: 'generation-1',
                proposal_record_id: 'proposal-1',
                expected_aggregate_revision: '9',
                expected_proposal_revision: '3',
                decision: 'approve',
            });
        });
        const retry = await screen.findByRole('button', {
            name: '보낼 메시지 확인: 생성 시도 generation-1',
        });
        expect(screen.queryByRole('heading', { name: '비밀 문 열기' })).not.toBeInTheDocument();
        expect(onRetry).not.toHaveBeenCalled();

        await fireEvent.click(retry);
        expect(onRetry).toHaveBeenCalledOnce();
        expect(onRetry).toHaveBeenCalledWith('generation-1');
    });

    it('renders deterministic unambiguous retry actions for each exact terminal attempt ID', async () => {
        const onRetry = vi.fn();
        const client: GenerationAttemptApprovalCapableClient = {
            expireGenerationAttemptProposals: vi.fn().mockResolvedValue({
                conversation_id: 'conversation-1',
                source_branch_id: 'branch-1',
                decisions: [
                    expiredReceipt('generation-2', 'proposal-2'),
                    expiredReceipt('generation-1', 'proposal-1'),
                ],
                has_more_due: false,
            }),
            listGenerationAttemptProposals: vi.fn().mockResolvedValue([]),
            listRetryableGenerationAttempts: vi.fn().mockResolvedValue([]),
            decideGenerationAttemptProposal: vi.fn(),
        };
        render(GenerationAttemptApprovals, {
            props: {
                client,
                conversationId: 'conversation-1',
                sourceBranchId: 'branch-1',
                onRetry,
                retryLabel: '정확한 시도 재개',
            },
        });

        const retryActions = await screen.findAllByRole('button', {
            name: /정확한 시도 재개: 생성 시도 generation-/,
        });
        expect(retryActions.map((button) => button.getAttribute('aria-label'))).toEqual([
            '정확한 시도 재개: 생성 시도 generation-2',
            '정확한 시도 재개: 생성 시도 generation-1',
        ]);
        expect(onRetry).not.toHaveBeenCalled();

        const secondRetryAction = retryActions.at(1);
        expect(secondRetryAction).toBeDefined();
        if (secondRetryAction === undefined) throw new Error('missing second retry action');
        await fireEvent.click(secondRetryAction);
        expect(onRetry).toHaveBeenCalledOnce();
        expect(onRetry).toHaveBeenCalledWith('generation-1');
    });
});
