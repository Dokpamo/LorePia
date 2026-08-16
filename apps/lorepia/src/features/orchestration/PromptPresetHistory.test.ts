import { cleanup, fireEvent, render, screen, waitFor } from '@testing-library/svelte';
import { afterEach, describe, expect, it, vi } from 'vitest';

import type { LorepiaClient, PromptPresetHistoryClientApi } from '../../lib/ipc/contracts';
import PromptPresetHistory from './PromptPresetHistory.svelte';

afterEach(cleanup);

const CURRENT_SHA = '3'.repeat(64);
const TARGET_SHA = '1'.repeat(64);
const DIFF_SHA = 'a'.repeat(64);
const REVIEW_SHA = 'b'.repeat(64);

function client(
    api: Partial<PromptPresetHistoryClientApi>,
): LorepiaClient & Partial<PromptPresetHistoryClientApi> {
    return api as LorepiaClient & Partial<PromptPresetHistoryClientApi>;
}

function revisions(rollbackAllowed: boolean) {
    return {
        revisions: [
            {
                revision_id: 'revision-1',
                revision: 1,
                sha256: TARGET_SHA,
                name: '첫 리비전',
                created_at: '2026-08-03T00:00:00Z',
                rollback_allowed: rollbackAllowed,
            },
            {
                revision_id: 'revision-3',
                revision: 3,
                sha256: CURRENT_SHA,
                name: '현재 리비전',
                created_at: '2026-08-03T00:02:00Z',
                rollback_allowed: rollbackAllowed,
            },
        ],
        truncated: false,
    };
}

describe('PromptPresetHistory', () => {
    it('renders application-built-in history with rollback controls disabled', async () => {
        const reviewPromptPresetRollback = vi.fn();
        render(PromptPresetHistory, {
            props: {
                client: client({
                    listPromptPresetRevisions: vi.fn().mockResolvedValue(revisions(false)),
                    reviewPromptPresetRollback,
                }),
                presetId: 'preset-built-in',
                currentRevision: 3,
            },
        });

        expect(
            await screen.findByText(
                '앱 내장 프롬프트 프리셋은 정책 보호를 위해 모든 롤백 동작이 비활성화됩니다.',
            ),
        ).toBeInTheDocument();
        expect(screen.getByRole('button', { name: '리비전 1 롤백 검토' })).toBeDisabled();
        expect(reviewPromptPresetRollback).not.toHaveBeenCalled();
    });

    it('shows the exact review hash before enabling explicit approval', async () => {
        const applyPromptPresetRollback = vi.fn();
        const diff = {
            preset_id: 'preset-1',
            from_revision_id: 'revision-3',
            from_revision: 3,
            from_sha256: CURRENT_SHA,
            to_revision_id: 'revision-1',
            to_revision: 1,
            to_sha256: TARGET_SHA,
            changed_paths: ['/blocks/0/template'],
            truncated: false,
            diff_sha256: DIFF_SHA,
        };
        render(PromptPresetHistory, {
            props: {
                client: client({
                    listPromptPresetRevisions: vi.fn().mockResolvedValue(revisions(true)),
                    diffPromptPresetRevisions: vi.fn().mockResolvedValue(diff),
                    reviewPromptPresetRollback: vi.fn().mockResolvedValue({
                        review_sha256: REVIEW_SHA,
                        preset_id: 'preset-1',
                        expected_current_state_revision: 3,
                        expected_current_revision_id: 'revision-3',
                        expected_current_sha256: CURRENT_SHA,
                        target_revision_id: 'revision-1',
                        target_revision: 1,
                        target_sha256: TARGET_SHA,
                        target_document_sha256: 'c'.repeat(64),
                        target_dependency_sha256: 'd'.repeat(64),
                        binding_snapshot_sha256: 'e'.repeat(64),
                        diff,
                        reviewed_at: '2026-08-03T00:03:00Z',
                    }),
                    applyPromptPresetRollback,
                }),
                presetId: 'preset-1',
                currentRevision: 3,
            },
        });

        await fireEvent.click(await screen.findByRole('button', { name: '리비전 1 롤백 검토' }));
        await waitFor(() => {
            expect(screen.getByText(REVIEW_SHA)).toBeInTheDocument();
        });
        expect(screen.getByRole('button', { name: '이 검토 해시로 롤백 승인' })).toBeEnabled();
        expect(applyPromptPresetRollback).not.toHaveBeenCalled();
    });
});
