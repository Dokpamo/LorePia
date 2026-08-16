<script lang="ts">
    import { tr } from '../lib/i18n';
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

    /* The main region shows either the transcript or settings; never both. */
    type MainView = 'chat' | 'settings';
    /* Characters and conversations are one hierarchy, disclosed one at a time. */
    type SidebarSection = 'characters' | 'conversations';

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
    let view = $state<MainView>('chat');
    let sidebarSection = $state<SidebarSection>('characters');
    /* Only meaningful below the sidebar breakpoint, where it slides over. */
    let sidebarOpen = $state(false);
    let orchestrationContextKey = '';
    let personaContextKey = '';
    let messageFocusRequest = $state<
        (MemoryRecordSourceNavigationDto & { request_id: number }) | null
    >(null);
    let nextMessageFocusRequestId = 0;

    function openSettings(): void {
        view = 'settings';
        sidebarOpen = false;
        void controller.loadProviders();
    }

    function toggleSettings(): void {
        if (view === 'settings') {
            view = 'chat';
            return;
        }
        openSettings();
    }

    function showChat(): void {
        view = 'chat';
        sidebarOpen = false;
    }

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
        showChat();
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
        if (view !== 'settings') return;
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
    <meta name="description" content={$tr('app.description')} />
</svelte:head>

<div class="app-shell" data-sidebar={sidebarOpen ? 'open' : 'closed'} data-view={view}>
    <aside class="sidebar" aria-label={$tr('app.nav.label')}>
        <div class="sidebar-head">
            <a class="brand" href="#main-content">
                <span class="brand-mark" aria-hidden="true">L</span>
                <span>LorePia</span>
                <span class="sr-only">— {$tr('app.brand.skip')}</span>
            </a>
            <button
                class="icon-button ghost compact sidebar-close"
                type="button"
                aria-label={$tr('app.nav.close')}
                onclick={() => (sidebarOpen = false)}
            >
                <span aria-hidden="true">×</span>
            </button>
        </div>

        <div class="sidebar-body">
            <section class="sidebar-section" class:open={sidebarSection === 'characters'}>
                <button
                    class="section-toggle"
                    type="button"
                    aria-expanded={sidebarSection === 'characters'}
                    onclick={() => (sidebarSection = 'characters')}
                >
                    <span class="chevron" aria-hidden="true">›</span>
                    <span class="section-name">{$tr('app.tab.library')}</span>
                    {#if appState.selected_character !== null}
                        <span class="section-value">{appState.selected_character.name}</span>
                    {/if}
                </button>
                <LibraryPane
                    state={appState}
                    {controller}
                    client={appClient}
                    onOpenConversations={() => (sidebarSection = 'conversations')}
                />
            </section>

            <section class="sidebar-section" class:open={sidebarSection === 'conversations'}>
                <button
                    class="section-toggle"
                    type="button"
                    aria-expanded={sidebarSection === 'conversations'}
                    disabled={appState.selected_character === null}
                    onclick={() => (sidebarSection = 'conversations')}
                >
                    <span class="chevron" aria-hidden="true">›</span>
                    <span class="section-name">{$tr('app.tab.conversations')}</span>
                </button>
                <ConversationPane state={appState} {controller} onOpenChat={showChat} />
            </section>
        </div>

        <div class="sidebar-foot">
            <div
                class="health-chip"
                class:unhealthy={!appState.bootstrap.value?.health.database_open}
            >
                <span class="health-dot" aria-hidden="true"></span>
                {#if appState.bootstrap.phase === 'loading'}
                    {$tr('app.core.connecting')}
                {:else if appState.bootstrap.phase === 'ready'}
                    {$tr('app.core.local')}
                {:else}
                    {$tr('app.core.checking')}
                {/if}
            </div>
            <button
                class="settings-button"
                type="button"
                aria-pressed={view === 'settings'}
                onclick={toggleSettings}
            >
                {view === 'settings' ? $tr('app.toggle.to_chat') : $tr('app.toggle.to_providers')}
            </button>
        </div>
    </aside>

    <button
        class="sidebar-scrim"
        type="button"
        tabindex="-1"
        aria-hidden="true"
        onclick={() => (sidebarOpen = false)}
    ></button>

    {#if appState.bootstrap.phase === 'error'}
        <main id="main-content" class="main">
            <div class="fatal-screen">
                <span class="large-mark" aria-hidden="true">!</span>
                <h1>{$tr('app.bootstrap.failed')}</h1>
                <p>{appState.bootstrap.error}</p>
                <button class="primary" type="button" onclick={() => void controller.start()}>
                    {$tr('app.bootstrap.retry')}
                </button>
            </div>
        </main>
    {:else}
        <main id="main-content" class="main">
            {#if view === 'settings'}
                <div class="settings-region">
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
            {:else}
                <ChatPane
                    {appState}
                    {controller}
                    client={appClient}
                    {orchestrationState}
                    {orchestrationController}
                    {messageFocusRequest}
                    onOpenSidebar={() => (sidebarOpen = true)}
                    onOpenOrchestrationStudio={openSettings}
                />
            {/if}
        </main>
    {/if}

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
