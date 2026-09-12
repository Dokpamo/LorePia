<script lang="ts">
    import { onMount, tick, untrack } from 'svelte';
    import { get } from 'svelte/store';
    import { ArrowLeft } from '@lucide/svelte';
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
    import {
        chatDisplayPreferences,
        createConversationDisplayModeResolver,
    } from '../../lib/chat-display';
    import { PortableRuntimeLifecycle } from '../../features/chat/portable-runtime-lifecycle.svelte';
    import PortableMessage from '../../features/chat/PortableMessage.svelte';
    import ImportReviewDialog from './WorkspaceImportReview.svelte';
    import ApplicationFrame from '../../ui/navigation/ApplicationFrame.svelte';
    import CharacterLibrary from '../../ui/navigation/CharacterLibrary.svelte';
    import CharacterOverview from '../../ui/navigation/CharacterOverview.svelte';
    import type { CharacterProfilePresentation } from '../../ui/navigation/character-profile-types';
    import ConversationLibrary from '../../ui/navigation/ConversationLibrary.svelte';
    import CreationHub from '../../ui/navigation/CreationHub.svelte';
    import type { RootTab } from '../../ui/navigation/navigation-types';
    import { WorkspaceNavigationController } from './workspace-navigation-controller';
    import { formatMessageDay } from '../../features/chat/chat-scroll.svelte';
    import ChatPage from '../../ui/workspace/ChatPage.svelte';
    import IconButton from '../../ui/workspace/IconButton.svelte';
    import WorkspaceSettings from './WorkspaceSettings.svelte';
    import WorkspaceFeatures from './DeferredWorkspaceFeatures.svelte';
    import WorkspaceCreator from './WorkspaceCreator.svelte';
    import CardRoomSurface from './runtime/CardRoomSurface.svelte';
    import CardRuntimeSettings from './runtime/CardRuntimeSettings.svelte';
    import WorkspaceChatExtras from './WorkspaceChatExtras.svelte';
    import type { SampleMessage, Overlay, Page } from '../../ui/workspace/view-types';
    import { MessageHistoryController } from '../controllers/message-history-controller';
    import { LiveChatSession } from './live-chat-session.svelte';
    import { characterViews, ConversationProjection } from './workspace-projection';
    import { workspaceFeedback } from './workspace-feedback';

    let {
        client,
        characterPresentations = {},
    }: {
        client?: LorepiaClient;
        characterPresentations?: Record<string, CharacterProfilePresentation>;
    } = $props();
    const appClient = untrack(() => client ?? createLiveLorepiaClient());
    const controller = new LorepiaAppController(appClient);
    let appState = $state.raw<LorepiaAppState>(structuredClone(INITIAL_APP_STATE));
    let refreshEpoch = $state(0);
    const messageHistory = new MessageHistoryController(appClient);
    const history = messageHistory.state;
    $effect(() => {
        const state = appState;
        untrack(() => messageHistory.sync(state));
    });
    const session = new LiveChatSession(
        controller,
        () => appState,
        {
            onMutation: () => (refreshEpoch += 1),
            onRemoved: async (scope) => {
                const branch = appState.branches.find(
                    (item) => item.id === appState.conversation_state?.active_branch_id,
                );
                if (branch) {
                    const branchId = branch.id;
                    const head = branch.head_message_id;
                    await runtime.forgetDeletedMessages(
                        scope,
                        branchId,
                        head,
                        appClient,
                        () =>
                            session.scope === scope &&
                            appState.branches.some(
                                (current) =>
                                    current.id === branchId && current.head_message_id === head,
                            ),
                    );
                } else runtime.resetScope();
            },
        },
        messageHistory,
    );
    let page = $state<Page>(0);
    let rootTab = $state<RootTab>('home');
    let overview = $state(false);
    let rootDetails = $state({ home: false, chats: false, create: false, settings: false });
    let featureSection = $state<'plugins' | undefined>();
    const navigation = new WorkspaceNavigationController(appClient, controller, () => appState);
    const catalog = navigation.state;
    const catalogItems = $derived($catalog.items);
    let overlay = $state<Overlay | 'providers' | 'studio' | null>(null);
    let featureOverlay = $state<'providers' | 'studio' | null>(null);
    let returnPage: Page = 0;
    let opener: HTMLElement | null = null;
    let notice = $state('');
    let mounted = true;
    const runtime = new PortableRuntimeLifecycle({
        currentMessages: () => runtimeMessages,
        currentMessageWindow: () => runtimeMessageWindow,
        currentLastAssistantMessage: () => runtimeLastAssistantMessage,
        displayMessages: () => runtimeDisplayMessages,
        providerWorkspace: () => appState.providers.workspace,
        primarySelection: () => controller.runtimeGenerationSelection(),
        sendMessage: (...args) => controller.sendMessage(...args),
        onNotice: (value: string) => (notice = value),
    });
    const subpage = $derived(
        runtime.profileLoading ||
            runtime.profileError !== null ||
            (!!runtime.profile &&
                (runtime.requiresRuntimeWorker ||
                    !!runtime.profile.background_markup.trim() ||
                    runtime.profile.display_transforms.length > 0 ||
                    runtime.profile.output_transforms.length > 0)),
    );
    const library = $derived(appState.library);
    const selectedCharacter = $derived(appState.selected_character);
    const characterConversations = $derived(appState.conversations);
    const characters = $derived(
        characterViews(
            {
                library,
                selected_character: selectedCharacter,
                conversations: characterConversations,
            },
            subpage,
        ),
    );
    const character = $derived(
        characters.find((item) => item.id === appState.selected_character?.id),
    );
    const characterNames = $derived(new Map(characters.map((item) => [item.id, item.name])));
    const allConversations = $derived(
        catalogItems.map((item) => ({
            id: item.id,
            characterId: item.character_id,
            characterName: characterNames.get(item.character_id) ?? '',
            title: item.title,
            date: formatMessageDay(item.updated_at),
            updatedAt: item.updated_at,
        })),
    );
    const projection = new ConversationProjection();
    const resolveDisplayMode = createConversationDisplayModeResolver();
    const displayConversationId = $derived(appState.selected_conversation?.id);
    const displayNativeMode = $derived(appState.conversation_state?.selected_mode ?? 'chat');
    const resolvedDisplayMode = $derived(
        resolveDisplayMode(displayConversationId, displayNativeMode, $chatDisplayPreferences),
    );
    const conversation = $derived(
        projection.project(
            {
                ...appState,
                messages: {
                    ...appState.messages,
                    items: $history.items,
                    start_index: $history.paged ? $history.start_index : undefined,
                    total_messages: $history.paged ? $history.total_messages : undefined,
                },
                chat: $history.has_newer
                    ? { ...appState.chat, live_assistant_message_id: null }
                    : appState.chat,
            },
            resolvedDisplayMode,
        ),
    );
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
            navigation.destroy();
            messageHistory.dispose();
            controller.destroy();
        };
    });
    function start() {
        return controller.start();
    }
    $effect(() => {
        const first = appState.library.characters[0];
        if (appState.library.phase === 'ready' && first)
            untrack(() => {
                if (mounted && !get(controller.state).selected_character)
                    void controller.selectCharacter(first, true);
            });
    });
    // Root store publications include streaming and unrelated workspace updates.
    // Track stable scope values so they cannot reload the large profile or revoke
    // the current runtime grant on every publication.
    const runtimeCharacterId = $derived(appState.selected_character?.id ?? null);
    const runtimeReady = $derived(
        appState.messages.phase === 'ready' ||
            (appState.messages.total_messages !== undefined &&
                appState.messages.items.length === Math.min(128, appState.messages.total_messages)),
    );
    const runtimeConversationId = $derived(appState.selected_conversation?.id ?? null);
    const runtimeBranchId = $derived(appState.conversation_state?.active_branch_id ?? null);
    const runtimeCharacterName = $derived(appState.selected_character?.name ?? '');
    const runtimeCharacterDescription = $derived(appState.selected_character?.description ?? '');
    const runtimeScope = $derived(
        runtimeConversationId && runtimeBranchId
            ? {
                  characterId: runtimeCharacterId,
                  conversationId: runtimeConversationId,
                  branchId: runtimeBranchId,
              }
            : null,
    );
    $effect(() =>
        runtime.loadProfile(
            appClient,
            runtimeScope?.characterId ?? null,
            runtimeScope?.conversationId ?? null,
            runtimeScope?.branchId ?? null,
        ),
    );
    $effect(() =>
        runtime.recreate({
            client: appClient,
            character:
                runtimeCharacterId === null
                    ? null
                    : {
                          name: runtimeCharacterName,
                          description: runtimeCharacterDescription,
                      },
            conversationId: runtimeReady ? runtimeConversationId : null,
            branchId: runtimeReady ? runtimeBranchId : null,
        }),
    );
    const runtimeMessages = $derived(appState.messages.items);
    const runtimeLastAssistantMessage = $derived(appState.messages.last_assistant_message);
    const runtimeWindowStart = $derived(appState.messages.start_index);
    const runtimeWindowTotal = $derived(appState.messages.total_messages);
    const runtimeWindowHead = $derived(appState.messages.head_message_id);
    const runtimeMessageWindow = $derived(
        runtimeWindowStart !== undefined && runtimeWindowTotal !== undefined
            ? {
                  start_index: runtimeWindowStart,
                  total_messages: runtimeWindowTotal,
                  head_message_id: runtimeWindowHead ?? null,
              }
            : undefined,
    );
    const runtimeLiveMessage = $derived(appState.chat.live_assistant_message_id);
    const runtimeDisplayMessages = $derived(
        runtimeMessages.filter((item) => item.id !== runtimeLiveMessage),
    );
    const runtimeGeneration = $derived(appState.chat.active_generation_id);
    const runtimeStreaming = $derived(
        appState.chat.live_assistant_message_id !== null ||
            !!appState.chat.streaming_text ||
            !!appState.chat.reasoning_text,
    );
    $effect(() => {
        if (runtimeReady)
            runtime.syncMessages({
                messages: runtimeMessages,
                activeGenerationId: runtimeGeneration,
                hasStreamingPresentation: runtimeStreaming,
            });
    });
    $effect(() => {
        if (!subpage && page === 2) page = 1;
    });
    async function selectCharacter(id: string) {
        if (await navigation.selectCharacter(id)) {
            overview = true;
            notice = '';
        }
    }
    async function selectConversation(id: string) {
        notice = '';
        if (await navigation.selectConversation(id)) {
            overview = false;
            page = 1;
        }
    }
    async function newConversation(id: string, trigger: HTMLButtonElement) {
        if (await navigation.selectCharacter(id)) open('new-chat', trigger);
    }
    $effect(() => {
        void rootTab;
        untrack(() => navigation.cancelNavigation());
    });
    $effect(() => {
        if (appState.bootstrap.phase === 'ready') {
            void refreshEpoch;
            void appState.library.characters;
            untrack(() => void navigation.load());
        }
    });
    function open(kind: Overlay, trigger: HTMLButtonElement) {
        if (kind === 'add-character') {
            void controller.beginImport();
            return;
        }
        opener = trigger;
        returnPage = trigger.closest('.ui-chat') ? 1 : page;
        featureOverlay = null;
        overlay = kind;
    }
    function close() {
        featureOverlay = null;
        featureSection = undefined;
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

<ApplicationFrame
    bind:rootTab
    nested={rootDetails[rootTab]}
    bind:page
    overlay={overview || overlay !== null || appState.import_flow.phase !== 'idle'}
    appearance={$themePreference}
    textScale={$chatTextSize === 'large' ? 1.1 : 1}
    conversationMode={conversation?.mode ?? 'chat'}
    rightPageAvailable={subpage && conversation !== null}
>
    {#snippet home()}
        <CharacterLibrary
            {characters}
            client={appClient}
            ondetail={(active: boolean) => (rootDetails.home = active)}
            ready={appState.bootstrap.phase === 'ready'}
            loaded={appState.library.phase === 'ready'}
            loading={appState.library.phase === 'loading' || appState.bootstrap.phase === 'loading'}
            onselect={(id: string) => void selectCharacter(id)}
            onadd={() => void controller.beginImport()}
        />
        {#if error && !overlay}<div class="ui-live-error" role="alert">
                <p>{workspaceFeedback(error)}</p>
                <button class="ui-result-action" onclick={() => void start()}
                    >{$tr('workspace.retry')}</button
                >
            </div>{/if}
    {/snippet}
    {#snippet chats()}
        <ConversationLibrary
            conversations={allConversations}
            {characters}
            ondetail={(active: boolean) => (rootDetails.chats = active)}
            loading={$catalog.loading}
            error={$catalog.error}
            onopen={(id: string) => void selectConversation(id)}
            onnew={(id: string, trigger: HTMLButtonElement) => void newConversation(id, trigger)}
            onretry={() => void navigation.load()}
        />
    {/snippet}
    {#snippet create()}
        <CreationHub
            client={appClient}
            ready={appState.bootstrap.phase === 'ready'}
            onimport={() => void controller.beginImport()}
            onprompt={() => (overlay = 'studio')}
            ondetail={(active: boolean) => (rootDetails.create = active)}
        />
    {/snippet}
    {#snippet settings()}
        <WorkspaceFeatures
            root
            active={rootTab === 'settings' && page === 0 && !overview && overlay === null}
            mode="app-settings"
            client={appClient}
            {appState}
            {controller}
            onclose={() => undefined}
            ondetail={(active: boolean) => (rootDetails.settings = active)}
        />
    {/snippet}
    {#snippet detail()}
        {#if overview && character}<CharacterOverview
                {character}
                presentation={characterPresentations[character.id]}
                greetings={appState.greeting_catalog.value?.character_id === character.id
                    ? appState.greeting_catalog.value.greetings
                    : []}
                selectedGreetingId={appState.greeting_catalog.selected_greeting_id}
                greetingRevisionId={appState.greeting_catalog.value?.character_id === character.id
                    ? appState.greeting_catalog.value.character_content_revision_id
                    : undefined}
                greetingsLoading={appState.greeting_catalog.phase === 'loading'}
                onselectGreeting={(id: string) => {
                    controller.selectGreeting(id);
                }}
                client={appClient}
                covered={overlay !== null}
                onclose={() => (overview = false)}
                onaction={open}
                conversations={allConversations}
                conversationsLoading={$catalog.loading}
                conversationsError={$catalog.error}
                onretryConversations={() => void navigation.load()}
                onchat={(id: string) => void selectConversation(id)}
                runtimeTarget={appState.selected_conversation?.character_id === character.id &&
                appState.conversation_state?.conversation_id === appState.selected_conversation.id
                    ? {
                          conversation_id: appState.selected_conversation.id,
                          branch_id: appState.conversation_state.active_branch_id,
                      }
                    : undefined}
            />{/if}
        {#if overlay === 'app-settings' || overlay === 'providers' || overlay === 'studio'}
            <WorkspaceFeatures
                mode={overlay}
                initialSection={featureSection}
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
                    overview = false;
                    page = 1;
                    void navigation.load();
                }}
                onproviders={() => (featureOverlay = 'providers')}
                onadvanced={() => (featureOverlay = 'studio')}
            >
                {#snippet cardSettings()}{#if subpage}<CardRuntimeSettings
                            {runtime}
                        />{/if}{/snippet}
            </WorkspaceSettings>
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
                client={appClient}
                personaName={runtime.personaName}
                {character}
                {conversation}
                {session}
                history={$history}
                loading={appState.messages.phase === 'loading'}
                loadError={appState.messages.error}
                onretry={() => {
                    if (appState.selected_conversation)
                        void controller.selectConversation(appState.selected_conversation);
                }}
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
                            ? (conversation?.totalMessages ?? conversation?.messages.length ?? 1) -
                              1
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
                {#snippet cardSurface()}{#if page === 1}<CardRoomSurface
                            {runtime}
                            client={appClient}
                            floating
                        />{/if}{/snippet}
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
            active={page === 2}
            onback={() => navigate(1)}
            onsettings={(trigger: HTMLButtonElement) => open('room-settings', trigger)}
        />{/snippet}
</ApplicationFrame>
<div class="sr-only" role="status" aria-live="polite">{appState.announcement}</div>
{#if appState.import_flow.phase !== 'idle'}<ImportReviewDialog
        state={appState}
        {controller}
        onCommitted={(result: ImportCommitResultDto) => {
            if (result.kind === 'character') {
                page = 0;
                rootTab = 'home';
                overview = false;
                featureOverlay = null;
                overlay = null;
                void controller.selectCharacter(result.character, true);
            }
        }}
    />{/if}
