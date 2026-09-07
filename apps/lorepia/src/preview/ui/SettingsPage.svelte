<script lang="ts">
    import { pageSlide } from './navigation-motion';
    import { ArrowLeft, ChevronDown } from '@lucide/svelte';
    import { onMount, untrack } from 'svelte';
    import { tr, type MessageKey } from '../../lib/i18n';
    import EditField from './EditField.svelte';
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
    let subpage = $state(untrack(() => character.subpage));
    let mode = $state(untrack(() => conversation.mode ?? 'chat'));
    let responsePreview = $state(untrack(() => conversation.responsePreview ?? 'complete'));
    let backButton: HTMLButtonElement;
    let panel: HTMLDivElement;
    onMount(() => backButton.focus({ preventScroll: true }));
</script>

<div class="ui-overlay-layer" transition:pageSlide>
    <div
        class="ui-overlay"
        bind:this={panel}
        data-kind={kind}
        use:edgeBack={{ onback: onclose }}
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
                    <label class="ui-settings-row" for="ui-theme">
                        <span>{$tr('uiPreview.theme')}</span>
                        <span class="ui-select-field">
                            <select
                                id="ui-theme"
                                value={appearance}
                                onchange={(event) => {
                                    const value = event.currentTarget.value;
                                    if (value === 'system' || value === 'light' || value === 'dark')
                                        onappearance(value);
                                }}
                            >
                                <option value="system">{$tr('uiPreview.system')}</option><option
                                    value="light">{$tr('uiPreview.light')}</option
                                ><option value="dark">{$tr('uiPreview.dark')}</option>
                            </select>
                            <ChevronDown aria-hidden="true" />
                        </span>
                    </label>
                    <label class="ui-settings-row" for="ui-text-size">
                        <span>{$tr('uiPreview.textSize')}</span>
                        <span class="ui-select-field">
                            <select
                                id="ui-text-size"
                                value={textScale}
                                onchange={(event) => ontextscale(Number(event.currentTarget.value))}
                            >
                                <option value={1}>{$tr('uiPreview.textDefault')}</option><option
                                    value={1.1}>{$tr('uiPreview.textLarge')}</option
                                >
                            </select>
                            <ChevronDown aria-hidden="true" />
                        </span>
                    </label>
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
                        onsave({ name, description, title, subpage, mode, responsePreview });
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
                            value={title}
                            onchange={(value: string) => (title = value.replaceAll('\n', ' '))}
                            placeholder={$tr('uiPreview.newChat')}
                            maxlength={60}
                        />
                        {#if kind === 'room-settings'}
                            <label class="ui-settings-row" for="ui-response-preview">
                                <span>{$tr('uiPreview.responsePreview')}</span>
                                <select id="ui-response-preview" bind:value={responsePreview}>
                                    <option value="complete"
                                        >{$tr('uiPreview.previewComplete')}</option
                                    >
                                    <option value="failed">{$tr('uiPreview.previewFailed')}</option>
                                </select>
                            </label>
                        {/if}
                    {:else}
                        <EditField
                            label={$tr('uiPreview.cardName')}
                            value={name}
                            onchange={(value: string) => (name = value.replaceAll('\n', ' '))}
                            placeholder={$tr('uiPreview.newCard')}
                            maxlength={40}
                        />
                        <EditField
                            label={$tr('uiPreview.cardDescription')}
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
                    <button
                        type="button"
                        class="ui-submit ui-pressable"
                        onclick={() =>
                            onsave({ name, description, title, subpage, mode, responsePreview })}
                    >
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
</div>
