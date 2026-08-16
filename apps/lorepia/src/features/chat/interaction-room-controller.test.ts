import { get } from 'svelte/store';
import { describe, expect, it, vi } from 'vitest';

import type {
    InteractionEffectEventDto,
    InteractionEffectHistoryItemDto,
    InteractionProposalListItemDto,
} from '../../lib/ipc/contracts';
import {
    InteractionRoomController,
    type InteractionRoomCapableClient,
} from './interaction-room-controller';

function choiceHistory(
    overrides: Partial<InteractionEffectHistoryItemDto> = {},
): InteractionEffectHistoryItemDto {
    return {
        effect_id: 'effect-choice-1',
        conversation_id: 'conversation-1',
        branch_id: 'branch-1',
        resulting_state_revision: 7,
        sequence: 2,
        event_created_at: '2026-08-03T00:00:00Z',
        replay_on_reopen: true,
        choice_status: 'pending',
        selected_choice_id: null,
        choice_decided_at_epoch_seconds: null,
        effect: {
            kind: 'present_choices',
            choices: [
                { id: 'choice-a', label: '왼쪽' },
                { id: 'choice-b', label: '오른쪽' },
            ],
        },
        ...overrides,
    };
}

function proposal(): InteractionProposalListItemDto {
    return {
        conversation_id: 'conversation-1',
        branch_id: 'branch-1',
        state_revision: 7,
        proposal_revision: 3,
        proposal: {
            id: 'proposal-1',
            title: '문을 연다',
            body: '상태를 변경합니다.',
            status: 'pending',
            source_interaction_state_revision: 7,
            requested_at_epoch_seconds: 1,
            expires_at_epoch_seconds: null,
            decided_at_epoch_seconds: null,
        },
    };
}

function capableClient(): InteractionRoomCapableClient & {
    emit: (delivery: InteractionEffectEventDto) => void;
    testMocks: {
        acknowledge: ReturnType<typeof vi.fn>;
        expireProposals: ReturnType<typeof vi.fn>;
        submitChoice: ReturnType<typeof vi.fn>;
        decideProposal: ReturnType<typeof vi.fn>;
    };
} {
    let listener: ((delivery: InteractionEffectEventDto) => void) | null = null;
    const acknowledge = vi.fn().mockResolvedValue(undefined);
    const expireProposals = vi.fn().mockResolvedValue({
        conversation_id: 'conversation-1',
        branch_id: 'branch-1',
        current_state_revision: 7,
        expired_proposals: [],
        has_more_expired: false,
    });
    const submitChoice = vi.fn().mockResolvedValue({
        choice_effect: choiceHistory({
            resulting_state_revision: 8,
            choice_status: 'consumed',
            selected_choice_id: 'choice-b',
            choice_decided_at_epoch_seconds: 2,
        }),
        resulting_state_revision: 8,
    });
    const decideProposal = vi.fn().mockResolvedValue({
        proposal: {
            ...proposal().proposal,
            status: 'approved',
            decided_at_epoch_seconds: 2,
        },
        state_revision: 8,
        effects: [],
    });
    const client = {
        listInteractionEffects: vi.fn().mockResolvedValue([]),
        subscribeInteractionEffects: vi.fn(
            (onDelivery: (delivery: InteractionEffectEventDto) => void) => {
                listener = onDelivery;
                return Promise.resolve(vi.fn());
            },
        ),
        acknowledgeInteractionEffect: acknowledge,
        retryInteractionEffect: vi.fn().mockResolvedValue(undefined),
        listReopenInteractionEffects: vi.fn().mockResolvedValue({
            current_state_revision: 7,
            items: [choiceHistory()],
            older_cursor: null,
        }),
        expireInteractionProposals: expireProposals,
        listInteractionProposals: vi.fn().mockResolvedValue([proposal()]),
        submitInteractionChoice: submitChoice,
        decideInteractionProposal: decideProposal,
        testMocks: { acknowledge, expireProposals, submitChoice, decideProposal },
        emit(delivery: InteractionEffectEventDto): void {
            listener?.(delivery);
        },
    };
    return client as unknown as InteractionRoomCapableClient & {
        emit: (delivery: InteractionEffectEventDto) => void;
        testMocks: {
            acknowledge: ReturnType<typeof vi.fn>;
            expireProposals: ReturnType<typeof vi.fn>;
            submitChoice: ReturnType<typeof vi.fn>;
            decideProposal: ReturnType<typeof vi.fn>;
        };
    };
}

