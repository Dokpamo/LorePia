<script lang="ts">
    import { onMount, tick } from 'svelte';
    import { cubicOut } from 'svelte/easing';
    import { Tween } from 'svelte/motion';
    import { t, tr } from '../../lib/i18n';
    import CharacterPage from './CharacterPage.svelte';
    import ChatPage from './ChatPage.svelte';
    import CreatorPage from './CreatorPage.svelte';
    import SettingsPage from './SettingsPage.svelte';
    import TextEditor from './TextEditor.svelte';
    import ChoiceSheet from '../../ui/workspace/ChoiceSheet.svelte';
    import { provideChoiceSheet } from '../../ui/workspace/choice-sheet.svelte';

    import { provideTextEditor } from './text-editor.svelte';
    import {
        createSampleCharacters,
        type Appearance,
        type Overlay,
        type Page,
        type SampleConversation,
        type UiFormValues,
    } from './sample-data';
    import { swipePages } from './swipe-pages';
    import { layoutMode, responsiveLayout, type ViewportSize } from './responsive-layout';
    import { blendLayout, layoutWeights } from './layout-motion';
    import { setNavigationTiming } from './navigation-motion';
    import { SampleChatSession } from './sample-chat.svelte';
    import { requestBack } from './edge-back';
    import { pressFeedback } from './press-feedback';
    import UiTooltip from './UiTooltip.svelte';

    let characters = $state(createSampleCharacters());
    const editor = provideTextEditor();
    const choices = provideChoiceSheet();
    let characterId = $state('a');
    let conversationId = $state('a1');
    let page = $state<Page>(0);
    let overlay = $state<Overlay | null>(null);
    let appearance = $state<Appearance>('system');
    let textScale = $state(1);
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
    let drafts = $state<Record<string, string>>({});
    let remembered = $state<Record<string, string>>({ a: 'a1', b: 'b1', c: 'c1' });
    let nextId = 1;
    let opener: HTMLElement | null = null;
    let overlayReturnPage: Page = 0;
    const character = $derived.by(() => {
        const found = characters.find((item) => item.id === characterId);
        if (!found) throw new Error('Selected sample character is missing.');
        return found;
    });
    const conversation = $derived.by(() => {
        const found = character.histories.find((item) => item.id === conversationId);
        if (!found) throw new Error('Selected sample conversation is missing.');
        return found;
    });
    const draft = $derived(drafts[conversationId] ?? '');
    const chatSession = new SampleChatSession(() => conversation);
    const targetLayout = $derived(responsiveLayout(viewportSize, page, character.subpage));
    const layout = $derived({
        ...targetLayout,
        ...blendLayout(viewportSize, character.subpage, composition.current),
    });

    function finishLayoutMotion(next: Page = page) {
        void composition.set(layoutWeights(targetLayout.mode, next), { duration: 0 });
    }

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
            chatSession.dispose();
            observer?.disconnect();
            reducedMotion.removeEventListener('change', onMotionPreference);
            void composition.set(composition.target, { duration: 0 });
            window.removeEventListener('resize', measure);
            if (resumeFrame !== undefined) cancelAnimationFrame(resumeFrame);
        };
    });

    function navigate(next: Page, fromGesture = false) {
        if (next === 2 && !character.subpage) return;
        if (!fromGesture) setNavigationTiming(track, layout.width * Math.abs(next - page));
        overlay = null;
        finishLayoutMotion(next);
        navigationMotion = true;
        page = next;
        activePage = next;
    }

    function selectCharacter(id: string) {
        const next = characters.find((item) => item.id === id);
        const selected = remembered[id] ?? next?.histories[0]?.id;
        if (!next || !selected) return;
        if (characterId !== id) chatSession.stop();
        characterId = id;
        conversationId = selected;
        if (!next.subpage && page === 2) navigate(1);
    }

    function selectConversation(id: string) {
        if (conversationId !== id) chatSession.stop();
        conversationId = id;
        remembered[characterId] = id;
        navigate(1);
        void tick().then(() =>
            viewport.querySelector<HTMLElement>('.ui-messages')?.focus({ preventScroll: true }),
        );
    }

    function openOverlay(kind: Overlay, trigger: HTMLButtonElement) {
        opener = trigger;
        overlayReturnPage = trigger.closest('.ui-chat') ? 1 : page;
        finishLayoutMotion(0);
        page = 0;
        activePage = 0;
        overlay = kind;
    }

    function closeOverlay() {
        navigate(overlayReturnPage);
        void tick().then(() => opener?.focus({ preventScroll: true }));
    }

    function newConversation(title: string): SampleConversation {
        return {
            id: `ui-conversation-${String(nextId++)}`,
            title: title.trim() || t('uiPreview.newChat'),
            date: t('uiPreview.now'),
            messages: [],
        };
    }

    function save(values: UiFormValues) {
        if (overlay === 'new-chat') {
            const item = newConversation(values.title);
            item.mode = values.mode ?? 'chat';
            item.messages.push({
                id: `${item.id}-first`,
                role: 'assistant',
                text: t('uiPreview.newStory'),
            });
            character.histories.unshift(item);
            selectConversation(item.id);
        } else if (overlay === 'add-character') {
            const id = `ui-character-${String(nextId++)}`;
            const item = newConversation('');
            const name = values.name.trim() || t('uiPreview.newCard');
            characters.push({
                id,
                name,
                description: values.description.trim(),
                thumbnail: name[0] ?? '',
                subpage: values.subpage,
                histories: [item],
            });
            remembered[id] = item.id;
            selectCharacter(id);
            closeOverlay();
        } else if (overlay === 'card-settings') {
            character.name = values.name.trim() || character.name;
            character.description = values.description.trim();
            character.subpage = values.subpage;
            closeOverlay();
        } else if (overlay === 'room-settings') {
            conversation.title = values.title.trim() || conversation.title;
            conversation.mode = values.mode ?? 'chat';
            conversation.responsePreview = values.responsePreview ?? 'complete';
            closeOverlay();
        }
    }

    function send() {
        const text = draft.trim();
        if (!text) return;
        if (chatSession.send(text)) drafts[conversationId] = '';
    }
