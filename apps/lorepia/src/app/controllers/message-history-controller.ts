import { t } from '../../lib/i18n';
import { get, writable } from 'svelte/store';
import type { MessageDto } from '../../lib/ipc/contracts';
import type { LorepiaAppState } from '../app-state';
import {
    INITIAL_HISTORY_MESSAGES,
    type HistoryPageClient,
    type MessageHistoryPage,
} from './recent-branch-messages';

export const MAX_HISTORY_MESSAGES = INITIAL_HISTORY_MESSAGES * 3;
export interface MessageHistoryState {
    scope: string;
    items: MessageDto[];
    start_index: number;
    total_messages: number;
    head_message_id: string | null;
    has_older: boolean;
    has_newer: boolean;
    loading: boolean;
    error: string | null;
    failed_direction?: 'older' | 'newer' | 'latest';
    paged: boolean;
}
const empty = (): MessageHistoryState => ({
    scope: '',
    items: [],
    start_index: 0,
    total_messages: 0,
    head_message_id: null,
    has_older: false,
    has_newer: false,
    loading: false,
    error: null,
    paged: false,
});

/** Bounded presentation-only history; runtime context and durable head authority stay in app state. */
export class MessageHistoryController {
    readonly state = writable<MessageHistoryState>(empty());
    private branchId = '';
    private epoch = 0;
    private source: MessageDto[] | null = null;
    private inFlight: Promise<void> | null = null;
    private queuedLatest: Promise<void> | null = null;
    private inFlightDirection: 'older' | 'newer' | 'latest' | null = null;
    constructor(private readonly client: HistoryPageClient) {}

    sync(app: LorepiaAppState): void {
        const branchId = app.conversation_state?.active_branch_id ?? '';
        const scope = `${app.selected_conversation?.id ?? ''}:${branchId}`;
        const current = get(this.state);
        if (scope !== current.scope) {
            this.epoch += 1;
            this.branchId = branchId;
            this.source = null;
            this.state.set({ ...empty(), scope });
        }
        if (this.source === app.messages.items) return;
        this.source = app.messages.items;
        const paged =
            app.messages.start_index !== undefined && app.messages.total_messages !== undefined;
        const total = app.messages.total_messages ?? app.messages.items.length;
        const active = get(this.state);
        // Preserve an older viewport while new messages arrive at the authoritative head.
        if (paged && active.paged && active.has_newer && total >= active.total_messages) {
            const head = app.messages.head_message_id ?? null;
            const changedHead = head !== active.head_message_id;
            if (changedHead) this.epoch += 1;
            this.state.set({
                ...active,
                total_messages: total,
                head_message_id: head,
                loading: changedHead ? false : active.loading,
            });
            return;
        }
        this.epoch += 1;
        const keep = paged
            ? Math.max(
                  INITIAL_HISTORY_MESSAGES,
                  Math.min(MAX_HISTORY_MESSAGES, active.items.length),
              )
            : app.messages.items.length;
        const items = paged ? app.messages.items.slice(-keep) : app.messages.items;
        const start = (app.messages.start_index ?? 0) + app.messages.items.length - items.length;
        this.state.set({
            ...empty(),
            scope,
            items,
            start_index: start,
            total_messages: total,
            head_message_id: app.messages.head_message_id ?? null,
            has_older: start > 0,
            has_newer: start + items.length < total,
            paged,
        });
    }

    load(direction: 'older' | 'newer' | 'latest'): Promise<void> {
        if (this.inFlight !== null) {
            if (direction !== 'latest' || this.inFlightDirection === 'latest') return this.inFlight;
            if (this.queuedLatest !== null) return this.queuedLatest;
            const scope = get(this.state).scope;
            const queued = this.inFlight.then(async () => {
                if (this.queuedLatest === queued) this.queuedLatest = null;
                if (get(this.state).scope === scope) await this.load('latest');
            });
            this.queuedLatest = queued;
            void queued.finally(() => {
                if (this.queuedLatest === queued) this.queuedLatest = null;
            });
            return queued;
        }
        const operation = this.loadPage(direction);
        this.inFlight = operation;
        this.inFlightDirection = direction;
        void operation.finally(() => {
            if (this.inFlight === operation) {
                this.inFlight = null;
                this.inFlightDirection = null;
            }
        });
        return operation;
    }

    private async loadPage(direction: 'older' | 'newer' | 'latest'): Promise<void> {
        const current = get(this.state);
        const call =
            'listBranchMessagesPage' in this.client
                ? this.client.listBranchMessagesPage?.bind(this.client)
                : undefined;
        if (!call || !current.paged || (current.loading && direction !== 'latest')) return;
        if (direction === 'older' && !current.has_older) return;
        if (direction === 'newer' && !current.has_newer) return;
        const anchor = direction === 'older' ? current.items[0]?.id : current.items.at(-1)?.id;
        if (direction !== 'latest' && !anchor) return;
        const epoch = ++this.epoch;
        this.state.set({ ...current, loading: true, error: null });
        try {
            const page = await call({
                branch_id: this.branchId,
                limit: INITIAL_HISTORY_MESSAGES,
                ...(direction === 'older' ? { before_message_id: anchor } : {}),
                ...(direction === 'newer' ? { after_message_id: anchor } : {}),
            });
            if (epoch !== this.epoch) return;
            if (direction !== 'latest' && page.head_message_id !== current.head_message_id) {
                await this.loadPage('latest');
                return;
            }
            this.publishPage(current, page, direction);
        } catch (error: unknown) {
            if (epoch === this.epoch)
                this.state.set({
                    ...get(this.state),
                    loading: false,
                    error: error instanceof Error ? error.message : t('ux.loading.failed'),
                    failed_direction: direction,
                });
        }
    }

    private publishPage(
        current: MessageHistoryState,
        page: MessageHistoryPage,
        direction: 'older' | 'newer' | 'latest',
    ): void {
        const existingIds = new Set(current.items.map((item) => item.id));
        const incoming = page.messages.filter((item) => !existingIds.has(item.id));
        let items =
            direction === 'latest'
                ? page.messages
                : direction === 'older'
                  ? [...incoming, ...current.items]
                  : [...current.items, ...incoming];
        let start = direction === 'newer' ? current.start_index : page.start_index;
        if (items.length > MAX_HISTORY_MESSAGES) {
            if (direction === 'older') items = items.slice(0, MAX_HISTORY_MESSAGES);
            else {
                start += items.length - MAX_HISTORY_MESSAGES;
                items = items.slice(-MAX_HISTORY_MESSAGES);
            }
        }
        this.state.set({
            ...current,
            items,
            start_index: start,
            total_messages: page.total_messages,
            head_message_id: page.head_message_id,
            has_older: start > 0,
            has_newer: start + items.length < page.total_messages,
            loading: false,
            error: null,
        });
    }

    dispose(): void {
        this.epoch += 1;
        this.source = null;
        this.state.set(empty());
    }
}
