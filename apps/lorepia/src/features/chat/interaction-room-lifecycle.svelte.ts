import {
    INITIAL_INTERACTION_ROOM_STATE,
    InteractionRoomController,
    type InteractionRoomCapableClient,
    type InteractionRoomState,
} from './interaction-room-controller';

export class InteractionRoomLifecycle {
    controller = $state<InteractionRoomController | null>(null);
    state = $state<InteractionRoomState>(structuredClone(INITIAL_INTERACTION_ROOM_STATE));

    #roomKey = '';

    syncRoom(conversationId: string | null, branchId: string | null): void {
        const controller = this.controller;
        const nextKey = conversationId && branchId ? `${conversationId}:${branchId}` : '';
        if (controller === null || nextKey === this.#roomKey) return;
        this.#roomKey = nextKey;
        void controller.loadRoom(conversationId, branchId);
    }

    mount(client: InteractionRoomCapableClient | undefined): (() => void) | undefined {
        if (client === undefined) return;
        const controller = new InteractionRoomController(client);
        this.controller = controller;
        const unsubscribe = controller.state.subscribe((value) => {
            this.state = value;
        });
        return () => {
            unsubscribe();
            controller.destroy();
            this.controller = null;
        };
    }
}
