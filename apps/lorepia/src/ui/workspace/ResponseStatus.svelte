<script lang="ts">
    import { tr } from '../../lib/i18n';
    import type { SampleMessage } from './view-types';
    let { message, busy, onretry }: { message: SampleMessage; busy: boolean; onretry: () => void } =
        $props();
    let delayed = $state(false);
    const progress = $derived(message.status === 'pending' ? message.text : null);
    $effect(() => {
        const text = progress;
        delayed = false;
        // New streamed text restarts the wait; an active reply is not a stalled reply.
        if (text === null) return;
        const timer = setTimeout(() => {
            delayed = true;
        }, 10000);
        return () => clearTimeout(timer);
    });
</script>

{#if message.status === 'pending'}
    <p class="ui-response-wait" role="status">
        {$tr(
            delayed
                ? 'uiPreview.replyDelayed'
                : message.text
                  ? 'uiPreview.replyWriting'
                  : 'uiPreview.replyWaiting',
        )}
    </p>
{:else if message.status === 'failed' || message.status === 'cancelled'}
    <div class="ui-response-state" role="status">
        <div>
            <span class:ui-critical={message.status === 'failed'}
                >{$tr(
                    message.status === 'failed'
                        ? 'uiPreview.replyFailed'
                        : 'uiPreview.replyCancelled',
                )}</span
            >
            {#if message.text}<small>{$tr('uiPreview.partialReplyKept')}</small>{/if}
        </div>
        <button class="ui-result-action ui-pressable" disabled={busy} onclick={onretry}
            ><span class="ui-press-visual">{$tr('uiPreview.retryReply')}</span></button
        >
    </div>
{/if}
