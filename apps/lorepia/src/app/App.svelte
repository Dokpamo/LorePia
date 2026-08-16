<script lang="ts">
    import { onMount, untrack } from 'svelte';

    import {
        INITIAL_APP_STATE,
        LorepiaAppController,
        type LorepiaAppState,
    } from './app-controller';
    import ChatPane from '../features/chat/ChatPane.svelte';
    import ConversationPane from '../features/conversations/ConversationPane.svelte';
    import ImportReviewDialog from '../features/import/ImportReviewDialog.svelte';
    import LibraryPane from '../features/library/LibraryPane.svelte';
    import {
        INITIAL_ORCHESTRATION_STATE,
        OrchestrationController,
        type OrchestrationState,
    } from '../features/orchestration/orchestration-controller';
    import {
        ContentPackageController,
        INITIAL_CONTENT_PACKAGE_STATE,
        type ContentPackageState,
    } from '../features/orchestration/content-package-controller';
    import ProviderSettings from '../features/providers/ProviderSettings.svelte';
    import {
        INITIAL_PERSONA_STATE,
        PersonaController,
        type PersonaState,
    } from '../features/personas/persona-controller';
    import type { PersonaClientApi } from '../features/personas/persona-contracts';
    import { createLiveLorepiaClient } from '../lib/ipc/client';
    import type { LorepiaClient, MemoryRecordSourceNavigationDto } from '../lib/ipc/contracts';

    type MobileView = 'library' | 'conversations' | 'chat' | 'providers';

    interface Props {
        client?: LorepiaClient;
    }

    let { client }: Props = $props();
    const appClient = untrack(() => client ?? createLiveLorepiaClient());
    const controller = untrack(() => new LorepiaAppController(appClient));
    const orchestrationController = untrack(() => new OrchestrationController(appClient));
    const contentPackageController = untrack(() => new ContentPackageController(appClient));
    const personaController = untrack(
        () => new PersonaController(appClient as LorepiaClient & Partial<PersonaClientApi>),
    );
    let appState = $state<LorepiaAppState>(structuredClone(INITIAL_APP_STATE));
    let orchestrationState = $state<OrchestrationState>(
        structuredClone(INITIAL_ORCHESTRATION_STATE),
    );
    let contentPackageState = $state<ContentPackageState>(
        structuredClone(INITIAL_CONTENT_PACKAGE_STATE),
    );
    let personaState = $state<PersonaState>(structuredClone(INITIAL_PERSONA_STATE));
    let mobileView = $state<MobileView>('library');
    let orchestrationContextKey = '';
    let personaContextKey = '';
    let messageFocusRequest = $state<
        (MemoryRecordSourceNavigationDto & { request_id: number }) | null
    >(null);
    let nextMessageFocusRequestId = 0;

    async function navigateToMemorySource(source: MemoryRecordSourceNavigationDto): Promise<void> {
        const conversation = appState.conversations.items.find(
            (candidate) => candidate.id === source.conversation_id,
        );
        if (conversation && appState.selected_conversation?.id !== source.conversation_id) {
            await controller.selectConversation(conversation);
        }
        if (appState.conversation_state?.active_branch_id !== source.branch_id) {
            await controller.selectBranch(source.branch_id);
        }
        mobileView = 'chat';
        messageFocusRequest = {
            ...source,
            request_id: ++nextMessageFocusRequestId,
        };
    }

    $effect(() => {
        const conversationId = appState.selected_conversation?.id ?? null;
        const branchId = appState.conversation_state?.active_branch_id ?? null;
        const nextKey = conversationId && branchId ? `${conversationId}:${branchId}` : '';
        if (nextKey === orchestrationContextKey) return;
        orchestrationContextKey = nextKey;
        void orchestrationController.loadContext(conversationId, branchId);
    });

    $effect(() => {
        const conversationId = appState.selected_conversation?.id ?? null;
        const branchId = appState.conversation_state?.active_branch_id ?? null;
        const contextKey = conversationId && branchId ? `${conversationId}:${branchId}` : '';
        controller.setRoomGenerationTarget(
            conversationId,
            branchId,
            orchestrationState.phase === 'ready' && orchestrationState.context_key === contextKey
                ? orchestrationState.workspace.generation_target
                : undefined,
        );
    });

    $effect(() => {
        if (mobileView !== 'providers') return;
        const conversationId = appState.selected_conversation?.id ?? null;
        const nextKey = conversationId ?? 'no-conversation';
        if (nextKey === personaContextKey) return;
        personaContextKey = nextKey;
        void personaController.loadContext(conversationId);
    });

    onMount(() => {
        let previousBootstrapPhase = appState.bootstrap.phase;
        const unsubscribe = controller.state.subscribe((value) => {
            const bootstrapBecameReady =
                previousBootstrapPhase !== 'ready' && value.bootstrap.phase === 'ready';
            previousBootstrapPhase = value.bootstrap.phase;
            appState = value;
            if (bootstrapBecameReady) void contentPackageController.loadPendingImports();
        });
        const unsubscribeOrchestration = orchestrationController.state.subscribe((value) => {
            orchestrationState = value;
        });
        const unsubscribeContentPackage = contentPackageController.state.subscribe((value) => {
            contentPackageState = value;
        });
        const unsubscribePersona = personaController.state.subscribe((value) => {
            personaState = value;
        });
        void controller.start();
        return () => {
            unsubscribe();
            unsubscribeOrchestration();
            unsubscribeContentPackage();
            unsubscribePersona();
            controller.destroy();
            orchestrationController.destroy();
            contentPackageController.destroy();
            personaController.destroy();
        };
    });
