<script lang="ts">
    import { ArrowUp, Minimize2 } from '@lucide/svelte';

    import type { ChatComposerState } from './composer-state.svelte';

    interface Props {
        state: ChatComposerState;
        disabled: boolean;
        onSubmit: () => void | Promise<void>;
    }

    let { state, disabled, onSubmit }: Props = $props();
</script>

<form
    class="composer-fullscreen"
    bind:this={state.fullscreenSurface}
    class:open={state.fullscreen}
    aria-label="전체화면 메시지 작성"
    aria-hidden={!state.fullscreen}
    inert={!state.fullscreen}
    onsubmit={(event) => {
        event.preventDefault();
        void onSubmit();
    }}
>
    <header class="composer-fullscreen-header">
        <button
            class="composer-fullscreen-close"
            bind:this={state.fullscreenCloseButton}
            type="button"
            aria-label="전체화면 입력 닫기"
            onclick={() => void state.setFullscreen(false)}
        >
            <Minimize2 aria-hidden="true" />
        </button>
        <span aria-hidden="true"></span>
        {#if state.draft.trim().length > 0}
            <button
                class="primary send-button"
                bind:this={state.fullscreenSendButton}
                type="submit"
                disabled={state.sending}
                aria-label="메시지 보내기"
            >
                <ArrowUp class="chat-send-icon" aria-hidden="true" />
            </button>
        {/if}
    </header>
    <div class="composer-fullscreen-text-region" bind:this={state.fullscreenTextRegion}>
        <label class="sr-only" for="chat-draft-fullscreen">전체화면 메시지</label>
        <textarea
            id="chat-draft-fullscreen"
            bind:this={state.fullscreenTextarea}
            bind:value={state.draft}
            maxlength="131072"
            {disabled}
            oncompositionstart={() => (state.compositionActive = true)}
            oncompositionend={() => (state.compositionActive = false)}
            onkeydown={(event) => state.handleFullscreenKeydown(event)}></textarea>
    </div>
</form>
