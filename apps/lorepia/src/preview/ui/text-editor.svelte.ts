import { getContext, setContext, tick } from 'svelte';
import type { EditorOrigin } from './editor-morph';

export interface TextEditRequest {
    label: string;
    value: string;
    placeholder?: string;
    maxlength?: number;
    message?: boolean;
    origin?: () => EditorOrigin;
    onchange: (value: string) => void;
    onsend?: () => void;
}

const key = Symbol('ui-preview-text-editor');

export class TextEditorState {
    request = $state<TextEditRequest | null>(null);
    present = $state(false);
    #opener: HTMLElement | null = null;

    open(request: TextEditRequest, opener?: HTMLElement) {
        this.#opener = opener ?? (document.activeElement as HTMLElement | null);
        this.request = request;
        this.present = true;
    }

    close() {
        this.request = null;
    }

    finish() {
        if (this.request) return;
        this.present = false;
        void tick().then(() => this.restoreFocus());
    }

    restoreFocus() {
        if (this.present) return;
        const opener = this.#opener;
        if (opener?.isConnected) opener.focus({ preventScroll: true });
    }
}

export function provideTextEditor() {
    return setContext(key, new TextEditorState());
}

export function useTextEditor(): TextEditorState {
    return getContext(key);
}
