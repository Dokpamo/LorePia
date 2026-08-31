<script lang="ts">
    import { ArrowUp, ChevronDown, Maximize2, Plus } from '@lucide/svelte';

    import type { ChatComposerState } from './composer-state.svelte';

    interface Props {
        state: ChatComposerState;
        desktop: boolean;
        composerConfigurationLabel: string;
        disabled: boolean;
        generationActive: boolean;
        onSubmit: () => void | Promise<void>;
        onOpenSettings: () => void;
        onCancelGeneration: () => void | Promise<void>;
    }

    let {
        state,
        desktop,
        composerConfigurationLabel,
        disabled,
        generationActive,
        onSubmit,
        onOpenSettings,
        onCancelGeneration,
    }: Props = $props();

    function measureComposer(node: HTMLTextAreaElement, draftValue: string) {
        return state.measureComposer(node, draftValue);
    }
</script>

<form
    class="composer"
    aria-label="메시지 작성"
    aria-hidden={state.fullscreen}
    inert={state.fullscreen}
    onsubmit={(event) => {
        event.preventDefault();
        void onSubmit();
    }}
>
    <div
        class="composer-field"
        bind:this={state.field}
        class:has-draft={state.draft.trim().length > 0}
        class:expanded={state.expanded}
        class:can-fullscreen={state.canFullscreen}
        class:overflows={state.overflows}
    >
        <div class="composer-text-region">
            <textarea
                id="chat-draft"
                aria-label="메시지"
                bind:this={state.textarea}
                bind:value={state.draft}
                use:measureComposer={state.draft}
                rows="1"
                maxlength="131072"
                placeholder={desktop ? '무엇이든 요청하세요' : undefined}
                {disabled}
                oncompositionstart={() => (state.compositionActive = true)}
                oncompositionend={() => (state.compositionActive = false)}
                onkeydown={(event) => state.handleKeydown(event)}></textarea>
        </div>
        <div class="composer-action-row">
            <button
                class="composer-leading-action"
                bind:this={state.leadingAction}
                type="button"
                aria-label="추가"
                onclick={() => state.focusTextarea()}
            >
                <Plus aria-hidden="true" />
            </button>
            <span class="composer-action-spacer" aria-hidden="true"></span>
            {#if desktop && composerConfigurationLabel !== ''}
                <button
                    class="composer-desktop-model"
                    type="button"
                    title={composerConfigurationLabel}
                    aria-label={`생성 설정: ${composerConfigurationLabel}`}
                    onclick={onOpenSettings}
                >
                    <span>{composerConfigurationLabel}</span>
                    <ChevronDown aria-hidden="true" />
                </button>
            {/if}
            <button
                class="composer-expand-action"
                class:available={state.canFullscreen}
                type="button"
                aria-label="전체화면으로 작성"
                aria-hidden={!state.canFullscreen}
                tabindex={state.canFullscreen ? 0 : -1}
                disabled={!state.canFullscreen}
                onclick={() => void state.setFullscreen(true)}
            >
                <Maximize2 aria-hidden="true" />
            </button>
            {#if generationActive}
                <button
                    class="danger compact composer-trailing-action"
                    type="button"
                    aria-label="응답 생성 취소"
                    onclick={() => void onCancelGeneration()}
                >
                    중지
                </button>
            {:else if state.draft.trim().length > 0 || desktop}
                <button
                    class="primary send-button composer-trailing-action"
                    bind:this={state.sendButton}
                    type="submit"
                    disabled={state.sending || state.draft.trim().length === 0}
                    aria-label="메시지 보내기"
                >
                    <ArrowUp class="chat-send-icon" aria-hidden="true" />
                </button>
            {/if}
        </div>
    </div>
</form>
