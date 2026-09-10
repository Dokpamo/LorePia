<script lang="ts">
    import { pageSlide } from './navigation-motion';
    import { ArrowLeft, ChevronRight, Minus } from '@lucide/svelte';
    import { onMount } from 'svelte';
    import { tr } from '../../lib/i18n';
    import IconButton from './IconButton.svelte';
    import EditField from './EditField.svelte';
    import { edgeBack, requestBack } from './edge-back';
    import type { RailEntry } from './folder-model';
    import type { SampleCharacter } from './view-types';
    let {
        source,
        entries,
        characters,
        ongroup,
        onextract,
        onrename,
        onclose,
    }: {
        source: string;
        entries: RailEntry[];
        characters: SampleCharacter[];
        ongroup: (source: string, target: string) => void;
        onextract: (id: string) => void;
        onrename: (id: string, name: string) => void;
        onclose: () => void;
    } = $props();
    const entry = $derived(
        entries.find((item) => item.id === source || item.cards.includes(source)),
    );
    const folder = $derived(entry?.id === source && entry.cards.length > 1);
    const title = (item: RailEntry) =>
        item.cards.length > 1
            ? item.name
            : (characters.find((card) => card.id === item.id)?.name ?? '');
    let panel: HTMLElement;
    onMount(() => panel.querySelector('button')?.focus({ preventScroll: true }));
</script>

<div class="ui-rail-organizer-layer" transition:pageSlide>
    <div
        bind:this={panel}
        class="ui-rail-organizer"
        role="dialog"
        tabindex="-1"
        aria-label={$tr('uiPreview.organizeCards')}
        data-ui-no-swipe
        use:edgeBack={{ onback: onclose }}
        onkeydown={(event) => {
            if (event.key === 'Escape') {
                event.stopPropagation();
                requestBack(panel);
            }
        }}
    >
        <header class="ui-page-header ui-navigation-header">
            <IconButton label={$tr('uiPreview.back')} onclick={() => requestBack(panel)}
                ><ArrowLeft /></IconButton
            >
            <div class="ui-title-group">
                <strong
                    >{$tr(folder ? 'uiPreview.folderSettings' : 'uiPreview.organizeCards')}</strong
                >
            </div>
        </header>
        <div class="ui-overlay-body">
            {#if folder && entry}
                <EditField
                    label={$tr('uiPreview.folderName')}
                    value={entry.name}
                    maxlength={40}
                    onchange={(value: string) => onrename(source, value)}
                />
                <div class="ui-settings-group">
                    {#each entry.cards as id (id)}
                        {@const card = characters.find((item) => item.id === id)}
                        {#if card}<div class="ui-folder-member-row">
                                <span>{card.name}</span>
                                <IconButton
                                    label={$tr('uiPreview.removeNamedCard', { name: card.name })}
                                    onclick={() => onextract(id)}><Minus /></IconButton
                                >
                            </div>{/if}
                    {/each}
                </div>
            {:else}
                <p class="ui-organize-hint">{$tr('uiPreview.folderHint')}</p>
                <div class="ui-settings-group">
                    {#each entries.filter((item) => item !== entry) as item (item.id)}
                        <button
                            type="button"
                            class="ui-organize-row ui-pressable"
                            onclick={() => ongroup(source, item.id)}
                        >
                            <span class="ui-press-visual"
                                ><span
                                    >{$tr(
                                        item.cards.length > 1
                                            ? 'uiPreview.addToFolder'
                                            : 'uiPreview.groupWith',
                                        { name: title(item) },
                                    )}</span
                                >
                                <ChevronRight aria-hidden="true" /></span
                            >
                        </button>
                    {/each}
                    {#if entry && entry.cards.length > 1}
                        <button
                            type="button"
                            class="ui-organize-row ui-pressable"
                            onclick={() => onextract(source)}
                        >
                            <span class="ui-press-visual"
                                ><span>{$tr('uiPreview.removeFromFolder')}</span><Minus
                                    aria-hidden="true"
                                /></span
                            >
                        </button>
                    {/if}
                </div>
            {/if}
        </div>
    </div>
</div>
