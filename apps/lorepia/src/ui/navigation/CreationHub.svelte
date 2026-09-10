<script lang="ts">
    import {
        UserRoundPlus,
        BookOpen,
        SlidersHorizontal,
        Blocks,
        NotebookPen,
        WandSparkles,
        Brain,
    } from '@lucide/svelte';
    import { tr } from '../../lib/i18n';
    import type { LorepiaClient } from '../../lib/ipc/contracts';
    import type { CreatorDocumentKind } from '../../features/orchestration/orchestration-controller';
    import NavigationHeader from './NavigationHeader.svelte';
    import NavigationRow from './NavigationRow.svelte';
    import MaterialLibrary from './MaterialLibrary.svelte';
    let {
        client,
        ready,
        onimport,
        onprompt,
        ondetail,
    }: {
        client: LorepiaClient;
        ready: boolean;
        onimport: () => void;
        onprompt: () => void;
        ondetail: (active: boolean) => void;
    } = $props();
    let family = $state<CreatorDocumentKind | null>(null);
    const families = [
        {
            kind: 'knowledge_book',
            title: 'navigation.createLorebook',
            hint: 'navigation.createLorebookHint',
            icon: BookOpen,
        },
        {
            kind: 'transform_set',
            title: 'navigation.createTransform',
            hint: 'navigation.createTransformHint',
            icon: SlidersHorizontal,
        },
        {
            kind: 'interaction_rule_set',
            title: 'navigation.createInteraction',
            hint: 'navigation.createInteractionHint',
            icon: WandSparkles,
        },
        {
            kind: 'memory_profile',
            title: 'navigation.createMemory',
            hint: 'navigation.createMemoryHint',
            icon: Brain,
        },
        {
            kind: 'content_module',
            title: 'navigation.createModule',
            hint: 'navigation.createModuleHint',
            icon: Blocks,
        },
    ] as const;
    const selectedFamily = $derived(families.find((item) => item.kind === family));
    $effect(() => ondetail(family !== null));
</script>

<div class="seed-root" inert={family !== null} aria-hidden={family !== null}>
    <NavigationHeader title={$tr('navigation.create')} />
    <div class="seed-scroll">
        <div class="seed-content">
            <section class="seed-group">
                <NavigationRow
                    title={$tr('navigation.createCharacter')}
                    description={$tr('navigation.createCharacterHint')}
                    onclick={onimport}
                    disabled={!ready}
                >
                    {#snippet prefix()}<UserRoundPlus />{/snippet}
                </NavigationRow>
            </section>
            <section class="seed-group">
                <h2>{$tr('navigation.createTools')}</h2>
                <NavigationRow
                    title={$tr('navigation.createPrompt')}
                    description={$tr('navigation.createPromptHint')}
                    onclick={onprompt}
                    disabled={!ready}
                >
                    {#snippet prefix()}<NotebookPen />{/snippet}
                </NavigationRow>
                {#each families as item (item.kind)}
                    <NavigationRow
                        title={$tr(item.title)}
                        description={$tr(item.hint)}
                        onclick={() => (family = item.kind)}
                        disabled={!ready}
                    >
                        {#snippet prefix()}<item.icon />{/snippet}
                    </NavigationRow>
                {/each}
            </section>
        </div>
    </div>
</div>
{#if family && selectedFamily}<MaterialLibrary
        {client}
        kind={family}
        title={$tr(selectedFamily.title)}
        onclose={() => (family = null)}
    />{/if}
