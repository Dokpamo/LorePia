import {
    SUPPORTED_CHAT_EVENT_VERSION,
    type ChatEventDto,
    type ChatStreamItemDto,
} from '../../lib/ipc/contracts';

export type ChatStreamDecision =
    | { type: 'apply'; event: ChatEventDto }
    | {
          type: 'live_snapshot';
          generationId: string;
          displayPrefix: string;
          reasoningPrefix: string;
          sequenceBaseline: number;
      }
    | { type: 'ignore'; reason: 'wrong_route' | 'wrong_generation' }
    | {
          type: 'reconcile';
          reason:
              | 'broadcast_lagged'
              | 'sequence_gap'
              | 'unsupported_event_version'
              | 'route_mismatch'
              | 'duplicate_or_decreasing_sequence'
              | 'event_after_terminal'
              | 'generation_mismatch'
              | 'invalid_live_snapshot'
              | 'terminal'
              | 'stream_closed';
          event: ChatEventDto | null;
          sequenceBaseline: number;
      };

export interface ChatStreamExpectation {
    conversationId: string;
    branchId: string;
    generationId?: string;
    assistantMessageId?: string;
    sequenceBaseline?: number;
    eventVersion?: number;
    requireLiveSnapshot?: boolean;
}

export function isTerminalChatEvent(event: ChatEventDto): boolean {
    return (
        event.kind.type === 'generation_finished' ||
        event.kind.type === 'generation_cancelled' ||
        event.kind.type === 'generation_failed'
    );
}

/**
 * Enforces route, wire-version and monotonic sequence invariants before a
 * renderer may apply streamed text. Source sequences may skip events filtered
 * by Core, so only duplicate or decreasing sequences require reconciliation.
 */
export class ChatStreamVerifier {
    private readonly conversationId: string;
    private readonly branchId: string;
    private readonly eventVersion: number;
    private generationId: string | null;
    private assistantMessageId: string | null;
    private lastSequence: number;
    private terminalSeen = false;
    private readonly requireLiveSnapshot: boolean;
    private liveSnapshotAccepted = false;

    constructor(expectation: ChatStreamExpectation) {
        this.conversationId = expectation.conversationId;
        this.branchId = expectation.branchId;
        this.generationId = expectation.generationId ?? null;
        this.assistantMessageId = expectation.assistantMessageId ?? null;
        this.lastSequence = expectation.sequenceBaseline ?? 0;
        this.eventVersion = expectation.eventVersion ?? SUPPORTED_CHAT_EVENT_VERSION;
        this.requireLiveSnapshot = expectation.requireLiveSnapshot ?? false;
    }

