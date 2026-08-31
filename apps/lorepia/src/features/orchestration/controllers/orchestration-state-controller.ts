import { get, writable, type Readable } from 'svelte/store';

import { INITIAL_ORCHESTRATION_STATE, type OrchestrationState } from './orchestration-state';

export class OrchestrationStateController {
    private readonly mutable = writable<OrchestrationState>(
        structuredClone(INITIAL_ORCHESTRATION_STATE),
    );
    readonly state: Readable<OrchestrationState> = this.mutable;

    private contextEpoch = 0;
    private roomDraftEpoch = 0;
    private planPreviewEpoch = 0;

    snapshot(): OrchestrationState {
        return get(this.mutable);
    }

    set(state: OrchestrationState): void {
        this.mutable.set(state);
    }

    update(updater: (state: OrchestrationState) => OrchestrationState): void {
        this.mutable.update(updater);
    }

    updateForContext(
        contextKey: string,
        updater: (state: OrchestrationState) => OrchestrationState,
    ): boolean {
        let applied = false;
        this.mutable.update((state) => {
            if (state.context_key !== contextKey) return state;
            applied = true;
            return updater(state);
        });
        return applied;
    }

    isCurrentContext(contextKey: string): boolean {
        return this.snapshot().context_key === contextKey;
    }

    beginContextLoad(): number {
        const epoch = ++this.contextEpoch;
        ++this.roomDraftEpoch;
        ++this.planPreviewEpoch;
        return epoch;
    }

    isContextEpoch(epoch: number): boolean {
        return epoch === this.contextEpoch;
    }

    currentContextEpoch(): number {
        return this.contextEpoch;
    }

    bumpRoomDraftEpoch(): void {
        ++this.roomDraftEpoch;
    }

    currentRoomDraftEpoch(): number {
        return this.roomDraftEpoch;
    }

    isRoomDraftEpoch(epoch: number): boolean {
        return epoch === this.roomDraftEpoch;
    }

    invalidatePlanPreviewForContext(contextKey: string): boolean {
        if (!this.isCurrentContext(contextKey)) return false;
        ++this.planPreviewEpoch;
        return true;
    }

    beginPlanPreviewRequest(): number {
        return ++this.planPreviewEpoch;
    }

    isPlanPreviewEpoch(epoch: number): boolean {
        return epoch === this.planPreviewEpoch;
    }

    invalidatePlanPreview(): void {
        ++this.planPreviewEpoch;
    }

    destroy(): void {
        ++this.contextEpoch;
    }
}
