import { writable, get } from 'svelte/store';
import type { ConversationDto, LorepiaClient } from '../../lib/ipc/contracts';
import type { LorepiaAppController, LorepiaAppState } from '../app-controller';
import { t } from '../../lib/i18n';

interface ConversationCatalog {
    items: ConversationDto[];
    loading: boolean;
    error: string | null;
}

/** Global list and cross-character navigation share one cancellation authority. */
export class WorkspaceNavigationController {
    readonly state = writable<ConversationCatalog>({ items: [], loading: false, error: null });
    private loadEpoch = 0;
    private navigationEpoch = 0;
    private destroyed = false;
    constructor(
        private client: LorepiaClient,
        private app: LorepiaAppController,
        private current: () => LorepiaAppState,
    ) {}
    async load() {
        const epoch = ++this.loadEpoch;
        this.state.update((state) => ({ ...state, loading: true, error: null }));
        try {
            const items = await this.client.listConversations(null);
            if (!this.destroyed && epoch === this.loadEpoch)
                this.state.set({ items, loading: false, error: null });
        } catch {
            if (!this.destroyed && epoch === this.loadEpoch)
                this.state.update((state) => ({
                    ...state,
                    loading: false,
                    error: t('navigation.loadFailed'),
                }));
        }
    }
    cancelNavigation() {
        this.navigationEpoch += 1;
    }
    async selectCharacter(id: string): Promise<boolean> {
        const epoch = ++this.navigationEpoch;
        const state = this.current();
        const character = state.library.characters.find((item) => item.id === id);
        if (!character) return false;
        if (state.selected_character?.id !== id) await this.app.selectCharacter(character, false);
        return (
            !this.destroyed &&
            epoch === this.navigationEpoch &&
            this.current().selected_character?.id === id
        );
    }
    async selectConversation(id: string): Promise<boolean> {
        const item =
            get(this.state).items.find((value) => value.id === id) ??
            this.current().conversations.items.find((value) => value.id === id);
        if (!item || !(await this.selectCharacter(item.character_id))) return false;
        const epoch = this.navigationEpoch;
        await this.app.selectConversation(item);
        return (
            !this.destroyed &&
            epoch === this.navigationEpoch &&
            this.current().selected_conversation?.id === id
        );
    }
    destroy() {
        this.destroyed = true;
        this.loadEpoch += 1;
        this.cancelNavigation();
    }
}
