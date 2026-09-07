<script lang="ts">
    import { pageSlide } from './navigation-motion';
    import { ArrowLeft } from '@lucide/svelte';
    import { onMount, tick, untrack } from 'svelte';
    import { t, tr, type MessageKey } from '../../lib/i18n';
    import EditField from './EditField.svelte';
    import DiscardChanges from './DiscardChanges.svelte';
    import ChoiceField from './ChoiceField.svelte';
    import ChoiceSheet from './ChoiceSheet.svelte';
    import type { ChoiceRequest } from './settings-choice';
    import { trapFocus } from './focus-trap';
    import { edgeBack, requestBack } from './edge-back';
    import type {
        Appearance,
        Overlay,
        SampleCharacter,
        SampleConversation,
        UiFormValues,
    } from './sample-data';

    let {
        kind,
        character,
        conversation,
        appearance,
        textScale,
        onappearance,
        ontextscale,
        onclose,
        onsave,
    }: {
        kind: Overlay;
        character: SampleCharacter;
        conversation: SampleConversation;
        appearance: Appearance;
        textScale: number;
        onappearance: (value: Appearance) => void;
        ontextscale: (value: number) => void;
        onclose: () => void;
        onsave: (values: UiFormValues) => void;
    } = $props();
    const titles: Record<Overlay, MessageKey> = {
        'app-settings': 'uiPreview.appSettings',
        'card-info': 'uiPreview.cardInfo',
        'card-settings': 'uiPreview.cardSettings',
        'room-settings': 'uiPreview.roomSettings',
        'new-chat': 'uiPreview.newChat',
        'add-character': 'uiPreview.add',
    };
    let name = $state(untrack(() => (kind === 'add-character' ? '' : character.name)));
    let description = $state(
        untrack(() => (kind === 'add-character' ? '' : character.description)),
    );
    let title = $state(untrack(() => (kind === 'new-chat' ? '' : conversation.title)));
    const nameRequired = $derived(
        kind === 'card-settings' ? t('uiPreview.cardNameRequired') : undefined,
    );
    const titleRequired = $derived(
        kind === 'room-settings' ? t('uiPreview.chatNameRequired') : undefined,
    );
    const nameError = $derived(nameRequired && !name.trim() ? nameRequired : undefined);
    const titleError = $derived(titleRequired && !title.trim() ? titleRequired : undefined);
    let subpage = $state(untrack(() => character.subpage));
    let mode = $state(untrack(() => conversation.mode ?? 'chat'));
    let responsePreview = $state(untrack(() => conversation.responsePreview ?? 'complete'));
    const original = untrack(() =>
        JSON.stringify({ name, description, title, subpage, mode, responsePreview }),
    );
    const dirty = $derived(
        JSON.stringify({ name, description, title, subpage, mode, responsePreview }) !== original,
    );
    let confirming = $state(false);
    let choice = $state<ChoiceRequest | null>(null);
    let choiceOpener: HTMLButtonElement;
    let backButton: HTMLButtonElement;
    let panel: HTMLDivElement;
    onMount(() => backButton.focus({ preventScroll: true }));
    function beforeBack() {
        if (confirming) return false;
        if (!dirty) return true;
        confirming = true;
        return false;
    }
    function keepEditing() {
        confirming = false;
        void tick().then(() => backButton.focus({ preventScroll: true }));
    }
    function openChoice(request: ChoiceRequest, opener: HTMLButtonElement) {
        choiceOpener = opener;
        choice = request;
    }
    function closeChoice() {
        choice = null;
        void tick().then(() => choiceOpener.focus({ preventScroll: true }));
    }
    function save() {
        if (nameError || titleError) {
            void tick().then(() =>
                panel.querySelector<HTMLElement>('.ui-edit-field[data-invalid="true"]')?.focus(),
            );
            return;
        }
        onsave({ name, description, title, subpage, mode, responsePreview });
    }
</script>

