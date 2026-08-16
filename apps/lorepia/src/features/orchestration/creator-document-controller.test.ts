import { get } from 'svelte/store';
import { describe, expect, it, vi } from 'vitest';

import type {
    CreatorContentModuleDocumentDto,
    CreatorMemoryProfileDocumentDto,
    RevisionedDto,
} from '../../lib/ipc/contracts';
import {
    OrchestrationController,
    emptyOrchestrationWorkspace,
    type OrchestrationCapableClient,
} from './orchestration-controller';

function revisioned<Value>(value: Value, revision: number): RevisionedDto<Value> {
    return {
        value,
        revision,
        created_at: '2026-08-03T00:00:00Z',
        updated_at: '2026-08-03T00:00:00Z',
        deleted_at: null,
    };
}

function deferred<Value>(): {
    promise: Promise<Value>;
    resolve: (value: Value) => void;
} {
    let resolvePromise!: (value: Value) => void;
    const promise = new Promise<Value>((resolve) => {
        resolvePromise = resolve;
    });
    return { promise, resolve: resolvePromise };
}

const MEMORY_PROFILE: CreatorMemoryProfileDocumentDto = {
    id: 'memory-default',
    name: 'Default memory',
    summary_task: 'summary-task',
    embedding_task: null,
    turns_per_summary: 8,
    recent_raw_budget: { max_tokens: 2_048 },
    episodic_budget: { max_tokens: 1_024 },
    semantic_budget: { max_tokens: 1_024 },
    retrieval_count: 8,
    recency_weight: 1,
    similarity_weight: 1,
    importance_weight: 1,
    preserve_invalidated_records: false,
    summary_schema: 'summary-v1',
};

function capableClient() {
    const upsertMemoryProfile = vi.fn(
        (input: { value: CreatorMemoryProfileDocumentDto; expected_revision: number | null }) =>
            Promise.resolve(revisioned(input.value, (input.expected_revision ?? 0) + 1)),
    );
    const deleteMemoryProfile = vi.fn(() => Promise.resolve(revisioned(MEMORY_PROFILE, 4)));
    const upsertContentModule = vi.fn(
        (input: { value: CreatorContentModuleDocumentDto; expected_revision: number | null }) =>
            Promise.resolve(revisioned(input.value, (input.expected_revision ?? 0) + 1)),
    );
    const deleteContentModule = vi.fn(
        (input: { content_module_id: string; expected_revision: number }) =>
            Promise.resolve(
                revisioned(
                    {
                        id: input.content_module_id,
                        name: 'Module',
                        version: '0.1.0',
                        prompt_fragments: [],
                        knowledge_book_ids: [],
                        control_specs: [],
                        transform_set_ids: [],
                        interaction_rule_set_ids: [],
                        asset_ids: [],
                        required_capabilities: [],
                        metadata: {
                            author: null,
                            license: 'proprietary',
                            redistribution_allowed: false,
                            homepage: null,
                            description: '',
                            tags: [],
                        },
                    } satisfies CreatorContentModuleDocumentDto,
                    input.expected_revision + 1,
                ),
            ),
    );
    const client = {
        getOrchestrationWorkspace: vi.fn((conversationId: string, branchId: string) =>
            Promise.resolve(emptyOrchestrationWorkspace(conversationId, branchId)),
        ),
        listTaskProfiles: vi.fn(() => Promise.resolve([])),
        listMemoryProfiles: vi.fn(() => Promise.resolve([revisioned(MEMORY_PROFILE, 3)])),
        listKnowledgeBooks: vi.fn(() => Promise.resolve([])),
        listTransformSets: vi.fn(() => Promise.resolve([])),
        listInteractionRuleSets: vi.fn(() => Promise.resolve([])),
        listContentModules: vi.fn(() => Promise.resolve([])),
        upsertMemoryProfile,
        deleteMemoryProfile,
        upsertContentModule,
        deleteContentModule,
    } as unknown as OrchestrationCapableClient;
    return {
        client,
        upsertMemoryProfile,
        deleteMemoryProfile,
        upsertContentModule,
        deleteContentModule,
    };
}

