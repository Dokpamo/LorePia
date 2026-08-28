<script lang="ts">
    import { Plus, Search, X } from '@lucide/svelte';
    import { tick } from 'svelte';
    import { t, tr } from '../../lib/i18n';
    import type { LorepiaAppState, LorepiaAppController } from '../../app/app-controller';
    import type {
        CharacterDto,
        ContentPackageClientApi,
        ContentSourceExportReceiptDto,
        LorepiaClient,
    } from '../../lib/ipc/contracts';
    import { normalizeClientError } from '../../lib/ipc/errors';
    import TrustedAsset from '../assets/TrustedAsset.svelte';

    type ExportCapableClient = LorepiaClient & Partial<ContentPackageClientApi>;

    interface Props {
        state: LorepiaAppState;
        controller: LorepiaAppController;
        client: ExportCapableClient;
        onOpenConversations: () => void;
        rootView?: boolean;
    }

    let {
        state: appState,
        controller,
        client,
        onOpenConversations,
        rootView = false,
    }: Props = $props();
    let exportingCharacterId = $state<string | null>(null);
    let exportReceipt = $state<ContentSourceExportReceiptDto | null>(null);
    let exportError = $state<string | null>(null);
    let exportAnnouncement = $state('');
    let searchQuery = $state('');
    let searchOpen = $state(false);
    let searchClosing = $state(false);
    let searchInput = $state<HTMLInputElement | null>(null);
    let searchContainer = $state<HTMLDivElement | null>(null);
    let characterFilter = $state('all');
    const visibleCharacters = $derived.by(() => {
        const filteredCharacters =
            characterFilter === 'all'
                ? appState.library.characters
                : appState.library.characters.filter(
                      (character) => character.id === characterFilter,
                  );
        const query = searchQuery.trim().toLocaleLowerCase('ko-KR');
        if (query === '') return filteredCharacters;
        return filteredCharacters.filter((character) =>
            `${character.name} ${character.description}`.toLocaleLowerCase('ko-KR').includes(query),
        );
    });

    function selectCharacter(character: CharacterDto): void {
        void controller.selectCharacter(character).then(onOpenConversations);
    }

    async function openSearch(): Promise<void> {
        searchOpen = true;
        await tick();
        searchInput?.focus();
    }

    function closeSearch(): void {
        if (!searchOpen || searchClosing) return;
        const container = searchContainer;
        const finishClosing = (): void => {
            searchOpen = false;
            searchQuery = '';
            searchClosing = false;
        };
        if (
            container !== null &&
            typeof container.getAnimations === 'function' &&
            !window.matchMedia('(prefers-reduced-motion: reduce)').matches
        ) {
            searchClosing = true;
            window.setTimeout(finishClosing, 380);
            return;
        }
        finishClosing();
    }

    function handleSearchKeydown(event: KeyboardEvent): void {
        if (event.key !== 'Escape') return;
        event.preventDefault();
        closeSearch();
    }

    function isSafeExportFileName(value: string): boolean {
        if (
            value.trim().length === 0 ||
            value === '.' ||
            value === '..' ||
            value.length > 1020 ||
            Array.from(value).length > 255 ||
            new TextEncoder().encode(value).length > 1020 ||
            value.includes('/') ||
            value.includes('\\')
        ) {
            return false;
        }
        for (let index = 0; index < value.length; index += 1) {
            const codeUnit = value.charCodeAt(index);
            if (codeUnit <= 0x1f || (codeUnit >= 0x7f && codeUnit <= 0x9f)) return false;
        }
        return true;
    }

    function projectCharacterExportReceipt(
        receipt: ContentSourceExportReceiptDto,
        characterId: string,
    ): ContentSourceExportReceiptDto | null {
        if (
            (receipt.kind !== 'character_card_v3' && receipt.kind !== 'charx_package') ||
            receipt.source_id !== characterId ||
            !/^[0-9a-f]{64}$/.test(receipt.sha256) ||
            !Number.isSafeInteger(receipt.size_bytes) ||
            receipt.size_bytes <= 0 ||
            !isSafeExportFileName(receipt.file_name)
        ) {
            return null;
        }
        return {
            kind: receipt.kind,
            source_id: receipt.source_id,
            sha256: receipt.sha256,
            size_bytes: receipt.size_bytes,
            file_name: receipt.file_name,
        };
    }

    async function exportCharacter(character: CharacterDto): Promise<void> {
        if (exportingCharacterId !== null) return;
        const exportContentSource = client.exportContentSource;
        if (exportContentSource === undefined) {
            exportError = t('library.export.unsupported');
            exportAnnouncement = exportError;
            return;
        }
        exportingCharacterId = character.id;
        exportError = null;
        exportAnnouncement = '';
        try {
            const receipt = await exportContentSource.call(client, {
                kind: 'character_source',
                character_id: character.id,
            });
            if (receipt === null) return;
            const projected = projectCharacterExportReceipt(receipt, character.id);
            if (projected === null) {
                exportError = t('library.export.mismatch');
                exportAnnouncement = exportError;
                return;
            }
            exportReceipt = projected;
            exportAnnouncement = t('library.export.success', { name: projected.file_name });
        } catch (error: unknown) {
            const normalized = normalizeClientError(error);
            exportError =
                normalized.messageKey === 'error.unexpected'
                    ? t('library.export.error')
                    : normalized.messageKey;
            exportAnnouncement = exportError;
        } finally {
            exportingCharacterId = null;
        }
    }
