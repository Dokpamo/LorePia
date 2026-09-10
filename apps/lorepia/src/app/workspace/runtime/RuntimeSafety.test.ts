import { cleanup, fireEvent, render, screen, waitFor } from '@testing-library/svelte';
import { afterEach, expect, it, vi } from 'vitest';
import { t } from '../../../lib/i18n';
import type { LorepiaAppController, MemoryQueryRetryState } from '../../app-controller';
import GenerationApprovals from './GenerationApprovals.svelte';
import MemoryRetries from './MemoryRetries.svelte';

afterEach(cleanup);

it('keeps unsafe proposal text hidden, blocks approval, and sends exact rejection authority', async () => {
    const decide = vi.fn().mockRejectedValue(new Error('failed'));
    const client = {
        expireGenerationAttemptProposals: vi.fn().mockResolvedValue({
            conversation_id: 'room',
            source_branch_id: 'branch',
            decisions: [],
            has_more_due: false,
        }),
        listGenerationAttemptProposals: vi.fn().mockResolvedValue([
            {
                conversation_id: 'room',
                source_branch_id: 'branch',
                proposed_branch_id: 'branch',
                generation_id: 'generation',
                aggregate_revision: '9',
                interaction_state_revision: '7',
                pending_proposal_count: 1,
                proposal_revision: '3',
                proposal: {
                    id: 'proposal',
                    title: 'UNSAFE TITLE',
                    body: 'UNSAFE BODY',
                    status: 'pending',
                    source_interaction_state_revision: '7',
                    requested_at_epoch_seconds: 1,
                    expires_at_epoch_seconds: null,
                    decided_at_epoch_seconds: null,
                    projection_rejection_reason: 'unsafe_native_text',
                },
            },
        ]),
        listRetryableGenerationAttempts: vi.fn().mockResolvedValue([]),
        decideGenerationAttemptProposal: decide,
    };
    const onRetry = vi.fn();
    render(GenerationApprovals, {
        client,
        conversationId: 'room',
        sourceBranchId: 'branch',
        onRetry,
        hideWhenInactive: true,
    });
    const approve = await screen.findByRole('button', {
        name: t('attempt_approval.approve.label', { index: 1 }),
    });
    expect(approve).toBeDisabled();
    expect(screen.queryByText('UNSAFE BODY')).not.toBeInTheDocument();
    await fireEvent.click(
        screen.getByRole('button', { name: t('attempt_approval.reject.label', { index: 1 }) }),
    );
    await waitFor(() =>
        expect(decide).toHaveBeenCalledWith({
            conversation_id: 'room',
            source_branch_id: 'branch',
            generation_id: 'generation',
            proposal_record_id: 'proposal',
            expected_aggregate_revision: '9',
            expected_proposal_revision: '3',
            decision: 'reject',
        }),
    );
    expect(await screen.findByRole('alert')).toBeInTheDocument();
    expect(onRetry).not.toHaveBeenCalled();
});

it('requires a fresh unknown-outcome review after a memory candidate revision changes', async () => {
    const candidate = {
        id: 'candidate',
        status: 'interrupted' as const,
        revision: 4,
        conversation_id: 'room',
        branch_id: 'branch',
        error_code: null,
        requires_unknown_outcome_acknowledgement: true,
    };
    const retry = vi.fn().mockResolvedValue(true);
    const controller = {
        retryMemoryQueryEmbedding: retry,
        refreshMemoryQueryRetries: vi.fn(),
    } as unknown as LorepiaAppController;
    const state: MemoryQueryRetryState = {
        phase: 'ready',
        error: null,
        notice: null,
        candidates: [candidate],
        interrupted_jobs: [],
        busy_id: null,
    };
    const view = render(MemoryRetries, { state, controller, headingId: 'memory-test' });
    await fireEvent.click(screen.getByRole('button', { name: t('memory.retry.review') }));
    expect(retry).not.toHaveBeenCalled();
    expect(screen.getByText(t('memory.retry.ack.embedding'))).toBeInTheDocument();
    const updated = { ...candidate, revision: 5 };
    await view.rerender({
        state: { ...state, candidates: [updated] },
        controller,
        headingId: 'memory-test',
    });
    expect(
        screen.queryByRole('button', { name: t('memory.retry.confirm') }),
    ).not.toBeInTheDocument();
    await fireEvent.click(screen.getByRole('button', { name: t('memory.retry.review') }));
    await fireEvent.click(screen.getByRole('button', { name: t('memory.retry.confirm') }));
    expect(retry).toHaveBeenCalledExactlyOnceWith(updated, true);
});
