<script lang="ts">
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
            exportError = '현재 Core가 안전한 콘텐츠 소스 내보내기 API를 제공하지 않습니다.';
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
                exportError = 'Core 내보내기 영수증이 선택한 캐릭터와 일치하지 않습니다.';
                exportAnnouncement = exportError;
                return;
            }
            exportReceipt = projected;
            exportAnnouncement = `${projected.file_name} 파일로 캐릭터 소스를 내보냈습니다.`;
        } catch (error: unknown) {
            const normalized = normalizeClientError(error);
            exportError =
                normalized.messageKey === 'error.unexpected'
                    ? '캐릭터 소스를 내보내지 못했습니다.'
                    : normalized.messageKey;
            exportAnnouncement = exportError;
        } finally {
            exportingCharacterId = null;
        }
    }
</script>

<section class="pane library-pane" aria-labelledby="library-title">
    <header class="pane-header">
        <div>
            <p class="eyebrow">Local library</p>
            <h1 id="library-title">서재</h1>
        </div>
        <button class="primary compact" type="button" onclick={() => void controller.beginImport()}>
            가져오기
        </button>
    </header>

    <div class="export-status" aria-live="polite" aria-atomic="true">
        {#if exportingCharacterId !== null}
            <p role="status">운영체제 저장 위치를 선택하고 있습니다.</p>
        {/if}
        {#if exportError}
            <p class="state-panel error" role="alert">{exportError}</p>
        {/if}
        {#if exportAnnouncement}
            <p class="sr-only">{exportAnnouncement}</p>
        {/if}
        {#if exportReceipt}
            <article class="export-receipt" aria-labelledby="character-export-title">
                <h2 id="character-export-title">최근 캐릭터 내보내기</h2>
                <p>파일명 {exportReceipt.file_name}</p>
                <p>크기 {exportReceipt.size_bytes}바이트</p>
                <p>SHA-256 <code>{exportReceipt.sha256}</code></p>
            </article>
        {/if}
    </div>

    {#if appState.library.phase === 'loading'}
        <div class="state-panel" role="status">캐릭터를 불러오는 중입니다.</div>
    {:else if appState.library.phase === 'error'}
        <div class="state-panel error" role="alert">
            <p>{appState.library.error}</p>
            <button type="button" onclick={() => void controller.loadLibrary()}>다시 시도</button>
        </div>
    {:else if appState.library.characters.length === 0}
        <div class="state-panel empty">
            <strong>아직 캐릭터가 없습니다.</strong>
            <p>로컬 CCv3 JSON 또는 CHARX 파일을 안전하게 검사한 뒤 추가할 수 있습니다.</p>
            <button class="primary" type="button" onclick={() => void controller.beginImport()}>
                첫 캐릭터 가져오기
            </button>
        </div>
    {:else}
        <ul class="entity-list" aria-label="캐릭터 목록">
            {#each appState.library.characters as character (character.id)}
                <li>
                    <div class="character-row">
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
                                        alt={`${character.name.slice(0, 256)} 캐릭터 이미지`}
                                    />
                                {/if}
                            </span>
                            <span class="entity-copy">
                                <strong>{character.name}</strong>
                                <span>{character.description || '설명이 없습니다.'}</span>
                            </span>
                        </button>
                        <button
                            class="compact"
                            type="button"
                            aria-label={`${character.name.slice(0, 256)} 캐릭터 소스 내보내기`}
                            disabled={exportingCharacterId !== null}
                            onclick={() => void exportCharacter(character)}
                        >
                            {exportingCharacterId === character.id ? '내보내는 중…' : '내보내기'}
                        </button>
                    </div>
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

    .character-row {
        display: grid;
        grid-template-columns: minmax(0, 1fr) auto;
        gap: 6px;
        align-items: center;
    }
</style>
