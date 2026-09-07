<script lang="ts">
    import { onMount, untrack, type Snippet } from 'svelte';
    import { cubicOut } from 'svelte/easing';
    import { Tween } from 'svelte/motion';
    import { tr } from '../../lib/i18n';
    import TextEditor from './TextEditor.svelte';
    import { provideTextEditor } from './text-editor.svelte';
    import type { Appearance, Page } from './view-types';
    import { swipePages } from './swipe-pages';
    import { layoutMode, responsiveLayout, type ViewportSize } from './responsive-layout';
    import { blendLayout, layoutWeights } from './layout-motion';
    import { setNavigationTiming } from './navigation-motion';
    import { requestBack } from './edge-back';
    import { pressFeedback } from './press-feedback';
    import UiTooltip from './UiTooltip.svelte';
    let {
        page = $bindable<Page>(0),
        overlay = false,
        subpage = false,
        appearance = 'system',
        textScale = 1,
        conversationMode = 'chat',
        management,
        chat,
        detail,
        creator,
        onback,
    }: {
        page?: Page;
        overlay?: boolean;
        subpage?: boolean;
        appearance?: Appearance;
        textScale?: number;
        conversationMode?: 'chat' | 'story';
        management: Snippet<[(page: Page) => void]>;
        chat: Snippet<[(page: Page) => void, boolean, boolean]>;
        detail?: Snippet;
        creator?: Snippet<[(page: Page) => void]>;
        onback: () => void;
    } = $props();
    const editor = provideTextEditor();
    let viewport: HTMLDivElement;
    let surface = $state<HTMLElement>();
    let track: HTMLDivElement;
    let viewportSize = $state<ViewportSize>({ width: 393, height: 748 });
    let activePage = $state<Page>(0);
    let resizing = $state(false);
    let navigationMotion = $state(true);
    let keyboardFocus = $state(false);
    const composition = new Tween(layoutWeights('mobile', 0), { duration: 180, easing: cubicOut });
    const reflowing = $derived(composition.current !== composition.target);
    const targetLayout = $derived(responsiveLayout(viewportSize, page, subpage));
    const layout = $derived({
        ...targetLayout,
        ...blendLayout(viewportSize, subpage, composition.current),
    });

    function finishLayoutMotion(next: Page = page) {
        void composition.set(layoutWeights(targetLayout.mode, next), { duration: 0 });
    }
    let observedPage = untrack(() => page);
    $effect(() => {
        const next = page;
        untrack(() => {
            if (observedPage === next) return;
            observedPage = next;
            setNavigationTiming(track, layout.width * Math.abs(next - activePage));
            finishLayoutMotion(next);
            navigationMotion = true;
            activePage = next;
        });
    });

    onMount(() => {
        let initialized = false;
        let resumeFrame: number | undefined;
        const reducedMotion = window.matchMedia('(prefers-reduced-motion: reduce)');
        const onMotionPreference = () => {
            if (reducedMotion.matches) finishLayoutMotion();
        };
        reducedMotion.addEventListener('change', onMotionPreference);
        const update = (size: ViewportSize) => {
            if (!size.width || !size.height) return;
            if (size.width === viewportSize.width && size.height === viewportSize.height) return;
            // Commit resize geometry together, without retargeting page-settle motion.
            resizing = true;
            navigationMotion = false;
            if (resumeFrame !== undefined) cancelAnimationFrame(resumeFrame);
            const nextMode = layoutMode(size.width);
            if (nextMode !== targetLayout.mode) {
                if (overlay) page = 0;
                else if (targetLayout.visible.includes(activePage)) page = activePage;
                void composition.set(layoutWeights(nextMode, page), {
                    duration: initialized && !reducedMotion.matches ? 180 : 0,
                });
            }
            viewportSize = size;
            initialized = true;
            resumeFrame = requestAnimationFrame(() => {
                resizing = false;
                resumeFrame = undefined;
            });
        };
        const measure = () =>
            update({ width: viewport.clientWidth, height: viewport.clientHeight });
        const observer =
            typeof ResizeObserver === 'undefined'
                ? null
                : new ResizeObserver(([entry]) => {
                      if (entry) update(entry.contentRect);
                  });
        measure();
        if (observer) observer.observe(viewport);
        else window.addEventListener('resize', measure);
        return () => {
            observer?.disconnect();
            reducedMotion.removeEventListener('change', onMotionPreference);
            void composition.set(composition.target, { duration: 0 });
            window.removeEventListener('resize', measure);
            if (resumeFrame !== undefined) cancelAnimationFrame(resumeFrame);
        };
    });

    function navigate(next: Page, fromGesture = false) {
        if (next === 2 && !subpage) return;
        if (!fromGesture) setNavigationTiming(track, layout.width * Math.abs(next - page));
        finishLayoutMotion(next);
        navigationMotion = true;
        page = next;
        activePage = next;
    }
