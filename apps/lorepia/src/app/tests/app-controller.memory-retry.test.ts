import { get } from 'svelte/store';
import { afterEach, describe, expect, it, vi } from 'vitest';

import type { ConversationBranchDto, ConversationStateDto } from '../../lib/ipc/contracts';
import { LorepiaAppController } from '../app-controller';
import { createAppControllerFixture } from './app-controller-test-support';

const { character, conversation, conversationState, branch, mockClient } =
    createAppControllerFixture();

afterEach(() => {
    vi.useRealTimers();
});

describe('LorepiaAppController memory query retry', () => {
    it.each(['edit', 'regenerate'] as const)(
        'refreshes retry candidates for the new branch after a successful %s',
        async (action) => {
            const nextBranch: ConversationBranchDto = {
                ...branch,
                id: 'branch-2',
                fork_message_id: 'message-user',
            };
            const nextConversationState: ConversationStateDto = {
                ...conversationState,
                active_branch_id: nextBranch.id,
            };
            const staleCandidate = {
                id: 'query-embedding-old-branch',
                status: 'failed' as const,
                revision: 3,
                conversation_id: conversation.id,
                branch_id: branch.id,
                error_code: 'provider_unavailable',
                requires_unknown_outcome_acknowledgement: false,
            };
            const nextCandidate = {
                ...staleCandidate,
                id: 'query-embedding-new-branch',
                revision: 1,
                branch_id: nextBranch.id,
            };
            const list = vi.fn(({ branch_id }: { branch_id: string }) =>
                Promise.resolve(branch_id === nextBranch.id ? [nextCandidate] : [staleCandidate]),
            );
            const startBranchGeneration = vi.fn(() =>
                Promise.resolve({ branch: nextBranch, generation_id: 'generation-2' }),
            );
            const client = mockClient({
                listRetryableMemoryQueryEmbeddings: list,
                listBranchMessages: () => Promise.resolve([]),
                selectBranch: () => Promise.resolve(nextConversationState),
                editUserMessage: startBranchGeneration,
                regenerateAssistantMessage: startBranchGeneration,
            });
            const controller = new LorepiaAppController(client);
            await controller.start();
            await controller.selectCharacter(character);
            await controller.selectConversation(conversation);
            await vi.waitFor(() =>
                expect(get(controller.state).memory_query_retries.candidates).toEqual([
                    staleCandidate,
                ]),
            );

            const succeeded =
                action === 'edit'
                    ? await controller.editUserMessage('message-user', '고친 메시지')
                    : await controller.regenerateAssistantMessage('message-assistant');

            expect(succeeded).toBe(true);
            expect(get(controller.state).conversation_state?.active_branch_id).toBe(nextBranch.id);
            await vi.waitFor(() =>
                expect(get(controller.state).memory_query_retries.candidates).toEqual([
                    nextCandidate,
                ]),
            );
            expect(get(controller.state).memory_query_retries.candidates).not.toContainEqual(
                staleCandidate,
            );
            expect(list).toHaveBeenCalledWith({
                conversation_id: conversation.id,
                branch_id: nextBranch.id,
                limit: 16,
            });
            controller.destroy();
        },
    );

    it('requires positive unknown-outcome acknowledgement and preserves the exact CAS revision', async () => {
        const candidate = {
            id: 'query-embedding-1',
            status: 'interrupted' as const,
            revision: 4,
            conversation_id: conversation.id,
            branch_id: branch.id,
            error_code: 'provider_unavailable',
            requires_unknown_outcome_acknowledgement: true,
        };
        const retry = vi.fn().mockResolvedValue({
            id: 'query-embedding-1',
            status: 'queued',
            revision: 5,
            conversation_id: conversation.id,
            branch_id: branch.id,
            error_code: null,
            requires_unknown_outcome_acknowledgement: false,
        });
        const list = vi.fn().mockResolvedValue([candidate]);
        const client = mockClient({
            listRetryableMemoryQueryEmbeddings: list,
            retryMemoryQueryEmbedding: retry,
        });
        const controller = new LorepiaAppController(client);
        await controller.selectConversation(conversation);
        await vi.waitFor(() =>
            expect(get(controller.state).memory_query_retries.candidates).toEqual([candidate]),
        );
        expect(list).toHaveBeenCalledWith({
            conversation_id: conversation.id,
            branch_id: branch.id,
            limit: 16,
        });

        await expect(controller.retryMemoryQueryEmbedding(candidate, false)).resolves.toBe(false);
        expect(retry).not.toHaveBeenCalled();

        await expect(controller.retryMemoryQueryEmbedding(candidate, true)).resolves.toBe(true);
        expect(retry).toHaveBeenCalledWith({
            conversation_id: candidate.conversation_id,
            branch_id: candidate.branch_id,
            id: candidate.id,
            expected_revision: 4,
            acknowledge_unknown_outcome: true,
        });
        expect(get(controller.state).memory_query_retries).toMatchObject({
            phase: 'ready',
            error: null,
            candidates: [],
            busy_id: null,
        });
        expect(get(controller.state).memory_query_retries.notice).toContain(
            '미리보기나 메시지 결과는 만들지 않았습니다',
        );
        expect(get(controller.state).memory_query_retries.notice).toContain(
            '계획 미리보기 또는 메시지 전송·편집·재생성',
        );
    });

    it.each(['failed', 'cancelled'] as const)(
        'retries a %s preparation without unknown-outcome acknowledgement',
        async (status) => {
            const candidate = {
                id: `query-embedding-${status}`,
                status,
                revision: 8,
                conversation_id: conversation.id,
                branch_id: branch.id,
                error_code: status === 'failed' ? 'provider_unavailable' : null,
                requires_unknown_outcome_acknowledgement: false,
            };
            const retry = vi.fn().mockResolvedValue({
                ...candidate,
                status: 'queued',
                revision: 9,
                error_code: null,
            });
            const controller = new LorepiaAppController(
                mockClient({
                    listRetryableMemoryQueryEmbeddings: () => Promise.resolve([candidate]),
                    retryMemoryQueryEmbedding: retry,
                }),
            );
            await controller.selectConversation(conversation);
            await vi.waitFor(() =>
                expect(get(controller.state).memory_query_retries.candidates).toEqual([candidate]),
            );

            await expect(controller.retryMemoryQueryEmbedding(candidate, false)).resolves.toBe(
                true,
            );

            expect(retry).toHaveBeenCalledWith({
                conversation_id: candidate.conversation_id,
                branch_id: candidate.branch_id,
                id: candidate.id,
                expected_revision: 8,
                acknowledge_unknown_outcome: false,
            });
        },
    );

    it('pins the CAS revision and requires acknowledgement for an interrupted memory job', async () => {
        const job = {
            memory_job_id: 'memory-job-1',
            kind: 'summary' as const,
            revision: 5,
            conversation_id: conversation.id,
            branch_id: branch.id,
            source_start_message_id: 'message-1',
            source_end_message_id: 'message-2',
            attempt: 1,
            interruption_count: 1,
            last_interrupted_at: '2026-01-01T00:00:00Z',
            last_error_code: 'process_restarted',
        };
        const retry = vi.fn().mockResolvedValue({
            memory_job_id: job.memory_job_id,
            kind: 'summary',
            status: 'queued',
            revision: 6,
            conversation_id: job.conversation_id,
            branch_id: job.branch_id,
            source_start_message_id: job.source_start_message_id,
            source_end_message_id: job.source_end_message_id,
            attempt: 1,
        });
        const controller = new LorepiaAppController(
            mockClient({
                listInterruptedMemoryJobs: () => Promise.resolve([job]),
                retryInterruptedMemoryJob: retry,
            }),
        );
        await controller.selectConversation(conversation);
        await vi.waitFor(() =>
            expect(get(controller.state).memory_query_retries.interrupted_jobs).toEqual([job]),
        );

        await expect(controller.retryInterruptedMemoryJob(job, false)).resolves.toBe(false);
        expect(retry).not.toHaveBeenCalled();

        await expect(controller.retryInterruptedMemoryJob(job, true)).resolves.toBe(true);
        expect(retry).toHaveBeenCalledWith({
            conversation_id: job.conversation_id,
            branch_id: job.branch_id,
            memory_job_id: job.memory_job_id,
            expected_revision: 5,
            acknowledge_unknown_outcome: true,
        });
        expect(get(controller.state).memory_query_retries.interrupted_jobs).toEqual([]);
    });

    it('keeps embedding retry candidates usable when the interrupted job listing fails', async () => {
        const candidate = {
            id: 'query-embedding-1',
            status: 'failed' as const,
            revision: 2,
            conversation_id: conversation.id,
            branch_id: branch.id,
            error_code: 'provider_unavailable',
            requires_unknown_outcome_acknowledgement: false,
        };
        const controller = new LorepiaAppController(
            mockClient({
                listRetryableMemoryQueryEmbeddings: () => Promise.resolve([candidate]),
                listInterruptedMemoryJobs: () => Promise.reject(new Error('listing unavailable')),
            }),
        );
        await controller.selectConversation(conversation);
        await vi.waitFor(() =>
            expect(get(controller.state).memory_query_retries.candidates).toEqual([candidate]),
        );

        expect(get(controller.state).memory_query_retries.interrupted_jobs).toEqual([]);
        expect(get(controller.state).memory_query_retries.phase).toBe('ready');
    });

    it('retains the candidate and reports an error when the retry receipt cannot be verified', async () => {
        const candidate = {
            id: 'query-embedding-failed',
            status: 'failed' as const,
            revision: 12,
            conversation_id: conversation.id,
            branch_id: branch.id,
            error_code: 'provider_unavailable',
            requires_unknown_outcome_acknowledgement: false,
        };
        const controller = new LorepiaAppController(
            mockClient({
                listRetryableMemoryQueryEmbeddings: () => Promise.resolve([candidate]),
                retryMemoryQueryEmbedding: () =>
                    Promise.resolve({
                        ...candidate,
                        status: 'queued',
                        revision: 99,
                        error_code: null,
                    }),
            }),
        );
        await controller.selectConversation(conversation);
        await vi.waitFor(() =>
            expect(get(controller.state).memory_query_retries.candidates).toEqual([candidate]),
        );

        await expect(controller.retryMemoryQueryEmbedding(candidate, false)).resolves.toBe(false);

        expect(get(controller.state).memory_query_retries).toMatchObject({
            phase: 'error',
            error: '재시도 결과를 검증하지 못했습니다. 목록을 새로고침해 상태를 확인하세요.',
            candidates: [candidate],
            busy_id: null,
            notice: null,
        });
        expect(get(controller.state).announcement).not.toContain(
            '임베딩 준비만 다시 대기열에 넣었습니다',
        );
    });
});