describe('OrchestrationController Creator documents', () => {
    it('loads safe documents and keeps exact revisions through update and delete CAS', async () => {
        const fixture = capableClient();
        const controller = new OrchestrationController(fixture.client);
        await controller.loadContext('conversation-1', 'branch-1');

        expect(get(controller.state).editable_memory_profiles).toEqual([
            {
                value: MEMORY_PROFILE,
                expected_revision: 3,
                dirty: false,
            },
        ]);

        expect(
            controller.stageMemoryProfile(MEMORY_PROFILE.id, {
                name: 'Edited memory',
            }),
        ).toBe(true);
        expect(await controller.saveCreatorDocument('memory_profile', MEMORY_PROFILE.id)).toBe(
            true,
        );
        expect(fixture.upsertMemoryProfile).toHaveBeenCalledWith({
            value: { ...MEMORY_PROFILE, name: 'Edited memory' },
            expected_revision: 3,
        });
        expect(get(controller.state).editable_memory_profiles[0]).toMatchObject({
            value: { id: MEMORY_PROFILE.id, name: 'Edited memory' },
            expected_revision: 4,
            dirty: false,
        });

        expect(await controller.deleteCreatorDocument('memory_profile', MEMORY_PROFILE.id)).toBe(
            true,
        );
        expect(fixture.deleteMemoryProfile).toHaveBeenCalledWith({
            memory_profile_id: MEMORY_PROFILE.id,
            expected_revision: 4,
        });
        expect(get(controller.state).editable_memory_profiles).toEqual([]);
    });

    it('creates with expected_revision null and does not report or remove a failed persisted write', async () => {
        const fixture = capableClient();
        const controller = new OrchestrationController(fixture.client);
        await controller.loadContext('conversation-1', 'branch-1');

        expect(controller.addCreatorDocumentDraft('content_module', 'module-new')).toBe(true);
        const draft = get(controller.state).editable_content_modules[0];
        expect(draft?.expected_revision).toBeNull();
        expect(
            controller.stageContentModule('module-new', {
                metadata: {
                    author: null,
                    license: 'proprietary',
                    redistribution_allowed: false,
                    homepage: null,
                    description: 'Local module',
                    tags: [],
                },
            }),
        ).toBe(true);
        expect(await controller.saveCreatorDocument('content_module', 'module-new')).toBe(true);
        const createInput = fixture.upsertContentModule.mock.calls[0]?.[0];
        expect(createInput?.expected_revision).toBeNull();
        expect(createInput?.value.id).toBe('module-new');
        expect(get(controller.state).editable_content_modules[0]).toMatchObject({
            expected_revision: 1,
            dirty: false,
        });

        fixture.deleteContentModule.mockRejectedValueOnce(new Error('revision conflict'));
        expect(await controller.deleteCreatorDocument('content_module', 'module-new')).toBe(false);
        const failedState = get(controller.state);
        expect(failedState.editable_content_modules).toHaveLength(1);
        expect(failedState.editable_creator_documents_error).not.toBeNull();
        expect(failedState.announcement).not.toContain('삭제했습니다');
        expect(fixture.deleteContentModule).toHaveBeenCalledWith({
            content_module_id: 'module-new',
            expected_revision: 1,
        });
    });

    it('keeps a newer Creator draft on top of the revision saved while it was edited', async () => {
        const fixture = capableClient();
        const pendingSave = deferred<RevisionedDto<CreatorMemoryProfileDocumentDto>>();
        fixture.upsertMemoryProfile.mockImplementationOnce(() => pendingSave.promise);
        const controller = new OrchestrationController(fixture.client);
        await controller.loadContext('conversation-1', 'branch-1');
        expect(
            controller.stageMemoryProfile(MEMORY_PROFILE.id, {
                name: 'Submitted memory',
            }),
        ).toBe(true);

        const saving = controller.saveCreatorDocument('memory_profile', MEMORY_PROFILE.id);
        await vi.waitFor(() => expect(fixture.upsertMemoryProfile).toHaveBeenCalledOnce());
        expect(
            controller.stageMemoryProfile(MEMORY_PROFILE.id, {
                summary_schema: 'newer-unsaved-schema',
            }),
        ).toBe(true);
        pendingSave.resolve(
            revisioned(
                {
                    ...MEMORY_PROFILE,
                    name: 'Submitted memory',
                },
                4,
            ),
        );

        await expect(saving).resolves.toBe(true);
        expect(fixture.upsertMemoryProfile.mock.calls[0]?.[0]).toEqual({
            value: { ...MEMORY_PROFILE, name: 'Submitted memory' },
            expected_revision: 3,
        });
        expect(get(controller.state).editable_memory_profiles[0]).toMatchObject({
            value: {
                name: 'Submitted memory',
                summary_schema: 'newer-unsaved-schema',
            },
            expected_revision: 4,
            dirty: true,
        });
        expect(get(controller.state).announcement).toContain('아직 저장되지 않았습니다');

        await expect(
            controller.saveCreatorDocument('memory_profile', MEMORY_PROFILE.id),
        ).resolves.toBe(true);
        expect(fixture.upsertMemoryProfile.mock.calls[1]?.[0]).toMatchObject({
            value: {
                name: 'Submitted memory',
                summary_schema: 'newer-unsaved-schema',
            },
            expected_revision: 4,
        });
        expect(get(controller.state).editable_memory_profiles[0]).toMatchObject({
            expected_revision: 5,
            dirty: false,
        });
    });
});
