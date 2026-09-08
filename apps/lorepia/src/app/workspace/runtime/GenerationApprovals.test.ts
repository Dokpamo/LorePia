import { cleanup, render, screen, waitFor } from '@testing-library/svelte';
import { get } from 'svelte/store';
import { afterEach, expect, it, vi } from 'vitest';
import { t } from '../../../lib/i18n';
import { deferred } from '../../../tests/deferred';
import { GenerationAttemptApprovalController } from '../../../features/chat/generation-attempt-approval-controller';
import GenerationApprovals from './GenerationApprovals.svelte';

afterEach(cleanup);

it('keeps empty approval refreshes hidden and reveals actionable results and errors', async () => {
    const expiry = {
        conversation_id: 'room',
        source_branch_id: 'branch',
        decisions: [],
        has_more_due: false,
    };
    let pending = deferred<typeof expiry>();
    const client = {
        expireGenerationAttemptProposals: vi.fn(() => pending.promise),
        listGenerationAttemptProposals: vi.fn().mockResolvedValue([]),
        listRetryableGenerationAttempts: vi.fn().mockResolvedValue([]),
        decideGenerationAttemptProposal: vi.fn(),
    };
    const controller = new GenerationAttemptApprovalController(client);
    const props = {
        client,
        controller,
        conversationId: 'room',
        sourceBranchId: 'branch',
        hideWhenInactive: true,
    };
    const view = render(GenerationApprovals, props);
    const heading = () => screen.queryByRole('heading', { name: t('attempt_approval.title') });
    // Every rejected send refreshes the room, including sends without a provider.
    for (let refreshEpoch = 0; refreshEpoch < 3; refreshEpoch += 1) {
        if (refreshEpoch > 0) {
            pending = deferred<typeof expiry>();
            await view.rerender({ ...props, refreshEpoch });
        }
        await waitFor(() => expect(get(controller.state).phase).toBe('loading'));
        expect(heading()).not.toBeInTheDocument();
        pending.resolve(expiry);
        await waitFor(() => expect(get(controller.state).phase).toBe('ready'));
        expect(heading()).not.toBeInTheDocument();
    }
    pending = deferred<typeof expiry>();
    await view.rerender({ ...props, refreshEpoch: 3 });
    pending.resolve({ ...expiry, has_more_due: true });
    expect(await screen.findByText(t('attempt_approval.too_many'))).toBeInTheDocument();
    expect(heading()).toBeInTheDocument();
    client.expireGenerationAttemptProposals.mockRejectedValueOnce(new Error('failed'));
    await view.rerender({ ...props, refreshEpoch: 4 });
    expect(await screen.findByRole('alert')).toBeInTheDocument();
    expect(heading()).toBeInTheDocument();
    controller.destroy();
});
