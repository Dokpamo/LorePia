<script lang="ts">
    import {
        ArrowLeft,
        ChevronRight,
        CircleAlert,
        House,
        MessageSquare,
        SlidersHorizontal,
        Sparkles,
    } from '@lucide/svelte';
    import { tr } from '../lib/i18n';
    import { onMount, untrack } from 'svelte';
    import lorepiaLogoMark from '../assets/lorepia-logo-mark.png';

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
    import type {
        SettingsDetailPage,
        SettingsSection,
    } from '../features/providers/settings-contracts';
    import {
        studioBaseDetailTitleKey,
        studioDetailHasFixedActions,
        studioDetailParent,
        studioNestedDetailTitleKey,
        type StudioDetailPage,
        type StudioSection,
    } from '../features/orchestration/studio-contracts';
    import {
        INITIAL_PERSONA_STATE,
        PersonaController,
        type PersonaState,
    } from '../features/personas/persona-controller';
    import type { PersonaClientApi } from '../features/personas/persona-contracts';
    import { createLiveLorepiaClient } from '../lib/ipc/client';
    import type { LorepiaClient, MemoryRecordSourceNavigationDto } from '../lib/ipc/contracts';

    /*
     * Phones and wide handhelds divide destinations in time under a bottom
     * bar. Desktop windows have enough room to keep the character/conversation
     * hierarchy beside the active workspace instead.
     */
    const DESKTOP_LAYOUT = '(min-width: 900px)';
    const REDUCED_MOTION = '(prefers-reduced-motion: reduce)';
    const SIDEBAR_EXIT_SETTLE_MS = 260;
    const MOBILE_TOP_FADE_DISTANCE_PX = 48;
    type MainView = 'home' | 'chat' | 'create' | 'settings';
    type HomeSection = 'characters' | 'conversations';

    interface Props {
        client?: LorepiaClient;
        initialSelection?: {
            characterId: string;
            conversationId?: string;
        };
    }

    let { client, initialSelection }: Props = $props();
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
    let chatThreadOpen = $state(false);
    /* Settings and studio entries open as dedicated screens inside the handheld shell. */
    let settingsSection = $state<SettingsSection | null>(null);
    let settingsDetailPage = $state<SettingsDetailPage>(null);
    let settingsEditorMode = $state<string | null>(null);
    let settingsEditorTitle = $state('');
    let personaEditorMode = $state<'create' | 'edit' | null>(null);
    let studioSection = $state<StudioSection | null>(null);
    let studioDetailPage = $state<StudioDetailPage>(null);
    let isDesktop = $state(false);
    let sidebarContentMounted = $state(false);
    let studioScrollElement = $state<HTMLDivElement>();
    let pushedTitleElement = $state<HTMLHeadingElement>();
    let pushedTopFadeProgress = $state(0);
    let sidebarUnmountTimer: ReturnType<typeof setTimeout> | undefined;
    let orchestrationContextKey = '';
    let personaContextKey = '';
    let studioRouteKey = '';
    let pushedTitleRouteKey = '';
    let initialSelectionStarted = false;
    let messageFocusRequest = $state<
        (MemoryRecordSourceNavigationDto & { request_id: number }) | null
    >(null);
    let nextMessageFocusRequestId = 0;

    function sidebarMotionDuration(duration: number): number {
        return typeof window !== 'undefined' && window.matchMedia(REDUCED_MOTION).matches
            ? 0
            : duration;
    }

    function showHome(): void {
        view = 'home';
        chatThreadOpen = false;
    }

    function showChat(): void {
        view = 'chat';
        chatThreadOpen = false;
    }

    function openChatThread(): void {
        view = 'chat';
        chatThreadOpen = true;
    }

    function openCreate(): void {
        view = 'create';
        studioSection = null;
        studioDetailPage = null;
    }

    function resetStudioDetailScroll(): void {
        pushedTopFadeProgress = 0;
        const scroller = studioScrollElement;
        if (!scroller) return;
        scroller.scrollTop = 0;
    }

    function openStudioSection(next: StudioSection): void {
        studioSection = next;
        studioDetailPage = null;
        resetStudioDetailScroll();
    }

    function closeStudioSection(): void {
        if (studioDetailPage !== null) {
            studioDetailPage = studioDetailParent(studioDetailPage);
            resetStudioDetailScroll();
            return;
        }
        studioSection = null;
        studioDetailPage = null;
        resetStudioDetailScroll();
    }

    function handleStudioDetailScroll(event: Event): void {
        const scroller = event.currentTarget as HTMLDivElement;
        handlePushedDetailScroll(scroller.scrollTop);
    }

    function handlePushedDetailScroll(scrollTop: number): void {
        pushedTopFadeProgress = Math.min(1, Math.max(0, scrollTop / MOBILE_TOP_FADE_DISTANCE_PX));
    }

    $effect(() => {
        const nextKey = `${view}:${studioSection ?? ''}:${studioDetailPage ?? ''}`;
        if (nextKey === studioRouteKey) return;
        studioRouteKey = nextKey;
        queueMicrotask(resetStudioDetailScroll);
    });

    $effect(() => {
        const nextKey =
            view === 'create' && studioSection !== null
                ? `studio:${studioSection}:${studioDetailPage ?? ''}`
                : view === 'settings' && settingsSection !== null
                  ? `settings:${settingsSection}:${settingsDetailPage ?? ''}:${settingsEditorMode ?? ''}:${personaEditorMode ?? ''}`
                  : '';
        if (nextKey === '') {
            pushedTitleRouteKey = '';
            return;
        }
        if (nextKey === pushedTitleRouteKey) return;
        pushedTitleRouteKey = nextKey;
        pushedTopFadeProgress = 0;
        queueMicrotask(() => pushedTitleElement?.focus({ preventScroll: true }));
    });

    function openSettings(): void {
        view = 'settings';
        settingsSection = null;
        settingsDetailPage = null;
        settingsEditorMode = null;
        settingsEditorTitle = '';
        personaEditorMode = null;
        void controller.loadProviders();
    }

    function openSettingsSection(next: SettingsSection): void {
        personaEditorMode = null;
        settingsDetailPage = null;
        settingsEditorMode = null;
        settingsEditorTitle = '';
        settingsSection = next;
    }

    function closeSettingsSection(): void {
        if (settingsSection === 'persona' && personaEditorMode !== null) {
            personaEditorMode = null;
            return;
        }
        if (settingsEditorMode !== null) {
            settingsEditorMode =
                settingsEditorMode === 'override-create' ||
                settingsEditorMode === 'override-edit' ||
                settingsEditorMode === 'override-readonly'
                    ? 'overrides'
                    : null;
            settingsEditorTitle = '';
            return;
        }
        if (settingsDetailPage !== null) {
            settingsDetailPage = null;
            settingsEditorTitle = '';
            return;
        }
        personaEditorMode = null;
        settingsDetailPage = null;
        settingsEditorMode = null;
        settingsEditorTitle = '';
        settingsSection = null;
    }

    function settingsDetailTitle(): string {
        if (settingsSection === 'persona' && personaEditorMode !== null) {
            return $tr(
                personaEditorMode === 'create' ? 'persona.editor.new' : 'persona.editor.edit',
            );
        }

        if (settingsDetailPage !== null) {
            if (settingsSection === 'target' && settingsDetailPage === 'preview') {
                return $tr('settings.page.target.preview');
            }
            if (settingsSection === 'connections') return $tr('settings.page.connection.detail');
            if (settingsSection === 'templates') {
                return settingsEditorTitle || $tr('settings.page.template.detail');
            }
            if (settingsSection === 'discovery') {
                if (settingsEditorMode === 'create') {
                    return settingsDetailPage === 'provider-discovery'
                        ? $tr('settings.page.discovery.create')
                        : $tr('settings.page.discovery.sync_create');
                }
                if (settingsEditorMode?.startsWith('session:')) {
                    return settingsEditorTitle || $tr('settings.page.discovery.session');
                }
                if (settingsEditorMode?.startsWith('job:')) {
                    return settingsEditorTitle || $tr('settings.page.discovery.sync_job');
                }
                return settingsDetailPage === 'provider-discovery'
                    ? $tr('settings.page.discovery.provider')
                    : $tr('settings.page.discovery.sync');
            }
            if (settingsSection === 'catalog') {
                if (settingsDetailPage === 'status') return $tr('settings.page.catalog.status');
                if (settingsDetailPage === 'import-review') {
                    return $tr('settings.page.catalog.import');
                }
                if (settingsDetailPage === 'rollback-review') {
                    return $tr('settings.page.catalog.rollback');
                }
                if (settingsDetailPage === 'diff') return $tr('settings.page.catalog.diff');
                if (settingsDetailPage.startsWith('revision:')) {
                    return $tr('settings.page.catalog.revision', {
                        revision: settingsDetailPage.slice('revision:'.length),
                    });
                }
            }
            if (settingsSection === 'advanced') {
                if (settingsEditorTitle !== '') return settingsEditorTitle;
                if (settingsEditorMode !== null) {
                    if (settingsEditorMode === 'create') {
                        return $tr('settings.page.advanced.create');
                    }
                    if (settingsEditorMode === 'edit') return $tr('settings.page.advanced.edit');
                    if (settingsEditorMode === 'effective') {
                        return $tr('settings.page.advanced.effective');
                    }
                    if (settingsEditorMode === 'overrides') {
                        return $tr('settings.page.advanced.overrides');
                    }
                    if (settingsEditorMode === 'override-create') {
                        return $tr('settings.page.advanced.override_create');
                    }
                    if (settingsEditorMode === 'override-edit') {
                        return $tr('settings.page.advanced.override_edit');
                    }
                    if (settingsEditorMode === 'override-readonly') {
                        return $tr('settings.page.advanced.override_readonly');
                    }
                    if (settingsEditorMode === 'observations') {
                        return $tr('settings.page.advanced.observations');
                    }
                    if (settingsEditorMode === 'parameters') {
                        return $tr('settings.page.advanced.parameters');
                    }
                }
                const titles: Record<string, string> = {
                    connections: $tr('settings.page.advanced.connections'),
                    routes: $tr('settings.page.advanced.routes'),
                    presets: $tr('settings.page.advanced.presets'),
                    capabilities: $tr('settings.page.advanced.capabilities'),
                };
                return titles[settingsDetailPage] ?? $tr('settings.section.advanced.title');
            }
        }

        return settingsSection === null ? '' : $tr(`settings.section.${settingsSection}.title`);
    }

    function studioDetailTitle(): string {
        if (studioDetailPage !== null) {
            const nestedTitleKey = studioNestedDetailTitleKey(studioDetailPage);
            if (nestedTitleKey !== null) return $tr(nestedTitleKey);

            const titleKey = studioBaseDetailTitleKey(studioDetailPage);
            if (titleKey !== null) {
                if (studioDetailPage === 'transforms' && studioSection !== 'memory') {
                    return $tr('studio.page.transforms.display');
                }
                return $tr(titleKey);
            }
        }
        return studioSection === null ? '' : $tr(`studio.section.${studioSection}.title`);
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
        openChatThread();
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
        const sourceTarget =
            orchestrationState.phase === 'ready' && orchestrationState.context_key === contextKey
                ? orchestrationState.workspace.generation_target
                : undefined;
        const generationTarget =
            sourceTarget === null || sourceTarget === undefined
                ? sourceTarget
                : {
                      model_route_id: sourceTarget.model_route_id,
                      generation_preset_id: sourceTarget.generation_preset_id,
                  };
        controller.setRoomGenerationTarget(conversationId, branchId, generationTarget);
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
        const cancelSidebarUnmount = (): void => {
            if (sidebarUnmountTimer === undefined) return;
            clearTimeout(sidebarUnmountTimer);
            sidebarUnmountTimer = undefined;
        };
        const syncLayout = (): void => {
            const nextIsDesktop = layout.matches;
            cancelSidebarUnmount();

            if (nextIsDesktop) {
                sidebarContentMounted = true;
                isDesktop = true;
                if (view === 'home') view = 'chat';
                return;
            }

            isDesktop = false;
            if (!sidebarContentMounted) return;

            const settleDuration = sidebarMotionDuration(SIDEBAR_EXIT_SETTLE_MS);
            if (settleDuration === 0) {
                sidebarContentMounted = false;
                return;
            }
            sidebarUnmountTimer = setTimeout(() => {
                sidebarUnmountTimer = undefined;
                if (!isDesktop) sidebarContentMounted = false;
            }, settleDuration);
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
            if (
                !initialSelectionStarted &&
                initialSelection !== undefined &&
                value.library.phase === 'ready'
            ) {
                const character = value.library.characters.find(
                    (candidate) => candidate.id === initialSelection.characterId,
                );
                if (character !== undefined) {
                    initialSelectionStarted = true;
                    void controller.selectCharacter(character).then(async () => {
                        if (initialSelection.conversationId === undefined) return;
                        const conversation = appState.conversations.items.find(
                            (candidate) => candidate.id === initialSelection.conversationId,
                        );
                        if (conversation !== undefined) {
                            await controller.selectConversation(conversation);
                        }
                    });
                }
            }
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
            cancelSidebarUnmount();
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

{#snippet navigator()}
    <div class="navigator">
        <section class="home-section" class:open={homeSection === 'characters'}>
            <button
                class="section-toggle"
                type="button"
                aria-expanded={homeSection === 'characters'}
                onclick={() => (homeSection = 'characters')}
            >
                <ChevronRight class="chevron" aria-hidden="true" />
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
                <ChevronRight class="chevron" aria-hidden="true" />
                <span class="section-name">{$tr('app.tab.conversations')}</span>
            </button>
            <ConversationPane state={appState} {controller} onOpenChat={openChatThread} />
        </section>
    </div>
{/snippet}

{#snippet createIcon()}
    <Sparkles class="nav-icon" aria-hidden="true" />
{/snippet}

{#snippet settingsIcon()}
    <SlidersHorizontal class="nav-icon" aria-hidden="true" />
{/snippet}

<svelte:head>
    <meta name="description" content={$tr('app.description')} />
</svelte:head>

<div class="app-shell" data-view={view} data-layout={isDesktop ? 'desktop' : 'mobile'}>
    <div class="sidebar-rail" aria-hidden={!isDesktop} inert={!isDesktop}>
        {#if sidebarContentMounted && appState.bootstrap.phase !== 'error'}
            <aside class="sidebar" aria-label={$tr('app.nav.label')}>
                <div class="sidebar-head">
                    <span class="sidebar-logo" aria-hidden="true">
                        <span
                            class="brand-logo-mark"
                            style:--logo-mask={`url("${lorepiaLogoMark}")`}
                        ></span>
                    </span>
                    <h1 class="index-title">LorePia</h1>
                </div>
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
    </div>

    {#if appState.bootstrap.phase === 'error'}
        <main id="main-content" class="main">
            <div class="fatal-screen">
                <span class="large-mark" aria-hidden="true"><CircleAlert /></span>
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
                <section class="mobile-root home-view" aria-label={$tr('app.tab.home')}>
                    <LibraryPane
                        state={appState}
                        {controller}
                        client={appClient}
                        rootView
                        onOpenConversations={showChat}
                    />
                </section>
            {:else if view === 'chat'}
                {#if isDesktop || chatThreadOpen}
                    <ChatPane
                        {appState}
                        {controller}
                        client={appClient}
                        {orchestrationState}
                        {orchestrationController}
                        {messageFocusRequest}
                        onOpenHome={showChat}
                    />
                {:else}
                    <section class="mobile-root chat-list-view" aria-label={$tr('app.tab.chat')}>
                        <ConversationPane
                            state={appState}
                            {controller}
                            client={appClient}
                            rootView
                            onOpenChat={openChatThread}
                        />
                    </section>
                {/if}
            {:else if view === 'create'}
                {#if studioSection === null}
                    {#if !isDesktop}
                        <header class="mobile-top-frame mobile-root-header">
                            <h1>{$tr('studio.title')}</h1>
                        </header>
                    {/if}
                {:else}
                    <header
                        class="mobile-top-frame mobile-top-frame-leading sub-header"
                        style:--mobile-top-fade-progress={pushedTopFadeProgress}
                    >
                        <button
                            class="icon-button ghost mobile-top-action mobile-top-action-left back-button"
                            type="button"
                            aria-label={$tr('app.nav.back')}
                            onclick={closeStudioSection}
                        >
                            <ArrowLeft aria-hidden="true" />
                        </button>
                        {#if studioSection !== null}
                            <h1 bind:this={pushedTitleElement} tabindex="-1">
                                {studioDetailTitle()}
                            </h1>
                        {/if}
                    </header>
                {/if}
                <div
                    bind:this={studioScrollElement}
                    class="view-scroll"
                    class:studio-detail-scroll={studioSection !== null}
                    class:studio-detail-has-actions={studioDetailHasFixedActions(studioDetailPage)}
                    onscroll={handleStudioDetailScroll}
                >
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
                        bind:detailPage={studioDetailPage}
                        onOpenSection={openStudioSection}
                        showIndexHeader={isDesktop}
                    />
                </div>
            {:else}
                {#if settingsSection !== null}
                    <header
                        class="mobile-top-frame mobile-top-frame-leading sub-header"
                        style:--mobile-top-fade-progress={pushedTopFadeProgress}
                    >
                        <button
                            class="icon-button ghost mobile-top-action mobile-top-action-left back-button"
                            type="button"
                            aria-label={$tr('app.nav.back')}
                            onclick={closeSettingsSection}
                        >
                            <ArrowLeft aria-hidden="true" />
                        </button>
                        <h1 bind:this={pushedTitleElement} tabindex="-1">
                            {settingsDetailTitle()}
                        </h1>
                    </header>
                {/if}
                <ProviderSettings
                    {appState}
                    {controller}
                    {personaState}
                    {personaController}
                    bind:personaEditorMode
                    bind:detailPage={settingsDetailPage}
                    bind:editorMode={settingsEditorMode}
                    bind:editorTitle={settingsEditorTitle}
                    section={settingsSection}
                    onOpenSection={openSettingsSection}
                    onDetailScroll={handlePushedDetailScroll}
                />
            {/if}
        </main>
    {/if}

    {#if !isDesktop && studioSection === null && !(view === 'chat' && chatThreadOpen) && !(view === 'settings' && settingsSection !== null)}
        <nav class="tab-bar" aria-label={$tr('app.nav.label')}>
            <button
                class="tab"
                type="button"
                aria-current={view === 'home' ? 'page' : undefined}
                onclick={showHome}
            >
                <House class="nav-icon" aria-hidden="true" />
                <span class="tab-label">{$tr('app.tab.home')}</span>
            </button>
            <button
                class="tab"
                type="button"
                aria-current={view === 'chat' ? 'page' : undefined}
                onclick={showChat}
            >
                <MessageSquare class="nav-icon" aria-hidden="true" />
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
