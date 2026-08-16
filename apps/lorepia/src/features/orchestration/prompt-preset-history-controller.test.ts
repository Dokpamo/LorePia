import { get } from 'svelte/store';
import { describe, expect, it, vi } from 'vitest';

import type {
    PromptPresetHistoryClientApi,
    PromptPresetRevisionDiffDto,
    PromptPresetRevisionListDto,
    PromptPresetRollbackReceiptDto,
    PromptPresetRollbackReviewDto,
} from '../../lib/ipc/contracts';
import {
    PromptPresetHistoryController,
    type PromptPresetHistoryCapableClient,
} from './prompt-preset-history-controller';

const SHA = {
    one: '1'.repeat(64),
    two: '2'.repeat(64),
    three: '3'.repeat(64),
    four: '4'.repeat(64),
    diff: 'a'.repeat(64),
    review: 'b'.repeat(64),
    document: 'c'.repeat(64),
    dependency: 'd'.repeat(64),
    bindings: 'e'.repeat(64),
    approval: 'f'.repeat(64),
};

function revisionList(rollbackAllowed = true, includeApplied = false): PromptPresetRevisionListDto {
    return {
        revisions: [
            {
                revision_id: 'revision-1',
                revision: 1,
                sha256: SHA.one,
                name: 'Preset revision 1',
                created_at: '2026-08-03T00:00:00Z',
                rollback_allowed: rollbackAllowed,
            },
            {
                revision_id: 'revision-2',
                revision: 2,
                sha256: SHA.two,
                name: 'Preset revision 2',
                created_at: '2026-08-03T00:01:00Z',
                rollback_allowed: rollbackAllowed,
            },
            {
                revision_id: 'revision-3',
                revision: 3,
                sha256: SHA.three,
                name: 'Preset revision 3',
                created_at: '2026-08-03T00:02:00Z',
                rollback_allowed: rollbackAllowed,
            },
            ...(includeApplied
                ? [
                      {
                          revision_id: 'revision-4',
                          revision: 4,
                          sha256: SHA.four,
                          name: 'Preset revision 4',
                          created_at: '2026-08-03T00:03:00Z',
                          rollback_allowed: rollbackAllowed,
                      },
                  ]
                : []),
        ],
        truncated: false,
    };
}

function revisionDiff(): PromptPresetRevisionDiffDto {
    return {
        preset_id: 'preset-1',
        from_revision_id: 'revision-3',
        from_revision: 3,
        from_sha256: SHA.three,
        to_revision_id: 'revision-1',
        to_revision: 1,
        to_sha256: SHA.one,
        changed_paths: ['/blocks/0/template'],
        truncated: false,
        diff_sha256: SHA.diff,
    };
}

function rollbackReview(
    diff: PromptPresetRevisionDiffDto = revisionDiff(),
): PromptPresetRollbackReviewDto {
    return {
        review_sha256: SHA.review,
        preset_id: 'preset-1',
        expected_current_state_revision: 3,
        expected_current_revision_id: 'revision-3',
        expected_current_sha256: SHA.three,
        target_revision_id: 'revision-1',
        target_revision: 1,
        target_sha256: SHA.one,
        target_document_sha256: SHA.document,
        target_dependency_sha256: SHA.dependency,
        binding_snapshot_sha256: SHA.bindings,
        diff,
        reviewed_at: '2026-08-03T00:04:00Z',
    };
}

function rollbackReceipt(): PromptPresetRollbackReceiptDto {
    return {
        preset_id: 'preset-1',
        target_revision: 1,
        applied_revision_id: 'revision-4',
        applied_revision: 4,
        applied_sha256: SHA.four,
        review_sha256: SHA.review,
        approval_id: 'approval-1',
        approval_sha256: SHA.approval,
        approved_at: '2026-08-03T00:05:00Z',
    };
}

function capableClient(
    api: Partial<PromptPresetHistoryClientApi>,
): PromptPresetHistoryCapableClient {
    return api as PromptPresetHistoryCapableClient;
}

