<script lang="ts">
    import { tick, untrack } from 'svelte';
    import type { LorepiaAppController, LorepiaAppState } from '../app-controller';
    import type { LorepiaClient, ConversationMode } from '../../lib/ipc/contracts';
    import { t, tr } from '../../lib/i18n';
    import type { Overlay } from '../../ui/workspace/view-types';
    import SettingsPanel from '../../ui/workspace/SettingsPanel.svelte';
    import SettingsRow from '../../ui/workspace/SettingsRow.svelte';
    import EditField from '../../ui/workspace/EditField.svelte';
    import ConversationStartFields from './ConversationStartFields.svelte';
    import { nextConversationTitle } from '../operations/conversation-title';
    import CharacterImage from '../../ui/workspace/CharacterImage.svelte';
    import DiscardChanges from '../../ui/workspace/DiscardChanges.svelte';
    import type { BackDecision } from '../../ui/workspace/edge-back';
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
    let busy = $state(false);
    let confirming = $state(false);
    let resumeBack: (() => Promise<void>) | undefined;
    let panel: { focusBack: () => void };
    let saveError = $state('');
    let afterDiscard = () => onclose();
    let name = $state(
        untrack(() =>
            kind === 'new-chat'
                ? (appState.pending_conversation_start?.conversation.title ??
                  nextConversationTitle(appState.conversations.items, t('uiPreview.newChat')))
                : '',
        ),
    );
    const originalName = untrack(() => name);
    let personaId = $state(untrack(() => appState.pending_conversation_start?.personaId ?? ''));
    const originalPersona = untrack(() => personaId);
    let greetingId = $state(
        untrack(
            () =>
                appState.pending_conversation_start?.greetingId ??
                appState.greeting_catalog.selected_greeting_id,
        ),
    );
    const originalGreeting = untrack(() => greetingId);
    const pendingStart = $derived(
        kind === 'new-chat' && appState.pending_conversation_start !== null,
    );
    let mode = $state<ConversationMode>(
        untrack(() =>
            kind === 'new-chat'
                ? (appState.pending_conversation_start?.mode ?? 'chat')
                : (appState.conversation_state?.selected_mode ?? 'chat'),
        ),
    );
    const originalMode = untrack(() => mode);
    const dirty = $derived(
        name.trim() !== originalName ||
            mode !== originalMode ||
            personaId !== originalPersona ||
            greetingId !== originalGreeting,
    );
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
    function beforeback(): BackDecision {
        if (busy) return false;
        if (dirty) {
            afterDiscard = onclose;
            return {
                confirm: (resume) => {
                    resumeBack = resume;
                    confirming = true;
                },
            };
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
    async function keepEditing() {
        confirming = false;
        await tick();
        await resumeBack?.();
        resumeBack = undefined;
        panel.focusBack();
    }
    async function save() {
        if (busy) return;
        saveError = '';
        busy = true;
        try {
            if (kind === 'new-chat') {
                if (greetingId && !controller.selectGreeting(greetingId)) {
                    saveError = t('chat.notice.greeting_reselect');
                    return;
                }
                if (await controller.openNewConversation(name, mode, personaId || undefined))
                    oncreated();
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
            {#if kind === 'new-chat'}
                <fieldset class="start-name" disabled={busy || pendingStart}>
                    <EditField
                        label={$tr('uiPreview.chatName')}
                        value={name}
                        onchange={(value: string) => (name = value.replaceAll('\n', ' '))}
                        placeholder={$tr('uiPreview.newChat')}
                        maxlength={60}
                        hint={$tr('uiPreview.settingsEditorHint')}
                    />
                </fieldset>
                <ConversationStartFields
                    {client}
                    catalog={appState.greeting_catalog.value}
                    bind:personaId
                    bind:greetingId
                    disabled={busy || pendingStart}
                />
            {/if}
            <fieldset class="ui-chat-mode-options" disabled={busy || pendingStart}>
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
            {#if kind === 'room-settings'}
                <dl class="ui-card-details">
                    <dt>{$tr('uiPreview.chatName')}</dt>
                    <dd>{appState.selected_conversation?.title}</dd>
                </dl>
            {/if}
            {#if saveError || error}<p class="ui-field-error" role="alert">
                    {saveError || workspaceFeedback(error ?? '')}
                </p>{/if}
            {#if pendingStart && !busy}<p class="ui-field-error" role="status">
                    {$tr('workspace.finishStart')}
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

<style>
    .start-name {
        border: 0;
        padding: 0;
        margin: 0;
        min-width: 0;
    }
</style>