</script>

<section
    class="pane library-pane"
    class:root-view={rootView}
    aria-labelledby={rootView ? 'library-root-title' : 'library-title'}
>
    {#if rootView}
        <header
            class="mobile-top-frame mobile-root-header library-root-header"
            class:search-active={searchOpen}
            class:search-closing={searchClosing}
        >
            <h1 id="library-root-title">캐릭터</h1>
            <div class="mobile-root-actions" aria-label="캐릭터 작업" aria-hidden={searchOpen}>
                <button
                    class="mobile-top-action library-search-shortcut"
                    type="button"
                    aria-label={$tr('library.search.label')}
                    tabindex={searchOpen ? -1 : undefined}
                    disabled={appState.library.phase === 'loading'}
                    onclick={() => void openSearch()}
                >
                    <Search aria-hidden="true" />
                </button>
                <button
                    class="mobile-top-action mobile-top-add-action"
                    type="button"
                    aria-label={$tr('library.empty.import')}
                    tabindex={searchOpen ? -1 : undefined}
                    onclick={() => void controller.beginImport()}
                >
                    <Plus aria-hidden="true" />
                </button>
            </div>
            {#if searchOpen}
                <div
                    class="library-top-search"
                    class:closing={searchClosing}
                    role="search"
                    bind:this={searchContainer}
                >
                    <Search class="library-search-origin-icon" aria-hidden="true" />
                    <Search class="library-top-search-icon" aria-hidden="true" />
                    <label class="sr-only" for="library-top-search-input"
                        >{$tr('library.search.label')}</label
                    >
                    <input
                        id="library-top-search-input"
                        type="search"
                        aria-label={$tr('library.search.label')}
                        placeholder={$tr('library.search.placeholder')}
                        autocomplete="off"
                        disabled={appState.library.phase === 'loading'}
                        bind:this={searchInput}
                        bind:value={searchQuery}
                        onkeydown={handleSearchKeydown}
                    />
                    <button
                        class="library-search-close"
                        type="button"
                        aria-label={$tr('library.search.close')}
                        onclick={closeSearch}
                    >
                        <X aria-hidden="true" />
                    </button>
                </div>
            {/if}
        </header>
        <div class="library-filter-strip" role="tablist" aria-label={$tr('library.filter.label')}>
            <button
                class="library-filter-pill"
                class:active={characterFilter === 'all'}
                type="button"
                role="tab"
                aria-selected={characterFilter === 'all'}
                aria-controls="library-filtered-list"
                onclick={() => (characterFilter = 'all')}
            >
                {$tr('library.filter.all')}
            </button>
            {#each appState.library.characters as character (character.id)}
                <button
                    class="library-filter-pill"
                    class:active={characterFilter === character.id}
                    type="button"
                    role="tab"
                    aria-selected={characterFilter === character.id}
                    aria-controls="library-filtered-list"
                    onclick={() => (characterFilter = character.id)}
                >
                    {character.name}
                </button>
            {/each}
        </div>
    {/if}
    <header class="pane-header">
        {#if !rootView}
            <h1 id="library-title" class="sr-only">{$tr('library.title')}</h1>
            <button
                class="compact import-character-button"
                type="button"
                aria-label={$tr('library.empty.import')}
                onclick={() => void controller.beginImport()}
            >
                <span class="import-character-mark" aria-hidden="true">
                    <Plus class="import-character-icon" />
                </span>
                <span class="import-character-label">{$tr('library.import')}</span>
            </button>
        {/if}
        {#if !rootView && appState.selected_character !== null && visibleCharacters.some((character) => character.id === appState.selected_character?.id)}
            {@const selected = appState.selected_character}
            <button
                class="compact export-character-button"
                type="button"
                aria-label={$tr('library.export.label', { name: selected.name.slice(0, 256) })}
                disabled={exportingCharacterId !== null}
                onclick={() => void exportCharacter(selected)}
            >
                {exportingCharacterId === selected.id
                    ? $tr('library.export.busy')
                    : $tr('library.export')}
            </button>
        {/if}
    </header>

    <div class="export-status" aria-live="polite" aria-atomic="true">
        {#if exportingCharacterId !== null}
            <p role="status">{$tr('library.export.picking')}</p>
        {/if}
        {#if exportError}
            <p class="state-panel error" role="alert">{exportError}</p>
        {/if}
        {#if exportAnnouncement}
            <p class="sr-only">{exportAnnouncement}</p>
        {/if}
        {#if exportReceipt}
            <article class="export-receipt" aria-labelledby="character-export-title">
                <h2 id="character-export-title">{$tr('library.export.title')}</h2>
                <p>{$tr('library.export.file', { name: exportReceipt.file_name })}</p>
                <p>{$tr('library.export.size', { bytes: exportReceipt.size_bytes })}</p>
                <p>SHA-256 <code>{exportReceipt.sha256}</code></p>
            </article>
        {/if}
    </div>

    {#if !rootView}
        <label class="library-search">
            <Search class="library-search-icon" aria-hidden="true" />
            <span class="sr-only">{$tr('library.search.label')}</span>
            <input
                type="search"
                aria-label={$tr('library.search.label')}
                placeholder={$tr('library.search.placeholder')}
                disabled={appState.library.phase === 'loading'}
                bind:value={searchQuery}
            />
        </label>
    {/if}

    {#if appState.library.phase === 'loading'}
        <div class="state-panel" role="status">{$tr('library.loading')}</div>
    {:else if appState.library.phase === 'error'}
        <div class="state-panel error" role="alert">
            <p>{appState.library.error}</p>
            <button type="button" onclick={() => void controller.loadLibrary()}
                >{$tr('library.retry')}</button
            >
        </div>
    {:else if appState.library.characters.length > 0}
        {#if visibleCharacters.length === 0}
            <div class="state-panel empty search-empty">
                <strong>{$tr('library.search.empty')}</strong>
                <button type="button" onclick={() => (searchQuery = '')}>
                    {$tr('library.search.clear')}
                </button>
            </div>
        {:else}
            <ul
                id={rootView ? 'library-filtered-list' : undefined}
                class="entity-list"
                role={rootView ? 'tabpanel' : undefined}
                aria-label={$tr('library.list.label')}
            >
                {#each visibleCharacters as character (character.id)}
                    <li>
                        <button
                            type="button"
                            class:active={appState.selected_character?.id === character.id}
                            class="entity-row"
                            class:mobile-root-row={rootView}
                            aria-pressed={appState.selected_character?.id === character.id}
                            onclick={() => selectCharacter(character)}
                        >
                            <span class="avatar">
                                {#if character.avatar_asset_id === null}
                                    <span aria-hidden="true">{character.name.slice(0, 1)}</span>
                                {:else}
                                    <TrustedAsset
                                        {client}
                                        selector={{
                                            kind: 'asset_id',
                                            asset_id: character.avatar_asset_id,
                                        }}
                                        expectedKind="image"
                                        alt={$tr('library.character.image', {
                                            name: character.name.slice(0, 256),
                                        })}
                                    />
                                {/if}
                            </span>
                            <span class="entity-copy">
                                <strong>{character.name}</strong>
                                <span
                                    >{character.description ||
                                        $tr('library.description.empty')}</span
                                >
                            </span>
                        </button>
                    </li>
                {/each}
            </ul>
        {/if}
    {/if}
</section>

<style>
    .export-status:empty {
        display: none;
    }

    .import-character-mark {
        display: none;
    }

    .import-character-mark :global(.import-character-icon) {
        width: 24px;
        height: 24px;
    }

    .library-root-header > h1,
    .library-root-header > .mobile-root-actions {
        transition:
            opacity 180ms ease,
            transform 240ms cubic-bezier(0.22, 1, 0.36, 1);
    }

    .library-filter-strip {
        display: flex;
        width: min(100%, var(--reading));
        min-height: clamp(39px, 15.561vw, 68px);
        flex: none;
        align-items: center;
        padding: clamp(4px, 1.831vw, 8px) max(var(--mobile-top-inset), env(safe-area-inset-right))
            clamp(6px, 2.288vw, 10px) max(var(--mobile-top-inset), env(safe-area-inset-left));
        gap: clamp(6px, 2.288vw, 10px);
        margin-inline: auto;
        overflow-x: auto;
        overscroll-behavior-x: contain;
        scrollbar-width: none;
        scroll-snap-type: x proximity;
    }

    .library-filter-strip::-webkit-scrollbar {
        display: none;
    }

    .library-filter-pill {
        display: inline-flex;
        height: var(--mobile-pill-control);
        min-height: var(--mobile-pill-control);
        min-width: max-content;
        flex: none;
        align-items: center;
        justify-content: center;
        padding: 0 clamp(11px, 4.577vw, 20px);
        border: 1px solid var(--line);
        border-radius: var(--radius-pill);
        background: var(--surface-raised);
        box-shadow: none;
        color: var(--ink);
        font-size: clamp(10px, 4.119vw, 18px);
        font-weight: 700;
        letter-spacing: -0.015em;
        scroll-snap-align: start;
        transition:
            background-color 140ms ease,
            border-color 140ms ease,
            color 140ms ease,
            transform 140ms ease;
    }

    :global(.app-shell[data-layout='mobile']) .library-filter-strip {
        min-height: clamp(37px, 15.561vw, 68px);
        padding: clamp(4px, 1.831vw, 8px) max(var(--mobile-top-inset), env(safe-area-inset-right))
            clamp(5px, 2.288vw, 10px) max(var(--mobile-top-inset), env(safe-area-inset-left));
        gap: clamp(5px, 2.288vw, 10px);
    }

    :global(.app-shell[data-layout='mobile']) .library-filter-pill {
        padding-inline: clamp(11px, 4.577vw, 20px);
        font-size: clamp(10px, 4.119vw, 18px);
    }

    .library-filter-pill.active {
        border-color: var(--ink);
        background: var(--ink);
        color: var(--bg);
    }

    @media (hover: hover) and (pointer: fine) {
        .library-filter-pill:hover:not(:disabled):not(.active) {
            border-color: var(--line-strong);
            background: var(--surface-hover);
        }

        .library-filter-pill.active:hover:not(:disabled) {
            border-color: var(--ink);
            background: var(--ink);
            color: var(--bg);
        }

        :global(.app-shell[data-layout='mobile'])
            .library-pane.root-view
            .mobile-root-row.active:hover {
            background: var(--surface-hover);
        }
    }

    .library-filter-pill:active {
        transform: scale(0.97);
    }

    .library-filter-pill:focus-visible {
        outline: 2px solid var(--accent);
        outline-offset: 0;
    }

    .library-root-header.search-active:not(.search-closing) > h1 {
        opacity: 0;
        transform: translateX(-6px);
    }

    .library-root-header.search-active:not(.search-closing) > .mobile-root-actions {
        opacity: 0;
        transform: scale(0.96);
    }

    .library-root-header.search-active > .mobile-root-actions {
        pointer-events: none;
    }

    .library-root-header.search-active.search-closing > h1,
    .library-root-header.search-active.search-closing > .mobile-root-actions {
        opacity: 1;
        transform: none;
        transition-delay: 100ms;
        transition-duration: 260ms;
    }

    .library-top-search {
        --library-search-origin-after: var(--mobile-top-action);
        --library-search-origin-icon: clamp(16px, 6.865vw, 24px);
        --library-search-edge-start: max(var(--mobile-top-inset), env(safe-area-inset-left));
        --library-search-edge-end: max(var(--mobile-top-inset), env(safe-area-inset-right));

        position: absolute;
        top: calc(
            env(safe-area-inset-top) + (var(--mobile-root-header) - var(--mobile-top-action)) / 2
        );
        right: var(--library-search-edge-end);
        display: flex;
        width: calc(100% - var(--library-search-edge-start) - var(--library-search-edge-end));
        height: var(--mobile-top-action);
        min-width: 0;
        min-height: var(--mobile-top-action);
        align-items: center;
        overflow: hidden;
        padding-left: clamp(9px, 3.89vw, 16px);
        border: 1px solid var(--line);
        border-radius: var(--radius-pill);
        background: var(--surface-raised);
        box-shadow: var(--shadow-1);
        color: var(--ink-muted);
        gap: clamp(5px, 2.288vw, 10px);
        animation: library-search-expand 420ms cubic-bezier(0.22, 1, 0.36, 1) both;
        transition:
            border-color 140ms ease,
            background-color 140ms ease;
    }

    .library-top-search.closing {
        animation: library-search-collapse 360ms cubic-bezier(0.4, 0, 0.2, 1) both;
    }

    .library-top-search:focus-within {
        border-color: var(--accent);
    }

    .library-top-search :global(.library-top-search-icon) {
        width: clamp(14px, 5.492vw, 20px);
        height: clamp(14px, 5.492vw, 20px);
        flex: none;
        fill: none;
        stroke: currentcolor;
        stroke-linecap: round;
        stroke-linejoin: round;
        stroke-width: 1.8;
        animation: library-search-content-in 420ms ease-out both;
    }

    .library-top-search :global(.library-search-origin-icon) {
        position: absolute;
        top: 50%;
        right: calc(
            var(--library-search-origin-after) +
                (var(--mobile-top-action) - var(--library-search-origin-icon)) / 2
        );
        width: var(--library-search-origin-icon);
        height: var(--library-search-origin-icon);
        fill: none;
        stroke: currentcolor;
        stroke-linecap: round;
        stroke-linejoin: round;
        stroke-width: 2;
        transform: translateY(-50%);
        animation: library-search-origin-out 420ms ease-out both;
        pointer-events: none;
    }

    .library-top-search input {
        width: 100%;
        min-width: 0;
        height: 100%;
        padding: 0;
        border: 0;
        outline: 0;
        background: transparent;
        color: var(--ink);
        font: inherit;
        font-size: var(--detail-support-type);
        animation: library-search-content-in 420ms ease-out both;
    }

    .library-top-search input::placeholder {
        color: var(--ink-subtle);
    }

    .library-top-search input::-webkit-search-cancel-button {
        display: none;
    }

    .library-search-close {
        display: grid;
        width: calc(var(--mobile-top-action) - clamp(4px, 1.831vw, 8px));
        height: calc(var(--mobile-top-action) - clamp(4px, 1.831vw, 8px));
        min-width: calc(var(--mobile-top-action) - clamp(4px, 1.831vw, 8px));
        padding: 0;
        border: 0;
        border-radius: 50%;
        background: transparent;
        color: var(--ink);
        place-items: center;
        transition:
            background-color 140ms ease,
            transform 140ms ease;
        animation: library-search-content-in 420ms ease-out both;
    }

    .library-top-search.closing :global(.library-top-search-icon),
    .library-top-search.closing input,
    .library-top-search.closing .library-search-close {
        animation: library-search-content-out 240ms ease-in both;
    }

    .library-top-search.closing :global(.library-search-origin-icon) {
        animation: library-search-origin-in 360ms cubic-bezier(0.22, 1, 0.36, 1) both;
    }

    .library-search-close:active {
        background: var(--surface-active);
        transform: scale(0.96);
    }

    .library-search-close:focus-visible {
        outline: 2px solid var(--accent);
        outline-offset: -2px;
    }

    .library-search-close :global(svg) {
        width: clamp(14px, 5.492vw, 20px);
        height: clamp(14px, 5.492vw, 20px);
        fill: none;
        stroke: currentcolor;
        stroke-linecap: round;
        stroke-linejoin: round;
        stroke-width: 2;
    }

    .library-search {
        display: flex;
        min-height: 44px;
        flex: none;
        align-items: center;
        padding: 0 clamp(7px, 3.204vw, 14px);
        border: 1px solid var(--line);
        border-radius: var(--radius-pill);
        margin: 2px 14px 8px;
        background: var(--surface-raised);
        box-shadow: var(--shadow-1);
        color: var(--ink-muted);
        gap: clamp(5px, 2.288vw, 10px);
    }

    .library-search:focus-within {
        border-color: var(--accent);
    }

    .library-search :global(.library-search-icon) {
        width: clamp(11px, 4.577vw, 20px);
        height: clamp(11px, 4.577vw, 20px);
        flex: none;
        fill: none;
        stroke: currentcolor;
        stroke-linecap: round;
        stroke-width: 1.8;
    }

    .library-search input {
        width: 100%;
        min-height: 42px;
        padding: 0;
        border: 0;
        outline: 0;
        background: transparent;
        color: var(--ink);
        font-size: 0.9375rem;
    }

    .library-search input::placeholder {
        color: var(--ink-subtle);
    }

    :global(.app-shell[data-layout='desktop']) .library-search {
        min-height: 30px;
        padding-inline: 8px;
        border-radius: var(--radius-sm);
        margin: 0 4px 6px;
        box-shadow: none;
    }

    :global(.app-shell[data-layout='desktop']) .library-search input {
        min-height: 28px;
        font-size: 11px;
    }

    :global(.app-shell[data-layout='desktop']) .library-search :global(.library-search-icon) {
        width: 14px;
        height: 14px;
    }

    :global(.app-shell[data-layout='desktop']) .import-character-mark {
        display: grid;
        place-items: center;
    }

    :global(.app-shell[data-layout='desktop'])
        .import-character-mark
        :global(.import-character-icon) {
        width: 14px;
        height: 14px;
        stroke-width: 1.8;
    }

    :global(.app-shell[data-layout='desktop']) .import-character-button {
        background: var(--surface-active);
        color: var(--ink);
        gap: 7px;
    }

    @keyframes library-search-expand {
        0% {
            right: calc(var(--library-search-edge-end) + var(--library-search-origin-after));
            width: var(--mobile-top-action);
            border-color: transparent;
            background-color: transparent;
            box-shadow: none;
            opacity: 1;
            transform: scaleY(0.94);
        }

        72% {
            border-color: var(--accent);
            background-color: var(--surface-raised);
            box-shadow: var(--shadow-1);
            opacity: 1;
            transform: scaleY(1.025);
        }

        100% {
            right: var(--library-search-edge-end);
            width: calc(100% - var(--library-search-edge-start) - var(--library-search-edge-end));
            border-color: var(--accent);
            background-color: var(--surface-raised);
            box-shadow: var(--shadow-1);
            opacity: 1;
            transform: scaleY(1);
        }
    }

    @keyframes library-search-collapse {
        from {
            right: var(--library-search-edge-end);
            width: calc(100% - var(--library-search-edge-start) - var(--library-search-edge-end));
            border-color: var(--accent);
            background-color: var(--surface-raised);
            box-shadow: var(--shadow-1);
            opacity: 1;
            transform: scaleY(1);
        }

        72% {
            opacity: 1;
            transform: scaleY(0.965);
        }

        to {
            right: calc(var(--library-search-edge-end) + var(--library-search-origin-after));
            width: var(--mobile-top-action);
            border-color: transparent;
            background-color: transparent;
            box-shadow: none;
            opacity: 1;
            transform: scaleY(1);
        }
    }

    @keyframes library-search-content-in {
        0%,
        34% {
            opacity: 0;
            transform: translateX(4px);
        }

        100% {
            opacity: 1;
            transform: translateX(0);
        }
    }

    @keyframes library-search-content-out {
        from {
            opacity: 1;
        }

        to {
            opacity: 0;
            transform: translateX(6px);
        }
    }

    @keyframes library-search-origin-out {
        0%,
        28% {
            right: calc((var(--mobile-top-action) - var(--library-search-origin-icon)) / 2);
            opacity: 1;
        }

        68%,
        100% {
            right: calc(
                var(--library-search-origin-after) +
                    (var(--mobile-top-action) - var(--library-search-origin-icon)) / 2
            );
            opacity: 0;
        }
    }

    @keyframes library-search-origin-in {
        0%,
        42% {
            right: calc(
                var(--library-search-origin-after) +
                    (var(--mobile-top-action) - var(--library-search-origin-icon)) / 2
            );
            opacity: 0;
        }

        100% {
            right: calc((var(--mobile-top-action) - var(--library-search-origin-icon)) / 2);
            opacity: 1;
        }
    }

    @media (prefers-reduced-motion: reduce) {
        .library-filter-pill,
        .library-root-header > h1,
        .library-root-header > .mobile-root-actions,
        .library-top-search,
        .library-top-search :global(.library-top-search-icon),
        .library-top-search :global(.library-search-origin-icon),
        .library-top-search input,
        .library-search-close {
            animation: none;
            transition: none;
        }

        .library-top-search :global(.library-search-origin-icon) {
            display: none;
        }
    }

    .search-empty {
        display: grid;
        justify-items: center;
        gap: 10px;
    }

    .export-receipt {
        margin: 0 10px 8px;
        padding: 10px 12px;
        border: 1px solid var(--line);
        border-radius: 10px;
        overflow-wrap: anywhere;
        background: var(--surface-raised);
    }

    .export-receipt h2,
    .export-receipt p {
        margin: 0;
    }

    .export-receipt h2 {
        margin-bottom: 6px;
        font-size: 0.82rem;
    }

    .library-pane.root-view .pane-header {
        min-height: 0;
        padding: 0;
    }

    .library-pane.root-view .export-character-button {
        display: none;
    }

    .library-pane.root-view .import-character-label {
        position: absolute;
        overflow: hidden;
        width: 1px;
        height: 1px;
        padding: 0;
        border: 0;
        margin: -1px;
        clip: rect(0, 0, 0, 0);
        white-space: nowrap;
    }

    .library-pane.root-view .entity-list {
        padding: 0 8px calc(var(--mobile-nav) + 20px + env(safe-area-inset-bottom));
        gap: 0;
    }

    :global(.app-shell[data-layout='mobile']) .library-pane.root-view .entity-list {
        padding-inline: 0;
        padding-bottom: calc(
            var(--mobile-nav) + clamp(8px, 4.577vw, 20px) + env(safe-area-inset-bottom)
        );
    }

    :global(.app-shell[data-layout='mobile']) .library-pane.root-view .mobile-root-row {
        min-height: clamp(46px, 19.222vw, 84px);
        padding: clamp(4px, 1.831vw, 8px) clamp(10px, 4.119vw, 18px);
        border-radius: 0;
        gap: clamp(8px, 3.204vw, 14px);
    }

    :global(.app-shell[data-layout='mobile']) .library-pane.root-view .mobile-root-row.active {
        background: transparent;
        color: var(--ink);
        font-weight: 400;
    }

    :global(.app-shell[data-layout='mobile']) .library-pane.root-view .mobile-root-row .avatar {
        width: clamp(35px, 14.645vw, 64px);
        height: clamp(35px, 14.645vw, 64px);
        background: var(--surface-active);
        color: var(--ink);
        font-size: clamp(11px, 4.577vw, 20px);
        font-weight: 700;
    }

    :global(.app-shell[data-layout='mobile'])
        .library-pane.root-view
        .mobile-root-row
        .entity-copy
        > strong {
        font-size: clamp(11px, 4.577vw, 20px);
        font-weight: 700;
    }

    :global(.app-shell[data-layout='mobile'])
        .library-pane.root-view
        .mobile-root-row
        .entity-copy
        > span {
        font-size: clamp(9px, 3.661vw, 16px);
    }

    @media (max-width: 899px) {
        :global(.app-shell[data-layout='mobile']) .library-filter-strip {
            min-height: clamp(37px, 15.561vw, 52px);
            padding: clamp(4px, 1.831vw, 6px)
                max(var(--mobile-top-inset), env(safe-area-inset-right)) clamp(5px, 2.288vw, 8px)
                max(var(--mobile-top-inset), env(safe-area-inset-left));
            gap: clamp(5px, 2.288vw, 8px);
        }

        :global(.app-shell[data-layout='mobile']) .library-filter-pill {
            padding-inline: clamp(11px, 4.577vw, 16px);
            font-size: clamp(10px, 4.119vw, 15px);
        }

        :global(.app-shell[data-layout='mobile']) .library-pane.root-view .mobile-root-row {
            min-height: clamp(46px, 19.222vw, 68px);
            padding: clamp(4px, 1.831vw, 6px) clamp(10px, 4.119vw, 16px);
            gap: clamp(8px, 3.204vw, 12px);
        }

        :global(.app-shell[data-layout='mobile']) .library-pane.root-view .mobile-root-row .avatar {
            width: clamp(35px, 14.645vw, 52px);
            height: clamp(35px, 14.645vw, 52px);
            font-size: clamp(11px, 4.577vw, 16px);
        }

        :global(.app-shell[data-layout='mobile'])
            .library-pane.root-view
            .mobile-root-row
            .entity-copy
            > strong {
            font-size: clamp(11px, 4.577vw, 16px);
        }

        :global(.app-shell[data-layout='mobile'])
            .library-pane.root-view
            .mobile-root-row
            .entity-copy
            > span {
            font-size: clamp(9px, 3.661vw, 14px);
        }
    }
</style>
