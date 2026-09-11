<script lang="ts">
    import { UserRound } from '@lucide/svelte';
    import { tr } from '../../lib/i18n';
    import type { LorepiaClient } from '../../lib/ipc/contracts';
    import type { SampleCharacter } from './view-types';
    import CharacterImage from './CharacterImage.svelte';

    let {
        client,
        character,
        user,
        personaName,
    }: {
        client?: Pick<LorepiaClient, 'resolveAssetDelivery'>;
        character: SampleCharacter;
        user: boolean;
        personaName?: string;
    } = $props();
</script>

<div class="ui-message-speaker">
    <span class="ui-message-avatar" aria-hidden="true">
        {#if !user && client && character.avatarAssetId}
            <CharacterImage {client} name={character.name} assetId={character.avatarAssetId} />
        {:else if user}<UserRound />
        {:else}<span>{character.thumbnail}</span>{/if}
    </span>
    <strong>{user ? (personaName ?? $tr('uiPreview.you')) : character.name}</strong>
</div>
