<script lang="ts">
    import { onMount, tick, untrack } from 'svelte';
    import { get } from 'svelte/store';
    import { Settings, Plus } from '@lucide/svelte';
    import {
        LorepiaAppController,
        INITIAL_APP_STATE,
        type LorepiaAppState,
    } from '../app-controller';
    import type { LorepiaClient } from '../../lib/ipc/contracts';
    import { createLiveLorepiaClient } from '../../lib/ipc/client';
    import { t, tr } from '../../lib/i18n';
    import { themePreference } from '../../lib/theme';
    import { PortableRuntimeLifecycle } from '../../features/chat/portable-runtime-lifecycle.svelte';
    import PortableMessage from '../../features/chat/PortableMessage.svelte';
    import ImportReviewDialog from '../../features/import/ImportReviewDialog.svelte';
    import WorkspaceFrame from '../../ui/workspace/WorkspaceFrame.svelte';
    import CharacterPage from '../../ui/workspace/CharacterPage.svelte';
    import ChatPage from '../../ui/workspace/ChatPage.svelte';
    import IconButton from '../../ui/workspace/IconButton.svelte';
    import WorkspaceSettings from './WorkspaceSettings.svelte';
    import WorkspaceFeatures from './WorkspaceFeatures.svelte';
    import WorkspaceCreator from './WorkspaceCreator.svelte';
    import WorkspaceChatExtras from './WorkspaceChatExtras.svelte';
    import type { Overlay, Page } from '../../ui/workspace/view-types';
    import { LiveChatSession } from './live-chat-session.svelte';
    import { characterViews, conversationView } from './workspace-projection';

    let { client }: { client?: LorepiaClient } = $props();
    const appClient = untrack(() => client ?? createLiveLorepiaClient());
    const controller = new LorepiaAppController(appClient);
    let appState = $state.raw<LorepiaAppState>(structuredClone(INITIAL_APP_STATE));
    let refreshEpoch = $state(0);
    const session = new LiveChatSession(controller, () => appState, {
        onMutation: () => (refreshEpoch += 1),
        onRemoved: () => runtime.resetScope(),
    });
    let page = $state<Page>(0);
    let overlay = $state<Overlay | 'providers' | 'studio' | null>(null);
    let returnPage: Page = 0;
    let opener: HTMLElement | null = null;
    let notice = $state('');
    let mounted = true;
    const runtime = new PortableRuntimeLifecycle({
        currentMessages: () => appState.messages.items,
        displayMessages: () =>
            appState.messages.items.filter(
                (item) => item.id !== appState.chat.live_assistant_message_id,
            ),
        providerWorkspace: () => appState.providers.workspace,
        primarySelection: () => controller.runtimeGenerationSelection(),
        sendMessage: (...args) => controller.sendMessage(...args),
        onNotice: (value) => (notice = value),
    });
    const subpage = $derived(
        !!runtime.profile &&
            (runtime.requiresRuntimeWorker ||
                !!runtime.profile.background_markup.trim() ||
                runtime.profile.display_transforms.length > 0 ||
                runtime.profile.output_transforms.length > 0),
    );
    const characters = $derived(characterViews(appState, subpage));
    const character = $derived(
        characters.find((item) => item.id === appState.selected_character?.id),
    );
    const conversation = $derived(conversationView(appState));
    const error = $derived(
        appState.bootstrap.error ??
            appState.library.error ??
            appState.conversations.error ??
            appState.messages.error ??
            appState.chat.error,
    );
    onMount(() => {
        const unsubscribe = controller.state.subscribe((value) => (appState = value));
        void start();
        return () => {
            mounted = false;
            unsubscribe();
            controller.destroy();
        };
    });
    async function start() {
        await controller.start();
        const state = get(controller.state);
        if (mounted && !state.selected_character && state.library.characters[0])
            await controller.selectCharacter(state.library.characters[0]);
    }
    $effect(() =>
        runtime.loadProfile(
            appClient,
            appState.selected_character?.id ?? null,
            appState.selected_conversation?.id ?? null,
            appState.conversation_state?.active_branch_id ?? null,
        ),
    );
    $effect(() =>
        runtime.recreate({
            client: appClient,
            character: appState.selected_character,
            conversationId: appState.selected_conversation?.id ?? null,
            branchId: appState.conversation_state?.active_branch_id ?? null,
        }),
    );
    $effect(() =>
        runtime.syncMessages({
            messages: appState.messages.items,
            activeGenerationId: appState.chat.active_generation_id,
            hasStreamingPresentation:
                appState.chat.live_assistant_message_id !== null ||
                !!appState.chat.streaming_text ||
                !!appState.chat.reasoning_text,
        }),
    );
    $effect(() => {
        if (!subpage && page === 2) page = 1;
    });
    function selectCharacter(id: string) {
        const item = appState.library.characters.find((value) => value.id === id);
        if (item) {
            notice = '';
            void controller.selectCharacter(item);
        }
    }
    function selectConversation(id: string, navigate: (page: Page) => void) {
        const item = appState.conversations.items.find((value) => value.id === id);
        if (!item) return;
        notice = '';
        void controller.selectConversation(item);
        navigate(1);
    }
    function open(kind: Overlay, trigger: HTMLButtonElement) {
        if (kind === 'add-character') {
            void controller.beginImport();
            return;
        }
        opener = trigger;
        returnPage = trigger.closest('.ui-chat') ? 1 : page;
        page = 0;
        overlay = kind;
    }
    function close() {
        overlay = null;
        page = returnPage;
        void tick().then(() => opener?.isConnected && opener.focus({ preventScroll: true }));
    }
    async function send() {
        notice = '';
        try {
            await session.send((text) =>
                runtime.dispatchInput(text, (content, variables) =>
                    controller.sendMessage(content, variables),
                ),
            );
        } catch (error) {
            notice = runtime.fail(error, t('chat.runtime.execution_failed'));
        }
    }
