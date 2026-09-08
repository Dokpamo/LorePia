<script lang="ts">
    import { onMount, tick, untrack } from 'svelte';
    import { get } from 'svelte/store';
    import { ArrowLeft, Image as ImageIcon, Plus } from '@lucide/svelte';
    import {
        LorepiaAppController,
        INITIAL_APP_STATE,
        type LorepiaAppState,
    } from '../app-controller';
    import type { ImportCommitResultDto, LorepiaClient } from '../../lib/ipc/contracts';
    import { createLiveLorepiaClient } from '../../lib/ipc/client';
    import { t, tr } from '../../lib/i18n';
    import { themePreference } from '../../lib/theme';
    import { chatTextSize } from '../../lib/display';
    import { PortableRuntimeLifecycle } from '../../features/chat/portable-runtime-lifecycle.svelte';
    import PortableMessage from '../../features/chat/PortableMessage.svelte';
    import ImportReviewDialog from './WorkspaceImportReview.svelte';
    import WorkspaceFrame from '../../ui/workspace/WorkspaceFrame.svelte';
    import CharacterPage from '../../ui/workspace/CharacterPage.svelte';
    import CharacterRail from '../../ui/workspace/CharacterRail.svelte';
    import ChatPage from '../../ui/workspace/ChatPage.svelte';
    import IconButton from '../../ui/workspace/IconButton.svelte';
    import WorkspaceSettings from './WorkspaceSettings.svelte';
    import WorkspaceFeatures from './WorkspaceFeatures.svelte';
    import WorkspaceCreator from './WorkspaceCreator.svelte';
    import WorkspaceChatExtras from './WorkspaceChatExtras.svelte';
    import type { SampleMessage, Overlay, Page } from '../../ui/workspace/view-types';
    import { LiveChatSession } from './live-chat-session.svelte';
    import { characterViews, conversationView } from './workspace-projection';
    import { workspaceFeedback } from './workspace-feedback';

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
    let featureOverlay = $state<'providers' | 'studio' | null>(null);
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
        onNotice: (value: string) => (notice = value),
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
            await controller.selectCharacter(state.library.characters[0], true);
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
            void controller.selectCharacter(item, true);
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
        featureOverlay = null;
        overlay = kind;
    }
    function close() {
        featureOverlay = null;
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
    textScale={$chatTextSize === 'large' ? 1.1 : 1}
    conversationMode={conversation?.mode ?? 'chat'}
    onback={close}
>
    {#snippet management(navigate: (page: Page) => void)}
        {#if character}
            <CharacterPage
                {characters}
                {character}
                client={appClient}
                conversationId={conversation?.id ?? ''}
                oncharacter={selectCharacter}
                onconversation={(id: string) => selectConversation(id, navigate)}
                onaction={open}
            />
            {#if appState.conversations.phase === 'loading'}<p class="ui-live-status" role="status">
                    {$tr('workspace.loading')}
                </p>{/if}
        {:else}
            <div class="ui-left-layout">
                <CharacterRail
                    {characters}
                    client={appClient}
                    selected=""
                    oncharacter={selectCharacter}
                    onaction={open}
                    onbusy={() => undefined}
                />
                <div class="ui-left-content">
                    <div
                        class="ui-character-banner"
                        role="img"
                        aria-label={$tr('uiPreview.cardImage')}
                    >
                        <ImageIcon aria-hidden="true" />
                    </div>
                    <div class="ui-overlay-body">
                        <p>
                            {appState.bootstrap.phase === 'loading' ||
                            appState.library.phase === 'loading'
                                ? $tr('workspace.loading')
                                : $tr('workspace.emptyLibrary')}
                        </p>
                        <button
                            class="ui-submit ui-pressable"
                            disabled={appState.bootstrap.phase !== 'ready'}
                            onclick={() => void controller.beginImport()}
                            ><span class="ui-press-visual"
                                ><Plus />{$tr('workspace.importCard')}</span
                            ></button
                        >
                    </div>
                </div>
            </div>
        {/if}
        {#if error && page === 0 && !overlay}<div class="ui-live-error" role="alert">
                <p>{workspaceFeedback(error)}</p>
                <button class="ui-result-action" onclick={() => void start()}
                    >{$tr('workspace.retry')}</button
                >
            </div>{/if}
    {/snippet}
    {#snippet detail()}
        {#if overlay === 'app-settings' || overlay === 'providers' || overlay === 'studio'}
            <WorkspaceFeatures
                mode={overlay}
                client={appClient}
                {appState}
                {controller}
                onclose={close}
            />
        {:else if overlay}
            <WorkspaceSettings
                kind={overlay}
                client={appClient}
                {appState}
                {controller}
                covered={featureOverlay !== null}
                onclose={close}
                oncreated={() => {
                    overlay = null;
                    page = 1;
                }}
                onproviders={() => (featureOverlay = 'providers')}
                onadvanced={() => (featureOverlay = 'studio')}
            />
        {/if}
        {#if featureOverlay}
            <WorkspaceFeatures
                mode={featureOverlay}
                client={appClient}
                {appState}
                {controller}
                onclose={() => (featureOverlay = null)}
            />
        {/if}
    {/snippet}
    {#snippet chat(
        navigate: (page: Page) => void,
        managementVisible: boolean,
        creatorVisible: boolean,
    )}
        {#if character && conversation}
            <ChatPage
                {character}
                {conversation}
                {session}
                externalNotice={workspaceFeedback(appState.announcement)}
                draft={session.draft}
                {managementVisible}
                {creatorVisible}
                onnavigate={navigate}
                ondraft={(value: string) => (session.draft = value)}
                onsend={() => void send()}
                onsettings={(trigger: HTMLButtonElement) => open('room-settings', trigger)}
                oninteract={() => undefined}
            >
                {#snippet renderMessage(message: SampleMessage, messageIndex: number)}
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
                        messageIndex={runtime.canReadChat ? messageIndex : undefined}
                        lastMessageId={runtime.canReadChat
                            ? (conversation?.messages.length ?? 1) - 1
                            : undefined}
                        onAction={(action: string) => void runtime.handleAction(action)}
                    />
                {/snippet}
                {#snippet extras()}<WorkspaceChatExtras
                        {appState}
                        {controller}
                        client={appClient}
                        {refreshEpoch}
                        onnotice={(value: string) => (notice = value)}
                    />{/snippet}
            </ChatPage>
            {#if error ?? (notice !== '' ? notice : appState.chat.reconcile_notice)}<div
                    class="ui-live-error"
                    role="alert"
                >
                    {workspaceFeedback(
                        error ?? (notice !== '' ? notice : appState.chat.reconcile_notice) ?? '',
                    )}
                </div>{/if}
            {#if appState.chat.reasoning_text && !appState.chat.streaming_text}<p
                    class="ui-live-status"
                    role="status"
                >
                    {$tr('workspace.reasoning')}
                </p>{/if}
        {:else}
            <header class="ui-page-header" class:ui-navigation-header={!managementVisible}>
                {#if !managementVisible}
                    <IconButton label={$tr('uiPreview.openManagement')} onclick={() => navigate(0)}
                        ><ArrowLeft /></IconButton
                    >
                {/if}
                <div class="ui-title-group">
                    <strong>{character?.name ?? $tr('uiPreview.chat')}</strong>
                </div>
            </header>
            <div class="ui-messages">
                <p class="ui-chat-empty">{$tr('workspace.chooseChat')}</p>
                <button
                    class="ui-submit ui-pressable"
                    disabled={!character}
                    onclick={(event) => open('new-chat', event.currentTarget)}
                    ><span class="ui-press-visual">{$tr('workspace.newChat')}</span></button
                >
            </div>
        {/if}
    {/snippet}
    {#snippet creator(navigate: (page: Page) => void)}<WorkspaceCreator
            {runtime}
            client={appClient}
            onback={() => navigate(1)}
        />{/snippet}
</WorkspaceFrame>
<div class="sr-only" role="status" aria-live="polite">{appState.announcement}</div>
{#if appState.import_flow.phase !== 'idle'}<ImportReviewDialog
        state={appState}
        {controller}
        onCommitted={(result: ImportCommitResultDto) => {
            if (result.kind === 'character') {
                page = 0;
                featureOverlay = null;
                overlay = null;
                void controller.selectCharacter(result.character, true);
            }
        }}
    />{/if}
