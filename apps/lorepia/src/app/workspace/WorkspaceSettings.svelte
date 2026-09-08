<script lang="ts">
    import { tick, untrack } from 'svelte';
    import type { LorepiaAppController, LorepiaAppState } from '../app-controller';
    import type { LorepiaClient, ConversationMode } from '../../lib/ipc/contracts';
    import { t, tr } from '../../lib/i18n';
    import type { Overlay } from '../../ui/workspace/view-types';
    import SettingsPanel from '../../ui/workspace/SettingsPanel.svelte';
    import SettingsRow from '../../ui/workspace/SettingsRow.svelte';
    import EditField from '../../ui/workspace/EditField.svelte';
    import ChoiceField from '../../ui/workspace/ChoiceField.svelte';
    import CharacterImage from '../../ui/workspace/CharacterImage.svelte';
    import DiscardChanges from '../../ui/workspace/DiscardChanges.svelte';
    import { useChoiceSheet } from '../../ui/workspace/choice-sheet.svelte';
    import { workspaceFeedback } from './workspace-feedback';

    let {
        kind,
        appState,
        controller,
        client,
        onclose,
        oncreated,
        onproviders,
        onadvanced,
        covered = false,
    }: {
        kind: Overlay;
        appState: LorepiaAppState;
        controller: LorepiaAppController;
        client: LorepiaClient;
        onclose: () => void;
        oncreated: () => void;
        onproviders: () => void;
        onadvanced: () => void;
        covered?: boolean;
    } = $props();
    const choices = useChoiceSheet();
    let busy = $state(false);
    let confirming = $state(false);
    let panel: { focusBack: () => void };
    let saveError = $state('');
    let afterDiscard = () => onclose();
    let name = $state('');
    let mode = $state<ConversationMode>(
        untrack(() =>
            kind === 'new-chat' ? 'chat' : (appState.conversation_state?.selected_mode ?? 'chat'),
        ),
    );
    const originalMode = untrack(() => mode);
    const dirty = $derived(name.trim() !== '' || mode !== originalMode);
    const title = $derived(
        $tr(
            kind === 'new-chat'
                ? 'uiPreview.newChat'
                : kind === 'room-settings'
                  ? 'uiPreview.roomSettings'
                  : kind === 'card-settings'
                    ? 'uiPreview.cardSettings'
                    : 'uiPreview.cardInfo',
        ),
    );
    const character = $derived(appState.selected_character);
    const error = $derived(appState.conversations.error ?? appState.greeting_catalog.error);
    function beforeback() {
        if (busy) return false;
        if (dirty) {
            afterDiscard = onclose;
            confirming = true;
            return false;
        }
        return true;
    }
    function navigate(action: () => void) {
        if (busy) return;
        if (dirty) {
            afterDiscard = action;
            confirming = true;
        } else action();
    }
    function keepEditing() {
        confirming = false;
        void tick().then(() => panel.focusBack());
    }
    function greetingLabel(id: string) {
        const items = appState.greeting_catalog.value?.greetings ?? [];
        const item = items.find((value) => value.id === id);
        return item?.kind === 'default'
            ? t('workspace.defaultGreeting')
            : t('workspace.alternateGreeting', {
                  number:
                      items
                          .filter((value) => value.kind === 'alternate')
                          .findIndex((value) => value.id === id) + 1,
              });
    }
    async function save() {
        if (busy) return;
        saveError = '';
        busy = true;
        try {
            if (kind === 'new-chat') {
                if (await controller.openNewConversation(name, mode)) oncreated();
            } else {
                if (
                    appState.conversation_state?.selected_mode === mode ||
                    (await controller.setConversationMode(mode))
                )
                    onclose();
                else
                    saveError =
                        workspaceFeedback(appState.announcement) || t('workspace.saveFailed');
            }
        } finally {
            busy = false;
        }
    }
</script>

<SettingsPanel
    bind:this={panel}
    {title}
    {kind}
    {onclose}
    {beforeback}
    disabled={busy}
    covered={covered || confirming}
