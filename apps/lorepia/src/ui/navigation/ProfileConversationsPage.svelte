<script lang="ts">
    import { MessageCircle } from '@lucide/svelte';
    import { tr } from '../../lib/i18n';
    import type { SampleCharacter } from '../workspace/view-types';
    import type { ConversationListItem } from './navigation-types';
    import { queryConversationLibrary } from './conversation-library-query';
    import SettingsPanel from '../workspace/SettingsPanel.svelte';
    import LastChatTime from './LastChatTime.svelte';

    let {
        character,
        conversations,
        loading,
        error,
        covered = false,
        onclose,
        onopen,
        onnew,
        onretry,
    }: {
        character: SampleCharacter;
        conversations: ConversationListItem[];
        loading: boolean;
        error: string | null;
        covered?: boolean;
        onclose: () => void;
        onopen: (id: string) => void;
        onnew: (trigger: HTMLButtonElement) => void;
        onretry: () => void;
    } = $props();
    const items = $derived(queryConversationLibrary(conversations, '', character.id, 'newest'));
</script>

<SettingsPanel
    title={$tr('uiPreview.history')}
    kind="profile-resource"
    inlineTitle
    {covered}
    {onclose}
>
    <p class="seed-profile-caption">{character.name}</p>
    {#if error}
        <div class="seed-inline-error" role="alert">
            <p>{error}</p>
            <button class="seed-secondary ui-pressable" onclick={onretry}
                ><span class="ui-press-visual">{$tr('workspace.retry')}</span></button
            >
        </div>
    {/if}
    {#if loading && !items.length}
        <p class="seed-profile-caption" role="status">{$tr('workspace.loading')}</p>
    {/if}
    <div class="seed-profile-conversations" aria-label={$tr('uiPreview.history')} role="region">
        {#each items as item (item.id)}
            <div class="seed-conversation-row">
                <button class="seed-row ui-pressable" onclick={() => onopen(item.id)}
                    ><span class="ui-press-visual"
                        ><span class="seed-row-copy"><strong>{item.title}</strong></span
                        ><LastChatTime value={item.updatedAt} /></span
                    ></button
                >
            </div>
        {:else}
            {#if !loading && !error}
                <div class="seed-empty">
                    <MessageCircle aria-hidden="true" />
                    <h3>{$tr('navigation.noChats')}</h3>
                    <p>{$tr('navigation.noCharacterChatsHint')}</p>
                </div>
            {/if}
        {/each}
    </div>
    {#snippet footer()}
        <button
            class="ui-submit seed-primary ui-pressable"
            onclick={(event) => onnew(event.currentTarget)}
            ><span class="ui-press-visual">{$tr('uiPreview.newChat')}</span></button
        >
    {/snippet}
</SettingsPanel>

<style>
    .seed-profile-conversations {
        margin-top: 16px;
    }
</style>
