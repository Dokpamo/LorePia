<script lang="ts">
    import { Plus, Settings } from '@lucide/svelte';
    import { tick, untrack } from 'svelte';
    import { flip } from 'svelte/animate';
    import { t, tr } from '../../lib/i18n';
    import IconButton from './IconButton.svelte';
    import RailOrganizer from './RailOrganizer.svelte';
    import { cardDrag } from './card-drag';
    import { extractCard, groupCards, syncRail, type RailEntry } from './folder-model';
    import type { Overlay, SampleCharacter } from './sample-data';
    let {
        characters,
        selected,
        oncharacter,
        onaction,
        onbusy,
    }: {
        characters: SampleCharacter[];
        selected: string;
        oncharacter: (id: string) => void;
        onaction: (kind: Overlay, trigger: HTMLButtonElement) => void;
        onbusy: (busy: boolean) => void;
    } = $props();
    let entries = $state<RailEntry[]>(
        untrack(() =>
            syncRail(
                [],
                characters.map((item) => item.id),
            ),
        ),
    );
    let openFolder = $state<string | null>(null);
    let organizing = $state<string | null>(null);
    let notice = $state('');
    let nextFolder = 1;
    let opener: HTMLElement | null = null;
    let rail: HTMLElement;
    const duration = () =>
        window.matchMedia('(prefers-reduced-motion: reduce)').matches ? 0 : 180;
    const card = (id: string) => characters.find((item) => item.id === id);
    $effect(() => {
        const ids = characters.map((item) => item.id);
        untrack(() => {
            entries = syncRail(entries, ids);
        });
    });
    function close() {
        organizing = null;
        onbusy(false);
        void tick().then(() => {
            const target = opener?.isConnected
                ? opener
                : rail.querySelector<HTMLElement>('[aria-pressed="true"]');
            target?.focus({ preventScroll: true });
        });
    }
    function organize(id: string, trigger: HTMLElement) {
        opener = trigger;
        organizing = id;
        onbusy(true);
    }
    function group(source: string, target: string) {
        const next = groupCards(entries, source, target, {
            id: `folder-${String(nextFolder)}`,
            name: t('uiPreview.folder', { number: nextFolder }),
        });
        if (next === entries) return;
        entries = next;
        nextFolder += 1;
        openFolder =
            entries.find((entry) => entry.cards.includes(selected) && entry.cards.length > 1)?.id ??
            null;
        notice = t('uiPreview.folderCreated');
        if (organizing) close();
    }
    function extract(id: string) {
        entries = extractCard(entries, id);
        notice = t('uiPreview.cardMovedOut');
        if (organizing) close();
    }
</script>

<aside
    bind:this={rail}
    class="ui-card-rail"
    aria-label={$tr('uiPreview.cardList')}
    data-ui-no-swipe
>
    <div class="ui-rail-base" inert={organizing !== null} aria-hidden={organizing !== null}>
        <div class="ui-rail-header">
            <IconButton
                label={$tr('uiPreview.appSettings')}
                onclick={(event: MouseEvent & { currentTarget: HTMLButtonElement }) =>
                    onaction('app-settings', event.currentTarget)}><Settings /></IconButton
            >
        </div>
        <div class="ui-character-list" use:cardDrag={{ ondrop: group }}>
            {#each entries as item (item.id)}
                <div class="ui-rail-entry" animate:flip={{ duration: duration() }}>
                    <button
                        type="button"
                        class="ui-character ui-pressable"
                        data-rail-id={item.id}
                        aria-label={item.cards.length > 1
                            ? $tr('uiPreview.folderOpen', { name: item.name })
                            : $tr('uiPreview.cardSelect', { name: card(item.id)?.name ?? '' })}
                        aria-pressed={item.cards.includes(selected)}
                        aria-expanded={item.cards.length > 1 ? openFolder === item.id : undefined}
                        title={item.cards.length > 1 ? item.name : card(item.id)?.name}
                        onclick={() => {
                            if (item.cards.length > 1)
                                openFolder = openFolder === item.id ? null : item.id;
                            else oncharacter(item.id);
                        }}
                        oncontextmenu={(event) => {
                            event.preventDefault();
                            organize(item.id, event.currentTarget);
                        }}
                        onkeydown={(event) => {
                            if (
                                event.key === 'ContextMenu' ||
                                (event.shiftKey && event.key === 'F10')
                            ) {
                                event.preventDefault();
                                organize(item.id, event.currentTarget);
                            }
                        }}
                    >
                        <span class="ui-character-content ui-press-visual" aria-hidden="true">
                            {#if item.cards.length > 1}<span class="ui-folder-grid"
                                    >{#each item.cards.slice(0, 4) as id (id)}<span
                                            >{card(id)?.thumbnail}</span
                                        >{/each}</span
                                >
                            {:else}<span class="ui-mini-card">{card(item.id)?.thumbnail}</span>{/if}
                        </span>
                    </button>
                    {#if item.cards.length > 1 && openFolder === item.id}
                        <div class="ui-folder-children">
                            {#each item.cards as id (id)}
                                <button
                                    type="button"
                                    class="ui-character ui-pressable"
                                    data-rail-id={id}
                                    aria-label={$tr('uiPreview.cardSelect', {
                                        name: card(id)?.name ?? '',
                                    })}
                                    aria-pressed={id === selected}
                                    title={card(id)?.name}
                                    onclick={() => oncharacter(id)}
                                    oncontextmenu={(event) => {
                                        event.preventDefault();
                                        organize(id, event.currentTarget);
                                    }}
                                    onkeydown={(event) => {
                                        if (
                                            event.key === 'ContextMenu' ||
                                            (event.shiftKey && event.key === 'F10')
                                        ) {
                                            event.preventDefault();
                                            organize(id, event.currentTarget);
                                        }
                                    }}
                                >
                                    <span
                                        class="ui-character-content ui-press-visual"
                                        aria-hidden="true"
                                        ><span class="ui-mini-card">{card(id)?.thumbnail}</span
                                        ></span
                                    >
                                </button>
                            {/each}
                            <IconButton
                                label={$tr('uiPreview.folderSettings')}
                                onclick={(
                                    event: MouseEvent & { currentTarget: HTMLButtonElement },
                                ) => organize(item.id, event.currentTarget)}
                                ><Settings /></IconButton
                            >
                        </div>
                    {/if}
                </div>
            {/each}
            <IconButton
                label={$tr('uiPreview.cardAdd')}
                onclick={(event: MouseEvent & { currentTarget: HTMLButtonElement }) =>
                    onaction('add-character', event.currentTarget)}><Plus /></IconButton
            >
        </div>
    </div>
    <span class="ui-sr" role="status">{notice}</span>
    {#if organizing}
        <RailOrganizer
            source={organizing}
            {entries}
            {characters}
            ongroup={group}
            onextract={extract}
            onclose={close}
            onrename={(id: string, name: string) => {
                const entry = entries.find((item) => item.id === id);
                if (entry && name.trim()) entry.name = name.trim();
            }}
        />
    {/if}
</aside>
