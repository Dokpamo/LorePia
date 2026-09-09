<script lang="ts">
    import { ArrowLeft, ArrowUp, Check, Minimize2, Search, X } from '@lucide/svelte';
    import { onMount, tick, untrack } from 'svelte';
    import { editorMorph } from './editor-morph';
    import { tr } from '../../lib/i18n';
    import { shouldSubmitComposer } from '../../features/chat/composer';
    import IconButton from './IconButton.svelte';
    import { edgeBack, requestBack, type BackDecision } from './edge-back';
    import type { TextEditRequest } from './text-editor.svelte';
    import SearchResults from './SearchResults.svelte';
    import { trapFocus } from './focus-trap';
    import DiscardChanges from './DiscardChanges.svelte';

    let {
        request,
        onclose,
        onclosed,
    }: {
        request: TextEditRequest;
        onclose: (afterClose?: () => void) => void;
        onclosed: () => void;
    } = $props();
    let value = $state(untrack(() => request.value));
    const origin = untrack(() => request.origin);
    let composing = $state(false);
    let attempted = $state(false);
    let confirming = $state(false);
    let resumeBack: (() => Promise<void>) | undefined;
    let saving = $state(false);
    let saveError = $state(false);
    const ids = $props.id();
    const dirty = $derived(value !== request.value);
    const error = $derived(
        (dirty || attempted) && request.requiredMessage && !value.trim()
            ? request.requiredMessage
            : undefined,
    );
    let input = $state<HTMLTextAreaElement | HTMLInputElement>();
    let panel: HTMLDivElement;
    const search = untrack(() => request.search);
    const results = $derived(
        search?.items.filter((item) =>
            item.title.toLocaleLowerCase('ko').includes(value.trim().toLocaleLowerCase('ko')),
        ) ?? [],
    );
    function change(next: string) {
        value = next;
        saveError = false;
        if (!request.applyOnDone) void request.onchange(next);
    }
    async function done() {
        if (composing || saving) return;
        attempted = true;
        if (request.requiredMessage && !value.trim()) {
            input?.focus({ preventScroll: true });
            return;
        }
        if (!request.applyOnDone) {
            onclose();
            return;
        }
        saving = true;
        saveError = false;
        try {
            const accepted = await request.onchange(value);
            if (accepted === false) saveError = true;
            else onclose();
        } catch {
            saveError = true;
        } finally {
            saving = false;
        }
    }
    function beforeBack(): BackDecision {
        if (saving) return false;
        if (confirming) return false;
        if (!request.applyOnDone || !dirty) return true;
        return {
            confirm: (resume) => {
                resumeBack = resume;
                confirming = true;
            },
        };
    }
    async function keepEditing() {
        confirming = false;
        await tick();
        await resumeBack?.();
        resumeBack = undefined;
        input?.focus({ preventScroll: true });
    }
    function close() {
        if (saving) return;
        // The composer folds back into its own field; other editors navigate back.
        if (request.message) onclose();
        else requestBack(panel);
    }
    function selectResult(id: string) {
        if (search) onclose(() => search.onselect(id));
    }
    function clearSearch() {
        change('');
        input?.focus({ preventScroll: true });
    }
    onMount(() => {
        if (!input) return;
        input.focus({ preventScroll: true });
        if (!search) input.setSelectionRange(value.length, value.length);
    });
    function send() {
        if (!value.trim() || composing) return;
        request.onsend?.();
        onclose();
    }
</script>

<div
    class="ui-editor-layer"
    class:ui-editor-morph={!!origin}
    transition:editorMorph={origin}
    onoutroend={(event) => {
        if (event.target !== event.currentTarget) return;
        origin?.().surface.removeAttribute('data-ui-editor-origin');
        onclosed();
    }}
