<script lang="ts">
    import { onDestroy, tick, untrack } from 'svelte';
    import { get } from 'svelte/store';
    import type { LorepiaAppController, LorepiaAppState } from '../app-controller';
    import type { LorepiaClient } from '../../lib/ipc/contracts';
    import {
        CHAT_DISPLAY_MODES,
        chatDisplayPreferences,
        conversationDisplayMode,
        generationMode,
        setConversationDisplayMode,
        type ChatDisplayMode,
    } from '../../lib/chat-display';
    import { t, tr } from '../../lib/i18n';
    import type { Overlay } from '../../ui/workspace/view-types';
    import SettingsPanel from '../../ui/workspace/SettingsPanel.svelte';
    import SettingsRow from '../../ui/workspace/SettingsRow.svelte';
    import ConversationStartFields from './ConversationStartFields.svelte';
    import ConversationPersonaCreate from './ConversationPersonaCreate.svelte';
    import type { PersonaController } from '../../features/personas/persona-controller';
    import type { PersonaDto } from '../../features/personas/persona-contracts';
    import { nextConversationTitle } from '../operations/conversation-title';
    import CharacterImage from '../../ui/workspace/CharacterImage.svelte';
    import DiscardChanges from '../../ui/workspace/DiscardChanges.svelte';
    import type { BackDecision } from '../../ui/workspace/edge-back';
    import { workspaceFeedback } from './workspace-feedback';
    import './conversation-start.css';

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
    const formId = $props.id();
    const characterId = untrack(() => appState.selected_character?.id);
    const conversationId = untrack(() => appState.selected_conversation?.id);
    let mounted = true;
    onDestroy(() => {
        mounted = false;
    });
    let busy = $state(false);
    let confirming = $state(false);
    let addingPersona = $state<PersonaController | null>(null);
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
    let mode = $state<ChatDisplayMode>(
        untrack(() => {
            const pending = appState.pending_conversation_start;
            if (kind === 'new-chat' && !pending) return 'default';
            return conversationDisplayMode(
                kind === 'new-chat' ? pending?.conversation.id : conversationId,
                (kind === 'new-chat'
                    ? pending?.mode
                    : appState.conversation_state?.selected_mode) ?? 'chat',
                get(chatDisplayPreferences),
            );
        }),
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
            const nativeMode = generationMode(mode);
            if (kind === 'new-chat') {
                if (greetingId && !controller.selectGreeting(greetingId)) {
                    saveError = t('chat.notice.greeting_reselect');
                    return;
                }
                const created = await controller.openNewConversation(
                    name,
                    nativeMode,
                    personaId || undefined,
                );
                const current = get(controller.state);
                if (!mounted || current.selected_character?.id !== characterId) return;
                const room = created
                    ? current.selected_conversation
                    : current.pending_conversation_start?.conversation;
                if (room && room.character_id === characterId)
                    setConversationDisplayMode(room.id, mode);
                if (created) oncreated();
            } else {
                if (get(controller.state).selected_conversation?.id !== conversationId) return;
                const saved =
                    appState.conversation_state?.selected_mode === nativeMode ||
                    (await controller.setConversationMode(nativeMode));
                const current = get(controller.state);
                if (!mounted || current.selected_conversation?.id !== conversationId) return;
                if (
                    saved &&
                    conversationId &&
                    current.conversation_state?.selected_mode === nativeMode
                ) {
                    setConversationDisplayMode(conversationId, mode);
                    onclose();
                } else
                    saveError =
                        workspaceFeedback(appState.announcement) || t('workspace.saveFailed');
            }
        } finally {
            busy = false;
        }
    }
</script>

{#snippet startAction()}
    <button
        type="submit"
        form={formId}
        class="ui-submit ui-pressable"
        disabled={busy || appState.greeting_catalog.phase !== 'ready'}
    >
        <span class="ui-press-visual"
            >{$tr(busy ? 'workspace.loading' : 'uiPreview.startChat')}</span
        >
    </button>
{/snippet}

<SettingsPanel
    bind:this={panel}
    {title}
    {kind}
    {onclose}
    {beforeback}
    disabled={busy}
    covered={covered || confirming || addingPersona !== null}
    inlineTitle={kind === 'new-chat'}
    footer={kind === 'new-chat' ? startAction : undefined}
>
    {#if kind === 'new-chat' || kind === 'room-settings'}
        <form
            id={formId}
            aria-label={title}
            class="ui-edit-form"
            onsubmit={(event) => {
                event.preventDefault();
                void save();
            }}
        >
            {#if kind === 'new-chat'}
                <ConversationStartFields
                    {client}
                    catalog={appState.greeting_catalog.value}
                    bind:name
                    bind:mode
                    bind:personaId
                    bind:greetingId
                    disabled={busy || pendingStart}
                    onaddpersona={(personas: PersonaController) => {
                        addingPersona = personas;
                    }}
                />
            {:else}
                <fieldset class="ui-chat-mode-options" disabled={busy || pendingStart}>
                    <legend>{$tr('uiPreview.conversationMode')}</legend>
                    {#each CHAT_DISPLAY_MODES as option (option.value)}
                        <label>
                            <input
                                type="radio"
                                name="ui-conversation-mode"
                                value={option.value}
                                bind:group={mode}
                            />
                            <span>
                                <strong>{$tr(option.label)}</strong>
                                <small>{$tr(option.hint)}</small>
                            </span>
                        </label>
                    {/each}
                </fieldset>
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
            {#if kind === 'room-settings'}<button
                    type="submit"
                    class="ui-submit ui-pressable"
                    disabled={busy}
                >
                    <span class="ui-press-visual">{$tr('uiPreview.save')}</span>
                </button>{/if}
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
{#if addingPersona}
    <ConversationPersonaCreate
        controller={addingPersona}
        {covered}
        onclose={() => {
            addingPersona = null;
        }}
        oncreated={(persona: PersonaDto) => {
            personaId = persona.value.id;
            addingPersona = null;
        }}
    />
{/if}
{#if confirming}
    <DiscardChanges onkeep={keepEditing} ondiscard={() => afterDiscard()} />
{/if}