</script>

<WorkspaceFrame
    bind:page
    overlay={overlay !== null || appState.import_flow.phase !== 'idle'}
    {subpage}
    appearance={$themePreference}
    conversationMode={conversation?.mode ?? 'chat'}
    onback={close}
>
    {#snippet management(navigate)}
        {#if character}
            <CharacterPage
                {characters}
                {character}
                client={appClient}
                conversationId={conversation?.id ?? ''}
                oncharacter={selectCharacter}
                onconversation={(id) => selectConversation(id, navigate)}
                onaction={open}
            />
            {#if appState.conversations.phase === 'loading'}<p class="ui-live-status" role="status">
                    {$tr('workspace.loading')}
                </p>{/if}
        {:else}
            <header class="ui-page-header">
                <IconButton
                    label={$tr('uiPreview.appSettings')}
                    onclick={(event) => open('app-settings', event.currentTarget)}
                    ><Settings /></IconButton
                >
                <h1>LorePia</h1>
            </header>
            <div class="ui-live-empty">
                <p>
                    {appState.bootstrap.phase === 'loading' || appState.library.phase === 'loading'
                        ? $tr('workspace.loading')
                        : $tr('workspace.emptyLibrary')}
                </p>
                <button
                    class="ui-submit ui-pressable"
                    disabled={appState.bootstrap.phase !== 'ready'}
                    onclick={() => void controller.beginImport()}
                    ><Plus />{$tr('workspace.importCard')}</button
                >
            </div>
        {/if}
        {#if error && page === 0 && !overlay}<div class="ui-live-error" role="alert">
                <p>{error}</p>
                <button class="ui-result-action" onclick={() => void start()}
                    >{$tr('workspace.retry')}</button
                >
            </div>{/if}
    {/snippet}
    {#snippet detail()}
        {#if overlay === 'app-settings' || overlay === 'providers' || overlay === 'studio'}
            <WorkspaceFeatures
                mode={overlay === 'studio' ? 'studio' : 'providers'}
                client={appClient}
                {appState}
                {controller}
                onclose={close}
            />
        {:else if overlay}
            <WorkspaceSettings
                kind={overlay}
                {appState}
                {controller}
                onclose={close}
                oncreated={() => {
                    overlay = null;
                    page = 1;
                }}
                onproviders={() => (overlay = 'providers')}
                onadvanced={() => (overlay = 'studio')}
            />
        {/if}
    {/snippet}
    {#snippet chat(navigate, managementVisible, creatorVisible)}
        {#if character && conversation}
            <ChatPage
                {character}
                {conversation}
                {session}
                draft={session.draft}
                {managementVisible}
                {creatorVisible}
                onnavigate={navigate}
                ondraft={(value) => (session.draft = value)}
                onsend={() => void send()}
                onsettings={(trigger) => open('room-settings', trigger)}
                oninteract={() => {}}
            >
                {#snippet renderMessage(message)}
                    <PortableMessage
                        text={message.source && message.status !== 'pending'
                            ? runtime.displayText(message.source)
                            : message.text}
                        client={appClient}
                        profile={runtime.profile}
                        enabled={runtime.displayApproved &&
                            runtime.canReadChat &&
                            message.role === 'assistant' &&
                            message.status !== 'pending'}
                        expandMacros={message.role === 'assistant'}
                        variables={runtime.variables}
                        backgroundMarkup={runtime.background}
                        lastCharacterMessage={runtime.lastCharacterMessage}
                        characterName={character?.name}
                        onAction={(action) => void runtime.handleAction(action)}
                    />
                {/snippet}
                {#snippet extras()}<WorkspaceChatExtras
                        {appState}
                        {controller}
                        client={appClient}
                        {refreshEpoch}
                        onnotice={(value) => (notice = value)}
                    />{/snippet}
            </ChatPage>
            {#if error || notice || appState.chat.reconcile_notice}<div
                    class="ui-live-error"
                    role="alert"
                >
                    {error || notice || appState.chat.reconcile_notice}
                </div>{/if}
            {#if appState.chat.reasoning_text && !appState.chat.streaming_text}<p
                    class="ui-live-status"
                    role="status"
                >
                    {$tr('workspace.reasoning')}
                </p>{/if}
        {:else}
            <div class="ui-live-empty">
                <p>{$tr('workspace.chooseChat')}</p>
                <button
                    class="ui-submit ui-pressable"
                    disabled={!character}
                    onclick={(event) => open('new-chat', event.currentTarget)}
                    >{$tr('workspace.newChat')}</button
                >
            </div>
        {/if}
    {/snippet}
    {#snippet creator(navigate)}<WorkspaceCreator
            {runtime}
            client={appClient}
            onback={() => navigate(1)}
        />{/snippet}
</WorkspaceFrame>
<div class="ui-sr" role="status" aria-live="polite">{appState.announcement}</div>
{#if appState.import_flow.phase !== 'idle'}<ImportReviewDialog
        state={appState}
        {controller}
        onCommitted={(result) => {
            if (result.kind === 'character') {
                page = 0;
                overlay = null;
                void controller.selectCharacter(result.character);
            }
        }}
    />{/if}