</script>

<svelte:window
    onkeydown={(event: KeyboardEvent) => {
        if (event.key === 'Tab') keyboardFocus = true;
        if (event.key === 'Escape') {
            if (choices.present) {
                choices.close();
                return;
            }
            const modal = viewport.querySelector<HTMLElement>(
                editor.present ? '.ui-text-editor' : '.ui-overlay',
            );
            if (modal) requestBack(modal);
            else if (!editor.present) navigate(page === 2 ? 1 : 0);
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
        style:--ui-density={layout.density}
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
            inert={editor.present || choices.present}
            aria-hidden={editor.present || choices.present}
            style:--ui-page-offset={`${String(layout.offset)}px`}
            use:swipePages={{
                enabled: layout.mode === 'mobile' && overlay === null,
                page,
                subpage: character.subpage,
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
                <div
                    class="ui-management-content"
                    inert={overlay !== null}
                    aria-hidden={overlay !== null}
                >
                    <CharacterPage
                        {characters}
                        {character}
                        {conversationId}
                        oncharacter={selectCharacter}
                        onconversation={selectConversation}
                        onaction={openOverlay}
                    />
                </div>
                {#if overlay}
                    {#key overlay}
                        <SettingsPage
                            kind={overlay}
                            {character}
                            {conversation}
                            {appearance}
                            {textScale}
                            onappearance={(value: Appearance) => {
                                appearance = value;
                            }}
                            ontextscale={(value: number) => {
                                textScale = value;
                            }}
                            onclose={closeOverlay}
                            onsave={save}
                        />
                    {/key}
                {/if}
            </section>
            <section
                class="ui-page ui-chat"
                data-conversation-mode={conversation.mode ?? 'chat'}
                onfocusin={() => {
                    activePage = 1;
                }}
                aria-label={$tr('uiPreview.chat')}
                inert={overlay !== null || !layout.visible.includes(1)}
                aria-hidden={overlay !== null || !layout.visible.includes(1)}
            >
                <ChatPage
                    {character}
                    {conversation}
                    session={chatSession}
                    {draft}
                    managementVisible={layout.visible.includes(0)}
                    creatorVisible={layout.visible.includes(2)}
                    onnavigate={navigate}
                    ondraft={(value: string) => {
                        drafts[conversationId] = value;
                    }}
                    oninteract={() => {
                        activePage = 1;
                    }}
                    onsettings={(trigger: HTMLButtonElement) =>
                        openOverlay('room-settings', trigger)}
                    onsend={send}
                />
            </section>
            {#if character.subpage}
                <section
                    class="ui-page ui-creator"
                    onfocusin={() => {
                        activePage = 2;
                    }}
                    aria-label={$tr('uiPreview.subpage')}
                    inert={overlay !== null || !layout.visible.includes(2)}
                    aria-hidden={overlay !== null || !layout.visible.includes(2)}
                >
                    <CreatorPage showBack={layout.mode !== 'wide'} onback={() => navigate(1)} />
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
        {#if choices.request}
            <ChoiceSheet
                request={choices.request}
                onclose={() => choices.close()}
                onclosed={() => choices.finish()}
            />
        {/if}
        {#if surface}<UiTooltip root={surface} />{/if}
    </main>
</div>
