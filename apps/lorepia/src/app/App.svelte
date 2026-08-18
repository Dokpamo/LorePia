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
    import OrchestrationStudio from '../features/orchestration/OrchestrationStudio.svelte';
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
    import type { SettingsSection } from '../features/providers/settings-contracts';
    import type { StudioSection } from '../features/orchestration/studio-contracts';
    import {
        INITIAL_PERSONA_STATE,
        PersonaController,
        type PersonaState,
    } from '../features/personas/persona-controller';
    import type { PersonaClientApi } from '../features/personas/persona-contracts';
    import { createLiveLorepiaClient } from '../lib/ipc/client';
    import type { LorepiaClient, MemoryRecordSourceNavigationDto } from '../lib/ipc/contracts';

    /*
     * One app, two layouts, because the two screens run out of different room.
     *
     * A phone is short on space, so it divides in time: four destinations under
     * a bottom tab bar, one visible at a time. A desktop is short on nothing
     * but attention, so it divides in space: the character and conversation
     * hierarchy stands in a docked sidebar beside the transcript, and switching
     * conversations never takes the transcript off screen. Home is therefore a
     * destination only on the phone — on a desktop it is the sidebar, always
     * there. Studio and settings are full-region modes on both.
     */
    const DESKTOP_LAYOUT = '(min-width: 900px)';
    type MainView = 'home' | 'chat' | 'create' | 'settings';
    /* Characters and conversations are one hierarchy, disclosed one at a time. */
    type HomeSection = 'characters' | 'conversations';

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
    let view = $state<MainView>('home');
    let homeSection = $state<HomeSection>('characters');
    /* Settings and the studio are lists; opening an entry pushes a screen. */
    let settingsSection = $state<SettingsSection | null>(null);
    let studioSection = $state<StudioSection | null>(null);
    let isDesktop = $state(false);
    let orchestrationContextKey = '';
    let personaContextKey = '';
    let messageFocusRequest = $state<
        (MemoryRecordSourceNavigationDto & { request_id: number }) | null
    >(null);
    let nextMessageFocusRequestId = 0;

    function showHome(): void {
        view = 'home';
    }

    function showChat(): void {
        view = 'chat';
    }

    function openCreate(): void {
        view = 'create';
        studioSection = null;
    }

    function openSettings(): void {
        view = 'settings';
        settingsSection = null;
        void controller.loadProviders();
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
        const layout = window.matchMedia(DESKTOP_LAYOUT);
        const syncLayout = (): void => {
            /*
             * Read the query fresh rather than trusting the list captured
             * above: a window resized by the host can update the media state
             * without delivering a change event to a list made earlier, which
             * would strand the app in the layout it started in.
             */
            isDesktop = window.matchMedia(DESKTOP_LAYOUT).matches;
            /* The sidebar already is home, so it is never a destination here. */
            if (isDesktop && view === 'home') view = 'chat';
        };
        syncLayout();
        layout.addEventListener('change', syncLayout);
        window.addEventListener('resize', syncLayout);

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
            layout.removeEventListener('change', syncLayout);
            window.removeEventListener('resize', syncLayout);
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

{#snippet identity()}
    <h1 class="index-title">LorePia</h1>
    <div class="health-chip" class:unhealthy={!appState.bootstrap.value?.health.database_open}>
        <span class="health-dot" aria-hidden="true"></span>
        {#if appState.bootstrap.phase === 'loading'}
            {$tr('app.core.connecting')}
        {:else if appState.bootstrap.phase === 'ready'}
            {$tr('app.core.local')}
        {:else}
            {$tr('app.core.checking')}
        {/if}
    </div>
{/snippet}

{#snippet navigator()}
    <div class="navigator">
        <section class="home-section" class:open={homeSection === 'characters'}>
            <button
                class="section-toggle"
                type="button"
                aria-expanded={homeSection === 'characters'}
                onclick={() => (homeSection = 'characters')}
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
                onOpenConversations={() => (homeSection = 'conversations')}
            />
        </section>

        <section class="home-section" class:open={homeSection === 'conversations'}>
            <button
                class="section-toggle"
                type="button"
                aria-expanded={homeSection === 'conversations'}
                disabled={appState.selected_character === null}
                onclick={() => (homeSection = 'conversations')}
            >
                <span class="chevron" aria-hidden="true">›</span>
                <span class="section-name">{$tr('app.tab.conversations')}</span>
            </button>
            <ConversationPane state={appState} {controller} onOpenChat={showChat} />
        </section>
    </div>
{/snippet}

{#snippet createIcon()}
    <svg class="nav-icon" viewBox="0 0 24 24" aria-hidden="true">
        <path d="m12 3 2.2 5.3L19.5 10l-5.3 2.2L12 17.5 9.8 12.2 4.5 10l5.3-1.7z" />
        <path d="M18 16.5 18.8 19l2.2.8-2.2.9-.8 2.3-.9-2.3-2.1-.9 2.1-.8z" />
    </svg>
{/snippet}

{#snippet settingsIcon()}
    <svg class="nav-icon" viewBox="0 0 24 24" aria-hidden="true">
        <path d="M4 7h10M18 7h2M4 17h2M10 17h10" />
        <circle cx="16" cy="7" r="2.4" />
        <circle cx="8" cy="17" r="2.4" />
    </svg>
{/snippet}

<svelte:head>
    <meta name="description" content={$tr('app.description')} />
</svelte:head>

<div class="app-shell" data-view={view} data-layout={isDesktop ? 'desktop' : 'mobile'}>
    {#if isDesktop && appState.bootstrap.phase !== 'error'}
        <aside class="sidebar" aria-label={$tr('app.nav.label')}>
            <div class="sidebar-head">{@render identity()}</div>
            {@render navigator()}
            <div class="sidebar-foot">
                <button
                    class="nav-row"
                    type="button"
                    aria-current={view === 'create' ? 'page' : undefined}
                    onclick={openCreate}
                >
                    {@render createIcon()}
                    <span>{$tr('app.view.create')}</span>
                </button>
                <button
                    class="nav-row"
                    type="button"
                    aria-current={view === 'settings' ? 'page' : undefined}
                    onclick={openSettings}
                >
                    {@render settingsIcon()}
                    <span>{$tr('app.tab.providers')}</span>
                </button>
            </div>
        </aside>
    {/if}

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
            {#if view === 'home'}
                <section class="home-view" aria-label={$tr('app.tab.home')}>
                    <header class="index-header">{@render identity()}</header>
                    {@render navigator()}
                </section>
            {:else if view === 'chat'}
                <ChatPane
                    {appState}
                    {controller}
                    client={appClient}
                    {orchestrationState}
                    {orchestrationController}
                    {messageFocusRequest}
                    onOpenHome={showHome}
                    onOpenOrchestrationStudio={openCreate}
                />
            {:else if view === 'create'}
                {#if studioSection !== null}
                    <header class="sub-header">
                        <button
                            class="icon-button ghost back-button"
                            type="button"
                            aria-label={$tr('app.nav.back')}
                            onclick={() => (studioSection = null)}
                        >
                            <span aria-hidden="true">‹</span>
                        </button>
                        <h1>{$tr(`studio.section.${studioSection}.title`)}</h1>
                    </header>
                {/if}
                <div class="view-scroll">
                    <OrchestrationStudio
                        client={appClient}
                        {appState}
                        {orchestrationState}
                        controller={orchestrationController}
                        appController={controller}
                        {contentPackageState}
                        {contentPackageController}
                        onNavigateToMemorySource={(source: MemoryRecordSourceNavigationDto) =>
                            void navigateToMemorySource(source)}
                        section={studioSection}
                        onOpenSection={(next: StudioSection) => (studioSection = next)}
                    />
                </div>
            {:else}
                {#if settingsSection !== null}
                    <header class="sub-header">
                        <button
                            class="icon-button ghost back-button"
                            type="button"
                            aria-label={$tr('app.nav.back')}
                            onclick={() => (settingsSection = null)}
                        >
                            <span aria-hidden="true">‹</span>
                        </button>
                        <h1>{$tr(`settings.section.${settingsSection}.title`)}</h1>
                    </header>
                {/if}
                <ProviderSettings
                    {appState}
                    {controller}
                    {personaState}
                    {personaController}
                    section={settingsSection}
                    onOpenSection={(next: SettingsSection) => (settingsSection = next)}
                />
            {/if}
        </main>
    {/if}

    {#if !isDesktop && settingsSection === null && studioSection === null}
        <nav class="tab-bar" aria-label={$tr('app.nav.label')}>
            <button
                class="tab"
                type="button"
                aria-current={view === 'home' ? 'page' : undefined}
                onclick={showHome}
            >
                <svg class="nav-icon" viewBox="0 0 24 24" aria-hidden="true">
                    <path d="M4 10.5 12 4l8 6.5V20a1 1 0 0 1-1 1h-4v-6H9v6H5a1 1 0 0 1-1-1z" />
                </svg>
                <span class="tab-label">{$tr('app.tab.home')}</span>
            </button>
            <button
                class="tab"
                type="button"
                aria-current={view === 'chat' ? 'page' : undefined}
                onclick={showChat}
            >
                <svg class="nav-icon" viewBox="0 0 24 24" aria-hidden="true">
                    <path d="M4 5h16v11H9l-5 4z" />
                </svg>
                <span class="tab-label">{$tr('app.tab.chat')}</span>
            </button>
            <button
                class="tab"
                type="button"
                aria-current={view === 'create' ? 'page' : undefined}
                onclick={openCreate}
            >
                {@render createIcon()}
                <span class="tab-label">{$tr('app.tab.create')}</span>
            </button>
            <button
                class="tab"
                type="button"
                aria-current={view === 'settings' ? 'page' : undefined}
                onclick={openSettings}
            >
                {@render settingsIcon()}
                <span class="tab-label">{$tr('app.tab.providers')}</span>
            </button>
        </nav>
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
