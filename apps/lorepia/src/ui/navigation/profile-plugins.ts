import type {
    LorepiaClient,
    OrchestrationDocumentClientApi,
    OrchestrationModuleScope,
} from '../../lib/ipc/contracts';
import type {
    CreatorDocumentsPage,
    PaginationClientApi,
    ReadPageCursor,
} from '../../lib/ipc/contracts/pagination';
import type {
    ContentModuleLifecycleClientApi,
    ContentModuleRuntimeTargetInput,
} from '../../features/orchestration/module-lifecycle-contracts';

type PluginClient = LorepiaClient &
    Partial<OrchestrationDocumentClientApi & PaginationClientApi & ContentModuleLifecycleClientApi>;
export interface ProfilePlugin {
    id: string;
    name: string;
    description: string;
    version: string;
    scopes: OrchestrationModuleScope[];
    status: 'configured' | 'applied' | 'disabled' | 'awaiting_approval' | 'needs_reapproval';
}
export interface ProfilePluginPage {
    items: ProfilePlugin[];
    next: ReadPageCursor | null;
    truncated: boolean;
}

/** A read-only presentation of existing bindings, never runtime admission authority. */
export async function readProfilePlugins(
    client: LorepiaClient,
    characterId: string,
    runtimeTarget: ContentModuleRuntimeTargetInput | undefined,
    after: ReadPageCursor | null = null,
): Promise<ProfilePluginPage> {
    const api = client as PluginClient;
    if (runtimeTarget && api.listContentModuleLifecycleBindings) {
        const result = await api.listContentModuleLifecycleBindings({
            runtime_target: runtimeTarget,
            limit: 100,
        });
        const groups = new Map<string, ProfilePlugin>();
        for (const entry of result.items) {
            const binding = entry.binding.binding;
            const dedicated = binding.scope === 'character' && binding.target_id === characterId;
            if (binding.scope === 'character' && !dedicated) continue;
            if (!dedicated && entry.disposition !== 'applied') continue;
            const existing = groups.get(binding.module_id);
            if (existing) {
                if (!existing.scopes.includes(binding.scope)) existing.scopes.push(binding.scope);
                if (entry.disposition === 'applied') existing.status = 'applied';
            } else
                groups.set(binding.module_id, {
                    id: binding.module_id,
                    name: entry.module_name,
                    description: '',
                    version:
                        entry.revisions.find(
                            (revision) => revision.revision_id === entry.approved_revision_id,
                        )?.version ?? '',
                    scopes: [binding.scope],
                    status: entry.disposition,
                });
        }
        return { items: [...groups.values()], next: null, truncated: result.truncated };
    }
    const listBindings = api.listContentModuleBindings?.bind(api);
    if (!listBindings) throw new Error('Plugin listing is unavailable');
    let page: Pick<CreatorDocumentsPage<'content_module'>, 'documents' | 'next_cursor'>;
    if (api.listCreatorDocumentsPage)
        page = await api.listCreatorDocumentsPage({ kind: 'content_module', after, limit: 100 });
    else if (api.listContentModules)
        page = { documents: await api.listContentModules(), next_cursor: null };
    else throw new Error('Plugin listing is unavailable');
    const items: ProfilePlugin[] = [];
    // Bound IPC concurrency while preserving the native document order.
    for (let offset = 0; offset < page.documents.length; offset += 8) {
        const chunk = await Promise.all(
            page.documents.slice(offset, offset + 8).map(async ({ value }) => {
                const bindings = (await listBindings({ content_module_id: value.id }))
                    .map((binding) => binding.value)
                    .filter(
                        (binding) =>
                            binding.module_id === value.id &&
                            ((binding.scope === 'character' && binding.target_id === characterId) ||
                                (binding.scope === 'app' &&
                                    binding.target_id === null &&
                                    binding.enabled &&
                                    binding.approved)),
                    );
                if (!bindings.length) return null;
                const enabled = bindings.filter((binding) => binding.enabled);
                return {
                    id: value.id,
                    name: value.name,
                    description: value.metadata.description,
                    version: value.version,
                    scopes: [...new Set(bindings.map((binding) => binding.scope))],
                    status: enabled.some((binding) => binding.approved)
                        ? 'configured'
                        : enabled.length
                          ? 'awaiting_approval'
                          : 'disabled',
                } satisfies ProfilePlugin;
            }),
        );
        items.push(...chunk.filter((item): item is NonNullable<typeof item> => item !== null));
    }
    return {
        items,
        next: page.next_cursor,
        truncated: !api.listCreatorDocumentsPage && page.documents.length >= 100,
    };
}
