import { get } from 'svelte/store';
import { describe, expect, it, vi } from 'vitest';

import type {
    GenerationAttemptProposalDecisionReceiptDto,
    GenerationAttemptProposalListItemDto,
    RetryableGenerationAttemptDto,
} from '../../lib/ipc/contracts';
import {
    GenerationAttemptApprovalController,
    MAX_GENERATION_ATTEMPT_RETRY_IDENTITIES,
    type GenerationAttemptApprovalCapableClient,
} from './generation-attempt-approval-controller';

function deferred<T>(): { promise: Promise<T>; resolve: (value: T) => void } {
    let resolvePromise!: (value: T) => void;
    return {
        promise: new Promise<T>((resolve) => {
            resolvePromise = resolve;
        }),
        resolve: (value: T) => resolvePromise(value),
    };
}

function pendingProposal(
    overrides: Partial<GenerationAttemptProposalListItemDto> = {},
): GenerationAttemptProposalListItemDto {
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
            title: '문을 연다',
            body: '현재 생성 시도에서 문을 여는 상태 변경을 승인합니다.',
            status: 'pending',
            source_interaction_state_revision: '7',
            requested_at_epoch_seconds: 1,
            expires_at_epoch_seconds: 30,
            decided_at_epoch_seconds: null,
        },
        ...overrides,
    };
}

function terminalReceipt(
    status: 'approved' | 'rejected' | 'expired' = 'approved',
    overrides: Partial<GenerationAttemptProposalDecisionReceiptDto> = {},
): GenerationAttemptProposalDecisionReceiptDto {
    return {
        ...pendingProposal(),
        aggregate_revision: '10',
        interaction_state_revision: status === 'approved' ? '8' : '7',
        pending_proposal_count: 0,
        proposal_revision: '4',
        proposal: {
            ...pendingProposal().proposal,
            status,
            decided_at_epoch_seconds: status === 'expired' ? 30 : 2,
        },
        approval_evidence_sha256: 'a'.repeat(64),
        exact_replay: false,
        ...overrides,
    };
}

function retryableAttempt(
    generationId = 'generation-1',
    overrides: Partial<RetryableGenerationAttemptDto> = {},
): RetryableGenerationAttemptDto {
    return {
        generation_id: generationId,
        status: 'before_generation_applied',
        created_at: '2026-08-10T00:00:00Z',
        updated_at: '2026-08-10T00:00:01Z',
        ...overrides,
    };
}

function capableClient(
    overrides: Partial<GenerationAttemptApprovalCapableClient> = {},
): GenerationAttemptApprovalCapableClient {
    return {
        expireGenerationAttemptProposals: vi.fn().mockResolvedValue({
            conversation_id: 'conversation-1',
            source_branch_id: 'branch-1',
            decisions: [],
            has_more_due: false,
        }),
        listGenerationAttemptProposals: vi.fn().mockResolvedValue([pendingProposal()]),
        listRetryableGenerationAttempts: vi.fn().mockResolvedValue([]),
        decideGenerationAttemptProposal: vi.fn().mockResolvedValue(terminalReceipt()),
        ...overrides,
    };
}