>
    <div
        class="ui-text-editor"
        class:ui-message-editor={request.message}
        bind:this={panel}
        role="dialog"
        tabindex="-1"
        aria-modal="true"
        aria-label={request.label}
        aria-busy={saving}
        inert={confirming}
        aria-hidden={confirming}
        use:edgeBack={{ onback: () => onclose(), beforeback: beforeBack }}
        data-ui-no-swipe
        onkeydowncapture={(event) => {
            if (event.key === 'Escape') {
                event.preventDefault();
                event.stopImmediatePropagation();
                close();
            }
            trapFocus(event);
        }}
    >
        <header
            class={request.message
                ? 'ui-compose-editor-header'
                : 'ui-page-header ui-navigation-header'}
        >
            <div data-editor-back>
                <IconButton
                    label={$tr(
                        request.message ? 'uiPreview.collapseComposer' : 'uiPreview.closeEditor',
                    )}
                    onclick={close}
                    >{#if request.message}<Minimize2 />{:else}<ArrowLeft />{/if}</IconButton
                >
            </div>
            {#if search}
                <div class="ui-search-input" role="search" aria-label={request.label}>
                    <Search aria-hidden="true" />
                    <input
                        readonly={saving}
                        type="search"
                        bind:this={input}
                        aria-label={request.label}
                        placeholder={request.placeholder}
                        {value}
                        maxlength={request.maxlength}
                        autocomplete="off"
                        spellcheck="false"
                        oninput={(event) => change(event.currentTarget.value)}
                        oncompositionstart={() => (composing = true)}
                        oncompositionend={() => (composing = false)}
                        onkeydown={(event) => {
                            if (composing || event.isComposing) return;
                            if (event.key === 'ArrowDown') {
                                event.preventDefault();
                                panel
                                    .querySelector<HTMLButtonElement>('.ui-search-results button')
                                    ?.focus();
                            }
                            if (event.key === 'Enter' && results[0]) {
                                event.preventDefault();
                                selectResult(results[0].id);
                            }
                        }}
                    />
                    {#if value}<IconButton
                            label={$tr('uiPreview.clearChatSearch')}
                            onclick={clearSearch}><X /></IconButton
                        >{/if}
                </div>
            {:else}
                <div class="ui-title-group">
                    {#if !request.message}<strong>{request.label}</strong>{/if}
                </div>
            {/if}
            {#if request.message}
                <div data-editor-send>
                    <IconButton
                        label={$tr('uiPreview.send')}
                        disabled={!value.trim() || composing}
                        onclick={send}><ArrowUp /></IconButton
                    >
                </div>
            {:else if !search}
                <IconButton
                    label={$tr('uiPreview.editDone')}
                    disabled={composing || saving}
                    onclick={done}><Check /></IconButton
                >
            {/if}
        </header>
        {#if search}
            <SearchResults
                items={results}
                query={value}
                onselect={selectResult}
                onclear={clearSearch}
            />
        {:else}<div class="ui-editor-text-region">
                <textarea
                    readonly={saving}
                    bind:this={input}
                    aria-label={request.label}
                    required={!!request.requiredMessage}
                    aria-invalid={error || saveError ? true : undefined}
                    aria-describedby={error
                        ? ids + '-error'
                        : saveError
                          ? ids + '-save-error'
                          : request.hint
                            ? ids + '-hint'
                            : undefined}
                    {value}
                    placeholder={request.placeholder}
                    maxlength={request.maxlength ?? 131072}
                    spellcheck="false"
                    oninput={(event) => {
                        change(event.currentTarget.value);
                    }}
                    oncompositionstart={() => (composing = true)}
                    oncompositionend={() => (composing = false)}
                    onkeydown={(event) => {
                        if (request.message && shouldSubmitComposer(event, composing)) {
                            event.preventDefault();
                            send();
                        }
                    }}></textarea>
            </div>{/if}
        {#if !search}<footer class="ui-editor-footer">
                {#if error}<span id={ids + '-error'} class="ui-field-error" role="alert"
                        >{error}</span
                    >
                {:else if saveError}<span
                        id={ids + '-save-error'}
                        class="ui-field-error"
                        role="alert">{$tr('workspace.saveFailed')}</span
                    >
                {:else if request.hint}<span id={ids + '-hint'} class="ui-editor-hint"
                        >{request.hint}</span
                    >{/if}
                {#if request.message}<span class="ui-editor-key-hint"
                        >{$tr('uiPreview.editorKeys')}</span
                    >
                {:else if request.maxlength}<span>{value.length} / {request.maxlength}</span>{/if}
            </footer>{/if}
    </div>
    {#if confirming}<DiscardChanges onkeep={keepEditing} ondiscard={() => onclose()} />{/if}
</div>
