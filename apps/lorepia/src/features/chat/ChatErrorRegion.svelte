<script lang="ts">
    import { X } from '@lucide/svelte';

    interface Props {
        chatError: string | null;
        runtimeError: string | null;
        interactionError: string | null;
        onDismissChat: () => void;
        onDismissRuntime: () => void;
        onDismissInteraction: () => void;
    }

    let {
        chatError,
        runtimeError,
        interactionError,
        onDismissChat,
        onDismissRuntime,
        onDismissInteraction,
    }: Props = $props();
</script>

<div class="chat-error-region" role="region" aria-label="채팅 오류 알림">
    {#if chatError !== null}
        <div class="chat-error-notice" role="alert">
            <p>{chatError}</p>
            <button
                class="chat-error-dismiss"
                type="button"
                aria-label="채팅 오류 닫기"
                title="닫기"
                onclick={onDismissChat}
            >
                <X aria-hidden="true" />
            </button>
        </div>
    {/if}
    {#if runtimeError !== null}
        <div class="chat-error-notice" role="alert">
            <p>{runtimeError}</p>
            <button
                class="chat-error-dismiss"
                type="button"
                aria-label="캐릭터 기능 오류 닫기"
                title="닫기"
                onclick={onDismissRuntime}
            >
                <X aria-hidden="true" />
            </button>
        </div>
    {/if}
    {#if interactionError !== null}
        <div class="chat-error-notice" role="alert">
            <p>{interactionError}</p>
            <button
                class="chat-error-dismiss"
                type="button"
                aria-label="대화 상호작용 오류 닫기"
                title="닫기"
                onclick={onDismissInteraction}
            >
                <X aria-hidden="true" />
            </button>
        </div>
    {/if}
</div>

<style>
    .chat-error-region {
        display: grid;
        position: absolute;
        z-index: 24;
        top: calc(
            env(safe-area-inset-top) + var(--mobile-top-offset) + var(--mobile-top-action) + 10px
        );
        right: 0;
        left: 0;
        justify-items: center;
        padding: 0 var(--chat-side-inset);
        gap: 8px;
        pointer-events: none;
    }

    .chat-error-notice {
        display: grid;
        width: min(100%, 680px);
        min-height: 38px;
        grid-template-columns: minmax(0, 1fr) auto;
        align-items: center;
        padding: 7px 7px 7px 12px;
        border: 1px solid var(--status-error-border);
        border-radius: 11px;
        background: var(--status-error-bg);
        box-shadow: var(--popover-shadow);
        color: var(--status-error-fg);
        font-size: 0.75rem;
        line-height: 1.45;
        animation: chat-error-notice-enter 180ms var(--panel-open-easing) both;
        gap: 10px;
        pointer-events: auto;
    }

    .chat-error-notice p {
        overflow-wrap: anywhere;
        margin: 0;
    }

    .chat-error-dismiss {
        display: grid;
        width: 28px;
        height: 28px;
        min-width: 28px;
        min-height: 28px;
        padding: 0;
        border: 0;
        border-radius: var(--radius-sm);
        background: transparent;
        color: currentcolor;
        place-items: center;
    }

    .chat-error-dismiss :global(svg) {
        width: 15px;
        height: 15px;
        stroke-width: 1.9;
    }

    :global(.app-shell[data-layout='desktop']) .chat-error-region {
        top: 72px;
        padding-inline: var(--chat-side-inset);
        transition:
            right var(--panel-close-duration) var(--panel-close-easing),
            padding-inline var(--panel-close-duration) var(--panel-close-easing);
    }

    :global(.app-shell[data-layout='desktop'] .chat-pane.utility-open) > .chat-error-region {
        right: var(--chat-utility-reserved-width);
        padding-inline: var(--chat-utility-side-inset);
        transition:
            right var(--panel-open-duration) var(--panel-open-easing),
            padding-inline var(--panel-open-duration) var(--panel-open-easing);
    }

    @keyframes chat-error-notice-enter {
        from {
            opacity: 0;
            transform: translate3d(0, -6px, 0) scale(0.99);
        }

        to {
            opacity: 1;
            transform: translate3d(0, 0, 0) scale(1);
        }
    }
</style>