describe('GenerationAttemptApprovalController', () => {
    it('expires due attempt proposals before every bounded durable restore', async () => {
        const expire = vi.fn().mockResolvedValue({
            conversation_id: 'conversation-1',
            source_branch_id: 'branch-1',
            decisions: [],
            has_more_due: false,
        });
        const list = vi.fn().mockResolvedValue([pendingProposal()]);
        const listRetryable = vi.fn().mockResolvedValue([]);
        const controller = new GenerationAttemptApprovalController(
            capableClient({
                expireGenerationAttemptProposals: expire,
                listGenerationAttemptProposals: list,
                listRetryableGenerationAttempts: listRetryable,
            }),
        );

        await expect(controller.loadRoom('conversation-1', 'branch-1')).resolves.toBe(true);
        expect(expire).toHaveBeenCalledWith({
            conversation_id: 'conversation-1',
            source_branch_id: 'branch-1',
            limit: 100,
        });
        expect(expire.mock.invocationCallOrder[0] ?? Infinity).toBeLessThan(
            list.mock.invocationCallOrder[0] ?? Infinity,
        );
        expect(list).toHaveBeenCalledWith({
            conversation_id: 'conversation-1',
            source_branch_id: 'branch-1',
            status: 'pending',
            limit: 100,
        });
        expect(listRetryable).toHaveBeenCalledWith({
            conversation_id: 'conversation-1',
            source_branch_id: 'branch-1',
            limit: 100,
        });
        expect(list.mock.invocationCallOrder[0] ?? Infinity).toBeLessThan(
            listRetryable.mock.invocationCallOrder[0] ?? Infinity,
        );
        expect(get(controller.state)).toMatchObject({
            phase: 'ready',
            proposals: [{ generation_id: 'generation-1', proposal: { id: 'proposal-1' } }],
        });

        await expect(controller.reload()).resolves.toBe(true);
        expect(expire).toHaveBeenCalledTimes(2);
        expect(list).toHaveBeenCalledTimes(2);
        expect(listRetryable).toHaveBeenCalledTimes(2);
        expect(get(controller.state).proposals).toHaveLength(1);
        controller.destroy();
    });

    it('ignores a late restore from a previously selected room', async () => {
        const firstExpiry = deferred<{
            conversation_id: string;
            source_branch_id: string;
            decisions: GenerationAttemptProposalDecisionReceiptDto[];
            has_more_due: boolean;
        }>();
        const expire = vi
            .fn()
            .mockImplementationOnce(() => firstExpiry.promise)
            .mockResolvedValueOnce({
                conversation_id: 'conversation-2',
                source_branch_id: 'branch-2',
                decisions: [],
                has_more_due: false,
            });
        const list = vi.fn().mockResolvedValue([
            pendingProposal({
                conversation_id: 'conversation-2',
                source_branch_id: 'branch-2',
                proposed_branch_id: 'branch-2',
                generation_id: 'generation-2',
            }),
        ]);
        const controller = new GenerationAttemptApprovalController(
            capableClient({
                expireGenerationAttemptProposals: expire,
                listGenerationAttemptProposals: list,
            }),
        );

        const staleLoad = controller.loadRoom('conversation-1', 'branch-1');
        await expect(controller.loadRoom('conversation-2', 'branch-2')).resolves.toBe(true);
        firstExpiry.resolve({
            conversation_id: 'conversation-1',
            source_branch_id: 'branch-1',
            decisions: [],
            has_more_due: false,
        });
        await expect(staleLoad).resolves.toBe(false);
        expect(get(controller.state)).toMatchObject({
            phase: 'ready',
            conversation_id: 'conversation-2',
            source_branch_id: 'branch-2',
            proposals: [{ generation_id: 'generation-2' }],
        });
        controller.destroy();
    });

    it('binds an unscoped retry projection to the exact requested room operation epoch', async () => {
        const firstProjection = deferred<unknown>();
        const listRetryable = vi
            .fn()
            .mockImplementationOnce(() => firstProjection.promise)
            .mockResolvedValueOnce([retryableAttempt('generation-2')]);
        const expire = vi.fn().mockImplementation((input: { conversation_id: string }) =>
            Promise.resolve({
                conversation_id: input.conversation_id,
                source_branch_id:
                    input.conversation_id === 'conversation-1' ? 'branch-1' : 'branch-2',
                decisions: [],
                has_more_due: false,
            }),
        );
        const list = vi.fn().mockImplementation((input: { conversation_id: string }) =>
            Promise.resolve(
                input.conversation_id === 'conversation-1'
                    ? []
                    : [
                          pendingProposal({
                              conversation_id: 'conversation-2',
                              source_branch_id: 'branch-2',
                              proposed_branch_id: 'branch-2',
                              generation_id: 'generation-2',
                          }),
                      ],
            ),
        );
        const controller = new GenerationAttemptApprovalController(
            capableClient({
                expireGenerationAttemptProposals: expire,
                listGenerationAttemptProposals: list,
                listRetryableGenerationAttempts: listRetryable,
            }),
        );

        const staleLoad = controller.loadRoom('conversation-1', 'branch-1');
        await vi.waitFor(() => expect(listRetryable).toHaveBeenCalledTimes(1));
        await expect(controller.loadRoom('conversation-2', 'branch-2')).resolves.toBe(true);
        firstProjection.resolve([retryableAttempt('generation-1')]);

        await expect(staleLoad).resolves.toBe(false);
        expect(get(controller.state)).toMatchObject({
            phase: 'ready',
            conversation_id: 'conversation-2',
            source_branch_id: 'branch-2',
            retry_generation_ids: ['generation-2'],
        });
        expect(listRetryable).toHaveBeenNthCalledWith(1, {
            conversation_id: 'conversation-1',
            source_branch_id: 'branch-1',
            limit: 100,
        });
        expect(listRetryable).toHaveBeenNthCalledWith(2, {
            conversation_id: 'conversation-2',
            source_branch_id: 'branch-2',
            limit: 100,
        });
        controller.destroy();
    });

    it('restores both allowed retry phases from durable projection after controller recreation', async () => {
        const listRetryable = vi
            .fn()
            .mockResolvedValue([
                retryableAttempt('dispatch-attempt', { status: 'dispatch_ready' }),
                retryableAttempt('before-attempt'),
            ]);
        const client = capableClient({
            listGenerationAttemptProposals: vi.fn().mockResolvedValue([]),
            listRetryableGenerationAttempts: listRetryable,
        });
        const first = new GenerationAttemptApprovalController(client);

        await expect(first.loadRoom('conversation-1', 'branch-1')).resolves.toBe(true);
        expect(get(first.state)).toMatchObject({
            retry_generation_ids: ['dispatch-attempt', 'before-attempt'],
            retry_available: true,
        });
        first.destroy();

        const recreated = new GenerationAttemptApprovalController(client);
        await expect(recreated.loadRoom('conversation-1', 'branch-1')).resolves.toBe(true);
        expect(get(recreated.state)).toMatchObject({
            retry_generation_ids: ['dispatch-attempt', 'before-attempt'],
            retry_available: true,
        });
        expect(listRetryable).toHaveBeenCalledTimes(2);
        recreated.destroy();
    });

    it('rejects malformed retry projection and backend failure without retaining stale success', async () => {
        const valid = retryableAttempt();
        const missingTimestamp: Record<string, unknown> = { ...valid };
        delete missingTimestamp.updated_at;
        const malformed: unknown[] = [
            [{ ...valid, status: 'awaiting_approval' }],
            [{ ...valid, generation_id: 'generation-with\ncontrol' }],
            [{ ...valid, generation_id: 'a'.repeat(257) }],
            [{ ...valid, generation_id: '가'.repeat(200) }],
            [valid, { ...valid }],
            [{ ...valid, created_at: 'not-a-timestamp' }],
            [
                {
                    ...valid,
                    created_at: '2026-08-10T00:00:02Z',
                    updated_at: '2026-08-10T00:00:01Z',
                },
            ],
            [{ ...valid, leaked_operation_id: 'must-not-cross' }],
            [missingTimestamp],
            null,
        ];
        const listRetryable = vi
            .fn()
            .mockResolvedValueOnce([valid])
            .mockResolvedValueOnce(malformed[0])
            .mockResolvedValueOnce(malformed[1])
            .mockResolvedValueOnce(malformed[2])
            .mockResolvedValueOnce(malformed[3])
            .mockResolvedValueOnce(malformed[4])
            .mockResolvedValueOnce(malformed[5])
            .mockResolvedValueOnce(malformed[6])
            .mockResolvedValueOnce(malformed[7])
            .mockResolvedValueOnce(malformed[8])
            .mockResolvedValueOnce(malformed[9])
            .mockRejectedValueOnce(new Error('backend unavailable'));
        const controller = new GenerationAttemptApprovalController(
            capableClient({
                listGenerationAttemptProposals: vi.fn().mockResolvedValue([]),
                listRetryableGenerationAttempts: listRetryable,
            }),
        );

        await expect(controller.loadRoom('conversation-1', 'branch-1')).resolves.toBe(true);
        expect(get(controller.state).retry_generation_ids).toEqual(['generation-1']);

        for (const index of malformed.keys()) {
            await expect(controller.reload()).resolves.toBe(false);
            expect(get(controller.state)).toMatchObject({
                phase: 'error',
                proposals: [],
                retry_generation_ids: [],
                retry_available: false,
            });
            expect(listRetryable).toHaveBeenCalledTimes(index + 2);
        }
        await expect(controller.reload()).resolves.toBe(false);
        expect(get(controller.state)).toMatchObject({
            phase: 'error',
            proposals: [],
            retry_generation_ids: [],
            retry_available: false,
        });
        controller.destroy();
    });

    it('accepts 1,024 pending items per attempt but rejects malformed authority and pages beyond 100', async () => {
        const malformed = pendingProposal({ aggregate_revision: '09' });
        const list = vi
            .fn()
            .mockResolvedValueOnce([
                pendingProposal({
                    pending_proposal_count: 1_024,
                    proposal: {
                        ...pendingProposal().proposal,
                        source_interaction_state_revision: '0',
                    },
                }),
            ])
            .mockResolvedValueOnce([malformed])
            .mockResolvedValueOnce(
                Array.from({ length: 101 }, (_, index) =>
                    pendingProposal({
                        generation_id: `generation-${String(index)}`,
                        proposal: {
                            ...pendingProposal().proposal,
                            id: `proposal-${String(index)}`,
                        },
                    }),
                ),
            )
            .mockResolvedValueOnce([
                pendingProposal({
                    generation_id: 'generation-with\ncontrol',
                }),
            ]);
        const controller = new GenerationAttemptApprovalController(
            capableClient({ listGenerationAttemptProposals: list }),
        );

        await expect(controller.loadRoom('conversation-1', 'branch-1')).resolves.toBe(true);
        expect(get(controller.state)).toMatchObject({
            phase: 'ready',
            proposals: [{ pending_proposal_count: 1_024 }],
        });
        await expect(controller.loadRoom('conversation-1', 'branch-1')).resolves.toBe(false);
        expect(get(controller.state)).toMatchObject({ phase: 'error', proposals: [] });
        await expect(controller.loadRoom('conversation-1', 'branch-1')).resolves.toBe(false);
        expect(get(controller.state).phase).toBe('error');
        await expect(controller.loadRoom('conversation-1', 'branch-1')).resolves.toBe(false);
        expect(get(controller.state).phase).toBe('error');
        controller.destroy();
    });

    it('submits exact string CAS and accepts an exact replay only for the matching terminal decision', async () => {
        const target = pendingProposal({ pending_proposal_count: 2 });
        const decide = vi.fn().mockResolvedValue(
            terminalReceipt('approved', {
                aggregate_revision: '11',
                pending_proposal_count: 0,
                exact_replay: true,
            }),
        );
        const list = vi.fn().mockResolvedValueOnce([target]).mockResolvedValueOnce([]);
        const controller = new GenerationAttemptApprovalController(
            capableClient({
                listGenerationAttemptProposals: list,
                decideGenerationAttemptProposal: decide,
            }),
        );
        await controller.loadRoom('conversation-1', 'branch-1');

        await expect(
            controller.decideProposal('generation-1', 'proposal-1', 'approve'),
        ).resolves.toBe(true);
        expect(decide).toHaveBeenCalledWith({
            conversation_id: 'conversation-1',
            source_branch_id: 'branch-1',
            generation_id: 'generation-1',
            proposal_record_id: 'proposal-1',
            expected_aggregate_revision: '9',
            expected_proposal_revision: '3',
            decision: 'approve',
        });
        expect(get(controller.state)).toMatchObject({
            phase: 'ready',
            proposals: [],
            busy_proposal_key: null,
            retry_generation_ids: ['generation-1'],
            retry_available: true,
            error: null,
        });
        expect(get(controller.state).announcement).toContain('이미 반영된 결정을 확인했습니다.');
        expect(get(controller.state).announcement).toContain('생성을 다시 시도하세요.');
        expect(list).toHaveBeenCalledTimes(2);
        controller.destroy();
    });

    it('keeps intermediate evidence null, advances remaining CAS, and accepts final rejection evidence', async () => {
        const first = pendingProposal({ pending_proposal_count: 2 });
        const second = pendingProposal({
            pending_proposal_count: 2,
            proposal: {
                ...pendingProposal().proposal,
                id: 'proposal-2',
                title: '창문을 닫는다',
            },
        });
        const firstReceipt: GenerationAttemptProposalDecisionReceiptDto = {
            ...first,
            aggregate_revision: '10',
            interaction_state_revision: '8',
            pending_proposal_count: 1,
            proposal_revision: '4',
            proposal: {
                ...first.proposal,
                status: 'approved',
                decided_at_epoch_seconds: 2,
            },
            approval_evidence_sha256: null,
            exact_replay: false,
        };
        const lastReceipt: GenerationAttemptProposalDecisionReceiptDto = {
            ...second,
            aggregate_revision: '11',
            interaction_state_revision: '8',
            pending_proposal_count: 0,
            proposal_revision: '4',
            proposal: {
                ...second.proposal,
                status: 'rejected',
                decided_at_epoch_seconds: 3,
            },
            approval_evidence_sha256: 'b'.repeat(64),
            exact_replay: false,
        };
        const decide = vi
            .fn()
            .mockResolvedValueOnce(firstReceipt)
            .mockResolvedValueOnce(lastReceipt);
        const controller = new GenerationAttemptApprovalController(
            capableClient({
                listGenerationAttemptProposals: vi.fn().mockResolvedValue([first, second]),
                decideGenerationAttemptProposal: decide,
            }),
        );
        await controller.loadRoom('conversation-1', 'branch-1');

        await expect(
            controller.decideProposal('generation-1', 'proposal-1', 'approve'),
        ).resolves.toBe(true);
        expect(get(controller.state)).toMatchObject({
            retry_generation_ids: [],
            retry_available: false,
            proposals: [
                {
                    aggregate_revision: '10',
                    interaction_state_revision: '8',
                    pending_proposal_count: 1,
                    proposal: { id: 'proposal-2' },
                },
            ],
        });

        await expect(
            controller.decideProposal('generation-1', 'proposal-2', 'reject'),
        ).resolves.toBe(true);
        expect(decide).toHaveBeenLastCalledWith({
            conversation_id: 'conversation-1',
            source_branch_id: 'branch-1',
            generation_id: 'generation-1',
            proposal_record_id: 'proposal-2',
            expected_aggregate_revision: '10',
            expected_proposal_revision: '3',
            decision: 'reject',
        });
        expect(get(controller.state)).toMatchObject({
            proposals: [],
            retry_generation_ids: ['generation-1'],
            retry_available: true,
        });
        controller.destroy();
    });

    it('offers retry only after expiry seals the last proposal with evidence', async () => {
        const remaining = pendingProposal({
            aggregate_revision: '10',
            pending_proposal_count: 1,
            proposal: {
                ...pendingProposal().proposal,
                id: 'proposal-2',
            },
        });
        const intermediateExpiry = terminalReceipt('expired', {
            pending_proposal_count: 1,
            approval_evidence_sha256: null,
        });
        const lastExpiry = terminalReceipt('expired', {
            ...remaining,
            aggregate_revision: '11',
            pending_proposal_count: 0,
            proposal_revision: '4',
            proposal: {
                ...remaining.proposal,
                status: 'expired',
                decided_at_epoch_seconds: 30,
            },
            approval_evidence_sha256: 'c'.repeat(64),
            exact_replay: false,
        });
        const expire = vi
            .fn()
            .mockResolvedValueOnce({
                conversation_id: 'conversation-1',
                source_branch_id: 'branch-1',
                decisions: [intermediateExpiry],
                has_more_due: true,
            })
            .mockResolvedValueOnce({
                conversation_id: 'conversation-1',
                source_branch_id: 'branch-1',
                decisions: [lastExpiry],
                has_more_due: false,
            });
        const list = vi.fn().mockResolvedValueOnce([remaining]).mockResolvedValueOnce([]);
        const controller = new GenerationAttemptApprovalController(
            capableClient({
                expireGenerationAttemptProposals: expire,
                listGenerationAttemptProposals: list,
            }),
        );

        await expect(controller.loadRoom('conversation-1', 'branch-1')).resolves.toBe(true);
        expect(get(controller.state)).toMatchObject({
            has_more_due: true,
            retry_generation_ids: [],
            retry_available: false,
        });
        expect(get(controller.state).announcement).not.toContain('생성을 다시 시도하세요.');

        await expect(controller.reload()).resolves.toBe(true);
        expect(get(controller.state)).toMatchObject({
            has_more_due: false,
            proposals: [],
            retry_generation_ids: ['generation-1'],
            retry_available: true,
        });
        expect(get(controller.state).announcement).toContain('생성을 다시 시도하세요.');
        controller.destroy();
    });

    it('retains bounded unique terminal attempt identities in first-seen order across expiry pages', async () => {
        const generation2 = terminalReceipt('expired', {
            generation_id: 'generation-2',
            proposal: {
                ...terminalReceipt('expired').proposal,
                id: 'proposal-2',
            },
        });
        const generation1 = terminalReceipt('expired');
        const expire = vi
            .fn()
            .mockResolvedValueOnce({
                conversation_id: 'conversation-1',
                source_branch_id: 'branch-1',
                decisions: [generation2],
                has_more_due: true,
            })
            .mockResolvedValueOnce({
                conversation_id: 'conversation-1',
                source_branch_id: 'branch-1',
                decisions: [generation1],
                has_more_due: false,
            })
            .mockResolvedValueOnce({
                conversation_id: 'conversation-1',
                source_branch_id: 'branch-1',
                decisions: [generation2],
                has_more_due: false,
            });
        const listRetryable = vi
            .fn()
            .mockResolvedValueOnce([retryableAttempt('generation-2')])
            .mockResolvedValueOnce([
                retryableAttempt('generation-2'),
                retryableAttempt('generation-1'),
            ])
            .mockResolvedValueOnce([
                retryableAttempt('generation-2'),
                retryableAttempt('generation-1'),
            ]);
        const controller = new GenerationAttemptApprovalController(
            capableClient({
                expireGenerationAttemptProposals: expire,
                listGenerationAttemptProposals: vi.fn().mockResolvedValue([]),
                listRetryableGenerationAttempts: listRetryable,
            }),
        );

        await expect(controller.loadRoom('conversation-1', 'branch-1')).resolves.toBe(true);
        expect(get(controller.state)).toMatchObject({
            has_more_due: true,
            retry_generation_ids: ['generation-2'],
            retry_available: false,
        });

        await expect(controller.reload()).resolves.toBe(true);
        expect(get(controller.state)).toMatchObject({
            retry_generation_ids: ['generation-2', 'generation-1'],
            retry_available: true,
        });

        await expect(controller.reload()).resolves.toBe(true);
        expect(get(controller.state)).toMatchObject({
            retry_generation_ids: ['generation-2', 'generation-1'],
            retry_available: true,
        });
        controller.destroy();
    });

    it('fails closed without stale retry success when the durable projection exceeds its bound', async () => {
        const terminalAttempts = Array.from(
            { length: MAX_GENERATION_ATTEMPT_RETRY_IDENTITIES },
            (_, index) =>
                terminalReceipt('expired', {
                    generation_id: `generation-${String(index)}`,
                    proposal: {
                        ...terminalReceipt('expired').proposal,
                        id: `proposal-${String(index)}`,
                    },
                }),
        );
        const expire = vi
            .fn()
            .mockResolvedValueOnce({
                conversation_id: 'conversation-1',
                source_branch_id: 'branch-1',
                decisions: terminalAttempts,
                has_more_due: true,
            })
            .mockResolvedValueOnce({
                conversation_id: 'conversation-1',
                source_branch_id: 'branch-1',
                decisions: [
                    terminalReceipt('expired', {
                        generation_id: 'generation-overflow',
                        proposal: {
                            ...terminalReceipt('expired').proposal,
                            id: 'proposal-overflow',
                        },
                    }),
                ],
                has_more_due: false,
            });
        const listRetryable = vi
            .fn()
            .mockResolvedValueOnce(
                terminalAttempts.map((receipt) => retryableAttempt(receipt.generation_id)),
            )
            .mockResolvedValueOnce([
                ...terminalAttempts.map((receipt) => retryableAttempt(receipt.generation_id)),
                retryableAttempt('generation-overflow'),
            ]);
        const controller = new GenerationAttemptApprovalController(
            capableClient({
                expireGenerationAttemptProposals: expire,
                listGenerationAttemptProposals: vi.fn().mockResolvedValue([]),
                listRetryableGenerationAttempts: listRetryable,
            }),
        );

        await expect(controller.loadRoom('conversation-1', 'branch-1')).resolves.toBe(true);
        expect(get(controller.state).retry_generation_ids).toHaveLength(
            MAX_GENERATION_ATTEMPT_RETRY_IDENTITIES,
        );

        await expect(controller.reload()).resolves.toBe(false);
        expect(get(controller.state)).toMatchObject({
            phase: 'error',
            retry_generation_ids: [],
            retry_available: false,
        });
        controller.destroy();
    });

    it('keeps the reviewed proposal when a terminal receipt changes authority or status', async () => {
        const controller = new GenerationAttemptApprovalController(
            capableClient({
                decideGenerationAttemptProposal: vi.fn().mockResolvedValue(
                    terminalReceipt('rejected', {
                        generation_id: 'generation-other',
                    }),
                ),
            }),
        );
        await controller.loadRoom('conversation-1', 'branch-1');

        await expect(
            controller.decideProposal('generation-1', 'proposal-1', 'approve'),
        ).resolves.toBe(false);
        expect(get(controller.state)).toMatchObject({
            phase: 'ready',
            proposals: [{ generation_id: 'generation-1', proposal: { status: 'pending' } }],
            busy_proposal_key: null,
            retry_available: false,
        });
        expect(get(controller.state).announcement).toContain('제안 목록을 다시 불러오세요.');
        controller.destroy();
    });
});