>
    {#if kind === 'new-chat' || kind === 'room-settings'}
        <form
            class="ui-edit-form"
            onsubmit={(event) => {
                event.preventDefault();
                void save();
            }}
        >
            <fieldset class="ui-chat-mode-options" disabled={busy}>
                <legend>{$tr('uiPreview.conversationMode')}</legend>
                {#each ['chat', 'story'] as value (value)}
                    <label>
                        <input type="radio" name="ui-conversation-mode" {value} bind:group={mode} />
                        <span>
                            <strong
                                >{$tr(
                                    value === 'chat' ? 'uiPreview.chatMode' : 'uiPreview.storyMode',
                                )}</strong
                            >
                            <small
                                >{$tr(
                                    value === 'chat'
                                        ? 'uiPreview.chatModeHint'
                                        : 'uiPreview.storyModeHint',
                                )}</small
                            >
                        </span>
                    </label>
                {/each}
            </fieldset>
            {#if kind === 'new-chat'}
                <EditField
                    label={$tr('uiPreview.chatName')}
                    value={name}
                    onchange={(value: string) => (name = value.replaceAll('\n', ' '))}
                    placeholder={$tr('uiPreview.newChat')}
                    maxlength={60}
                    hint={$tr('uiPreview.settingsEditorHint')}
                />
                {#if appState.greeting_catalog.value}
                    <ChoiceField
                        label={$tr('workspace.greeting')}
                        value={appState.greeting_catalog.selected_greeting_id
                            ? greetingLabel(appState.greeting_catalog.selected_greeting_id)
                            : $tr('workspace.noGreeting')}
                        onopen={(opener: HTMLButtonElement) =>
                            choices.open(
                                {
                                    label: t('workspace.greeting'),
                                    value: appState.greeting_catalog.selected_greeting_id ?? '',
                                    options: (appState.greeting_catalog.value?.greetings ?? [])
                                        .filter((item) => item.enabled)
                                        .map((item) => ({
                                            value: item.id,
                                            label: greetingLabel(item.id),
                                        })),
                                    onselect: (value) => {
                                        controller.selectGreeting(value);
                                    },
                                },
                                opener,
                            )}
                    />
                {/if}
            {:else}
                <dl class="ui-card-details">
                    <dt>{$tr('uiPreview.chatName')}</dt>
                    <dd>{appState.selected_conversation?.title}</dd>
                </dl>
            {/if}
            {#if saveError || error}<p class="ui-field-error" role="alert">
                    {saveError || workspaceFeedback(error ?? '')}
                </p>{/if}
            <button
                type="submit"
                class="ui-submit ui-pressable"
                disabled={busy ||
                    (kind === 'new-chat' && appState.greeting_catalog.phase !== 'ready')}
            >
                <span class="ui-press-visual"
                    >{$tr(kind === 'new-chat' ? 'uiPreview.startChat' : 'uiPreview.save')}</span
                >
            </button>
        </form>
        {#if kind === 'room-settings'}
            <section class="ui-settings-group">
                <SettingsRow
                    label={$tr('workspace.aiSettings')}
                    disabled={busy}
                    onclick={() => navigate(onproviders)}
                />
                <SettingsRow
                    label={$tr('workspace.advanced')}
                    disabled={busy}
                    onclick={() => navigate(onadvanced)}
                />
            </section>
        {/if}
    {:else}
        <div class="ui-card-cover" role="img" aria-label={$tr('uiPreview.cardImage')}>
            {#if character?.avatar_asset_id}
                <CharacterImage
                    {client}
                    assetId={character.avatar_asset_id}
                    name={character.name}
                />
            {:else}{character?.name.slice(0, 1)}{/if}
        </div>
        <dl class="ui-card-details">
            <dt>{$tr('uiPreview.cardName')}</dt>
            <dd>{character?.name}</dd>
            <dt>{$tr('uiPreview.cardDescription')}</dt>
            <dd>
                {#if character?.description}
                    {character.description}
                {:else}
                    {$tr('workspace.noDescription')}
                {/if}
            </dd>
        </dl>
        {#if kind === 'card-settings'}
            <section class="ui-settings-group">
                <SettingsRow label={$tr('workspace.advanced')} onclick={onadvanced} />
            </section>
            <p class="ui-live-hint">{$tr('workspace.cardReadOnly')}</p>
        {/if}
    {/if}
</SettingsPanel>
{#if confirming}
    <DiscardChanges onkeep={keepEditing} ondiscard={() => afterDiscard()} />
{/if}