describe('InteractionRoomController', () => {
    it('subscribes before restoring the newest bounded room snapshot and acknowledges live effects', async () => {
        const client = capableClient();
        const controller = new InteractionRoomController(client);

        await expect(controller.loadRoom('conversation-1', 'branch-1')).resolves.toBe(true);
        expect(client.testMocks.expireProposals).toHaveBeenCalledWith({
            conversation_id: 'conversation-1',
            branch_id: 'branch-1',
            limit: 100,
        });
        const listInteractionProposals = client.listInteractionProposals;
        if (listInteractionProposals === undefined) {
            throw new Error('capable client is missing proposal listing');
        }
        expect(
            client.testMocks.expireProposals.mock.invocationCallOrder[0] ?? Infinity,
        ).toBeLessThan(vi.mocked(listInteractionProposals).mock.invocationCallOrder[0] ?? Infinity);
        expect(client.listReopenInteractionEffects).toHaveBeenCalledWith({
            conversation_id: 'conversation-1',
            branch_id: 'branch-1',
            limit: 100,
        });
        expect(client.listInteractionProposals).toHaveBeenCalledWith({
            conversation_id: 'conversation-1',
            branch_id: 'branch-1',
            status: 'pending',
            limit: 100,
        });
        expect(get(controller.state)).toMatchObject({
            phase: 'ready',
            current_state_revision: 7,
            effects: [{ effect_id: 'effect-choice-1', choice_status: 'pending' }],
            pending_proposals: [{ proposal: { id: 'proposal-1' } }],
        });

        client.emit({
            delivery_id: 'delivery-1',
            effect_id: 'effect-event-1',
            conversation_id: 'conversation-1',
            branch_id: 'branch-1',
            resulting_state_revision: 8,
            event_created_at: '2026-08-03T00:00:01Z',
            effect: { kind: 'visible_system_event', text: '문이 열렸습니다.' },
        });
        await vi.waitFor(() => {
            expect(client.testMocks.acknowledge).toHaveBeenCalledWith('delivery-1');
        });
        expect(get(controller.state)).toMatchObject({
            current_state_revision: 8,
            effects: [{ effect_id: 'effect-choice-1' }, { effect_id: 'effect-event-1' }],
        });

        client.emit({
            delivery_id: 'delivery-other-room',
            effect_id: 'effect-other-room',
            conversation_id: 'conversation-2',
            branch_id: 'branch-2',
            resulting_state_revision: 9,
            event_created_at: '2026-08-03T00:00:02Z',
            effect: { kind: 'visible_system_event', text: '다른 방' },
        });
        expect(client.testMocks.acknowledge).not.toHaveBeenCalledWith('delivery-other-room');
        controller.destroy();
    });

    it('retains and acknowledges a content-free rejected legacy projection', async () => {
        const client = capableClient();
        const controller = new InteractionRoomController(client);
        await controller.loadRoom('conversation-1', 'branch-1');

        client.emit({
            delivery_id: 'delivery-rejected',
            effect_id: 'effect-rejected',
            conversation_id: 'conversation-1',
            branch_id: 'branch-1',
            resulting_state_revision: 8,
            event_created_at: '2026-08-03T00:00:01Z',
            effect: { kind: 'projection_rejected', reason: 'invalid_stored_effect' },
        });

        await vi.waitFor(() => {
            expect(client.testMocks.acknowledge).toHaveBeenCalledWith('delivery-rejected');
        });
        expect(get(controller.state)).toMatchObject({
            phase: 'ready',
            current_state_revision: 8,
            announcement: '호환되지 않는 저장 상호작용을 안전하게 숨겼습니다.',
            effects: [
                { effect_id: 'effect-choice-1' },
                {
                    effect_id: 'effect-rejected',
                    effect: { kind: 'projection_rejected', reason: 'invalid_stored_effect' },
                },
            ],
        });
        controller.destroy();
    });

    it('keeps redacted legacy proposals rejectable but never approvable', async () => {
        const client = capableClient();
        const redacted = {
            ...proposal(),
            proposal: {
                ...proposal().proposal,
                title: 'Stored proposal unavailable',
                body: 'The original proposal text cannot be displayed safely.',
                projection_rejection_reason: 'unsafe_native_text' as const,
            },
        };
        const listProposals = client.listInteractionProposals;
        if (listProposals === undefined) throw new Error('proposal listing unavailable');
        vi.mocked(listProposals).mockResolvedValueOnce([redacted]);
        client.testMocks.decideProposal.mockResolvedValueOnce({
            proposal: {
                ...redacted.proposal,
                status: 'rejected',
                decided_at_epoch_seconds: 2,
            },
            state_revision: 8,
            effects: [],
        });
        const controller = new InteractionRoomController(client);
        await controller.loadRoom('conversation-1', 'branch-1');

        await expect(controller.decideProposal('proposal-1', 'approve')).resolves.toBe(false);
        expect(client.testMocks.decideProposal).not.toHaveBeenCalled();
        expect(get(controller.state).announcement).toContain('승인할 수 없습니다');

        await expect(controller.decideProposal('proposal-1', 'reject')).resolves.toBe(true);
        expect(client.testMocks.decideProposal).toHaveBeenCalledWith({
            conversation_id: 'conversation-1',
            branch_id: 'branch-1',
            proposal_record_id: 'proposal-1',
            expected_state_revision: 7,
            expected_proposal_revision: 3,
            decision: 'reject',
        });
        controller.destroy();
    });

    it('expires ordinary due proposals before every room restore and reports an idempotent retry', async () => {
        const client = capableClient();
        const expired = {
            ...proposal(),
            state_revision: 8,
            proposal_revision: 4,
            proposal: {
                ...proposal().proposal,
                status: 'expired' as const,
                expires_at_epoch_seconds: 2,
                decided_at_epoch_seconds: 2,
            },
        };
        client.testMocks.expireProposals
            .mockResolvedValueOnce({
                conversation_id: 'conversation-1',
                branch_id: 'branch-1',
                current_state_revision: 8,
                expired_proposals: [expired],
                has_more_expired: false,
            })
            .mockResolvedValueOnce({
                conversation_id: 'conversation-1',
                branch_id: 'branch-1',
                current_state_revision: 8,
                expired_proposals: [],
                has_more_expired: false,
            });
        const listInteractionProposals = client.listInteractionProposals;
        const listReopenInteractionEffects = client.listReopenInteractionEffects;
        if (listInteractionProposals === undefined || listReopenInteractionEffects === undefined) {
            throw new Error('capable client is missing room restore methods');
        }
        vi.mocked(listInteractionProposals).mockResolvedValue([]);
        vi.mocked(listReopenInteractionEffects).mockResolvedValue({
            current_state_revision: 8,
            items: [],
            older_cursor: null,
        });
        const controller = new InteractionRoomController(client);

        await expect(controller.loadRoom('conversation-1', 'branch-1')).resolves.toBe(true);
        expect(get(controller.state)).toMatchObject({
            phase: 'ready',
            current_state_revision: 8,
            pending_proposals: [],
            announcement: '만료된 승인 제안을 정리했습니다. 생성을 다시 시도할 수 있습니다.',
        });

        await expect(controller.loadRoom('conversation-1', 'branch-1')).resolves.toBe(true);
        expect(client.testMocks.expireProposals).toHaveBeenCalledTimes(2);
        expect(get(controller.state)).toMatchObject({
            phase: 'ready',
            current_state_revision: 8,
            pending_proposals: [],
            error: null,
        });
        controller.destroy();
    });

    it('blocks ordinary decisions until every bounded page of expired proposals is drained', async () => {
        const client = capableClient();
        client.testMocks.expireProposals
            .mockResolvedValueOnce({
                conversation_id: 'conversation-1',
                branch_id: 'branch-1',
                current_state_revision: 7,
                expired_proposals: [],
                has_more_expired: true,
            })
            .mockResolvedValueOnce({
                conversation_id: 'conversation-1',
                branch_id: 'branch-1',
                current_state_revision: 7,
                expired_proposals: [],
                has_more_expired: false,
            });
        const controller = new InteractionRoomController(client);

        await expect(controller.loadRoom('conversation-1', 'branch-1')).resolves.toBe(true);
        expect(get(controller.state)).toMatchObject({
            phase: 'ready',
            has_more_expired_proposals: true,
            pending_proposals: [{ proposal: { id: 'proposal-1' } }],
        });
        await expect(controller.decideProposal('proposal-1', 'approve')).resolves.toBe(false);
        expect(client.testMocks.decideProposal).not.toHaveBeenCalled();

        await expect(controller.reload()).resolves.toBe(true);
        expect(get(controller.state).has_more_expired_proposals).toBe(false);
        await expect(controller.decideProposal('proposal-1', 'approve')).resolves.toBe(true);
        expect(client.testMocks.decideProposal).toHaveBeenCalledOnce();
        controller.destroy();
    });

    it('submits a choice with the snapshot current revision and consumes only the matching effect', async () => {
        const client = capableClient();
        const controller = new InteractionRoomController(client);
        await controller.loadRoom('conversation-1', 'branch-1');

        await expect(controller.submitChoice('effect-choice-1', 'choice-b')).resolves.toBe(true);
        expect(client.testMocks.submitChoice).toHaveBeenCalledWith({
            conversation_id: 'conversation-1',
            branch_id: 'branch-1',
            effect_id: 'effect-choice-1',
            choice_id: 'choice-b',
            expected_state_revision: 7,
        });
        expect(get(controller.state)).toMatchObject({
            current_state_revision: 8,
            effects: [
                {
                    effect_id: 'effect-choice-1',
                    choice_status: 'consumed',
                    selected_choice_id: 'choice-b',
                },
            ],
            busy_effect_id: null,
        });
        controller.destroy();
    });

    it('decides a pending proposal with both room and proposal CAS revisions', async () => {
        const client = capableClient();
        const controller = new InteractionRoomController(client);
        await controller.loadRoom('conversation-1', 'branch-1');

        await expect(controller.decideProposal('proposal-1', 'approve')).resolves.toBe(true);
        expect(client.testMocks.decideProposal).toHaveBeenCalledWith({
            conversation_id: 'conversation-1',
            branch_id: 'branch-1',
            proposal_record_id: 'proposal-1',
            expected_state_revision: 7,
            expected_proposal_revision: 3,
            decision: 'approve',
        });
        expect(get(controller.state)).toMatchObject({
            current_state_revision: 8,
            pending_proposals: [],
            busy_proposal_id: null,
        });
        controller.destroy();
    });

    it('retains the exact pending proposal when a decision receipt changes immutable authority', async () => {
        const client = capableClient();
        client.testMocks.decideProposal.mockResolvedValueOnce({
            proposal: {
                ...proposal().proposal,
                title: 'tampered title',
                status: 'approved',
                decided_at_epoch_seconds: 2,
            },
            state_revision: 8,
            effects: [],
        });
        const controller = new InteractionRoomController(client);
        await controller.loadRoom('conversation-1', 'branch-1');

        await expect(controller.decideProposal('proposal-1', 'approve')).resolves.toBe(false);
        expect(get(controller.state)).toMatchObject({
            pending_proposals: [{ proposal: { id: 'proposal-1', status: 'pending' } }],
            busy_proposal_id: null,
        });
        controller.destroy();
    });
});
