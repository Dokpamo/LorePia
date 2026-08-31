<script lang="ts">
    import { Check, Copy, GitBranch, Pencil, RefreshCw, Trash2, X } from '@lucide/svelte';

    import type { MessageDto } from '../../lib/ipc/contracts';
    import { tr } from '../../lib/i18n';
    import type { MessageActionsState } from './message-actions.svelte';

    interface Props {
        message: MessageDto;
        state: MessageActionsState;
        generationActive: boolean;
        onCopy: (message: MessageDto) => void | Promise<void>;
        onCreateBranch: (messageId: string) => void | Promise<void>;
        onRegenerate: (messageId: string) => void | Promise<void>;
        onRemove: (messageId: string) => void | Promise<void>;
    }

    let {
        message,
        state,
        generationActive,
        onCopy,
        onCreateBranch,
        onRegenerate,
        onRemove,
    }: Props = $props();
</script>

<div class="message-actions" aria-label="메시지 작업">
    <!-- prettier-ignore -->
    <button
        type="button"
        aria-label="복사"
        title="복사"
        onclick={() => void onCopy(message)}
    >
        <Copy aria-hidden="true" />
    </button>
    <button
        type="button"
        aria-label="여기서 분기"
        title="여기서 분기"
        disabled={generationActive}
        onclick={() => void onCreateBranch(message.id)}
    >
        <GitBranch aria-hidden="true" />
    </button>
    {#if message.role === 'user'}
        <button
            type="button"
            aria-label="편집"
            title="편집"
            disabled={generationActive}
            onclick={() => state.beginEdit(message)}
        >
            <Pencil aria-hidden="true" />
        </button>
    {:else if message.role === 'assistant'}
        <button
            type="button"
            aria-label="재생성"
            title="재생성"
            disabled={generationActive}
            onclick={() => void onRegenerate(message.id)}
        >
            <RefreshCw aria-hidden="true" />
        </button>
    {/if}
    {#if state.pendingRemoveId === message.id}
        <button
            class="danger"
            type="button"
            aria-label={$tr('chat.message.remove_confirm')}
            title={$tr('chat.message.remove_confirm')}
            onclick={() => {
                state.confirmRemove();
                void onRemove(message.id);
            }}
        >
            <Check aria-hidden="true" />
        </button>
        <!-- prettier-ignore -->
        <button
            type="button"
            aria-label="취소"
            title="취소"
            onclick={() => state.cancelRemove()}
        >
            <X aria-hidden="true" />
        </button>
    {:else}
        <button
            type="button"
            aria-label={$tr('chat.message.remove_from_here')}
            title={$tr('chat.message.remove_from_here')}
            disabled={generationActive}
            onclick={() => state.requestRemove(message.id)}
        >
            <Trash2 aria-hidden="true" />
        </button>
    {/if}
</div>