<div class="ui-overlay-layer" transition:pageSlide>
    <div
        class="ui-overlay"
        bind:this={panel}
        data-kind={kind}
        role="dialog"
        aria-modal="true"
        aria-label={$tr(titles[kind])}
        tabindex="-1"
        inert={confirming || !!choice}
        aria-hidden={confirming || !!choice}
        use:edgeBack={{ onback: onclose, beforeback: beforeBack }}
        onkeydown={trapFocus}
        data-ui-no-swipe
    >
        <header class="ui-page-header ui-navigation-header">
            <button
                bind:this={backButton}
                type="button"
                class="ui-icon-button ui-pressable"
                aria-label={$tr('uiPreview.back')}
                onclick={() => requestBack(panel)}
                ><span class="ui-press-visual"><ArrowLeft aria-hidden="true" /></span></button
            >
        </header>
        <div class="ui-overlay-body">
            <h1 class="ui-detail-title">{$tr(titles[kind])}</h1>
            {#if kind === 'app-settings'}
                <section class="ui-settings-group" aria-label={$tr('uiPreview.appearance')}>
                    <h2>{$tr('uiPreview.appearance')}</h2>
                    <ChoiceField
                        label={$tr('uiPreview.theme')}
                        value={$tr(
                            appearance === 'system'
                                ? 'uiPreview.system'
                                : appearance === 'light'
                                  ? 'uiPreview.light'
                                  : 'uiPreview.dark',
                        )}
                        onopen={(opener: HTMLButtonElement) =>
                            openChoice(
                                {
                                    label: t('uiPreview.theme'),
                                    value: appearance,
                                    options: [
                                        { value: 'system', label: t('uiPreview.system') },
                                        { value: 'light', label: t('uiPreview.light') },
                                        { value: 'dark', label: t('uiPreview.dark') },
                                    ],
                                    onselect: (value) => {
                                        if (
                                            value === 'system' ||
                                            value === 'light' ||
                                            value === 'dark'
                                        )
                                            onappearance(value);
                                    },
                                },
                                opener,
                            )}
                    />
                    <ChoiceField
                        label={$tr('uiPreview.textSize')}
                        value={$tr(
                            textScale === 1 ? 'uiPreview.textDefault' : 'uiPreview.textLarge',
                        )}
                        onopen={(opener: HTMLButtonElement) =>
                            openChoice(
                                {
                                    label: t('uiPreview.textSize'),
                                    value: String(textScale),
                                    options: [
                                        { value: '1', label: t('uiPreview.textDefault') },
                                        { value: '1.1', label: t('uiPreview.textLarge') },
                                    ],
                                    onselect: (value) => ontextscale(Number(value)),
                                },
                                opener,
                            )}
                    />
                </section>
                <section class="ui-settings-group" aria-label={$tr('uiPreview.connections')}>
                    <h2>{$tr('uiPreview.connections')}</h2>
                    <div class="ui-settings-row">
                        <span>{$tr('uiPreview.aiConnection')}</span><small
                            >{$tr('uiPreview.aiSample')}</small
                        >
                    </div>
                    <div class="ui-settings-row">
                        <span>{$tr('uiPreview.data')}</span><small
                            >{$tr('uiPreview.sessionData')}</small
                        >
                    </div>
                </section>
            {:else if kind === 'card-info'}
                <div class="ui-card-cover" role="img" aria-label={$tr('uiPreview.cardImage')}>
                    {character.thumbnail}
                </div>
                <dl class="ui-card-details">
                    <dt>{$tr('uiPreview.cardName')}</dt>
                    <dd>{character.name}</dd>
                    <dt>{$tr('uiPreview.cardDescription')}</dt>
                    <dd>{character.description}</dd>
                </dl>
            {:else}
                <form
                    class="ui-edit-form"
                    onsubmit={(event) => {
                        event.preventDefault();
                        save();
                    }}
                >
                    {#if kind === 'new-chat' || kind === 'room-settings'}
                        <fieldset class="ui-chat-mode-options">
                            <legend>{$tr('uiPreview.conversationMode')}</legend>
                            {#each ['chat', 'story'] as value (value)}
                                <label>
                                    <input
                                        type="radio"
                                        name="ui-conversation-mode"
                                        {value}
                                        bind:group={mode}
                                    />
                                    <span
                                        ><strong
                                            >{$tr(
                                                value === 'chat'
                                                    ? 'uiPreview.chatMode'
                                                    : 'uiPreview.storyMode',
                                            )}</strong
                                        ><small
                                            >{$tr(
                                                value === 'chat'
                                                    ? 'uiPreview.chatModeHint'
                                                    : 'uiPreview.storyModeHint',
                                            )}</small
                                        ></span
                                    >
                                </label>
                            {/each}
                        </fieldset>
                        <EditField
                            label={$tr('uiPreview.chatName')}
                            hint={$tr('uiPreview.settingsEditorHint')}
                            requiredMessage={titleRequired}
                            error={titleError}
                            value={title}
                            onchange={(value: string) => (title = value.replaceAll('\n', ' '))}
                            placeholder={$tr('uiPreview.newChat')}
                            maxlength={60}
                        />
                        {#if kind === 'room-settings'}
                            <ChoiceField
                                label={$tr('uiPreview.responsePreview')}
                                value={$tr(
                                    responsePreview === 'complete'
                                        ? 'uiPreview.previewComplete'
                                        : responsePreview === 'failed'
                                          ? 'uiPreview.previewFailed'
                                          : 'uiPreview.previewSlow',
                                )}
                                onopen={(opener: HTMLButtonElement) =>
                                    openChoice(
                                        {
                                            label: t('uiPreview.responsePreview'),
                                            value: responsePreview,
                                            options: [
                                                {
                                                    value: 'complete',
                                                    label: t('uiPreview.previewComplete'),
                                                },
                                                {
                                                    value: 'failed',
                                                    label: t('uiPreview.previewFailed'),
                                                },
                                                {
                                                    value: 'slow',
                                                    label: t('uiPreview.previewSlow'),
                                                },
                                            ],
                                            onselect: (value) => {
                                                if (
                                                    value === 'complete' ||
                                                    value === 'failed' ||
                                                    value === 'slow'
                                                )
                                                    responsePreview = value;
                                            },
                                        },
                                        opener,
                                    )}
                            />
                        {/if}
                    {:else}
                        <EditField
                            label={$tr('uiPreview.cardName')}
                            hint={$tr('uiPreview.settingsEditorHint')}
                            requiredMessage={nameRequired}
                            error={nameError}
                            value={name}
                            onchange={(value: string) => (name = value.replaceAll('\n', ' '))}
                            placeholder={$tr('uiPreview.newCard')}
                            maxlength={40}
                        />
                        <EditField
                            label={$tr('uiPreview.cardDescription')}
                            hint={$tr('uiPreview.settingsEditorHint')}
                            value={description}
                            onchange={(value: string) => (description = value)}
                            maxlength={300}
                        />
                        <label class="ui-checkbox-row"
                            ><span>{$tr('uiPreview.useSubpage')}</span><input
                                type="checkbox"
                                bind:checked={subpage}
                            /></label
                        >
                    {/if}
                    <button type="button" class="ui-submit ui-pressable" onclick={save}>
                        <span class="ui-press-visual"
                            >{$tr(
                                kind === 'new-chat'
                                    ? 'uiPreview.startChat'
                                    : kind === 'add-character'
                                      ? 'uiPreview.add'
                                      : 'uiPreview.save',
                            )}</span
                        >
                    </button>
                </form>
            {/if}
        </div>
    </div>
    {#if confirming}<DiscardChanges onkeep={keepEditing} ondiscard={onclose} />{/if}
    {#if choice}<ChoiceSheet request={choice} onclose={closeChoice} />{/if}
</div>