it('keeps redacted generation proposals rejectable but never approvable', async () => {
    const redacted = pendingProposal({
        proposal: {
            ...pendingProposal().proposal,
            title: 'Stored proposal unavailable',
            body: 'The original proposal text cannot be displayed safely.',
            projection_rejection_reason: 'unsafe_native_text',
        },
    });
    const decide = vi.fn().mockResolvedValue(
        terminalReceipt('rejected', {
            ...redacted,
            aggregate_revision: '10',
            pending_proposal_count: 0,
            proposal_revision: '4',
            proposal: {
                ...redacted.proposal,
                status: 'rejected',
                decided_at_epoch_seconds: 2,
            },
        }),
    );
    const controller = new GenerationAttemptApprovalController(
        capableClient({
            listGenerationAttemptProposals: vi.fn().mockResolvedValue([redacted]),
            decideGenerationAttemptProposal: decide,
        }),
    );
    await controller.loadRoom('conversation-1', 'branch-1');

    await expect(controller.decideProposal('generation-1', 'proposal-1', 'approve')).resolves.toBe(
        false,
    );
    expect(decide).not.toHaveBeenCalled();
    expect(get(controller.state).announcement).toContain('승인할 수 없습니다');

    await expect(controller.decideProposal('generation-1', 'proposal-1', 'reject')).resolves.toBe(
        true,
    );
    expect(decide).toHaveBeenCalledWith({
        conversation_id: 'conversation-1',
        source_branch_id: 'branch-1',
        generation_id: 'generation-1',
        proposal_record_id: 'proposal-1',
        expected_aggregate_revision: '9',
        expected_proposal_revision: '3',
        decision: 'reject',
    });
    controller.destroy();
});