</script>

<svelte:head>
    <meta
        name="description"
        content="로컬 데이터와 운영체제 자격증명 저장소를 사용하는 LorePia 캐릭터 채팅"
    />
</svelte:head>

<div class="app-shell">
    <header class="app-bar">
        <a class="brand" href="#main-content" aria-label="LorePia 본문으로 이동">
            <span class="brand-mark" aria-hidden="true">L</span>
            <span>LorePia</span>
        </a>
        <div class="app-bar-actions">
            <div
                class="health-chip"
                class:unhealthy={!appState.bootstrap.value?.health.database_open}
            >
                <span class="health-dot" aria-hidden="true"></span>
                {#if appState.bootstrap.phase === 'loading'}
                    Core 연결 중
                {:else if appState.bootstrap.phase === 'ready'}
                    로컬 Core
                {:else}
                    연결 확인 필요
                {/if}
            </div>
            <button
                class="settings-button"
                type="button"
                aria-pressed={mobileView === 'providers'}
                onclick={() => {
                    mobileView = mobileView === 'providers' ? 'library' : 'providers';
                    if (mobileView === 'providers') void controller.loadProviders();
                }}
            >
                {mobileView === 'providers' ? '대화로' : '설정'}
            </button>
        </div>
    </header>

    {#if appState.bootstrap.phase === 'error'}
        <main id="main-content" class="fatal-screen">
            <span class="large-mark" aria-hidden="true">!</span>
            <h1>앱을 시작하지 못했습니다.</h1>
            <p>{appState.bootstrap.error}</p>
            <button class="primary" type="button" onclick={() => void controller.start()}>
                다시 시도
            </button>
        </main>
    {:else}
        <main
            id="main-content"
            class="workspace"
            class:provider-mode={mobileView === 'providers'}
            data-mobile-view={mobileView}
        >
            <div class="workspace-pane library-slot">
                <LibraryPane
                    state={appState}
                    {controller}
                    client={appClient}
                    onOpenConversations={() => (mobileView = 'conversations')}
                />
            </div>
            <div class="workspace-pane conversations-slot">
                <ConversationPane
                    state={appState}
                    {controller}
                    onOpenChat={() => (mobileView = 'chat')}
                />
            </div>
            <div class="workspace-pane chat-slot">
                <ChatPane
                    {appState}
                    {controller}
                    client={appClient}
                    {orchestrationState}
                    {orchestrationController}
                    {messageFocusRequest}
                    onOpenOrchestrationStudio={() => {
                        mobileView = 'providers';
                        void controller.loadProviders();
                    }}
                />
            </div>
            <div class="workspace-pane providers-slot">
                <ProviderSettings
                    client={appClient}
                    {appState}
                    {controller}
                    {orchestrationState}
                    {orchestrationController}
                    {contentPackageState}
                    {contentPackageController}
                    {personaState}
                    {personaController}
                    onNavigateToMemorySource={(source: MemoryRecordSourceNavigationDto) =>
                        void navigateToMemorySource(source)}
                />
            </div>
        </main>
    {/if}

    <nav class="mobile-tabs" aria-label="주요 화면">
        <button
            type="button"
            class:active={mobileView === 'library'}
            aria-current={mobileView === 'library' ? 'page' : undefined}
            onclick={() => (mobileView = 'library')}
        >
            <span aria-hidden="true">⌂</span>
            서재
        </button>
        <button
            type="button"
            class:active={mobileView === 'conversations'}
            aria-current={mobileView === 'conversations' ? 'page' : undefined}
            onclick={() => (mobileView = 'conversations')}
        >
            <span aria-hidden="true">☰</span>
            대화
        </button>
        <button
            type="button"
            class:active={mobileView === 'chat'}
            aria-current={mobileView === 'chat' ? 'page' : undefined}
            onclick={() => (mobileView = 'chat')}
        >
            <span aria-hidden="true">✦</span>
            채팅
        </button>
        <button
            type="button"
            class:active={mobileView === 'providers'}
            aria-current={mobileView === 'providers' ? 'page' : undefined}
            onclick={() => {
                mobileView = 'providers';
                void controller.loadProviders();
            }}
        >
            <span aria-hidden="true">⚙</span>
            설정
        </button>
    </nav>

    <div class="sr-only" role="status" aria-live="polite" aria-atomic="true">
        {appState.announcement}
        {orchestrationState.announcement}
        {contentPackageState.announcement}
        {personaState.announcement}
    </div>

    {#if appState.import_flow.phase !== 'idle'}
        <ImportReviewDialog state={appState} {controller} />
    {/if}
</div>
