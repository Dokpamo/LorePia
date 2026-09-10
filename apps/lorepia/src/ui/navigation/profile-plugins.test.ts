import { expect, it, vi } from 'vitest';
import { createPreviewClient } from '../../preview/mock-client';
import {
    DEMO_CONTENT_MODULE_BINDINGS,
    DEMO_CONTENT_MODULE_DOCUMENTS,
} from '../../preview/demo-data';
import type {
    ContentModuleLifecycleBindingDto,
    ContentModuleLifecycleClientApi,
} from '../../features/orchestration/module-lifecycle-contracts';
import type {
    LorepiaClient,
    ListContentModuleBindingsInput,
    ModuleBindingDocumentDto,
    RevisionedDto,
} from '../../lib/ipc/contracts';
import { readProfilePlugins } from './profile-plugins';

it('shows matching dedicated and approved app plugins, excluding other characters and unapproved common plugins', async () => {
    const client = createPreviewClient();
    const [source] = DEMO_CONTENT_MODULE_DOCUMENTS;
    const [record] = DEMO_CONTENT_MODULE_BINDINGS;
    if (!source || !record) throw new Error('Missing fixtures');
    client.listContentModules = vi.fn().mockResolvedValue(
        ['dedicated', 'common', 'other', 'unapproved', 'off'].map((id) => ({
            ...source,
            value: { ...source.value, id },
        })),
    );
    client.listContentModuleBindings = vi.fn(
        ({
            content_module_id,
        }: ListContentModuleBindingsInput): Promise<RevisionedDto<ModuleBindingDocumentDto>[]> =>
            Promise.resolve([
                {
                    ...record,
                    value: {
                        ...record.value,
                        module_id: content_module_id,
                        scope: ['common', 'unapproved', 'off'].includes(content_module_id)
                            ? 'app'
                            : 'character',
                        target_id:
                            content_module_id === 'dedicated'
                                ? 'current'
                                : content_module_id === 'other'
                                  ? 'someone-else'
                                  : null,
                        approved: content_module_id !== 'unapproved',
                        enabled: content_module_id !== 'off',
                    },
                },
            ]),
    );
    const result = await readProfilePlugins(client, 'current', undefined);
    expect(result.items.map((item) => item.id)).toEqual(['dedicated', 'common']);
    expect(result.items.map((item) => item.status)).toEqual(['configured', 'configured']);
    expect(result.next).toBeNull();
});

it('uses native application status for a known conversation, keeps dedicated disabled items and merges applicable scopes', async () => {
    const [document] = DEMO_CONTENT_MODULE_BINDINGS;
    if (!document) throw new Error('Missing fixture');
    const record = document.value;
    function binding(
        scope: 'character' | 'conversation' | 'app',
        target: string | null,
        status: ContentModuleLifecycleBindingDto['disposition'],
    ): ContentModuleLifecycleBindingDto {
        return {
            binding: {
                binding: {
                    ...record,
                    module_id: target === 'other' ? 'other-module' : 'module',
                    scope,
                    target_id: target,
                    conversation_id: null,
                    priority: 0,
                    resolution_mode: 'active',
                    pinned_revision_id: null,
                    package_import_approval_id: null,
                    activation_approval_id: null,
                    activation_review_sha256: null,
                    activation_plan_sha256: null,
                    variable_overrides: { values: [] },
                },
                state_revision: 1,
                updated_at: record.created_at,
            },
            disposition: status,
            approved_revision_id: record.revision_id,
            module_name: 'A plugin',
            revision_source_sha256: 'a'.repeat(64),
            revisions: [],
            revisions_truncated: false,
        };
    }
    const api = createPreviewClient() as LorepiaClient & Partial<ContentModuleLifecycleClientApi>;
    api.listContentModuleLifecycleBindings = vi.fn().mockResolvedValue({
        items: [
            binding('character', 'current', 'disabled'),
            binding('conversation', 'chat', 'applied'),
            binding('character', 'other', 'applied'),
        ],
        truncated: false,
        workspace_review_sha256: 'b'.repeat(64),
        workspace_state_revision: 1,
    });
    const target = { conversation_id: 'chat', branch_id: 'branch' };
    const result = await readProfilePlugins(api, 'current', target);
    expect(api.listContentModuleLifecycleBindings).toHaveBeenCalledExactlyOnceWith({
        runtime_target: target,
        limit: 100,
    });
    expect(result.items).toHaveLength(1);
    expect(result.items[0]).toMatchObject({
        scopes: ['character', 'conversation'],
        status: 'applied',
    });
});
