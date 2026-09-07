<script lang="ts">
    import { ArrowLeft, ChevronRight } from '@lucide/svelte';
    import { tick } from 'svelte';
    import type { LorepiaAppController, LorepiaAppState } from '../app-controller';
    import { t, tr } from '../../lib/i18n';
    import { setThemePreference, themePreference } from '../../lib/theme';
    import type { Overlay } from '../../ui/workspace/view-types';
    import IconButton from '../../ui/workspace/IconButton.svelte';
    import ChoiceField from '../../ui/workspace/ChoiceField.svelte';
    import ChoiceSheet from '../../ui/workspace/ChoiceSheet.svelte';
    import type { ChoiceRequest } from '../../ui/workspace/settings-choice';
    import { edgeBack, requestBack } from '../../ui/workspace/edge-back';
    import { pageSlide } from '../../ui/workspace/navigation-motion';
    import { trapFocus } from '../../ui/workspace/focus-trap';

    let {
        kind,
        appState,
        controller,
        onclose,
        oncreated,
        onproviders,
        onadvanced,
    }: {
        kind: Overlay;
        appState: LorepiaAppState;
        controller: LorepiaAppController;
        onclose: () => void;
        oncreated: () => void;
        onproviders: () => void;
        onadvanced: () => void;
    } = $props();
    let choice = $state<ChoiceRequest | null>(null);
    let opener: HTMLElement | null = null;
    let busy = $state(false);
    let panel: HTMLElement;
    const title = $derived(
        kind === 'app-settings'
            ? $tr('uiPreview.appSettings')
            : kind === 'new-chat'
              ? $tr('uiPreview.newChat')
              : kind === 'room-settings'
                ? $tr('uiPreview.roomSettings')
                : $tr('uiPreview.cardInfo'),
    );
    function greetingLabel(id: string) {
        const items = appState.greeting_catalog.value?.greetings ?? [];
        const item = items.find((value) => value.id === id);
        return item?.kind === 'default'
            ? t('workspace.defaultGreeting')
            : t('workspace.alternateGreeting', {
                  number:
                      items
                          .filter((value) => value.kind === 'alternate')
                          .findIndex((value) => value.id === id) + 1,
              });
    }
    function openChoice(request: ChoiceRequest, trigger: HTMLElement) {
        if (!request.options.length) return;
        opener = trigger;
        choice = request;
    }
    function closeChoice() {
        choice = null;
        void tick().then(() => opener?.focus({ preventScroll: true }));
    }
    async function create() {
        if (busy) return;
        busy = true;
        try {
            if (await controller.openNewConversation()) oncreated();
        } finally {
            busy = false;
        }
    }
    async function mode(value: string) {
        if (value !== 'chat' && value !== 'story') return;
        busy = true;
        try {
            await controller.setConversationMode(value);
        } finally {
            busy = false;
        }
    }
</script>

<div
    class="ui-overlay ui-live-settings"
    transition:pageSlide
    bind:this={panel}
    role="dialog"
    aria-modal="true"
    aria-label={title}
    tabindex="-1"
    use:edgeBack={{ onback: onclose, enabled: !choice && !busy }}
    onkeydown={(event) => {
        trapFocus(event);
        if (event.key === 'Escape') {
            event.stopPropagation();
            if (choice) closeChoice();
            else if (!busy) requestBack(panel);
        }
    }}