describe('PromptPresetHistoryController', () => {
    it('reviews exact hashes and applies only the minimal confirmation input', async () => {
        const listPromptPresetRevisions = vi
            .fn<PromptPresetHistoryClientApi['listPromptPresetRevisions']>()
            .mockResolvedValueOnce(revisionList())
            .mockResolvedValueOnce(revisionList(true, true));
        const diffPromptPresetRevisions = vi
            .fn<PromptPresetHistoryClientApi['diffPromptPresetRevisions']>()
            .mockResolvedValue(revisionDiff());
        const reviewPromptPresetRollback = vi
            .fn<PromptPresetHistoryClientApi['reviewPromptPresetRollback']>()
            .mockResolvedValue(rollbackReview());
        const applyPromptPresetRollback = vi
            .fn<PromptPresetHistoryClientApi['applyPromptPresetRollback']>()
            .mockResolvedValue(rollbackReceipt());
        const controller = new PromptPresetHistoryController(
            capableClient({
                listPromptPresetRevisions,
                diffPromptPresetRevisions,
                reviewPromptPresetRollback,
                applyPromptPresetRollback,
            }),
            () => 'approval-1',
        );

        await controller.load('preset-1', 3);
        expect(await controller.reviewTarget(1)).toBe(true);
        expect(await controller.applyReviewedRollback()).toEqual(rollbackReceipt());

        expect(diffPromptPresetRevisions).toHaveBeenCalledWith({
            prompt_preset_id: 'preset-1',
            from_revision: 3,
            to_revision: 1,
        });
        expect(reviewPromptPresetRollback).toHaveBeenCalledWith({
            prompt_preset_id: 'preset-1',
            expected_current_revision: 3,
            target_revision: 1,
        });
        expect(applyPromptPresetRollback).toHaveBeenCalledWith({
            prompt_preset_id: 'preset-1',
            expected_current_revision: 3,
            target_revision: 1,
            approval_id: 'approval-1',
            expected_review_sha256: SHA.review,
        });
        expect(JSON.stringify(applyPromptPresetRollback.mock.calls)).not.toContain(
            'target_document_sha256',
        );
        expect(JSON.stringify(applyPromptPresetRollback.mock.calls)).not.toContain('diff_sha256');
        expect(get(controller.state)).toMatchObject({
            phase: 'ready',
            current_revision: 4,
            review: null,
            receipt: rollbackReceipt(),
            error: null,
        });
    });

    it('disables application-built-in history before calling review commands', async () => {
        const diffPromptPresetRevisions = vi.fn();
        const reviewPromptPresetRollback = vi.fn();
        const controller = new PromptPresetHistoryController(
            capableClient({
                listPromptPresetRevisions: vi.fn().mockResolvedValue(revisionList(false)),
                diffPromptPresetRevisions,
                reviewPromptPresetRollback,
            }),
        );

        await controller.load('preset-1', 3);
        expect(await controller.reviewTarget(1)).toBe(false);
        expect(diffPromptPresetRevisions).not.toHaveBeenCalled();
        expect(reviewPromptPresetRollback).not.toHaveBeenCalled();
        expect(get(controller.state)).toMatchObject({
            phase: 'error',
            review: null,
            receipt: null,
            error: '앱 내장 프롬프트 프리셋은 롤백할 수 없습니다.',
        });
    });

    it('rejects a mismatched review without exposing an apply path', async () => {
        const mismatched = revisionDiff();
        mismatched.diff_sha256 = '9'.repeat(64);
        const applyPromptPresetRollback = vi.fn();
        const controller = new PromptPresetHistoryController(
            capableClient({
                listPromptPresetRevisions: vi.fn().mockResolvedValue(revisionList()),
                diffPromptPresetRevisions: vi.fn().mockResolvedValue(revisionDiff()),
                reviewPromptPresetRollback: vi.fn().mockResolvedValue(rollbackReview(mismatched)),
                applyPromptPresetRollback,
            }),
        );

        await controller.load('preset-1', 3);
        expect(await controller.reviewTarget(1)).toBe(false);
        expect(await controller.applyReviewedRollback()).toBeNull();
        expect(applyPromptPresetRollback).not.toHaveBeenCalled();
        expect(get(controller.state)).toMatchObject({
            phase: 'error',
            review: null,
            receipt: null,
        });
    });

    it('keeps a caller-stable approval ID but never reports stale apply rejection as success', async () => {
        const applyPromptPresetRollback = vi
            .fn<PromptPresetHistoryClientApi['applyPromptPresetRollback']>()
            .mockRejectedValue({
                code: 'stale_revision',
                message_key: 'error.stale_revision',
                recoverable: true,
                operation_id: null,
                field_errors: [],
            });
        const controller = new PromptPresetHistoryController(
            capableClient({
                listPromptPresetRevisions: vi.fn().mockResolvedValue(revisionList()),
                diffPromptPresetRevisions: vi.fn().mockResolvedValue(revisionDiff()),
                reviewPromptPresetRollback: vi.fn().mockResolvedValue(rollbackReview()),
                applyPromptPresetRollback,
            }),
            () => 'approval-stable',
        );

        await controller.load('preset-1', 3);
        await controller.reviewTarget(1);
        expect(await controller.applyReviewedRollback()).toBeNull();
        expect(await controller.applyReviewedRollback()).toBeNull();

        expect(applyPromptPresetRollback).toHaveBeenCalledTimes(2);
        expect(applyPromptPresetRollback.mock.calls[0]?.[0].approval_id).toBe('approval-stable');
        expect(applyPromptPresetRollback.mock.calls[1]?.[0].approval_id).toBe('approval-stable');
        expect(get(controller.state)).toMatchObject({
            phase: 'error',
            current_revision: 3,
            receipt: null,
            approval_id: 'approval-stable',
            error: 'error.stale_revision',
            announcement: '',
        });
    });

    it('fails closed when an apply response does not prove a new revision', async () => {
        const invalidReceipt = rollbackReceipt();
        invalidReceipt.applied_revision = 3;
        const controller = new PromptPresetHistoryController(
            capableClient({
                listPromptPresetRevisions: vi.fn().mockResolvedValue(revisionList()),
                diffPromptPresetRevisions: vi.fn().mockResolvedValue(revisionDiff()),
                reviewPromptPresetRollback: vi.fn().mockResolvedValue(rollbackReview()),
                applyPromptPresetRollback: vi.fn().mockResolvedValue(invalidReceipt),
            }),
            () => 'approval-1',
        );

        await controller.load('preset-1', 3);
        await controller.reviewTarget(1);
        expect(await controller.applyReviewedRollback()).toBeNull();
        expect(get(controller.state)).toMatchObject({
            phase: 'error',
            current_revision: 3,
            receipt: null,
            announcement: '',
        });
    });
});
