<script lang="ts">
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
    }

    let { state: appState, controller, client, onOpenConversations }: Props = $props();
    let exportingCharacterId = $state<string | null>(null);
    let exportReceipt = $state<ContentSourceExportReceiptDto | null>(null);
    let exportError = $state<string | null>(null);
    let exportAnnouncement = $state('');

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

<section class="pane library-pane" aria-labelledby="library-title">
    <header class="pane-header">
        <h1 id="library-title" class="sr-only">{$tr('library.title')}</h1>
        <button class="compact" type="button" onclick={() => void controller.beginImport()}>
            {$tr('library.import')}
        </button>
        {#if appState.selected_character !== null}
            {@const selected = appState.selected_character}
            <button
                class="compact"
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
        <div class="state-panel empty">
            <strong>{$tr('library.empty.title')}</strong>
            <p>{$tr('library.empty.hint')}</p>
            <button class="primary" type="button" onclick={() => void controller.beginImport()}>
                {$tr('library.empty.import')}
            </button>
        </div>
    {:else}
        <ul class="entity-list" aria-label={$tr('library.list.label')}>
            {#each appState.library.characters as character (character.id)}
                <li>
                    <button
                        type="button"
                        class:active={appState.selected_character?.id === character.id}
                        class="entity-row"
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

    /*
     * Export is a rare, deliberate action. Keeping it hidden until the row is
     * hovered or focused lets the list read as a roster of characters instead
     * of a table of controls, without removing it from the tab order.
     */

    @media (hover: none) {
        .character-row > :global(button:last-child) {
            opacity: 1;
        }
    }
</style>