>
    <div class="ui-settings-base" inert={!!choice} aria-hidden={!!choice}>
        <header class="ui-page-header ui-navigation-header">
            <IconButton
                label={$tr('workspace.back')}
                disabled={busy}
                onclick={() => requestBack(panel)}><ArrowLeft /></IconButton
            >
            <h1>{title}</h1>
        </header>
        <div class="ui-settings-body">
            {#if kind === 'app-settings'}
                <section class="ui-settings-group">
                    <ChoiceField
                        label={$tr('uiPreview.theme')}
                        value={$tr(
                            $themePreference === 'dark'
                                ? 'uiPreview.dark'
                                : $themePreference === 'light'
                                  ? 'uiPreview.light'
                                  : 'uiPreview.system',
                        )}
                        onopen={(trigger) =>
                            openChoice(
                                {
                                    label: t('uiPreview.theme'),
                                    value: $themePreference,
                                    options: [
                                        { value: 'system', label: t('uiPreview.system') },
                                        { value: 'light', label: t('uiPreview.light') },
                                        { value: 'dark', label: t('uiPreview.dark') },
                                    ],
                                    onselect: (value) =>
                                        setThemePreference(value as 'system' | 'light' | 'dark'),
                                },
                                trigger,
                            )}
                    />
                    <button class="ui-choice-field ui-pressable" onclick={onproviders}
                        ><span class="ui-press-visual"
                            ><span>{$tr('workspace.aiSettings')}</span><ChevronRight /></span
                        ></button
                    >
                    <button class="ui-choice-field ui-pressable" onclick={onadvanced}
                        ><span class="ui-press-visual"
                            ><span>{$tr('workspace.advanced')}</span><ChevronRight /></span
                        ></button
                    >
                </section>
            {:else if kind === 'new-chat'}
                <section class="ui-settings-group">
                    {#if appState.greeting_catalog.phase === 'loading'}<p role="status">
                            {$tr('workspace.loading')}
                        </p>
                    {:else if appState.greeting_catalog.value}
                        <ChoiceField
                            label={$tr('workspace.greeting')}
                            value={appState.greeting_catalog.selected_greeting_id
                                ? greetingLabel(appState.greeting_catalog.selected_greeting_id)
                                : $tr('workspace.noGreeting')}
                            onopen={(trigger) =>
                                openChoice(
                                    {
                                        label: t('workspace.greeting'),
                                        value: appState.greeting_catalog.selected_greeting_id ?? '',
                                        options: (appState.greeting_catalog.value?.greetings ?? [])
                                            .filter((item) => item.enabled)
                                            .map((item) => ({
                                                value: item.id,
                                                label: greetingLabel(item.id),
                                            })),
                                        onselect: (value) => {
                                            controller.selectGreeting(value);
                                        },
                                    },
                                    trigger,
                                )}
                        />
                    {/if}
                    {#if appState.greeting_catalog.error}<p role="alert">
                            {appState.greeting_catalog.error}
                        </p>{/if}
                </section>
                <button
                    class="ui-submit ui-pressable"
                    disabled={busy || appState.greeting_catalog.phase !== 'ready'}
                    onclick={() => void create()}>{$tr('workspace.newChat')}</button
                >
            {:else if kind === 'room-settings'}
                <section class="ui-settings-group">
                    <ChoiceField
                        label={$tr('workspace.roomMode')}
                        value={$tr(
                            appState.conversation_state?.selected_mode === 'story'
                                ? 'uiPreview.storyMode'
                                : 'uiPreview.chatMode',
                        )}
                        onopen={(trigger) => {
                            if (!busy)
                                openChoice(
                                    {
                                        label: t('workspace.roomMode'),
                                        value: appState.conversation_state?.selected_mode ?? 'chat',
                                        options: [
                                            { value: 'chat', label: t('uiPreview.chatMode') },
                                            { value: 'story', label: t('uiPreview.storyMode') },
                                        ],
                                        onselect: (value) => void mode(value),
                                    },
                                    trigger,
                                );
                        }}
                    />
                    <button class="ui-choice-field ui-pressable" onclick={onproviders}
                        ><span class="ui-press-visual"
                            ><span>{$tr('workspace.aiSettings')}</span><ChevronRight /></span
                        ></button
                    >
                    <button class="ui-choice-field ui-pressable" onclick={onadvanced}
                        ><span class="ui-press-visual"
                            ><span>{$tr('workspace.advanced')}</span><ChevronRight /></span
                        ></button
                    >
                </section>
            {:else}
                <section class="ui-settings-group">
                    <h2>{appState.selected_character?.name}</h2>
                    <p class="ui-live-description">
                        {appState.selected_character?.description || $tr('workspace.noDescription')}
                    </p>
                </section>
                <p class="ui-live-hint">{$tr('workspace.cardReadOnly')}</p>
                <button class="ui-choice-field ui-pressable" onclick={onadvanced}
                    ><span class="ui-press-visual"
                        ><span>{$tr('workspace.advanced')}</span><ChevronRight /></span
                    ></button
                >
            {/if}
            {#if appState.conversations.error || appState.messages.error}<p role="alert">
                    {appState.conversations.error ?? appState.messages.error}
                </p>{/if}
        </div>
    </div>
    {#if choice}<ChoiceSheet request={choice} onclose={closeChoice} />{/if}
</div>
