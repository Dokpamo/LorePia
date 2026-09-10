import { getContext, setContext, tick } from 'svelte';
import type { ChoiceRequest } from './settings-choice';

const key = Symbol('workspace-choice-sheet');

export class ChoiceSheetState {
    request = $state<ChoiceRequest | null>(null);
    present = $state(false);
    #opener: HTMLElement | null = null;
    #afterClose: (() => void) | undefined;

    open(request: ChoiceRequest, opener: HTMLElement) {
        this.#opener = opener;
        this.#afterClose = undefined;
        this.request = request;
        this.present = true;
    }

    close(afterClose?: () => void) {
        this.#afterClose = afterClose;
        this.request = null;
    }

    finish() {
        if (this.request) return;
        this.present = false;
        const afterClose = this.#afterClose;
        this.#afterClose = undefined;
        void tick().then(() => {
            if (afterClose) {
                if (!this.present) afterClose();
                return;
            }
            // Covered pages resume their own focus effects during the same flush.
            // Restore the specific selector after those effects, not before them.
            requestAnimationFrame(() => {
                if (!this.present && this.#opener?.isConnected && !this.#opener.closest('[inert]'))
                    this.#opener.focus({ preventScroll: true });
            });
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
