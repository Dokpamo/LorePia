<script lang="ts">
    import { ChevronRight, Plus, Search, X } from '@lucide/svelte';
    import { tick } from 'svelte';
    import type { LorepiaAppController, LorepiaAppState } from '../../app/app-controller';
    import MobileMenuRow from '../../components/mobile/MobileMenuRow.svelte';
    import { tr } from '../../lib/i18n';
    import type { CharacterDto, LorepiaClient } from '../../lib/ipc/contracts';
    import MobileAvatar from '../../components/mobile/MobileAvatar.svelte';
    import { safeConversationPreview } from '../conversations/conversation-preview';
    import { characterDescriptionPreview } from './character-preview';
    import { enterCharacter } from './character-entry';

    interface Props {
        state: LorepiaAppState;
        controller: LorepiaAppController;
        client: LorepiaClient;
        onOpenChat: (opened: boolean) => void;
        onOpenSettings: (section: 'connections' | 'target') => void;
    }
    let { state: appState, controller, client, onOpenChat, onOpenSettings }: Props = $props();
    let searchOpen = $state(false);
    let query = $state('');
    let input = $state<HTMLInputElement>();
    let searchButton = $state<HTMLButtonElement>();
    const characters = $derived(
        appState.library.characters.filter((character) =>
            `${character.name} ${character.description}`
                .toLocaleLowerCase('ko-KR')
                .includes(query.trim().toLocaleLowerCase('ko-KR')),
        ),
    );

    async function openSearch(): Promise<void> {
        searchOpen = true;
        await tick();
        input?.focus();
    }
    async function closeSearch(): Promise<void> {
        searchOpen = false;
        query = '';
        await tick();
        searchButton?.focus();
    }
    const needsModel = $derived(
        appState.providers.phase === 'ready' &&
            appState.providers.workspace.settings.selected_model_route_id === null &&
            appState.providers.workspace.settings.selected_provider_profile_id === null,
    );

    const needsConnection = $derived(
        appState.providers.workspace.connections.length === 0 &&
            appState.providers.workspace.legacy_profiles.length === 0,
    );

    function openCharacter(character: CharacterDto): void {
        void enterCharacter(controller, character).then(onOpenChat);
    }
</script>

<section class="mobile-screen" aria-label={$tr('app.tab.home')}>
    <header class="mobile-page-header">
        <h1 class="mobile-wordmark">LorePia</h1>
        <button
            bind:this={searchButton}
            class="mobile-icon-button"
            type="button"
            aria-label={$tr('library.search.label')}
            aria-expanded={searchOpen}
            disabled={appState.library.phase === 'loading'}
            onclick={() => void openSearch()}
        >
            <Search aria-hidden="true" />
        </button>
    </header>
    <div class="mobile-page-scroll">
        {#if searchOpen}
            <div class="mobile-search-field">
                <Search aria-hidden="true" />
                <input
                    bind:this={input}
                    bind:value={query}
                    type="search"
                    aria-label={$tr('library.search.label')}
                    placeholder={$tr('library.search.placeholder')}
                    onkeydown={(event) => {
                        if (event.key === 'Escape') void closeSearch();
                    }}
                />
                <button
                    class="mobile-icon-button"
                    type="button"
                    aria-label={$tr('library.search.close')}
                    onclick={() => void closeSearch()}><X aria-hidden="true" /></button
                >
            </div>
        {/if}
        {#if !searchOpen}
            <h2 class="mobile-home-headline">{$tr('mobile.home.headline')}</h2>
            {#if appState.selected_conversation !== null}
                <section
                    class="mobile-card mobile-resume-card"
                    aria-label={$tr('mobile.home.resume')}
                >
                    <h2>{$tr('mobile.home.resume')}</h2>
                    <button
                        class="mobile-resume-entry"
                        type="button"
                        onclick={() => onOpenChat(true)}
                    >
                        <MobileAvatar {client} character={appState.selected_character} />
                        <span class="mobile-list-copy">
                            <strong
                                >{appState.selected_conversation.title ||
                                    appState.selected_character?.name}</strong
                            >
                            <small
                                >{safeConversationPreview(
                                    appState.selected_conversation,
                                    appState,
                                    $tr('mobile.home.resume_hint'),
                                )}</small
                            >
                        </span>
                        <span class="mobile-resume-action"
                            >{$tr('mobile.home.continue')}<ChevronRight aria-hidden="true" /></span
                        >
                    </button>
                </section>
            {/if}
        {/if}
        <section class="mobile-card" aria-label={$tr('library.list.label')}>
            <header class="mobile-card-heading">
                <h2>{$tr('mobile.home.characters')}</h2>
                {#if appState.library.characters.length > 0}
                    <button
                        class="mobile-text-button"
                        type="button"
                        aria-label={$tr('library.empty.import')}
                        onclick={() => void controller.beginImport()}
                        ><Plus aria-hidden="true" />{$tr('mobile.home.add')}</button
                    >
                {/if}
            </header>
            {#if appState.library.phase === 'loading'}
                <div class="mobile-empty-state" role="status">{$tr('library.loading')}</div>
            {:else if appState.library.phase === 'error'}
                <div class="mobile-empty-state" role="alert">
                    <p>{appState.library.error}</p>
                    <button
                        class="mobile-button"
                        type="button"
                        onclick={() => void controller.loadLibrary()}>{$tr('library.retry')}</button
                    >
                </div>
            {:else if characters.length === 0}
                <div class="mobile-empty-state">
                    <p>{query ? $tr('library.search.empty') : $tr('mobile.home.empty')}</p>
                    {#if query}
                        <button class="mobile-button" type="button" onclick={() => (query = '')}
                            >{$tr('library.search.clear')}</button
                        >
                    {:else}
                        <button
                            class="mobile-button mobile-button-primary"
                            type="button"
                            onclick={() => void controller.beginImport()}
                            >{$tr('library.empty.import')}</button
                        >
                    {/if}
                </div>
            {:else}
                <ul class="mobile-card-list">
                    {#each characters as character (character.id)}
                        <li>
                            <button
                                class="mobile-character-row"
                                type="button"
                                onclick={() => openCharacter(character)}
                            >
                                <MobileAvatar {client} {character} />
                                <span class="mobile-list-copy">
                                    <strong>{character.name}</strong>
                                    <small
                                        >{characterDescriptionPreview(character.description) ||
                                            $tr('library.description.empty')}</small
                                    >
                                </span>
                                <ChevronRight class="mobile-chevron" aria-hidden="true" />
                            </button>
                        </li>
                    {/each}
                </ul>
            {/if}
        </section>
        {#if !searchOpen && needsModel}
            <section class="mobile-card mobile-compact-card">
                <MobileMenuRow
                    label={$tr(
                        needsConnection ? 'mobile.home.connect' : 'mobile.home.choose_model',
                    )}
                    description={$tr(
                        needsConnection ? 'mobile.home.connect_hint' : 'mobile.target.hint',
                    )}
                    onSelect={() => onOpenSettings(needsConnection ? 'connections' : 'target')}
                />
            </section>
        {/if}
    </div>
</section>
