import { getContext, setContext, tick } from 'svelte';
import type { ChoiceRequest } from './settings-choice';

const key = Symbol('workspace-choice-sheet');

export class ChoiceSheetState {
    request = $state<ChoiceRequest | null>(null);
    present = $state(false);
    #opener: HTMLElement | null = null;

    open(request: ChoiceRequest, opener: HTMLElement) {
        this.#opener = opener;
        this.request = request;
        this.present = true;
    }

    close() {
        this.request = null;
    }

    finish() {
        if (this.request) return;
        this.present = false;
        void tick().then(() => {
            if (this.#opener?.isConnected && !this.#opener.closest('[inert]'))
                this.#opener.focus({ preventScroll: true });
        });
    }
}

export function provideChoiceSheet() {
    return setContext(key, new ChoiceSheetState());
}

export function useChoiceSheet(): ChoiceSheetState {
    return getContext(key);
}

/** Existing standalone feature panels may render outside WorkspaceFrame. */
export function useOptionalChoiceSheet(): ChoiceSheetState | undefined {
    return getContext(key);
}