</script>

<svelte:window
    onkeydown={(event: KeyboardEvent) => {
        if (event.key === 'Tab') keyboardFocus = true;
        if (event.key === 'Escape') {
            const modal = viewport.querySelector<HTMLElement>(
                editor.present ? '.ui-text-editor' : '.ui-overlay',
            );
            if (modal) requestBack(modal);
            else if (!editor.present) {
                if (overlay) onback();
                else navigate(page === 2 ? 1 : 0);
            }
        }
    }}
/>

<div class="ui-viewport" data-appearance={appearance} bind:this={viewport}>
    <main
        class="ui-preview"
        bind:this={surface}
        use:pressFeedback
        data-layout={layout.mode}
        data-reflowing={reflowing}
        data-resizing={resizing}
        data-navigation-motion={navigationMotion}
        data-keyboard-focus={keyboardFocus}
        data-appearance={appearance}
        style:--ui-text-scale={textScale}
        style:width={layout.scale < 1 ? `${String(layout.width)}px` : undefined}
        style:height={layout.scale < 1 ? `${String(layout.height)}px` : undefined}
        style:transform={layout.scale < 1 ? `scale(${String(layout.scale)})` : undefined}
        style:--ui-left-width={`${String(layout.left)}px`}
        style:--ui-chat-width={`${String(layout.chat)}px`}
        style:--ui-right-width={`${String(layout.right)}px`}
        onpointerdowncapture={() => {
            keyboardFocus = false;
            if (reflowing) finishLayoutMotion();
        }}
    >
        <div
            class="ui-track"
            bind:this={track}
            inert={editor.present}
            aria-hidden={editor.present}
            style:--ui-page-offset={`${String(layout.offset)}px`}
            use:swipePages={{
                enabled: layout.mode === 'mobile' && !overlay,
                page,
                subpage: subpage,
                navigate: (next) => navigate(next, true),
                onsettle: () => {
                    navigationMotion = true;
                },
            }}
        >
            <section
                class="ui-page ui-management"
                onfocusin={() => {
                    activePage = 0;
                }}
                aria-label={$tr('uiPreview.management')}
                inert={!layout.visible.includes(0)}
                aria-hidden={!layout.visible.includes(0)}
            >
                <div class="ui-management-content" inert={overlay} aria-hidden={overlay}>
                    {@render management(navigate)}
                </div>
                {#if overlay && detail}{@render detail()}{/if}
            </section>
            <section
                class="ui-page ui-chat"
                data-conversation-mode={conversationMode}
                onfocusin={() => {
                    activePage = 1;
                }}
                aria-label={$tr('uiPreview.chat')}
                inert={overlay || !layout.visible.includes(1)}
                aria-hidden={overlay || !layout.visible.includes(1)}
            >
                {@render chat(navigate, layout.visible.includes(0), layout.visible.includes(2))}
            </section>
            {#if subpage}
                <section
                    class="ui-page ui-creator"
                    onfocusin={() => {
                        activePage = 2;
                    }}
                    aria-label={$tr('uiPreview.subpage')}
                    inert={overlay || !layout.visible.includes(2)}
                    aria-hidden={overlay || !layout.visible.includes(2)}
                >
                    {#if creator}{@render creator(navigate)}{/if}
                </section>
            {/if}
        </div>
        {#if editor.request}
            <TextEditor
                request={editor.request}
                onclose={(afterClose?: () => void) => editor.close(afterClose)}
                onclosed={() => editor.finish()}
            />
        {/if}
        {#if surface}<UiTooltip root={surface} />{/if}
    </main>
</div>
