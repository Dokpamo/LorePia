<script lang="ts">
    import { Plus, Search, UserRoundPlus } from '@lucide/svelte';
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
    const visibleCharacters = $derived.by(() => {
        const query = searchQuery.trim().toLocaleLowerCase('ko-KR');
        if (query === '') return appState.library.characters;
        return appState.library.characters.filter((character) =>
            `${character.name} ${character.description}`.toLocaleLowerCase('ko-KR').includes(query),
        );
    });

    function selectCharacter(character: CharacterDto): void {
        void controller.selectCharacter(character).then(onOpenConversations);
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

<section class="pane library-pane" class:root-view={rootView} aria-labelledby="library-title">
    <header class="pane-header">
        <h1 id="library-title" class="sr-only">{$tr('library.title')}</h1>
        {#if !rootView || appState.library.characters.length > 0}
            <button
                class="compact import-character-button"
                class:mobile-root-fab={rootView}
                type="button"
                aria-label={$tr('library.empty.import')}
                onclick={() => void controller.beginImport()}
            >
                <span
                    class="import-character-mark"
                    class:mobile-root-fab-mark={rootView}
                    aria-hidden="true"
                >
                    <Plus class="import-character-icon" />
                </span>
                <span class="import-character-label">{$tr('library.import')}</span>
            </button>
        {/if}
        {#if appState.selected_character !== null && visibleCharacters.some((character) => character.id === appState.selected_character?.id)}
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

    <label class="library-search" class:mobile-root-search={rootView}>
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

    {#if appState.library.phase === 'loading'}
        <div class="state-panel" role="status">{$tr('library.loading')}</div>
    {:else if appState.library.phase === 'error'}
        <div class="state-panel error" role="alert">
            <p>{appState.library.error}</p>
            <button type="button" onclick={() => void controller.loadLibrary()}
                >{$tr('library.retry')}</button
            >
        </div>
    {:else if appState.library.characters.length === 0}
        {#if rootView}
            <div class="mobile-root-contact-action">
                <button
                    class="primary mobile-root-contact-button"
                    type="button"
                    onclick={() => void controller.beginImport()}
                >
                    <UserRoundPlus class="import-contact-icon" aria-hidden="true" />
                    <span>{$tr('library.empty.import')}</span>
                </button>
            </div>
        {/if}
    {:else if visibleCharacters.length === 0}
        <div class="state-panel empty search-empty">
            <strong>{$tr('library.search.empty')}</strong>
            <button type="button" onclick={() => (searchQuery = '')}>
                {$tr('library.search.clear')}
            </button>
        </div>
    {:else}
        <ul class="entity-list" aria-label={$tr('library.list.label')}>
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
                            <span>{character.description || $tr('library.description.empty')}</span>
                        </span>
                    </button>
                </li>
            {/each}
        </ul>
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

    .library-search {
        display: flex;
        min-height: 44px;
        flex: none;
        align-items: center;
        padding: 0 clamp(10px, 3.204vw, 14px);
        border: 1px solid var(--line);
        border-radius: var(--radius-pill);
        margin: 2px 14px 8px;
        background: var(--surface-raised);
        box-shadow: var(--shadow-1);
        color: var(--ink-muted);
        gap: clamp(8px, 2.288vw, 10px);
    }

    .library-search:focus-within {
        border-color: var(--accent);
    }

    .library-search :global(.library-search-icon) {
        width: clamp(16px, 4.577vw, 20px);
        height: clamp(16px, 4.577vw, 20px);
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
        padding: 0 8px calc(var(--mobile-nav) + 92px + env(safe-area-inset-bottom));
        gap: 0;
    }
</style>
