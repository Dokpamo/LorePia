<script lang="ts">
    import { onMount, untrack } from 'svelte';
    import { Plus } from '@lucide/svelte';
    import { tr } from '../../lib/i18n';
    import type { LorepiaClient } from '../../lib/ipc/contracts';
    import {
        type CreatorDocumentKind,
        type CreatorDocumentValue,
        type EditableCreatorDocumentState,
    } from '../../features/orchestration/orchestration-controller';
    import {
        knowledgeBookDraft,
        memoryProfileDraft,
        transformSetDraft,
        interactionRuleSetDraft,
        contentModuleDraft,
    } from '../../features/orchestration/controllers/orchestration-state';
    import { MaterialLibraryController } from '../../features/orchestration/material-library-controller';
    import SettingsPanel from '../workspace/SettingsPanel.svelte';
    import NavigationRow from './NavigationRow.svelte';
    import MaterialEditor from './MaterialEditor.svelte';
    let {
        client,
        kind,
        title,
        onclose,
    }: { client: LorepiaClient; kind: CreatorDocumentKind; title: string; onclose: () => void } =
        $props();
    const controller = untrack(() => new MaterialLibraryController(client));
    const store = controller.state;
    let draft = $state<CreatorDocumentValue | null>(null);
    const documents = $derived.by((): EditableCreatorDocumentState<CreatorDocumentValue>[] => {
        if (kind === 'knowledge_book') return $store.editable_knowledge_books;
        if (kind === 'memory_profile') return $store.editable_memory_profiles;
        if (kind === 'transform_set') return $store.editable_transform_sets;
        if (kind === 'interaction_rule_set') return $store.editable_interaction_rule_sets;
        return $store.editable_content_modules;
    });
    onMount(() => {
        void controller.load();
        return () => controller.destroy();
    });
    function create() {
        const builders = {
            knowledge_book: knowledgeBookDraft,
            memory_profile: memoryProfileDraft,
            transform_set: transformSetDraft,
            interaction_rule_set: interactionRuleSetDraft,
            content_module: contentModuleDraft,
        };
        draft = { ...builders[kind](crypto.randomUUID()), name: '' };
    }
</script>

<SettingsPanel {title} {onclose} covered={draft !== null}>
    {#if $store.phase === 'loading'}<p role="status">{$tr('workspace.loading')}</p>{/if}
    {#if $store.phase === 'error'}<p role="alert">{$tr('navigation.loadFailed')}</p>
        <button class="seed-secondary ui-pressable" onclick={() => void controller.load()}
            ><span class="ui-press-visual">{$tr('workspace.retry')}</span></button
        >{/if}
    <button class="seed-primary ui-pressable" disabled={$store.phase !== 'ready'} onclick={create}
        ><span class="ui-press-visual"><Plus />{$tr('navigation.createMaterial')}</span></button
    >
    <section class="seed-group">
        <h2>{$tr('navigation.documentList')}</h2>
        {#each documents.filter((item) => item.expected_revision !== null) as item (item.value.id)}
            <NavigationRow
                title={item.value.name}
                onclick={() => (draft = structuredClone(item.value))}
            />
        {:else}<p class="seed-content seed-secondary-text">
                {$tr('navigation.noMaterials')}
            </p>{/each}
    </section>
    {#if $store.creator_document_cursors?.[kind]}<button
            class="seed-secondary ui-pressable"
            disabled={$store.editable_creator_documents_loading}
            onclick={() => void controller.documents.loadMoreCreatorDocuments(kind)}
            ><span class="ui-press-visual">{$tr('navigation.loadMore')}</span></button
        >{/if}
</SettingsPanel>
{#if draft}<MaterialEditor
        value={draft}
        {kind}
        {title}
        controller={controller.documents}
        orchestrationState={$store}
        onclose={() => (draft = null)}
    />{/if}