    accept(item: ChatStreamItemDto): ChatStreamDecision {
        if (this.requireLiveSnapshot && !this.liveSnapshotAccepted) {
            return this.acceptRequiredLiveSnapshot(item);
        }
        if (item.type === 'closed') {
            return {
                type: 'reconcile',
                reason: 'stream_closed',
                event: null,
                sequenceBaseline: this.lastSequence,
            };
        }
        if (item.type === 'reconciliation_required') {
            const marker = item.payload;
            if (marker.reason === 'live_snapshot') {
                return this.reconcile('invalid_live_snapshot');
            }
            if (
                marker.conversation_id !== this.conversationId ||
                marker.branch_id !== this.branchId
            ) {
                return { type: 'ignore', reason: 'wrong_route' };
            }
            if (this.generationId !== null && marker.generation_id !== this.generationId) {
                return { type: 'ignore', reason: 'wrong_generation' };
            }
            return {
                type: 'reconcile',
                reason: marker.reason,
                event: null,
                sequenceBaseline:
                    marker.reason === 'duplicate_or_decreasing_sequence'
                        ? this.lastSequence
                        : (marker.observed_sequence ?? marker.last_sequence ?? this.lastSequence),
            };
        }

        const event = item.payload;
        if (event.conversation_id !== this.conversationId) {
            return { type: 'ignore', reason: 'wrong_route' };
        }
        if (event.branch_id !== this.branchId) {
            return {
                type: 'reconcile',
                reason: 'route_mismatch',
                event,
                sequenceBaseline: this.lastSequence,
            };
        }
        if (event.event_version !== this.eventVersion) {
            return {
                type: 'reconcile',
                reason: 'unsupported_event_version',
                event,
                sequenceBaseline: this.lastSequence,
            };
        }

        if (this.generationId === null) {
            this.generationId = event.generation_id;
        } else if (event.generation_id !== this.generationId) {
            return { type: 'ignore', reason: 'wrong_generation' };
        }

        if (event.assistant_message_id === null) {
            return {
                type: 'reconcile',
                reason: 'route_mismatch',
                event,
                sequenceBaseline: this.lastSequence,
            };
        }
        if (this.assistantMessageId === null) {
            this.assistantMessageId = event.assistant_message_id;
        } else if (event.assistant_message_id !== this.assistantMessageId) {
            return {
                type: 'reconcile',
                reason: 'route_mismatch',
                event,
                sequenceBaseline: this.lastSequence,
            };
        }

        if (this.terminalSeen) {
            return {
                type: 'reconcile',
                reason: 'event_after_terminal',
                event,
                sequenceBaseline: this.lastSequence,
            };
        }
        if (event.sequence <= this.lastSequence) {
            return {
                type: 'reconcile',
                reason: 'duplicate_or_decreasing_sequence',
                event,
                sequenceBaseline: this.lastSequence,
            };
        }
        this.lastSequence = event.sequence;
        if (isTerminalChatEvent(event)) {
            this.terminalSeen = true;
            return {
                type: 'reconcile',
                reason: 'terminal',
                event,
                sequenceBaseline: this.lastSequence,
            };
        }
        return { type: 'apply', event };
    }

    private acceptRequiredLiveSnapshot(item: ChatStreamItemDto): ChatStreamDecision {
        if (item.type !== 'reconciliation_required' || item.payload.reason !== 'live_snapshot') {
            return this.reconcile('invalid_live_snapshot');
        }
        const marker = item.payload;
        if (marker.conversation_id !== this.conversationId || marker.branch_id !== this.branchId) {
            return this.reconcile('route_mismatch');
        }
        if (this.generationId === null || marker.generation_id !== this.generationId) {
            return this.reconcile('generation_mismatch');
        }
        if (marker.supported_event_version !== this.eventVersion) {
            return this.reconcile('unsupported_event_version');
        }
        if (
            marker.last_sequence !== this.lastSequence ||
            marker.observed_sequence === null ||
            marker.observed_sequence < marker.last_sequence ||
            marker.dropped_events !== null ||
            marker.display_prefix === null ||
            marker.reasoning_prefix === null
        ) {
            return this.reconcile('invalid_live_snapshot');
        }
        this.lastSequence = marker.observed_sequence;
        this.liveSnapshotAccepted = true;
        return {
            type: 'live_snapshot',
            generationId: marker.generation_id,
            displayPrefix: marker.display_prefix,
            reasoningPrefix: marker.reasoning_prefix,
            sequenceBaseline: marker.observed_sequence,
        };
    }

    private reconcile(
        reason: Extract<ChatStreamDecision, { type: 'reconcile' }>['reason'],
    ): ChatStreamDecision {
        return {
            type: 'reconcile',
            reason,
            event: null,
            sequenceBaseline: this.lastSequence,
        };
    }

    bindGeneration(generationId: string): boolean {
        if (this.generationId === null) {
            this.generationId = generationId;
            return true;
        }
        return this.generationId === generationId;
    }

    getGenerationId(): string | null {
        return this.generationId;
    }

    getLastSequence(): number {
        return this.lastSequence;
    }

    resetAfterReconciliation(generationId: string, lastSequence = this.lastSequence): void {
        this.generationId = generationId;
        this.lastSequence = lastSequence;
        this.terminalSeen = false;
    }
}
