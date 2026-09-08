<script lang="ts">
    import { BookOpen, Image, Settings, MessageSquare, Blocks } from '@lucide/svelte';
    import { tr } from '../../lib/i18n';
    import type { LorepiaClient } from '../../lib/ipc/contracts';
    import type { SampleCharacter, Overlay } from '../workspace/view-types';
    import CharacterImage from '../workspace/CharacterImage.svelte';
    import SettingsPanel from '../workspace/SettingsPanel.svelte';
    import NavigationRow from './NavigationRow.svelte';
    let {
        character,
        client,
        onclose,
        onaction,
        onchat,
        onmaterials,
        onplugins,
        covered = false,
    }: {
        covered?: boolean;
        character: SampleCharacter;
        client: LorepiaClient;
        onclose: () => void;
        onaction: (kind: Overlay, trigger: HTMLButtonElement) => void;
        onchat: () => void;
        onmaterials: () => void;
        onplugins: () => void;
    } = $props();
</script>

<SettingsPanel title={$tr('navigation.characterInfo')} {onclose} {covered}>
    <div class="seed-character-overview">
        <div class="seed-character-portrait">
            {#if character.avatarAssetId}<CharacterImage
                    {client}
                    name={character.name}
                    assetId={character.avatarAssetId}
                />
            {:else}<span>{character.thumbnail}</span>{/if}
        </div>
        <h2>{character.name}</h2>
        <p>{character.description}</p>
    </div>
    <button
        class="ui-submit seed-primary ui-pressable"
        onclick={(event: MouseEvent & { currentTarget: HTMLButtonElement }) =>
            onaction('new-chat', event.currentTarget)}
    >
        <span class="ui-press-visual"
            ><MessageSquare aria-hidden="true" />{$tr('navigation.startChat')}</span
        >
    </button>
    <section class="seed-group">
        <NavigationRow title={$tr('navigation.viewChats')} onclick={onchat}
            >{#snippet prefix()}<MessageSquare />{/snippet}</NavigationRow
        >
        <NavigationRow
            title={$tr('uiPreview.cardInfo')}
            onclick={(event: MouseEvent & { currentTarget: HTMLButtonElement }) =>
                onaction('card-info', event.currentTarget)}
        >
            {#snippet prefix()}<Image />{/snippet}
        </NavigationRow>
        <NavigationRow
            title={$tr('navigation.characterMaterials')}
            description={$tr('navigation.characterTools')}
            onclick={onmaterials}
        >
            {#snippet prefix()}<BookOpen />{/snippet}
        </NavigationRow>
        <NavigationRow title={$tr('settings.section.plugins.title')} onclick={onplugins}>
            {#snippet prefix()}<Blocks />{/snippet}
        </NavigationRow>
        <NavigationRow
            title={$tr('uiPreview.cardSettings')}
            onclick={(event: MouseEvent & { currentTarget: HTMLButtonElement }) =>
                onaction('card-settings', event.currentTarget)}
        >
            {#snippet prefix()}<Settings />{/snippet}
        </NavigationRow>
    </section>
</SettingsPanel>
