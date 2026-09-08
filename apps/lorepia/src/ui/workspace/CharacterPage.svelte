<script lang="ts">
    import type { LorepiaClient } from '../../lib/ipc/contracts';
    import CharacterImage from './CharacterImage.svelte';
    import {
        ChevronRight,
        Image as ImageIcon,
        Info,
        MessageSquare,
        Plus,
        Search,
        Settings,
        X,
    } from '@lucide/svelte';
    import { t, tr } from '../../lib/i18n';
    import IconButton from './IconButton.svelte';
    import CharacterRail from './CharacterRail.svelte';
    import { useTextEditor } from './text-editor.svelte';
    import type { Overlay, SampleCharacter } from './view-types';
    let {
        characters,
        client,
        character,
        conversationId,
        oncharacter,
        onconversation,
        onaction,
    }: {
        characters: SampleCharacter[];
        client?: LorepiaClient;
        character: SampleCharacter;
        conversationId: string;
        oncharacter: (id: string) => void;
        onconversation: (id: string) => void;
        onaction: (kind: Overlay, opener: HTMLButtonElement) => void;
    } = $props();
    let organizing = $state(false);
    let queries = $state<Record<string, string>>({});
    const editor = useTextEditor();
    const query = $derived(queries[character.id] ?? '');
    const histories = $derived(
        character.histories.filter((item) =>
            item.title.toLocaleLowerCase('ko').includes(query.trim().toLocaleLowerCase('ko')),
        ),
    );
    function search(trigger: HTMLButtonElement) {
        const id = character.id;
        editor.open(
            {
                label: t('uiPreview.findChat'),
                value: query,
                maxlength: 100,
                placeholder: t('uiPreview.findChatPlaceholder'),
                search: {
                    items: character.histories.map(({ id, title, date }) => ({ id, title, date })),
                    onselect: onconversation,
                },
                onchange: (value) => {
                    queries[id] = value;
                },
            },
            trigger,
        );
    }
</script>

<div class="ui-left-layout">
    <CharacterRail
        {characters}
        {client}
        selected={character.id}
        {oncharacter}
        {onaction}
        onbusy={(value: boolean) => (organizing = value)}
    />
    <div class="ui-left-content" inert={organizing} aria-hidden={organizing}>
        <div class="ui-character-banner" role="img" aria-label={$tr('uiPreview.cardImage')}>
            {#if client && character.avatarAssetId}<CharacterImage
                    {client}
                    assetId={character.avatarAssetId}
                    name={character.name}
                />{:else}<ImageIcon aria-hidden="true" />{/if}
        </div>
        <header class="ui-character-summary">
            <div class="ui-card-identity">
                <h1 class="ui-card-name">{character.name}</h1>
                <p class="ui-card-description">{character.description}</p>
            </div>
            <IconButton
                label={$tr('uiPreview.cardSettings')}
                onclick={(event: MouseEvent & { currentTarget: HTMLButtonElement }) =>
                    onaction('card-settings', event.currentTarget)}><Settings /></IconButton
            >
        </header>
        <button
            type="button"
            class="ui-card-info-link ui-pressable"
            onclick={(event) => onaction('card-info', event.currentTarget)}
        >
            <span class="ui-press-visual"
                ><Info aria-hidden="true" /><span>{$tr('uiPreview.cardInfo')}</span><ChevronRight
                    aria-hidden="true"
                /></span
            >
        </button>
        <div class="ui-history-search">
            <button
                type="button"
                class="ui-find-chat ui-pressable"
                aria-label={$tr('uiPreview.findChat')}
                onclick={(event) => search(event.currentTarget)}
            >
                <span class="ui-press-visual"
                    ><Search aria-hidden="true" /><span>{query || $tr('uiPreview.findChat')}</span
                    ></span
                >
            </button>
            {#if query}<IconButton
                    label={$tr('uiPreview.clearChatSearch')}
                    onclick={() => {
                        queries[character.id] = '';
                    }}><X /></IconButton
                >{/if}
            <IconButton
                label={$tr('uiPreview.newChatAdd')}
                onclick={(event: MouseEvent & { currentTarget: HTMLButtonElement }) =>
                    onaction('new-chat', event.currentTarget)}><Plus /></IconButton
            >
        </div>
        <section class="ui-conversation-group" aria-label={$tr('uiPreview.history')}>
            <div class="ui-section-heading"><h2>{$tr('uiPreview.history')}</h2></div>
            <div class="ui-history">
                {#each histories as item (item.id)}
                    <div class="ui-history-row" data-current={item.id === conversationId}>
                        <button
                            type="button"
                            class="ui-history-item ui-pressable"
                            aria-pressed={item.id === conversationId}
                            aria-label={$tr('uiPreview.historySelect', {
                                title: item.title,
                                date: item.date,
                            })}
                            onclick={() => onconversation(item.id)}
                        >
                            <span class="ui-history-content ui-press-visual">
                                <MessageSquare aria-hidden="true" />
                                <span><strong>{item.title}</strong><small>{item.date}</small></span>
                            </span>
                        </button>
                        {#if item.id === conversationId}
                            <IconButton
                                label={$tr('uiPreview.namedRoomSettings', { title: item.title })}
                                onclick={(
                                    event: MouseEvent & { currentTarget: HTMLButtonElement },
                                ) => onaction('room-settings', event.currentTarget)}
                                ><Settings /></IconButton
                            >
                        {/if}
                    </div>
                {:else}
                    <p class="ui-history-empty" role="status">
                        {$tr(query.trim() ? 'uiPreview.noChatMatches' : 'uiPreview.noChats')}
                    </p>
                    {#if !query.trim()}<button
                            class="ui-result-action ui-pressable"
                            onclick={(event) => onaction('new-chat', event.currentTarget)}
                            ><span class="ui-press-visual">{$tr('uiPreview.newChat')}</span></button
                        >{/if}
                {/each}
            </div>
        </section>
    </div>
</div>
