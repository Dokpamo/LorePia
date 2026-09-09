<script lang="ts">
    import { untrack, type Snippet } from 'svelte';
    import type { Appearance, Page } from '../workspace/view-types';
    import { ROOT_TABS, type RootTab } from './navigation-types';
    import BottomNavigation from './BottomNavigation.svelte';
    import TextEditor from '../workspace/TextEditor.svelte';
    import ChoiceSheet from '../workspace/ChoiceSheet.svelte';
    import UiTooltip from '../workspace/UiTooltip.svelte';
    import { provideTextEditor } from '../workspace/text-editor.svelte';
    import { provideChoiceSheet } from '../workspace/choice-sheet.svelte';
    import { pressFeedback } from '../workspace/press-feedback';
    import { edgeBack, requestBack } from '../workspace/edge-back';
    import { rootTabSwipe } from './root-tab-swipe';
    import './root-tab-swipe.css';

    let {
        page = $bindable<Page>(0),
        rootTab = $bindable<RootTab>('home'),
        overlay = false,
        nested = false,
        appearance = 'system',
        textScale = 1,
        conversationMode = 'chat',
        home,
        chats,
        create,
        settings,
        chat,
        detail,
        creator,
    }: {
        page?: Page;
        rootTab?: RootTab;
        overlay?: boolean;
        nested?: boolean;
        appearance?: Appearance;
        textScale?: number;
        conversationMode?: 'chat' | 'story';
        home: Snippet;
        chats: Snippet;
        create: Snippet;
        settings: Snippet;
        chat: Snippet<[(page: Page) => void, boolean, boolean]>;
        detail: Snippet;
        creator: Snippet<[(page: Page) => void]>;
    } = $props();
    const editor = provideTextEditor();
    const choices = provideChoiceSheet();
    let surface = $state<HTMLElement>();
    let chatSurface: HTMLElement;
    let creatorSurface: HTMLElement;
    let keyboardFocus = $state(false);
    let visited = $state<RootTab[]>([untrack(() => rootTab)]);
    const blocked = $derived(overlay || editor.present || choices.present);
    const snippets = $derived({ home, chats, create, settings });
    $effect(() => {
        if (!visited.includes(rootTab)) visited = [...visited, rootTab];
    });

    function navigate(next: Page) {
        if (next < page) requestBack(page === 2 ? creatorSurface : chatSurface);
        else page = next;
    }
    // Chat stays mounted to preserve its draft and scroll position. Recreate only
    // the gesture's presentation lifetime after a committed departure.
    function persistentBack(node: HTMLElement, options: { enabled: boolean; destination: Page }) {
        let gesture: ReturnType<typeof edgeBack> | undefined;
        function update(next: typeof options) {
            gesture?.destroy();
            delete node.dataset.backDismissed;
            delete node.dataset.backSettling;
            node.style.removeProperty('--ui-back-offset');
            if (next.enabled) node.parentElement?.removeAttribute('data-interactive-dismissed');
            gesture = next.enabled
                ? edgeBack(node, {
                      onback: () => {
                          node.parentElement?.setAttribute('data-interactive-dismissed', 'true');
                          page = next.destination;
                      },
                  })
                : undefined;
        }
        update(options);
        return { update, destroy: () => gesture?.destroy() };
    }
</script>

<svelte:window
    onkeydown={(event: KeyboardEvent) => {
        if (event.key !== 'Escape' || event.defaultPrevented || !surface) return;
        if (surface.querySelector('.ui-confirm-layer')) return;
        if (choices.present) {
            choices.close();
            return;
        }
        const modal = [
            ...surface.querySelectorAll<HTMLElement>(
                editor.present ? '.ui-text-editor' : '.ui-overlay[role="dialog"]',
            ),
        ]
            .reverse()
            .find((node) => !node.closest('[inert], [hidden], [aria-hidden="true"]'));
        if (modal) requestBack(modal);
        else if (!blocked && page !== 0) navigate(page === 2 ? 1 : 0);
    }}
/>

<main
    class="ui-preview seed-app"
    bind:this={surface}
    use:pressFeedback
    data-layout="mobile"
    data-appearance={appearance}
    data-keyboard-focus={keyboardFocus}
    style:--ui-text-scale={textScale}
    onkeydowncapture={(event) => {
        if (event.key === 'Tab') keyboardFocus = true;
    }}
    onpointerdowncapture={() => (keyboardFocus = false)}
>
    <div class="seed-root-stack ui-management" data-navigation-visible={!nested}>
        <div
            class="seed-roots ui-management-content"
            inert={page !== 0 || blocked}
            aria-hidden={page !== 0 || blocked}
            style:--seed-active-tab={ROOT_TABS.indexOf(rootTab)}
            data-root-swipe-enabled={page === 0 && !blocked && !nested}
            use:rootTabSwipe={{
                enabled: page === 0 && !blocked && !nested,
                index: ROOT_TABS.indexOf(rootTab),
                count: ROOT_TABS.length,
                prepare: (index: number) => {
                    const tab = ROOT_TABS[index];
                    if (tab && !visited.includes(tab)) visited = [...visited, tab];
                },
                select: (index: number) => {
                    const tab = ROOT_TABS[index];
                    if (tab) rootTab = tab;
                },
            }}
        >
            {#each visited as tab (tab)}
                <section
                    class="seed-root"
                    inert={rootTab !== tab}
                    aria-hidden={rootTab !== tab}
                    data-root-tab={tab}
                    style:--seed-tab-index={ROOT_TABS.indexOf(tab)}
                >
                    {@render snippets[tab]()}
                </section>
            {/each}
        </div>
        <div
            class="seed-tab-container"
            hidden={nested}
            inert={blocked || nested || page !== 0}
            aria-hidden={blocked || nested || page !== 0}
        >
            <BottomNavigation selected={rootTab} onchange={(tab: RootTab) => (rootTab = tab)} />
        </div>
    </div>
    <section
        class="seed-detail-page"
        class:seed-detail-active={page !== 0}
        inert={page !== 1 || blocked}
        aria-hidden={page !== 1 || blocked}
    >
        <div
            class="ui-page ui-chat"
            bind:this={chatSurface}
            data-conversation-mode={conversationMode}
            use:persistentBack={{ enabled: page === 1 && !blocked, destination: 0 }}
        >
            {@render chat(navigate, false, false)}
        </div>
    </section>
    <section
        class="seed-detail-page"
        class:seed-detail-active={page === 2}
        inert={page !== 2 || blocked}
        aria-hidden={page !== 2 || blocked}
    >
        <div
            class="ui-page ui-creator"
            bind:this={creatorSurface}
            use:persistentBack={{ enabled: page === 2 && !blocked, destination: 1 }}
        >
            {@render creator(navigate)}
        </div>
    </section>
    <div
        class="seed-modal-stack"
        class:seed-modal-active={overlay}
        inert={editor.present || choices.present}
    >
        {@render detail()}
    </div>
    {#if editor.request}<TextEditor
            request={editor.request}
            onclose={(afterClose?: () => void) => editor.close(afterClose)}
            onclosed={() => editor.finish()}
        />{/if}
    {#if choices.request}<ChoiceSheet
            request={choices.request}
            onclose={() => choices.close()}
            onclosed={() => choices.finish()}
        />{/if}
    {#if surface}<UiTooltip root={surface} />{/if}
</main>
